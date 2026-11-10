//! Helpers that all three transcript readers share: line reading, text clean-up, line and
//! ask builders, and the state decision.

use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::path::Path;

use aivory_protocol::{Ask, AskKind, Line, LineKind, SessionState, UnixMs};
use serde_json::Value;

use super::{Derived, IDLE_WINDOW_MS, WORKING_WINDOW_MS};
use crate::timeutil::format_utc_minutes;

/// Longest text of an agent or user line, in characters.
pub(crate) const MAX_TEXT_CHARS: usize = 2000;
/// Longest main argument of a tool line, in characters.
pub(crate) const MAX_ARG_CHARS: usize = 200;
/// Output lines that a tool result shows. The rest becomes `… N more lines`.
pub(crate) const MAX_OUTPUT_LINES: usize = 3;
/// Longest output line, in characters.
pub(crate) const MAX_OUTPUT_CHARS: usize = 240;
/// Lines of one user prompt that the transcript shows.
pub(crate) const MAX_PROMPT_LINES: usize = 8;
/// Added and removed lines of one edit that the transcript shows.
pub(crate) const MAX_DIFF_LINES: usize = 40;
/// Steps of a plan ask.
pub(crate) const MAX_PLAN_STEPS: usize = 12;

/// Sizes of the windows that `derive_state` reads from the end of a file, smallest first.
const TAIL_WINDOWS: [u64; 3] = [256 * 1024, 4 * 1024 * 1024, 32 * 1024 * 1024];

/// A failure while a reader runs. The caller adds the path.
#[derive(Debug)]
pub(crate) enum ReadError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ReadError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// What a state scan found. `decide` turns it into a [`Derived`].
#[derive(Debug, Default)]
pub(crate) struct Facts {
    /// The last entry is an error that ended the turn.
    pub error: Option<Ask>,
    /// A tool call waits for an answer.
    pub ask: Option<Ask>,
    /// The turn ended with a final agent text.
    pub ended: bool,
    /// Time of the last entry.
    pub since: Option<UnixMs>,
    /// A later change time than the file mtime, for example a running sub-agent.
    pub activity: Option<UnixMs>,
}

/// Apply the state rules in their fixed order: e, q, w, i, p.
pub(crate) fn decide(facts: Option<Facts>, now: UnixMs, mtime: UnixMs) -> Derived {
    let Some(f) = facts else {
        return Derived {
            state: SessionState::Parked,
            ask: None,
            since: mtime,
        };
    };
    let since = f.since.unwrap_or(mtime);
    if let Some(ask) = f.error {
        return Derived {
            state: SessionState::Error,
            ask: Some(ask),
            since,
        };
    }
    if let Some(ask) = f.ask {
        return Derived {
            state: SessionState::NeedsYou,
            ask: Some(ask),
            since,
        };
    }
    let changed = f.activity.map_or(mtime, |a| a.max(mtime));
    let age = now.saturating_sub(changed);
    let state = if !f.ended && age < WORKING_WINDOW_MS {
        SessionState::Working
    } else if f.ended && age < IDLE_WINDOW_MS {
        SessionState::Idle
    } else {
        SessionState::Parked
    };
    Derived {
        state,
        ask: None,
        since,
    }
}

/// Call `f` with each non-empty line (without the line break). The reader streams: the
/// function holds one line at a time.
pub(crate) fn for_each_line<R: BufRead>(mut reader: R, mut f: impl FnMut(&[u8])) -> io::Result<()> {
    let mut buf = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            return Ok(());
        }
        let line = trim_eol(&buf);
        if !line.is_empty() {
            f(line);
        }
    }
}

fn trim_eol(mut line: &[u8]) -> &[u8] {
    while let [rest @ .., b'\n' | b'\r'] = line {
        line = rest;
    }
    line
}

/// Read windows from the end of a file, growing, until `f` returns a value. `f` gets the
/// complete lines of the window. The first line of a window that does not start at the
/// file start can be cut off, so it is dropped.
pub(crate) fn with_tail<T>(
    path: &Path,
    mut f: impl FnMut(&[&[u8]]) -> Option<T>,
) -> io::Result<Option<T>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut buf = Vec::new();
    for window in TAIL_WINDOWS {
        let take = window.min(len);
        let whole = take == len;
        buf.clear();
        file.seek(SeekFrom::Start(len - take))?;
        (&mut file).take(take).read_to_end(&mut buf)?;
        let mut lines: Vec<&[u8]> = buf
            .split(|b| *b == b'\n')
            .map(trim_eol)
            .filter(|l| !l.is_empty())
            .collect();
        if !whole && !lines.is_empty() {
            lines.remove(0);
        }
        if let Some(v) = f(&lines) {
            return Ok(Some(v));
        }
        if whole {
            break;
        }
    }
    Ok(None)
}

