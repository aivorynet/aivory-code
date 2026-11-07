//! Error type of the crate.

use std::io;

/// Result alias of the crate.
pub type Result<T> = std::result::Result<T, VcsError>;

/// Error of a VCS or ledger operation.
#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    /// The process or the file system failed.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// The VCS command exited with an error.
    #[error("`{command}` failed (exit {code:?}): {stderr}")]
    Command {
        /// The command line, without secrets.
        command: String,
        /// The exit code, or `None` when a signal ended the process.
        code: Option<i32>,
        /// The text on stderr, trimmed.
        stderr: String,
    },
    /// The ledger database failed.
    #[error("ledger database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The output of the VCS has an unexpected shape.
    #[error("cannot parse VCS output: {0}")]
    Parse(String),
    /// The caller gave a bad value.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A turn, a snapshot, or a ref does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The ledger schema is newer than this crate.
    #[error("ledger schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew {
        /// The version in the database.
        found: i64,
        /// The highest version this crate knows.
        supported: i64,
    },
}
