//! Code intelligence and the turn ledger: the `code/*` and `ledger/*` methods
//! (`docs/design/03-code-intelligence.md`, `05-vcs-ledger.md`).
//!
//! Conventions of this module:
//!
//! - A line number starts at 1. A column (`col`, `start`, `end`) is a byte offset in the UTF-8
//!   text of the line and starts at 0. An end column is exclusive.
//! - A path is relative to the root of the codebase and uses `/`.
//! - A request with `session` reads the files and the index delta of that session. Without it,
//!   the request reads the main checkout of the codebase.
//! - A list result is paged: `page` starts at 1.
//! - Times are [`UnixMs`].
//! - `source` tells which engine answered: the symbol index, or a language server (LSP) that
//!   runs for the codebase. `code/file` starts the language server of the file in the
//!   background. The queries use it as soon as it runs, and never wait for its start.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::UnixMs;

/// A range of lines, from 1, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub from: u32,
    pub to: u32,
}

/// The engine that answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodeSource {
    /// The symbol index (declarations and name references; no type resolution).
    #[default]
    Index,
    /// A language server.
    Lsp,
    /// The syntax tree of the file alone.
    Syntax,
}

/// The kind of a highlighted token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
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

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SymbolKind {
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
    Section,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FoldKind {
    Block,
    Comment,
    Imports,
}

/// One highlighted token. A token does not cross a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    pub line: u32,
    pub start: u32,
    pub end: u32,
    pub kind: TokenKind,
}

/// A node of the structure view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutlineNode {
    pub name: String,
    pub kind: SymbolKind,
    /// The line of the name.
    pub line: u32,
    /// The first line of the declaration.
    pub start_line: u32,
    /// The last line of the declaration.
    pub end_line: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OutlineNode>,
}

/// A range that an editor can fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fold {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: FoldKind,
}

/// The level of a finding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FindingLevel {
    None,
    Note,
    Warning,
    Error,
}

/// One diagnostic: a row of a SARIF log, without the parts that no client needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// For example `aivory/syntax-error`, `aivory/long-function`, or a linter rule.
    pub rule: String,
    pub level: FindingLevel,
    pub message: String,
    pub path: String,
    pub line: u32,
    pub col: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_col: Option<u32>,
    /// The tool that found it: `aivory-syntax`, `aivory-rules`, `clippy`, or a language server.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// The stable id of the finding (`aivoryFingerprint/v1`). It stays the same when the
    /// finding moves in its declaration.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
}

/// A place in a file, with the text of its line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub path: String,
    pub line: u32,
    pub col: u32,
    /// The length of the name in bytes, when the engine knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u32>,
    /// The text of the line, trimmed, at most 160 characters. Empty when the file is not
    /// readable.
    pub preview: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SymbolKind>,
    /// The enclosing declaration, for example `Invoice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// code/file

/// Parameters of `code/file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodeFileParams {
    pub codebase: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// A git revision (`HEAD`, a branch, a commit), or `turn:<n>` with `session`: the snapshot
    /// at the end of turn `n`. Default: the working tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The lines to highlight (the viewport). Default: lines 1 to 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<LineRange>,
}

/// Result of `code/file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodeFile {
    pub path: String,
    /// The language name (`rust`, `typescript`, ...), or empty for a file without a grammar.
    pub language: String,
    /// The whole text, cut at 1 MiB (then `truncated` is true).
    pub text: String,
    pub total_lines: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The lines that `highlights` covers.
    pub highlighted: LineRange,
    pub highlights: Vec<Highlight>,
    pub outline: Vec<OutlineNode>,
    pub folding: Vec<Fold>,
    /// The findings in this file: the stored findings of the last turn that changed it in
    /// this session, or the syntax errors of the text.
    pub diagnostics: Vec<Finding>,
}

// ---------------------------------------------------------------------------------------------
// code/definition, code/references, code/hover

/// Parameters of `code/definition`, `code/references`, and `code/hover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodePosParams {
    pub codebase: String,
    pub path: String,
    /// The cursor.
    pub line: u32,
    pub col: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// Default 20, at most 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_page: Option<u32>,
}

/// A page of places.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlacePage {
    /// The name under the cursor that the query used.
    pub name: String,
    pub items: Vec<Place>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    /// `total` is a lower bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub source: CodeSource,
    /// The index of the codebase is not complete yet: ask again in a moment.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub indexing: bool,
}

/// Result of `code/hover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hover {
    /// The name under the cursor. Empty when the cursor is not on a name.
    pub name: String,
    /// Markdown. The declaration header (`signature`) in a code block, then the doc text when
    /// a language server gave one.
    pub text: String,
    /// The first line of the declaration, when a definition was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SymbolKind>,
    /// The token under the cursor: its line, start, and end column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Highlight>,
    /// The definitions, at most 5 (see `code/definition` for all).
    pub definitions: Vec<Place>,
    pub source: CodeSource,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub indexing: bool,
}

// ---------------------------------------------------------------------------------------------
// code/symbols

