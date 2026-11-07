//! AIVory Code version control: the `Vcs` trait, the Git backend, the turn ledger, turn
//! snapshots, and blame by prompt (design `docs/design/05-vcs-ledger.md`).
//!
//! The daemon calls [`finish_turn`] at each turn end. A snapshot never changes the working
//! tree, the index, `HEAD`, or a branch of the user. It writes a hidden commit on
//! `refs/aivory/turns/<session>/<n>` through a temporary index file.
//!
//! Turn 0 of a session is the baseline: take a snapshot with `n = 0` at the session start.
//! Then turn 1 has a parent snapshot, and [`revert_turn`] has a state to return to.

mod error;
mod git;
mod ledger;
mod prompt;
mod types;
mod vcs;

pub use error::{Result, VcsError};
pub use git::GitVcs;
pub use ledger::{Ledger, LedgerOp, SCHEMA_VERSION, Turn, TurnFile, TurnTool};
pub use prompt::{
    PromptBlameLine, TimelineEvent, blame_by_prompt, file_timeline, finish_turn, revert_turn,
};
pub use types::{
    BASELINE_TURN, BlameLine, ChangeStatus, CommitInfo, FileChange, LogOptions, MergeMessage,
    MergeOutcome, PublishRequest, PublishedCommit, RefInfo, Snapshot, SnapshotRequest, StatusEntry,
    TRAILER_AGENTS, TRAILER_TASK, TRAILER_TURNS, TURN_REF_PREFIX, TurnRef, Workspace,
    WorkspaceRequest, trailer_value,
};
pub use vcs::Vcs;
