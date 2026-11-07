//! AIVory Code symbol index: stub store, segments, session deltas, queries.
//!
//! This crate is layer L2 of `docs/design/03-code-intelligence.md`. One [`Index`] serves all
//! sessions of a repo. A session reads the base segments of its snapshot plus its own delta.
//!
//! - [`StubStore`] keeps the L1 stubs of `aivory-syntax` on disk, keyed by content id, grammar
//!   version, and extractor version. A file with the same content is parsed once, across
//!   branches and worktrees. [`ContentId`] is the git blob id of a committed file, and the
//!   blake3 hash of any other content.
//! - A segment ([`Segment`]) is an immutable file for a batch of stubs: a file table, `fst`
//!   maps from names to postings, and reference postings. A [`Snapshot`] is the list of
//!   segments of one commit. [`Index::refresh`] and [`Index::update_files`] write new segments
//!   for changed files. [`Index::merge`] and [`Index::spawn_merge`] join small segments.
//! - A session delta is a small segment in memory with the files that the session changed and
//!   tombstones for the files that it removed ([`Index::session_update`]). Every query takes
//!   an optional session id.
//! - Queries: [`Index::definition`], [`Index::references`], [`Index::symbols`],
//!   [`Index::outline`], [`Index::file_stub`]. Results are compact (path, line, line text) and
//!   paged; [`Page::render`] writes the text format of the agent tools.
//!   [`Index::sessions_touching`] reads only the session deltas (the conflict radar).
//!
//! Open items: the index reads segments with file reads, not with a memory map (a memory map
//! needs `unsafe` code). It keeps only the file table and the `fst` maps of a segment in
//! memory.

mod codec;
mod error;
mod fsutil;
mod id;
mod index;
mod indexer;
mod query;
mod segment;
mod store;

pub use error::{Error, Result};
pub use id::ContentId;
pub use index::{Index, IndexStats, SessionReport, SessionUse, Snapshot};
pub use indexer::{IndexOptions, MergePolicy, MergeReport, RefreshReport};
pub use query::{CompactHit, DefHit, Layer, Page, PageReq, RefHit, SymbolHit, View};
pub use segment::{
    DeclPosting, FileEntry, IndexedFile, RefPosting, Segment, build_segment, segment_id,
};
pub use store::{StubBlob, StubStore, pack_stub, unpack_stub};
