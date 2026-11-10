//! Transcript lines as calm display lines for the themed looks. The agent text of one message
//! is parsed as markdown, so tables, code fences, headings and emphasis never show their raw
//! markers (`**`, `|---|`, ```` ``` ````). No drawing here: each look draws the lines in its own
//! grammar.

use aivory_protocol::{Line, LineKind};

use super::markdown::{self, MdBlock};

/// What a display line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DKind {
    /// A prompt of the owner.
    You,
    /// Prose of the agent.
    Agent,
    /// A markdown heading of the agent.
    Heading,
    /// A list item of the agent (`• text` or `1. text`).
    Bullet,
    /// A line of a code block.
    Code,
    /// A table row, cells padded to the column widths. `head` marks the head row.
    Table,
    /// A tool call; `label` holds the tool (`Edit`, `Bash`).
    Tool,
    /// The output of a tool.
    Output,
    Error,
    /// Turn marks, subagent notes, diff lines and other meta text.
    Meta,
}

/// One line on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct DLine {
    pub kind: DKind,
    pub text: String,
    /// The first line of a message: the place for a speaker label. The other lines of the
    /// message carry no label.
    pub first: bool,
    /// The head row of a table.
    pub head: bool,
    pub label: Option<String>,
}

impl DLine {
    fn new(kind: DKind, text: impl Into<String>, first: bool) -> Self {
        Self {
            kind,
            text: text.into(),
            first,
            head: false,
            label: None,
        }
    }

    /// The speaker of a message line: `You`, `Agent`, the tool label, or `None`.
    pub fn speaker(&self) -> Option<&str> {
        match self.kind {
            DKind::You => Some("You"),
            DKind::Agent | DKind::Heading | DKind::Bullet | DKind::Code | DKind::Table => {
                Some("Agent")
            }
            DKind::Tool => self.label.as_deref(),
            DKind::Error => Some("Error"),
            DKind::Output | DKind::Meta => None,
        }
    }
}

/// All display lines of a tail, oldest first.
pub fn lines(tail: &[Line]) -> Vec<DLine> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tail.len() {
        let kind = tail[i].kind;
        if matches!(kind, LineKind::Agent | LineKind::User) {
            // One message = consecutive lines of the same speaker.
            let mut j = i;
            while j < tail.len() && tail[j].kind == kind {
                j += 1;
            }
            let text = tail[i..j]
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let start = out.len();
            if kind == LineKind::User {
                for t in text.lines().filter(|t| !t.trim().is_empty()) {
                    out.push(DLine::new(DKind::You, t.trim_end(), false));
                }
            } else {
                for b in &markdown::cached(&text).blocks {
                    block(b, &mut out);
                }
            }
            if let Some(first) = out.get_mut(start) {
                first.first = true;
            }
            i = j;
            continue;
        }
        let l = &tail[i];
        let kind = match l.kind {
            LineKind::Tool => DKind::Tool,
            LineKind::Output => DKind::Output,
            LineKind::Error => DKind::Error,
            _ => DKind::Meta,
        };
        if !l.text.trim().is_empty() || kind == DKind::Tool {
            let mut d = DLine::new(kind, l.text.trim_end(), true);
            d.label = l.label.clone();
            out.push(d);
        }
        i += 1;
    }
    out
}

/// The newest `n` display lines. The list starts at the first line of a message when one is
/// in the first half, so the view never opens in the middle of a table or a code block.
pub fn last(tail: &[Line], n: usize) -> Vec<DLine> {
    let all = lines(tail);
    let skip = all.len().saturating_sub(n);
    let mut v = all[skip..].to_vec();
    if let Some(at) = v.iter().take(n / 2 + 1).position(|d| d.first)
        && at > 0
    {
        v.drain(..at);
    }
    v
}

/// The newest `n` lines for a short preview ("last steps", a detail card, an event list):
/// prose, headings, list items, prompts, tool calls and errors only. Table rows, code lines,
/// tool output and meta lines are left out, so a small preview never shows a table fragment.
pub fn steps(tail: &[Line], n: usize) -> Vec<DLine> {
    let mut v: Vec<DLine> = lines(tail)
        .into_iter()
        .filter(|d| {
            matches!(
                d.kind,
                DKind::You
                    | DKind::Agent
                    | DKind::Heading
                    | DKind::Bullet
                    | DKind::Tool
                    | DKind::Error
            ) && !d.text.trim().is_empty()
        })
        .collect();
    let skip = v.len().saturating_sub(n);
    v.drain(..skip);
    v
}

/// The plain one-line text of a pane line (a status line, a feed row): markdown markers
/// removed, the first line only.
pub fn plain_line(text: &str) -> String {
    let md = markdown::cached(text);
    let mut out = Vec::new();
    for b in &md.blocks {
        block(b, &mut out);
    }
    out.into_iter()
        .map(|d| d.text)
        .find(|t| !t.trim().is_empty())
        .unwrap_or_default()
}

