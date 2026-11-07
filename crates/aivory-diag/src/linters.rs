//! Linters of the project: clippy, ruff, eslint, and golangci-lint.
//!
//! The pipeline runs a linter only when its binary is on `PATH` (or in the project, for eslint
//! and ruff) and the project has files for it. A linter runs once for the project, with a
//! timeout, and the pipeline keeps only the findings in the changed files. The parsers turn the
//! JSON output of the linters into [`RawFinding`]s, and the pipeline turns those into SARIF.
//!
//! The parsers of clippy, ruff, and eslint follow the documented JSON of those tools. The parser
//! of golangci-lint follows the v1 and v2 JSON (`Issues` list) and is not tested against a real
//! binary yet.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::sarif::Level;

/// The linters that the pipeline knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Linter {
    Clippy,
    Ruff,
    Eslint,
    GolangciLint,
}

impl Linter {
    pub const ALL: [Linter; 4] = [Self::Clippy, Self::Ruff, Self::Eslint, Self::GolangciLint];

    /// The name of the tool in the SARIF run.
    pub fn name(self) -> &'static str {
        match self {
            Self::Clippy => "clippy",
            Self::Ruff => "ruff",
            Self::Eslint => "eslint",
            Self::GolangciLint => "golangci-lint",
        }
    }

    /// True when a changed file is a file of this linter.
    pub fn handles(self, path: &str) -> bool {
        let ext = path.rsplit('.').next().unwrap_or("");
        match self {
            Self::Clippy => ext == "rs",
            Self::Ruff => matches!(ext, "py" | "pyi"),
            Self::Eslint => matches!(
                ext,
                "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts"
            ),
            Self::GolangciLint => ext == "go",
        }
    }
}

/// A finding of a linter, before it becomes SARIF. Lines and columns start at 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFinding {
    pub rule: String,
    pub level: Level,
    pub message: String,
    /// The path relative to the repo root, with `/` as separator.
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

/// A linter that was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub linter: Linter,
    pub exe: PathBuf,
}

/// Look for `name` in the folders of `path_var`. On Windows the extensions of `PATHEXT` count.
pub fn which_in(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_owned())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };
    for dir in std::env::split_paths(path_var) {
        let plain = dir.join(name);
        if cfg!(not(windows)) && plain.is_file() {
            return Some(plain);
        }
        for ext in &exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Find the linters that the project can use: the binary exists and the project has the files
/// for it. `path_var` is the value of `PATH` (use `std::env::var_os("PATH")`).
pub fn detect(repo_root: &Path, path_var: &OsStr) -> Vec<Detected> {
    let mut out = Vec::new();
    // clippy: `cargo clippy` needs `cargo` and `cargo-clippy`.
    if repo_root.join("Cargo.toml").is_file()
        && which_in("cargo-clippy", path_var).is_some()
        && let Some(cargo) = which_in("cargo", path_var)
    {
        out.push(Detected {
            linter: Linter::Clippy,
            exe: cargo,
        });
    }
    // ruff: in the virtual environment of the project first, then on PATH.
    let venv_ruff = ["Scripts", "bin"].iter().find_map(|d| {
        let dir = repo_root.join(".venv").join(d);
        which_in("ruff", dir.as_os_str())
    });
    if let Some(exe) = venv_ruff.or_else(|| which_in("ruff", path_var)) {
        out.push(Detected {
            linter: Linter::Ruff,
            exe,
        });
    }
    // eslint: `node_modules/.bin` of the project first.
    let local_eslint = which_in(
        "eslint",
        repo_root.join("node_modules").join(".bin").as_os_str(),
    );
    if let Some(exe) = local_eslint.or_else(|| which_in("eslint", path_var)) {
        out.push(Detected {
            linter: Linter::Eslint,
            exe,
        });
    }
    if repo_root.join("go.mod").is_file()
        && let Some(exe) = which_in("golangci-lint", path_var)
    {
        out.push(Detected {
            linter: Linter::GolangciLint,
            exe,
        });
    }
    out
}

/// The output of a command.
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub timed_out: bool,
}

/// Run a command and wait at most `timeout`. After the timeout the command is killed.
pub fn run_command(cmd: &mut Command, timeout: Duration) -> std::io::Result<CmdOutput> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_end(&mut buf);
        }
        buf
    });
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_end(&mut buf);
        }
        buf
    });
    let deadline = Instant::now() + timeout;
    let (success, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status.success(), false);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break (false, true);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    Ok(CmdOutput {
        stdout,
        stderr,
        success,
        timed_out,
    })
}

