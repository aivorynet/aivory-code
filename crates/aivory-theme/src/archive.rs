//! Safe unpacking of a theme archive (ADR 0024 section 8).
//!
//! The unpacker never calls `ZipArchive::extract`. It reads each entry itself and writes the
//! file itself. It does not trust any size in the headers: it counts the bytes that it inflates
//! and stops at the caps.
//!
//! Rejected: links and special files, absolute paths, `..` paths, backslashes, Windows reserved
//! names, names that differ only in case, file types outside the allow list, more than 500
//! files, more than 20 MB unpacked, and an unpacked size over 100 times the archive size.
//! Entries with a part that starts with a dot (`.github`, `.gitignore`) are left out.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use zip::CompressionMethod;

use crate::caps;
use crate::paths;

/// The result of an unpack.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extracted {
    /// Relative path with `/`, mapped to the SHA-256 (lowercase hex) of the file.
    pub files: BTreeMap<String, String>,
    /// Bytes of all files.
    pub bytes: u64,
}

/// The SHA-256 of some bytes, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

struct Entry {
    index: usize,
    path: String,
}

/// Unpack `archive` into the empty folder `dest`. With `strip_top` all entries must lie in one
/// top folder (a GitHub tag zip), and the unpacker removes that folder from the paths.
pub fn extract(archive: &[u8], dest: &Path, strip_top: bool) -> Result<Extracted, String> {
    if archive.len() as u64 > caps::ARCHIVE_BYTES {
        return Err(format!(
            "archive is larger than {} bytes",
            caps::ARCHIVE_BYTES
        ));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|e| format!("not a readable zip archive: {e}"))?;
    if zip.len() > caps::FILES * 2 {
        return Err(format!("more than {} entries", caps::FILES * 2));
    }

    // Pass 1: names and types only. No entry is inflated here.
    let mut entries = Vec::new();
    let mut top: Option<String> = None;
    let mut seen_lower = HashSet::new();
    let mut dirs_lower = HashSet::new();
    let mut files_lower = HashSet::new();
    for index in 0..zip.len() {
        let e = zip
            .by_index_raw(index)
            .map_err(|e| format!("entry {index}: {e}"))?;
        let raw = e
            .name()
            .map_err(|e| format!("entry {index}: {e}"))?
            .to_string();
        paths::check_relative(&raw).map_err(|m| format!("entry {index}: {m}"))?;
        if e.encrypted() {
            return Err(format!("entry \"{raw}\" is encrypted"));
        }
        let mode_type = e.unix_mode().map(|m| m & 0o170_000).unwrap_or(0);
        if mode_type == 0o120_000 {
            return Err(format!("entry \"{raw}\" is a link"));
        }
        if !matches!(mode_type, 0 | 0o100_000 | 0o040_000) {
            return Err(format!("entry \"{raw}\" is a special file"));
        }
        if !e.is_dir()
            && !matches!(
                e.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err(format!(
                "entry \"{raw}\" uses an unsupported compression method"
            ));
        }
        let is_dir = e.is_dir() || raw.ends_with('/');
        drop(e);

        let mut path = raw.trim_end_matches('/').to_string();
        if strip_top {
            let (first, rest) = match path.split_once('/') {
                Some((f, r)) => (f.to_string(), Some(r.to_string())),
                None if is_dir => (path.clone(), None),
                None => return Err(format!("entry \"{raw}\" is not in a top folder")),
            };
            match &top {
                None => top = Some(first),
                Some(t) if *t == first => {}
                Some(t) => {
                    return Err(format!("entry \"{raw}\" is outside the top folder \"{t}\""));
                }
            }
            match rest {
                Some(r) => path = r,
                None => continue,
            }
        }
        if path.is_empty() || paths::is_hidden(&path) {
            continue;
        }
        let lower = path.to_lowercase();
        if !seen_lower.insert((lower.clone(), is_dir)) {
            if is_dir {
                continue; // a repeated folder entry is harmless
            }
            return Err(format!(
                "name collision (same name, or a name that differs only in case): \"{path}\""
            ));
        }
        // Every parent folder of this path.
        let mut acc = String::new();
        let parts: Vec<&str> = lower.split('/').collect();
        let last = if is_dir { parts.len() } else { parts.len() - 1 };
        for p in &parts[..last] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            dirs_lower.insert(acc.clone());
        }
        if !is_dir {
            files_lower.insert(lower);
            if paths::kind_of(&path).is_none() {
                return Err(format!("file type not allowed: \"{path}\""));
            }
            entries.push(Entry { index, path });
        }
    }
    if let Some(clash) = files_lower.intersection(&dirs_lower).next() {
        return Err(format!(
            "name collision between a file and a folder: \"{clash}\""
        ));
    }
    if entries.len() > caps::FILES {
        return Err(format!("more than {} files", caps::FILES));
    }

    // Pass 2: inflate and write, with a count of the real bytes.
    let max_total = caps::UNPACKED_BYTES.min((archive.len() as u64).saturating_mul(caps::RATIO));
    let mut out = Extracted::default();
    for entry in &entries {
        let kind = paths::kind_of(&entry.path)
            .ok_or_else(|| format!("file type not allowed: \"{}\"", entry.path))?;
        let cap = paths::size_cap(&entry.path, kind);
        let mut file = zip
            .by_index(entry.index)
            .map_err(|e| format!("\"{}\": {e}", entry.path))?;
        let target = dest.join(&entry.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create folder: {e}"))?;
        }
        let mut sink = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| format!("cannot create \"{}\": {e}", entry.path))?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut buf = [0u8; 32 * 1024];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| format!("\"{}\": {e}", entry.path))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            out.bytes += n as u64;
            if size > cap {
                return Err(format!("\"{}\" is larger than {cap} bytes", entry.path));
            }
            if out.bytes > caps::UNPACKED_BYTES {
                return Err(format!(
                    "more than {} bytes after unpacking",
                    caps::UNPACKED_BYTES
                ));
            }
            if out.bytes > max_total {
                return Err(format!(
                    "compression ratio above {}:1 (zip bomb)",
                    caps::RATIO
                ));
            }
            hasher.update(&buf[..n]);
            sink.write_all(&buf[..n])
                .map_err(|e| format!("cannot write \"{}\": {e}", entry.path))?;
        }
        out.files
            .insert(entry.path.clone(), hex(&hasher.finalize()));
    }
    Ok(out)
}

