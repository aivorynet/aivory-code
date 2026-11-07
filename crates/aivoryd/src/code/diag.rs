//! The diagnostics pipeline at the end of a turn, and the tables `diag_run` and
//! `diag_finding` (before and after the turn) in `aivory.db`.
//!
//! The "before" state is the stage 1 and 2 result (syntax, rules) on the text of the files in
//! the snapshot before the turn. The "after" state is the full pipeline (also the linters and
//! the language servers that run) on the files now. `added` and `removed` compare only the tools
//! that ran on both sides ([`COMPARABLE`]).

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use aivory_diag::{Level, Pipeline, PipelineConfig, SarifResult, TurnInput};
use aivory_protocol::code::{Finding, FindingLevel};
use aivory_vcs::{GitVcs, Vcs};
use rusqlite::{Connection, params};

use super::state::{CodeCore, TurnEnd};

/// The tools whose findings exist before and after a turn.
pub const COMPARABLE: [&str; 2] = ["aivory-syntax", "aivory-rules"];
/// The most files that one turn checks.
const MAX_FILES: usize = 200;
const BUSY: Duration = Duration::from_secs(5);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS diag_run (
  session_id TEXT NOT NULL,
  n INTEGER NOT NULL,
  at INTEGER NOT NULL,
  files TEXT NOT NULL,
  notes TEXT NOT NULL,
  PRIMARY KEY (session_id, n)
);
CREATE TABLE IF NOT EXISTS diag_finding (
  session_id TEXT NOT NULL,
  n INTEGER NOT NULL,
  side TEXT NOT NULL,
  tool TEXT NOT NULL,
  rule TEXT NOT NULL,
  level TEXT NOT NULL,
  message TEXT NOT NULL,
  path TEXT NOT NULL,
  line INTEGER NOT NULL,
  col INTEGER NOT NULL,
  end_line INTEGER,
  end_col INTEGER,
  fingerprint TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS diag_finding_turn ON diag_finding (session_id, n, side);
";

fn open(db: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db).map_err(|e| e.to_string())?;
    conn.busy_timeout(BUSY).map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    Ok(conn)
}

/// The text of files: from a map first, then from the disk below `root`.
struct Texts<'a> {
    root: &'a Path,
    over: &'a HashMap<String, String>,
    cache: HashMap<String, Option<String>>,
}

impl<'a> Texts<'a> {
    fn new(root: &'a Path, over: &'a HashMap<String, String>) -> Self {
        Self {
            root,
            over,
            cache: HashMap::new(),
        }
    }

    fn line(&mut self, path: &str, line: u32) -> String {
        let text = self.cache.entry(path.to_owned()).or_insert_with(|| {
            self.over
                .get(path)
                .cloned()
                .or_else(|| std::fs::read_to_string(self.root.join(path)).ok())
        });
        text.as_deref()
            .and_then(|t| t.lines().nth((line as usize).saturating_sub(1)))
            .unwrap_or("")
            .to_owned()
    }
}

/// A column of SARIF (from 1, in characters) as a byte offset from 0.
fn byte_col(line: &str, col1: u32) -> u32 {
    let chars = (col1 as usize).saturating_sub(1);
    let bytes: usize = line.chars().take(chars).map(char::len_utf8).sum();
    // A column past the end of the line (or an unknown line) keeps the character count.
    let beyond = chars.saturating_sub(line.chars().count());
    u32::try_from(bytes + beyond).unwrap_or(u32::MAX)
}

fn level(l: Level) -> FindingLevel {
    match l {
        Level::None => FindingLevel::None,
        Level::Note => FindingLevel::Note,
        Level::Warning => FindingLevel::Warning,
        Level::Error => FindingLevel::Error,
    }
}

