//! The diagnostics pipeline: run after each turn, in this order, and stop a stage when its time
//! budget ends (`docs/design/03-code-intelligence.md`, "Diagnostics pipeline").
//!
//! 1. Syntax errors of `aivory-syntax` (50 ms).
//! 2. Rules (500 ms): the built-in quality rules, the risky-API pack, and the ast-grep YAML rules
//!    of `<repo>/.aivory/rules`.
//! 3. Linters of the project (clippy, ruff, eslint, golangci-lint), when installed, for the
//!    changed files.
//! 4. LSP diagnostics, when a language server runs for the lineage.
//!
//! The result is a SARIF log with one run per tool. The daemon runs the pipeline before and
//! after a turn and compares the two reports ([`PipelineReport::diff`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_syntax::Language;
use lsp_types::NumberOrString;

use crate::compare::{Diff, diff};
use crate::linters::{self, Linter, RawFinding};
use crate::lsp::{Ctx, Lineage, LspPool, Overlay, OverlayFile};
use crate::rules::{ParsedFile, RuleSet, Thresholds};
use crate::sarif::{Level, Region, Run, SarifLog, SarifResult, disambiguate};
use crate::source::FileCtx;

pub const RULE_SYNTAX_ERROR: &str = "aivory/syntax-error";

/// The time budget of each stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    pub syntax: Duration,
    pub rules: Duration,
    pub linters: Duration,
    pub lsp: Duration,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            syntax: Duration::from_millis(50),
            rules: Duration::from_millis(500),
            linters: Duration::from_secs(30),
            lsp: Duration::from_secs(3),
        }
    }
}

/// The settings of the pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub budgets: Budgets,
    pub thresholds: Thresholds,
    /// Run the linters of stage 3.
    pub linters: bool,
    /// A file larger than this is skipped (bytes).
    pub max_file_bytes: u64,
    /// The `PATH` where the linters are searched.
    pub path_var: std::ffi::OsString,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            budgets: Budgets::default(),
            thresholds: Thresholds::default(),
            linters: true,
            max_file_bytes: 2 * 1024 * 1024,
            path_var: linters::process_path(),
        }
    }
}

/// What the pipeline works on after a turn.
#[derive(Debug, Clone, Copy)]
pub struct TurnInput<'a> {
    /// The root of the session's working tree. The files are read from here.
    pub repo_root: &'a Path,
    /// The changed files, relative to `repo_root`, with `/` as separator.
    pub changed_files: &'a [String],
    /// The text of files that is not on disk (or differs from the disk), by path.
    pub contents: Option<&'a HashMap<String, String>>,
    /// The lineage for the LSP pool. Without it stage 4 is skipped.
    pub lineage: Option<&'a Lineage>,
    pub session: &'a str,
}

/// What one stage did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageReport {
    pub stage: &'static str,
    pub elapsed: Duration,
    /// True when the budget ended before the stage finished.
    pub truncated: bool,
    pub results: usize,
    /// Facts for the user: skipped tools, rule files that did not load, files that failed.
    pub notes: Vec<String>,
}

/// The result of one pipeline run.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineReport {
    pub log: SarifLog,
    pub stages: Vec<StageReport>,
}

impl PipelineReport {
    /// All results.
    pub fn results(&self) -> impl Iterator<Item = &SarifResult> {
        self.log.results()
    }

    /// The change from `self` (the state before a turn) to `after` (the state after the turn).
    pub fn diff(&self, after: &PipelineReport) -> Diff {
        diff(self.log.results(), after.log.results())
    }

    pub fn stage(&self, name: &str) -> Option<&StageReport> {
        self.stages.iter().find(|s| s.stage == name)
    }
}

/// The pipeline.
#[derive(Debug)]
pub struct Pipeline {
    cfg: PipelineConfig,
    pool: Option<Arc<LspPool>>,
}

impl Pipeline {
    pub fn new(cfg: PipelineConfig) -> Self {
        Self { cfg, pool: None }
    }

    /// Use an LSP pool for stage 4.
    pub fn with_pool(mut self, pool: Arc<LspPool>) -> Self {
        self.pool = Some(pool);
        self
    }

    pub fn config(&self) -> &PipelineConfig {
        &self.cfg
    }