/// Pack the files of a theme folder into a zip, in path order, with fixed timestamps. Hidden
/// files, `theme.lock` and `.upstream` files are left out. The caller checks the theme first.
pub fn pack_dir(dir: &Path) -> Result<Vec<u8>, String> {
    use zip::write::SimpleFileOptions;
    let files = crate::package::scan(dir).map_err(|i| crate::validate::format_issues(&i))?;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for f in files {
        if f.rel == "theme.lock" || f.rel.ends_with(".upstream") {
            continue;
        }
        let bytes = fs::read(&f.abs).map_err(|e| format!("{}: {e}", f.rel))?;
        w.start_file(f.rel.as_str(), o).map_err(|e| e.to_string())?;
        w.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    w.finish()
        .map(Cursor::into_inner)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod testzip {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    /// Build a zip of `(name, bytes)` entries with deflate.
    pub fn build(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, o).unwrap();
            } else {
                w.start_file(*name, o).unwrap();
                w.write_all(data).unwrap();
            }
        }
        w.finish().unwrap().into_inner()
    }

    /// A zip with a symlink entry.
    pub fn with_symlink(name: &str, target: &str) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.add_symlink(name, target, SimpleFileOptions::default())
            .unwrap();
        w.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::testzip::{build, with_symlink};
    use super::*;

    fn run(zip: &[u8], strip: bool) -> Result<Extracted, String> {
        let dir = tempfile::tempdir().unwrap();
        extract(zip, dir.path(), strip)
    }

    #[test]
    fn a_good_archive_unpacks_with_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let z = build(&[("theme.toml", b"id = 1"), ("layouts/desktop/a.json", b"{}")]);
        let x = extract(&z, dir.path(), false).unwrap();
        assert_eq!(x.files.len(), 2);
        assert_eq!(x.files["theme.toml"], sha256_hex(b"id = 1"));
        assert!(dir.path().join("layouts/desktop/a.json").is_file());
    }

    #[test]
    fn the_top_folder_is_stripped_and_dot_files_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let z = build(&[
            ("repo-1.0.0/", b""),
            ("repo-1.0.0/theme.toml", b"x"),
            ("repo-1.0.0/.github/workflows/ci.yml", b"x"),
            ("repo-1.0.0/.gitignore", b"x"),
        ]);
        let x = extract(&z, dir.path(), true).unwrap();
        assert_eq!(x.files.keys().collect::<Vec<_>>(), ["theme.toml"]);
        assert!(!dir.path().join(".github").exists());
        assert!(run(&build(&[("a/x.toml", b"x"), ("b/y.toml", b"x")]), true).is_err());
    }

    #[test]
    fn a_zip_bomb_stops() {
        let zeros = vec![0u8; 3 * 1024 * 1024];
        let z = build(&[("assets/images/a.png", &zeros)]);
        assert!(z.len() < 30_000, "{}", z.len());
        let e = run(&z, false).unwrap_err();
        assert!(e.contains("ratio"), "{e}");
    }

    #[test]
    fn a_dot_dot_path_fails() {
        for name in [
            "../evil.toml",
            "a/../../evil.toml",
            "/abs.toml",
            "a\\b.toml",
            "nul.toml",
        ] {
            let z = build(&[(name, b"x")]);
            assert!(run(&z, false).is_err(), "{name}");
        }
    }

    #[test]
    fn a_symlink_fails() {
        let e = run(&with_symlink("link.toml", "/etc/passwd"), false).unwrap_err();
        assert!(e.contains("link"), "{e}");
    }

    #[test]
    fn case_collisions_and_bad_types_fail() {
        assert!(run(&build(&[("A.toml", b"x"), ("a.toml", b"x")]), false).is_err());
        assert!(run(&build(&[("a.md", b"x"), ("A.md/x.md", b"x")]), false).is_err());
        assert!(run(&build(&[("run.exe", b"x")]), false).is_err());
        assert!(run(&build(&[("a.js", b"x")]), false).is_err());
    }

    #[test]
    fn size_caps_hold() {
        let big = vec![b'a'; (caps::LAYOUT_BYTES + 1) as usize];
        // Random-ish data so the ratio rule does not fire first.
        let mut noisy = big.clone();
        let mut x = 12345u32;
        for b in &mut noisy {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *b = (x >> 16) as u8;
        }
        let e = run(&build(&[("layouts/desktop/a.json", &noisy)]), false).unwrap_err();
        assert!(e.contains("larger than"), "{e}");
        let many: Vec<String> = (0..501).map(|i| format!("f{i}.md")).collect();
        let entries: Vec<(&str, &[u8])> = many.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
        assert!(run(&build(&entries), false).unwrap_err().contains("files"));
    }

    #[test]
    fn an_archive_over_the_cap_fails() {
        let big = vec![0u8; (caps::ARCHIVE_BYTES + 1) as usize];
        assert!(run(&big, false).unwrap_err().contains("larger"));
    }
}
