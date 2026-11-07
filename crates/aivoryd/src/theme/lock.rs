//! `theme.lock` (ADR 0024 section 8): the source, the version, the SHA-256 of the archive and
//! the SHA-256 of each upstream file. The same version with another archive hash is an error
//! (a moved tag).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "theme.lock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    /// `github:<owner>/<repo>`.
    pub source: String,
    pub version: String,
    /// The Git tag that gave this version.
    #[serde(default)]
    pub tag: String,
    pub archive_sha256: String,
    /// The upstream hash of each package file.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

impl Lock {
    pub fn read(dir: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(dir.join(FILE)).map_err(|e| format!("cannot read {FILE}: {e}"))?;
        toml::from_str(&text).map_err(|e| format!("{FILE} is not valid: {e}"))
    }

    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(dir.join(FILE), text).map_err(|e| format!("cannot write {FILE}: {e}"))
    }

    /// The files that differ from the upstream hash, or that the user deleted.
    pub fn edited(&self, dir: &Path) -> Result<Vec<String>, String> {
        let now = aivory_theme::package::hash_files(dir)?;
        let mut out: Vec<String> = self
            .files
            .iter()
            .filter(|(p, h)| now.get(*p) != Some(*h))
            .map(|(p, _)| p.clone())
            .collect();
        // A file that the user added is not an edit of an upstream file.
        out.sort();
        Ok(out)
    }

    /// The moved-tag rule: a version that is locked already must have the same archive hash.
    pub fn check_same_version(&self, version: &str, archive_sha256: &str) -> Result<(), String> {
        if self.version == version && self.archive_sha256 != archive_sha256 {
            return Err(format!(
                "moved tag: version {version} was added with archive hash {} and the source now gives {archive_sha256}; the theme is not changed",
                self.archive_sha256
            ));
        }
        Ok(())
    }
}
