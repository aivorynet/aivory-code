//! Segments: immutable files with a file table, `fst` name maps, and posting lists (L2).
//!
//! File layout (all integers little endian):
//!
//! ```text
//! magic "AIVSEG01" | format u32 | extractor u32 | 7 x (offset u64, length u64)
//! sections: meta | names fst | lower-case names fst | references fst
//!           | declaration postings | reference postings | packed stubs
//! ```
//!
//! - `meta` is the file table (sorted by path) and the tombstones, in postcard format,
//!   compressed with lz4.
//! - An `fst` maps a name to the offset of its posting list. `names` holds the names as they
//!   are, `lower` the lower-cased names (for search), and `refs` the names of the references.
//! - A posting list is a `u32` byte length followed by varints. Declaration postings hold
//!   (file, declaration, kind, line). Reference postings hold (file, range, line, column).
//! - The stub section holds one packed stub per file (see [`crate::pack_stub`]).
//!
//! The reader keeps only the file table and the three `fst` maps in memory. It reads posting
//! lists and stubs from the file when a query needs them. (A memory map would need `unsafe`
//! code, which the workspace denies. The OS file cache gives the same effect.)

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::hash::{BuildHasherDefault, Hasher};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aivory_syntax::{DeclKind, EXTRACTOR_VERSION, Language, Stub};
use fst::{Map, MapBuilder};
use serde::{Deserialize, Serialize};

use crate::codec::{Reader, put_varint};
use crate::fsutil::read_exact_at;
use crate::store::{StubBlob, unpack_stub};
use crate::{ContentId, Error, Result};

const MAGIC: &[u8; 8] = b"AIVSEG01";
const FORMAT: u32 = 1;
const SECTIONS: usize = 7;
const HEADER_LEN: usize = 8 + 4 + 4 + SECTIONS * 16;
/// A segment keeps at most this many decoded stubs.
const STUB_CACHE: usize = 256;

/// A fast, simple hasher for the posting maps of the builder.
#[derive(Default)]
struct FxHasher(u64);

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(word))
                .wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

const KINDS: [DeclKind; 19] = [
    DeclKind::Module,
    DeclKind::Namespace,
    DeclKind::Class,
    DeclKind::Struct,
    DeclKind::Enum,
    DeclKind::Variant,
    DeclKind::Interface,
    DeclKind::Trait,
    DeclKind::Impl,
    DeclKind::Function,
    DeclKind::Method,
    DeclKind::Constructor,
    DeclKind::Field,
    DeclKind::Property,
    DeclKind::Variable,
    DeclKind::Constant,
    DeclKind::TypeAlias,
    DeclKind::Macro,
    DeclKind::Section,
];

fn kind_to_u8(kind: DeclKind) -> u8 {
    KINDS.iter().position(|k| *k == kind).unwrap_or(0) as u8
}

fn kind_from_u8(byte: u8) -> DeclKind {
    KINDS
        .get(byte as usize)
        .copied()
        .unwrap_or(DeclKind::Variable)
}

/// One file ready for a segment: its path, content id, and stub.
#[derive(Debug, Clone)]
pub struct IndexedFile {
    /// The path relative to the repo root, with `/` separators.
    pub path: String,
    pub content: ContentId,
    pub blob: StubBlob,
}

/// A row of the file table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub content: ContentId,
    /// The blake3 hash of the source (equal to `Stub::hash`).
    pub hash: String,
    pub language: Language,
    stub_off: u64,
    stub_len: u32,
    pub decls: u32,
    pub refs: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Meta {
    files: Vec<FileEntry>,
    tombstones: Vec<String>,
}

/// A declaration in a posting list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclPosting {
    pub file: u32,
    /// The index in `Stub::declarations`.
    pub decl: u32,
    pub kind: DeclKind,
    /// The line of the name, from 0.
    pub line: u32,
}

/// A reference in a posting list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefPosting {
    pub file: u32,
    pub start_byte: u32,
    pub end_byte: u32,
    /// The line, from 0.
    pub line: u32,
    /// The byte column, from 0.
    pub col: u32,
}

// ---------------------------------------------------------------------------------------------
// Builder

