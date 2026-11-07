//! An open document: source text plus its parse tree, with incremental reparse.

use tree_sitter::{InputEdit, Parser, Point, Tree};

use crate::language::data;
use crate::{Error, HighlightSpan, Language, Stub, SyntaxDiagnostic, diagnostics, highlight};

/// A source text and its syntax tree. Keep one `Document` per open file: [`Document::edit`]
/// reuses the old tree, so a reparse after a small edit costs well under a millisecond for
/// typical files.
pub struct Document {
    language: Language,
    source: String,
    tree: Tree,
    parser: Parser,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("language", &self.language)
            .field("bytes", &self.source.len())
            .finish()
    }
}

impl Document {
    /// Parse `source` in full.
    pub fn new(language: Language, source: impl Into<String>) -> Result<Self, Error> {
        let source = source.into();
        if source.len() > u32::MAX as usize {
            return Err(Error::TooLarge);
        }
        let mut parser = Parser::new();
        parser
            .set_language(&data(language)?.ts)
            .map_err(|e| Error::Grammar(language, e.to_string()))?;
        let tree = parser.parse(&source, None).ok_or(Error::Parse)?;
        Ok(Self {
            language,
            source,
            tree,
            parser,
        })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// The blake3 hash of the source as 64 hex characters (the stub key).
    pub fn hash(&self) -> String {
        crate::stub::hash_source(&self.source)
    }

    /// Replace the bytes `start_byte..old_end_byte` with `new_text` and reparse incrementally.
    /// The bounds must lie on character boundaries.
    pub fn edit(
        &mut self,
        start_byte: usize,
        old_end_byte: usize,
        new_text: &str,
    ) -> Result<(), Error> {
        let src = &self.source;
        if start_byte > old_end_byte
            || old_end_byte > src.len()
            || !src.is_char_boundary(start_byte)
            || !src.is_char_boundary(old_end_byte)
        {
            return Err(Error::BadEdit);
        }
        if src.len() - (old_end_byte - start_byte) + new_text.len() > u32::MAX as usize {
            return Err(Error::TooLarge);
        }
        let start_position = point_at(src, start_byte);
        let old_end_position = advance(start_position, &src[start_byte..old_end_byte]);
        let new_end_position = advance(start_position, new_text);
        self.tree.edit(&InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte + new_text.len(),
            start_position,
            old_end_position,
            new_end_position,
        });
        self.source
            .replace_range(start_byte..old_end_byte, new_text);
        self.tree = self
            .parser
            .parse(&self.source, Some(&self.tree))
            .ok_or(Error::Parse)?;
        Ok(())
    }

    /// Set a new text. The method finds the common prefix and suffix and reparses
    /// incrementally for the changed middle part only.
    pub fn replace_text(&mut self, new_source: &str) -> Result<(), Error> {
        let old = self.source.as_bytes();
        let new = new_source.as_bytes();
        let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        while !self.source.is_char_boundary(prefix) || !new_source.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let max_suffix = old.len().min(new.len()) - prefix;
        let mut suffix = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        while !self.source.is_char_boundary(old.len() - suffix)
            || !new_source.is_char_boundary(new.len() - suffix)
        {
            suffix -= 1;
        }
        self.edit(
            prefix,
            old.len() - suffix,
            &new_source[prefix..new.len() - suffix],
        )
    }

    /// The syntax errors of the current tree, sorted by position.
    pub fn syntax_errors(&self) -> Vec<SyntaxDiagnostic> {
        diagnostics::collect(&self.tree, &self.source)
    }

    /// The semantic highlight of the whole text.
    pub fn highlights(&self) -> Result<Vec<HighlightSpan>, Error> {
        highlight::highlight(self.language, &self.source)
    }

    /// The semantic highlight of the bytes `range` only, for the visible lines of a view. The
    /// method reuses the tree and does not parse again. Tokens are cut at the bounds of the
    /// range. A range past the end of the text is clamped.
    pub fn highlights_in(
        &self,
        range: std::ops::Range<usize>,
    ) -> Result<Vec<HighlightSpan>, Error> {
        highlight::highlight_range(self.language, &self.tree, &self.source, range)
    }

    /// The semantic highlight of the lines `first..end` (0-based, `end` excluded). See
    /// [`Document::highlights_in`].
    pub fn highlights_in_lines(
        &self,
        first: usize,
        end: usize,
    ) -> Result<Vec<HighlightSpan>, Error> {
        self.highlights_in(highlight::line_bytes(&self.source, first, end))
    }

    /// The stub of the current text. The method reuses the tree and does not parse again.
    pub fn stub(&self) -> Result<Stub, Error> {
        Stub::from_tree(self.language, &self.tree, &self.source)
    }

    /// The tree as an S-expression, for debugging and tests.
    pub fn to_sexp(&self) -> String {
        self.tree.root_node().to_sexp()
    }
}

/// The line and byte column of a byte offset.
fn point_at(text: &str, byte: usize) -> Point {
    let head = &text.as_bytes()[..byte];
    let row = head.iter().filter(|&&b| b == b'\n').count();
    let line_start = head.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    Point {
        row,
        column: byte - line_start,
    }
}

/// The point after `text`, when the text starts at `from`.
fn advance(from: Point, text: &str) -> Point {
    let bytes = text.as_bytes();
    match bytes.iter().rposition(|&b| b == b'\n') {
        None => Point {
            row: from.row,
            column: from.column + bytes.len(),
        },
        Some(last) => Point {
            row: from.row + bytes.iter().filter(|&&b| b == b'\n').count(),
            column: bytes.len() - last - 1,
        },
    }
}
