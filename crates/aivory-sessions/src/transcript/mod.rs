//! Transcript view of a native session: the lines of a terminal pane, the session state, and
//! the sub-agents.
//!
//! - [`normalize`] reads a Claude, Codex or Gemini transcript file in a streaming way and
//!   gives [`Line`]s. The text has no decoration; the clients add it.
//! - [`tail`] and [`history`] cut the lines for the pane and for the scroll-back.
//! - [`derive_state`] gives the state of a session that the daemon does not run itself. It
//!   reads only the end of the file.
//! - [`subagents`] lists the sub-agents of a Claude session.
//! - [`trace`] gives the spans and the DAG of the deep dive.
//!
//! The readers skip a line that is not valid JSON. The files stay read-only.

mod claude;
mod codex;
mod common;
mod gemini;
mod spans;

use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use aivory_protocol::{Ask, HistoryPage, Line, SessionState, SessionTrace, UnixMs};

use crate::model::Provider;
use common::{ReadError, decide};

pub use claude::{subagent_path, subagents};

/// A session whose file changed less than this long ago, and whose turn did not end, runs.
pub const WORKING_WINDOW_MS: i64 = 90_000;
/// A session whose turn ended and whose file changed less than this long ago is idle.
/// An older session is parked.
pub const IDLE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// A transcript file cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not a valid transcript: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl TranscriptError {
    fn new(path: &Path, e: ReadError) -> Self {
        match e {
            ReadError::Io(source) => Self::Io {
                path: path.to_path_buf(),
                source,
            },
            ReadError::Json(source) => Self::Json {
                path: path.to_path_buf(),
                source,
            },
        }
    }
}

/// The state of a session, derived from the end of its transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub state: SessionState,
    /// The open question, for [`SessionState::NeedsYou`] and [`SessionState::Error`].
    pub ask: Option<Ask>,
    /// The time of the last entry.
    pub since: UnixMs,
}

/// Turn a transcript file into lines. The file is read once, line by line. A line that is not
/// valid JSON is skipped. A Gemini `.json` file is one JSON document: an invalid document is an
/// error.
pub fn normalize(provider: Provider, path: &Path) -> Result<Vec<Line>, TranscriptError> {
    let file = File::open(path).map_err(|e| TranscriptError::new(path, e.into()))?;
    let reader = BufReader::with_capacity(256 * 1024, file);
    let result = match provider {
        Provider::Claude => claude::normalize(reader).map_err(ReadError::from),
        Provider::Codex => codex::normalize(reader).map_err(ReadError::from),
        Provider::Gemini => gemini::normalize(reader, gemini::is_document(path)),
    };
    result.map_err(|e| TranscriptError::new(path, e))
}

/// The last `n` lines, for the pane of the war room.
pub fn tail(lines: &[Line], n: usize) -> &[Line] {
    &lines[lines.len().saturating_sub(n)..]
}

/// One page of the full history: lines `start .. start + count`, cut at the end of `lines`.
pub fn history(
    session: &str,
    subagent: Option<&str>,
    lines: &[Line],
    start: u32,
    count: u32,
) -> HistoryPage {
    let total = lines.len();
    let from = (start as usize).min(total);
    let to = from.saturating_add(count as usize).min(total);
    HistoryPage {
        session: session.to_string(),
        subagent: subagent.map(str::to_string),
        start: u32::try_from(from).unwrap_or(u32::MAX),
        total: u32::try_from(total).unwrap_or(u32::MAX),
        lines: lines[from..to].to_vec(),
    }
}

/// Derive the state of a session from the end of its transcript.
///
/// Rules, in this order:
/// - `e`: the last entry is an API error, or an error that ended the turn.
/// - `q`: a tool call has no result. A question tool gives `AskKind::Question`, a plan tool
///   `AskKind::Plan`, any other tool `AskKind::Permission`.
/// - `w`: the file changed less than [`WORKING_WINDOW_MS`] before `now`, and the turn did not
///   end.
/// - `i`: the turn ended, and the file changed less than [`IDLE_WINDOW_MS`] before `now`.
/// - `p`: everything else.
///
/// `since` is the time of the last entry; it is `mtime` when no entry has a time.
pub fn derive_state(
    provider: Provider,
    path: &Path,
    now: UnixMs,
    mtime: UnixMs,
) -> Result<Derived, TranscriptError> {
    let facts = match provider {
        Provider::Claude => claude::facts(path),
        Provider::Codex => codex::facts(path),
        Provider::Gemini => gemini::facts(path),
    }
    .map_err(|e| TranscriptError::new(path, e))?;
    Ok(decide(facts, now, mtime))
}

/// The span trace of a session: the waterfall and the DAG of the deep dive.
///
/// The session file is read once. A Claude session adds one read of each sub-agent file. A
/// line that is not valid JSON is skipped. A span that is still open has no `end` and the state
/// `working`. The duration of an open DAG node runs to `now`. The cost is `0` and the context
/// is `0 %`, because the crate has no price table. The DAG has no positions: the clients place
/// the cards.
pub fn trace(
    provider: Provider,
    path: &Path,
    session_id: &str,
    now: UnixMs,
) -> Result<SessionTrace, TranscriptError> {
    spans::trace(provider, path, session_id, now)
}
