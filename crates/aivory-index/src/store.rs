//! The stub store: stubs on disk, keyed by (content id, grammar version, extractor version).
//!
//! A stub is parsed once per content. Two branches, two worktrees, and two repos that hold the
//! same file share one stub. A stub is stored in postcard format, compressed with lz4.
//!
//! Many small files are slow on some file systems (20,000 loose files cost 20 seconds on
//! Windows). So the store writes **packs**: a new stub waits in memory, and [`StubStore::flush`]
//! writes the waiting stubs as one `<id>.pack` file of concatenated stubs plus a small
//! `<id>.idx` file (key, offset, length). The store reads the `.idx` files when it is
//! created, and it reads one stub with one positional read. [`StubStore::compact`] joins small
//! packs and drops stubs of old extractor versions.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::sync::{OnceLock, atomic::AtomicU64};

use aivory_syntax::{EXTRACTOR_VERSION, Language, Stub};

use crate::fsutil::{read_exact_at, write_atomic};
use crate::{ContentId, Error, Result};

/// The store writes a pack when this many stubs wait.
const FLUSH_AT: usize = 2048;
/// [`StubStore::compact`] joins packs with fewer entries than this.
const SMALL_PACK: usize = 4096;

/// A stub with its packed form (what the store and the segments keep).
#[derive(Debug, Clone)]
pub struct StubBlob {
    pub stub: Stub,
    /// `lz4(postcard(stub))`.
    pub packed: Vec<u8>,
}

impl StubBlob {
    /// Pack a stub.
    pub fn new(stub: Stub) -> Result<Self> {
        let packed = pack_stub(&stub)?;
        Ok(Self { stub, packed })
    }
}

/// Pack a stub: postcard, then lz4 with the size in front.
pub fn pack_stub(stub: &Stub) -> Result<Vec<u8>> {
    let raw = postcard::to_stdvec(stub)?;
    Ok(lz4_flex::compress_prepend_size(&raw))
}

/// Unpack the result of [`pack_stub`].
pub fn unpack_stub(packed: &[u8]) -> Result<Stub> {
    let raw = lz4_flex::decompress_size_prepended(packed)
        .map_err(|e| Error::Codec(format!("stub does not decompress: {e}")))?;
    Ok(postcard::from_bytes(&raw)?)
}

/// A short tag of the grammar version of a language.
fn grammar_tag(language: Language) -> &'static str {
    static TAGS: OnceLock<Vec<String>> = OnceLock::new();
    let tags = TAGS.get_or_init(|| {
        Language::ALL
            .iter()
            .map(|l| {
                blake3::hash(l.grammar_version().as_bytes())
                    .to_hex()
                    .as_str()[..8]
                    .to_owned()
            })
            .collect()
    });
    let i = Language::ALL
        .iter()
        .position(|l| *l == language)
        .unwrap_or(0);
    &tags[i]
}

/// The key of a stub: content id, language, grammar tag, extractor version.
fn entry_key(id: &ContentId, language: Language) -> String {
    format!(
        "{}.{}.{}.{EXTRACTOR_VERSION}",
        id.key(),
        language.name(),
        grammar_tag(language)
    )
}

/// Where a stub lies in a pack.
#[derive(Debug, Clone, Copy)]
struct Loc {
    pack: usize,
    off: u64,
    len: u32,
}

#[derive(Debug)]
struct Pack {
    id: String,
    file: File,
}

#[derive(Debug, Default)]
struct Packs {
    packs: Vec<Pack>,
    index: HashMap<String, Loc>,
}

#[derive(Debug)]
struct Inner {
    dir: PathBuf,
    packs: RwLock<Packs>,
    /// Stubs that wait for the next pack.
    pending: Mutex<HashMap<String, Arc<Vec<u8>>>>,
}

/// The folder of the stubs. Cloning is cheap: all clones share the state.
#[derive(Debug, Clone)]
pub struct StubStore {
    inner: Arc<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl Inner {
    fn flush(&self) -> Result<()> {
        let waiting: Vec<(String, Arc<Vec<u8>>)> = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.is_empty() {
                return Ok(());
            }
            pending.drain().collect()
        };
        let result = self.write_pack(&waiting);
        if result.is_err() {
            // Keep the stubs for the next try.
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.extend(waiting);
        }
        result
    }