fn finding(tool: &str, r: &SarifResult, texts: &mut Texts<'_>) -> Option<Finding> {
    let path = r.file()?.to_owned();
    let region = r.region()?;
    let col = |line: u32, c: Option<u32>, texts: &mut Texts<'_>| {
        c.map(|c| {
            let text = texts.line(&path, line);
            byte_col(&text, c)
        })
    };
    let end_line = region.end_line;
    Some(Finding {
        rule: r.rule_id.clone(),
        level: level(r.level),
        message: r.message.text.clone(),
        line: region.start_line,
        col: col(region.start_line, region.start_column, texts).unwrap_or(0),
        end_line,
        end_col: col(
            end_line.unwrap_or(region.start_line),
            region.end_column,
            texts,
        ),
        tool: tool.to_owned(),
        fingerprint: r.fingerprint().unwrap_or("").to_owned(),
        path,
    })
}

fn findings(report: &aivory_diag::PipelineReport, texts: &mut Texts<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    for run in &report.log.runs {
        let tool = run.tool.driver.name.as_str();
        out.extend(run.results.iter().filter_map(|r| finding(tool, r, texts)));
    }
    out
}

fn notes(report: &aivory_diag::PipelineReport) -> Vec<String> {
    let mut out = Vec::new();
    for s in &report.stages {
        out.push(format!(
            "{}: {} result{}{}",
            s.stage,
            s.results,
            if s.results == 1 { "" } else { "s" },
            if s.truncated {
                " (cut by the time budget)"
            } else {
                ""
            },
        ));
        out.extend(s.notes.iter().map(|n| format!("{}: {n}", s.stage)));
    }
    out
}

fn key(f: &Finding) -> (String, String, String) {
    let id = if f.fingerprint.is_empty() {
        f.message.clone()
    } else {
        f.fingerprint.clone()
    };
    (f.rule.clone(), f.path.clone(), id)
}

fn comparable(f: &&Finding) -> bool {
    COMPARABLE.contains(&f.tool.as_str())
}

/// The change from `before` to `after` for the tools in [`COMPARABLE`]: matched by fingerprint,
/// as `aivory_diag::diff` does.
pub fn diff_rows(before: &[Finding], after: &[Finding]) -> (Vec<Finding>, Vec<Finding>) {
    let mut pool: HashMap<_, Vec<&Finding>> = HashMap::new();
    for f in before.iter().filter(comparable) {
        pool.entry(key(f)).or_default().push(f);
    }
    let mut added = Vec::new();
    for f in after.iter().filter(comparable) {
        if pool.get_mut(&key(f)).and_then(Vec::pop).is_none() {
            added.push(f.clone());
        }
    }
    let removed = pool.into_values().flatten().cloned().collect();
    (added, removed)
}

fn level_text(l: FindingLevel) -> &'static str {
    match l {
        FindingLevel::None => "none",
        FindingLevel::Note => "note",
        FindingLevel::Warning => "warning",
        FindingLevel::Error => "error",
    }
}

fn level_from(text: &str) -> FindingLevel {
    match text {
        "error" => FindingLevel::Error,
        "warning" => FindingLevel::Warning,
        "note" => FindingLevel::Note,
        _ => FindingLevel::None,
    }
}

