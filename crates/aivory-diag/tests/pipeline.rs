//! Tests of the diagnostics pipeline and the command runner.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_diag::linters::run_command;
use aivory_diag::lsp::lsp_types::Position;
use aivory_diag::lsp::{Ctx, Lineage, LspPool};
use aivory_diag::{Budgets, Level, Pipeline, PipelineConfig, TurnInput};
use common::*;

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn pipeline() -> Pipeline {
    Pipeline::new(PipelineConfig {
        linters: false,
        ..PipelineConfig::default()
    })
}

fn input<'a>(root: &'a Path, files: &'a [String]) -> TurnInput<'a> {
    TurnInput {
        repo_root: root,
        changed_files: files,
        contents: None,
        lineage: None,
        session: "s",
    }
}

const CLEAN: &str = "pub fn run(x: Option<i32>) -> i32 {\n    x.unwrap_or(0)\n}\n";
const RISKY: &str = "pub fn run(x: Option<i32>) -> i32 {\n    x.unwrap()\n}\n";
const BROKEN: &str = "pub fn run( -> i32 {\n    1\n}\n";

#[test]
fn the_pipeline_collects_syntax_errors_and_rule_findings() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", RISKY);
    write(dir.path(), "src/b.rs", BROKEN);
    write(
        dir.path(),
        "src/c.py",
        "import os\ndef f(s):\n    return eval(s)\n",
    );
    write(dir.path(), "README.md", "# text\n");
    write(
        dir.path(),
        ".aivory/rules/team.yml",
        "id: team/no-unwrap-or\nlanguage: rust\nseverity: hint\nmessage: unwrap_or found\nrule:\n  pattern: $X.unwrap_or($Y)\n",
    );
    let files: Vec<String> = [
        "src/a.rs",
        "src/b.rs",
        "src/c.py",
        "src/gone.rs",
        "README.md",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let report = pipeline().run(&input(dir.path(), &files));

    let ids: Vec<(&str, &str)> = report
        .results()
        .map(|r| (r.rule_id.as_str(), r.file().unwrap()))
        .collect();
    assert!(
        ids.contains(&("aivory/risky-unwrap", "src/a.rs")),
        "{ids:?}"
    );
    assert!(ids.contains(&("aivory/risky-eval", "src/c.py")), "{ids:?}");
    let syntax: Vec<_> = report
        .results()
        .filter(|r| r.rule_id == "aivory/syntax-error")
        .collect();
    assert!(!syntax.is_empty());
    assert!(
        syntax
            .iter()
            .all(|r| r.file() == Some("src/b.rs") && r.level == Level::Error)
    );
    assert!(report.results().all(|r| r.fingerprint().is_some()));

    let names: Vec<&str> = report.stages.iter().map(|s| s.stage).collect();
    assert_eq!(names, vec!["syntax", "rules", "linters", "lsp"]);
    assert!(report.stages.iter().all(|s| !s.truncated));
    // One run per tool; the SARIF log serializes.
    let text = serde_json::to_string(&report.log).unwrap();
    assert!(text.contains("\"aivory-syntax\"") && text.contains("\"aivory-rules\""));
}

#[test]
fn a_turn_that_adds_errors_gets_errors_added() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", CLEAN);
    let files = vec!["src/a.rs".to_owned()];
    let p = pipeline();
    let before = p.run(&input(dir.path(), &files));
    assert_eq!(before.results().count(), 0);

    // The turn breaks the file.
    write(dir.path(), "src/a.rs", BROKEN);
    let after = p.run(&input(dir.path(), &files));
    let d = before.diff(&after);
    assert!(d.errors_added() >= 1, "{d:?}");
    assert!(d.removed.is_empty());

    // A later turn that fixes the file removes the errors.
    write(dir.path(), "src/a.rs", CLEAN);
    let fixed = p.run(&input(dir.path(), &files));
    let d2 = after.diff(&fixed);
    assert_eq!(d2.added.len(), 0);
    assert_eq!(d2.removed.len(), after.results().count());
}