/// Build the bytes of a segment. A path that occurs twice keeps its last file. A tombstone for
/// a path that has a file is dropped.
pub fn build_segment(files: &[&IndexedFile], tombstones: &[String]) -> Result<Vec<u8>> {
    // Sorted by path; the last file of a path wins.
    let mut sorted: Vec<&IndexedFile> = Vec::with_capacity(files.len());
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|a, b| files[*a].path.cmp(&files[*b].path).then(a.cmp(b)));
    for i in order {
        match sorted.last() {
            Some(prev) if prev.path == files[i].path => *sorted.last_mut().unwrap() = files[i],
            _ => sorted.push(files[i]),
        }
    }
    let mut tombs: Vec<String> = tombstones
        .iter()
        .filter(|t| sorted.binary_search_by(|f| f.path.as_str().cmp(t)).is_err())
        .cloned()
        .collect();
    tombs.sort();
    tombs.dedup();

    let mut stubs: Vec<u8> = Vec::new();
    let mut entries: Vec<FileEntry> = Vec::with_capacity(sorted.len());
    let mut decl_map: FxMap<&str, Vec<DeclPosting>> = FxMap::default();
    let mut ref_map: FxMap<&str, Vec<RefPosting>> = FxMap::default();
    for (id, file) in sorted.iter().enumerate() {
        let id = id as u32;
        let stub: &Stub = &file.blob.stub;
        entries.push(FileEntry {
            path: file.path.clone(),
            content: file.content.clone(),
            hash: stub.hash.clone(),
            language: stub.language,
            stub_off: stubs.len() as u64,
            stub_len: file.blob.packed.len() as u32,
            decls: stub.declarations.len() as u32,
            refs: stub.references.len() as u32,
        });
        stubs.extend_from_slice(&file.blob.packed);
        for (n, d) in stub.declarations.iter().enumerate() {
            if d.name.is_empty() {
                continue;
            }
            decl_map
                .entry(d.name.as_str())
                .or_default()
                .push(DeclPosting {
                    file: id,
                    decl: n as u32,
                    kind: d.kind,
                    line: d.name_range.start_line,
                });
        }
        for r in &stub.references {
            if r.name.is_empty() {
                continue;
            }
            ref_map
                .entry(r.name.as_str())
                .or_default()
                .push(RefPosting {
                    file: id,
                    start_byte: r.range.start_byte,
                    end_byte: r.range.end_byte,
                    line: r.range.start_line,
                    col: r.range.start_col,
                });
        }
    }

    // Declaration postings, exact names.
    let mut names: Vec<&str> = decl_map.keys().copied().collect();
    names.sort_unstable();
    let mut decl_section: Vec<u8> = Vec::new();
    let mut names_fst = MapBuilder::new(Vec::new())?;
    for name in &names {
        names_fst.insert(name.as_bytes(), decl_section.len() as u64)?;
        encode_decls(&decl_map[name], &mut decl_section);
    }
    let names_fst = names_fst.into_inner()?;

    // Lower-case names. Names that differ in case share one list.
    let mut lower_map: BTreeMap<String, Vec<DeclPosting>> = BTreeMap::new();
    for name in &names {
        lower_map
            .entry(name.to_lowercase())
            .or_default()
            .extend_from_slice(&decl_map[name]);
    }
    let mut lower_fst = MapBuilder::new(Vec::new())?;
    for (name, list) in &mut lower_map {
        list.sort_unstable_by_key(|p| (p.file, p.decl));
        lower_fst.insert(name.as_bytes(), decl_section.len() as u64)?;
        encode_decls(list, &mut decl_section);
    }
    let lower_fst = lower_fst.into_inner()?;

    // Reference postings.
    let mut ref_names: Vec<&str> = ref_map.keys().copied().collect();
    ref_names.sort_unstable();
    let mut ref_section: Vec<u8> = Vec::new();
    let mut refs_fst = MapBuilder::new(Vec::new())?;
    for name in &ref_names {
        let list = ref_map.get_mut(name).expect("key from the same map");
        list.sort_unstable_by_key(|p| (p.file, p.start_byte));
        refs_fst.insert(name.as_bytes(), ref_section.len() as u64)?;
        encode_refs(list, &mut ref_section);
    }
    let refs_fst = refs_fst.into_inner()?;

    let meta = lz4_flex::compress_prepend_size(&postcard::to_stdvec(&Meta {
        files: entries,
        tombstones: tombs,
    })?);

    let parts: [&[u8]; SECTIONS] = [
        &meta,
        &names_fst,
        &lower_fst,
        &refs_fst,
        &decl_section,
        &ref_section,
        &stubs,
    ];
    let mut out = Vec::with_capacity(HEADER_LEN + parts.iter().map(|p| p.len()).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&EXTRACTOR_VERSION.to_le_bytes());
    let mut offset = HEADER_LEN as u64;
    for part in parts {
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(part.len() as u64).to_le_bytes());
        offset += part.len() as u64;
    }
    for part in parts {
        out.extend_from_slice(part);
    }
    Ok(out)
}