/// Run the pipeline for the files of a turn and store the findings before and after.
pub fn run_turn(core: &CodeCore, t: &TurnEnd) -> Result<(), String> {
    let files: Vec<String> = t.files.iter().take(MAX_FILES).cloned().collect();
    let vcs = GitVcs::new();
    // The text of the files in the snapshot before the turn. A new file has none.
    let mut before_files = Vec::new();
    let mut contents: HashMap<String, String> = HashMap::new();
    if let Some(parent) = &t.parent_snapshot {
        for f in &files {
            if let Ok(bytes) = vcs.show_file(&t.workdir, parent, f)
                && let Ok(text) = String::from_utf8(bytes)
            {
                contents.insert(f.clone(), text);
                before_files.push(f.clone());
            }
        }
    }
    let fast = Pipeline::new(PipelineConfig {
        linters: false,
        ..PipelineConfig::default()
    });
    let before = fast.run(&TurnInput {
        repo_root: &t.workdir,
        changed_files: &before_files,
        contents: Some(&contents),
        lineage: None,
        session: &t.session,
    });
    // The text after the turn is the snapshot of the turn, not the folder now: the session
    // can have gone on. A file that the snapshot lacks was deleted. The linters read the folder.
    let mut after_files = Vec::new();
    let mut after_contents: HashMap<String, String> = HashMap::new();
    match &t.snapshot {
        Some(snap) => {
            for f in &files {
                if let Ok(bytes) = vcs.show_file(&t.workdir, snap, f)
                    && let Ok(text) = String::from_utf8(bytes)
                {
                    after_contents.insert(f.clone(), text);
                    after_files.push(f.clone());
                }
            }
        }
        None => after_files.clone_from(&files),
    }
    let lineage = CodeCore::lineage(&t.workdir);
    let after = Pipeline::new(PipelineConfig::default())
        .with_pool(core.pool().clone())
        .run(&TurnInput {
            repo_root: &t.workdir,
            changed_files: &after_files,
            contents: Some(&after_contents),
            lineage: Some(&lineage),
            session: &t.session,
        });
    let before_rows = findings(&before, &mut Texts::new(&t.workdir, &contents));
    let after_rows = findings(&after, &mut Texts::new(&t.workdir, &after_contents));
    let (added, removed) = diff_rows(&before_rows, &after_rows);
    let mut run_notes = notes(&after);
    if t.files.len() > files.len() {
        run_notes.push(format!(
            "only the first {MAX_FILES} of {} files were checked",
            t.files.len()
        ));
    }
    store(
        core.db(),
        t,
        &files,
        &run_notes,
        &before_rows,
        &after_rows,
        (added.len(), removed.len()),
    )
}

fn store(
    db: &Path,
    t: &TurnEnd,
    files: &[String],
    notes: &[String],
    before: &[Finding],
    after: &[Finding],
    counts: (usize, usize),
) -> Result<(), String> {
    let mut conn = open(db)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM diag_finding WHERE session_id = ?1 AND n = ?2",
        params![t.session, t.n],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR REPLACE INTO diag_run (session_id, n, at, files, notes) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![t.session, t.n, t.ended_ms, files.join("\n"), notes.join("\n")],
    )
    .map_err(|e| e.to_string())?;
    {
        let mut ins = tx
            .prepare(
                "INSERT INTO diag_finding (session_id, n, side, tool, rule, level, message, path, \
                 line, col, end_line, end_col, fingerprint) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )
            .map_err(|e| e.to_string())?;
        for (side, rows) in [("before", before), ("after", after)] {
            for f in rows {
                ins.execute(params![
                    t.session,
                    t.n,
                    side,
                    f.tool,
                    f.rule,
                    level_text(f.level),
                    f.message,
                    f.path,
                    f.line,
                    f.col,
                    f.end_line,
                    f.end_col,
                    f.fingerprint,
                ])
                .map_err(|e| e.to_string())?;
            }
        }
    }
    // The turn row of the ledger keeps the counts. The table exists when the turn is recorded.
    let _ = tx.execute(
        "UPDATE turn SET diagnostics_added = ?3, diagnostics_removed = ?4 \
         WHERE session_id = ?1 AND n = ?2",
        params![t.session, t.n, counts.0 as i64, counts.1 as i64],
    );
    tx.commit().map_err(|e| e.to_string())
}

/// The stored findings of one turn.
#[derive(Debug, Clone, Default)]
pub struct Stored {
    pub n: u32,
    pub rows: Vec<Finding>,
    pub added: Vec<Finding>,
    pub removed: Vec<Finding>,
    pub notes: Vec<String>,
}

