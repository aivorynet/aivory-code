//! Map the stream-json output of Claude Code to [`RunEvent`]s.
//!
//! Output events (CLI 2.1.306): `system` (subtype `init`), `assistant` (text, tool_use),
//! `user` (tool_result), `result`, `control_response`, `rate_limit_event`. The mapper reads
//! whole messages, not partial deltas. It ignores every type it does not know.

use std::collections::HashMap;

use aivory_protocol::Line;
use serde_json::Value;

use super::RunEvent;
use crate::mcp::truncate;

/// Window of a model whose result does not tell the window.
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
pub(super) const MAX_LINE_CHARS: usize = 240;
const MAX_DIFF_LINES: usize = 8;
const MAX_REMEMBERED_TOOLS: usize = 256;

#[derive(Debug, Default)]
pub(super) struct Mapper {
    /// Input tokens of the last main-thread API call: the size of the context now.
    context_tokens: u64,
    /// Tokens of the session so far: input, cache writes and output of each turn.
    tokens: u64,
    /// Tool name by tool-use id, so that a result line can say what ran.
    tools: HashMap<String, String>,
}

impl Mapper {
    pub fn map(&mut self, v: &Value) -> Vec<RunEvent> {
        match v.get("type").and_then(Value::as_str) {
            Some("system") => init(v),
            Some("assistant") => self.assistant(v),
            Some("user") => self.user(v),
            Some("result") => self.result(v),
            _ => Vec::new(),
        }
    }

    fn assistant(&mut self, v: &Value) -> Vec<RunEvent> {
        let sub = v.get("parent_tool_use_id").is_some_and(|p| !p.is_null());
        if !sub && let Some(u) = v.pointer("/message/usage") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            self.context_tokens =
                n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
        }
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if !text.trim().is_empty() {
                        let line = if sub {
                            Line::subagent("subagent", truncate(text.trim(), MAX_LINE_CHARS))
                        } else {
                            Line::agent(text.trim())
                        };
                        out.push(RunEvent::Line(line));
                    }
                }
                Some("tool_use") => out.extend(self.tool_use(block, sub)),
                _ => {}
            }
        }
        out
    }

    fn tool_use(&mut self, block: &Value, sub: bool) -> Vec<RunEvent> {
        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
        let input = block.get("input").unwrap_or(&Value::Null);
        if let Some(id) = block.get("id").and_then(Value::as_str) {
            if self.tools.len() >= MAX_REMEMBERED_TOOLS {
                self.tools.clear();
            }
            self.tools.insert(id.to_string(), name.to_string());
        }
        let detail = tool_detail(name, input);
        let mut out = Vec::new();
        if matches!(name, "Task" | "Agent") {
            let kind = input
                .get("subagent_type")
                .and_then(Value::as_str)
                .unwrap_or("subagent");
            out.push(RunEvent::Line(Line::subagent(kind, detail)));
        } else if sub {
            out.push(RunEvent::Line(Line::subagent(name, detail)));
        } else {
            out.push(RunEvent::Line(Line::tool(name, detail)));
        }
        if name == "Edit" {
            let text = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
            diff_lines(text("old_string"), text("new_string"), &mut out);
        }
        out
    }

    fn user(&mut self, v: &Value) -> Vec<RunEvent> {
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let text = result_text(block.get("content"));
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                self.tools.remove(id);
            }
            if text.trim().is_empty() {
                continue;
            }
            let summary = summarize(&text);
            out.push(RunEvent::Line(if is_error {
                Line::error(summary)
            } else {
                Line::output(summary)
            }));
        }
        out
    }

    fn result(&mut self, v: &Value) -> Vec<RunEvent> {
        let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
        let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let mut out = Vec::new();
        // An interrupt ends the turn with `error_during_execution` and an `aborted_*` reason.
        let aborted = v
            .get("terminal_reason")
            .and_then(Value::as_str)
            .is_some_and(|r| r.starts_with("aborted"));
        if aborted {
            out.push(RunEvent::Line(Line::meta("Interrupted")));
        } else if is_error || (!subtype.is_empty() && subtype != "success") {
            let text = v
                .get("result")
                .and_then(Value::as_str)
                .or_else(|| v.pointer("/errors/0").and_then(Value::as_str))
                .filter(|t| !t.is_empty())
                .map_or_else(
                    || format!("The turn failed ({subtype})."),
                    |t| truncate(t, MAX_LINE_CHARS),
                );
            out.push(RunEvent::Line(Line::error(text)));
        }
        let window = v
            .get("modelUsage")
            .and_then(Value::as_object)
            .and_then(|m| {
                m.values()
                    .filter_map(|u| u.get("contextWindow").and_then(Value::as_u64))
                    .max()
            })
            .filter(|w| *w > 0)
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let pct = (self.context_tokens.saturating_mul(100) / window).min(100);
        if let Some(u) = v.get("usage") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            self.tokens +=
                n("input_tokens") + n("cache_creation_input_tokens") + n("output_tokens");
        }
        out.push(RunEvent::TurnEnded {
            cost_usd: v
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            context_pct: u8::try_from(pct).unwrap_or(100),
            tokens: self.tokens,
        });
        out
    }
}

