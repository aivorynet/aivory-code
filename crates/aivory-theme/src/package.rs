//! The manifest (`theme.toml`), the scan of a theme folder, and the file hashes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use aivory_protocol::theme::{CATALOG_VERSION, DisplayClass, ThemeManifest};
use serde_json::Value;

use crate::archive::{hex, sha256_hex};
use crate::{Issue, caps, paths};

/// A SemVer version. The order follows SemVer; a pre-release is lower than its release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Semver {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Semver {
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = || format!("\"{text}\" is not a SemVer version (x.y.z)");
        let core = text.split('+').next().unwrap_or(text);
        let (nums, pre) = match core.split_once('-') {
            Some((n, p)) => (n, Some(p.to_string())),
            None => (core, None),
        };
        if pre.as_deref() == Some("") {
            return Err(bad());
        }
        let mut it = nums.split('.');
        let mut next = || -> Result<u64, String> {
            let p = it.next().ok_or_else(bad)?;
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            p.parse().map_err(|_| bad())
        };
        let (major, minor, patch) = (next()?, next()?, next()?);
        if it.next().is_some() {
            return Err(bad());
        }
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl PartialOrd for Semver {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Semver {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let core =
            (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if core != Equal {
            return core;
        }
        match (&self.pre, &other.pre) {
            (None, None) => Equal,
            (None, Some(_)) => Greater,
            (Some(_), None) => Less,
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}

/// The version of this build, for the `min_app` rule.
pub fn app_version() -> Semver {
    Semver::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Semver {
        major: 0,
        minor: 0,
        patch: 0,
        pre: None,
    })
}

/// Parse `theme.toml`.
pub fn parse_manifest(text: &str) -> Result<ThemeManifest, String> {
    toml::from_str(text).map_err(|e| e.to_string().replace('\n', " "))
}

/// The rules of the manifest that a schema cannot check. `tag` is the Git tag of the release.
pub fn check_manifest(m: &ThemeManifest, tag: Option<&str>, app: &Semver) -> Vec<Issue> {
    let file = "theme.toml";
    let mut out = Vec::new();
    let mut bad = |ptr: &str, rule: String| out.push(Issue::new(file, ptr, rule));
    let id_ok = (3..=40).contains(&m.id.len())
        && m.id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !m.id.starts_with('-')
        && !m.id.ends_with('-');
    if !id_ok {
        bad(
            "/id",
            format!("id \"{}\" must be 3 to 40 characters of [a-z0-9-]", m.id),
        );
    }
    if m.name.trim().is_empty() || m.name.chars().count() > 80 {
        bad("/name", "name must have 1 to 80 characters".to_string());
    }
    if m.description.chars().count() > 500 {
        bad(
            "/description",
            "description is longer than 500 characters".to_string(),
        );
    }
    if m.author.name.trim().is_empty() || m.author.name.chars().count() > 80 {
        bad(
            "/author/name",
            "author name must have 1 to 80 characters".to_string(),
        );
    }
    if let Some(url) = &m.author.url
        && !url.starts_with("https://")
    {
        bad(
            "/author/url",
            "the author URL must start with https://".to_string(),
        );
    }
    let license_ok = !m.license.trim().is_empty()
        && m.license.len() <= 120
        && m.license
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | ' ' | '(' | ')'));
    if !license_ok {
        bad("/license", "license must be an SPDX expression".to_string());
    }
    match Semver::parse(&m.version) {
        Err(e) => bad("/version", e),
        Ok(_) => {
            if let Some(tag) = tag {
                let t = tag.strip_prefix('v').unwrap_or(tag);
                if t != m.version {
                    bad(
                        "/version",
                        format!(
                            "version \"{}\" must equal the tag \"{tag}\" without the v",
                            m.version
                        ),
                    );
                }
            }
        }
    }
    match Semver::parse(&m.min_app) {
        Err(e) => bad("/min_app", e),
        Ok(min) if min > *app => bad(
            "/min_app",
            format!("the theme needs app {} or newer", m.min_app),
        ),
        Ok(_) => {}
    }
    if m.catalog == 0 || m.catalog > CATALOG_VERSION {
        bad(
            "/catalog",
            format!(
                "catalog {} is not supported (this app has {CATALOG_VERSION})",
                m.catalog
            ),
        );
    }
    if !m.displays.contains(&DisplayClass::Desktop) {
        bad("/displays", "displays must include \"desktop\"".to_string());
    }
    let mut seen = std::collections::BTreeSet::new();
    if !m.displays.iter().all(|d| seen.insert(*d)) {
        bad("/displays", "displays has a duplicate".to_string());
    }
    if m.tokens.is_empty() || m.tokens.len() > 8 {
        bad("/tokens", "tokens must list 1 to 8 files".to_string());
    }
    for (i, t) in m.tokens.iter().enumerate() {
        let ok = paths::check_relative(t).is_ok()
            && !paths::is_hidden(t)
            && t.ends_with(".toml")
            && t != "theme.toml";
        if !ok {
            bad(
                &format!("/tokens/{i}"),
                format!("\"{t}\" is not a valid token file"),
            );
        }
    }
    out
}

