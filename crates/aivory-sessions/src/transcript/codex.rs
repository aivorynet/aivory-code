//! Codex CLI transcripts: `rollout-<ts>-<uuid>.jsonl`.
//!
//! One object per line: `{timestamp, type, payload}`.
//! - `session_meta`: `payload{id, cwd, cli_version, git{branch}}`.
//! - `turn_context`: `payload{model, approval_policy, ...}`.
//! - `event_msg`: `payload.type` is `user_message{message}`, `agent_message{message}`,
//!   `error{message}`, `task_started`, `task_complete`, `turn_aborted`, `token_count`, ...
//!   The user prompt and the agent text come from these events. The `response_item` copies of
//!   them are skipped, because they also carry the injected context.
//! - `response_item`: `payload.type` is `function_call{name, arguments (a JSON string),
//!   call_id}`, `custom_tool_call{name, input, call_id}` (`apply_patch`),
//!   `local_shell_call{action{command}}`, `web_search_call{action}` and the outputs
//!   `function_call_output` and `custom_tool_call_output{call_id, output}`.
//!
//! A shell output is `Exit code: N\nWall time: ...\nOutput:\n<text>`, an `apply_patch` output
//! is a JSON object `{output, metadata{exit_code}}`.

use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::Path;

use aivory_protocol::{Line, LineKind, UnixMs};
use serde::Deserialize;
use serde_json::Value;

use super::common::{
    Facts, ReadError, between, error_ask, for_each_line, main_arg, one_line, patch_lines,
    permission_ask, plan_ask, push_diff, push_output, push_prompt, push_text, question_ask,
    str_field, turn_line, with_tail,
};
use crate::timeutil::parse_rfc3339_ms;

mod trace;

pub(super) use trace::lane;

const PLAN_OPEN: &str = "<proposed_plan>";
const PLAN_CLOSE: &str = "</proposed_plan>";
const QUESTION_TOOL: &str = "request_user_input";
const PATCH_TOOL: &str = "apply_patch";
const SUBAGENT_TOOLS: [&str; 1] = ["spawn_agent"];
/// With this approval policy Codex never asks to run a command.
const NEVER_ASK_POLICY: &str = "never";

#[derive(Deserialize)]
struct CodexLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<Value>,
}

impl CodexLine {
    fn time(&self) -> Option<UnixMs> {
        self.timestamp.as_deref().and_then(parse_rfc3339_ms)
    }
}

/// The text of a `response_item` message.
fn message_text(payload: &Value) -> String {
    payload
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The arguments of a `function_call`: a JSON string.
fn call_args(payload: &Value) -> Value {
    match payload.get("arguments") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    }
}

/// The arguments of a `custom_tool_call` as a JSON object: the files of a patch, or the input.
fn custom_call_args(payload: &Value) -> Value {
    let input = str_field(payload, "input").unwrap_or_default();
    if str_field(payload, "name") == Some(PATCH_TOOL) {
        serde_json::json!({ "path": files_label(&patch_lines(input).0) })
    } else {
        serde_json::json!({ "command": input })
    }
}

/// The text and the exit code of a tool output.
fn parse_output(out: &str) -> (String, Option<i64>) {
    if out.starts_with('{')
        && let Ok(v) = serde_json::from_str::<Value>(out)
        && let Some(text) = v.get("output").and_then(Value::as_str)
    {
        let code = v
            .get("metadata")
            .and_then(|m| m.get("exit_code"))
            .and_then(Value::as_i64);
        return (text.to_string(), code);
    }
    if let Some(rest) = out.strip_prefix("Exit code: ") {
        let code = rest
            .lines()
            .next()
            .and_then(|l| l.trim().parse::<i64>().ok());
        let body = out
            .lines()
            .take(5)
            .position(|l| l.trim() == "Output:")
            .map_or_else(
                || out.to_string(),
                |i| out.lines().skip(i + 1).collect::<Vec<_>>().join("\n"),
            );
        return (body, code);
    }
    (out.to_string(), None)
}

// ---- normalize ------------------------------------------------------------------------------

pub(super) fn normalize<R: BufRead>(reader: R) -> io::Result<Vec<Line>> {
    let mut n = Normalizer::default();
    for_each_line(reader, |raw| {
        if let Ok(line) = serde_json::from_slice::<CodexLine>(raw) {
            n.line(&line);
        }
    })?;
    Ok(n.out)
}

#[derive(Default)]
struct Normalizer {
    out: Vec<Line>,
    turns: u32,
    model: Option<String>,
    /// Tool name by `call_id`, to read the output.
    calls: HashMap<String, String>,
}

