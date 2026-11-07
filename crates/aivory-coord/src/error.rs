//! The error type of the crate.

/// What can fail in the coordination layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The SQLite database failed.
    #[error("coordination database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A stored JSON value cannot be read or written.
    #[error("coordination json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The version control layer failed.
    #[error("version control error: {0}")]
    Vcs(#[from] aivory_vcs::VcsError),
    /// The symbol index failed.
    #[error("index error: {0}")]
    Index(String),
    /// The database schema is newer than this crate.
    #[error("coordination schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew {
        /// The version in the database.
        found: i64,
        /// The version that this crate writes.
        supported: i64,
    },
    /// The caller gave a bad value.
    #[error("invalid input: {0}")]
    Invalid(String),
}

/// Result of the coordination layer.
pub type Result<T> = std::result::Result<T, Error>;
