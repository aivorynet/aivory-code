//! Parameters and results of the action methods behind the controls of the desktop app:
//! `repos/add`, `repos/remove`, `project/new`, `map/addPart`, `providers/addAcp`,
//! `providers/removeAcp`, `settings/importClaude`, `daemon/restart`, and `daemon/scale`.
//!
//! The methods that write to the machine (`repos/*`, `project/new`, `map/addPart`,
//! `providers/*Acp`, `settings/importClaude`, `daemon/restart`) answer a paired phone with
//! `NOT_AVAILABLE`: only the local client may use them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::{ProviderInfo, Settings};

/// Parameters of `repos/add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoAddParams {
    /// A folder inside a git repository. The daemon registers the root of the repository.
    pub path: String,
}

/// A codebase that the user registered. The daemon scans it without a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PinnedRepo {
    /// The codebase id.
    pub id: String,
    /// The root of the repository.
    pub path: String,
}

/// The first files of `project/new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProjectTemplate {
    /// `README.md` and `.gitignore`.
    #[default]
    Empty,
    /// Plus `Cargo.toml` and `src/main.rs`.
    Rust,
    /// Plus `package.json` and `index.js`.
    Node,
    /// Plus `pyproject.toml` and `main.py`.
    Python,
}

/// Parameters of `project/new`: create `<parent>/<name>`, run `git init`, write the template
/// and `.aivory/map.toml`, and register the folder as a codebase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNewParams {
    /// The folder name, also the codebase id. No path separators.
    pub name: String,
    /// An existing folder.
    pub parent: String,
    /// Default: `empty`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<ProjectTemplate>,
}

/// Parameters of `map/addPart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddPartParams {
    pub codebase: String,
    pub name: String,
    /// Globs relative to the repository root, for example `crates/pay/**`.
    pub paths: Vec<String>,
}

/// An ACP agent that the user added in Settings. Stored with the settings (`Settings::acp`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgent {
    /// The provider id: `acp-` and the name in lower case.
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables of the agent process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Parameters of `providers/addAcp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddAcpParams {
    pub name: String,
    /// A program name on the PATH, or the path of a file.
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
}

/// Result of `providers/addAcp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcpAdded {
    /// The new card, with the badge `acp`.
    pub provider: ProviderInfo,
    /// `<command> --version` ran, or the file of the command exists.
    pub found: bool,
    /// The first line of the `--version` output, when the command printed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Parameters of `providers/removeAcp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRef {
    pub id: String,
}

/// One value that `settings/importClaude` changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportChange {
    /// The settings key, for example `providers.claude.model`.
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Value>,
    pub to: Value,
}

/// Result of `settings/importClaude`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// `~/.claude/settings.json` exists and holds JSON.
    pub found: bool,
    /// The values that changed. Empty when both sides agree.
    pub changes: Vec<ImportChange>,
    /// What the daemon did not import, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    pub settings: Settings,
}

/// Parameters of `daemon/restart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonRef {
    /// A daemon id of `settings/get`.
    pub daemon: String,
}

/// Parameters of `daemon/scale`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonScaleParams {
    pub daemon: String,
    /// 1-64. The daemon writes it to the autopilot value `maxRunning`.
    pub max_running: u32,
}
