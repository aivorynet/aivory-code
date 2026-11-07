//! The indexer: walk a repo, parse changed files in parallel, write segments, merge segments.
//!
//! - [`Index::refresh`] walks a checkout (the walk respects `.gitignore`), compares every
//!   file with the snapshot, parses only the files that differ, and writes them as new
//!   segments. Files that are gone get tombstones. A cold index is a refresh of an empty
//!   snapshot.
//! - [`Index::update_files`] does the same for a list of changed paths, without a walk.
//! - [`Index::merge`] joins small segments of a snapshot. [`Index::spawn_merge`] runs it in a
//!   background thread.
//!
//! The content id of a file is its git blob id when git has the file in its index and the file
//! is unchanged (`git ls-files -s`, minus `git diff-files`). Other files get the blake3 hash
//! of their content. The stub store holds one stub per content id.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aivory_syntax::Language;
use rayon::prelude::*;

use crate::index::Snapshot;
use crate::segment::{IndexedFile, Segment, build_segment};
use crate::store::{StubBlob, unpack_stub};
use crate::{ContentId, Error, Index, Result, StubStore};

/// Options of the indexer.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// A larger file is not indexed.
    pub max_file_bytes: u64,
    /// The number of files in one new segment.
    pub batch_files: usize,
    /// Use git blob ids for committed files.
    pub use_git: bool,
    /// Folders that the walk skips (for example the folder of the index itself).
    pub skip_dirs: Vec<PathBuf>,
    /// A refresh merges the segments of the snapshot when there are more than this many.
    pub max_segments: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 2 * 1024 * 1024,
            batch_files: 2048,
            use_git: true,
            skip_dirs: Vec::new(),
            max_segments: 16,
        }
    }
}

/// What a refresh did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshReport {
    /// Files that the walk found and the indexer can read.
    pub files: usize,
    /// Files that the refresh parsed.
    pub parsed: usize,
    /// Changed files whose stub came from the store (parsed before, in another snapshot or repo).
    pub from_store: usize,
    /// Files equal to the snapshot.
    pub unchanged: usize,
    /// Files of the snapshot that are gone (they got tombstones).
    pub removed: usize,
    /// Files that the indexer left out: too large, not UTF-8, or not readable.
    pub skipped: usize,
    /// Files whose content id is a git blob id.
    pub git_ids: usize,
    /// New segment files.
    pub segments_written: usize,
    pub elapsed: Duration,
}

/// What a merge did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub segments_before: usize,
    pub segments_after: usize,
}

/// When and how much to merge.
#[derive(Debug, Clone)]
pub struct MergePolicy {
    /// Merge until the snapshot has at most this many segments.
    pub max_segments: usize,
    /// Never build a segment with more files than this.
    pub max_merged_files: usize,
}

impl Default for MergePolicy {
    fn default() -> Self {
        Self {
            max_segments: 8,
            max_merged_files: 100_000,
        }
    }
}

#[derive(Debug)]
struct ScanFile {
    /// Relative to the root, with `/` separators.
    path: String,
    abs: PathBuf,
    language: Language,
    id: Option<ContentId>,
}

enum Outcome {
    Unchanged,
    Skipped,
    Indexed(Box<IndexedFile>, bool),
}

/// (content id, source hash) of the live files of a snapshot.
type Known = HashMap<String, (ContentId, String)>;

fn known_files(snapshot: &Snapshot) -> Known {
    let mut known = Known::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for seg in snapshot.segments.iter().rev() {
        for e in seg.files() {
            if seen.insert(e.path.as_str()) {
                known.insert(e.path.clone(), (e.content.clone(), e.hash.clone()));
            }
        }
        for t in seg.tombstones() {
            seen.insert(t.as_str());
        }
    }
    known
}

