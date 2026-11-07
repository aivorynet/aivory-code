//! Plain data types shared by the syntax layer. All of them implement serde.

use serde::{Deserialize, Serialize};

/// A source range. Lines and columns start at 0. A column is a byte offset in its line
/// (the tree-sitter convention), not a character count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Range {
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl Range {
    pub(crate) fn from_node(node: &tree_sitter::Node) -> Self {
        let (s, e) = (node.start_position(), node.end_position());
        Self {
            start_byte: node.start_byte() as u32,
            end_byte: node.end_byte() as u32,
            start_line: s.row as u32,
            start_col: s.column as u32,
            end_line: e.row as u32,
            end_col: e.column as u32,
        }
    }

    /// True when `other` lies inside this range (the bounds may be equal).
    pub fn contains(&self, other: &Range) -> bool {
        self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
    }

    /// The byte range as `usize` bounds, for slicing the source.
    pub fn bytes(&self) -> std::ops::Range<usize> {
        self.start_byte as usize..self.end_byte as usize
    }
}

/// How a syntax problem looks in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyntaxErrorKind {
    /// The parser skipped input that it could not use (an `ERROR` node).
    Unexpected,
    /// The parser inserted a token that the input lacks (a `MISSING` node). The range is empty.
    Missing,
}

/// One syntax error found by the parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxDiagnostic {
    pub range: Range,
    pub kind: SyntaxErrorKind,
    /// A short message, for example "missing `;`" or "unexpected token `)`".
    pub message: String,
}

/// The fixed set of token kinds of the semantic highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    Keyword,
    Type,
    Function,
    Variable,
    String,
    Number,
    Comment,
    Operator,
    Punctuation,
    Constant,
    Attribute,
    Macro,
}

/// A highlighted byte range. Spans do not overlap and they are sorted by position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightSpan {
    pub start_byte: u32,
    pub end_byte: u32,
    pub kind: TokenKind,
}

/// What a declaration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclKind {
    Module,
    Namespace,
    Class,
    Struct,
    Enum,
    Variant,
    Interface,
    Trait,
    Impl,
    Function,
    Method,
    Constructor,
    Field,
    Property,
    Variable,
    Constant,
    TypeAlias,
    Macro,
    /// A named region of a data or markup file (TOML table, CSS rule, HTML element with an id).
    Section,
}

impl DeclKind {
    /// True for kinds that can contain members (class, struct, trait, impl, ...).
    pub fn is_type_like(self) -> bool {
        matches!(
            self,
            Self::Class | Self::Struct | Self::Enum | Self::Interface | Self::Trait | Self::Impl
        )
    }
}

/// The visibility of a declaration, as far as the syntax shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    /// Visible in the package, crate, assembly, or module (`pub(crate)`, `internal`, Java default).
    Internal,
    Protected,
    Private,
    /// The language has no visibility, or the syntax does not tell.
    Unknown,
}

/// One declaration in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub name: String,
    pub kind: DeclKind,
    /// The range of the whole declaration.
    pub range: Range,
    /// The range of the name.
    pub name_range: Range,
    /// The declaration header up to its body, on one line, 200 bytes at most.
    pub signature: String,
    /// The range of the doc comment (or the Python docstring), when there is one.
    pub doc: Option<Range>,
    pub visibility: Visibility,
    /// The name of the enclosing declaration.
    pub container: Option<String>,
    /// The index of the enclosing declaration in `Stub::declarations`.
    pub parent: Option<u32>,
}

/// One import (`use`, `import`, `#include`, `require`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The module or file path as written, without quotes.
    pub path: String,
    pub alias: Option<String>,
    pub range: Range,
}

/// An identifier that is not the name of a declaration (a candidate for "find usages").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub name: String,
    pub range: Range,
}

/// A node of the structure view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineItem {
    pub name: String,
    pub kind: DeclKind,
    pub range: Range,
    pub name_range: Range,
    pub children: Vec<OutlineItem>,
}

/// What a folding range covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FoldKind {
    Block,
    Comment,
    Imports,
}

/// A foldable region. `start_line` stays visible. The lines `start_line + 1 ..= end_line` fold.
/// Lines start at 0, and `end_line > start_line` always holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldingRange {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: FoldKind,
}
