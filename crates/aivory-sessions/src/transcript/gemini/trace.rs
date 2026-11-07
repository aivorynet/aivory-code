//! Spans of a Gemini session.
//!
//! - A `gemini` message is one LLM call. It ends at the time of the message. It starts at the
//!   last prompt, or at the end of the last tool call before it. `tokens.input` and
//!   `tokens.tool` are the tokens in. `tokens.output` and `tokens.thoughts` are the tokens out.
//! - A tool call starts at the time of its message. It ends at its own `timestamp`, but not
//!   before it starts. A call whose `status` is not `success`, `error` or `cancelled`, and that
//!   has no result, is open.
//! - A `user` message that is not the session context opens a turn.

use std::io::BufRead;

use aivory_protocol::UnixMs;
use serde_json::Value;

use super::{CONTEXT_PREFIX, Msg, Tool, content_text, load, tool_result};
use crate::timeutil::parse_rfc3339_ms;
use crate::transcript::common::{ReadError, main_arg};
use crate::transcript::spans::{Lane, LlmUpdate};

pub(in crate::transcript) fn lane<R: BufRead>(
    reader: R,
    document: bool,
    fallback: UnixMs,
) -> Result<Lane, ReadError> {
    let session = load(reader, document)?;
    let started = session.start_time.as_deref().and_then(parse_rfc3339_ms);
    let mut lane = Lane::new(started.unwrap_or(fallback));
    for (index, m) in session.msgs.iter().enumerate() {
        let at = lane.at(m.timestamp.as_deref().and_then(parse_rfc3339_ms));
        match m.kind.as_deref() {
            Some("user") => {
                let text = content_text(m.content.as_ref());
                if !text.trim().is_empty() && !text.trim_start().starts_with(CONTEXT_PREFIX) {
                    lane.prompt(at, &text);
                }
            }
            Some("gemini") => answer(&mut lane, index, m, at),
            Some("error") => lane.fail(at),
            _ => {}
        }
    }
    Ok(lane)
}

fn answer(lane: &mut Lane, index: usize, m: &Msg, at: UnixMs) {
    let tools: &[Tool] = m.tool_calls.as_deref().unwrap_or_default();
    let text = content_text(m.content.as_ref());
    let count = |key: &str| {
        m.tokens
            .as_ref()
            .and_then(|t| t.get(key))
            .and_then(Value::as_u64)
    };
    let has_tokens = m.tokens.is_some();
    lane.llm(
        at,
        &LlmUpdate {
            key: None,
            model: m.model.as_deref(),
            tokens_in: has_tokens.then(|| count("input").unwrap_or(0) + count("tool").unwrap_or(0)),
            tokens_out: has_tokens
                .then(|| count("output").unwrap_or(0) + count("thoughts").unwrap_or(0)),
            text: Some(&text)
                .filter(|t| !t.trim().is_empty())
                .map(String::as_str),
            calls: tools
                .iter()
                .map(|t| t.name.as_deref().unwrap_or("tool"))
                .collect(),
            // Whether `input` includes the cached count is not known: no split.
            ..LlmUpdate::default()
        },
    );
    for (i, t) in tools.iter().enumerate() {
        let name = t.name.as_deref().unwrap_or("tool");
        let args = t.args.as_ref().unwrap_or(&Value::Null);
        // The id of a call is unique in a message only, so the key holds the message.
        let key = format!("{index}:{}", t.id.as_deref().unwrap_or(&i.to_string()));
        lane.tool_start(at, Some(&key), name, main_arg(args), None);
        let (output, failed) = tool_result(t);
        if is_closed(t) {
            let end = t
                .timestamp
                .as_ref()
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_ms)
                .map_or(at, |e| e.max(at));
            lane.tool_end(end, &key, &output, failed);
        }
    }
    if tools.is_empty() {
        lane.finish();
    } else {
        lane.resume();
    }
}

/// A call that ended: it has a final status or a result.
fn is_closed(t: &Tool) -> bool {
    matches!(t.status.as_deref(), Some("success" | "error" | "cancelled"))
        || t.result
            .as_ref()
            .and_then(Value::as_array)
            .is_some_and(|r| !r.is_empty())
}
