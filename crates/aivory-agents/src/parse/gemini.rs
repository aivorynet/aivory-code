//! Gemini CLI: `gemini -o stream-json -p <prompt>`.
//!
//! Event types (from the CLI's `JsonStreamEventType`): `init`, `message`, `tool_use`,
//! `tool_result`, `error`, `result`. The CLI could not be run on this machine (its
//! Code Assist tier is rejected), so these shapes come from the CLI source.

use aivory_sessions::Provider;
use serde_json::Value;

use super::{EventParser, json_line, output_text, str_of, u64_of};
use crate::event::{AgentEvent, Usage};

const P: Provider = Provider::Gemini;

#[derive(Debug, Default)]
pub struct GeminiParser;

impl EventParser for GeminiParser {
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
            "init" => vec![AgentEvent::Started {
                session_id: str_of(&v, "session_id"),
                model: str_of(&v, "model"),
                cwd: None,
            }],
            "message" => match v.get("role").and_then(Value::as_str) {
                Some("assistant") => vec![AgentEvent::TextDelta {
                    text: str_of(&v, "content").unwrap_or_default(),
                }],
                // The CLI echoes the user prompt.
                _ => vec![AgentEvent::raw(P, "message/user", v)],
            },
            "tool_use" => vec![AgentEvent::ToolCall {
                id: str_of(&v, "tool_id").unwrap_or_default(),
                name: str_of(&v, "tool_name").unwrap_or_default(),
                input: v.get("parameters").cloned().unwrap_or(Value::Null),
            }],
            "tool_result" => {
                let is_error = v.get("status").and_then(Value::as_str) == Some("error");
                let output = if is_error {
                    str_of(&v.get("error").cloned().unwrap_or(Value::Null), "message")
                        .or_else(|| v.get("output").map(output_text))
                        .unwrap_or_default()
                } else {
                    v.get("output").map(output_text).unwrap_or_default()
                };
                vec![AgentEvent::ToolResult {
                    id: str_of(&v, "tool_id").unwrap_or_default(),
                    output,
                    is_error,
                }]
            }
            "error" => {
                let fatal = v.get("severity").and_then(Value::as_str) == Some("error");
                vec![AgentEvent::Error {
                    message: str_of(&v, "message").unwrap_or_default(),
                    fatal,
                }]
            }
            "result" => {
                if v.get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s != "success")
                {
                    let message = v
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Gemini turn failed")
                        .to_string();
                    return vec![AgentEvent::Error {
                        message,
                        fatal: true,
                    }];
                }
                let usage = v.get("stats").map(|s| Usage {
                    input_tokens: u64_of(s, "input_tokens"),
                    output_tokens: u64_of(s, "output_tokens"),
                    cached_input_tokens: u64_of(s, "cached"),
                    reasoning_tokens: 0,
                });
                let duration_ms = v.pointer("/stats/duration_ms").and_then(Value::as_u64);
                vec![AgentEvent::TurnCompleted {
                    usage,
                    cost_usd: None,
                    duration_ms,
                }]
            }
            other => vec![AgentEvent::raw(P, other, v)],
        }
    }
}