fn rows_of(conn: &Connection, session: &str, n: u32, side: &str) -> Result<Vec<Finding>, String> {
    let mut st = conn
        .prepare(
            "SELECT tool, rule, level, message, path, line, col, end_line, end_col, fingerprint \
             FROM diag_finding WHERE session_id = ?1 AND n = ?2 AND side = ?3 \
             ORDER BY path, line, col, rule",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![session, n, side], |r| {
            Ok(Finding {
                tool: r.get(0)?,
                rule: r.get(1)?,
                level: level_from(&r.get::<_, String>(2)?),
                message: r.get(3)?,
                path: r.get(4)?,
                line: r.get(5)?,
                col: r.get(6)?,
                end_line: r.get(7)?,
                end_col: r.get(8)?,
                fingerprint: r.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// The findings of a turn of a session (the last turn with a run when `turn` is `None`).
pub fn load(db: &Path, session: &str, turn: Option<u32>) -> Result<Option<Stored>, String> {
    if !db.exists() {
        return Ok(None);
    }
    let conn = open(db)?;
    let run: Option<(u32, String)> = match turn {
        Some(n) => conn.query_row(
            "SELECT n, notes FROM diag_run WHERE session_id = ?1 AND n = ?2",
            params![session, n],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ),
        None => conn.query_row(
            "SELECT n, notes FROM diag_run WHERE session_id = ?1 ORDER BY n DESC LIMIT 1",
            params![session],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ),
    }
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.to_string()),
    })?;
    let Some((n, notes)) = run else {
        return Ok(None);
    };
    let rows = rows_of(&conn, session, n, "after")?;
    let before = rows_of(&conn, session, n, "before")?;
    let (added, removed) = diff_rows(&before, &rows);
    Ok(Some(Stored {
        n,
        rows,
        added,
        removed,
        notes: notes.lines().map(str::to_owned).collect(),
    }))
}

/// The findings of one file in the last run of the session that checked it.
pub fn latest_for_file(db: &Path, session: &str, path: &str) -> Result<Vec<Finding>, String> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let conn = open(db)?;
    let n: Option<u32> = conn
        .query_row(
            "SELECT max(n) FROM diag_run WHERE session_id = ?1 AND \
             instr(char(10) || files || char(10), char(10) || ?2 || char(10)) > 0",
            params![session, path],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    match n {
        Some(n) => Ok(rows_of(&conn, session, n, "after")?
            .into_iter()
            .filter(|f| f.path == path)
            .collect()),
        None => Ok(Vec::new()),
    }
}

/// Run the fast stages (syntax and rules) on files of a checkout.
pub fn quick(root: &Path, files: &[String]) -> (Vec<Finding>, Vec<String>) {
    let pipeline = Pipeline::new(PipelineConfig {
        linters: false,
        ..PipelineConfig::default()
    });
    let files: Vec<String> = files.iter().take(MAX_FILES).cloned().collect();
    let report = pipeline.run(&TurnInput {
        repo_root: root,
        changed_files: &files,
        contents: None,
        lineage: None,
        session: "",
    });
    let none = HashMap::new();
    let rows = findings(&report, &mut Texts::new(root, &none));
    let notes = notes(&report)
        .into_iter()
        .filter(|n| !n.contains("linters are off"))
        .collect();
    (rows, notes)
}

/// The paths with changes in the working tree of `root` (git status).
pub fn changed_paths(root: &Path) -> Vec<String> {
    GitVcs::new()
        .status(root)
        .map(|list| {
            list.into_iter()
                .filter(|e| e.worktree != 'D' && e.index != 'D')
                .map(|e| e.path)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(tool: &str, rule: &str, fp: &str) -> Finding {
        Finding {
            rule: rule.into(),
            level: FindingLevel::Warning,
            message: "m".into(),
            path: "a.rs".into(),
            line: 1,
            col: 0,
            end_line: None,
            end_col: None,
            tool: tool.into(),
            fingerprint: fp.into(),
        }
    }

    #[test]
    fn byte_columns_follow_the_text() {
        assert_eq!(byte_col("abc", 2), 1);
        assert_eq!(byte_col("äb", 2), 2);
        assert_eq!(byte_col("", 5), 4);
    }

    #[test]
    fn the_diff_compares_only_the_shared_tools() {
        let before = vec![f("aivory-rules", "r1", "x"), f("aivory-rules", "r2", "y")];
        let after = vec![
            f("aivory-rules", "r1", "x"),
            f("aivory-rules", "r3", "z"),
            f("clippy", "c1", "q"),
        ];
        let (added, removed) = diff_rows(&before, &after);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].rule, "r3");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].rule, "r2");
    }
}