impl Normalizer {
    fn line(&mut self, l: &CodexLine) {
        let Some(p) = &l.payload else { return };
        let ptype = str_field(p, "type");
        match (l.kind.as_deref(), ptype) {
            (Some("session_meta"), _) => self.begin(p),
            (Some("turn_context"), _) => {
                if let Some(model) = str_field(p, "model")
                    && self.model.as_deref() != Some(model)
                {
                    self.out.push(Line::meta(format!("model · {model}")));
                    self.model = Some(model.to_string());
                }
            }
            (Some("compacted"), _) => self.out.push(Line::meta("context compacted")),
            (Some("event_msg"), Some(t)) => self.event(t, p, l.time()),
            (Some("response_item"), Some(t)) => self.item(t, p),
            _ => {}
        }
    }

    fn begin(&mut self, p: &Value) {
        let mut text = String::from("session start");
        let branch = p.get("git").and_then(|g| str_field(g, "branch"));
        for part in [str_field(p, "cwd"), branch] {
            if let Some(part) = part.filter(|s| !s.is_empty()) {
                text.push_str(" · ");
                text.push_str(part);
            }
        }
        self.out.push(Line::meta(text));
    }

    fn event(&mut self, kind: &str, p: &Value, at: Option<UnixMs>) {
        let message = str_field(p, "message").unwrap_or_default();
        match kind {
            "user_message" if !message.trim().is_empty() => {
                self.turns += 1;
                self.out.push(turn_line(self.turns, at));
                push_prompt(&mut self.out, message);
            }
            "agent_message" => push_text(&mut self.out, LineKind::Agent, message),
            "error" | "stream_error" => push_output(&mut self.out, message, true),
            "turn_aborted" => self.out.push(Line::meta("turn aborted")),
            "context_compacted" => self.out.push(Line::meta("context compacted")),
            _ => {}
        }
    }

    fn item(&mut self, kind: &str, p: &Value) {
        match kind {
            "function_call" => {
                let name = str_field(p, "name").unwrap_or("tool");
                self.remember(p, name);
                let args = call_args(p);
                self.tool_line(name, &args);
            }
            "custom_tool_call" => {
                let name = str_field(p, "name").unwrap_or("tool");
                self.remember(p, name);
                let input = str_field(p, "input").unwrap_or_default();
                if name == PATCH_TOOL {
                    let (files, diff) = patch_lines(input);
                    self.out.push(Line::tool(name, files_label(&files)));
                    push_diff(&mut self.out, diff);
                } else {
                    self.out.push(Line::tool(name, one_line(input)));
                }
            }
            "local_shell_call" => {
                let command = p.get("action").map(main_arg).unwrap_or_default();
                self.out.push(Line::tool("shell", command));
            }
            "web_search_call" => {
                let text = p.get("action").map(main_arg).unwrap_or_default();
                self.out.push(Line::tool("web_search", text));
            }
            "function_call_output" | "custom_tool_call_output" => self.output(p),
            _ => {}
        }
    }

    fn remember(&mut self, p: &Value, name: &str) {
        if let Some(id) = str_field(p, "call_id") {
            self.calls.insert(id.to_string(), name.to_string());
        }
    }

    fn tool_line(&mut self, name: &str, args: &Value) {
        if SUBAGENT_TOOLS.contains(&name) {
            let kind = str_field(args, "agent_type").unwrap_or(name);
            let text = str_field(args, "message")
                .or_else(|| str_field(args, "task"))
                .map(one_line)
                .unwrap_or_default();
            self.out.push(Line::subagent(kind, text));
        } else {
            self.out.push(Line::tool(name, main_arg(args)));
        }
    }

    fn output(&mut self, p: &Value) {
        let raw = str_field(p, "output").unwrap_or_default();
        let (text, code) = parse_output(raw);
        let failed = code.is_some_and(|c| c != 0);
        let tool = str_field(p, "call_id").and_then(|id| self.calls.get(id));
        if tool.is_some_and(|t| t == PATCH_TOOL) && !failed {
            return;
        }
        push_output(&mut self.out, &text, failed);
    }
}

fn files_label(files: &[String]) -> String {
    match files {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} (+{} more)", rest.len()),
    }
}

// ---- state ----------------------------------------------------------------------------------

struct Pending {
    id: String,
    name: String,
    args: Value,
}

#[derive(Default)]
struct StateScan {
    seen: bool,
    /// A turn is open: `task_started` or a prompt came, `task_complete` did not.
    open: bool,
    saw_turn_events: bool,
    last_assistant: bool,
    pending: Vec<Pending>,
    error: Option<String>,
    plan: Option<String>,
    approval: Option<String>,
    last_time: Option<UnixMs>,
}

