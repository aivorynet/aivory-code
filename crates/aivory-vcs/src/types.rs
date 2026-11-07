//! Plain data types of the [`Vcs`](crate::Vcs) trait.

use std::fmt;
use std::path::PathBuf;

use crate::ledger::Turn;

/// Turn number of the baseline snapshot. The daemon takes it at the session start, before turn 1.
pub const BASELINE_TURN: u32 = 0;

/// Prefix of the snapshot refs. A snapshot lives at `refs/aivory/turns/<session>/<n>`.
pub const TURN_REF_PREFIX: &str = "refs/aivory/turns";

/// Name of the commit trailer that lists turns, for example `AIVory-Turns: S-88/7,S-88/8`.
pub const TRAILER_TURNS: &str = "AIVory-Turns";
/// Name of the commit trailer with the task id.
pub const TRAILER_TASK: &str = "AIVory-Task";
/// Name of the commit trailer with the agent list.
pub const TRAILER_AGENTS: &str = "AIVory-Agents";

/// A reference to one turn: session id and turn number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TurnRef {
    /// Session id.
    pub session: String,
    /// Turn number.
    pub n: u32,
}

impl TurnRef {
    /// Parse `S-88/7`. The last `/` separates the number.
    pub fn parse(s: &str) -> Option<Self> {
        let (session, n) = s.trim().rsplit_once('/')?;
        if session.is_empty() {
            return None;
        }
        Some(Self {
            session: session.to_owned(),
            n: n.trim().parse().ok()?,
        })
    }

    /// Parse a comma-separated list such as `S-88/7,S-88/8`. The parser skips bad items.
    pub fn parse_list(s: &str) -> Vec<Self> {
        s.split(',').filter_map(Self::parse).collect()
    }
}

impl fmt::Display for TurnRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.session, self.n)
    }
}

/// Kind of a file change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeStatus {
    /// The file exists only after the change.
    Added,
    /// The content or the mode changed.
    Modified,
    /// The file exists only before the change.
    Deleted,
    /// The type changed (file, symlink, submodule).
    TypeChanged,
}

/// One changed file of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the repository root, with `/` separators.
    pub path: String,
    /// Kind of change.
    pub status: ChangeStatus,
    /// Added lines (0 for a binary file).
    pub added: u64,
    /// Removed lines (0 for a binary file).
    pub removed: u64,
    /// True for a binary file.
    pub binary: bool,
    /// Blob id after the change. `None` for a deleted file.
    pub blob_after: Option<String>,
}

/// One line of the working tree status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Status letter of the index (`' '`, `M`, `A`, `D`, `R`, `?`, ...).
    pub index: char,
    /// Status letter of the working tree.
    pub worktree: char,
    /// Path relative to the repository root.
    pub path: String,
    /// Old path of a rename or a copy.
    pub orig_path: Option<String>,
}

/// Options of [`Vcs::log`](crate::Vcs::log).
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    /// Start revision. `None` means `HEAD`.
    pub rev: Option<String>,
    /// Limit the log to commits that touch this path.
    pub path: Option<String>,
    /// Maximum number of commits.
    pub max_count: Option<usize>,
}

/// A commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    /// Commit id.
    pub id: String,
    /// Parent ids.
    pub parents: Vec<String>,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Commit time (Unix seconds).
    pub time: i64,
    /// First line of the message.
    pub subject: String,
    /// Rest of the message.
    pub body: String,
    /// Turns that the `AIVory-Turns` trailer names.
    pub turns: Vec<TurnRef>,
}

/// A ref and the object id that it points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefInfo {
    /// Full ref name, for example `refs/heads/main`.
    pub name: String,
    /// Object id.
    pub id: String,
}

/// One line of a blame result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// Line number in the blamed version (1-based).
    pub line: u32,
    /// Line number in the commit that last changed the line (1-based).
    pub orig_line: u32,
    /// Commit id. All zeros for a line that is not committed.
    pub commit: String,
    /// True when the line is not committed.
    pub uncommitted: bool,
    /// Path of the file in that commit (differs after a rename).
    pub orig_path: String,
    /// Author time of the commit (Unix seconds).
    pub time: i64,
    /// Summary line of the commit.
    pub summary: String,
    /// Content of the line, without the line break.
    pub content: String,
}