fn init(v: &Value) -> Vec<RunEvent> {
    if v.get("subtype").and_then(Value::as_str) != Some("init") {
        return Vec::new();
    }
    match v.get("session_id").and_then(Value::as_str) {
        Some(id) => vec![RunEvent::Started {
            session_id: id.to_string(),
        }],
        None => Vec::new(),
    }
}

/// The short text after the tool name: the command, the path, the pattern.
fn tool_detail(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str);
    let text = match name {
        "Bash" | "PowerShell" => field("command").map(first_line),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => field("file_path")
            .or_else(|| field("notebook_path"))
            .map(str::to_string),
        "Glob" | "Grep" => field("pattern").map(str::to_string),
        "WebFetch" => field("url").map(str::to_string),
        "WebSearch" => field("query").map(str::to_string),
        "Task" | "Agent" => field("description").map(str::to_string),
        "ToolSearch" => field("query").map(str::to_string),
        "ExitPlanMode" => field("plan").map(plan_title),
        "AskUserQuestion" => input
            .pointer("/questions/0/question")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    };
    truncate(&text.unwrap_or_else(|| input.to_string()), MAX_LINE_CHARS)
}

/// The first heading or text line of a plan.
fn plan_title(plan: &str) -> String {
    plan.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("plan")
        .to_string()
}

pub(super) fn first_line(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").to_string();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first
    }
}

pub(super) fn diff_lines(old: &str, new: &str, out: &mut Vec<RunEvent>) {
    for (lines, removed) in [(old, true), (new, false)] {
        for l in lines.lines().take(MAX_DIFF_LINES) {
            let l = truncate(l, MAX_LINE_CHARS);
            out.push(RunEvent::Line(if removed {
                Line::removed(l)
            } else {
                Line::added(l)
            }));
        }
        if lines.lines().count() > MAX_DIFF_LINES {
            let more = lines.lines().count() - MAX_DIFF_LINES;
            let note = format!("… {more} more lines");
            out.push(RunEvent::Line(if removed {
                Line::removed(note)
            } else {
                Line::added(note)
            }));
        }
    }
}

/// The text of a tool result: a string, or a list of text blocks.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The first non-empty line, and how many lines follow.
pub(super) fn summarize(text: &str) -> String {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next().unwrap_or("").trim_end();
    let more = lines.count();
    let first = truncate(first, MAX_LINE_CHARS);
    if more == 0 {
        first
    } else {
        format!("{first} (+{more} lines)")
    }
}

#[cfg(test)]
mod tests {
    use aivory_protocol::LineKind;
    use serde_json::json;

    use super::*;

