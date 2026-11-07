//! Transcript lines grouped into blocks: a tool call with its result rows, a run of agent
//! rows, a run of prompt rows. Pure logic; `draw.rs` turns a block into elements.

use std::collections::HashSet;

use aivory_protocol::{Line, LineKind};

/// A tool result shows this many rows before the fold.
pub const FOLD_ROWS: usize = 4;
/// A diff shows this many rows before the fold.
pub const FOLD_DIFF_ROWS: usize = 12;

/// Why the last tool call of a transcript has no result yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pending {
    #[default]
    No,
    /// The session works: the call runs.
    Running,
    /// The session waits for an answer: the call asks for a permission.
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Meta,
    User,
    Agent,
    Subagent,
    Turn,
    Error,
    /// A tool call: the head row (`Tool` line) and its result rows. A run of result rows
    /// without a head (the transcript tail starts in the middle of a call) is a call too.
    Call,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The index of the first line in the full transcript. It names the block for the fold.
    pub abs: u32,
    pub kind: BlockKind,
    pub lines: Vec<Line>,
    pub pending: Pending,
    pub expanded: bool,
}

/// What the state dot of a tool call shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Waiting,
    Done,
    Failed,
}

impl Block {
    /// The head line of a call.
    pub fn head(&self) -> Option<&Line> {
        self.lines.first().filter(|l| l.kind == LineKind::Tool)
    }

    /// The result rows of a call.
    pub fn results(&self) -> &[Line] {
        match self.head() {
            Some(_) => &self.lines[1..],
            None => &self.lines,
        }
    }

    pub fn tool_state(&self) -> ToolState {
        if self.results().iter().any(|l| l.kind == LineKind::Error) {
            return ToolState::Failed;
        }
        match self.pending {
            Pending::Running => ToolState::Running,
            Pending::Waiting => ToolState::Waiting,
            Pending::No => ToolState::Done,
        }
    }
}

fn is_result(kind: LineKind) -> bool {
    matches!(
        kind,
        LineKind::Output | LineKind::Added | LineKind::Removed | LineKind::Error
    )
}

/// Group `lines` (the first one has the index `first` in the full transcript).
pub fn group(lines: &[Line], first: u32, pending: Pending, expanded: &HashSet<u32>) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let kind = lines[i].kind;
        let (block_kind, len) = match kind {
            LineKind::User | LineKind::Agent => {
                (BlockKind::from(kind), run(lines, i, |k| k == kind))
            }
            LineKind::Tool => (BlockKind::Call, 1 + run(lines, i + 1, is_result)),
            LineKind::Output | LineKind::Added | LineKind::Removed => {
                (BlockKind::Call, run(lines, i, is_result))
            }
            LineKind::Error => (BlockKind::Error, run(lines, i, |k| k == LineKind::Error)),
            LineKind::Meta => (BlockKind::Meta, 1),
            LineKind::Subagent => (BlockKind::Subagent, 1),
            LineKind::Turn => (BlockKind::Turn, 1),
        };
        let abs = first + i as u32;
        blocks.push(Block {
            abs,
            kind: block_kind,
            lines: lines[i..i + len].to_vec(),
            pending: Pending::No,
            expanded: expanded.contains(&abs),
        });
        i += len;
    }
    if pending != Pending::No
        && let Some(last) = blocks.last_mut()
        && last.kind == BlockKind::Call
        && last.head().is_some()
        && last.results().is_empty()
    {
        last.pending = pending;
    }
    blocks
}

/// The length of the run of lines from `from` on that `keep` accepts.
fn run(lines: &[Line], from: usize, keep: impl Fn(LineKind) -> bool) -> usize {
    lines[from.min(lines.len())..]
        .iter()
        .take_while(|l| keep(l.kind))
        .count()
}

impl From<LineKind> for BlockKind {
    fn from(kind: LineKind) -> Self {
        match kind {
            LineKind::User => Self::User,
            LineKind::Agent => Self::Agent,
            LineKind::Tool | LineKind::Output | LineKind::Added | LineKind::Removed => Self::Call,
            LineKind::Error => Self::Error,
            LineKind::Meta => Self::Meta,
            LineKind::Subagent => Self::Subagent,
            LineKind::Turn => Self::Turn,
        }
    }
}

/// The marker row at the top of a transcript that has older lines.
pub fn earlier_marker(start: u32) -> Option<String> {
    match start {
        0 => None,
        1 => Some("↑ 1 earlier line".to_string()),
        n => Some(format!("↑ {n} earlier lines")),
    }
}

/// How the result rows of a call show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    /// Rows shown (from the start of the results).
    pub shown: usize,
    /// The rows behind the fold plus the rows that the daemon cut off.
    pub hidden: usize,
    /// More rows than the fold limit: a click on the marker toggles the fold.
    pub foldable: bool,
    pub expanded: bool,
}

