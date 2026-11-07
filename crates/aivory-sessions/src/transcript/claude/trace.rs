//! Spans of a Claude session: the lane of the main file and a lane for each sub-agent file.
//!
//! - `assistant` entries with the same `message.id` are one LLM call. Claude writes one entry
//!   per block, and each entry has the usage of the whole message. The call starts at the last
//!   prompt or tool result before it and ends at its last block.
//! - A `tool_use` block starts a tool call. The `tool_result` block with the same id ends it.
//! - A call of `Agent` or `Task` is a sub-agent call. The meta file of a sub-agent names the
//!   call in `toolUseId`.
//! - Tokens in are `input_tokens`, `cache_creation_input_tokens` and `cache_read_input_tokens`.
//!   Tokens out are `output_tokens`.

use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::Path;

use aivory_protocol::UnixMs;
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use super::{
    Content, Entry, Meta, Prompt, SUBAGENT_TOOLS, SYNTHETIC_MODEL, agent_id, classify_prompt,
    is_system_error, mtime_ms, origin_is_human, parse_attachment, subagent_files,
};
use crate::transcript::common::{ReadError, for_each_line, main_arg, one_line, str_field};
use crate::transcript::spans::{AgentLane, Lane, LlmUpdate, NO_META_KIND, Spawn, head};

#[derive(Deserialize, Default)]
struct Usage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// True when the usage block names a cache count. Old transcripts have none.
fn has_cache_fields(u: &Usage) -> bool {
    u.cache_creation_input_tokens.is_some() || u.cache_read_input_tokens.is_some()
}

/// The lane of the main file and the lanes of the sub-agent files. The sub-agent files are
/// read in parallel.
pub(in crate::transcript) fn lanes(
    path: &Path,
    fallback: UnixMs,
) -> Result<(Lane, Vec<AgentLane>), ReadError> {
    let main = file_lane(path, fallback)?;
    let agents = subagent_files(path)
        .par_iter()
        .filter_map(|p| agent_lane(p, fallback))
        .collect();
    Ok((main, agents))
}

fn file_lane(path: &Path, fallback: UnixMs) -> io::Result<Lane> {
    let reader = BufReader::with_capacity(256 * 1024, File::open(path)?);
    let mut lane = Lane::new(fallback);
    for_each_line(reader, |raw| {
        if let Ok(entry) = serde_json::from_slice::<Entry>(raw) {
            feed(&mut lane, &entry);
        }
    })?;
    Ok(lane)
}

