//! The error type of the crate.

use std::path::PathBuf;

/// What can go wrong in the diagnostics layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rule error in {file}: {message}")]
    Rule { file: String, message: String },
    #[error("no language server is known for {0}")]
    NoServer(String),
    #[error("language server `{0}` is not installed")]
    ServerNotFound(String),
    #[error("language server failed: {0}")]
    Server(String),
    #[error("language server request `{method}` failed: {message}")]
    Request { method: String, message: String },
    #[error("language server request `{0}` timed out")]
    Timeout(String),
    #[error("language server has no support for `{0}`")]
    Unsupported(String),
    #[error("not a file path: {0}")]
    BadPath(PathBuf),
}

/// The result type of the crate.
pub type Result<T> = std::result::Result<T, Error>;
