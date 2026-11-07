//! The index: snapshots, segment files, session deltas, and the query entry points.
//!
//! Folder layout under the folder that the caller gives:
//!
//! ```text
//! stubs/       the stub store (shared by all repos and snapshots)
//! segments/    <id>.seg, immutable segment files
//! snapshots/   <name>.json, the segment list of each snapshot
//! ```
//!
//! A [`Snapshot`] is the list of segments of one commit (or one branch tip), oldest first. A
//! session reads its snapshot plus its own delta. The delta is a small segment in memory with
//! the files that the session changed, and tombstones for the files that it removed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use aivory_syntax::DeclKind;
use serde::{Deserialize, Serialize};

use crate::fsutil::write_atomic;
use crate::query::{DefHit, Layer, Page, PageReq, RefHit, SymbolHit, View};
use crate::segment::{IndexedFile, Segment, build_segment, segment_id};
use crate::{Error, Result, StubStore};

/// The segment list of one commit or branch tip.
#[derive(Debug)]
pub struct Snapshot {
    pub name: String,
    /// The folder of the checkout that the snapshot was built from. Reference lines of results
    /// come from files below it.
    pub root: Option<PathBuf>,
    /// The segments, oldest first.
    pub segments: Vec<Arc<Segment>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: u32,
    name: String,
    root: Option<String>,
    segments: Vec<String>,
}

#[derive(Debug)]
struct SessionState {
    base: String,
    root: PathBuf,
    files: BTreeMap<String, IndexedFile>,
    tombstones: BTreeSet<String>,
    segment: Option<Arc<Segment>>,
}

#[derive(Debug, Default)]
struct State {
    snapshots: BTreeMap<String, Arc<Snapshot>>,
    current: Option<String>,
    sessions: HashMap<String, Arc<SessionState>>,
}

#[derive(Debug)]
pub(crate) struct Inner {
    pub dir: PathBuf,
    pub store: StubStore,
    state: RwLock<State>,
    /// Serializes the operations that change snapshots (refresh, merge).
    pub modify: Mutex<()>,
}

/// The index of the repos in one folder. Cloning is cheap: all clones share the state.
#[derive(Debug, Clone)]
pub struct Index {
    pub(crate) inner: Arc<Inner>,
}

/// What a session update did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionReport {
    /// Files that the update parsed.
    pub parsed: usize,
    /// Files whose stub came from the store.
    pub from_store: usize,
    /// Files in the delta after the update.
    pub files: usize,
    /// Tombstones in the delta after the update.
    pub tombstones: usize,
}

/// The use of a name in the delta of one session (for the conflict radar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUse {
    pub session: String,
    pub definitions: Vec<DefHit>,
    pub references: Vec<RefHit>,
}

/// Numbers about the index, for reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub snapshots: usize,
    /// Segments in use (a segment shared by two snapshots counts once).
    pub segments: usize,
    pub segment_bytes: u64,
    /// Bytes that the open segments keep in memory.
    pub resident_bytes: u64,
    /// Live files of the current snapshot.
    pub files: usize,
}

fn lock_err<T>(e: std::sync::PoisonError<T>) -> T {
    e.into_inner()
}

/// A snapshot name as a file name.
fn safe_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
            out.push(c);
        } else {
            out.push_str(&format!("_{:x}_", c as u32));
        }
    }
    out
}

