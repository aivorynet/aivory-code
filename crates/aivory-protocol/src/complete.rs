//! Completion in a chat field: `/` lists the slash commands of the agent of a session (the
//! built-in ones that work in a headless run, and the custom commands, prompts and skills of
//! the user and the project), `@` lists files of the codebase and the subagents.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the typed word asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CompleteKind {
    /// A word that starts with `/` at the start of the message.
    Command,
    /// A word that starts with `@`.
    Mention,
}

/// The kind of one suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CompleteItemKind {
    /// A built-in command of the agent CLI.
    Command,
    /// A custom command or prompt file of the user or the project.
    Custom,
    /// A skill.
    Skill,
    /// A file or a folder of the codebase.
    File,
    /// A subagent.
    Agent,
}

/// `compose/complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteParams {
    pub kind: CompleteKind,
    /// The typed word without its `/` or `@`.
    pub query: String,
    /// The session of the chat field: it gives the agent and the folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The agent when there is no session yet (`claude`, `codex`, `gemini`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The folder when there is no session yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// One suggestion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteItem {
    pub kind: CompleteItemKind,
    /// The text of the row, for example `/review` or `src/main.rs`.
    pub label: String,
    /// The text that replaces the typed word, for example `/review ` or `@src/main.rs `.
    pub insert: String,
    /// One short line: the description of a command, the folder of a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The suggestions, best first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteList {
    pub items: Vec<CompleteItem>,
}

/// The most suggestions in one list.
pub const COMPLETE_MAX: usize = 40;

/// The word at `cursor` (a byte offset) that asks for completion: its kind, the byte range
/// of the word with its `/` or `@`, and the query. A `/` word counts only at the start of
/// the text.
pub fn word_at(
    text: &str,
    cursor: usize,
) -> Option<(CompleteKind, std::ops::Range<usize>, String)> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &text[start..cursor];
    // The word must end at the cursor: a letter right after the cursor continues it.
    if text[cursor..]
        .chars()
        .next()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    let kind = match word.chars().next()? {
        '/' if start == 0 => CompleteKind::Command,
        '@' => CompleteKind::Mention,
        _ => return None,
    };
    Some((kind, start..cursor, word[1..].to_string()))
}

/// The score of `label` for `query` (lower is better), or `None` when it does not match:
/// a prefix first, then a word start, then any place, then the letters in order.
pub fn rank(label: &str, query: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let l = label.to_lowercase();
    let q = query.to_lowercase();
    if l.starts_with(&q) {
        return Some(0);
    }
    if let Some(i) = l.find(&q) {
        let at_word = l[..i]
            .chars()
            .last()
            .is_some_and(|c| matches!(c, '/' | '\\' | '-' | '_' | '.' | ':' | ' '));
        return Some(if at_word { 1 } else { 2 });
    }
    let mut rest = q.chars().peekable();
    for c in l.chars() {
        if rest.peek() == Some(&c) {
            rest.next();
        }
    }
    rest.peek().is_none().then_some(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slash_word_counts_only_at_the_start() {
        let (k, r, q) = word_at("/rev", 4).expect("word");
        assert_eq!((k, r, q.as_str()), (CompleteKind::Command, 0..4, "rev"));
        assert!(word_at("see /rev", 8).is_none());
        assert!(word_at("/review now", 11).is_none());
    }

    #[test]
    fn an_at_word_counts_anywhere() {
        let (k, r, q) = word_at("look at @src/ma", 15).expect("word");
        assert_eq!((k, r, q.as_str()), (CompleteKind::Mention, 8..15, "src/ma"));
        assert!(word_at("mail a@b", 4).is_none());
        // The cursor in the middle of a word: no completion.
        assert!(word_at("@main.rs", 3).is_none());
    }

    #[test]
    fn a_prefix_ranks_before_a_middle_match() {
        assert_eq!(rank("review", "rev"), Some(0));
        assert_eq!(rank("security-review", "rev"), Some(1));
        assert_eq!(rank("preview", "rev"), Some(2));
        assert_eq!(rank("src/main.rs", "smr"), Some(3));
        assert_eq!(rank("init", "x"), None);
    }
}
