//! One source file with the help that findings need: line text, columns, and the enclosing
//! declaration for the fingerprint.

use std::sync::OnceLock;

use aivory_syntax::{Document, Language};

use crate::sarif::{Level, Region, SarifResult};

/// A source file in memory.
#[derive(Debug)]
pub struct FileCtx {
    /// The path relative to the repo root, with `/` as separator.
    pub path: String,
    pub source: String,
    pub language: Option<Language>,
    /// Start byte of every line.
    line_starts: Vec<usize>,
    /// (start line, end line, qualified name) of every declaration; lines start at 0.
    /// Built on the first use: a clean file never needs it.
    decls: OnceLock<Vec<(u32, u32, String)>>,
}

impl FileCtx {
    pub fn new(path: &str, source: String, language: Option<Language>) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            path: path.replace('\\', "/"),
            source,
            language,
            line_starts,
            decls: OnceLock::new(),
        }
    }

    fn decls(&self) -> &[(u32, u32, String)] {
        self.decls.get_or_init(|| {
            let mut decls = Vec::new();
            if let Some(doc) = self
                .language
                .and_then(|l| Document::new(l, self.source.as_str()).ok())
                && let Ok(stub) = doc.stub()
            {
                for d in &stub.declarations {
                    let name = match &d.container {
                        Some(c) => format!("{c}::{}", d.name),
                        None => d.name.clone(),
                    };
                    decls.push((d.range.start_line, d.range.end_line, name));
                }
            }
            decls
        })
    }

    /// The text of a line (0-based), without the line end.
    pub fn line_text(&self, line0: usize) -> &str {
        let Some(&start) = self.line_starts.get(line0) else {
            return "";
        };
        let end = self
            .line_starts
            .get(line0 + 1)
            .copied()
            .unwrap_or(self.source.len());
        self.source[start..end].trim_end_matches(['\n', '\r'])
    }

    /// The number of lines.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The (line, character column) of a byte offset. Both start at 1.
    pub fn byte_to_line_col(&self, byte: usize) -> (u32, u32) {
        let byte = byte.min(self.source.len());
        let line0 = self.line_starts.partition_point(|&s| s <= byte) - 1;
        let start = self.line_starts[line0];
        let mut end = byte;
        while end > start && !self.source.is_char_boundary(end) {
            end -= 1;
        }
        let col = self.source[start..end].chars().count();
        (line0 as u32 + 1, col as u32 + 1)
    }

    /// The character column (starts at 1) of a UTF-16 column (starts at 0) in a line (0-based).
    pub fn utf16_to_col(&self, line0: usize, col16: u32) -> u32 {
        let mut units = 0u32;
        let mut chars = 0u32;
        for c in self.line_text(line0).chars() {
            if units >= col16 {
                break;
            }
            units += c.len_utf16() as u32;
            chars += 1;
        }
        chars + 1
    }

    /// The qualified name of the innermost declaration around a line (0-based).
    pub fn declaration_at(&self, line0: u32) -> Option<&str> {
        self.decls()
            .iter()
            .filter(|(s, e, _)| *s <= line0 && line0 <= *e)
            .min_by_key(|(s, e, _)| e - s)
            .map(|(_, _, n)| n.as_str())
    }

    /// Make a result with the fingerprint of the flagged line. `region` starts at line 1.
    pub fn result(
        &self,
        rule_id: &str,
        level: Level,
        message: &str,
        region: Region,
    ) -> SarifResult {
        let line0 = region.start_line.saturating_sub(1);
        SarifResult::new(rule_id, level, message, &self.path, region)
            .with_fingerprint(self.line_text(line0 as usize), self.declaration_at(line0))
    }

    /// Make a result from a byte range of the source.
    pub fn result_bytes(
        &self,
        rule_id: &str,
        level: Level,
        message: &str,
        start: usize,
        end: usize,
    ) -> SarifResult {
        let (sl, sc) = self.byte_to_line_col(start);
        let (el, ec) = self.byte_to_line_col(end);
        self.result(rule_id, level, message, Region::span(sl, sc, el, ec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_characters() {
        let f = FileCtx::new("a.txt", "ab\nä€x\n".to_owned(), None);
        assert_eq!(f.byte_to_line_col(0), (1, 1));
        assert_eq!(f.byte_to_line_col(3), (2, 1));
        // 'ä' is 2 bytes, '€' is 3 bytes: byte 8 is 'x'
        assert_eq!(f.byte_to_line_col(8), (2, 3));
        assert_eq!(f.utf16_to_col(1, 2), 3);
        assert_eq!(f.line_text(1), "ä€x");
        assert_eq!(f.line_text(9), "");
    }

    #[test]
    fn finds_the_enclosing_declaration() {
        let src = "struct A;\nimpl A {\n    fn go(&self) {\n        let x = 1;\n    }\n}\n";
        let f = FileCtx::new("a.rs", src.to_owned(), Some(Language::Rust));
        let name = f.declaration_at(3).unwrap();
        assert!(name.ends_with("go"), "{name}");
    }
}
