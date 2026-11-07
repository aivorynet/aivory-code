//! The rows of the Focus editor. Pure logic: the hunks of a file diff, the rows of the File,
//! Diff and Split views, the folders of the explorer, the turn of a hunk. The elements are in
//! `editor.rs`.

use std::collections::BTreeMap;

use aivory_protocol::UnixMs;
use aivory_protocol::session::{DiffFile, DiffLine, DiffLineKind};

/// One hunk of a file diff.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@` line.
    pub header: String,
    /// The turn that wrote it: the blame of its lines, else the transcript.
    pub turn: Option<u32>,
    pub at: Option<UnixMs>,
    /// The first line of the hunk in the new file.
    pub new_start: u32,
    pub added: u32,
    pub removed: u32,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The last line of the hunk in the new file.
    pub fn new_end(&self) -> u32 {
        self.lines
            .iter()
            .filter_map(|l| l.new)
            .max()
            .unwrap_or(self.new_start)
    }
}

/// The start line of the new side in `@@ -a,b +c,d @@`.
pub fn header_new_start(header: &str) -> Option<u32> {
    let plus = header.split_whitespace().find(|w| w.starts_with('+'))?;
    plus[1..].split(',').next()?.parse().ok()
}

/// The hunks of a file. `fallback[i]` is the turn of hunk `i` when the lines carry no blame.
pub fn hunks(file: &DiffFile, fallback: &[u32]) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    for line in &file.lines {
        if line.kind == DiffLineKind::Hunk || out.is_empty() {
            let header = if line.kind == DiffLineKind::Hunk {
                line.text.clone()
            } else {
                String::new()
            };
            out.push(Hunk {
                new_start: header_new_start(&header).unwrap_or(1),
                header,
                turn: None,
                at: None,
                added: 0,
                removed: 0,
                lines: Vec::new(),
            });
            if line.kind == DiffLineKind::Hunk {
                continue;
            }
        }
        let Some(h) = out.last_mut() else { continue };
        match line.kind {
            DiffLineKind::Added => h.added += 1,
            DiffLineKind::Removed => h.removed += 1,
            _ => {}
        }
        if let Some(b) = &line.blame
            && h.turn.is_none()
            && line.kind != DiffLineKind::Context
        {
            h.turn = Some(b.turn);
            h.at = Some(b.at);
        }
        if h.lines.is_empty()
            && let Some(n) = line.new
        {
            h.new_start = n;
        }
        h.lines.push(line.clone());
    }
    for (i, h) in out.iter_mut().enumerate() {
        if h.turn.is_none() {
            h.turn = fallback.get(i).or(fallback.last()).copied();
        }
    }
    out
}

/// What a line shows in the gutter and in the tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Context,
    Added,
    Removed,
}

/// One row of the File or Diff view.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// The header of hunk `usize` of the file.
    Hunk(usize),
    Line {
        no: Option<u32>,
        text: String,
        mark: Mark,
        hunk: Option<usize>,
    },
    /// `n` unchanged lines are hidden.
    Fold(u32),
}

impl Row {
    fn line(no: Option<u32>, text: &str, mark: Mark, hunk: Option<usize>) -> Self {
        Self::Line {
            no,
            text: text.to_string(),
            mark,
            hunk,
        }
    }
}

