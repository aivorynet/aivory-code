//! Gemini CLI transcripts: `chats/session-<ts>-<id8>.{json,jsonl}`.
//!
//! - `.json` (older CLI): one object `{sessionId, startTime, messages[]}`.
//! - `.jsonl` (CLI 0.49): a header record `{sessionId, startTime, kind}`, then message records.
//!   A record with the `id` of an earlier record replaces it. `{"$set": ...}` records update
//!   the metadata. `{"$rewindTo": id}` removes the message with this id and all later ones.
//!
//! A message is `{id, timestamp, type, content, model?, toolCalls?}` with `type` `user`,
//! `gemini`, `info`, `warning` or `error`. A tool call sits in the `toolCalls` of the `gemini`
//! message, with its own `status` (`success`, `error`, `cancelled`, ...), `args`, and
//! `result[].functionResponse.response{output | error}`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use aivory_protocol::{Line, LineKind};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use super::common::{
    Facts, ReadError, added_lines, error_ask, for_each_line, main_arg, one_line, permission_ask,
    plan_ask, push_diff, push_output, push_prompt, push_text, question_ask, replace_lines,
    str_field, turn_line, with_tail,
};
use crate::timeutil::{format_utc_minutes, parse_rfc3339_ms};

mod trace;

pub(super) use trace::lane;

const SUBAGENT_TOOLS: [&str; 4] = [
    "cli_help",
    "codebase_investigator",
    "generalist",
    "delegate_to_agent",
];
const EDIT_TOOLS: [&str; 2] = ["replace", "write_file"];
const QUESTION_TOOL: &str = "ask_user";
const PLAN_TOOL: &str = "exit_plan_mode";
const CONTEXT_PREFIX: &str = "<session_context>";

#[derive(Deserialize, Default)]
struct Msg {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    content: Option<Value>,
    model: Option<String>,
    #[serde(rename = "toolCalls")]
    tool_calls: Option<Vec<Tool>>,
    /// `{input, output, cached, thoughts, tool, total}`.
    tokens: Option<Value>,
    #[serde(rename = "$rewindTo")]
    rewind_to: Option<String>,
    #[serde(rename = "$set")]
    set: Option<IgnoredAny>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "startTime")]
    start_time: Option<String>,
}

#[derive(Deserialize, Default)]
struct Tool {
    id: Option<String>,
    name: Option<String>,
    args: Option<Value>,
    status: Option<String>,
    /// The time when the call ended.
    timestamp: Option<Value>,
    result: Option<Value>,
    #[serde(rename = "resultDisplay")]
    result_display: Option<Value>,
}

#[derive(Deserialize, Default)]
struct Doc {
    #[serde(rename = "startTime")]
    start_time: Option<String>,
    #[serde(default)]
    messages: Vec<Msg>,
}

/// The messages of a file, in order, after the replacements and rewinds.
#[derive(Default)]
struct Session {
    start_time: Option<String>,
    msgs: Vec<Msg>,
    index: HashMap<String, usize>,
}

impl Session {
    fn from_doc(doc: Doc) -> Self {
        let mut s = Self {
            start_time: doc.start_time,
            ..Self::default()
        };
        for m in doc.messages {
            s.upsert(m);
        }
        s
    }

    /// One record of a `.jsonl` file. A line that is not valid JSON is skipped.
    fn feed(&mut self, raw: &[u8]) {
        let Ok(m) = serde_json::from_slice::<Msg>(raw) else {
            return;
        };
        if let Some(id) = &m.rewind_to {
            if let Some(at) = self.index.get(id).copied() {
                self.msgs.truncate(at);
                self.index.retain(|_, i| *i < at);
            }
            return;
        }
        if m.set.is_some() {
            return;
        }
        if m.kind.is_none() {
            // The header record.
            if m.session_id.is_some() && self.start_time.is_none() {
                self.start_time = m.start_time;
            }
            return;
        }
        self.upsert(m);
    }

    fn upsert(&mut self, m: Msg) {
        match m.id.clone().filter(|id| !id.is_empty()) {
            Some(id) => match self.index.get(&id) {
                Some(&at) => self.msgs[at] = m,
                None => {
                    self.index.insert(id, self.msgs.len());
                    self.msgs.push(m);
                }
            },
            None => self.msgs.push(m),
        }
    }
}

pub(super) fn is_document(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("json")
}

fn load<R: BufRead>(reader: R, document: bool) -> Result<Session, ReadError> {
    if document {
        return Ok(Session::from_doc(serde_json::from_reader(reader)?));
    }
    let mut s = Session::default();
    for_each_line(reader, |raw| s.feed(raw))?;
    Ok(s)
}

/// The text of a message `content`: a string, or the `text` of a list of parts.
fn content_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The output of a tool call and whether it failed.
fn tool_result(t: &Tool) -> (String, bool) {
    let mut text = String::new();
    let mut failed = t.status.as_deref() == Some("error");
    let parts = t.result.as_ref().and_then(Value::as_array);
    for part in parts.into_iter().flatten() {
        let response = part.get("functionResponse").and_then(|f| f.get("response"));
        for (key, is_error) in [("output", false), ("error", true)] {
            if let Some(s) = response.and_then(|r| str_field(r, key)) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(s);
                failed |= is_error;
            }
        }
    }
    if text.is_empty()
        && let Some(s) = t.result_display.as_ref().and_then(Value::as_str)
    {
        text = s.to_string();
    }
    (text, failed)
}

