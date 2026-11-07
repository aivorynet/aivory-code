use std::path::{Path, PathBuf};

/// Root directories of the three native stores. Tests point these at synthetic fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRoots {
    /// Claude Code home, normally `~/.claude`. Sessions live in `<root>/projects`.
    pub claude: PathBuf,
    /// Codex home, normally `~/.codex`. Sessions live in `<root>/sessions`.
    pub codex: PathBuf,
    /// Gemini home, normally `~/.gemini`. Sessions live in `<root>/tmp/*/chats`.
    pub gemini: PathBuf,
}

impl StoreRoots {
    /// Defaults below a home directory: `.claude`, `.codex`, `.gemini`.
    pub fn from_home(home: &Path) -> Self {
        Self {
            claude: home.join(".claude"),
            codex: home.join(".codex"),
            gemini: home.join(".gemini"),
        }
    }

    /// Defaults for the current user, or `None` when the home directory is unknown.
    pub fn for_current_user() -> Option<Self> {
        std::env::home_dir().map(|h| Self::from_home(&h))
    }
}