fn run_git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// The git blob ids of the tracked files below `root` whose worktree copy equals the index.
fn git_ids(root: &Path) -> Option<HashMap<String, String>> {
    let listing = run_git(root, &["ls-files", "-s", "-z"])?;
    let mut ids = HashMap::new();
    for line in listing.split(|b| *b == 0) {
        let line = std::str::from_utf8(line).ok()?;
        // "<mode> <oid> <stage>\t<path>"
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split(' ');
        let (mode, oid, stage) = (parts.next()?, parts.next()?, parts.next()?);
        if stage == "0" && mode != "160000" {
            ids.insert(path.to_owned(), oid.to_owned());
        }
    }
    if let Some(dirty) = run_git(root, &["diff-files", "--name-only", "-z", "--relative"]) {
        for path in dirty
            .split(|b| *b == 0)
            .filter_map(|p| std::str::from_utf8(p).ok())
        {
            ids.remove(path);
        }
    }
    Some(ids)
}

fn scan_repo(root: &Path, opts: &IndexOptions) -> Vec<ScanFile> {
    let ids = if opts.use_git { git_ids(root) } else { None };
    let skip = opts.skip_dirs.clone();
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(false)
        .require_git(false)
        .git_global(false)
        .follow_links(false)
        .filter_entry(move |e| {
            e.file_name() != ".git" && !skip.iter().any(|s| e.path() == s.as_path())
        });
    let mut out = Vec::new();
    for entry in walker.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let Some(language) = Language::detect(rel) else {
            continue;
        };
        let path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let id = ids
            .as_ref()
            .and_then(|m| m.get(&path))
            .map(|oid| ContentId::Git(oid.clone()));
        out.push(ScanFile {
            path,
            abs: entry.into_path(),
            language,
            id,
        });
    }
    out
}

fn process(store: &StubStore, file: &ScanFile, known: &Known, max_bytes: u64) -> Outcome {
    let known_entry = known.get(&file.path);
    if let (Some(id), Some((kid, _))) = (&file.id, known_entry)
        && id == kid
    {
        return Outcome::Unchanged;
    }
    // A stub in the store spares the read of the file.
    if let Some(id) = &file.id
        && let Some(blob) = store.get(id, file.language)
    {
        return Outcome::Indexed(
            Box::new(IndexedFile {
                path: file.path.clone(),
                content: id.clone(),
                blob,
            }),
            false,
        );
    }
    let Ok(bytes) = std::fs::read(&file.abs) else {
        return Outcome::Skipped;
    };
    if bytes.len() as u64 > max_bytes {
        return Outcome::Skipped;
    }
    let b3 = ContentId::of_bytes(&bytes);
    if let Some((kid, khash)) = known_entry
        && (&b3 == kid || khash == b3.hex())
    {
        return Outcome::Unchanged;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Outcome::Skipped;
    };
    let id = file.id.clone().unwrap_or(b3);
    match store.get_or_parse(&id, file.language, || Ok(text)) {
        Ok((blob, parsed)) => Outcome::Indexed(
            Box::new(IndexedFile {
                path: file.path.clone(),
                content: id,
                blob,
            }),
            parsed,
        ),
        Err(_) => Outcome::Skipped,
    }
}

/// Read and index one file below `root`. Returns `None` when the file has no known language,
/// is not readable, is larger than `max_bytes`, or is not UTF-8. The boolean is true when the
/// call parsed (false: the stub came from the store).
pub(crate) fn index_path(
    store: &StubStore,
    root: &Path,
    rel: &str,
    max_bytes: u64,
) -> Result<Option<(IndexedFile, bool)>> {
    let Some(language) = Language::detect(rel) else {
        return Ok(None);
    };
    let scan = ScanFile {
        path: rel.to_owned(),
        abs: root.join(rel),
        language,
        id: None,
    };
    Ok(match process(store, &scan, &Known::new(), max_bytes) {
        Outcome::Indexed(file, parsed) => Some((*file, parsed)),
        _ => None,
    })
}

