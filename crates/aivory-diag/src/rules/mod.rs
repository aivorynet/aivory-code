//! The rule engine: built-in tree rules, the risky-API pack, and ast-grep YAML rules.
//!
//! `ast-grep-core` and `ast-grep-config` (MIT) run the YAML rules. They use tree-sitter 0.27,
//! the version of `aivory-syntax`, so one tree-sitter links and no own rule engine is needed.
//! Rules of a repo live in `<repo>/.aivory/rules/*.yml`.

mod native;
mod risky;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use aivory_syntax::Language;
use ast_grep_config::{GlobalRules, RuleCollection, RuleConfig, Severity, from_yaml_string};
use ast_grep_core::tree_sitter::StrDoc;

use crate::lang::SgLang;
use crate::sarif::{Level, SarifResult};
use crate::source::FileCtx;

pub use native::{
    RULE_COMPLEXITY, RULE_DEEP_NESTING, RULE_DUPLICATE, RULE_LONG_FUNCTION, Thresholds,
};
pub use risky::{RISKY, Risky, is_test_path};

/// The folder of the rules of a repo, relative to the repo root.
pub const RULES_DIR: &str = ".aivory/rules";

/// A rule file that did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleLoadError {
    pub file: String,
    pub message: String,
}

/// A parsed source file. The pipeline parses each file once for all rules.
pub struct ParsedFile {
    pub ctx: Arc<FileCtx>,
    lang: SgLang,
    root: native::Root,
}

impl std::fmt::Debug for ParsedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedFile")
            .field("path", &self.ctx.path)
            .finish()
    }
}

impl ParsedFile {
    /// Parse a file. The result is `None` when the rule engine does not support the language.
    pub fn parse(ctx: Arc<FileCtx>) -> Option<Self> {
        let lang = SgLang::new(ctx.language?)?;
        let doc = StrDoc::try_new(&ctx.source, lang).ok()?;
        Some(Self {
            ctx,
            lang,
            root: ast_grep_core::AstGrep::doc(doc),
        })
    }

    pub fn language(&self) -> Language {
        self.lang.language()
    }
}

/// All rules of a run: the built-in packs and the rules of the repo.
pub struct RuleSet {
    pub thresholds: Thresholds,
    collection: RuleCollection<SgLang>,
    skip_tests: HashSet<String>,
    errors: Vec<RuleLoadError>,
    rule_count: usize,
}

impl std::fmt::Debug for RuleSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleSet")
            .field("rule_count", &self.rule_count)
            .field("errors", &self.errors)
            .finish()
    }
}

impl RuleSet {
    /// The built-in rules only.
    pub fn builtin() -> Self {
        Self::build(Thresholds::default(), Vec::new(), Vec::new())
    }

    /// The built-in rules plus the rules in `<repo>/.aivory/rules/*.yml` (and `*.yaml`).
    /// A broken rule file does not stop the others: see [`RuleSet::errors`].
    pub fn load(repo_root: &Path) -> Self {
        Self::load_with(repo_root, Thresholds::default())
    }

    pub fn load_with(repo_root: &Path, thresholds: Thresholds) -> Self {
        let mut texts = Vec::new();
        let mut errors = Vec::new();
        let dir = repo_root.join(RULES_DIR);
        let mut files: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                        e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml")
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_owned();
            match std::fs::read_to_string(&path) {
                Ok(text) => texts.push((name, text)),
                Err(e) => errors.push(RuleLoadError {
                    file: name,
                    message: e.to_string(),
                }),
            }
        }
        Self::build(thresholds, texts, errors)
    }

    /// Build a set from rule texts: `(file name, YAML)`. Used by [`RuleSet::load`] and by tests.
    pub fn from_yaml(texts: Vec<(String, String)>) -> Self {
        Self::build(Thresholds::default(), texts, Vec::new())
    }

    fn build(
        thresholds: Thresholds,
        texts: Vec<(String, String)>,
        mut errors: Vec<RuleLoadError>,
    ) -> Self {
        let globals = GlobalRules::default();
        let mut configs: Vec<RuleConfig<SgLang>> = Vec::new();
        let builtin = risky::yaml();
        let mut all = vec![("<builtin>".to_owned(), builtin)];
        all.extend(texts);
        for (file, text) in &all {
            match from_yaml_string::<SgLang>(text, &globals) {
                Ok(list) => configs.extend(list),
                Err(e) => errors.push(RuleLoadError {
                    file: file.clone(),
                    message: e.to_string(),
                }),
            }
        }
        let rule_count = configs.len();
        let collection = RuleCollection::try_new(configs).unwrap_or_default();
        Self {
            thresholds,
            collection,
            skip_tests: risky::skip_test_ids().into_iter().collect(),
            errors,
            rule_count,
        }
    }

    /// The rule files that did not load.
    pub fn errors(&self) -> &[RuleLoadError] {
        &self.errors
    }

    /// The number of YAML rules (per language) that loaded.
    pub fn rule_count(&self) -> usize {
        self.rule_count
    }

    /// Run the rules that look at one file: the built-in tree rules and the YAML rules. The scan
    /// stops when `deadline` passes. The result tells whether the scan finished.
    pub fn scan_file(
        &self,
        file: &ParsedFile,
        deadline: Instant,
        out: &mut Vec<SarifResult>,
    ) -> bool {
        if !native::scan_functions(&file.ctx, &file.root, &self.thresholds, deadline, out) {
            return false;
        }
        let path = Path::new(&file.ctx.path);
        let test_path = is_test_path(&file.ctx.path, file.language());
        for rule in self.collection.get_rule_from_lang(path, file.lang) {
            if Instant::now() >= deadline {
                return false;
            }
            let level = match rule.severity {
                Severity::Off => continue,
                Severity::Error => Level::Error,
                Severity::Warning => Level::Warning,
                Severity::Info | Severity::Hint => Level::Note,
            };
            let skip_tests = self.skip_tests.contains(&rule.id);
            if skip_tests && test_path {
                continue;
            }
            for m in file.root.root().find_all(&rule.matcher) {
                if skip_tests && file.language() == Language::Rust && in_rust_test_code(&m) {
                    continue;
                }
                let range = m.range();
                out.push(file.ctx.result_bytes(
                    &rule.id,
                    level,
                    &rule.get_message(&m),
                    range.start,
                    range.end,
                ));
            }
        }
        true
    }

    /// Run the duplicate-block rule over the files together.
    pub fn scan_duplicates(
        &self,
        files: &[ParsedFile],
        deadline: Instant,
        out: &mut Vec<SarifResult>,
    ) -> bool {
        let inputs: Vec<_> = files
            .iter()
            .map(|f| native::DupInput {
                ctx: &f.ctx,
                root: &f.root,
            })
            .collect();
        native::scan_duplicates(&inputs, &self.thresholds, deadline, out)
    }
}

/// True when the node lies in a Rust item with a `test` attribute (`#[test]`, `#[cfg(test)]`,
/// `#[tokio::test]`).
fn in_rust_test_code(node: &native::Node<'_>) -> bool {
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        let mut prev = n.prev();
        while let Some(p) = prev {
            let kind = p.kind();
            if kind == "attribute_item" {
                let text = p.text();
                if text
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .any(|w| w == "test")
                {
                    return true;
                }
            } else if !kind.contains("comment") {
                break;
            }
            prev = p.prev();
        }
        cur = n.parent();
    }
    false
}

#[cfg(test)]
mod tests;