/// The rows of the File view: the whole text with the hunks in place. The deleted lines sit
/// before the line that follows them. Without a text, the hunks stand alone between folds.
pub fn file_rows(text: Option<&str>, hunks: &[Hunk]) -> Vec<Row> {
    let Some(text) = text else {
        return diff_rows(hunks);
    };
    // For every line of the new file: the rows that come before it.
    let mut before: BTreeMap<u32, Vec<Row>> = BTreeMap::new();
    let mut added: BTreeMap<u32, usize> = BTreeMap::new();
    for (i, h) in hunks.iter().enumerate() {
        let mut next = h.new_start;
        before.entry(h.new_start).or_default().push(Row::Hunk(i));
        for l in &h.lines {
            match l.kind {
                DiffLineKind::Removed => before.entry(next).or_default().push(Row::line(
                    None,
                    &l.text,
                    Mark::Removed,
                    Some(i),
                )),
                DiffLineKind::Added => {
                    let n = l.new.unwrap_or(next);
                    added.insert(n, i);
                    next = n + 1;
                }
                DiffLineKind::Context => next = l.new.unwrap_or(next) + 1,
                DiffLineKind::Hunk => {}
            }
        }
    }
    let mut rows = Vec::new();
    let mut total = 0;
    for (ix, line) in text.split('\n').enumerate() {
        let no = ix as u32 + 1;
        total = no;
        if let Some(extra) = before.remove(&no) {
            rows.extend(extra);
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (mark, hunk) = match added.get(&no) {
            Some(h) => (Mark::Added, Some(*h)),
            None => (Mark::Context, None),
        };
        rows.push(Row::line(Some(no), line, mark, hunk));
    }
    // Rows after the last line (a deletion at the end of the file).
    for (n, extra) in before {
        if n > total {
            rows.extend(extra);
        }
    }
    rows
}

/// The rows of the Diff view: every hunk with its lines, folds between the hunks.
pub fn diff_rows(hunks: &[Hunk]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut last_end: Option<u32> = None;
    for (i, h) in hunks.iter().enumerate() {
        match last_end {
            Some(end) if h.new_start > end + 1 => rows.push(Row::Fold(h.new_start - end - 1)),
            None if h.new_start > 1 => rows.push(Row::Fold(h.new_start - 1)),
            _ => {}
        }
        rows.push(Row::Hunk(i));
        for l in &h.lines {
            let (mark, no) = match l.kind {
                DiffLineKind::Added => (Mark::Added, l.new),
                DiffLineKind::Removed => (Mark::Removed, None),
                _ => (Mark::Context, l.new),
            };
            if l.kind != DiffLineKind::Hunk {
                rows.push(Row::line(no, &l.text, mark, Some(i)));
            }
        }
        last_end = Some(h.new_end());
    }
    rows
}

/// The rows of the File view without a diff: every line of the text.
pub fn plain_rows(text: &str) -> Vec<Row> {
    text.split('\n')
        .enumerate()
        .map(|(i, l)| {
            Row::line(
                Some(i as u32 + 1),
                l.strip_suffix('\r').unwrap_or(l),
                Mark::Context,
                None,
            )
        })
        .collect()
}

/// The row of the header of hunk `hunk`.
pub fn row_of_hunk(rows: &[Row], hunk: usize) -> Option<usize> {
    rows.iter().position(|r| *r == Row::Hunk(hunk))
}

/// What one side of a split row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Context,
    Added,
    Removed,
    /// No line on this side: the hatched filler.
    Blank,
}

/// One row of the Split view: the old side and the new side.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitRow {
    pub left: (Option<u32>, String, Side),
    pub right: (Option<u32>, String, Side),
}

/// The rows of the Split view. A run of deleted lines pairs with the added lines after it.
pub fn split_rows(hunks: &[Hunk]) -> Vec<SplitRow> {
    let mut rows = Vec::new();
    for h in hunks {
        let mut old = Vec::new();
        let mut new = Vec::new();
        let flush = |old: &mut Vec<(Option<u32>, String)>,
                     new: &mut Vec<(Option<u32>, String)>,
                     rows: &mut Vec<SplitRow>| {
            for i in 0..old.len().max(new.len()) {
                let l = old.get(i);
                let r = new.get(i);
                rows.push(SplitRow {
                    left: match l {
                        Some((n, t)) => (*n, t.clone(), Side::Removed),
                        None => (None, String::new(), Side::Blank),
                    },
                    right: match r {
                        Some((n, t)) => (*n, t.clone(), Side::Added),
                        None => (None, String::new(), Side::Blank),
                    },
                });
            }
            old.clear();
            new.clear();
        };
        for l in &h.lines {
            match l.kind {
                DiffLineKind::Removed => old.push((l.old, l.text.clone())),
                DiffLineKind::Added => new.push((l.new, l.text.clone())),
                DiffLineKind::Context => {
                    flush(&mut old, &mut new, &mut rows);
                    rows.push(SplitRow {
                        left: (l.old, l.text.clone(), Side::Context),
                        right: (l.new, l.text.clone(), Side::Context),
                    });
                }
                DiffLineKind::Hunk => {}
            }
        }
        flush(&mut old, &mut new, &mut rows);
    }
    rows
}

/// The turns that changed a file, each once, oldest first.
pub fn turns_of(hunks: &[Hunk]) -> Vec<u32> {
    let mut turns: Vec<u32> = hunks.iter().filter_map(|h| h.turn).collect();
    turns.sort_unstable();
    turns.dedup();
    turns
}

/// A row of the explorer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub name: String,
    /// The path of a file; `None` for a folder.
    pub path: Option<String>,
}

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: Vec<String>,
}

/// The folders of the explorer for these paths. A folder with one folder and no file in it
/// joins its child (`crates/orch/src`).
pub fn tree(paths: &[String]) -> Vec<TreeRow> {
    let mut root = Dir::default();
    for p in paths {
        let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        let Some((file, dirs)) = parts.split_last() else {
            continue;
        };
        let mut at = &mut root;
        for d in dirs {
            at = at.dirs.entry((*d).to_string()).or_default();
        }
        if !at.files.iter().any(|f| f == file) {
            at.files.push((*file).to_string());
        }
    }
    let mut rows = Vec::new();
    walk(&root, "", 0, &mut rows);
    rows
}