    fn lines(events: &[RunEvent]) -> Vec<(LineKind, Option<&str>, &str)> {
        events
            .iter()
            .filter_map(|e| match e {
                RunEvent::Line(l) => Some((l.kind, l.label.as_deref(), l.text.as_str())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn init_gives_started() {
        let mut m = Mapper::default();
        let e = m.map(&json!({"type": "system", "subtype": "init", "session_id": "s-1"}));
        assert_eq!(
            e,
            [RunEvent::Started {
                session_id: "s-1".into()
            }]
        );
        assert!(
            m.map(&json!({"type": "system", "subtype": "hook_started"}))
                .is_empty()
        );
    }

    #[test]
    fn assistant_text_and_tool_use() {
        let mut m = Mapper::default();
        let e = m.map(&json!({"type": "assistant", "message": {"content": [
            {"type": "text", "text": "I run it."},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi\nsecond"}},
        ]}}));
        assert_eq!(
            lines(&e),
            [
                (LineKind::Agent, None, "I run it."),
                (LineKind::Tool, Some("Bash"), "echo hi …"),
            ]
        );
    }

    #[test]
    fn plan_and_question_tools_show_a_short_text() {
        let mut m = Mapper::default();
        let e = m.map(&json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "ExitPlanMode", "input": {"plan": "# Plan: x
        1. a"}},
            {"type": "tool_use", "id": "t2", "name": "AskUserQuestion",
             "input": {"questions": [{"question": "Which color?", "options": []}]}},
        ]}}));
        assert_eq!(
            lines(&e),
            [
                (LineKind::Tool, Some("ExitPlanMode"), "Plan: x"),
                (LineKind::Tool, Some("AskUserQuestion"), "Which color?"),
            ]
        );
    }

    #[test]
    fn edit_gives_diff_lines() {
        let mut m = Mapper::default();
        let e = m.map(&json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Edit",
             "input": {"file_path": "a.rs", "old_string": "let a = 1;", "new_string": "let a = 2;\nlet b = 3;"}},
        ]}}));
        assert_eq!(
            lines(&e),
            [
                (LineKind::Tool, Some("Edit"), "a.rs"),
                (LineKind::Removed, None, "let a = 1;"),
                (LineKind::Added, None, "let a = 2;"),
                (LineKind::Added, None, "let b = 3;"),
            ]
        );
    }

    #[test]
    fn tool_result_is_one_output_line() {
        let mut m = Mapper::default();
        let e = m.map(&json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "aivory-p2b\nline2\nline3"},
        ]}}));
        assert_eq!(
            lines(&e),
            [(LineKind::Output, None, "aivory-p2b (+2 lines)")]
        );
        let e = m.map(&json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t2", "is_error": true,
             "content": [{"type": "text", "text": "denied"}]},
        ]}}));
        assert_eq!(lines(&e), [(LineKind::Error, None, "denied")]);
    }

    #[test]
    fn result_gives_cost_and_context() {
        let mut m = Mapper::default();
        m.map(&json!({"type": "assistant", "message": {
            "usage": {"input_tokens": 10, "cache_creation_input_tokens": 20_000, "cache_read_input_tokens": 10_000},
            "content": []}}));
        let e = m.map(
            &json!({"type": "result", "subtype": "success", "is_error": false,
            "total_cost_usd": 0.0123,
            "modelUsage": {"claude-haiku": {"contextWindow": 200_000}}}),
        );
        assert_eq!(
            e,
            [RunEvent::TurnEnded {
                cost_usd: 0.0123,
                context_pct: 15,
                tokens: 0
            }]
        );
    }

    #[test]
    fn the_token_count_adds_up_over_turns_without_cache_reads() {
        let mut m = Mapper::default();
        let usage = json!({"input_tokens": 10, "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 5000, "output_tokens": 40});
        let tokens = |e: &[RunEvent]| match e.last() {
            Some(RunEvent::TurnEnded { tokens, .. }) => *tokens,
            other => panic!("no turn end: {other:?}"),
        };
        let first = m.map(&json!({"type": "result", "subtype": "success", "usage": usage}));
        assert_eq!(tokens(&first), 150);
        let second = m.map(&json!({"type": "result", "subtype": "success", "usage": usage}));
        assert_eq!(tokens(&second), 300);
    }

    #[test]
    fn failed_result_adds_an_error_line_and_still_ends_the_turn() {
        let mut m = Mapper::default();
        let e = m.map(
            &json!({"type": "result", "subtype": "error_max_turns", "is_error": true,
            "result": "", "total_cost_usd": 0.5}),
        );
        assert_eq!(
            lines(&e),
            [(LineKind::Error, None, "The turn failed (error_max_turns).")]
        );
        assert!(matches!(e.last(), Some(RunEvent::TurnEnded { .. })));
    }

    #[test]
    fn a_failed_result_uses_the_errors_list() {
        let mut m = Mapper::default();
        let e = m.map(
            &json!({"type": "result", "subtype": "error_during_execution",
            "is_error": true, "errors": ["No conversation found with session ID: x"]}),
        );
        assert_eq!(
            lines(&e),
            [(
                LineKind::Error,
                None,
                "No conversation found with session ID: x"
            )]
        );
    }

    #[test]
    fn an_interrupted_turn_gives_a_meta_line() {
        let mut m = Mapper::default();
        let e = m.map(
            &json!({"type": "result", "subtype": "error_during_execution",
            "is_error": true, "terminal_reason": "aborted_tools", "total_cost_usd": 0.004}),
        );
        assert_eq!(lines(&e), [(LineKind::Meta, None, "Interrupted")]);
        assert!(matches!(e.last(), Some(RunEvent::TurnEnded { .. })));
    }

    #[test]
    fn subagent_messages_are_subagent_lines() {
        let mut m = Mapper::default();
        let e = m.map(&json!({"type": "assistant", "parent_tool_use_id": "t9",
            "message": {"usage": {"input_tokens": 999_999}, "content": [
                {"type": "text", "text": "Looking."}]}}));
        assert_eq!(
            lines(&e),
            [(LineKind::Subagent, Some("subagent"), "Looking.")]
        );
        // The subagent usage does not count as the context of the main thread.
        let r = m.map(&json!({"type": "result", "subtype": "success"}));
        assert_eq!(
            r,
            [RunEvent::TurnEnded {
                cost_usd: 0.0,
                context_pct: 0,
                tokens: 0
            }]
        );
    }

    #[test]
    fn unknown_types_are_ignored() {
        let mut m = Mapper::default();
        assert!(m.map(&json!({"type": "rate_limit_event"})).is_empty());
        assert!(m.map(&json!({"type": "control_response"})).is_empty());
        assert!(m.map(&json!({"no_type": 1})).is_empty());
    }
}
