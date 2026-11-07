//! The [`Vcs`] trait: one interface for every version control system.
//!
//! Every method takes the repository (or workspace) folder as the first argument, so a backend
//! value is cheap and has no state. The unit of a snapshot is a *changeset*: a tree with a
//! parent, like in Scion. Mercurial, Jujutsu, and Scion backends are later work. They need
//! only this trait.

use std::fmt::Debug;
use std::path::Path;

use crate::error::{Result, VcsError};
use crate::types::{
    BlameLine, CommitInfo, FileChange, LogOptions, MergeOutcome, PublishRequest, PublishedCommit,
    RefInfo, Snapshot, SnapshotRequest, StatusEntry, Workspace, WorkspaceRequest,
};

/// Version control operations that the daemon needs.
///
/// A backend must keep one promise: [`Vcs::snapshot`], [`Vcs::diff_workdir`], and every
/// *Read* operation never change the working tree, the index, `HEAD`, or a branch of the user.
pub trait Vcs: Send + Sync + Debug {
    /// Short backend name, for example `git`.
    fn name(&self) -> &'static str;

    // ---- Read ----------------------------------------------------------------------------

    /// Resolve a revision to a commit id.
    fn resolve(&self, repo: &Path, rev: &str) -> Result<String>;

    /// Status of the working tree (staged, unstaged, and untracked files).
    fn status(&self, repo: &Path) -> Result<Vec<StatusEntry>>;

    /// Changed files between two revisions. `from = None` means the empty tree.
    fn diff_trees(&self, repo: &Path, from: Option<&str>, to: &str) -> Result<Vec<FileChange>>;

    /// Changed files between a revision and the working tree. The working tree side includes
    /// untracked files that the ignore rules allow.
    fn diff_workdir(&self, workdir: &Path, from: &str) -> Result<Vec<FileChange>>;

    /// Unified diff text between two revisions, for the whole tree or for one path.
    fn diff_patch(
        &self,
        repo: &Path,
        from: Option<&str>,
        to: &str,
        path: Option<&str>,
    ) -> Result<String>;

    /// Commit log, newest first.
    fn log(&self, repo: &Path, opts: &LogOptions) -> Result<Vec<CommitInfo>>;

    /// Blame of a file. `rev = None` blames the working tree. `range` is 1-based and inclusive.
    fn blame(
        &self,
        repo: &Path,
        rev: Option<&str>,
        path: &str,
        range: Option<(u32, u32)>,
    ) -> Result<Vec<BlameLine>>;

    /// Blame `contents` (a version of `path`) as a change on top of `base_rev`. Lines that equal
    /// the history of `base_rev` get their commit. Lines that differ are uncommitted. This
    /// aligns the lines with the history exactly, with no guess.
    fn blame_contents(
        &self,
        repo: &Path,
        base_rev: &str,
        path: &str,
        contents: &[u8],
    ) -> Result<Vec<BlameLine>>;

    /// Content of a file at a revision.
    fn show_file(&self, repo: &Path, rev: &str, path: &str) -> Result<Vec<u8>>;

    /// Refs, optionally limited to a prefix such as `refs/heads/`.
    fn refs(&self, repo: &Path, prefix: Option<&str>) -> Result<Vec<RefInfo>>;

    // ---- Workspaces ------------------------------------------------------------------------

    /// Create a workspace (for Git, a linked worktree) under the folder that the caller gives.
    fn create_workspace(&self, repo: &Path, req: &WorkspaceRequest) -> Result<Workspace>;

    /// Remove a workspace. `force` removes it also when it has changes.
    fn remove_workspace(&self, repo: &Path, workspace: &Path, force: bool) -> Result<()>;

    // ---- Changesets ------------------------------------------------------------------------

    /// Write the working tree as a hidden changeset. Returns `None` when the tree equals the
    /// previous snapshot of the session (nothing to record).
    fn snapshot(&self, workdir: &Path, req: &SnapshotRequest) -> Result<Option<Snapshot>>;

    /// Make the working tree equal to a changeset. This changes the folder: the caller decides
    /// when. Files that the ignore rules hide stay as they are.
    fn restore(&self, workdir: &Path, rev: &str) -> Result<()>;

    /// Snapshots of a session, ordered by turn number.
    fn list_snapshots(&self, repo: &Path, session: &str) -> Result<Vec<Snapshot>>;

    /// The latest snapshot of a session with a turn number below `n`.
    fn snapshot_before(&self, repo: &Path, session: &str, n: u32) -> Result<Option<Snapshot>> {
        Ok(self
            .list_snapshots(repo, session)?
            .into_iter()
            .rev()
            .find(|s| s.n < n))
    }

    // ---- Merge -----------------------------------------------------------------------------

    /// Merge two revisions in memory. No working tree changes.
    fn trial_merge(&self, repo: &Path, ours: &str, theirs: &str) -> Result<MergeOutcome>;

    // ---- Remote ----------------------------------------------------------------------------

    /// Fetch from a remote.
    fn fetch(&self, repo: &Path, remote: &str) -> Result<()>;

    /// Push one ref spec, for example `refs/heads/feature:refs/heads/feature`.
    fn push_ref(&self, repo: &Path, remote: &str, refspec: &str) -> Result<()>;

    /// List the refs of a remote.
    fn list_remote_refs(&self, repo: &Path, remote: &str) -> Result<Vec<RefInfo>>;

    // ---- Publish ---------------------------------------------------------------------------

    /// Build a normal commit on a branch from turns. It does not push.
    /// Returns `None` when the branch already has the content.
    fn publish_turns(&self, repo: &Path, req: &PublishRequest) -> Result<Option<PublishedCommit>>;
}

/// Reject a revision, ref, or path argument that a command line could read as an option.
pub(crate) fn check_arg(kind: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('-') || value.contains(['\0', '\n']) {
        return Err(VcsError::Invalid(format!("bad {kind} `{value}`")));
    }
    Ok(())
}