/// Remove ANSI escape sequences, carriage returns and tabs from tool output.
pub(crate) fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {}
            '\t' => out.push_str("  "),
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// Cut a string to `max` characters. A cut string ends with `…`.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The first non-empty line, cut to `MAX_ARG_CHARS`. A text with more lines gets ` …`.
pub(crate) fn one_line(s: &str) -> String {
    let clean = clean_text(s);
    let mut lines = clean.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return String::new();
    };
    let mut out = truncate(first, MAX_ARG_CHARS);
    if lines.next().is_some() {
        out.push_str(" …");
    }
    out
}

fn text_rows(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
}

/// One line per non-empty row of an agent text, then a preview line per media that it names.
pub(crate) fn push_text(out: &mut Vec<Line>, kind: LineKind, text: &str) {
    for row in text_rows(&clean_text(text)) {
        out.push(Line::new(kind, truncate(row, MAX_TEXT_CHARS)));
    }
    push_media(out, text);
}

/// The most media previews that one text adds.
const MAX_MEDIA: usize = 4;

/// A preview line for each image, video, PDF, or media URL that `text` names.
pub(crate) fn push_media(out: &mut Vec<Line>, text: &str) {
    out.extend(
        aivory_protocol::media::scan(text, MAX_MEDIA)
            .into_iter()
            .map(Line::media),
    );
}

/// Save an image of a transcript (base64 data) in `<cache>/media/<hash>.<ext>` and return
/// its preview line. The same image twice is one file. `None` for bad data or no cache.
pub(crate) fn stored_image(mime: &str, data: &str) -> Option<Line> {
    use base64::Engine as _;
    use std::hash::{Hash, Hasher};
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    let dir = crate::cache::cache_dir()?.join("media");
    let path = dir.join(format!("{:016x}.{ext}", h.finish()));
    if !path.is_file() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()?;
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, bytes).ok()?;
    }
    let media = aivory_protocol::Media::of(&path.to_string_lossy())?;
    Some(Line::media(media))
}

/// The preview line of a `data:image/…;base64,…` URL (Codex `input_image`).
pub(crate) fn data_url_image(url: &str) -> Option<Line> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    stored_image(mime, data)
}

/// A user prompt: the first `MAX_PROMPT_LINES` rows, then `… N more lines`.
pub(crate) fn push_prompt(out: &mut Vec<Line>, text: &str) {
    let clean = clean_text(text);
    let rows: Vec<&str> = text_rows(&clean).collect();
    for row in rows.iter().take(MAX_PROMPT_LINES) {
        out.push(Line::user(truncate(row, MAX_TEXT_CHARS)));
    }
    if rows.len() > MAX_PROMPT_LINES {
        out.push(Line::user(more_lines(rows.len() - MAX_PROMPT_LINES)));
    }
    push_media(out, text);
}

/// A tool result: the first `MAX_OUTPUT_LINES` rows, then `… N more lines`. A failed
/// result gets the error kind.
pub(crate) fn push_output(out: &mut Vec<Line>, text: &str, is_error: bool) {
    let kind = if is_error {
        LineKind::Error
    } else {
        LineKind::Output
    };
    let clean = clean_text(text);
    let rows: Vec<&str> = text_rows(&clean).collect();
    for row in rows.iter().take(MAX_OUTPUT_LINES) {
        out.push(Line::new(
            kind,
            truncate(row.trim_start(), MAX_OUTPUT_CHARS),
        ));
    }
    if rows.len() > MAX_OUTPUT_LINES {
        out.push(Line::new(kind, more_lines(rows.len() - MAX_OUTPUT_LINES)));
    }
    push_media(out, text);
}

pub(crate) fn more_lines(n: usize) -> String {
    format!("… {n} more lines")
}

/// Cut a diff to `MAX_DIFF_LINES` and append the marker, then add it to `out`.
pub(crate) fn push_diff(out: &mut Vec<Line>, mut diff: Vec<Line>) {
    if diff.len() > MAX_DIFF_LINES {
        let extra = diff.len() - MAX_DIFF_LINES;
        diff.truncate(MAX_DIFF_LINES);
        diff.push(Line::output(more_lines(extra)));
    }
    out.extend(diff);
}

