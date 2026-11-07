//! Claude Code transcripts: `<projects>/<slug>/<sessionId>.jsonl`.
//!
//! One JSON object per line. The conversation is in the entries of type `user`, `assistant`
//! and `system`. Everything else is bookkeeping and is skipped (`attachment`, `queue-operation`,
//! `ai-title`, `last-prompt`, `file-history-*`, `api-request*`, ...). The only attachment that
//! counts is `queued_command` with `origin.kind == "human"`: a prompt typed during a turn.
//!
//! - `assistant`: `message.content[]` holds `thinking`, `text` and `tool_use` blocks. Claude
//!   writes one entry per block. An API error is an entry with `isApiErrorMessage: true`.
//! - `user`: `message.content` is a string or blocks. A `tool_result` block answers a
//!   `tool_use` by `tool_use_id`. `isMeta` entries are noise.
//! - Sub-agents: `<projects>/<slug>/<sessionId>/subagents/agent-<id>.jsonl` with the same entry
//!   format (`isSidechain: true`) and `agent-<id>.meta.json` with `agentType`, `description`,
//!   `toolUseId` and `finished`. Older versions wrote `<slug>/agent-<id>.jsonl`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use aivory_protocol::{Ask, Line, LineKind, SessionState, Subagent, UnixMs};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::common::{
    Facts, ReadError, added_lines, between, error_ask, for_each_line, main_arg, permission_ask,
    plan_ask, push_diff, push_output, push_prompt, push_text, question_ask, replace_lines,
    str_field, turn_line, with_tail,
};
use super::derive_state;
use crate::model::Provider;
use crate::timeutil::parse_rfc3339_ms;

mod trace;

pub(super) use trace::lanes;

const SYNTHETIC_MODEL: &str = "<synthetic>";
/// Tools whose result is a fixed success text. The diff shows the change.
const SILENT_RESULT_TOOLS: [&str; 5] = ["Edit", "Write", "MultiEdit", "NotebookEdit", "TodoWrite"];
const SUBAGENT_TOOLS: [&str; 2] = ["Agent", "Task"];
/// Blocks that Claude puts in a prompt for the model only. They are not the user's text.
const DROP_TAGS: [&str; 15] = [
    "system-reminder",
    "local-command-caveat",
    "local-command-stdout",
    "local-command-stderr",
    "bash-stdout",
    "bash-stderr",
    "command-message",
    "ide_opened_file",
    "ide_selection",
    "ide_diagnostics",
    "task-notification",
    "user-prompt-submit-hook",
    "agent-message",
    "cross-session-message",
    "artifact-content-authored-by-others",
];
const INTERRUPT_PREFIX: &str = "[Request interrupted by user";

#[derive(Deserialize, Default)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    subtype: Option<String>,
    level: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    #[serde(rename = "isCompactSummary")]
    is_compact_summary: Option<bool>,
    #[serde(rename = "isApiErrorMessage")]
    is_api_error: Option<bool>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "permissionMode")]
    permission_mode: Option<String>,
    error: Option<Value>,
    /// The text of a `system` entry.
    content: Option<Value>,
    origin: Option<Value>,
    message: Option<Box<RawValue>>,
    attachment: Option<Box<RawValue>>,
}

#[derive(Deserialize, Default)]
struct Attachment {
    #[serde(rename = "type")]
    kind: Option<String>,
    prompt: Option<String>,
    origin: Option<Value>,
}

#[derive(Deserialize, Default)]
struct Message {
    /// The id of the API message. Claude writes one entry per block, all with this id.
    id: Option<String>,
    model: Option<String>,
    usage: Option<Box<RawValue>>,
    stop_reason: Option<String>,
    content: Option<Content>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Deserialize, Default)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
    tool_use_id: Option<String>,
    content: Option<Value>,
    is_error: Option<bool>,
}

impl Entry {
    fn time(&self) -> Option<UnixMs> {
        self.timestamp.as_deref().and_then(parse_rfc3339_ms)
    }

    fn message(&self) -> Option<Message> {
        serde_json::from_str(self.message.as_ref()?.get()).ok()
    }

    fn human(&self) -> bool {
        origin_is_human(self.origin.as_ref())
    }

    fn api_error(&self) -> bool {
        self.is_api_error == Some(true)
    }
}

