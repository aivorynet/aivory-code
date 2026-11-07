use std::borrow::Cow;

use aivory_protocol::warroom::Part;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::error::MapError;

/// Finds the part that owns a file.
///
/// The first part in file order with a matching glob wins. A glob matches
/// a path relative to the repository root, with `/` between folders:
/// `*` stays inside one folder, `**` crosses folders. A glob may also use
/// `\` between folders, and so may the path.
#[derive(Debug, Clone)]
pub struct PartMatcher {
    parts: Vec<(String, GlobSet)>,
}

impl PartMatcher {
    /// Compile the globs of `parts`. Fails on the first bad glob.
    pub fn new(parts: &[Part]) -> Result<Self, MapError> {
        let mut compiled = Vec::with_capacity(parts.len());
        for part in parts {
            let mut set = GlobSetBuilder::new();
            for pattern in &part.paths {
                let glob = compile(pattern).map_err(|reason| MapError::BadGlob {
                    line: None,
                    part: part.name.clone(),
                    glob: pattern.clone(),
                    reason,
                })?;
                set.add(glob);
            }
            let set = set.build().map_err(|e| MapError::BadGlob {
                line: None,
                part: part.name.clone(),
                glob: String::new(),
                reason: e.to_string(),
            })?;
            compiled.push((part.name.clone(), set));
        }
        Ok(Self { parts: compiled })
    }

    /// The name of the part that owns `rel_path`, or `None` when no part matches.
    ///
    /// `rel_path` is relative to the repository root and may use `\` or `/`.
    pub fn part_of(&self, rel_path: &str) -> Option<&str> {
        let path = normalize(rel_path);
        self.parts
            .iter()
            .find(|(_, set)| set.is_match(path.as_ref()))
            .map(|(name, _)| name.as_str())
    }
}

/// Compile one glob. The error is the reason, without the glob text.
pub(crate) fn compile(pattern: &str) -> Result<globset::Glob, String> {
    let normal = normalize(pattern);
    if normal.is_empty() {
        return Err("the pattern is empty".to_owned());
    }
    GlobBuilder::new(&normal)
        .literal_separator(true)
        .build()
        .map_err(|e| e.kind().to_string())
}

/// Use `/` between folders and drop a leading `./` or `/`.
fn normalize(path: &str) -> Cow<'_, str> {
    let mut text = Cow::Borrowed(path.trim());
    if text.contains('\\') {
        text = Cow::Owned(text.replace('\\', "/"));
    }
    let trimmed = text.trim_start_matches("./").trim_start_matches('/');
    if trimmed.len() != text.len() {
        text = Cow::Owned(trimmed.to_owned());
    }
    text
}