fn diff_row(kind: LineKind, row: &str) -> Line {
    Line::new(kind, truncate(&clean_text(row), MAX_TEXT_CHARS))
}

/// The changed rows of a replacement: removed rows, then added rows. Rows that the old
/// and the new text share at the start and at the end are left out.
pub(crate) fn replace_lines(old: &str, new: &str) -> Vec<Line> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let max_skip = old.len().min(new.len());
    let mut head = 0;
    while head < max_skip && old[head] == new[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < max_skip - head && old[old.len() - 1 - tail] == new[new.len() - 1 - tail] {
        tail += 1;
    }
    let mut out = Vec::new();
    for row in &old[head..old.len() - tail] {
        if !row.trim().is_empty() {
            out.push(diff_row(LineKind::Removed, row));
        }
    }
    for row in &new[head..new.len() - tail] {
        if !row.trim().is_empty() {
            out.push(diff_row(LineKind::Added, row));
        }
    }
    out
}

/// All rows of a new file as added rows.
pub(crate) fn added_lines(content: &str) -> Vec<Line> {
    content
        .lines()
        .filter(|r| !r.trim().is_empty())
        .map(|r| diff_row(LineKind::Added, r))
        .collect()
}

/// The files and the changed rows of a Codex `apply_patch` text.
pub(crate) fn patch_lines(patch: &str) -> (Vec<String>, Vec<Line>) {
    let mut files = Vec::new();
    let mut diff = Vec::new();
    for row in patch.lines() {
        if let Some(rest) = row.strip_prefix("*** ") {
            for key in ["Update File: ", "Add File: ", "Delete File: "] {
                if let Some(path) = rest.strip_prefix(key) {
                    files.push(path.trim().to_string());
                }
            }
        } else if let Some(added) = row.strip_prefix('+') {
            if !added.trim().is_empty() {
                diff.push(diff_row(LineKind::Added, added));
            }
        } else if let Some(removed) = row.strip_prefix('-')
            && !removed.trim().is_empty()
        {
            diff.push(diff_row(LineKind::Removed, removed));
        }
    }
    (files, diff)
}

const ARG_KEYS: [&str; 17] = [
    "command",
    "cmd",
    "file_path",
    "filePath",
    "notebook_path",
    "pattern",
    "query",
    "url",
    "path",
    "dir_path",
    "absolute_path",
    "skill",
    "description",
    "prompt",
    "question",
    "task",
    "name",
];

fn string_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// The main argument of a tool call: the command, the file path, the pattern, and so on.
pub(crate) fn main_arg(input: &Value) -> String {
    let Some(obj) = input.as_object() else {
        return one_line(&string_value(input));
    };
    for key in ARG_KEYS {
        if let Some(v) = obj.get(key) {
            let s = string_value(v);
            if !s.trim().is_empty() {
                return one_line(&s);
            }
        }
    }
    obj.values()
        .find_map(|v| v.as_str().filter(|s| !s.trim().is_empty()))
        .map(one_line)
        .unwrap_or_default()
}

pub(crate) fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The divider before a user prompt: label `TURN n`, text the time of the prompt.
pub(crate) fn turn_line(n: u32, at: Option<UnixMs>) -> Line {
    Line::labeled(
        LineKind::Meta,
        format!("TURN {n}"),
        at.map(format_utc_minutes).unwrap_or_default(),
    )
}

// ---- asks ------------------------------------------------------------------------------

/// A permission ask: the text is the command or the path.
pub(crate) fn permission_ask(id: &str, tool: &str, input: &Value) -> Ask {
    let mut text = main_arg(input);
    if text.is_empty() {
        text = tool.to_string();
    }
    Ask {
        id: id.to_string(),
        kind: AskKind::Permission,
        text,
        options: vec!["Allow".into(), "Deny".into()],
        steps: Vec::new(),
        reason: str_field(input, "description")
            .or_else(|| str_field(input, "justification"))
            .map(one_line)
            .filter(|r| !r.is_empty()),
        risky: false,
    }
}

fn option_label(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => str_field(v, "label")
            .or_else(|| str_field(v, "title"))
            .map(str::to_string),
        _ => None,
    }
    .map(|s| one_line(&s))
    .filter(|s| !s.is_empty())
}

