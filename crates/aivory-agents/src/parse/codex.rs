//! Codex CLI: `codex exec --json`.
//!
//! Event types: `thread.started`, `turn.started`, `item.started|updated|completed`,
//! `turn.completed`, `turn.failed`, `error`. Observed on this machine: `thread.started`,
//! `turn.started`, `error`, `turn.failed` (every model was rejected for the ChatGPT account).
//! The `item.*` shapes follow the CLI's published exec event schema.
//! Item types: `agent_message`, `reasoning`, `command_execution`, `file_change`,
//! `mcp_tool_call`, `web_search`, `todo_list`, `error`.

use std::collections::HashSet;

use aivory_sessions::Provider;
use serde_json::{Value, json};

use super::{EventParser, json_line, output_text, str_of, u64_of};
use crate::event::{AgentEvent, Usage};

const P: Provider = Provider::Codex;

#[derive(Debug, Default)]
pub struct CodexParser {
    announced: HashSet<String>,
    last_error: Option<String>,
}

impl EventParser for CodexParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let v = match json_line(P, line) {
            Ok(v) => v,
            Err(events) => return events,
        };
        let kind = v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match kind.as_str() {
            "thread.started" => {
                vec![AgentEvent::Started {
                    session_id: str_of(&v, "thread_id"),
                    model: None,
                    cwd: None,
                }]
            }
            "item.started" | "item.updated" | "item.completed" => {
                let phase = kind.trim_start_matches("item.").to_string();
                match v.get("item") {
                    Some(item) => self.item(&phase, item),
                    None => vec![AgentEvent::raw(P, kind, v)],
                }
            }
            "turn.completed" => {
                let usage = v.get("usage").map(|u| Usage {
                    input_tokens: u64_of(u, "input_tokens"),
                    output_tokens: u64_of(u, "output_tokens"),
                    cached_input_tokens: u64_of(u, "cached_input_tokens"),
                    reasoning_tokens: u64_of(u, "reasoning_output_tokens"),
                });
                vec![AgentEvent::TurnCompleted {
                    usage,
                    cost_usd: None,
                    duration_ms: None,
                }]
            }
            "turn.failed" => {
                let message = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(error_text)
                    .unwrap_or_else(|| "turn failed".into());
                self.error(message, true)
            }
            "error" => {
                let message = str_of(&v, "message")
                    .map(|m| error_text(&m))
                    .unwrap_or_else(|| "error".into());
                self.error(message, false)
            }
            other => vec![AgentEvent::raw(P, other, v)],
        }
    }
}

impl CodexParser {
    /// Codex prints `error` and then `turn.failed` with the same text. Report it once, as fatal.
    fn error(&mut self, message: String, fatal: bool) -> Vec<AgentEvent> {
        if self.last_error.as_deref() == Some(message.as_str()) {
            return if fatal {
                vec![AgentEvent::Error {
                    message,
                    fatal: true,
                }]
            } else {
                Vec::new()
            };
        }
        self.last_error = Some(message.clone());
        // The first (`error`) event is not fatal on its own; `turn.failed` ends the turn.
        vec![AgentEvent::Error { message, fatal }]
    }

    /// `true` the first time a tool id is seen.
    fn announce(&mut self, id: &str) -> bool {
        self.announced.insert(id.to_string())
    }

    fn item(&mut self, phase: &str, item: &Value) -> Vec<AgentEvent> {
        let id = str_of(item, "id").unwrap_or_default();
        let done = phase == "completed";
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        let status_failed = item.get("status").and_then(Value::as_str) == Some("failed");
        let mut out = Vec::new();
        match item_type {
            "agent_message" if done => {
                out.push(AgentEvent::TextDelta {
                    text: str_of(item, "text").unwrap_or_default(),
                });
            }
            "command_execution" => {
                if self.announce(&id) {
                    out.push(AgentEvent::ToolCall {
                        id: id.clone(),
                        name: "command_execution".into(),
                        input: json!({ "command": item.get("command") }),
                    });
                }
                if done {
                    let exit_failed = item
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .is_some_and(|c| c != 0);
                    out.push(AgentEvent::ToolResult {
                        id,
                        output: str_of(item, "aggregated_output").unwrap_or_default(),
                        is_error: exit_failed || status_failed,
                    });
                }
            }
            "file_change" => {
                if self.announce(&id) {
                    out.push(AgentEvent::ToolCall {
                        id: id.clone(),
                        name: "file_change".into(),
                        input: json!({ "changes": item.get("changes") }),
                    });
                }
                if done {
                    out.push(AgentEvent::ToolResult {
                        id,
                        output: str_of(item, "status").unwrap_or_default(),
                        is_error: status_failed,
                    });
                }
            }
            "mcp_tool_call" => {
                if self.announce(&id) {
                    let name = format!(
                        "{}/{}",
                        str_of(item, "server").unwrap_or_default(),
                        str_of(item, "tool").unwrap_or_default()
                    );
                    out.push(AgentEvent::ToolCall {
                        id: id.clone(),
                        name,
                        input: item.get("arguments").cloned().unwrap_or(Value::Null),
                    });
                }
                if done {
                    let error = item
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let output = error.clone().unwrap_or_else(|| {
                        item.get("result")
                            .map(|r| output_text(r.get("content").unwrap_or(r)))
                            .unwrap_or_default()
                    });
                    out.push(AgentEvent::ToolResult {
                        id,
                        output,
                        is_error: error.is_some() || status_failed,
                    });
                }
            }
            "web_search" => {
                if self.announce(&id) {
                    out.push(AgentEvent::ToolCall {
                        id: id.clone(),
                        name: "web_search".into(),
                        input: json!({ "query": item.get("query") }),
                    });
                }
                if done {
                    out.push(AgentEvent::ToolResult {
                        id,
                        output: String::new(),
                        is_error: status_failed,
                    });
                }
            }
            "error" => out.push(AgentEvent::Error {
                message: str_of(item, "message").unwrap_or_default(),
                fatal: false,
            }),
            _ => out.push(AgentEvent::raw(
                P,
                format!("item.{phase}/{item_type}"),
                item.clone(),
            )),
        }
        out
    }
}

/// Codex wraps API errors as a JSON string: `{"type":"error","status":400,"error":{"message":"..."}}`.
fn error_text(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| raw.to_string())
}