impl Index {
    /// Bring the snapshot `name` up to date with the checkout in `root`. The snapshot is
    /// created when it does not exist (use [`Index::fork_snapshot`] first to start from
    /// another snapshot).
    pub fn refresh(&self, root: &Path, name: &str, opts: &IndexOptions) -> Result<RefreshReport> {
        let started = Instant::now();
        let items = scan_repo(root, opts);
        let mut report = self.apply(root, name, &items, None, opts)?;
        report.elapsed = started.elapsed();
        Ok(report)
    }

    /// Like [`Index::refresh`], for a list of paths (relative to `root`, with `/`) that the
    /// caller knows to be changed, added, or removed. No walk happens.
    pub fn update_files(
        &self,
        root: &Path,
        name: &str,
        paths: &[String],
        opts: &IndexOptions,
    ) -> Result<RefreshReport> {
        let started = Instant::now();
        let items: Vec<ScanFile> = paths
            .iter()
            .filter(|p| root.join(p).is_file())
            .filter_map(|p| {
                Some(ScanFile {
                    path: p.clone(),
                    abs: root.join(p),
                    language: Language::detect(p)?,
                    id: None,
                })
            })
            .collect();
        let mut report = self.apply(root, name, &items, Some(paths), opts)?;
        report.elapsed = started.elapsed();
        Ok(report)
    }

    /// `all_paths`: `None` means a full walk (every known path may be gone); `Some` lists the
    /// paths that the call may remove.
    fn apply(
        &self,
        root: &Path,
        name: &str,
        items: &[ScanFile],
        paths: Option<&[String]>,
        opts: &IndexOptions,
    ) -> Result<RefreshReport> {
        let _guard = self.inner.modify.lock().unwrap_or_else(|e| e.into_inner());
        let base = self.snapshot(name);
        let known = base.as_deref().map(known_files).unwrap_or_default();
        let mut report = RefreshReport::default();
        let mut live: HashSet<&str> = HashSet::new();
        let mut written: Vec<Arc<Segment>> = Vec::new();
        for chunk in items.chunks(opts.batch_files.max(1)) {
            let outcomes: Vec<Outcome> = chunk
                .par_iter()
                .map(|f| process(&self.inner.store, f, &known, opts.max_file_bytes))
                .collect();
            let mut files: Vec<IndexedFile> = Vec::new();
            for (f, outcome) in chunk.iter().zip(outcomes) {
                match outcome {
                    Outcome::Unchanged => {
                        report.unchanged += 1;
                        live.insert(&f.path);
                    }
                    Outcome::Skipped => report.skipped += 1,
                    Outcome::Indexed(file, parsed) => {
                        if parsed {
                            report.parsed += 1;
                        } else {
                            report.from_store += 1;
                        }
                        live.insert(&f.path);
                        files.push(*file);
                    }
                }
                if matches!(f.id, Some(ContentId::Git(_))) {
                    report.git_ids += 1;
                }
            }
            if !files.is_empty() {
                let refs: Vec<&IndexedFile> = files.iter().collect();
                written.push(self.write_segment(build_segment(&refs, &[])?)?);
            }
        }
        // The stubs reach the disk before the snapshot that lists them.
        self.inner.store.flush()?;
        report.files = report.unchanged + report.parsed + report.from_store;
        let candidates: Vec<&String> = match paths {
            None => known.keys().collect(),
            Some(p) => p.iter().filter(|p| known.contains_key(*p)).collect(),
        };
        let removed: Vec<String> = candidates
            .into_iter()
            .filter(|p| !live.contains(p.as_str()))
            .cloned()
            .collect();
        report.removed = removed.len();
        let mut segments = base
            .as_ref()
            .map(|b| b.segments.clone())
            .unwrap_or_default();
        if !removed.is_empty() {
            segments.push(self.write_segment(build_segment(&[], &removed)?)?);
            report.segments_written += 1;
        }
        report.segments_written += written.len();
        segments.extend(written);
        let changed = report.segments_written > 0 || base.is_none();
        if changed {
            self.publish(Snapshot {
                name: name.to_owned(),
                root: Some(root.to_path_buf()),
                segments,
            })?;
            self.merge_locked(
                name,
                &MergePolicy {
                    max_segments: opts.max_segments,
                    ..MergePolicy::default()
                },
            )?;
        } else if let Some(b) = &base
            && b.root.as_deref() != Some(root)
        {
            self.publish(Snapshot {
                name: name.to_owned(),
                root: Some(root.to_path_buf()),
                segments: b.segments.clone(),
            })?;
        }
        Ok(report)
    }