/// A question ask from `{questions: [{question, options: [{label}]}]}` (the first question)
/// or from `{question, options}`.
pub(crate) fn question_ask(id: &str, input: &Value) -> Ask {
    let first = input
        .get("questions")
        .and_then(Value::as_array)
        .and_then(|q| q.first())
        .unwrap_or(input);
    let text = str_field(first, "question")
        .or_else(|| str_field(first, "header"))
        .map(one_line)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Question".to_string());
    let options = first
        .get("options")
        .and_then(Value::as_array)
        .map(|o| o.iter().filter_map(option_label).collect())
        .unwrap_or_default();
    Ask {
        id: id.to_string(),
        kind: AskKind::Question,
        text,
        options,
        steps: Vec::new(),
        reason: None,
        risky: false,
    }
}

fn strip_list_marker(row: &str) -> Option<&str> {
    let t = row.trim_start();
    for m in ["- [ ] ", "- [x] ", "- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some(rest);
        }
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &t[digits..];
        for m in [". ", ") "] {
            if let Some(rest) = rest.strip_prefix(m) {
                return Some(rest);
            }
        }
    }
    None
}

/// The steps of a plan text: its list items, or else its headings (after the title).
pub(crate) fn plan_steps(plan: &str) -> Vec<String> {
    let clean = |s: &str| one_line(&s.replace("**", "").replace('`', ""));
    let mut steps: Vec<String> = plan
        .lines()
        .filter_map(strip_list_marker)
        .map(clean)
        .filter(|s| !s.is_empty())
        .take(MAX_PLAN_STEPS)
        .collect();
    if steps.is_empty() {
        steps = plan
            .lines()
            .filter(|r| r.trim_start().starts_with('#'))
            .map(|r| clean(r.trim_start().trim_start_matches('#')))
            .filter(|s| !s.is_empty())
            .skip(1)
            .take(MAX_PLAN_STEPS)
            .collect();
    }
    steps
}

/// A plan ask. The text is the first row of the plan.
pub(crate) fn plan_ask(id: &str, plan: Option<&str>) -> Ask {
    let plan = plan.unwrap_or_default();
    let title = plan
        .lines()
        .map(|r| r.trim().trim_start_matches('#').trim())
        .find(|r| !r.is_empty())
        .map(|r| one_line(&r.replace("**", "")))
        .unwrap_or_else(|| "Approve the plan?".to_string());
    Ask {
        id: id.to_string(),
        kind: AskKind::Plan,
        text: title,
        options: vec!["Approve".into(), "Keep planning".into()],
        steps: plan_steps(plan),
        reason: None,
        risky: false,
    }
}

/// An error ask: the text is the first row of the error.
pub(crate) fn error_ask(id: &str, text: &str) -> Ask {
    let text = one_line(text);
    Ask {
        id: id.to_string(),
        kind: AskKind::Error,
        text: if text.is_empty() {
            "Stopped on an error".to_string()
        } else {
            text
        },
        options: vec!["Retry".into()],
        steps: Vec::new(),
        reason: None,
        risky: false,
    }
}

