//! Read-only index of the native session stores of Claude Code, Codex CLI, and Gemini CLI.
//!
//! The crate never writes to the store directories. The catalog reads only the head and the
//! tail of each session file, scans in parallel, and keeps a cache keyed by (path, mtime,
//! size).
//!
//! Entry point: [`SessionCatalog`]. The [`transcript`] module turns one session file into
//! terminal-pane lines, a session state and the sub-agents.

mod cache;
mod catalog;
mod claude;
mod codex;
mod gemini;
mod jsonl;
mod model;
mod roots;
mod store;
mod text;
pub mod timeutil;
pub mod transcript;

pub use cache::default_cache_file;
pub use catalog::{FindError, ScanReport, ScanStats, SessionCatalog};
pub use claude::ClaudeStore;
pub use codex::CodexStore;
pub use gemini::GeminiStore;
pub use model::{Provider, SessionFilter, SessionSummary};
pub use roots::StoreRoots;
pub use store::{FileRole, Parsed, SessionFile, SessionStore};
