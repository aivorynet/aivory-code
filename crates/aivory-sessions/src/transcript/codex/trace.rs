//! Spans of a Codex session.
//!
//! - An LLM call is the run of `response_item` entries (reasoning, message, tool calls) between
//!   two user side events. It starts at the last prompt or tool output. It ends at its last
//!   item. The `token_count` event after it gives the tokens (`last_token_usage`).
//! - A tool call starts at its `function_call`, `custom_tool_call` or `local_shell_call` item
//!   and ends at the output item with the same `call_id`. A shell call that exits with a code
//!   other than `0` failed. A web search has no output and ends where it starts.
//! - A `user_message` event opens a turn. `task_complete` and `turn_aborted` end it.

use std::io::{self, BufRead};

use aivory_protocol::UnixMs;
use serde_json::Value;

use super::{CodexLine, PATCH_TOOL, call_args, files_label, message_text, parse_output};
use crate::transcript::common::{for_each_line, main_arg, one_line, patch_lines, str_field};
use crate::transcript::spans::{Lane, LlmUpdate};

pub(in crate::transcript) fn lane<R: BufRead>(reader: R, fallback: UnixMs) -> io::Result<Lane> {
    let mut feed = Feed {
        lane: Lane::new(fallback),
        key: None,
        count: 0,
    };
    for_each_line(reader, |raw| {
        if let Ok(line) = serde_json::from_slice::<CodexLine>(raw) {
            feed.line(&line);
        }
    })?;
    Ok(feed.lane)
}

struct Feed {
    lane: Lane,
    /// The key of the LLM call that is open.
    key: Option<String>,
    count: u32,
}

impl Feed {
    fn line(&mut self, l: &CodexLine) {
        let Some(p) = &l.payload else { return };
        let at = self.lane.at(l.time());
        let ptype = str_field(p, "type");
        match (l.kind.as_deref(), ptype) {
            (Some("turn_context"), _) => {
                self.lane
                    .set_model(str_field(p, "model").unwrap_or_default());
            }
            (Some("event_msg"), Some(t)) => self.event(t, p, at),
            (Some("response_item"), Some(t)) => self.item(t, p, at),
            _ => {}
        }
    }

    fn event(&mut self, kind: &str, p: &Value, at: UnixMs) {
        let message = str_field(p, "message").unwrap_or_default();
        match kind {
            "user_message" if !message.trim().is_empty() => {
                self.key = None;
                self.lane.prompt(at, message);
            }
            "agent_message" => self.llm(at, Some(message), None),
            "token_count" => {
                let usage = p.get("info").and_then(|i| i.get("last_token_usage"));
                if let (Some(key), Some(u)) = (&self.key, usage) {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    // `input_tokens` includes the cached tokens.
                    let input = n("input_tokens");
                    let cached = u
                        .get("cached_input_tokens")
                        .and_then(Value::as_u64)
                        .map(|c| c.min(input));
                    self.lane.llm_tokens(key, input, n("output_tokens"), cached);
                }
                self.key = None;
            }
            "task_complete" | "turn_aborted" => {
                self.key = None;
                self.lane.finish();
            }
            "error" | "stream_error" => self.lane.fail(at),
            _ => {}
        }
    }

    /// The model asked for something: it opens an LLM call, or adds to the open call.
    fn llm(&mut self, at: UnixMs, text: Option<&str>, call: Option<&str>) {
        if self.key.is_none() {
            self.count += 1;
            self.key = Some(format!("c{}", self.count));
        }
        self.lane.llm(
            at,
            &LlmUpdate {
                key: self.key.as_deref(),
                text: text.filter(|t| !t.trim().is_empty()),
                calls: call.into_iter().collect(),
                ..LlmUpdate::default()
            },
        );
        self.lane.resume();
    }

    fn item(&mut self, kind: &str, p: &Value, at: UnixMs) {
        let call_id = str_field(p, "call_id");
        match kind {
            "reasoning" => self.llm(at, None, None),
            "message" if str_field(p, "role") == Some("assistant") => {
                self.llm(at, Some(&message_text(p)), None);
            }
            "function_call" => {
                let name = str_field(p, "name").unwrap_or("tool");
                self.llm(at, None, Some(name));
                self.lane
                    .tool_start(at, call_id, name, main_arg(&call_args(p)), None);
            }
            "custom_tool_call" => {
                let name = str_field(p, "name").unwrap_or("tool");
                let input = str_field(p, "input").unwrap_or_default();
                let arg = if name == PATCH_TOOL {
                    files_label(&patch_lines(input).0)
                } else {
                    one_line(input)
                };
                self.llm(at, None, Some(name));
                self.lane.tool_start(at, call_id, name, arg, None);
            }
            "local_shell_call" => {
                self.llm(at, None, Some("shell"));
                let arg = p.get("action").map(main_arg).unwrap_or_default();
                self.lane.tool_start(at, call_id, "shell", arg, None);
            }
            "web_search_call" => {
                self.llm(at, None, Some("web_search"));
                let arg = p.get("action").map(main_arg).unwrap_or_default();
                self.lane.tool_instant(at, "web_search", arg);
            }
            "function_call_output" | "custom_tool_call_output" => {
                let (text, code) = parse_output(str_field(p, "output").unwrap_or_default());
                // The next response is a new LLM call.
                self.key = None;
                if let Some(id) = call_id {
                    self.lane
                        .tool_end(at, id, &text, code.is_some_and(|c| c != 0));
                }
            }
            _ => {}
        }
    }
}