/// The outcome of one linter run.
#[derive(Debug, Clone)]
pub struct LinterOutcome {
    pub linter: Linter,
    /// Findings in the changed files only.
    pub findings: Vec<RawFinding>,
    /// A reason when the linter did not run or failed.
    pub note: Option<String>,
    pub timed_out: bool,
}

/// Run one linter for the changed files (paths relative to the repo root) and parse its output.
pub fn run_linter(
    found: &Detected,
    repo_root: &Path,
    changed: &[String],
    timeout: Duration,
) -> LinterOutcome {
    let files: Vec<&String> = changed.iter().filter(|f| found.linter.handles(f)).collect();
    let mut outcome = LinterOutcome {
        linter: found.linter,
        findings: Vec::new(),
        note: None,
        timed_out: false,
    };
    if files.is_empty() {
        outcome.note = Some("no changed file for this linter".to_owned());
        return outcome;
    }
    let mut cmd = Command::new(&found.exe);
    cmd.current_dir(repo_root);
    match found.linter {
        Linter::Clippy => {
            cmd.args(["clippy", "--message-format=json", "--quiet"]);
        }
        Linter::Ruff => {
            cmd.args(["check", "--output-format", "json", "--no-cache", "--"]);
            cmd.args(&files);
        }
        Linter::Eslint => {
            cmd.args(["--format", "json", "--"]);
            cmd.args(&files);
        }
        Linter::GolangciLint => {
            let v2 = Command::new(&found.exe)
                .arg("--version")
                .stdin(Stdio::null())
                .output()
                .map(|o| {
                    let t = String::from_utf8_lossy(&o.stdout).into_owned();
                    t.contains("version 2") || t.contains("version v2")
                })
                .unwrap_or(false);
            if v2 {
                cmd.args([
                    "run",
                    "--output.json.path=stdout",
                    "--show-stats=false",
                    "./...",
                ]);
            } else {
                cmd.args(["run", "--out-format", "json", "./..."]);
            }
        }
    }
    let output = match run_command(&mut cmd, timeout) {
        Ok(o) => o,
        Err(e) => {
            outcome.note = Some(format!("cannot start {}: {e}", found.linter.name()));
            return outcome;
        }
    };
    if output.timed_out {
        outcome.timed_out = true;
        outcome.note = Some(format!("{} timed out", found.linter.name()));
        return outcome;
    }
    let all = match found.linter {
        Linter::Clippy => parse_clippy(&output.stdout, repo_root),
        Linter::Ruff => parse_ruff(&output.stdout, repo_root),
        Linter::Eslint => parse_eslint(&output.stdout, repo_root),
        Linter::GolangciLint => parse_golangci(&output.stdout, repo_root),
    };
    match all {
        Ok(list) => {
            outcome.findings = list
                .into_iter()
                .filter(|f| files.iter().any(|c| same_path(c, &f.file)))
                .collect();
        }
        Err(e) => {
            outcome.note = Some(format!(
                "cannot read the output of {}: {e}",
                found.linter.name()
            ));
        }
    }
    outcome
}

fn same_path(a: &str, b: &str) -> bool {
    let n = |s: &str| {
        s.replace('\\', "/")
            .trim_start_matches("./")
            .to_ascii_lowercase()
    };
    n(a) == n(b)
}

/// A path of a linter as a path relative to the repo root, with `/` as separator.
pub fn relativize(repo_root: &Path, file: &str) -> String {
    let p = Path::new(file);
    let rel = if p.is_absolute() {
        p.strip_prefix(repo_root)
            .map(Path::to_path_buf)
            .or_else(|_| {
                // `C:\x` against `c:\x` and the `\\?\` prefix on Windows.
                let norm = |s: &str| s.replace('\\', "/").trim_start_matches("//?/").to_owned();
                let (f, r) = (norm(file), norm(&repo_root.to_string_lossy()));
                match f.to_ascii_lowercase().strip_prefix(&r.to_ascii_lowercase()) {
                    Some(rest) => Ok(PathBuf::from(
                        f[f.len() - rest.len()..].trim_start_matches('/'),
                    )),
                    None => Err(()),
                }
            })
            .unwrap_or_else(|_| p.to_path_buf())
    } else {
        p.to_path_buf()
    };
    rel.to_string_lossy().replace('\\', "/")
}

