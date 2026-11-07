use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Provider, SessionSummary};

/// What a candidate file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRole {
    /// A main session file.
    Session,
    /// A sidechain (sub-agent) file. `parent_id` is `None` when only the file content knows it.
    Sidechain { parent_id: Option<String> },
}

/// A candidate session file found by [`SessionStore::enumerate`].
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub mtime_ms: i64,
    pub size: u64,
    pub role: FileRole,
    /// Working directory known from outside the file, used when the file has none.
    pub cwd_hint: Option<String>,
}

/// The result of parsing one file. This is the unit that the cache stores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[allow(clippy::large_enum_variant)] // one summary per file; boxing would only add indirection
pub enum Parsed {
    Session(SessionSummary),
    Sidechain { parent_id: String },
}

/// One native session store.
pub trait SessionStore: Send + Sync {
    fn provider(&self) -> Provider;

    /// List candidate files. Must be cheap: directory listing and metadata only.
    fn enumerate(&self) -> Vec<SessionFile>;

    /// Read the head and the tail of one file. Returns `None` for a file that is not a session.
    fn parse(&self, file: &SessionFile) -> Option<Parsed>;
}

pub(crate) fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