fn walk(dir: &Dir, prefix: &str, depth: usize, rows: &mut Vec<TreeRow>) {
    for (name, child) in &dir.dirs {
        let (mut label, mut node) = (name.clone(), child);
        while node.files.is_empty() && node.dirs.len() == 1 {
            let (n, c) = node.dirs.iter().next().expect("one folder");
            label = format!("{label}/{n}");
            node = c;
        }
        rows.push(TreeRow {
            depth,
            name: label.clone(),
            path: None,
        });
        walk(node, &format!("{prefix}{label}/"), depth + 1, rows);
    }
    let mut files = dir.files.clone();
    files.sort();
    for f in files {
        rows.push(TreeRow {
            depth,
            path: Some(format!("{prefix}{f}")),
            name: f,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dl(kind: DiffLineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old,
            new,
            text: text.to_string(),
            blame: None,
        }
    }

    fn sample() -> DiffFile {
        DiffFile {
            path: "src/a.rs".into(),
            added: 2,
            removed: 1,
            lines: vec![
                dl(DiffLineKind::Hunk, None, None, "@@ -2,3 +2,4 @@"),
                dl(DiffLineKind::Context, Some(2), Some(2), "two"),
                dl(DiffLineKind::Removed, Some(3), None, "three"),
                dl(DiffLineKind::Added, None, Some(3), "THREE"),
                dl(DiffLineKind::Added, None, Some(4), "THREE B"),
                dl(DiffLineKind::Context, Some(4), Some(5), "four"),
                dl(DiffLineKind::Hunk, None, None, "@@ -9,1 +10,1 @@"),
                dl(DiffLineKind::Added, None, Some(10), "ten"),
            ],
        }
    }

    #[test]
    fn a_header_gives_the_new_start() {
        assert_eq!(header_new_start("@@ -2,3 +7,4 @@ fn x"), Some(7));
        assert_eq!(header_new_start("@@ -2 +9 @@"), Some(9));
        assert_eq!(header_new_start("nonsense"), None);
    }

    #[test]
    fn hunks_count_lines_and_take_the_fallback_turn() {
        let h = hunks(&sample(), &[2, 5]);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].added, h[0].removed, h[0].new_start), (2, 1, 2));
        assert_eq!(h[0].new_end(), 5);
        assert_eq!((h[0].turn, h[1].turn), (Some(2), Some(5)));
        assert_eq!(turns_of(&h), vec![2, 5]);
        // One turn for many hunks: the last turn repeats.
        assert_eq!(hunks(&sample(), &[4])[1].turn, Some(4));
        assert_eq!(hunks(&sample(), &[])[0].turn, None);
    }

    #[test]
    fn the_file_view_puts_deleted_lines_before_the_next_line() {
        let text = "one\ntwo\nTHREE\nTHREE B\nfour\nfive\nsix\nseven\neight\nten";
        let rows = file_rows(Some(text), &hunks(&sample(), &[]));
        let h0 = row_of_hunk(&rows, 0).expect("hunk 0");
        assert!(matches!(&rows[h0 + 1], Row::Line { no: Some(2), .. }));
        assert!(matches!(
            &rows[h0 + 2],
            Row::Line { mark: Mark::Removed, text, .. } if text == "three"
        ));
        assert!(matches!(
            &rows[h0 + 3],
            Row::Line {
                no: Some(3),
                mark: Mark::Added,
                hunk: Some(0),
                ..
            }
        ));
        assert!(row_of_hunk(&rows, 1).is_some());
        // The text holds 10 lines, the diff adds 1 removed row and 2 hunk rows.
        assert_eq!(rows.len(), 13);
    }

    #[test]
    fn the_diff_view_folds_the_gap_between_hunks() {
        let rows = diff_rows(&hunks(&sample(), &[]));
        assert_eq!(rows[0], Row::Fold(1));
        assert!(rows.contains(&Row::Fold(4)));
        assert_eq!(file_rows(None, &hunks(&sample(), &[])), rows);
    }

    #[test]
    fn the_split_view_pairs_deleted_and_added_lines() {
        let rows = split_rows(&hunks(&sample(), &[]));
        assert_eq!(rows[0].left.2, Side::Context);
        assert_eq!(rows[1].left.2, Side::Removed);
        assert_eq!(rows[1].right.2, Side::Added);
        assert_eq!(rows[2].left.2, Side::Blank);
        assert_eq!(rows[2].right.1, "THREE B");
    }

    #[test]
    fn the_explorer_joins_single_folders() {
        let paths: Vec<String> = [
            "crates/orch/src/budget.rs",
            "crates/orch/src/spawn.rs",
            "tests/budget.rs",
            "Cargo.toml",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let rows = tree(&paths);
        assert_eq!(rows[0].name, "crates/orch/src");
        assert_eq!(rows[0].path, None);
        assert_eq!(rows[1].path.as_deref(), Some("crates/orch/src/budget.rs"));
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[3].name, "tests");
        assert_eq!(
            rows.last().and_then(|r| r.path.as_deref()),
            Some("Cargo.toml")
        );
    }
}