    /// Merge small adjacent segments of a snapshot until it has at most `policy.max_segments`.
    pub fn merge(&self, name: &str, policy: &MergePolicy) -> Result<MergeReport> {
        let _guard = self.inner.modify.lock().unwrap_or_else(|e| e.into_inner());
        self.merge_locked(name, policy)
    }

    /// Run [`Index::merge`] in a background thread. Queries keep running while it works.
    pub fn spawn_merge(&self, name: &str, policy: MergePolicy) -> JoinHandle<Result<MergeReport>> {
        let (index, name) = (self.clone(), name.to_owned());
        std::thread::spawn(move || index.merge(&name, &policy))
    }

    fn merge_locked(&self, name: &str, policy: &MergePolicy) -> Result<MergeReport> {
        let snapshot = self
            .snapshot(name)
            .ok_or_else(|| Error::UnknownSnapshot(name.to_owned()))?;
        let mut segments = snapshot.segments.clone();
        let before = segments.len();
        while segments.len() > policy.max_segments.max(1) && segments.len() > 1 {
            let size = |s: &Arc<Segment>| s.files().len() + s.tombstones().len();
            let (at, total) = (0..segments.len() - 1)
                .map(|i| (i, size(&segments[i]) + size(&segments[i + 1])))
                .min_by_key(|(_, total)| *total)
                .expect("two segments at least");
            if total > policy.max_merged_files {
                break;
            }
            let merged = merge_pair(&segments[at], &segments[at + 1], at == 0)?;
            let merged = self.write_segment(merged)?;
            segments.splice(at..=at + 1, [merged]);
        }
        let after = segments.len();
        if after != before {
            self.publish(Snapshot {
                name: name.to_owned(),
                root: snapshot.root.clone(),
                segments,
            })?;
        }
        Ok(MergeReport {
            segments_before: before,
            segments_after: after,
        })
    }
}

/// The bytes of a segment with the live files of `older` and `newer`. `oldest` is true when
/// no segment is older than `older`; then the tombstones are not needed any more.
fn merge_pair(older: &Segment, newer: &Segment, oldest: bool) -> Result<Vec<u8>> {
    let mut files: Vec<IndexedFile> = Vec::new();
    let mut load = |seg: &Segment, skip: &dyn Fn(&str) -> bool| -> Result<()> {
        for (id, e) in seg.files().iter().enumerate() {
            if skip(&e.path) {
                continue;
            }
            let packed = seg.stub_packed(id as u32)?;
            files.push(IndexedFile {
                path: e.path.clone(),
                content: e.content.clone(),
                blob: StubBlob {
                    stub: unpack_stub(&packed)?,
                    packed,
                },
            });
        }
        Ok(())
    };
    load(older, &|p| {
        newer.find_file(p).is_some() || newer.has_tombstone(p)
    })?;
    load(newer, &|_| false)?;
    let tombstones: Vec<String> = if oldest {
        Vec::new()
    } else {
        older
            .tombstones()
            .iter()
            .chain(newer.tombstones())
            .cloned()
            .collect()
    };
    let refs: Vec<&IndexedFile> = files.iter().collect();
    build_segment(&refs, &tombstones)
}
