//! Parse cache keyed by (path, mtime, size).

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{Parsed, SessionFile};

/// Bump when a parser change makes old cache entries wrong.
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Entry {
    pub path: PathBuf,
    pub mtime_ms: i64,
    pub size: u64,
    /// `None` records a file that is not a session, so that it is not parsed again.
    pub parsed: Option<Parsed>,
}

#[derive(Default)]
pub(crate) struct ScanCache {
    map: HashMap<PathBuf, Entry>,
}

impl ScanCache {
    pub fn load(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        match serde_json::from_slice::<CacheFile>(&bytes) {
            Ok(f) if f.version == CACHE_VERSION => Self {
                map: f.entries.into_iter().map(|e| (e.path.clone(), e)).collect(),
            },
            _ => Self::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// `Some(parsed)` on a hit. The inner option is the cached parse result.
    pub fn get(&self, file: &SessionFile) -> Option<&Option<Parsed>> {
        self.map
            .get(&file.path)
            .filter(|e| e.mtime_ms == file.mtime_ms && e.size == file.size)
            .map(|e| &e.parsed)
    }

    /// Write the cache atomically (temporary file, then rename). Errors are ignored: a cache is optional.
    pub fn save(path: &Path, entries: Vec<Entry>) {
        let file = CacheFile {
            version: CACHE_VERSION,
            entries,
        };
        let Ok(bytes) = serde_json::to_vec(&file) else {
            return;
        };
        if let Some(dir) = path.parent()
            && fs::create_dir_all(dir).is_err()
        {
            return;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let written = fs::File::create(&tmp).and_then(|mut f| f.write_all(&bytes));
        if written.is_err() || fs::rename(&tmp, path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

/// Default cache file: `$AIVORY_CACHE_DIR/sessions-cache.json`, else the per-user cache directory.
pub fn default_cache_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("sessions-cache.json"))
}

/// The cache folder: `$AIVORY_CACHE_DIR`, else the per-user cache folder of AIVory Code.
pub fn cache_dir() -> Option<PathBuf> {
    let dir = if let Some(d) = std::env::var_os("AIVORY_CACHE_DIR") {
        PathBuf::from(d)
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("aivory")
            .join("code")
    } else if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(x).join("aivory-code")
    } else if cfg!(target_os = "macos") {
        std::env::home_dir()?
            .join("Library")
            .join("Caches")
            .join("aivory-code")
    } else {
        std::env::home_dir()?.join(".cache").join("aivory-code")
    };
    Some(dir)
}