/// One file of a theme folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    /// Relative path with `/`.
    pub rel: String,
    pub abs: PathBuf,
    pub size: u64,
}

/// List the files of a theme folder. A link, a special file, a path that breaks the path rules,
/// more than 500 files and more than 20 MB are errors. Hidden files are left out.
pub fn scan(dir: &Path) -> Result<Vec<ScannedFile>, Vec<Issue>> {
    let mut files = Vec::new();
    let mut issues = Vec::new();
    walk(dir, "", &mut files, &mut issues, 0);
    let total: u64 = files.iter().map(|f| f.size).sum();
    if files.len() > caps::FILES {
        issues.push(Issue::new(
            ".",
            "",
            format!("more than {} files", caps::FILES),
        ));
    }
    if total > caps::UNPACKED_BYTES {
        issues.push(Issue::new(
            ".",
            "",
            format!("more than {} bytes", caps::UNPACKED_BYTES),
        ));
    }
    if issues.is_empty() {
        files.sort_by(|a, b| a.rel.cmp(&b.rel));
        Ok(files)
    } else {
        Err(issues)
    }
}

fn is_reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

fn walk(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<ScannedFile>,
    issues: &mut Vec<Issue>,
    depth: usize,
) {
    if depth > 12 || files.len() > caps::FILES + 1 {
        issues.push(Issue::new(
            if prefix.is_empty() { "." } else { prefix },
            "",
            "folder is too deep or has too many files",
        ));
        return;
    }
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            issues.push(Issue::new(
                prefix_or_dot(prefix),
                "",
                format!("cannot read the folder: {e}"),
            ));
            return;
        }
    };
    for ent in rd.flatten() {
        let Some(name) = ent.file_name().to_str().map(str::to_string) else {
            issues.push(Issue::new(
                prefix_or_dot(prefix),
                "",
                "file name is not UTF-8",
            ));
            continue;
        };
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(ent.path()) else {
            issues.push(Issue::new(&rel, "", "cannot read the file"));
            continue;
        };
        if meta.file_type().is_symlink() || is_reparse(&meta) {
            issues.push(Issue::new(&rel, "", "links are not allowed"));
        } else if meta.is_dir() {
            if let Err(m) = paths::check_relative(&rel) {
                issues.push(Issue::new(&rel, "", m));
            } else {
                walk(&ent.path(), &rel, files, issues, depth + 1);
            }
        } else if meta.is_file() {
            if let Err(m) = paths::check_relative(&rel) {
                issues.push(Issue::new(&rel, "", m));
            } else {
                files.push(ScannedFile {
                    rel,
                    abs: ent.path(),
                    size: meta.len(),
                });
            }
        } else {
            issues.push(Issue::new(&rel, "", "special files are not allowed"));
        }
    }
}

fn prefix_or_dot(p: &str) -> &str {
    if p.is_empty() { "." } else { p }
}

/// The SHA-256 of each package file of a folder. `theme.lock` and `.upstream` files are not
/// package files.
pub fn hash_files(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let files = scan(dir).map_err(|i| {
        i.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let mut out = BTreeMap::new();
    for f in files {
        if f.rel == "theme.lock" || f.rel.ends_with(".upstream") {
            continue;
        }
        let bytes = fs::read(&f.abs).map_err(|e| format!("{}: {e}", f.rel))?;
        out.insert(f.rel, sha256_hex(&bytes));
    }
    Ok(out)
}

/// The SHA-256 of a file in lowercase hex.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 32 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// A theme that passed every check.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub manifest: ThemeManifest,
    /// The merged tokens as JSON.
    pub tokens: Value,
    /// `layouts[class][screen]`: the `root` node of each layout file.
    pub layouts: BTreeMap<DisplayClass, BTreeMap<String, Value>>,
    /// `parts/<name>.json`: the `root` node of each part.
    pub parts: BTreeMap<String, Value>,
    /// Languages with a strings table.
    pub languages: Vec<String>,
    pub files: usize,
    pub bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_order() {
        let p = |s: &str| Semver::parse(s).unwrap();
        assert!(p("1.2.3") < p("1.10.0"));
        assert!(p("1.0.0-rc1") < p("1.0.0"));
        assert!(Semver::parse("1.0").is_err());
        assert!(Semver::parse("v1.0.0").is_err());
        assert!(Semver::parse("1.0.0.0").is_err());
    }
}
