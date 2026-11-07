//! `file:` URIs and paths.

use std::path::{Path, PathBuf};

fn encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
}

fn decode(s: &str) -> String {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `file:` URI of an absolute path. The `\\?\` prefix of Windows is removed.
pub fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_start_matches("//?/");
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        out.push('/');
    }
    encode(text, &mut out);
    out
}

/// The path of a `file:` URI, or `None` for another scheme.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Skip the (empty) authority.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut path = decode(rest);
    let b = path.as_bytes();
    // `/C:/dir` -> `C:/dir`
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

/// `path` relative to `root` with `/` as separator, when `path` lies under `root` (the drive
/// letter and the case on Windows do not matter). Otherwise the path as a string.
pub fn relative_to(root: &Path, path: &Path) -> String {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("//?/")
            .to_owned()
    };
    let (r, p) = (norm(root), norm(path));
    let r = r.trim_end_matches('/');
    let under = |a: &str, b: &str| {
        a.len() > b.len() && a[..b.len()].eq_ignore_ascii_case(b) && a.as_bytes()[b.len()] == b'/'
    };
    if under(&p, r) {
        p[r.len() + 1..].to_owned()
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_round_trip_posix_path() {
        let uri = path_to_uri(Path::new("/home/a b/ä.rs"));
        assert_eq!(uri, "file:///home/a%20b/%C3%A4.rs");
        assert_eq!(uri_to_path(&uri).unwrap(), PathBuf::from("/home/a b/ä.rs"));
    }

    #[test]
    fn uri_round_trip_windows_path() {
        let uri = path_to_uri(Path::new(r"C:\Work\my repo\a.rs"));
        assert_eq!(uri, "file:///C:/Work/my%20repo/a.rs");
        assert_eq!(
            uri_to_path(&uri).unwrap(),
            PathBuf::from("C:/Work/my repo/a.rs")
        );
        // VS Code style
        assert_eq!(
            uri_to_path("file:///c%3A/x/y.rs").unwrap(),
            PathBuf::from("c:/x/y.rs")
        );
        assert_eq!(
            path_to_uri(Path::new(r"\\?\C:\x\y.rs")),
            "file:///C:/x/y.rs"
        );
        assert!(uri_to_path("https://x").is_none());
    }

    #[test]
    fn relative_paths_under_a_root() {
        assert_eq!(
            relative_to(Path::new("/r/repo"), Path::new("/r/repo/src/a.rs")),
            "src/a.rs"
        );
        assert_eq!(
            relative_to(Path::new(r"C:\r"), Path::new("c:/R/src/a.rs")),
            "src/a.rs"
        );
        assert_eq!(
            relative_to(Path::new("/r/repo"), Path::new("/other/a.rs")),
            "/other/a.rs"
        );
        assert_eq!(
            relative_to(Path::new("/r/repo"), Path::new("/r/repo2/a.rs")),
            "/r/repo2/a.rs"
        );
    }
}