    fn write_pack(&self, entries: &[(String, Arc<Vec<u8>>)]) -> Result<()> {
        let dir = self.dir.join("packs");
        std::fs::create_dir_all(&dir)?;
        let mut data: Vec<u8> = Vec::new();
        let mut rows: Vec<(String, u64, u32)> = Vec::with_capacity(entries.len());
        for (key, bytes) in entries {
            rows.push((key.clone(), data.len() as u64, bytes.len() as u32));
            data.extend_from_slice(bytes);
        }
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = format!(
            "{}-{}-{:?}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now(),
            rows.len()
        );
        let id = blake3::hash(nonce.as_bytes()).to_hex().as_str()[..16].to_owned();
        write_atomic(&dir.join(format!("{id}.pack")), &data)?;
        write_atomic(&dir.join(format!("{id}.idx")), &postcard::to_stdvec(&rows)?)?;
        let file = File::open(dir.join(format!("{id}.pack")))?;
        let mut packs = self.packs.write().unwrap_or_else(|e| e.into_inner());
        let n = packs.packs.len();
        packs.packs.push(Pack { id, file });
        for (key, off, len) in rows {
            packs.index.insert(key, Loc { pack: n, off, len });
        }
        Ok(())
    }
}

impl StubStore {
    /// A store in `dir`. The call reads the pack index files of the folder; a missing folder
    /// is an empty store, and the store creates it with the first pack.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let mut packs = Packs::default();
        if let Ok(entries) = std::fs::read_dir(dir.join("packs")) {
            let mut idx_files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "idx"))
                .collect();
            idx_files.sort();
            for idx in idx_files {
                let Ok(rows) = std::fs::read(&idx)
                    .map_err(Error::from)
                    .and_then(|b| Ok(postcard::from_bytes::<Vec<(String, u64, u32)>>(&b)?))
                else {
                    continue;
                };
                let Ok(file) = File::open(idx.with_extension("pack")) else {
                    continue;
                };
                let n = packs.packs.len();
                packs.packs.push(Pack {
                    id: idx
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    file,
                });
                for (key, off, len) in rows {
                    packs.index.insert(key, Loc { pack: n, off, len });
                }
            }
        }
        Self {
            inner: Arc::new(Inner {
                dir,
                packs: RwLock::new(packs),
                pending: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    /// The number of stubs in the store (written and waiting).
    pub fn len(&self) -> usize {
        let written = self
            .inner
            .packs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .index
            .len();
        written
            + self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_packed(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(bytes) = self
            .inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            return Some(bytes.as_ref().clone());
        }
        let packs = self.inner.packs.read().unwrap_or_else(|e| e.into_inner());
        let loc = *packs.index.get(key)?;
        let mut buf = vec![0; loc.len as usize];
        read_exact_at(&packs.packs[loc.pack].file, &mut buf, loc.off).ok()?;
        Some(buf)
    }

    /// The stub of a content, when the store has one for the current grammar and extractor.
    /// A damaged entry counts as missing.
    pub fn get(&self, id: &ContentId, language: Language) -> Option<StubBlob> {
        let packed = self.read_packed(&entry_key(id, language))?;
        let stub = unpack_stub(&packed).ok()?;
        (stub.extractor_version == EXTRACTOR_VERSION && stub.language == language)
            .then_some(StubBlob { stub, packed })
    }

    /// True when the store has a stub (the check does not decode it).
    pub fn contains(&self, id: &ContentId, language: Language) -> bool {
        let key = entry_key(id, language);
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key)
            || self
                .inner
                .packs
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .index
                .contains_key(&key)
    }

    /// Add a stub. It waits in memory until [`StubStore::flush`] (or until enough stubs wait).
    pub fn put(&self, id: &ContentId, language: Language, blob: &StubBlob) -> Result<()> {
        let key = entry_key(id, language);
        if self.contains(id, language) {
            return Ok(());
        }
        let full = {
            let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.insert(key, Arc::new(blob.packed.clone()));
            pending.len() >= FLUSH_AT
        };
        if full {
            self.inner.flush()?;
        }
        Ok(())
    }

    /// Write the waiting stubs to disk as a new pack.
    pub fn flush(&self) -> Result<()> {
        self.inner.flush()
    }

    /// Join the small packs into one and drop the stubs of old extractor versions. Returns
    /// the number of packs that the call removed.
    pub fn compact(&self) -> Result<usize> {
        self.inner.flush()?;
        let (rows, old_ids, old_count) = {
            let packs = self.inner.packs.read().unwrap_or_else(|e| e.into_inner());
            let mut counts: HashMap<usize, usize> = HashMap::new();
            for loc in packs.index.values() {
                *counts.entry(loc.pack).or_default() += 1;
            }
            let small: Vec<usize> = (0..packs.packs.len())
                .filter(|p| counts.get(p).copied().unwrap_or(0) < SMALL_PACK)
                .collect();
            if small.len() < 2 {
                return Ok(0);
            }
            let suffix = format!(".{EXTRACTOR_VERSION}");
            let mut rows: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
            for (key, loc) in &packs.index {
                if small.contains(&loc.pack) && key.ends_with(&suffix) {
                    let mut buf = vec![0; loc.len as usize];
                    read_exact_at(&packs.packs[loc.pack].file, &mut buf, loc.off)?;
                    rows.push((key.clone(), Arc::new(buf)));
                }
            }
            let ids: Vec<String> = small.iter().map(|p| packs.packs[*p].id.clone()).collect();
            (rows, ids, small.len())
        };
        self.inner.write_pack(&rows)?;
        // The new pack is in the index now. Reload the folder without the old packs.
        let dir = self.inner.dir.join("packs");
        for id in &old_ids {
            let _ = std::fs::remove_file(dir.join(format!("{id}.idx")));
        }
        let fresh = StubStore::new(&self.inner.dir);
        {
            let mut mine = self.inner.packs.write().unwrap_or_else(|e| e.into_inner());
            let mut theirs = fresh.inner.packs.write().unwrap_or_else(|e| e.into_inner());
            std::mem::swap(&mut *mine, &mut *theirs);
        }
        // Close the old pack files before the removal (Windows needs it).
        drop(fresh);
        for id in &old_ids {
            let _ = std::fs::remove_file(dir.join(format!("{id}.pack")));
        }
        Ok(old_count)
    }

    /// The stub of a content: from the store, or parsed from `source` and added. The second
    /// value is true when the call parsed.
    pub fn get_or_parse(
        &self,
        id: &ContentId,
        language: Language,
        source: impl FnOnce() -> Result<String>,
    ) -> Result<(StubBlob, bool)> {
        if let Some(blob) = self.get(id, language) {
            return Ok((blob, false));
        }
        let text = source()?;
        let blob = StubBlob::new(Stub::parse(language, &text)?)?;
        // A failed write loses only the cache, not the result.
        let _ = self.put(id, language, &blob);
        Ok((blob, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_flush_reopen_compact() {
        let dir = tempfile::tempdir().unwrap();
        let store = StubStore::new(dir.path());
        let id = ContentId::of_bytes(b"fn a() {}");
        assert!(store.get(&id, Language::Rust).is_none());
        let (blob, parsed) = store
            .get_or_parse(&id, Language::Rust, || Ok("fn a() {}".into()))
            .unwrap();
        assert!(parsed);
        // A waiting stub is readable.
        let (again, parsed) = store
            .get_or_parse(&id, Language::Rust, || panic!("must not read"))
            .unwrap();
        assert!(!parsed);
        assert_eq!(blob.stub, again.stub);
        // The same content under another language is another entry.
        assert!(store.get(&id, Language::Python).is_none());

        // After a flush, a second store reads it from the pack.
        store.flush().unwrap();
        let other = StubStore::new(dir.path());
        assert_eq!(other.get(&id, Language::Rust).unwrap().stub, blob.stub);
        assert_eq!(other.len(), 1);

        // Two more packs, then compact: one pack, all stubs still there.
        for n in 0..2 {
            let src = format!("fn f{n}() {{}}");
            let id = ContentId::of_bytes(src.as_bytes());
            other.get_or_parse(&id, Language::Rust, || Ok(src)).unwrap();
            other.flush().unwrap();
        }
        assert_eq!(
            std::fs::read_dir(dir.path().join("packs")).unwrap().count(),
            6
        );
        assert_eq!(other.compact().unwrap(), 3);
        assert_eq!(
            std::fs::read_dir(dir.path().join("packs")).unwrap().count(),
            2
        );
        assert_eq!(other.len(), 3);
        assert_eq!(StubStore::new(dir.path()).len(), 3);
        assert!(other.get(&id, Language::Rust).is_some());
    }

    #[test]
    fn drop_flushes() {
        let dir = tempfile::tempdir().unwrap();
        let id = ContentId::of_bytes(b"x = 1");
        {
            let store = StubStore::new(dir.path());
            store
                .get_or_parse(&id, Language::Python, || Ok("x = 1".into()))
                .unwrap();
        }
        assert!(StubStore::new(dir.path()).contains(&id, Language::Python));
    }
}
