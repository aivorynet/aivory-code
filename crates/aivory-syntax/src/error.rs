//! The error type of the syntax layer.

use crate::Language;

/// Errors of the syntax layer. Source code with syntax errors is not an error: the parser
/// recovers, and the problems become [`crate::SyntaxDiagnostic`] values.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// The grammar does not match the tree-sitter runtime.
    #[error("the {0:?} grammar does not load: {1}")]
    Grammar(Language, String),
    /// A built-in query does not compile against the grammar (a bug in this crate or an update).
    #[error("the {which} query of {lang:?} does not compile: {message}")]
    Query {
        lang: Language,
        which: &'static str,
        message: String,
    },
    /// The parser stopped without a tree.
    #[error("the parser returned no tree")]
    Parse,
    /// Ranges are `u32`, so a source above 4 GiB is not supported.
    #[error("the source is larger than 4 GiB")]
    TooLarge,
    /// An edit has a range outside the text or inside a UTF-8 character.
    #[error("the edit range is not valid for the text")]
    BadEdit,
}
