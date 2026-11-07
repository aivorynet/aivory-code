//! Claude Code: `claude -p --output-format stream-json --verbose --include-partial-messages`.
//!
//! Observed event types (CLI 2.1.306): `system` (subtype `init`, hooks), `stream_event`
//! (wraps the API stream: `message_start`, `content_block_delta`, ...), `assistant`, `user`
//! (tool results), `rate_limit_event`, `result`.

use aivory_sessions::Provider;
use serde_json::Value;

use super::{EventParser, json_line, output_text, str_of, u64_of};
use crate::event::{AgentEvent, Usage};

const P: Provider = Provider::Claude;

#[derive(Debug, Default)]
pub struct ClaudeParser {
    /// Text deltas were seen for the current API message, so its `assistant` text is a duplicate.
    streamed_text: bool,
}

impl EventParser for ClaudeParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let v = match json_line(P, line) {
            Ok(v) => v,
            Err(events) => return events,
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
                vec![AgentEvent::Started {
                    session_id: str_of(&v, "session_id"),
                    model: str_of(&v, "model"),
                    cwd: str_of(&v, "cwd"),
                }]
            }
            "stream_event" => self.stream_event(&v),
            "assistant" => self.assistant(&v),
            "user" => user(&v),
            "result" => result(&v),
            "control_request" => control_request(&v),
            _ => vec![raw(&v, kind)],
        }
    }
}

fn raw(v: &Value, kind: &str) -> AgentEvent {
    let full = match v.get("subtype").and_then(Value::as_str) {
        Some(sub) => format!("{kind}/{sub}"),
        None => kind.to_string(),
    };
    AgentEvent::raw(P, full, v.clone())
}

impl ClaudeParser {
    fn stream_event(&mut self, v: &Value) -> Vec<AgentEvent> {
        let event = v.get("event").unwrap_or(&Value::Null);
        let inner = event.get("type").and_then(Value::as_str).unwrap_or("");
        match inner {
            "message_start" => {
                self.streamed_text = false;
                vec![AgentEvent::raw(P, "stream_event/message_start", v.clone())]
            }
            "content_block_delta" => {
                let delta = event.get("delta").unwrap_or(&Value::Null);
                match (
                    delta.get("type").and_then(Value::as_str),
                    delta.get("text").and_then(Value::as_str),
                ) {
                    (Some("text_delta"), Some(text)) => {
                        self.streamed_text = true;
                        vec![AgentEvent::TextDelta {
                            text: text.to_string(),
                        }]
                    }
                    _ => vec![AgentEvent::raw(
                        P,
                        "stream_event/content_block_delta",
                        v.clone(),
                    )],
                }
            }
            other => vec![AgentEvent::raw(
                P,
                format!("stream_event/{other}"),
                v.clone(),
            )],
        }
    }

    fn assistant(&mut self, v: &Value) -> Vec<AgentEvent> {
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
            return vec![raw(v, "assistant")];
        };
        let mut out = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if !self.streamed_text
                        && let Some(text) = block.get("text").and_then(Value::as_str)
                    {
                        out.push(AgentEvent::TextDelta {
                            text: text.to_string(),
                        });
                    }
                }
                Some("tool_use") => out.push(AgentEvent::ToolCall {
                    id: str_of(block, "id").unwrap_or_default(),
                    name: str_of(block, "name").unwrap_or_default(),
                    input: block.get("input").cloned().unwrap_or(Value::Null),
                }),
                Some(other) => out.push(AgentEvent::raw(
                    P,
                    format!("assistant/{other}"),
                    block.clone(),
                )),
                None => out.push(AgentEvent::raw(P, "assistant/unknown_block", block.clone())),
            }
        }
        out
    }
}

fn user(v: &Value) -> Vec<AgentEvent> {
    let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
        return vec![raw(v, "user")];
    };
    let mut out = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
            out.push(AgentEvent::ToolResult {
                id: str_of(block, "tool_use_id").unwrap_or_default(),
                output: block.get("content").map(output_text).unwrap_or_default(),
                is_error: block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        } else {
            out.push(AgentEvent::raw(P, "user/content", block.clone()));
        }
    }
    out
}

fn result(v: &Value) -> Vec<AgentEvent> {
    let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
    if is_error || (!subtype.is_empty() && subtype != "success") {
        let message = str_of(v, "result")
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| format!("Claude turn failed ({subtype})"));
        return vec![AgentEvent::Error {
            message,
            fatal: true,
        }];
    }
    let mut out = Vec::new();
    if let Some(denials) = v.get("permission_denials").and_then(Value::as_array) {
        for d in denials {
            out.push(AgentEvent::PermissionRequest {
                id: str_of(d, "tool_use_id").unwrap_or_default(),
                tool: str_of(d, "tool_name").unwrap_or_default(),
                input: d.get("tool_input").cloned().unwrap_or(Value::Null),
                outcome: Some("denied".to_string()),
            });
        }
    }
    let usage = v.get("usage").map(|u| Usage {
        input_tokens: u64_of(u, "input_tokens"),
        output_tokens: u64_of(u, "output_tokens"),
        cached_input_tokens: u64_of(u, "cache_read_input_tokens"),
        reasoning_tokens: u
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    });
    out.push(AgentEvent::TurnCompleted {
        usage,
        cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
        duration_ms: v.get("duration_ms").and_then(Value::as_u64),
    });
    out
}

/// Only seen with `--permission-prompt-tool stdio`; mapped for completeness.
fn control_request(v: &Value) -> Vec<AgentEvent> {
    let req = v.get("request").unwrap_or(&Value::Null);
    if req.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
        return vec![AgentEvent::PermissionRequest {
            id: str_of(v, "request_id").unwrap_or_default(),
            tool: str_of(req, "tool_name").unwrap_or_default(),
            input: req.get("input").cloned().unwrap_or(Value::Null),
            outcome: None,
        }];
    }
    vec![raw(v, "control_request")]
}