#[test]
fn a_moved_finding_is_unchanged_in_the_diff() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", RISKY);
    let files = vec!["src/a.rs".to_owned()];
    let p = pipeline();
    let before = p.run(&input(dir.path(), &files));
    write(
        dir.path(),
        "src/a.rs",
        &format!("// header\n// more\n\n{RISKY}"),
    );
    let after = p.run(&input(dir.path(), &files));
    let d = before.diff(&after);
    assert_eq!(d.unchanged.len(), 1, "{d:?}");
    assert!(d.added.is_empty() && d.removed.is_empty());
    assert_eq!(d.unchanged[0].start_line(), Some(5));
}

#[test]
fn contents_override_the_disk() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", CLEAN);
    let files = vec!["src/a.rs".to_owned()];
    let mut contents = HashMap::new();
    contents.insert("src/a.rs".to_owned(), RISKY.to_owned());
    let mut i = input(dir.path(), &files);
    i.contents = Some(&contents);
    let report = pipeline().run(&i);
    assert!(report.results().any(|r| r.rule_id == "aivory/risky-unwrap"));
}

#[test]
fn an_expired_budget_stops_the_stage() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", RISKY);
    write(dir.path(), "src/b.rs", BROKEN);
    let files = vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()];
    let p = Pipeline::new(PipelineConfig {
        linters: false,
        budgets: Budgets {
            syntax: Duration::ZERO,
            rules: Duration::ZERO,
            ..Budgets::default()
        },
        ..PipelineConfig::default()
    });
    let report = p.run(&input(dir.path(), &files));
    assert!(report.stage("syntax").unwrap().truncated);
    assert!(report.stage("rules").unwrap().truncated);
    assert_eq!(report.results().count(), 0);
}

#[test]
fn a_broken_rule_file_is_a_note_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", RISKY);
    write(
        dir.path(),
        ".aivory/rules/bad.yml",
        "id: x\nlanguage: cobol\nrule: {pattern: a}\n",
    );
    let files = vec!["src/a.rs".to_owned()];
    let report = pipeline().run(&input(dir.path(), &files));
    assert!(report.results().any(|r| r.rule_id == "aivory/risky-unwrap"));
    let notes = &report.stage("rules").unwrap().notes;
    assert!(notes.iter().any(|n| n.contains("bad.yml")), "{notes:?}");
}

#[test]
fn lsp_diagnostics_join_the_report_when_a_server_runs() {
    let dir = tempfile::tempdir().unwrap();
    let lineage: Lineage = lineage(dir.path(), "repo");
    write(
        &lineage.root,
        "src/lib.rs",
        "pub fn a() {}\n// FIXME later\n",
    );
    let pool = Arc::new(LspPool::new(
        config(),
        vec![fake_spec(None)],
        Arc::new(FixedProbe(1)),
    ));
    let p = pipeline().with_pool(pool.clone());
    let files = vec!["src/lib.rs".to_owned()];
    let mut i = input(&lineage.root, &files);
    i.lineage = Some(&lineage);

    // No server runs yet: the stage is skipped and no server starts.
    let report = p.run(&i);
    assert_eq!(report.stage("lsp").unwrap().results, 0);
    assert!(pool.running().is_empty());

    // Start the server with a lens query, then run again.
    let ov = overlay("s", &[]);
    pool.hover(
        &Ctx {
            lineage: &lineage,
            overlay: &ov,
        },
        "src/lib.rs",
        Position {
            line: 0,
            character: 0,
        },
    )
    .unwrap();
    let report = p.run(&i);
    let lsp: Vec<_> = report
        .results()
        .filter(|r| r.rule_id == "lsp/fixme")
        .collect();
    assert_eq!(lsp.len(), 1, "{:?}", report.stage("lsp"));
    assert_eq!(lsp[0].start_line(), Some(2));
    assert_eq!(lsp[0].level, Level::Warning);
    assert!(lsp[0].fingerprint().is_some());
}

#[test]
fn run_command_stops_a_slow_command() {
    let started = Instant::now();
    let mut cmd = std::process::Command::new(fake_exe());
    cmd.args(["--sleep", "30"]);
    let out = run_command(&mut cmd, Duration::from_millis(300)).unwrap();
    assert!(out.timed_out);
    assert!(!out.success);
    assert!(started.elapsed() < Duration::from_secs(10));

    let mut cmd = std::process::Command::new(fake_exe());
    cmd.args(["--sleep", "0"]);
    let out = run_command(&mut cmd, Duration::from_secs(20)).unwrap();
    assert!(!out.timed_out);
    assert!(out.success);
}