impl StateScan {
    fn feed(&mut self, l: &CodexLine) {
        let Some(p) = &l.payload else { return };
        let ptype = str_field(p, "type").unwrap_or_default();
        match (l.kind.as_deref(), ptype) {
            (Some("turn_context"), _) => {
                if let Some(a) = str_field(p, "approval_policy") {
                    self.approval = Some(a.to_string());
                }
                return;
            }
            (Some("event_msg"), "token_count") | (Some("session_meta"), _) => return,
            (Some("event_msg"), "user_message") => {
                self.open = true;
                self.pending.clear();
                self.error = None;
                self.plan = None;
                self.last_assistant = false;
            }
            (Some("event_msg"), "task_started") => {
                self.open = true;
                self.saw_turn_events = true;
                self.error = None;
                self.plan = None;
            }
            (Some("event_msg"), "task_complete") => {
                self.open = false;
                self.saw_turn_events = true;
                self.pending.clear();
            }
            (Some("event_msg"), "turn_aborted") => {
                self.open = false;
                self.saw_turn_events = true;
                self.pending.clear();
            }
            (Some("event_msg"), "error" | "stream_error") => {
                self.error = Some(str_field(p, "message").unwrap_or_default().to_string());
            }
            (Some("event_msg"), "agent_message") => {
                self.assistant(str_field(p, "message").unwrap_or_default());
            }
            (Some("response_item"), "message") if str_field(p, "role") == Some("assistant") => {
                self.assistant(&message_text(p));
            }
            (Some("response_item"), "function_call" | "custom_tool_call" | "local_shell_call") => {
                self.last_assistant = false;
                let args = match ptype {
                    "function_call" => call_args(p),
                    "local_shell_call" => p.get("action").cloned().unwrap_or(Value::Null),
                    _ => custom_call_args(p),
                };
                self.pending.push(Pending {
                    id: str_field(p, "call_id").unwrap_or_default().to_string(),
                    name: str_field(p, "name").unwrap_or("shell").to_string(),
                    args,
                });
            }
            (Some("response_item"), "function_call_output" | "custom_tool_call_output") => {
                if let Some(id) = str_field(p, "call_id") {
                    self.pending.retain(|c| c.id != id);
                }
            }
            (Some("response_item"), "reasoning") | (Some("event_msg"), "agent_reasoning") => {
                self.last_assistant = false;
            }
            _ => {}
        }
        self.seen = true;
        if let Some(t) = l.time() {
            self.last_time = Some(t);
        }
    }

    fn assistant(&mut self, text: &str) {
        self.last_assistant = true;
        self.plan = between(text, PLAN_OPEN, PLAN_CLOSE).map(str::to_string);
    }

    fn finish(self) -> Option<Facts> {
        if !self.seen {
            return None;
        }
        let ended = if self.saw_turn_events {
            !self.open
        } else {
            self.last_assistant
        };
        let mut facts = Facts {
            since: self.last_time,
            ended,
            ..Facts::default()
        };
        let stamp = self.last_time.unwrap_or_default();
        if let Some(text) = &self.error {
            facts.error = Some(error_ask(&format!("error-{stamp}"), text));
            return Some(facts);
        }
        let never_asks = self.approval.as_deref() == Some(NEVER_ASK_POLICY);
        let call = self
            .pending
            .iter()
            .find(|c| c.name == QUESTION_TOOL)
            .or_else(|| {
                self.pending
                    .iter()
                    .find(|c| !never_asks && !SUBAGENT_TOOLS.contains(&c.name.as_str()))
            });
        facts.ask = match call {
            Some(c) if c.name == QUESTION_TOOL => Some(question_ask(&c.id, &c.args)),
            Some(c) => Some(permission_ask(&c.id, &c.name, &c.args)),
            None if ended && self.pending.is_empty() => self
                .plan
                .as_deref()
                .map(|plan| plan_ask(&format!("plan-{stamp}"), Some(plan))),
            None => None,
        };
        Some(facts)
    }
}

pub(super) fn facts(path: &Path) -> Result<Option<Facts>, ReadError> {
    Ok(with_tail(path, |lines| {
        let mut scan = StateScan::default();
        for raw in lines {
            if let Ok(line) = serde_json::from_slice::<CodexLine>(raw) {
                scan.feed(&line);
            }
        }
        scan.finish()
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_formats() {
        assert_eq!(
            parse_output("Exit code: 2\nWall time: 0.1 seconds\nOutput:\nboom\nmore"),
            ("boom\nmore".to_string(), Some(2))
        );
        assert_eq!(
            parse_output(r#"{"output":"ok","metadata":{"exit_code":0}}"#),
            ("ok".to_string(), Some(0))
        );
        assert_eq!(parse_output("plain"), ("plain".to_string(), None));
    }

    #[test]
    fn files_label_counts_rest() {
        assert_eq!(files_label(&[]), "");
        assert_eq!(files_label(&["a".into()]), "a");
        assert_eq!(
            files_label(&["a".into(), "b".into(), "c".into()]),
            "a (+2 more)"
        );
    }
}
