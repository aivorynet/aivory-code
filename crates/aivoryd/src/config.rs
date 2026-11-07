use std::path::PathBuf;

use aivory_sessions::{SessionCatalog, StoreRoots, default_cache_file};

/// Daemon settings. Defaults come from the home directory of the current user.
#[derive(Debug, Clone)]
pub struct Config {
    pub roots: StoreRoots,
    /// Where the scan cache lives. `None` disables the cache.
    pub cache_file: Option<PathBuf>,
}

impl Config {
    /// Defaults for the current user.
    pub fn from_environment() -> Result<Self, String> {
        let roots = StoreRoots::for_current_user().ok_or("cannot find the home directory")?;
        Ok(Self {
            roots,
            cache_file: default_cache_file(),
        })
    }

    pub fn catalog(&self) -> SessionCatalog {
        SessionCatalog::new(&self.roots).with_cache_file(self.cache_file.clone())
    }
}
