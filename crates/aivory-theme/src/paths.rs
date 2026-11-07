//! Path rules and the file-type allow list.

/// Check one relative path of an archive entry or of a file in a theme folder. The path uses `/`.
/// A trailing `/` (a folder entry) is fine.
pub fn check_relative(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".to_string());
    }
    if path.contains('\\') {
        return Err(format!("backslash in path \"{path}\""));
    }
    if path.starts_with('/') {
        return Err(format!("absolute path \"{path}\""));
    }
    if path.len() > 240 {
        return Err("path longer than 240 bytes".to_string());
    }
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    for comp in trimmed.split('/') {
        check_component(comp, path)?;
    }
    Ok(())
}

fn check_component(comp: &str, path: &str) -> Result<(), String> {
    if comp.is_empty() {
        return Err(format!("empty path part in \"{path}\""));
    }
    if comp == "." || comp == ".." {
        return Err(format!("\"{comp}\" in path \"{path}\""));
    }
    if comp
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '$'))
    {
        return Err(format!("forbidden character in path \"{path}\""));
    }
    if comp.ends_with('.') || comp.ends_with(' ') {
        return Err(format!(
            "path part ends with a dot or a space in \"{path}\""
        ));
    }
    if is_reserved_name(comp) {
        return Err(format!("Windows reserved name in path \"{path}\""));
    }
    Ok(())
}

/// A Windows device name (`CON`, `NUL`, `COM1`, ...), with or without an extension.
pub fn is_reserved_name(comp: &str) -> bool {
    let stem = comp.split('.').next().unwrap_or(comp).to_ascii_uppercase();
    let stem = stem.trim_end_matches(' ');
    matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CONIN" | "CONOUT")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// True when a part of the path starts with a dot (`.github`, `.gitignore`). The unpacker
/// leaves these out; a theme does not use them.
pub fn is_hidden(path: &str) -> bool {
    path.split('/').any(|c| c.starts_with('.'))
}

/// The kind of a file in a theme package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Toml,
    Json,
    Markdown,
    License,
    Font,
    Png,
    Jpeg,
    Webp,
    Svg,
    /// `theme.lock`.
    Lock,
}

impl FileKind {
    pub fn is_image(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg | Self::Webp | Self::Svg)
    }
}

/// The kind of a file by its path, or `None` when the type is not allowed (ADR 0024 section 8).
/// `theme.lock` and `<file>.upstream` are allowed next to the package files.
pub fn kind_of(path: &str) -> Option<FileKind> {
    if path == "theme.lock" {
        return Some(FileKind::Lock);
    }
    if let Some(base) = path.strip_suffix(".upstream") {
        // The base file must be a package file; a lock file has no upstream copy.
        return kind_of(base).filter(|k| *k != FileKind::Lock);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let upper = name.to_ascii_uppercase();
    if upper == "LICENSE" || upper == "LICENSE.TXT" || upper == "LICENSE.MD" {
        return Some(FileKind::License);
    }
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "toml" => FileKind::Toml,
        "json" => FileKind::Json,
        "md" => FileKind::Markdown,
        "ttf" | "otf" => FileKind::Font,
        "png" => FileKind::Png,
        "jpg" | "jpeg" => FileKind::Jpeg,
        "webp" => FileKind::Webp,
        "svg" => FileKind::Svg,
        _ => return None,
    })
}

/// The size cap of a file by its place and kind.
pub fn size_cap(path: &str, kind: FileKind) -> u64 {
    let base = path.strip_suffix(".upstream").unwrap_or(path);
    if kind == FileKind::Font || kind.is_image() {
        crate::caps::ASSET_BYTES
    } else if kind == FileKind::Json && (base.starts_with("layouts/") || base.starts_with("parts/"))
    {
        crate::caps::LAYOUT_BYTES
    } else {
        crate::caps::OTHER_BYTES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_paths_fail() {
        for p in [
            "../x",
            "a/../b",
            "/etc/passwd",
            "a\\b",
            "C:/x",
            "a//b",
            "con.txt",
            "a/NUL",
            "lpt1.png",
            "x.",
            "a/./b",
        ] {
            assert!(check_relative(p).is_err(), "{p}");
        }
        for p in [
            "theme.toml",
            "layouts/desktop/warroom.json",
            "assets/fonts/",
            "com0.png",
        ] {
            assert!(check_relative(p).is_ok(), "{p}");
        }
    }

    #[test]
    fn file_types() {
        assert_eq!(kind_of("theme.lock"), Some(FileKind::Lock));
        assert_eq!(kind_of("strings/en.toml.upstream"), Some(FileKind::Toml));
        assert_eq!(kind_of("theme.lock.upstream"), None);
        assert_eq!(kind_of("LICENSE"), Some(FileKind::License));
        assert_eq!(kind_of("a.exe"), None);
        assert_eq!(kind_of("a.js"), None);
        assert_eq!(kind_of("noext"), None);
    }
}