/// The text between two markers.
pub(crate) fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clean_text_removes_ansi_and_tabs() {
        assert_eq!(clean_text("\u{1b}[31mred\u{1b}[0m\tx\r\n"), "red  x\n");
        assert_eq!(clean_text("a\u{1b}]0;title\u{7}b"), "ab");
    }

    #[test]
    fn output_is_cut_at_three_rows() {
        let mut out = Vec::new();
        push_output(&mut out, "1\n2\n\n3\n4\n5\n", false);
        let texts: Vec<_> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["1", "2", "3", "… 2 more lines"]);
        assert!(out.iter().all(|l| l.kind == LineKind::Output));
        let mut err = Vec::new();
        push_output(&mut err, "boom", true);
        assert_eq!(err, vec![Line::error("boom")]);
    }

    #[test]
    fn replace_skips_shared_rows() {
        let d = replace_lines("a\nb\nc\nd", "a\nB\nc\nd");
        assert_eq!(d, vec![Line::removed("b"), Line::added("B")]);
        assert!(replace_lines("same", "same").is_empty());
        assert_eq!(replace_lines("", "x"), vec![Line::added("x")]);
    }

    #[test]
    fn diff_is_capped() {
        let mut out = Vec::new();
        let rows: Vec<Line> = (0..45).map(|i| Line::added(i.to_string())).collect();
        push_diff(&mut out, rows);
        assert_eq!(out.len(), MAX_DIFF_LINES + 1);
        assert_eq!(out.last().unwrap().text, "… 5 more lines");
    }

    #[test]
    fn patch_gives_files_and_rows() {
        let p = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n ctx\n-old\n+new\n*** Add File: b.txt\n+line\n*** End Patch";
        let (files, diff) = patch_lines(p);
        assert_eq!(files, ["src/a.rs", "b.txt"]);
        assert_eq!(
            diff,
            vec![
                Line::removed("old"),
                Line::added("new"),
                Line::added("line")
            ]
        );
    }

    #[test]
    fn main_arg_prefers_command_and_path() {
        assert_eq!(
            main_arg(&json!({"command": "ls -la", "description": "x"})),
            "ls -la"
        );
        assert_eq!(
            main_arg(&json!({"command": ["git", "status"]})),
            "git status"
        );
        assert_eq!(main_arg(&json!({"pattern": "foo", "path": "src"})), "foo");
        assert_eq!(
            main_arg(&json!({"file_path": "a/b.rs", "limit": 3})),
            "a/b.rs"
        );
        assert_eq!(main_arg(&json!({"command": "a\nb"})), "a …");
        assert_eq!(main_arg(&json!({"zzz": "first", "n": 1})), "first");
        assert_eq!(main_arg(&json!({"n": 1})), "");
    }

    #[test]
    fn plan_steps_from_list_or_headings() {
        let plan = "# Plan title\n\n1. First step\n2) Second **step**\n- Third\n";
        assert_eq!(plan_steps(plan), ["First step", "Second step", "Third"]);
        assert_eq!(plan_steps("# T\n## One\n## Two"), ["One", "Two"]);
        assert_eq!(plan_ask("id", Some(plan)).text, "Plan title");
    }

    #[test]
    fn question_ask_reads_labels() {
        let q = json!({"questions": [{"question": "Which?", "options": [{"label": "A"}, {"label": "B", "description": "d"}]}]});
        let a = question_ask("id", &q);
        assert_eq!(a.text, "Which?");
        assert_eq!(a.options, ["A", "B"]);
        assert_eq!(a.kind, AskKind::Question);
    }

    #[test]
    fn decide_applies_rules_in_order() {
        let now = 10_000_000_000;
        let fresh = now - 1000;
        let idle = now - 5 * 60_000;
        let old = now - IDLE_WINDOW_MS - 1;
        let f = |ended| Facts {
            ended,
            ..Facts::default()
        };
        assert_eq!(
            decide(Some(f(false)), now, fresh).state,
            SessionState::Working
        );
        assert_eq!(
            decide(Some(f(false)), now, idle).state,
            SessionState::Parked
        );
        assert_eq!(decide(Some(f(true)), now, idle).state, SessionState::Idle);
        assert_eq!(decide(Some(f(true)), now, fresh).state, SessionState::Idle);
        assert_eq!(decide(Some(f(true)), now, old).state, SessionState::Parked);
        assert_eq!(decide(None, now, fresh).state, SessionState::Parked);
        let with_ask = Facts {
            ask: Some(error_ask("a", "x")),
            ..Facts::default()
        };
        assert_eq!(
            decide(Some(with_ask), now, old).state,
            SessionState::NeedsYou
        );
    }
}

#[cfg(test)]
mod media_tests {
    use super::*;
    use aivory_protocol::MediaKind;

    #[test]
    fn texts_get_a_preview_line_per_media() {
        let mut out = Vec::new();
        push_text(
            &mut out,
            LineKind::Agent,
            "Saved the chart to C:/out/chart.png.\nThe demo: /tmp/demo.mp4",
        );
        let media: Vec<_> = out.iter().filter_map(|l| l.media.clone()).collect();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].kind, MediaKind::Image);
        assert_eq!(media[1].kind, MediaKind::Video);
        assert_eq!(out.last().unwrap().text, "video demo.mp4");
        assert_eq!(out.last().unwrap().kind, LineKind::Meta);
    }

    #[test]
    fn bad_image_data_gives_no_line() {
        assert!(data_url_image("data:image/png;base64").is_none());
        assert!(data_url_image("https://e.org/a.png").is_none());
        assert!(stored_image("image/tiff", "AAAA").is_none());
    }
}
