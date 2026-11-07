//! Plain data for the Bridge screens, taken from the pane of a session: chat rows, test
//! counts, the turn number, the meta line. No drawing here.

use aivory_protocol::{Line, LineKind, Pane};

use crate::bridge_kit::*;

/// `claude` as `Claude`.
pub fn provider_name(provider: &str) -> String {
    let mut c = provider.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The first line of a text, cut to `max` characters.
fn first(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > max {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}
/// The color of a tool by its label: edits periwinkle, shells orange, agents salmon.
pub fn tool_color(label: &str) -> u32 {
    match label.to_lowercase().as_str() {
        "edit" | "write" | "multiedit" | "patch" => PE,
        "bash" | "shell" | "run" => OR,
        "task" | "agent" => SA,
        _ => PK,
    }
}

/// The number in front of a word in a text: `12 passed;` gives 12 for `passed`.
fn number_before(text: &str, word: &str) -> Option<u32> {
    let toks: Vec<&str> = text.split_whitespace().collect();
    toks.windows(2).find_map(|w| {
        (w[1].trim_matches(|c: char| !c.is_alphanumeric()) == word)
            .then(|| {
                w[0].trim_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .ok()
            })
            .flatten()
    })
}

// TODO(daemon): a `session/tests` method would replace this read of the transcript lines.
/// The counts of the last test run in the tail: passed and failed. `None` without a run.
pub fn tests_of(tail: &[Line]) -> Option<(u32, u32)> {
    tail.iter().rev().find_map(|l| {
        if !matches!(l.kind, LineKind::Output | LineKind::Tool) {
            return None;
        }
        let passed = number_before(&l.text, "passed");
        let failed = number_before(&l.text, "failed");
        (passed.is_some() || failed.is_some()).then(|| (passed.unwrap_or(0), failed.unwrap_or(0)))
    })
}

/// The lines of the test runs in the tail, newest last, at most `n`.
pub fn test_lines(tail: &[Line], n: usize) -> Vec<String> {
    let mut v: Vec<String> = tail
        .iter()
        .filter(|l| {
            matches!(l.kind, LineKind::Output | LineKind::Tool)
                && (number_before(&l.text, "passed").is_some()
                    || number_before(&l.text, "failed").is_some())
        })
        .map(|l| first(&l.text, 120))
        .collect();
    let skip = v.len().saturating_sub(n);
    v.split_off(skip)
}

/// The number of the latest turn, from its divider line.
pub fn turn_of(p: &Pane) -> Option<u32> {
    p.tail
        .iter()
        .rev()
        .filter(|l| l.kind == LineKind::Turn)
        .find_map(|l| {
            let digits: String = l
                .label
                .as_deref()
                .unwrap_or(&l.text)
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
}

/// How long a session waits: `6 min`, or `now` under a minute.
pub fn wait_text(minutes: i64) -> String {
    if minutes < 1 {
        "now".to_string()
    } else {
        format!("{minutes} min")
    }
}
/// The number of turns (user prompts) in a transcript.
pub fn turn_count(lines: &[Line]) -> usize {
    lines.iter().filter(|l| l.kind == LineKind::User).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, label: Option<&str>, text: &str) -> Line {
        Line {
            kind,
            label: label.map(String::from),
            text: text.into(),
        }
    }

    #[test]
    fn test_counts_come_from_the_output() {
        let tail = vec![
            line(LineKind::Tool, Some("Bash"), "cargo test -p payment"),
            line(
                LineKind::Output,
                None,
                "test result: ok. 12 passed; 0 failed",
            ),
        ];
        assert_eq!(tests_of(&tail), Some((12, 0)));
        assert_eq!(test_lines(&tail, 3).len(), 1);
        assert_eq!(tests_of(&tail[..1]), None);
    }

    #[test]
    fn a_wait_reads_in_minutes() {
        assert_eq!(wait_text(0), "now");
        assert_eq!(wait_text(6), "6 min");
    }
}