// ---- normalize ------------------------------------------------------------------------------

pub(super) fn normalize<R: BufRead>(reader: R, document: bool) -> Result<Vec<Line>, ReadError> {
    Ok(render(&load(reader, document)?))
}

fn render(s: &Session) -> Vec<Line> {
    let mut out = Vec::new();
    let mut turns = 0u32;
    let mut model: Option<&str> = None;
    let started = s.start_time.as_deref().and_then(parse_rfc3339_ms);
    out.push(Line::meta(match started {
        Some(t) => format!("session start · {}", format_utc_minutes(t)),
        None => "session start".to_string(),
    }));
    for m in &s.msgs {
        let at = m.timestamp.as_deref().and_then(parse_rfc3339_ms);
        let text = content_text(m.content.as_ref());
        match m.kind.as_deref() {
            Some("user") => {
                if text.trim().is_empty() || text.trim_start().starts_with(CONTEXT_PREFIX) {
                    continue;
                }
                turns += 1;
                out.push(turn_line(turns, at));
                push_prompt(&mut out, &text);
            }
            Some("gemini") => {
                if let Some(name) = m.model.as_deref()
                    && model != Some(name)
                {
                    out.push(Line::meta(format!("model · {name}")));
                    model = Some(name);
                }
                push_text(&mut out, LineKind::Agent, &text);
                for t in m.tool_calls.iter().flatten() {
                    render_tool(&mut out, t);
                }
            }
            Some("info" | "warning") => {
                let note = one_line(&text);
                if !note.is_empty() {
                    out.push(Line::meta(note));
                }
            }
            Some("error") => push_output(&mut out, &text, true),
            _ => {}
        }
    }
    out
}

fn render_tool(out: &mut Vec<Line>, t: &Tool) {
    let name = t.name.as_deref().unwrap_or("tool");
    let args = t.args.as_ref().unwrap_or(&Value::Null);
    if SUBAGENT_TOOLS.contains(&name) {
        let label = str_field(args, "agent_name").unwrap_or(name);
        out.push(Line::subagent(label, main_arg(args)));
    } else {
        out.push(Line::tool(name, main_arg(args)));
        let diff = match name {
            "replace" => replace_lines(
                str_field(args, "old_string").unwrap_or_default(),
                str_field(args, "new_string").unwrap_or_default(),
            ),
            "write_file" => added_lines(str_field(args, "content").unwrap_or_default()),
            _ => Vec::new(),
        };
        push_diff(out, diff);
    }
    let (text, failed) = tool_result(t);
    if failed || !EDIT_TOOLS.contains(&name) {
        push_output(out, &text, failed);
    }
}

// ---- state ----------------------------------------------------------------------------------

/// A tool call is open when it has no final status.
fn is_open(t: &Tool) -> bool {
    match t.status.as_deref() {
        Some("success" | "error" | "cancelled") => false,
        Some(_) => true,
        None => t.result.is_none(),
    }
}

fn scan(s: &Session) -> Option<Facts> {
    let last = s
        .msgs
        .iter()
        .rev()
        .find(|m| matches!(m.kind.as_deref(), Some("user" | "gemini" | "error")))?;
    let at = last.timestamp.as_deref().and_then(parse_rfc3339_ms);
    let mut facts = Facts {
        since: at,
        ..Facts::default()
    };
    let id = last.id.clone().unwrap_or_default();
    match last.kind.as_deref() {
        Some("error") => {
            let text = content_text(last.content.as_ref());
            facts.error = Some(error_ask(&format!("error-{id}"), &text));
        }
        Some("gemini") => {
            let calls = last.tool_calls.as_deref().unwrap_or_default();
            let open = calls.iter().find(|t| {
                is_open(t)
                    && t.name
                        .as_deref()
                        .is_none_or(|n| !SUBAGENT_TOOLS.contains(&n))
            });
            facts.ask = open.map(|t| {
                let name = t.name.as_deref().unwrap_or("tool");
                let args = t.args.as_ref().unwrap_or(&Value::Null);
                let call_id = t.id.as_deref().unwrap_or(&id);
                match name {
                    QUESTION_TOOL => question_ask(call_id, args),
                    PLAN_TOOL => plan_ask(call_id, str_field(args, "plan")),
                    _ => permission_ask(call_id, name, args),
                }
            });
            // Answers of finished tool calls go back to the model: the turn goes on.
            facts.ended = calls.is_empty();
        }
        _ => {}
    }
    Some(facts)
}

pub(super) fn facts(path: &Path) -> Result<Option<Facts>, ReadError> {
    if is_document(path) {
        let s = load(BufReader::new(File::open(path)?), true)?;
        return Ok(scan(&s));
    }
    Ok(with_tail(path, |lines| {
        let mut s = Session::default();
        for raw in lines {
            s.feed(raw);
        }
        scan(&s)
    })?)
}
