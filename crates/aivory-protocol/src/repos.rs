//! The repositories screen (`Repos.dc.html`) and `.aivory/map.toml`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{SessionState, UnixMs};
use crate::warroom::{Codebase, Part};

/// A row of the parts table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PartStats {
    pub name: String,
    pub paths: Vec<String>,
    /// Live sessions in this part.
    pub sessions: Vec<String>,
    /// Files that live sessions claim.
    pub claims: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change: Option<UnixMs>,
    pub lines: u32,
}

/// A row of the file tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    /// Relative to the repository root. Directories end with `/`.
    pub path: String,
    pub name: String,
    pub dir: bool,
    pub depth: u8,
    /// The part tag of a part root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// The session that claims the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    /// The state of that session (an orange pill for needs you).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_state: Option<SessionState>,
}

/// A row of the activity tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    pub at: UnixMs,
    pub provider: String,
    pub session: String,
    pub what: String,
    pub added: u32,
    pub removed: u32,
    /// USD.
    pub cost: f64,
}

/// `.aivory/map.toml`: `version`, `updated`, `[[part]]` blocks with `name` and `paths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MapFile {
    pub codebase: String,
    pub version: u32,
    pub updated: UnixMs,
    /// The agent that wrote the last version, for example `claude opus-5.5`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub parts: Vec<Part>,
    /// The file text, for "View map.toml".
    pub source: String,
}

/// Result of `repos/get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoDetail {
    pub codebase: Codebase,
    pub parts: Vec<PartStats>,
    pub files: Vec<FileNode>,
    /// Newest first.
    pub activity: Vec<ActivityRow>,
}

/// Result of `repos/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoList {
    pub codebases: Vec<Codebase>,
}