    fn load(&self, input: &TurnInput<'_>, notes: &mut Vec<String>) -> Vec<Arc<FileCtx>> {
        let mut out = Vec::new();
        for rel in input.changed_files {
            let rel = rel.replace('\\', "/");
            let text = match input.contents.and_then(|c| c.get(&rel)) {
                Some(t) => t.clone(),
                None => {
                    let path = input.repo_root.join(&rel);
                    match std::fs::metadata(&path) {
                        Ok(m) if m.len() > self.cfg.max_file_bytes => {
                            notes.push(format!("{rel}: file is too large"));
                            continue;
                        }
                        Ok(_) => {}
                        // A deleted file has no findings.
                        Err(_) => continue,
                    }
                    match std::fs::read_to_string(&path) {
                        Ok(t) => t,
                        Err(_) => {
                            notes.push(format!("{rel}: not a text file"));
                            continue;
                        }
                    }
                }
            };
            let language = Language::detect(&rel);
            out.push(Arc::new(FileCtx::new(&rel, text, language)));
        }
        out
    }

    /// Run the pipeline for the changed files of a turn.
    pub fn run(&self, input: &TurnInput<'_>) -> PipelineReport {
        let mut log = SarifLog::default();
        let mut stages = Vec::new();
        let mut load_notes = Vec::new();
        let loaded = self.load(input, &mut load_notes);

        // Stage 1: syntax errors.
        let started = Instant::now();
        let deadline = started + self.cfg.budgets.syntax;
        let mut run = Run::new("aivory-syntax");
        let mut truncated = false;
        for f in &loaded {
            let Some(lang) = f.language else { continue };
            if Instant::now() >= deadline {
                truncated = true;
                break;
            }
            let Ok(errors) = aivory_syntax::syntax_errors(lang, &f.source) else {
                continue;
            };
            for e in errors {
                run.results.push(f.result_bytes(
                    RULE_SYNTAX_ERROR,
                    Level::Error,
                    &e.message,
                    e.range.start_byte as usize,
                    e.range.end_byte as usize,
                ));
            }
        }
        finish_run(&mut run);
        stages.push(StageReport {
            stage: "syntax",
            elapsed: started.elapsed(),
            truncated,
            results: run.results.len(),
            notes: load_notes,
        });
        log.runs.push(run);

        // Stage 2: rules.
        let started = Instant::now();
        let deadline = started + self.cfg.budgets.rules;
        let mut run = Run::new("aivory-rules");
        let rules = RuleSet::load_with(input.repo_root, self.cfg.thresholds);
        let mut notes: Vec<String> = rules
            .errors()
            .iter()
            .map(|e| format!("rule file {}: {}", e.file, e.message))
            .collect();
        let mut truncated = false;
        let mut parsed: Vec<ParsedFile> = Vec::new();
        for f in &loaded {
            let Some(p) = ParsedFile::parse(f.clone()) else {
                continue;
            };
            if !truncated && !rules.scan_file(&p, deadline, &mut run.results) {
                truncated = true;
            }
            parsed.push(p);
        }
        if !truncated && !rules.scan_duplicates(&parsed, deadline, &mut run.results) {
            truncated = true;
        }
        finish_run(&mut run);
        stages.push(StageReport {
            stage: "rules",
            elapsed: started.elapsed(),
            truncated,
            results: run.results.len(),
            notes: std::mem::take(&mut notes),
        });
        log.runs.push(run);

        // The contexts for stages 3 and 4.
        let ctxs: HashMap<String, &FileCtx> =
            loaded.iter().map(|c| (c.path.clone(), &**c)).collect();

        // Stage 3: linters.
        let started = Instant::now();
        let mut notes = Vec::new();
        let mut truncated = false;
        let mut linter_runs: Vec<Run> = Vec::new();
        if self.cfg.linters {
            let found = linters::detect(input.repo_root, &self.cfg.path_var);
            let timeout = self.cfg.budgets.linters;
            let changed: Vec<String> = input
                .changed_files
                .iter()
                .map(|f| f.replace('\\', "/"))
                .collect();
            let outcomes: Vec<linters::LinterOutcome> = std::thread::scope(|scope| {
                let handles: Vec<_> = found
                    .iter()
                    .map(|d| {
                        let changed = &changed;
                        scope.spawn(move || {
                            linters::run_linter(d, input.repo_root, changed, timeout)
                        })
                    })
                    .collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            for o in outcomes {
                if o.timed_out {
                    truncated = true;
                }
                if let Some(n) = &o.note
                    && !n.starts_with("no changed file")
                {
                    notes.push(n.clone());
                }
                if o.note
                    .as_deref()
                    .is_some_and(|n| n.starts_with("no changed file"))
                {
                    continue;
                }
                let mut run = Run::new(o.linter.name());
                for f in o.findings {
                    run.results.push(raw_to_result(input, &ctxs, &f));
                }
                finish_run(&mut run);
                linter_runs.push(run);
            }
            if found.is_empty() {
                notes.push("no linter found".to_owned());
            }
        } else {
            notes.push("linters are off".to_owned());
        }
        stages.push(StageReport {
            stage: "linters",
            elapsed: started.elapsed(),
            truncated,
            results: linter_runs.iter().map(|r| r.results.len()).sum(),
            notes,
        });
        log.runs.extend(linter_runs);

        // Stage 4: LSP diagnostics from servers that run.
        let started = Instant::now();
        let mut notes = Vec::new();
        let mut run = Run::new("lsp");
        let mut truncated = false;
        if let (Some(pool), Some(lineage)) = (&self.pool, input.lineage) {
            let overlay = Overlay::new(
                input.session,
                ctxs.values()
                    .map(|c| OverlayFile {
                        path: c.path.clone(),
                        text: c.source.clone(),
                    })
                    .collect(),
            );
            let mut by_lang: HashMap<Language, Vec<String>> = HashMap::new();
            for c in ctxs.values() {
                if let Some(l) = c.language {
                    by_lang.entry(l).or_default().push(c.path.clone());
                }
            }
            let deadline = started + self.cfg.budgets.lsp;
            for (lang, mut files) in by_lang {
                if !pool.is_running(lineage, lang) {
                    continue;
                }
                files.sort();
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    truncated = true;
                    break;
                }
                let ctx = Ctx {
                    lineage,
                    overlay: &overlay,
                };
                match pool.diagnostics_within(&ctx, &files, left) {
                    Ok(list) => {
                        for fd in list {
                            let Some(fc) = ctxs.get(&fd.file) else {
                                continue;
                            };
                            for d in fd.diagnostics {
                                run.results.push(lsp_to_result(fc, &d));
                            }
                        }
                    }
                    Err(e) => notes.push(format!("lsp {}: {e}", lang.name())),
                }
            }
        } else {
            notes.push("no language server pool".to_owned());
        }
        finish_run(&mut run);
        stages.push(StageReport {
            stage: "lsp",
            elapsed: started.elapsed(),
            truncated,
            results: run.results.len(),
            notes,
        });
        log.runs.push(run);

        PipelineReport { log, stages }
    }
}

/// Sort the results of a run and make the fingerprints unique.
fn finish_run(run: &mut Run) {
    run.results.sort_by(|a, b| {
        (a.file(), a.start_line(), &a.rule_id).cmp(&(b.file(), b.start_line(), &b.rule_id))
    });
    disambiguate(&mut run.results);
}

fn raw_to_result(
    input: &TurnInput<'_>,
    ctxs: &HashMap<String, &FileCtx>,
    f: &RawFinding,
) -> SarifResult {
    let region = Region {
        start_line: f.line.max(1),
        start_column: Some(f.column.max(1)),
        end_line: f.end_line,
        end_column: f.end_column,
    };
    if let Some(ctx) = ctxs.get(&f.file) {
        return ctx.result(&f.rule, f.level, &f.message, region);
    }
    // A file that the rule stage did not parse (a language without a rule grammar, for example
    // eslint on a `.mts` file): read it for the fingerprint.
    let path: PathBuf = input.repo_root.join(&f.file);
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let ctx = FileCtx::new(&f.file, text, Language::detect(&f.file));
    ctx.result(&f.rule, f.level, &f.message, region)
}

fn lsp_to_result(ctx: &FileCtx, d: &lsp_types::Diagnostic) -> SarifResult {
    let level = match d.severity {
        Some(lsp_types::DiagnosticSeverity::ERROR) | None => Level::Error,
        Some(lsp_types::DiagnosticSeverity::WARNING) => Level::Warning,
        _ => Level::Note,
    };
    let code = match &d.code {
        Some(NumberOrString::Number(n)) => n.to_string(),
        Some(NumberOrString::String(s)) => s.clone(),
        None => d.source.clone().unwrap_or_else(|| "diagnostic".to_owned()),
    };
    let (sl, el) = (d.range.start.line as usize, d.range.end.line as usize);
    let region = Region::span(
        sl as u32 + 1,
        ctx.utf16_to_col(sl, d.range.start.character),
        el as u32 + 1,
        ctx.utf16_to_col(el, d.range.end.character),
    );
    ctx.result(&format!("lsp/{code}"), level, &d.message, region)
}

/// The linters that the pipeline can run, for a settings screen.
pub fn known_linters() -> [Linter; 4] {
    Linter::ALL
}