impl Index {
    /// Open the index in `dir` (the folder is created). Snapshots whose segments are missing
    /// or damaged are left out; the next refresh builds them again.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        for sub in ["segments", "snapshots"] {
            std::fs::create_dir_all(dir.join(sub))?;
        }
        let mut state = State::default();
        let mut open: HashMap<String, Arc<Segment>> = HashMap::new();
        for entry in std::fs::read_dir(dir.join("snapshots"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(manifest) = std::fs::read(&path).map_err(Error::from).and_then(|b| {
                serde_json::from_slice::<Manifest>(&b).map_err(|e| Error::Codec(e.to_string()))
            }) else {
                continue;
            };
            let mut segments = Vec::new();
            let mut ok = true;
            for id in &manifest.segments {
                if let Some(seg) = open.get(id) {
                    segments.push(seg.clone());
                    continue;
                }
                match Segment::open(&dir.join("segments").join(format!("{id}.seg"))) {
                    Ok(seg) => {
                        let seg = Arc::new(seg);
                        open.insert(id.clone(), seg.clone());
                        segments.push(seg);
                    }
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                state.snapshots.insert(
                    manifest.name.clone(),
                    Arc::new(Snapshot {
                        name: manifest.name,
                        root: manifest.root.map(PathBuf::from),
                        segments,
                    }),
                );
            }
        }
        state.current = std::fs::read_to_string(dir.join("current"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|n| state.snapshots.contains_key(n));
        Ok(Self {
            inner: Arc::new(Inner {
                store: StubStore::new(dir.join("stubs")),
                dir,
                state: RwLock::new(state),
                modify: Mutex::new(()),
            }),
        })
    }

    /// The stub store.
    pub fn store(&self) -> &StubStore {
        &self.inner.store
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    fn read_state(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.inner.state.read().unwrap_or_else(lock_err)
    }

    fn write_state(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.inner.state.write().unwrap_or_else(lock_err)
    }

    // -----------------------------------------------------------------------------------------
    // Snapshots

    pub fn snapshot_names(&self) -> Vec<String> {
        self.read_state().snapshots.keys().cloned().collect()
    }

    pub fn snapshot(&self, name: &str) -> Option<Arc<Snapshot>> {
        self.read_state().snapshots.get(name).cloned()
    }

    /// The name of the snapshot that a query without a session reads.
    pub fn current(&self) -> Option<String> {
        self.read_state().current.clone()
    }

    /// Select the snapshot that a query without a session reads.
    pub fn set_current(&self, name: &str) -> Result<()> {
        let mut state = self.write_state();
        if !state.snapshots.contains_key(name) {
            return Err(Error::UnknownSnapshot(name.to_owned()));
        }
        state.current = Some(name.to_owned());
        std::fs::write(self.inner.dir.join("current"), name)?;
        Ok(())
    }

    /// Start snapshot `to` with the segments of snapshot `from` (for a new branch or commit).
    /// A refresh of `to` then parses only the files that differ.
    pub fn fork_snapshot(&self, from: &str, to: &str) -> Result<()> {
        let _guard = self.inner.modify.lock().unwrap_or_else(lock_err);
        let base = self
            .snapshot(from)
            .ok_or_else(|| Error::UnknownSnapshot(from.to_owned()))?;
        self.publish(Snapshot {
            name: to.to_owned(),
            root: base.root.clone(),
            segments: base.segments.clone(),
        })
    }

    /// Remove a snapshot. Its segment files stay until [`Index::gc`].
    pub fn delete_snapshot(&self, name: &str) -> Result<()> {
        let _guard = self.inner.modify.lock().unwrap_or_else(lock_err);
        let mut state = self.write_state();
        state.snapshots.remove(name);
        if state.current.as_deref() == Some(name) {
            state.current = None;
            let _ = std::fs::remove_file(self.inner.dir.join("current"));
        }
        let manifest = self
            .inner
            .dir
            .join("snapshots")
            .join(format!("{}.json", safe_name(name)));
        match std::fs::remove_file(manifest) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Write a segment file (when it does not exist) and open it.
    pub(crate) fn write_segment(&self, bytes: Vec<u8>) -> Result<Arc<Segment>> {
        let path = self
            .inner
            .dir
            .join("segments")
            .join(format!("{}.seg", segment_id(&bytes)));
        if !path.is_file() {
            write_atomic(&path, &bytes)?;
        }
        Ok(Arc::new(Segment::open(&path)?))
    }

    /// Save the manifest of a snapshot and make it visible. The first snapshot becomes the
    /// current one.
    pub(crate) fn publish(&self, snapshot: Snapshot) -> Result<()> {
        let manifest = Manifest {
            format: 1,
            name: snapshot.name.clone(),
            root: snapshot
                .root
                .as_ref()
                .map(|r| r.to_string_lossy().into_owned()),
            segments: snapshot
                .segments
                .iter()
                .map(|s| s.id().to_owned())
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| Error::Codec(e.to_string()))?;
        write_atomic(
            &self
                .inner
                .dir
                .join("snapshots")
                .join(format!("{}.json", safe_name(&snapshot.name))),
            &json,
        )?;
        let mut state = self.write_state();
        if state.current.is_none() {
            state.current = Some(snapshot.name.clone());
            let _ = std::fs::write(self.inner.dir.join("current"), &snapshot.name);
        }
        state
            .snapshots
            .insert(snapshot.name.clone(), Arc::new(snapshot));
        Ok(())
    }

    /// Delete the segment files that no snapshot lists. Returns the number of removed files.
    /// A file that is still open (Windows) stays for the next call.
    pub fn gc(&self) -> Result<usize> {
        let _guard = self.inner.modify.lock().unwrap_or_else(lock_err);
        let used: HashSet<String> = self
            .read_state()
            .snapshots
            .values()
            .flat_map(|s| s.segments.iter().map(|g| g.id().to_owned()))
            .collect();
        let mut removed = 0;
        for entry in std::fs::read_dir(self.inner.dir.join("segments"))? {
            let path = entry?.path();
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let stale = path.extension().is_none_or(|e| e != "seg") || !used.contains(&stem);
            if stale && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        self.inner.store.compact()?;
        Ok(removed)
    }

    /// Write the stubs that wait in memory to disk.
    pub fn flush(&self) -> Result<()> {
        self.inner.store.flush()
    }

    /// Numbers about the index.
    pub fn stats(&self) -> IndexStats {
        let state = self.read_state();
        let mut seen = HashSet::new();
        let mut stats = IndexStats {
            snapshots: state.snapshots.len(),
            ..IndexStats::default()
        };
        for snap in state.snapshots.values() {
            for seg in &snap.segments {
                if seen.insert(seg.id().to_owned()) {
                    stats.segment_bytes += seg.size_bytes();
                    stats.resident_bytes += seg.resident_bytes();
                }
            }
        }
        stats.segments = seen.len();
        drop(state);
        stats.files = self.view(None).map(|v| v.file_count()).unwrap_or(0);
        stats
    }

    // -----------------------------------------------------------------------------------------
    // Views and queries

    /// The segments that a query reads. A known session reads its delta and the snapshot that
    /// it was based on. No session, or an unknown one, reads the current snapshot.
    pub fn view(&self, session: Option<&str>) -> Result<View> {
        let state = self.read_state();
        let delta = session.and_then(|s| state.sessions.get(s));
        let base = match delta {
            Some(d) => Some(d.base.as_str()),
            None => state.current.as_deref(),
        };
        let mut layers = Vec::new();
        if let Some(d) = delta
            && let Some(seg) = &d.segment
        {
            layers.push(Layer {
                segment: seg.clone(),
                root: Some(d.root.clone()),
            });
        }
        if let Some(name) = base {
            let snap = state
                .snapshots
                .get(name)
                .ok_or_else(|| Error::UnknownSnapshot(name.to_owned()))?;
            for seg in snap.segments.iter().rev() {
                layers.push(Layer {
                    segment: seg.clone(),
                    root: snap.root.clone(),
                });
            }
        }
        Ok(View::new(layers))
    }

    /// Where a name (or a qualified path such as `billing::Invoice::total`) is defined.
    pub fn definition(
        &self,
        session: Option<&str>,
        query: &str,
        req: PageReq,
    ) -> Result<Page<DefHit>> {
        self.view(session)?.definition(query, req)
    }

    /// Where a name is used (the references of the stubs; no resolve check).
    pub fn references(
        &self,
        session: Option<&str>,
        name: &str,
        req: PageReq,
    ) -> Result<Page<RefHit>> {
        self.view(session)?.references(name, req)
    }

    /// Search declarations by name (case-insensitive, prefix first, then fuzzy).
    pub fn symbols(
        &self,
        session: Option<&str>,
        query: &str,
        kind: Option<DeclKind>,
        req: PageReq,
    ) -> Result<Page<SymbolHit>> {
        self.view(session)?.symbols(query, kind, req)
    }

    /// The structure view of a file.
    pub fn outline(
        &self,
        session: Option<&str>,
        path: &str,
    ) -> Result<Option<Vec<aivory_syntax::OutlineItem>>> {
        self.view(session)?.outline(path)
    }

    /// The stub of a file.
    pub fn file_stub(
        &self,
        session: Option<&str>,
        path: &str,
    ) -> Result<Option<Arc<aivory_syntax::Stub>>> {
        self.view(session)?.file_stub(path)
    }

    // -----------------------------------------------------------------------------------------
    // Session deltas

    /// The ids of the sessions that have a delta.
    pub fn sessions(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.read_state().sessions.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Update the delta of a session. `changed` are paths (relative to `root`, with `/`) that
    /// the session added or changed. The method reads them from `root`, the worktree of the
    /// session. `removed` are paths that the session deleted. A changed path that cannot be
    /// read, or that has no known language, counts as removed. The first call sets the base
    /// snapshot (`base`, or the current one).
    pub fn session_update(
        &self,
        session: &str,
        base: Option<&str>,
        root: &Path,
        changed: &[String],
        removed: &[String],
    ) -> Result<SessionReport> {
        let old = self.read_state().sessions.get(session).cloned();
        let base_name = match (&old, base) {
            (Some(o), None) => o.base.clone(),
            (_, Some(b)) => b.to_owned(),
            (None, None) => self
                .current()
                .ok_or_else(|| Error::UnknownSnapshot("(none)".to_owned()))?,
        };
        if self.snapshot(&base_name).is_none() {
            return Err(Error::UnknownSnapshot(base_name));
        }
        let mut files = old.as_ref().map(|o| o.files.clone()).unwrap_or_default();
        let mut tombstones = old
            .as_ref()
            .map(|o| o.tombstones.clone())
            .unwrap_or_default();
        let mut report = SessionReport::default();
        for path in changed {
            match crate::indexer::index_path(&self.inner.store, root, path, u64::MAX)? {
                Some((file, parsed)) => {
                    if parsed {
                        report.parsed += 1;
                    } else {
                        report.from_store += 1;
                    }
                    tombstones.remove(path);
                    files.insert(path.clone(), file);
                }
                None => {
                    files.remove(path);
                    tombstones.insert(path.clone());
                }
            }
        }
        for path in removed {
            files.remove(path);
            tombstones.insert(path.clone());
        }
        let list: Vec<&IndexedFile> = files.values().collect();
        let tombs: Vec<String> = tombstones.iter().cloned().collect();
        let segment = if list.is_empty() && tombs.is_empty() {
            None
        } else {
            Some(Arc::new(Segment::from_bytes(build_segment(
                &list, &tombs,
            )?)?))
        };
        report.files = files.len();
        report.tombstones = tombstones.len();
        let state = SessionState {
            base: base_name,
            root: root.to_path_buf(),
            files,
            tombstones,
            segment,
        };
        self.write_state()
            .sessions
            .insert(session.to_owned(), Arc::new(state));
        Ok(report)
    }

    /// Remove the delta of a session. Returns true when the session had one.
    pub fn session_drop(&self, session: &str) -> bool {
        self.write_state().sessions.remove(session).is_some()
    }

    /// The paths that a session changed and the paths that it removed.
    pub fn session_paths(&self, session: &str) -> Option<(Vec<String>, Vec<String>)> {
        let state = self.read_state();
        let s = state.sessions.get(session)?;
        Some((
            s.files.keys().cloned().collect(),
            s.tombstones.iter().cloned().collect(),
        ))
    }

    /// Which live sessions define or use `name` in the files that they changed. Only the
    /// deltas are read (tier 3 of the conflict radar: callers of a changed symbol).
    pub fn sessions_touching(&self, name: &str) -> Result<Vec<SessionUse>> {
        let deltas: Vec<(String, Layer)> = self
            .read_state()
            .sessions
            .iter()
            .filter_map(|(id, s)| {
                s.segment.as_ref().map(|seg| {
                    (
                        id.clone(),
                        Layer {
                            segment: seg.clone(),
                            root: Some(s.root.clone()),
                        },
                    )
                })
            })
            .collect();
        let mut out = Vec::new();
        for (session, layer) in deltas {
            let view = View::new(vec![layer]);
            let all = PageReq::new(1, 1_000);
            let definitions = view.definition(name, all)?.items;
            let references = view.references(name, all)?.items;
            if !definitions.is_empty() || !references.is_empty() {
                out.push(SessionUse {
                    session,
                    definitions,
                    references,
                });
            }
        }
        out.sort_by(|a, b| a.session.cmp(&b.session));
        Ok(out)
    }
}