impl Fold {
    /// The marker row under the results, if there is one.
    pub fn label(&self) -> Option<String> {
        if self.foldable && self.expanded {
            Some(if self.hidden > 0 {
                format!("… fold · {} more lines were cut", self.hidden)
            } else {
                "… fold".to_string()
            })
        } else if self.hidden > 0 {
            Some(format!(
                "… +{} line{}",
                self.hidden,
                if self.hidden == 1 { "" } else { "s" }
            ))
        } else {
            None
        }
    }
}

/// The rows that the daemon cut off, from its last row `… N more lines`.
pub fn split_cut(results: &[Line]) -> (&[Line], usize) {
    if let Some((last, rest)) = results.split_last()
        && let Some(n) = last
            .text
            .strip_prefix("… ")
            .and_then(|t| t.strip_suffix(" more lines"))
            .and_then(|n| n.parse::<usize>().ok())
    {
        return (rest, n);
    }
    (results, 0)
}

pub fn fold(results: &[Line], expanded: bool) -> Fold {
    let (rows, cut) = split_cut(results);
    let limit = if rows
        .iter()
        .any(|l| matches!(l.kind, LineKind::Added | LineKind::Removed))
    {
        FOLD_DIFF_ROWS
    } else {
        FOLD_ROWS
    };
    let foldable = rows.len() > limit;
    if foldable && !expanded {
        Fold {
            shown: limit,
            hidden: rows.len() - limit + cut,
            foldable,
            expanded,
        }
    } else {
        Fold {
            shown: rows.len(),
            hidden: cut,
            foldable,
            expanded,
        }
    }
}

/// One row of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub sign: DiffSign,
    /// The line number in the file, when it is known.
    pub number: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSign {
    Added,
    Removed,
    Context,
    /// A `@@ -1,2 +3,4 @@` header.
    Hunk,
}

/// The rows of a diff with their line numbers. The numbers come from `@@` headers, and a
/// `Write` call starts at line 1. Else the protocol has no numbers and the column stays empty.
pub fn diff_rows(tool: Option<&str>, rows: &[Line]) -> Vec<DiffRow> {
    let mut old: Option<u32> = None;
    let mut new: Option<u32> = if matches!(tool, Some("Write" | "NotebookEdit"))
        && rows.iter().all(|l| l.kind == LineKind::Added)
    {
        Some(1)
    } else {
        None
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.kind == LineKind::Output
            && let Some((o, n)) = hunk_start(&row.text)
        {
            old = Some(o);
            new = Some(n);
            out.push(DiffRow {
                sign: DiffSign::Hunk,
                number: None,
                text: row.text.clone(),
            });
            continue;
        }
        let (sign, number) = match row.kind {
            LineKind::Added => (DiffSign::Added, bump(&mut new)),
            LineKind::Removed => (DiffSign::Removed, bump(&mut old)),
            _ => {
                bump(&mut old);
                (DiffSign::Context, bump(&mut new))
            }
        };
        out.push(DiffRow {
            sign,
            number,
            text: row.text.clone(),
        });
    }
    out
}

fn bump(n: &mut Option<u32>) -> Option<u32> {
    let now = *n;
    if let Some(v) = n {
        *v += 1;
    }
    now
}