/// The JSON object or array in a text that has other output around it.
fn json_slice(text: &str) -> &str {
    let start = text.find(['{', '[']).unwrap_or(0);
    let end = text.rfind(['}', ']']).map_or(text.len(), |i| i + 1);
    &text[start..end.max(start)]
}

// ---- clippy ----------------------------------------------------------------------------------

#[derive(Deserialize)]
struct CargoLine {
    reason: Option<String>,
    message: Option<RustcMessage>,
}

#[derive(Deserialize)]
struct RustcMessage {
    message: String,
    level: String,
    code: Option<RustcCode>,
    #[serde(default)]
    spans: Vec<RustcSpan>,
}

#[derive(Deserialize)]
struct RustcCode {
    code: String,
}

#[derive(Deserialize)]
struct RustcSpan {
    file_name: String,
    line_start: u32,
    line_end: u32,
    column_start: u32,
    column_end: u32,
    is_primary: bool,
}

/// Parse the output of `cargo clippy --message-format=json` (one JSON object per line).
pub fn parse_clippy(output: &str, repo_root: &Path) -> Result<Vec<RawFinding>, String> {
    let mut out = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(l) = serde_json::from_str::<CargoLine>(line) else {
            continue;
        };
        if l.reason.as_deref() != Some("compiler-message") {
            continue;
        }
        let Some(m) = l.message else { continue };
        let level = match m.level.as_str() {
            "error" | "error: internal compiler error" => Level::Error,
            "warning" => Level::Warning,
            _ => continue,
        };
        let Some(span) = m.spans.iter().find(|s| s.is_primary) else {
            continue;
        };
        let rule = match m.code.as_ref().map(|c| c.code.as_str()) {
            Some(c) if c.contains("::") => c.to_owned(),
            Some(c) => format!("rustc::{c}"),
            None => "rustc::error".to_owned(),
        };
        out.push(RawFinding {
            rule,
            level,
            message: m.message,
            file: relativize(repo_root, &span.file_name),
            line: span.line_start,
            column: span.column_start,
            end_line: Some(span.line_end),
            end_column: Some(span.column_end),
        });
    }
    Ok(out)
}

// ---- ruff ------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct RuffItem {
    code: Option<String>,
    message: String,
    filename: String,
    location: RowCol,
    end_location: Option<RowCol>,
}

#[derive(Deserialize)]
struct RowCol {
    row: u32,
    column: u32,
}

/// Parse the output of `ruff check --output-format json`.
pub fn parse_ruff(output: &str, repo_root: &Path) -> Result<Vec<RawFinding>, String> {
    let text = json_slice(output);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<RuffItem> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok(items
        .into_iter()
        .map(|i| RawFinding {
            level: if i.code.is_none() {
                Level::Error
            } else {
                Level::Warning
            },
            rule: format!("ruff/{}", i.code.as_deref().unwrap_or("syntax-error")),
            message: i.message,
            file: relativize(repo_root, &i.filename),
            line: i.location.row,
            column: i.location.column,
            end_line: i.end_location.as_ref().map(|e| e.row),
            end_column: i.end_location.as_ref().map(|e| e.column),
        })
        .collect())
}

// ---- eslint ----------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EslintFile {
    file_path: String,
    #[serde(default)]
    messages: Vec<EslintMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EslintMessage {
    rule_id: Option<String>,
    severity: u8,
    message: String,
    line: Option<u32>,
    column: Option<u32>,
    end_line: Option<u32>,
    end_column: Option<u32>,
}

/// Parse the output of `eslint --format json`.
pub fn parse_eslint(output: &str, repo_root: &Path) -> Result<Vec<RawFinding>, String> {
    let text = json_slice(output);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let files: Vec<EslintFile> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for f in files {
        let file = relativize(repo_root, &f.file_path);
        for m in f.messages {
            out.push(RawFinding {
                level: if m.severity >= 2 {
                    Level::Error
                } else {
                    Level::Warning
                },
                rule: format!("eslint/{}", m.rule_id.as_deref().unwrap_or("parse-error")),
                message: m.message,
                file: file.clone(),
                line: m.line.unwrap_or(1),
                column: m.column.unwrap_or(1),
                end_line: m.end_line,
                end_column: m.end_column,
            });
        }
    }
    Ok(out)
}

