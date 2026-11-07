//! Theme packages (ADR 0024): parse, validate and unpack a theme. This crate has no network
//! code. The daemon downloads an archive and passes the bytes to [`archive::extract`].
//!
//! - [`paths`]: the rules for a file path and the file-type allow list.
//! - [`archive`]: safe unpacking of a zip by read and write (never `ZipArchive::extract`).
//! - [`package`]: the manifest, the scan of a folder, the file hashes.
//! - [`tokens`], [`strings`], [`layout`], [`assets`]: the checks of each part.
//! - [`validate`]: the whole check of a theme folder.

pub mod archive;
pub mod assets;
pub mod layout;
pub mod package;
pub mod paths;
pub mod sample;
pub mod strings;
pub mod tokens;
pub mod validate;

use std::fmt;

/// The caps of ADR 0024 section 8.
pub mod caps {
    /// The archive.
    pub const ARCHIVE_BYTES: u64 = 10 * 1024 * 1024;
    /// All files after unpacking.
    pub const UNPACKED_BYTES: u64 = 20 * 1024 * 1024;
    pub const FILES: usize = 500;
    /// One asset (font or image).
    pub const ASSET_BYTES: u64 = 4 * 1024 * 1024;
    /// One layout file.
    pub const LAYOUT_BYTES: u64 = 512 * 1024;
    /// Every other file (manifest, tokens, strings, Markdown, license).
    pub const OTHER_BYTES: u64 = 1024 * 1024;
    pub const NODES: usize = 4000;
    pub const DEPTH: usize = 40;
    pub const FONT_FAMILIES: usize = 8;
    pub const IMAGE_PX: u32 = 4096;
    /// Characters of one text literal.
    pub const TEXT_CHARS: usize = 200;
    /// Unpacked bytes per archive byte.
    pub const RATIO: u64 = 100;
}

/// One validation error: the file, the JSON pointer inside it, and the rule that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub file: String,
    pub pointer: String,
    pub rule: String,
}

impl Issue {
    pub fn new(
        file: impl Into<String>,
        pointer: impl Into<String>,
        rule: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            pointer: pointer.into(),
            rule: rule.into(),
        }
    }
}

impl fmt::Display for Issue {
    /// `layouts/desktop/warroom.json /root/children/2/on/press/action: unknown action "delete"`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pointer.is_empty() {
            write!(f, "{}: {}", self.file, self.rule)
        } else {
            write!(f, "{} {}: {}", self.file, self.pointer, self.rule)
        }
    }
}

/// Escape one key for a JSON pointer (RFC 6901).
pub fn pointer_key(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}