/// Parameters of `code/symbols`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SymbolsParams {
    pub codebase: String,
    /// A name, a prefix, or a few letters in order (`inv` finds `Invoice`).
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SymbolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_page: Option<u32>,
}

/// A declaration found by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub path: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The declaration header on one line.
    pub signature: String,
}

/// Result of `code/symbols`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SymbolPage {
    pub items: Vec<Symbol>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub indexing: bool,
}

// ---------------------------------------------------------------------------------------------
// code/diagnostics

/// Parameters of `code/diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsParams {
    pub codebase: String,
    /// With a session: the stored findings of that session. Without it: the daemon runs the
    /// fast stages (syntax and rules) now on the files that changed in the checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The turn. Default: the last turn that has stored findings. Needs `session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    /// Only this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Result of `code/diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsResult {
    /// The turn of the findings. `None` for a run without a session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    /// The findings after the turn, in the files that the turn changed.
    pub rows: Vec<Finding>,
    /// In `rows`, not in the state before the turn. `added` and `removed` compare the stages
    /// that ran on both sides (syntax and rules; linters and language servers run only after
    /// the turn).
    pub added: Vec<Finding>,
    /// In the state before the turn, not in `rows`.
    pub removed: Vec<Finding>,
    /// The stages that ran and the notes of the run (skipped tools, truncated stages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// code/rename

/// Parameters of `code/rename`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenameParams {
    pub codebase: String,
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub new_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// One text edit. The range is in the old text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub new_text: String,
}

/// The edits of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileEdits {
    pub path: String,
    pub edits: Vec<TextEdit>,
}

/// Result of `code/rename`. The daemon never applies the edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenamePreview {
    pub old_name: String,
    pub new_name: String,
    pub files: Vec<FileEdits>,
    /// `false` for an answer of the index: it renames every use of the name, also in
    /// unrelated scopes. A language server gives exact edits.
    pub exact: bool,
    pub source: CodeSource,
}

// ---------------------------------------------------------------------------------------------
// ledger/*

/// Parameters of `ledger/turns`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerTurnsParams {
    pub session: String,
}

/// A file that a turn changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnFileStat {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

/// The longest prompt in a ledger result, in characters.
pub const PROMPT_CHARS: usize = 200;

/// One turn of the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerTurn {
    /// The turn number. Turn 0 is the baseline at the start of the session.
    pub n: u32,
    /// At most [`PROMPT_CHARS`] characters.
    pub prompt: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub started_at: UnixMs,
    pub ended_at: UnixMs,
    /// USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    /// The turn has a snapshot that `ledger/revert` and `code/file` (`rev: "turn:n"`) can use.
    pub snapshot: bool,
    pub files: Vec<TurnFileStat>,
    /// The findings that the turn added and removed (`code/diagnostics` has the rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics_added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics_removed: Option<u32>,
}

/// Result of `ledger/turns`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnList {
    pub session: String,
    pub turns: Vec<LedgerTurn>,
}

/// Parameters of `ledger/blame`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlameParams {
    pub codebase: String,
    pub path: String,
    /// The session that owns the uncommitted lines. Without it, only committed lines have a
    /// turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<LineRange>,
}

/// The commit of a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommitRef {
    /// The first 12 characters of the commit id.
    pub id: String,
    pub summary: String,
    pub time: UnixMs,
}

/// The turn that wrote a line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlameTurn {
    pub session: String,
    pub n: u32,
    /// At most [`PROMPT_CHARS`] characters.
    pub prompt: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// The end of the turn.
    pub time: UnixMs,
    /// USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// A run of lines with the same commit and the same turn. A hunk of one line is one line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlameHunk {
    /// The first line.
    pub line: u32,
    /// The number of lines.
    pub count: u32,
    /// `None`: the lines are not committed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitRef>,
    /// `None`: no turn of the ledger wrote these lines (a human, or an unknown tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<BlameTurn>,
}

/// Result of `ledger/blame`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileBlame {
    pub path: String,
    pub hunks: Vec<BlameHunk>,
}

/// Parameters of `ledger/timeline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelineParams {
    pub codebase: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// A turn in a timeline or a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnId {
    pub session: String,
    pub n: u32,
}

/// One event of the history of a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TimelineEvent {
    Commit {
        id: String,
        summary: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        author: String,
        time: UnixMs,
        /// The turns that the commit carries (the `Aivory-Turns` trailer).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        turns: Vec<TurnId>,
    },
    Turn {
        session: String,
        n: u32,
        prompt: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        model: String,
        time: UnixMs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
        /// The lines of this file that the turn added and removed.
        added: u32,
        removed: u32,
    },
}

/// Result of `ledger/timeline`: the events of one file, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub path: String,
    pub events: Vec<TimelineEvent>,
}

/// Parameters of `ledger/revert`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevertParams {
    pub session: String,
    /// Undo this turn: the working tree of the session returns to the snapshot before it.
    pub turn: u32,
}

/// Result of `ledger/revert`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevertResult {
    pub session: String,
    pub turn: u32,
    /// The turn number of the snapshot that the working tree has now.
    pub restored: u32,
    /// The first 12 characters of the snapshot commit.
    pub snapshot: String,
}