fn agent_lane(path: &Path, fallback: UnixMs) -> Option<AgentLane> {
    let id = agent_id(path)?;
    let meta: Meta = fs::read(path.with_extension("meta.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let lane = file_lane(path, mtime_ms(path).unwrap_or(fallback)).ok()?;
    Some(AgentLane {
        id,
        kind: meta.agent_type.unwrap_or_else(|| NO_META_KIND.to_string()),
        task: meta
            .description
            .as_deref()
            .map(one_line)
            .unwrap_or_default(),
        tool_use_id: meta.tool_use_id,
        finished: meta.finished == Some(true),
        lane,
    })
}

fn feed(lane: &mut Lane, e: &Entry) {
    match e.kind.as_deref() {
        Some("user") => user(lane, e),
        Some("assistant") => assistant(lane, e),
        Some("system") if is_system_error(e) => {
            let at = lane.at(e.time());
            lane.fail(at);
        }
        Some("attachment") => queued_prompt(lane, e),
        _ => {}
    }
}

/// The first `OUTPUT_CHARS` characters of the content of a `tool_result` block, which is a
/// string or a list of parts. The function does not copy the whole text.
fn snippet(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => head(s),
        Some(Value::Array(parts)) => {
            let mut text = String::new();
            for t in parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
            {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.extend(t.chars().take(1200));
                if text.len() >= 1200 {
                    break;
                }
            }
            head(&text)
        }
        _ => String::new(),
    }
}

fn user(lane: &mut Lane, e: &Entry) {
    if e.is_meta == Some(true) || e.is_compact_summary == Some(true) {
        return;
    }
    let Some(msg) = e.message() else { return };
    let at = lane.at(e.time());
    for b in msg.blocks() {
        if b.kind.as_deref() == Some("tool_result")
            && let Some(id) = &b.tool_use_id
        {
            lane.tool_end(
                at,
                id,
                &snippet(b.content.as_ref()),
                b.is_error == Some(true),
            );
        }
    }
    if e.human() {
        prompt(lane, at, &msg.text());
    }
}

fn prompt(lane: &mut Lane, at: UnixMs, raw: &str) {
    match classify_prompt(raw) {
        Prompt::Text(text) => lane.prompt(at, &text),
        Prompt::Interrupt => lane.finish(),
        Prompt::Skip => {}
    }
}

/// A prompt that the user typed while the agent worked.
fn queued_prompt(lane: &mut Lane, e: &Entry) {
    let Some(a) = parse_attachment(e) else { return };
    if a.kind.as_deref() == Some("queued_command")
        && origin_is_human(a.origin.as_ref())
        && let Some(text) = a.prompt
    {
        let at = lane.at(e.time());
        prompt(lane, at, &text);
    }
}

fn assistant(lane: &mut Lane, e: &Entry) {
    let Some(msg) = e.message() else { return };
    let at = lane.at(e.time());
    let synthetic = msg.model.as_deref() == Some(SYNTHETIC_MODEL);
    if e.api_error() || (synthetic && e.error.is_some()) {
        lane.fail(at);
        return;
    }
    if synthetic {
        return;
    }
    let usage: Option<Usage> = msg
        .usage
        .as_ref()
        .and_then(|u| serde_json::from_str(u.get()).ok());
    let blocks = msg.blocks();
    let text = match &msg.content {
        Some(Content::Text(t)) => Some(t.as_str()),
        _ => blocks
            .iter()
            .filter(|b| b.kind.as_deref() == Some("text"))
            .filter_map(|b| b.text.as_deref())
            .find(|t| !t.trim().is_empty()),
    };
    let tools = blocks
        .iter()
        .filter(|b| b.kind.as_deref() == Some("tool_use"));
    lane.llm(
        at,
        &LlmUpdate {
            key: msg.id.as_deref().or(e.request_id.as_deref()),
            model: msg.model.as_deref(),
            tokens_in: usage.as_ref().map(|u| {
                u.input_tokens.unwrap_or(0)
                    + u.cache_creation_input_tokens.unwrap_or(0)
                    + u.cache_read_input_tokens.unwrap_or(0)
            }),
            tokens_out: usage.as_ref().map(|u| u.output_tokens.unwrap_or(0)),
            cache_read: usage
                .as_ref()
                .filter(|u| has_cache_fields(u))
                .map(|u| u.cache_read_input_tokens.unwrap_or(0)),
            cache_write: usage
                .as_ref()
                .filter(|u| has_cache_fields(u))
                .map(|u| u.cache_creation_input_tokens.unwrap_or(0)),
            text,
            calls: tools
                .clone()
                .map(|b| b.name.as_deref().unwrap_or("tool"))
                .collect(),
        },
    );
    for b in tools {
        let name = b.name.as_deref().unwrap_or("tool");
        let input = b.input.as_ref().unwrap_or(&Value::Null);
        let spawn = SUBAGENT_TOOLS.contains(&name).then(|| Spawn {
            kind: str_field(input, "subagent_type")
                .unwrap_or("general-purpose")
                .to_string(),
            task: str_field(input, "description")
                .or_else(|| str_field(input, "prompt"))
                .map(one_line)
                .unwrap_or_default(),
        });
        lane.tool_start(at, b.id.as_deref(), name, main_arg(input), spawn);
    }
    match msg.stop_reason.as_deref() {
        None | Some("" | "tool_use") => lane.resume(),
        Some(_) => lane.finish(),
    }
}