/// `origin.kind` is `human` for text the user typed. Other kinds are machine messages.
fn origin_is_human(origin: Option<&Value>) -> bool {
    origin
        .and_then(|o| o.get("kind"))
        .and_then(Value::as_str)
        .is_none_or(|k| k == "human")
}

impl Message {
    fn blocks(&self) -> &[Block] {
        match &self.content {
            Some(Content::Blocks(b)) => b,
            _ => &[],
        }
    }

    /// The text of the message: a string, or the text blocks joined by line breaks.
    fn text(&self) -> String {
        match &self.content {
            Some(Content::Text(t)) => t.clone(),
            Some(Content::Blocks(blocks)) => join_text(blocks),
            None => String::new(),
        }
    }
}

fn join_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks.iter().filter(|b| b.kind.as_deref() == Some("text")) {
        if let Some(t) = &b.text {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(t);
        }
    }
    out
}

/// The text of a `tool_result` block: a string, or the text parts of a list.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ---- prompt text ----------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Prompt {
    /// Only machine text. Nothing to show.
    Skip,
    Interrupt,
    Text(String),
}

fn remove_tag_blocks(text: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        // The tag name ends at `>` or at a space (attributes). Another name is not this tag.
        if !after.starts_with(['>', ' ']) {
            out.push_str(&rest[..start + open.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..start]);
        rest = match after.find(&close) {
            Some(end) => &after[end + close.len()..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn classify_prompt(raw: &str) -> Prompt {
    if raw.trim_start().starts_with(INTERRUPT_PREFIX) {
        return Prompt::Interrupt;
    }
    if raw.contains("<command-name>") {
        let name = between(raw, "<command-name>", "</command-name>").unwrap_or_default();
        let args = between(raw, "<command-args>", "</command-args>").unwrap_or_default();
        let text = format!("{} {}", name.trim(), args.trim());
        let text = text.trim();
        return if text.is_empty() {
            Prompt::Skip
        } else {
            Prompt::Text(text.to_string())
        };
    }
    if let Some(cmd) = between(raw, "<bash-input>", "</bash-input>") {
        return Prompt::Text(format!("! {}", cmd.trim()));
    }
    let mut text = raw.to_string();
    if text.contains('<') {
        for tag in DROP_TAGS {
            text = remove_tag_blocks(&text, tag);
        }
    }
    let text = text.trim();
    if text.is_empty() {
        Prompt::Skip
    } else {
        Prompt::Text(text.to_string())
    }
}

// ---- normalize ------------------------------------------------------------------------------

pub(super) fn normalize<R: BufRead>(reader: R) -> io::Result<Vec<Line>> {
    let mut n = Normalizer::default();
    for_each_line(reader, |raw| {
        if let Ok(entry) = serde_json::from_slice::<Entry>(raw) {
            n.entry(&entry);
        }
    })?;
    Ok(n.out)
}

#[derive(Default)]
struct Normalizer {
    out: Vec<Line>,
    turns: u32,
    started: bool,
    model: Option<String>,
    /// Tool name by `tool_use` id, to read the result.
    tools: HashMap<String, String>,
}

impl Normalizer {
    fn entry(&mut self, e: &Entry) {
        match e.kind.as_deref() {
            Some("user") => self.user(e),
            Some("assistant") => self.assistant(e),
            Some("system") => self.system(e),
            Some("attachment") => self.attachment(e),
            _ => {}
        }
    }

    /// The first conversation entry opens the session.
    fn begin(&mut self, e: &Entry) {
        if self.started {
            return;
        }
        self.started = true;
        let mut text = String::from("session start");
        for part in [e.cwd.as_deref(), e.git_branch.as_deref()] {
            if let Some(p) = part.filter(|p| !p.is_empty()) {
                text.push_str(" · ");
                text.push_str(p);
            }
        }
        self.out.push(Line::meta(text));
    }

    fn user(&mut self, e: &Entry) {
        self.begin(e);
        if e.is_meta == Some(true) || e.is_compact_summary == Some(true) {
            return;
        }
        let Some(msg) = e.message() else { return };
        for b in msg.blocks() {
            if b.kind.as_deref() == Some("tool_result") {
                self.result(b);
            }
        }
        if e.human() {
            self.prompt(&msg.text(), e.time());
        }
    }

    fn prompt(&mut self, raw: &str, at: Option<UnixMs>) {
        match classify_prompt(raw) {
            Prompt::Skip => {}
            Prompt::Interrupt => self.out.push(Line::meta("interrupted")),
            Prompt::Text(text) => {
                self.turns += 1;
                self.out.push(turn_line(self.turns, at));
                push_prompt(&mut self.out, &text);
            }
        }
    }

    fn result(&mut self, b: &Block) {
        let is_error = b.is_error == Some(true);
        let name = b.tool_use_id.as_ref().and_then(|id| self.tools.get(id));
        if !is_error && name.is_some_and(|n| SILENT_RESULT_TOOLS.contains(&n.as_str())) {
            return;
        }
        push_output(&mut self.out, &result_text(b.content.as_ref()), is_error);
    }

    fn assistant(&mut self, e: &Entry) {
        self.begin(e);
        let Some(msg) = e.message() else { return };
        if e.api_error() || (msg.model.as_deref() == Some(SYNTHETIC_MODEL) && e.error.is_some()) {
            let mut text = msg.text();
            if text.trim().is_empty() {
                text = error_text(e);
            }
            push_output(&mut self.out, &text, true);
            return;
        }
        if msg.model.as_deref() == Some(SYNTHETIC_MODEL) {
            return;
        }
        if let Some(model) = msg.model.as_deref()
            && self.model.as_deref() != Some(model)
        {
            self.out.push(Line::meta(format!("model · {model}")));
            self.model = Some(model.to_string());
        }
        match &msg.content {
            Some(Content::Text(t)) => push_text(&mut self.out, LineKind::Agent, t),
            Some(Content::Blocks(blocks)) => {
                for b in blocks {
                    match b.kind.as_deref() {
                        Some("text") => {
                            push_text(
                                &mut self.out,
                                LineKind::Agent,
                                b.text.as_deref().unwrap_or(""),
                            );
                        }
                        Some("tool_use") => self.tool_use(b),
                        _ => {}
                    }
                }
            }
            None => {}
        }
    }

    fn tool_use(&mut self, b: &Block) {
        let name = b.name.as_deref().unwrap_or("tool");
        if let Some(id) = &b.id {
            // The same call can appear twice when a file is resumed.
            if self.tools.insert(id.clone(), name.to_string()).is_some() {
                return;
            }
        }
        let input = b.input.as_ref().unwrap_or(&Value::Null);
        if SUBAGENT_TOOLS.contains(&name) {
            let kind = str_field(input, "subagent_type").unwrap_or("general-purpose");
            let task = str_field(input, "description")
                .or_else(|| str_field(input, "prompt"))
                .map(super::common::one_line)
                .unwrap_or_default();
            self.out.push(Line::subagent(kind, task));
            return;
        }
        self.out.push(Line::tool(name, main_arg(input)));
        let diff = match name {
            "Edit" => replace_lines(
                str_field(input, "old_string").unwrap_or_default(),
                str_field(input, "new_string").unwrap_or_default(),
            ),
            "MultiEdit" => input
                .get("edits")
                .and_then(Value::as_array)
                .map(|edits| {
                    edits
                        .iter()
                        .flat_map(|ed| {
                            replace_lines(
                                str_field(ed, "old_string").unwrap_or_default(),
                                str_field(ed, "new_string").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            "Write" => added_lines(str_field(input, "content").unwrap_or_default()),
            "NotebookEdit" => added_lines(str_field(input, "new_source").unwrap_or_default()),
            _ => Vec::new(),
        };
        push_diff(&mut self.out, diff);
    }

    fn system(&mut self, e: &Entry) {
        if e.subtype.as_deref() == Some("compact_boundary") {
            self.out.push(Line::meta("context compacted"));
        } else if is_system_error(e) {
            push_output(&mut self.out, &error_text(e), true);
        }
    }

    fn attachment(&mut self, e: &Entry) {
        let Some(a) = parse_attachment(e) else { return };
        if a.kind.as_deref() == Some("queued_command")
            && origin_is_human(a.origin.as_ref())
            && let Some(prompt) = a.prompt
        {
            self.begin(e);
            self.prompt(&prompt, e.time());
        }
    }
}

fn parse_attachment(e: &Entry) -> Option<Attachment> {
    serde_json::from_str(e.attachment.as_ref()?.get()).ok()
}

fn is_system_error(e: &Entry) -> bool {
    e.subtype.as_deref() == Some("api_error") || e.level.as_deref() == Some("error")
}

/// The text of an error entry: the text of `content`, else the name in `error`.
fn error_text(e: &Entry) -> String {
    for v in [e.content.as_ref(), e.error.as_ref()].into_iter().flatten() {
        if let Some(s) = v.as_str().filter(|s| !s.trim().is_empty()) {
            return s.to_string();
        }
    }
    "API error".to_string()
}

// ---- state ----------------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq)]
enum Last {
    #[default]
    None,
    Prompt,
    Interrupted,
    ToolResult,
    /// An assistant entry with a thinking block only: the model still writes.
    Streaming,
    /// An assistant entry with a tool call.
    Tool,
    /// An assistant entry with text and no tool call.
    Text {
        stop_reason: Option<String>,
    },
    ApiError(String),
}

struct Pending {
    id: String,
    name: String,
    input: Value,
}

#[derive(Default)]
struct StateScan {
    last: Last,
    last_time: Option<UnixMs>,
    pending: Vec<Pending>,
    mode: Option<String>,
}

impl StateScan {
    fn feed(&mut self, e: &Entry) {
        // Claude writes the mode on its own entry and on every user entry.
        if e.permission_mode.is_some() {
            self.mode.clone_from(&e.permission_mode);
        }
        match e.kind.as_deref() {
            Some("permission-mode") => {}
            Some("user") => self.user(e),
            Some("assistant") => self.assistant(e),
            Some("system") if is_system_error(e) => {
                self.set(Last::ApiError(error_text(e)), e.time());
            }
            Some("attachment") => {
                if let Some(a) = parse_attachment(e)
                    && a.kind.as_deref() == Some("queued_command")
                    && origin_is_human(a.origin.as_ref())
                {
                    self.pending.clear();
                    self.set(Last::Prompt, e.time());
                }
            }
            _ => {}
        }
    }

    fn set(&mut self, last: Last, at: Option<UnixMs>) {
        self.last = last;
        if at.is_some() {
            self.last_time = at;
        }
    }

    fn user(&mut self, e: &Entry) {
        if e.is_meta == Some(true) || e.is_compact_summary == Some(true) {
            return;
        }
        let Some(msg) = e.message() else { return };
        let mut answered = false;
        for b in msg.blocks() {
            if b.kind.as_deref() == Some("tool_result") {
                if let Some(id) = &b.tool_use_id {
                    self.pending.retain(|p| &p.id != id);
                }
                answered = true;
            }
        }
        if answered {
            self.set(Last::ToolResult, e.time());
        }
        let text = msg.text();
        if text.trim().is_empty() {
            return;
        }
        // A machine message from a finished task or another session wakes the agent too.
        match classify_prompt(&text) {
            Prompt::Interrupt => {
                self.pending.clear();
                self.set(Last::Interrupted, e.time());
            }
            Prompt::Text(_) => {
                self.pending.clear();
                self.set(Last::Prompt, e.time());
            }
            Prompt::Skip if !e.human() => {
                self.pending.clear();
                self.set(Last::Prompt, e.time());
            }
            Prompt::Skip => {}
        }
    }

    fn assistant(&mut self, e: &Entry) {
        let Some(msg) = e.message() else { return };
        if e.api_error() || (msg.model.as_deref() == Some(SYNTHETIC_MODEL) && e.error.is_some()) {
            let mut text = msg.text();
            if text.trim().is_empty() {
                text = error_text(e);
            }
            self.set(Last::ApiError(text), e.time());
            return;
        }
        if msg.model.as_deref() == Some(SYNTHETIC_MODEL) {
            return;
        }
        let mut has_tool = false;
        let mut has_text = false;
        for b in msg.blocks() {
            match b.kind.as_deref() {
                Some("tool_use") => {
                    has_tool = true;
                    if let Some(id) = &b.id
                        && !self.pending.iter().any(|p| &p.id == id)
                    {
                        self.pending.push(Pending {
                            id: id.clone(),
                            name: b.name.clone().unwrap_or_default(),
                            input: b.input.clone().unwrap_or(Value::Null),
                        });
                    }
                }
                Some("text") => has_text |= b.text.as_deref().is_some_and(|t| !t.trim().is_empty()),
                _ => {}
            }
        }
        if matches!(msg.content, Some(Content::Text(ref t)) if !t.trim().is_empty()) {
            has_text = true;
        }
        let last = if has_tool {
            Last::Tool
        } else if has_text {
            Last::Text {
                stop_reason: msg.stop_reason.clone(),
            }
        } else {
            Last::Streaming
        };
        self.set(last, e.time());
    }

    /// `None` when the window holds no conversation entry.
    fn finish(self) -> Option<(Facts, bool)> {
        if self.last == Last::None {
            return None;
        }
        let mut facts = Facts {
            since: self.last_time,
            ..Facts::default()
        };
        if let Last::ApiError(text) = &self.last {
            let id = format!("error-{}", self.last_time.unwrap_or_default());
            facts.error = Some(error_ask(&id, text));
            return Some((facts, false));
        }
        facts.ask = self.ask();
        facts.ended = match &self.last {
            Last::Text { stop_reason } => stop_reason.as_deref() != Some("tool_use"),
            Last::Interrupted => true,
            _ => false,
        };
        let waits_on_subagent = self
            .pending
            .iter()
            .any(|p| SUBAGENT_TOOLS.contains(&p.name.as_str()));
        Some((facts, waits_on_subagent))
    }

    /// The ask of the first tool call that waits for the user. A question or a plan comes
    /// first. A sub-agent call is not an ask: the agent works.
    fn ask(&self) -> Option<Ask> {
        for p in &self.pending {
            match p.name.as_str() {
                "AskUserQuestion" => return Some(question_ask(&p.id, &p.input)),
                "ExitPlanMode" => {
                    return Some(plan_ask(&p.id, str_field(&p.input, "plan")));
                }
                _ => {}
            }
        }
        self.pending
            .iter()
            .find(|p| {
                !SUBAGENT_TOOLS.contains(&p.name.as_str())
                    && !auto_approved(self.mode.as_deref(), &p.name)
            })
            .map(|p| permission_ask(&p.id, &p.name, &p.input))
    }
}

/// A tool that the permission mode runs without a question.
fn auto_approved(mode: Option<&str>, tool: &str) -> bool {
    match mode {
        // `auto` lets a classifier decide without a dialog; `dontAsk` denies without one.
        Some("bypassPermissions" | "auto" | "dontAsk") => true,
        Some("acceptEdits") => matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit"),
        _ => false,
    }
}

pub(super) fn facts(path: &Path) -> Result<Option<Facts>, ReadError> {
    let scanned = with_tail(path, |lines| {
        let mut scan = StateScan::default();
        for raw in lines {
            if let Ok(entry) = serde_json::from_slice::<Entry>(raw) {
                scan.feed(&entry);
            }
        }
        scan.finish()
    })?;
    Ok(scanned.map(|(mut facts, waits_on_subagent)| {
        if waits_on_subagent {
            // The session file is quiet while a sub-agent works. The sub-agent files change.
            facts.activity = newest_subagent_mtime(path);
        }
        facts
    }))
}

// ---- sub-agents -----------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct Meta {
    #[serde(rename = "agentType")]
    agent_type: Option<String>,
    description: Option<String>,
    #[serde(rename = "toolUseId")]
    tool_use_id: Option<String>,
    finished: Option<bool>,
}

fn subagent_dir(session: &Path) -> PathBuf {
    session.with_extension("").join("subagents")
}

pub(super) fn mtime_ms(path: &Path) -> Option<UnixMs> {
    let t = fs::metadata(path).ok()?.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_millis()).ok()
}

fn newest_subagent_mtime(session: &Path) -> Option<UnixMs> {
    fs::read_dir(subagent_dir(session))
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl"))
        .filter_map(|e| mtime_ms(&e.path()))
        .max()
}

fn read_first_entry(path: &Path) -> Option<Entry> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str(line.trim()).ok()
}

/// The id of a sub-agent file: `agent-<id>.jsonl` gives `<id>`.
fn agent_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    Some(
        name.strip_prefix("agent-")?
            .strip_suffix(".jsonl")?
            .to_string(),
    )
}

/// Sub-agent files of a session: the `subagents/` folder, or else the old `agent-*.jsonl`
/// files next to the session file whose first entry names this session.
fn subagent_files(session: &Path) -> Vec<PathBuf> {
    let list = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| agent_id(p).is_some())
                    .collect()
            })
            .unwrap_or_default()
    };
    let dir = subagent_dir(session);
    if dir.is_dir() {
        return list(&dir);
    }
    let Some(id) = session.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let Some(parent) = session.parent() else {
        return Vec::new();
    };
    // The old files name the session in `sessionId` of the first line.
    list(parent)
        .into_iter()
        .filter(|p| first_line_session_id(p).as_deref() == Some(id))
        .collect()
}

fn first_line_session_id(path: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct First {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
    }
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str::<First>(line.trim()).ok()?.session_id
}

/// The path of the transcript of one sub-agent, or `None` when it does not exist.
pub fn subagent_path(session: &Path, subagent_id: &str) -> Option<PathBuf> {
    let valid = !subagent_id.is_empty()
        && subagent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return None;
    }
    let file = format!("agent-{subagent_id}.jsonl");
    let new_layout = subagent_dir(session).join(&file);
    if new_layout.is_file() {
        return Some(new_layout);
    }
    let old_layout = session.parent()?.join(&file);
    old_layout.is_file().then_some(old_layout)
}

