//! The code service of the daemon: code intelligence (`code/*`), the turn ledger (`ledger/*`),
//! and the read-only code tools of the agents (`references`, `definition`, `symbols`,
//! `outline`, `diagnostics`).
//!
//! - [`CodeCore`] is the shared state of one data folder: one [`aivory_index::Index`] for each
//!   codebase (opened on the first query, refreshed in a background thread), one
//!   [`aivory_diag::lsp::LspPool`] (stopped when idle by a timer of 30 seconds), and the worker
//!   that handles the end of a turn.
//! - At the end of a turn the ledger worker ([`crate::ledger`]) calls [`turn_ended`]. The core
//!   then updates the session delta of the index and runs the diagnostics pipeline on the files
//!   of the turn. The findings before and after the turn go to the tables `diag_run` and
//!   `diag_finding` of `aivory.db`.
//! - [`CodeService`] answers the methods. [`CodeService::demo`] answers them from the
//!   synthetic files of [`aivory_protocol::demo`].
//! - [`CodeTools`] implements the MCP tools for the agents on the same core.
//!
//! The service never writes to a working tree of the user. `ledger/revert` restores a snapshot
//! only into a linked worktree (a folder whose `.git` is a file).

mod demo;
mod diag;
mod resolve;
mod service;
mod state;
mod tools;

pub use resolve::{LiveResolver, Resolver, SessionCtx, StaticResolver, repo_root};
pub use service::CodeService;
pub use state::{CodeCore, TurnEnd, core_for, turn_ended};
pub use tools::CodeTools;

/// True for the methods that this service answers.
pub fn handles(method: &str) -> bool {
    method.starts_with("code/") || method.starts_with("ledger/")
}