/// The first old and new line of a `@@ -a,b +c,d @@` header.
pub fn hunk_start(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(' ')?;
    let first = |s: &str| s.split(',').next()?.parse::<u32>().ok();
    Some((first(old)?, first(new)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(n: usize) -> Vec<Line> {
        (0..n).map(|i| Line::output(format!("row {i}"))).collect()
    }

    #[test]
    fn a_tool_call_takes_its_result_rows() {
        let mut lines = vec![Line::agent("a"), Line::tool("Bash", "ls")];
        lines.extend(out(2));
        lines.push(Line::added("x"));
        lines.push(Line::subagent("t", "w"));
        lines.push(Line::agent("b"));
        lines.push(Line::agent("c"));
        let blocks = group(&lines, 10, Pending::No, &HashSet::new());
        let kinds: Vec<_> = blocks
            .iter()
            .map(|b| (b.kind, b.lines.len(), b.abs))
            .collect();
        assert_eq!(
            kinds,
            [
                (BlockKind::Agent, 1, 10),
                (BlockKind::Call, 4, 11),
                (BlockKind::Subagent, 1, 15),
                (BlockKind::Agent, 2, 16),
            ]
        );
        assert_eq!(blocks[1].head().unwrap().label.as_deref(), Some("Bash"));
        assert_eq!(blocks[1].results().len(), 3);
    }

    #[test]
    fn a_tail_may_start_with_result_rows() {
        let mut lines = out(2);
        lines.push(Line::tool("Read", "a.rs"));
        let blocks = group(&lines, 0, Pending::No, &HashSet::new());
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].head().is_none());
        assert_eq!(blocks[0].results().len(), 2);
    }

    #[test]
    fn only_a_call_without_result_is_pending() {
        let lines = vec![Line::tool("Bash", "ls")];
        let b = group(&lines, 0, Pending::Running, &HashSet::new());
        assert_eq!(b[0].tool_state(), ToolState::Running);
        let b = group(&lines, 0, Pending::Waiting, &HashSet::new());
        assert_eq!(b[0].tool_state(), ToolState::Waiting);
        let lines = vec![Line::tool("Bash", "ls"), Line::output("x")];
        let b = group(&lines, 0, Pending::Running, &HashSet::new());
        assert_eq!(b[0].tool_state(), ToolState::Done);
        let lines = vec![Line::tool("Bash", "ls"), Line::error("boom")];
        let b = group(&lines, 0, Pending::No, &HashSet::new());
        assert_eq!(b[0].tool_state(), ToolState::Failed);
    }

    #[test]
    fn the_expanded_set_names_blocks_by_their_first_line() {
        let lines = vec![Line::agent("a"), Line::tool("Bash", "ls")];
        let open: HashSet<u32> = [6].into();
        let b = group(&lines, 5, Pending::No, &open);
        assert!(!b[0].expanded);
        assert!(b[1].expanded);
    }

    #[test]
    fn a_long_result_folds_to_four_rows() {
        let f = fold(&out(10), false);
        assert_eq!((f.shown, f.hidden, f.foldable), (4, 6, true));
        assert_eq!(f.label().as_deref(), Some("… +6 lines"));
        let f = fold(&out(10), true);
        assert_eq!((f.shown, f.hidden), (10, 0));
        assert_eq!(f.label().as_deref(), Some("… fold"));
        let f = fold(&out(4), false);
        assert_eq!((f.shown, f.foldable, f.label()), (4, false, None));
        assert_eq!(fold(&out(5), false).label().as_deref(), Some("… +1 line"));
    }

    #[test]
    fn rows_that_the_daemon_cut_off_count_as_hidden() {
        let mut rows = out(3);
        rows.push(Line::output("… 40 more lines"));
        let f = fold(&rows, false);
        assert_eq!((f.shown, f.hidden, f.foldable), (3, 40, false));
        assert_eq!(f.label().as_deref(), Some("… +40 lines"));
        let mut rows = out(6);
        rows.push(Line::output("… 40 more lines"));
        let f = fold(&rows, false);
        assert_eq!((f.shown, f.hidden), (4, 42));
    }

    #[test]
    fn a_diff_folds_later_than_a_result() {
        let rows: Vec<Line> = (0..10).map(|i| Line::added(format!("l{i}"))).collect();
        assert_eq!(fold(&rows, false).shown, 10);
        let rows: Vec<Line> = (0..20).map(|i| Line::added(format!("l{i}"))).collect();
        assert_eq!(fold(&rows, false).shown, FOLD_DIFF_ROWS);
    }

    #[test]
    fn the_earlier_marker_counts_lines() {
        assert_eq!(earlier_marker(0), None);
        assert_eq!(earlier_marker(1).as_deref(), Some("↑ 1 earlier line"));
        assert_eq!(earlier_marker(140).as_deref(), Some("↑ 140 earlier lines"));
    }

    #[test]
    fn a_hunk_header_sets_the_line_numbers() {
        assert_eq!(hunk_start("@@ -12,7 +14,8 @@ fn x"), Some((12, 14)));
        assert_eq!(hunk_start("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(hunk_start("nope"), None);
        let rows = vec![
            Line::output("@@ -10,2 +20,2 @@"),
            Line::removed("old"),
            Line::added("new"),
            Line::output("same"),
        ];
        let d = diff_rows(Some("Edit"), &rows);
        assert_eq!(d[0].sign, DiffSign::Hunk);
        assert_eq!((d[1].sign, d[1].number), (DiffSign::Removed, Some(10)));
        assert_eq!((d[2].sign, d[2].number), (DiffSign::Added, Some(20)));
        assert_eq!((d[3].sign, d[3].number), (DiffSign::Context, Some(21)));
    }

    #[test]
    fn a_write_numbers_from_one_and_an_edit_has_no_numbers() {
        let rows = vec![Line::added("a"), Line::added("b")];
        let d = diff_rows(Some("Write"), &rows);
        assert_eq!([d[0].number, d[1].number], [Some(1), Some(2)]);
        let d = diff_rows(Some("Edit"), &rows);
        assert_eq!([d[0].number, d[1].number], [None, None]);
    }
}
