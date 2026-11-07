//! The themes folder: `themes/` next to `aivory.db` in the per-user data folder (`AIVORY_CACHE_DIR`
//! moves both). A staged download lives in `themes/.staging/` until `theme/add` renames it.

use std::fs;
use std::path::{Path, PathBuf};

/// The id rule of the manifest: `[a-z0-9-]`, 3 to 40 characters.
pub fn valid_id(id: &str) -> bool {
    (3..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `themes/` in the folder of the scan cache (the data folder of `aivory.db`). Without a
    /// cache file (tests) it is a scratch folder of this process, never the real data folder.
    pub fn for_cache_file(cache_file: Option<&Path>) -> Self {
        match cache_file.and_then(Path::parent) {
            Some(dir) => Self::new(dir.join("themes")),
            None => Self::new(
                std::env::temp_dir().join(format!("aivoryd-themes-{}", std::process::id())),
            ),
        }
    }

    /// The themes folder of the user: next to the cache file, or in the default data folder.
    pub fn for_user(cache_file: Option<&Path>) -> Self {
        let dir = cache_file
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| {
                aivory_sessions::default_cache_file()
                    .and_then(|f| f.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| std::env::temp_dir().join("aivory-code"));
        Self::new(dir.join("themes"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn staging(&self) -> PathBuf {
        self.root.join(".staging")
    }

    pub fn registries_file(&self) -> PathBuf {
        self.root.join("registries.json")
    }

    pub fn dir(&self, id: &str) -> Result<PathBuf, String> {
        if valid_id(id) {
            Ok(self.root.join(id))
        } else {
            Err(format!("\"{id}\" is not a theme id"))
        }
    }

    pub fn exists(&self, id: &str) -> bool {
        self.dir(id).is_ok_and(|d| d.join("theme.toml").is_file())
    }

    /// The ids of the installed themes, sorted.
    pub fn ids(&self) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| valid_id(n))
            .collect();
        out.sort();
        out
    }

    /// A new empty folder in the staging area.
    pub fn new_staging(&self, name: &str) -> Result<PathBuf, String> {
        let dir = self.staging().join(name);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(dir)
    }

    /// Remove the staging area (leftovers of a crash).
    pub fn clean_staging(&self) {
        let _ = fs::remove_dir_all(self.staging());
    }

    /// Move a staged folder to `themes/<id>` with one rename. An installed theme is an error.
    pub fn commit(&self, staged: &Path, id: &str) -> Result<PathBuf, String> {
        let target = self.dir(id)?;
        if target.exists() {
            return Err(format!(
                "theme \"{id}\" is installed already; use theme update"
            ));
        }
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        fs::rename(staged, &target).map_err(|e| format!("cannot install the theme: {e}"))?;
        Ok(target)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let dir = self.dir(id)?;
        if !dir.join("theme.toml").is_file() {
            return Err(format!("theme \"{id}\" is not installed"));
        }
        fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove the theme: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_checked_before_a_path_is_built() {
        let s = Store::new(PathBuf::from("themes"));
        for bad in ["..", "../x", "a/b", "AB", "ab", "-abc", r"a\bc", "con:x"] {
            assert!(s.dir(bad).is_err(), "{bad}");
        }
        assert!(s.dir("my-theme").is_ok());
    }
}
