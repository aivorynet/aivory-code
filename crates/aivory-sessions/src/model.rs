use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A supported agent CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Claude, Provider::Codex, Provider::Gemini];

    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
            Provider::Gemini => "gemini",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "claude" => Ok(Provider::Claude),
            "codex" => Ok(Provider::Codex),
            "gemini" => Ok(Provider::Gemini),
            other => Err(format!(
                "unknown provider '{other}' (expected claude, codex, or gemini)"
            )),
        }
    }
}

/// One native session. Times are Unix epoch milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub provider: Provider,
    pub id: String,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub created: Option<i64>,
    /// Last modification time of the session file.
    pub updated: i64,
    pub git_branch: Option<String>,
    pub cli_version: Option<String>,
    pub model: Option<String>,
    /// The last permission mode that the transcript names, in the words of the CLI: Claude
    /// `default`, `plan`, `acceptEdits`, `auto`, `bypassPermissions`; Codex
    /// `<approval_policy>/<sandbox type>`, for example `on-request/workspace-write`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The last effort level that the transcript names (`low` … `max`, Codex `xhigh`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The main session file.
    pub path: PathBuf,
    /// Sidechain (sub-agent) files that belong to this session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidechains: Vec<PathBuf>,
}

/// Filter for [`crate::SessionCatalog::list`].
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub provider: Option<Provider>,
    /// Keep sessions whose cwd equals this path or lies below it.
    pub cwd: Option<String>,
    pub limit: Option<usize>,
}

impl SessionFilter {
    pub fn matches(&self, s: &SessionSummary) -> bool {
        if self.provider.is_some_and(|p| p != s.provider) {
            return false;
        }
        if let Some(want) = &self.cwd {
            let Some(have) = &s.cwd else { return false };
            if !path_is_within(&normalize_path(have), &normalize_path(want)) {
                return false;
            }
        }
        true
    }
}

/// Normalise a path string for comparison: forward slashes, no trailing slash,
/// lower case on Windows.
pub(crate) fn normalize_path(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    s
}

fn path_is_within(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/') || base.ends_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_matches_prefix_on_boundary_only() {
        assert!(path_is_within("c:/a/b", "c:/a"));
        assert!(path_is_within("c:/a", "c:/a"));
        assert!(!path_is_within("c:/ab", "c:/a"));
    }

    #[test]
    fn provider_round_trips() {
        for p in Provider::ALL {
            assert_eq!(p.as_str().parse::<Provider>().unwrap(), p);
        }
        assert!("other".parse::<Provider>().is_err());
    }
}