/// The sub-agents of a Claude session, in the order they started. The name is the agent type,
/// the task is the short description of the call. A sub-agent that the meta file marks as
/// finished is `Done`, else its state comes from its own transcript.
pub fn subagents(session: &Path, now: UnixMs) -> Vec<Subagent> {
    let mut found: Vec<(UnixMs, Subagent)> = subagent_files(session)
        .into_iter()
        .filter_map(|path| {
            let id = agent_id(&path)?;
            let meta: Meta = fs::read(path.with_extension("meta.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let mtime = mtime_ms(&path).unwrap_or(0);
            let started = read_first_entry(&path)
                .and_then(|e| e.time())
                .unwrap_or(mtime);
            let mut state = derive_state(Provider::Claude, &path, now, mtime)
                .map_or(SessionState::Parked, |d| d.state);
            if meta.finished == Some(true) && state != SessionState::Error {
                state = SessionState::Done;
            }
            Some((
                started,
                Subagent {
                    id,
                    name: meta.agent_type.unwrap_or_else(|| "agent".to_string()),
                    state,
                    task: meta
                        .description
                        .as_deref()
                        .map(super::common::one_line)
                        .unwrap_or_default(),
                },
            ))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    found.into_iter().map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_skips_machine_text() {
        assert_eq!(
            classify_prompt("<system-reminder>x</system-reminder>\n"),
            Prompt::Skip
        );
        assert_eq!(
            classify_prompt("<local-command-caveat>c</local-command-caveat>"),
            Prompt::Skip
        );
        assert_eq!(
            classify_prompt("<ide_opened_file>f</ide_opened_file>fix it"),
            Prompt::Text("fix it".into())
        );
        assert_eq!(
            classify_prompt("[Request interrupted by user for tool use]"),
            Prompt::Interrupt
        );
        assert_eq!(
            classify_prompt(
                "<command-name>/compact</command-name><command-args>now</command-args>"
            ),
            Prompt::Text("/compact now".into())
        );
        assert_eq!(
            classify_prompt("<bash-input>ls</bash-input>"),
            Prompt::Text("! ls".into())
        );
        // A tag with a longer name is not the dropped tag.
        assert_eq!(
            classify_prompt("<system-reminders>keep</system-reminders>"),
            Prompt::Text("<system-reminders>keep</system-reminders>".into())
        );
    }

    #[test]
    fn modes_approve_tools() {
        assert!(auto_approved(Some("bypassPermissions"), "Bash"));
        assert!(auto_approved(Some("auto"), "Bash"));
        assert!(auto_approved(Some("dontAsk"), "Bash"));
        assert!(!auto_approved(Some("default"), "Bash"));
        assert!(auto_approved(Some("acceptEdits"), "Edit"));
        assert!(!auto_approved(Some("acceptEdits"), "Bash"));
        assert!(!auto_approved(None, "Edit"));
    }
}
