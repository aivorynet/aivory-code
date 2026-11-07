//! AIVory Code diagnostics: LSP pool, rule packs, linters, SARIF pipeline per turn.
//!
//! This crate is the diagnostics part of `docs/design/03-code-intelligence.md`:
//!
//! - [`sarif`]: a small serde subset of SARIF 2.1.0 and the stable fingerprint of a finding.
//! - [`compare`]: `diff(before, after)` by fingerprint, and the score input "errors added".
//! - [`rules`]: the built-in quality rules (long function, deep nesting, cyclomatic complexity,
//!   duplicated blocks, risky APIs) and the ast-grep YAML rules of `<repo>/.aivory/rules`.
//! - [`linters`]: clippy, ruff, eslint, and golangci-lint: detection, run with a timeout, parsers.
//! - [`lsp`]: the LSP pool (one server per repo lineage) with hover, definition, references,
//!   rename, diagnostics, semantic tokens, and code actions.
//! - [`pipeline`]: the four stages with a time budget each, producing one SARIF log per turn.
//!
//! The crate has a blocking API. The daemon calls it from `spawn_blocking`.

pub mod compare;
pub mod error;
pub mod lang;
pub mod linters;
pub mod lsp;
pub mod pipeline;
pub mod rules;
pub mod sarif;
pub mod source;

pub use compare::{Diff, ScoreInput, diff};
pub use error::{Error, Result};
pub use pipeline::{
    Budgets, Pipeline, PipelineConfig, PipelineReport, RULE_SYNTAX_ERROR, StageReport, TurnInput,
};
pub use rules::{ParsedFile, RuleSet, Thresholds};
pub use sarif::{FINGERPRINT_KEY, Level, Region, SarifLog, SarifResult, fingerprint};
pub use source::FileCtx;