/// The id of a segment: the first 16 hex characters of the blake3 hash of its bytes.
pub fn segment_id(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().as_str()[..16].to_owned()
}

fn finish_list(out: &mut [u8], start: usize) {
    let len = (out.len() - start - 4) as u32;
    out[start..start + 4].copy_from_slice(&len.to_le_bytes());
}

fn encode_decls(list: &[DeclPosting], out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    put_varint(out, list.len() as u64);
    let mut prev_file = 0;
    for p in list {
        put_varint(out, u64::from(p.file - prev_file));
        put_varint(out, u64::from(p.decl));
        out.push(kind_to_u8(p.kind));
        put_varint(out, u64::from(p.line));
        prev_file = p.file;
    }
    finish_list(out, start);
}

fn encode_refs(list: &[RefPosting], out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    put_varint(out, list.len() as u64);
    let (mut prev_file, mut prev_start, mut prev_line) = (0u32, 0u32, 0u32);
    for (i, p) in list.iter().enumerate() {
        let new_file = i == 0 || p.file != prev_file;
        put_varint(out, u64::from(p.file - prev_file));
        if new_file {
            put_varint(out, u64::from(p.start_byte));
            put_varint(out, u64::from(p.line));
        } else {
            put_varint(out, u64::from(p.start_byte - prev_start));
            // Lines can only grow inside a file (the list is sorted by byte).
            put_varint(out, u64::from(p.line - prev_line));
        }
        put_varint(out, u64::from(p.end_byte - p.start_byte));
        put_varint(out, u64::from(p.col));
        (prev_file, prev_start, prev_line) = (p.file, p.start_byte, p.line);
    }
    finish_list(out, start);
}

// ---------------------------------------------------------------------------------------------
// Reader

#[derive(Debug, Clone, Copy, Default)]
struct Section {
    off: u64,
    len: u64,
}

enum Backing {
    Mem(Vec<u8>),
    File(File),
}

impl Backing {
    fn read(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        match self {
            Backing::Mem(bytes) => {
                let (off, end) = (off as usize, off as usize + len);
                bytes
                    .get(off..end)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| Error::Corrupt("read past the end".into()))
            }
            Backing::File(file) => {
                let mut buf = vec![0; len];
                read_exact_at(file, &mut buf, off)?;
                Ok(buf)
            }
        }
    }
}

/// An open segment. It is immutable and safe to share between threads.
pub struct Segment {
    id: String,
    meta: Meta,
    names: Map<Vec<u8>>,
    lower: Map<Vec<u8>>,
    refs: Map<Vec<u8>>,
    sections: [Section; SECTIONS],
    backing: Backing,
    size: u64,
    cache: Mutex<HashMap<u32, Arc<Stub>>>,
}

impl std::fmt::Debug for Segment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segment")
            .field("id", &self.id)
            .field("files", &self.meta.files.len())
            .field("tombstones", &self.meta.tombstones.len())
            .field("bytes", &self.size)
            .finish()
    }
}