/// Request for a new workspace (a Git worktree).
#[derive(Debug, Clone)]
pub struct WorkspaceRequest {
    /// Folder that holds workspaces. The workspace goes to `<root>/<name>`.
    pub root: PathBuf,
    /// Folder name of the workspace.
    pub name: String,
    /// Branch to check out, or to create when `create_branch` is true. `None` detaches `HEAD`.
    pub branch: Option<String>,
    /// Create `branch` at `start_point`.
    pub create_branch: bool,
    /// Revision to start from. `None` means `HEAD`.
    pub start_point: Option<String>,
    /// Directories for a sparse checkout (cone mode). Empty means a full checkout.
    pub sparse: Vec<String>,
}

/// A workspace that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// Folder of the workspace.
    pub path: PathBuf,
    /// Checked out branch (short name), `None` when detached.
    pub branch: Option<String>,
    /// Commit id of `HEAD`.
    pub head: String,
}

/// Request for a turn snapshot.
#[derive(Debug, Clone, Default)]
pub struct SnapshotRequest {
    /// Session id.
    pub session: String,
    /// Turn number. Use [`BASELINE_TURN`] for the baseline.
    pub n: u32,
    /// Provider, for the commit trailer.
    pub provider: String,
    /// Model, for the commit trailer.
    pub model: String,
    /// Commit time (Unix seconds). `None` means now.
    pub time: Option<i64>,
    /// Parent snapshot. `None` means the latest snapshot of the session with a lower number.
    pub parent: Option<String>,
}

/// A turn snapshot: a hidden commit on `refs/aivory/turns/<session>/<n>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Session id.
    pub session: String,
    /// Turn number.
    pub n: u32,
    /// Commit id.
    pub commit: String,
    /// Tree id.
    pub tree: String,
    /// Parent snapshot commit. `None` for the first snapshot of a session.
    pub parent: Option<String>,
    /// Full ref name.
    pub ref_name: String,
}

/// One message of a merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeMessage {
    /// Paths that the message names.
    pub paths: Vec<String>,
    /// Message kind, for example `CONFLICT (contents)`.
    pub kind: String,
    /// Message text.
    pub text: String,
}

/// Result of a trial merge. The trial merge never changes a working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// True when the merge has no conflict.
    pub clean: bool,
    /// Tree of the merge result. With conflicts, the tree holds the conflict markers.
    pub tree: String,
    /// Conflicted paths.
    pub conflicts: Vec<String>,
    /// Messages of the merge.
    pub messages: Vec<MergeMessage>,
}

/// Request to publish turns as a normal commit.
#[derive(Debug, Clone, Default)]
pub struct PublishRequest {
    /// Branch (short name) that receives the commit. A missing branch starts at `base`.
    pub branch: String,
    /// Start point for a new branch. `None` means `HEAD`.
    pub base: Option<String>,
    /// Subject line of the commit.
    pub subject: String,
    /// Message body, without trailers.
    pub body: Option<String>,
    /// Task id for the `AIVory-Task` trailer.
    pub task_id: Option<String>,
    /// Turns that the commit contains, oldest first. The commit tree is the snapshot of the last
    /// turn that has one.
    pub turns: Vec<Turn>,
    /// Author name and email. `None` uses the Git config.
    pub author: Option<(String, String)>,
    /// Checkout folder. When it has `branch` checked out, its index follows the new commit.
    /// Its files stay as they are.
    pub sync_checkout: Option<PathBuf>,
    /// Also write a note with the turn list on `refs/notes/aivory`.
    pub notes: bool,
}

/// A commit that [`Vcs::publish_turns`](crate::Vcs::publish_turns) built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedCommit {
    /// Commit id.
    pub commit: String,
    /// Tree id.
    pub tree: String,
    /// Parent commit.
    pub parent: Option<String>,
    /// Full ref name of the branch.
    pub ref_name: String,
    /// Full commit message.
    pub message: String,
}

/// Parse the value of the first trailer `key` in a commit message.
pub fn trailer_value<'a>(message: &'a str, key: &str) -> Option<&'a str> {
    message.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
    })
}