fn block(b: &MdBlock, out: &mut Vec<DLine>) {
    match b {
        MdBlock::Heading { text, .. } => out.push(DLine::new(DKind::Heading, &text.text, false)),
        MdBlock::Para(text) => {
            for t in text.text.lines().filter(|t| !t.trim().is_empty()) {
                out.push(DLine::new(DKind::Agent, t, false));
            }
        }
        MdBlock::Code { text, .. } => {
            for t in text.lines() {
                out.push(DLine::new(DKind::Code, t, false));
            }
        }
        MdBlock::Quote(blocks) => {
            for b in blocks {
                block(b, out);
            }
        }
        MdBlock::List { start, items } => {
            for (k, item) in items.iter().enumerate() {
                let mark = match (item.task, start) {
                    (Some(true), _) => "[x] ".to_string(),
                    (Some(false), _) => "[ ] ".to_string(),
                    (None, Some(s)) => format!("{}. ", s + k as u64),
                    (None, None) => "• ".to_string(),
                };
                let mut inner = Vec::new();
                for b in &item.blocks {
                    block(b, &mut inner);
                }
                for (m, mut d) in inner.into_iter().enumerate() {
                    d.text = if m == 0 {
                        format!("{mark}{}", d.text)
                    } else {
                        format!("{}{}", " ".repeat(mark.chars().count()), d.text)
                    };
                    if d.kind == DKind::Agent {
                        d.kind = DKind::Bullet;
                    }
                    out.push(d);
                }
            }
        }
        MdBlock::Table { head, rows, widths } => {
            // A plain text row: one long cell must not push all other columns far apart, so a
            // column is at most TABLE_COL characters and a longer cell ends with an ellipsis.
            let row = |cells: Vec<&str>| {
                cells
                    .iter()
                    .enumerate()
                    .map(|(c, t)| {
                        let w = widths.get(c).copied().unwrap_or(0).min(TABLE_COL);
                        let cell = if t.chars().count() > w {
                            let cut: String = t.chars().take(w.saturating_sub(1)).collect();
                            format!("{cut}…")
                        } else {
                            (*t).to_string()
                        };
                        format!("{cell:<w$}")
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
                    .trim_end()
                    .to_string()
            };
            let mut h = DLine::new(
                DKind::Table,
                row(head.iter().map(|c| c.text.as_str()).collect()),
                false,
            );
            h.head = true;
            out.push(h);
            for r in rows {
                out.push(DLine::new(
                    DKind::Table,
                    row(r.iter().map(|c| c.text.as_str()).collect()),
                    false,
                ));
            }
        }
        MdBlock::Rule => {}
    }
}

/// The widest column of a plain text table row, in characters.
const TABLE_COL: usize = 28;
#[cfg(test)]
mod tests {
    use super::*;

    fn agent(t: &str) -> Line {
        Line::new(LineKind::Agent, t)
    }

    #[test]
    fn markdown_markers_do_not_show() {
        let tail = vec![
            Line::new(LineKind::User, "explain the retry"),
            agent("## Retry design"),
            agent("The handler retries with **exponential backoff**."),
            agent("| Attempt | Delay |"),
            agent("|---|---|"),
            agent("| 1 | 250 ms |"),
            agent("```rust"),
            agent("fn delay() {}"),
            agent("```"),
        ];
        let v = lines(&tail);
        let text: Vec<&str> = v.iter().map(|d| d.text.as_str()).collect();
        assert!(
            text.iter()
                .all(|t| !t.contains("**") && !t.contains("|---"))
        );
        assert!(text.iter().all(|t| !t.contains("```")));
        assert!(
            v.iter()
                .any(|d| d.kind == DKind::Heading && d.text == "Retry design")
        );
        assert!(
            v.iter()
                .any(|d| d.kind == DKind::Code && d.text == "fn delay() {}")
        );
        assert!(
            v.iter()
                .any(|d| d.kind == DKind::Table && d.text.starts_with("Attempt"))
        );
        // One label per message: the prompt and the first agent line.
        assert_eq!(v.iter().filter(|d| d.first).count(), 2);
    }

    #[test]
    fn a_long_table_cell_does_not_spread_the_columns() {
        let long = "x".repeat(60);
        let md = format!("| a | b |\n|---|---|\n| {long} | y |\n| z | w |");
        let v = lines(&[agent(&md)]);
        let rows: Vec<&DLine> = v.iter().filter(|d| d.kind == DKind::Table).collect();
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert!(r.text.chars().count() <= TABLE_COL + 2 + 1, "{}", r.text);
        }
        assert!(rows[1].text.contains('…'));
        // The second column starts at the same place in every row.
        let at = |r: &DLine, c: char| r.text.chars().position(|x| x == c);
        assert_eq!(at(rows[1], 'y'), at(rows[2], 'w'));
    }

    #[test]
    fn the_last_lines_start_at_a_message() {
        let mut tail = vec![Line::new(LineKind::User, "go")];
        for k in 0..6 {
            tail.push(agent(&format!("line {k}")));
        }
        tail.push(Line::new(LineKind::User, "next"));
        tail.push(agent("done"));
        let v = last(&tail, 4);
        assert!(v[0].first);
        assert_eq!(v[0].text, "next");
    }

    #[test]
    fn steps_leave_out_tables_and_code() {
        let tail = vec![
            agent("Next I add the column."),
            agent("| Attempt | Delay |"),
            agent("|---|---|"),
            agent("| 1 | 250 ms |"),
        ];
        let v = steps(&tail, 3);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].text, "Next I add the column.");
    }

    #[test]
    fn a_plain_line_drops_the_markers() {
        assert_eq!(plain_line("**Bold** and `code`"), "Bold and code");
    }
}
