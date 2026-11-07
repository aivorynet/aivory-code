//! AIVory Code syntax layer: tree-sitter parse, stubs, outline, syntax errors.
//!
//! This crate is layer L0 (parse tree, syntax errors, highlight) and the producer of the L1
//! stubs of `docs/design/03-code-intelligence.md`. The public API is small:
//!
//! - [`Language::detect`] finds the language of a file from its name and extension.
//! - [`Document`] holds the text of an open file with its tree. [`Document::edit`] and
//!   [`Document::replace_text`] reparse incrementally.
//!   [`Document::syntax_errors`], [`Document::highlights`], and [`Document::stub`] read from it.
//! - [`Stub::parse`] builds the stub of a closed file in one call. A stub has the
//!   declarations, imports, references, outline, and folding ranges, plus the key
//!   ([`Stub::key`]) under which the index stores it.
//! - [`highlight`] highlights a text without a [`Document`].
//! - [`Document::highlights_in`] and [`Document::highlights_in_lines`] highlight only the
//!   visible bytes or lines on the parsed tree (the viewport highlight).
//!
//! Types of tree-sitter do not appear in the public API. Offsets are byte offsets in UTF-8 text.
//! Lines start at 0, and a column counts bytes (see [`Range`]).
//!
//! Declarations come from the queries in `queries/*.tags.scm`: every declaration has a
//! `@definition.<kind>` capture and a `@name` capture. The grammars' own `tags.scm` files differ
//! in captures and quality (no struct or enum kinds, no fields, no constants), so this crate
//! has one query set with the same captures for all languages. Imports come from
//! `queries/*.imports.scm` with the captures `@import`, `@path`, and `@alias`.
//! Change a query or the extractor code: raise [`EXTRACTOR_VERSION`].

mod decls;
mod diagnostics;
mod document;
mod error;
mod highlight;
mod language;
mod stub;
mod types;

pub use document::Document;
pub use error::Error;
pub use highlight::highlight;
pub use language::Language;
pub use stub::{EXTRACTOR_VERSION, Stub};
pub use types::{
    DeclKind, Declaration, FoldKind, FoldingRange, HighlightSpan, Import, OutlineItem, Range,
    Reference, SyntaxDiagnostic, SyntaxErrorKind, TokenKind, Visibility,
};

/// The syntax errors of a text. The function parses the text itself.
pub fn syntax_errors(language: Language, source: &str) -> Result<Vec<SyntaxDiagnostic>, Error> {
    Ok(Document::new(language, source)?.syntax_errors())
}