// ---- golangci-lint ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GolangciOut {
    issues: Option<Vec<GolangciIssue>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GolangciIssue {
    from_linter: String,
    text: String,
    #[serde(default)]
    severity: String,
    pos: GolangciPos,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GolangciPos {
    filename: String,
    line: u32,
    #[serde(default)]
    column: u32,
}

/// Parse the JSON of `golangci-lint run` (v1 `--out-format json`, v2 `--output.json.path`).
pub fn parse_golangci(output: &str, repo_root: &Path) -> Result<Vec<RawFinding>, String> {
    let text = json_slice(output);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed: GolangciOut = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok(parsed
        .issues
        .unwrap_or_default()
        .into_iter()
        .map(|i| RawFinding {
            level: if i.severity.eq_ignore_ascii_case("error") {
                Level::Error
            } else {
                Level::Warning
            },
            rule: format!("golangci/{}", i.from_linter),
            message: i.text,
            file: relativize(repo_root, &i.pos.filename),
            line: i.pos.line,
            column: i.pos.column.max(1),
            end_line: None,
            end_column: None,
        })
        .collect())
}

/// The value of `PATH` of this process.
pub fn process_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\work\repo")
        } else {
            PathBuf::from("/work/repo")
        }
    }

    fn abs(rel: &str) -> String {
        root().join(rel).to_string_lossy().into_owned()
    }

    #[test]
    fn clippy_json_lines_become_findings() {
        let out = [
            r#"{"reason":"compiler-artifact","package_id":"x"}"#,
            r#"{"reason":"compiler-message","message":{"message":"this `return` statement is unneeded","level":"warning","code":{"code":"clippy::needless_return","explanation":null},"spans":[{"file_name":"src/lib.rs","line_start":10,"line_end":10,"column_start":5,"column_end":13,"is_primary":true}],"children":[]}}"#,
            r#"{"reason":"compiler-message","message":{"message":"unused variable: `x`","level":"warning","code":{"code":"unused_variables"},"spans":[{"file_name":"src/other.rs","line_start":3,"line_end":3,"column_start":9,"column_end":10,"is_primary":true},{"file_name":"src/other.rs","line_start":1,"line_end":1,"column_start":1,"column_end":2,"is_primary":false}]}}"#,
            r#"{"reason":"compiler-message","message":{"message":"mismatched types","level":"error","code":{"code":"E0308"},"spans":[{"file_name":"src/lib.rs","line_start":4,"line_end":4,"column_start":1,"column_end":2,"is_primary":true}]}}"#,
            r#"{"reason":"compiler-message","message":{"message":"1 warning emitted","level":"warning","code":null,"spans":[]}}"#,
            r#"{"reason":"compiler-message","message":{"message":"help text","level":"help","code":null,"spans":[{"file_name":"a.rs","line_start":1,"line_end":1,"column_start":1,"column_end":2,"is_primary":true}]}}"#,
            "not json",
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        let list = parse_clippy(&out, &root()).unwrap();
        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(list[0].rule, "clippy::needless_return");
        assert_eq!(list[0].level, Level::Warning);
        assert_eq!(
            (list[0].file.as_str(), list[0].line, list[0].column),
            ("src/lib.rs", 10, 5)
        );
        assert_eq!(list[1].rule, "rustc::unused_variables");
        assert_eq!(list[1].line, 3);
        assert_eq!(list[2].level, Level::Error);
        assert_eq!(list[2].rule, "rustc::E0308");
    }

    #[test]
    fn ruff_json_becomes_findings() {
        let out = format!(
            r#"[{{"cell":null,"code":"F401","end_location":{{"column":10,"row":1}},"filename":{},"fix":null,"location":{{"column":8,"row":1}},"message":"`os` imported but unused","noqa_row":1,"url":"u"}},
            {{"cell":null,"code":null,"end_location":{{"column":2,"row":5}},"filename":{},"fix":null,"location":{{"column":1,"row":5}},"message":"SyntaxError: x","noqa_row":null,"url":null}}]"#,
            serde_json::to_string(&abs("pkg/a.py")).unwrap(),
            serde_json::to_string(&abs("pkg/b.py")).unwrap(),
        );
        let list = parse_ruff(&out, &root()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].rule, "ruff/F401");
        assert_eq!(list[0].file, "pkg/a.py");
        assert_eq!(
            (list[0].line, list[0].column, list[0].end_column),
            (1, 8, Some(10))
        );
        assert_eq!(list[0].level, Level::Warning);
        assert_eq!(list[1].rule, "ruff/syntax-error");
        assert_eq!(list[1].level, Level::Error);
        assert!(parse_ruff("", &root()).unwrap().is_empty());
        assert!(parse_ruff("[]", &root()).unwrap().is_empty());
        assert!(parse_ruff("{oops", &root()).is_err());
    }

    #[test]
    fn eslint_json_becomes_findings() {
        let out = format!(
            r#"[{{"filePath":{},"messages":[
              {{"ruleId":"no-unused-vars","severity":2,"message":"'x' is defined but never used.","line":3,"column":7,"endLine":3,"endColumn":8}},
              {{"ruleId":"semi","severity":1,"message":"Missing semicolon.","line":4,"column":10}},
              {{"ruleId":null,"severity":2,"message":"Parsing error: Unexpected token","line":9,"column":1}}],
              "errorCount":2,"warningCount":1}},
              {{"filePath":{},"messages":[],"errorCount":0,"warningCount":0}}]"#,
            serde_json::to_string(&abs("src/a.ts")).unwrap(),
            serde_json::to_string(&abs("src/b.ts")).unwrap(),
        );
        let list = parse_eslint(&out, &root()).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].rule, "eslint/no-unused-vars");
        assert_eq!(list[0].level, Level::Error);
        assert_eq!(list[0].file, "src/a.ts");
        assert_eq!(list[1].level, Level::Warning);
        assert_eq!(list[2].rule, "eslint/parse-error");
    }

    #[test]
    fn golangci_json_becomes_findings() {
        let out = r#"{"Issues":[{"FromLinter":"govet","Text":"printf: bad verb","Severity":"","SourceLines":["x"],"Pos":{"Filename":"cmd/main.go","Offset":10,"Line":12,"Column":3}},
            {"FromLinter":"errcheck","Text":"unchecked","Severity":"error","Pos":{"Filename":"a.go","Line":1,"Column":0}}],"Report":{}}"#;
        let list = parse_golangci(out, &root()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].rule, "golangci/govet");
        assert_eq!(
            (list[0].file.as_str(), list[0].line, list[0].column),
            ("cmd/main.go", 12, 3)
        );
        assert_eq!(list[1].level, Level::Error);
        assert_eq!(list[1].column, 1);
        let none = parse_golangci(r#"{"Issues":null}"#, &root()).unwrap();
        assert!(none.is_empty());
        // Text around the JSON (v2 prints a summary line) is ignored.
        let wrapped = format!("note\n{out}\n");
        assert_eq!(parse_golangci(&wrapped, &root()).unwrap().len(), 2);
    }

    #[test]
    fn handles_files_by_extension() {
        assert!(Linter::Clippy.handles("src/a.rs"));
        assert!(!Linter::Clippy.handles("a.py"));
        assert!(Linter::Eslint.handles("a/b.tsx"));
        assert!(Linter::Ruff.handles("x.py"));
        assert!(Linter::GolangciLint.handles("x.go"));
    }

    #[test]
    fn relativize_handles_absolute_and_relative_paths() {
        assert_eq!(relativize(&root(), &abs("a/b.rs")), "a/b.rs");
        assert_eq!(relativize(&root(), "a\\b.rs"), "a/b.rs");
        if cfg!(windows) {
            assert_eq!(relativize(&root(), r"c:\WORK\repo\x\y.py"), "x/y.py");
        }
    }

    #[test]
    fn detect_finds_binaries_on_the_given_path() {
        let bin = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let name = |n: &str| {
            if cfg!(windows) {
                format!("{n}.exe")
            } else {
                n.to_owned()
            }
        };
        for n in ["cargo", "cargo-clippy", "ruff", "golangci-lint"] {
            std::fs::write(bin.path().join(name(n)), "").unwrap();
        }
        std::fs::write(repo.path().join("Cargo.toml"), "").unwrap();
        let path = std::env::join_paths([bin.path()]).unwrap();
        let found = detect(repo.path(), &path);
        let kinds: Vec<_> = found.iter().map(|d| d.linter).collect();
        // No go.mod: no golangci-lint. No eslint binary.
        assert_eq!(kinds, vec![Linter::Clippy, Linter::Ruff]);
        std::fs::write(repo.path().join("go.mod"), "").unwrap();
        // A project-local eslint wins.
        let local = repo.path().join("node_modules").join(".bin");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join(name("eslint")), "").unwrap();
        let found = detect(repo.path(), &path);
        let eslint = found.iter().find(|d| d.linter == Linter::Eslint).unwrap();
        assert!(eslint.exe.starts_with(&local));
        assert!(found.iter().any(|d| d.linter == Linter::GolangciLint));
        // Nothing on an empty PATH.
        assert!(
            detect(repo.path(), OsStr::new(""))
                .iter()
                .all(|d| d.linter == Linter::Eslint)
        );
    }
}