impl Segment {
    /// Open a segment file. The id is the file stem.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        let mut header = vec![0; HEADER_LEN];
        file.read_exact(&mut header)
            .map_err(|_| Error::Corrupt("file is shorter than the header".into()))?;
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::init(id, Backing::File(file), &header, size)
    }

    /// A segment in memory (a session delta, or a test).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(Error::Corrupt("data is shorter than the header".into()));
        }
        let header = bytes[..HEADER_LEN].to_vec();
        let (id, size) = (segment_id(&bytes), bytes.len() as u64);
        Self::init(id, Backing::Mem(bytes), &header, size)
    }

    fn init(id: String, backing: Backing, header: &[u8], size: u64) -> Result<Self> {
        if &header[..8] != MAGIC {
            return Err(Error::Corrupt("bad magic".into()));
        }
        let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        if word(8) != FORMAT {
            return Err(Error::Corrupt(format!("unknown format {}", word(8))));
        }
        if word(12) != EXTRACTOR_VERSION {
            return Err(Error::Corrupt(format!(
                "built with extractor version {}",
                word(12)
            )));
        }
        let mut sections = [Section::default(); SECTIONS];
        for (i, s) in sections.iter_mut().enumerate() {
            let at = 16 + i * 16;
            s.off = u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
            s.len = u64::from_le_bytes(header[at + 8..at + 16].try_into().unwrap());
            if s.off.checked_add(s.len).is_none_or(|end| end > size) {
                return Err(Error::Corrupt(format!("section {i} is out of bounds")));
            }
        }
        let load = |i: usize| backing.read(sections[i].off, sections[i].len as usize);
        let meta_raw = lz4_flex::decompress_size_prepended(&load(0)?)
            .map_err(|e| Error::Corrupt(format!("meta: {e}")))?;
        let meta: Meta = postcard::from_bytes(&meta_raw)?;
        Ok(Self {
            id,
            meta,
            names: Map::new(load(1)?)?,
            lower: Map::new(load(2)?)?,
            refs: Map::new(load(3)?)?,
            sections,
            backing,
            size,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The size of the segment in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.size
    }

    /// The bytes that the open segment keeps in memory: the three maps and the file table
    /// (the table size is an estimate).
    pub fn resident_bytes(&self) -> u64 {
        let table: usize = self
            .meta
            .files
            .iter()
            .map(|f| std::mem::size_of::<FileEntry>() + f.path.len() + f.hash.len() + 64)
            .sum();
        (self.sections[1].len + self.sections[2].len + self.sections[3].len) + table as u64
    }

    /// The file table, sorted by path.
    pub fn files(&self) -> &[FileEntry] {
        &self.meta.files
    }

    /// The paths that this segment removes from older segments, sorted.
    pub fn tombstones(&self) -> &[String] {
        &self.meta.tombstones
    }

    pub fn is_empty(&self) -> bool {
        self.meta.files.is_empty() && self.meta.tombstones.is_empty()
    }

    /// The file id of a path.
    pub fn find_file(&self, path: &str) -> Option<u32> {
        self.meta
            .files
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .ok()
            .map(|i| i as u32)
    }

    pub fn has_tombstone(&self, path: &str) -> bool {
        self.meta
            .tombstones
            .binary_search_by(|t| t.as_str().cmp(path))
            .is_ok()
    }

    pub(crate) fn names_map(&self, lower: bool) -> &Map<Vec<u8>> {
        if lower { &self.lower } else { &self.names }
    }

    /// The packed stub of a file.
    pub fn stub_packed(&self, file: u32) -> Result<Vec<u8>> {
        let e = self
            .meta
            .files
            .get(file as usize)
            .ok_or_else(|| Error::Corrupt("file id out of range".into()))?;
        self.backing
            .read(self.sections[6].off + e.stub_off, e.stub_len as usize)
    }

    /// The stub of a file. The segment keeps a small cache of decoded stubs.
    pub fn stub(&self, file: u32) -> Result<Arc<Stub>> {
        if let Some(stub) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&file)
        {
            return Ok(stub.clone());
        }
        let stub = Arc::new(unpack_stub(&self.stub_packed(file)?)?);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= STUB_CACHE {
            cache.clear();
        }
        cache.insert(file, stub.clone());
        Ok(stub)
    }

    fn read_list(&self, section: usize, rel: u64) -> Result<Vec<u8>> {
        let s = self.sections[section];
        let head = self.backing.read(s.off + rel, 4)?;
        let len = u32::from_le_bytes(head[..4].try_into().unwrap()) as usize;
        self.backing.read(s.off + rel + 4, len)
    }

    /// The declaration postings of a key in `names` or `lower`, from the offset that the map
    /// holds.
    pub(crate) fn decls_at(&self, rel: u64) -> Result<Vec<DeclPosting>> {
        let raw = self.read_list(4, rel)?;
        let bad = || Error::Corrupt("declaration postings".into());
        let mut r = Reader::new(&raw);
        let count = r.varint().ok_or_else(bad)? as usize;
        let mut out = Vec::with_capacity(count);
        let mut file = 0u32;
        for _ in 0..count {
            file += r.u32().ok_or_else(bad)?;
            out.push(DeclPosting {
                file,
                decl: r.u32().ok_or_else(bad)?,
                kind: kind_from_u8(r.byte().ok_or_else(bad)?),
                line: r.u32().ok_or_else(bad)?,
            });
        }
        Ok(out)
    }

    /// The declarations with this name (`lower`: compare lower-cased, `name` lower-cased).
    pub fn decl_postings(&self, lower: bool, name: &str) -> Result<Vec<DeclPosting>> {
        match self.names_map(lower).get(name.as_bytes()) {
            Some(rel) => self.decls_at(rel),
            None => Ok(Vec::new()),
        }
    }

    /// The references with this name.
    pub fn ref_postings(&self, name: &str) -> Result<Vec<RefPosting>> {
        let Some(rel) = self.refs.get(name.as_bytes()) else {
            return Ok(Vec::new());
        };
        let raw = self.read_list(5, rel)?;
        let bad = || Error::Corrupt("reference postings".into());
        let mut r = Reader::new(&raw);
        let count = r.varint().ok_or_else(bad)? as usize;
        let mut out = Vec::with_capacity(count);
        let (mut file, mut start, mut line) = (0u32, 0u32, 0u32);
        for i in 0..count {
            let file_delta = r.u32().ok_or_else(bad)?;
            let new_file = i == 0 || file_delta != 0;
            file += file_delta;
            if new_file {
                start = r.u32().ok_or_else(bad)?;
                line = r.u32().ok_or_else(bad)?;
            } else {
                start += r.u32().ok_or_else(bad)?;
                line += r.u32().ok_or_else(bad)?;
            }
            let len = r.u32().ok_or_else(bad)?;
            out.push(RefPosting {
                file,
                start_byte: start,
                end_byte: start + len,
                line,
                col: r.u32().ok_or_else(bad)?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_syntax::Language;

    fn file(path: &str, src: &str) -> IndexedFile {
        IndexedFile {
            path: path.into(),
            content: ContentId::of_bytes(src.as_bytes()),
            blob: StubBlob::new(Stub::parse(Language::Rust, src).unwrap()).unwrap(),
        }
    }

    #[test]
    fn build_and_read_back() {
        let a = file(
            "src/a.rs",
            "pub struct Foo;\nfn go() { helper(); helper(); }\n",
        );
        let b = file("src/b.rs", "fn helper() {}\nfn Go() { helper(); }\n");
        let bytes = build_segment(&[&b, &a], &["old.rs".into(), "src/a.rs".into()]).unwrap();
        let seg = Segment::from_bytes(bytes).unwrap();
        assert_eq!(seg.files().len(), 2);
        assert_eq!(seg.files()[0].path, "src/a.rs");
        // A tombstone for a present file is dropped.
        assert_eq!(seg.tombstones(), ["old.rs"]);
        assert!(seg.has_tombstone("old.rs") && !seg.has_tombstone("src/a.rs"));

        let foo = seg.decl_postings(false, "Foo").unwrap();
        assert_eq!(foo.len(), 1);
        assert_eq!((foo[0].file, foo[0].kind), (0, DeclKind::Struct));
        // `go` and `Go` differ in case but share the lower-case entry.
        assert_eq!(seg.decl_postings(false, "go").unwrap().len(), 1);
        assert_eq!(seg.decl_postings(true, "go").unwrap().len(), 2);

        let refs = seg.ref_postings("helper").unwrap();
        assert_eq!(refs.len(), 3);
        assert_eq!(refs.iter().filter(|r| r.file == 0).count(), 2);
        let stub = seg.stub(refs[0].file).unwrap();
        let r = stub
            .references
            .iter()
            .find(|r| r.range.start_byte == refs[0].start_byte)
            .unwrap();
        assert_eq!(
            (r.range.start_line, r.range.start_col),
            (refs[0].line, refs[0].col)
        );
        assert_eq!(r.range.end_byte, refs[0].end_byte);
    }

    #[test]
    fn rejects_damaged_data() {
        assert!(Segment::from_bytes(vec![0; 10]).is_err());
        let a = file("a.rs", "fn a() {}");
        let mut bytes = build_segment(&[&a], &[]).unwrap();
        bytes[0] = b'X';
        assert!(Segment::from_bytes(bytes).is_err());
    }
}
