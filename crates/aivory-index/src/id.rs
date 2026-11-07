//! The identity of file content.

use serde::{Deserialize, Serialize};

/// What names the content of a file. A committed file has its git blob id. Any other file has
/// the blake3 hash of its content. Equal ids mean equal content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ContentId {
    /// A git blob id (40 or 64 hex characters).
    Git(String),
    /// The blake3 hash of the content (64 hex characters).
    Blake3(String),
}

impl ContentId {
    /// The blake3 id of a byte string.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self::Blake3(blake3::hash(bytes).to_hex().to_string())
    }

    /// The hex part of the id.
    pub fn hex(&self) -> &str {
        match self {
            Self::Git(h) | Self::Blake3(h) => h,
        }
    }

    /// A file-name safe key, for example `git-3b18e5...`.
    pub fn key(&self) -> String {
        match self {
            Self::Git(h) => format!("git-{h}"),
            Self::Blake3(h) => format!("b3-{h}"),
        }
    }
}
