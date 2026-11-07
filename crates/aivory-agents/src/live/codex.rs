//! The Codex dialect: `codex app-server` (CLI 0.124.0), JSON-RPC 2.0 lines on stdio.
//!
//! The message shapes come from `codex app-server generate-json-schema`. The client sends
//! `initialize`, the `initialized` notification, then `thread/start` (or `thread/resume`, or
//! `thread/fork`), then `turn/start` for each user turn. The server answers with
//! notifications (`item/started`, `item/completed`, `turn/completed`, `thread/tokenUsage/updated`)
//! and with requests that need an answer:
//!
//! - `item/commandExecution/requestApproval` -> `{decision}`
//! - `item/fileChange/requestApproval` -> `{decision}`
//! - `item/permissions/requestApproval` -> `{permissions, scope}`
//! - `item/tool/requestUserInput` -> `{answers: {<question id>: {answers: [text]}}}`
//!
//! The decisions are `accept`, `acceptForSession`, `decline`, and `cancel`.

use std::collections::{HashMap, HashSet};

use aivory_protocol::{Ask, AskKind, Line};
use serde_json::{Map, Value, json};

use super::rpc::{AskSlot, Dialect, Io, Tag};
use super::stream::{MAX_LINE_CHARS, diff_lines, first_line, summarize};
use super::{RunEvent, StartOptions};
use crate::mcp::{Answer, truncate};

const INIT: Tag = "initialize";
const THREAD: Tag = "thread";
const TURN: Tag = "turn";
const INTERRUPT: Tag = "interrupt";

const MAX_REMEMBERED: usize = 256;

pub(super) struct Codex {
    opts: StartOptions,
    thread: Option<String>,
    turn: Option<String>,
    used_tokens: u64,
    window: u64,
    /// Items of kind `fileChange` that already showed their lines.
    shown: HashSet<String>,
    /// Item id -> the paths of a file change, for the text of its approval ask.
    paths: HashMap<String, String>,
    /// The turn already reported an error line.
    turn_error: bool,
}

impl Codex {
    pub fn new(opts: &StartOptions) -> Self {
        Self {
            opts: opts.clone(),
            thread: None,
            turn: None,
            used_tokens: 0,
            window: 0,
            shown: HashSet::new(),
            paths: HashMap::new(),
            turn_error: false,
        }
    }

    /// The parameters that `thread/start`, `thread/resume`, and `thread/fork` share.
    fn thread_params(&self) -> Map<String, Value> {
        let mut p = Map::new();
        p.insert("cwd".into(), json!(self.opts.cwd.display().to_string()));
        if let Some(v) = &self.opts.permission_mode {
            p.insert("approvalPolicy".into(), json!(v));
        }
        if let Some(v) = &self.opts.sandbox {
            p.insert("sandbox".into(), json!(v));
        }
        if let Some(v) = &self.opts.model {
            p.insert("model".into(), json!(v));
        }
        if let Some(v) = &self.opts.instructions {
            p.insert("developerInstructions".into(), json!(v));
        }
        p
    }

    fn context_pct(&self) -> u8 {
        if self.window == 0 {
            return 0;
        }
        u8::try_from((self.used_tokens.saturating_mul(100) / self.window).min(100)).unwrap_or(100)
    }

    fn item(&mut self, item: &Value, started: bool, io: &Io<'_>) {
        let text = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("");
        let id = text("id").to_string();
        let failed = matches!(text("status"), "failed" | "declined");
        let mut out = Vec::new();
        match text("type") {
            "agentMessage" | "plan" if !started => {
                let t = text("text").trim();
                if !t.is_empty() {
                    out.push(RunEvent::Line(Line::agent(t)));
                }
            }
            "commandExecution" if started => {
                out.push(RunEvent::Line(Line::tool(
                    "Bash",
                    truncate(&first_line(text("command")), MAX_LINE_CHARS),
                )));
            }
            "commandExecution" if !started => {
                let output = text("aggregatedOutput");
                let code = item.get("exitCode").and_then(Value::as_i64);
                if failed || code.is_some_and(|c| c != 0) {
                    let why = if text("status") == "declined" {
                        "The command was declined.".to_string()
                    } else {
                        let first = summarize(output);
                        match code {
                            Some(c) if first.is_empty() => format!("exit code {c}"),
                            Some(c) => format!("exit code {c}: {first}"),
                            None => first,
                        }
                    };
                    out.push(RunEvent::Line(Line::error(why)));
                } else if !output.trim().is_empty() {
                    out.push(RunEvent::Line(Line::output(summarize(output))));
                }
            }
            "fileChange" => {
                let changes = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if !changes.is_empty() {
                    let names: Vec<&str> = changes
                        .iter()
                        .filter_map(|c| c.get("path").and_then(Value::as_str))
                        .collect();
                    if self.paths.len() >= MAX_REMEMBERED {
                        self.paths.clear();
                        self.shown.clear();
                    }
                    self.paths.insert(id.clone(), names.join(", "));
                }
                if !self.shown.contains(&id) && !changes.is_empty() {
                    self.shown.insert(id.clone());
                    for change in &changes {
                        file_change_lines(change, &mut out);
                    }
                }
                if !started && failed {
                    out.push(RunEvent::Line(Line::error(
                        if text("status") == "declined" {
                            "The change was declined."
                        } else {
                            "The change failed."
                        },
                    )));
                }
            }
            "mcpToolCall" if started => {
                let args = item
                    .get("arguments")
                    .map(Value::to_string)
                    .unwrap_or_default();
                out.push(RunEvent::Line(Line::tool(
                    format!("{}.{}", text("server"), text("tool")),
                    truncate(&args, MAX_LINE_CHARS),
                )));
            }
            "mcpToolCall" | "dynamicToolCall" if !started && failed => {
                let why = item
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("The tool call failed.");
                out.push(RunEvent::Line(Line::error(truncate(why, MAX_LINE_CHARS))));
            }
            "dynamicToolCall" if started => {
                out.push(RunEvent::Line(Line::tool(text("tool"), "")));
            }
            "webSearch" if started => {
                out.push(RunEvent::Line(Line::tool(
                    "WebSearch",
                    truncate(text("query"), MAX_LINE_CHARS),
                )));
            }
            "collabAgentToolCall" if started => {
                out.push(RunEvent::Line(Line::subagent(
                    text("tool"),
                    truncate(text("prompt"), MAX_LINE_CHARS),
                )));
            }
            "contextCompaction" if !started => {
                out.push(RunEvent::Line(Line::meta("Context compacted")));
            }
            _ => {}
        }
        for e in out {
            io.emit(e);
        }
    }

    fn turn_completed(&mut self, turn: &Value, io: &Io<'_>) {
        match turn.get("status").and_then(Value::as_str) {
            Some("interrupted") => io.line(Line::meta("Interrupted")),
            Some("failed") if !self.turn_error => {
                let why = turn
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .map_or_else(
                        || "The turn failed.".to_string(),
                        |m| truncate(m, MAX_LINE_CHARS),
                    );
                io.line(Line::error(why));
            }
            _ => {}
        }
        self.turn = None;
        self.turn_error = false;
        io.turn_ended(0.0, self.context_pct());
    }

    fn approval_ask(&mut self, id: &Value, method: &str, p: &Value, io: &Io<'_>) {
        let str_of = |k: &str| p.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
        let (text, data) = match method {
            "item/commandExecution/requestApproval" => {
                let text = str_of("command").map_or_else(
                    || {
                        p.pointer("/networkApprovalContext/host")
                            .and_then(Value::as_str)
                            .map_or_else(
                                || "A command".to_string(),
                                |h| format!("network access to {h}"),
                            )
                    },
                    str::to_string,
                );
                (text, json!({ "t": "decision" }))
            }
            "item/fileChange/requestApproval" => {
                let item = str_of("itemId").unwrap_or("");
                let text = self
                    .paths
                    .get(item)
                    .cloned()
                    .or_else(|| str_of("grantRoot").map(str::to_string))
                    .unwrap_or_else(|| "A file change".to_string());
                (text, json!({ "t": "decision" }))
            }
            _ => {
                let perms = p.get("permissions").cloned().unwrap_or_else(|| json!({}));
                let text = str_of("reason").map_or_else(
                    || format!("More permissions: {}", truncate(&perms.to_string(), 200)),
                    str::to_string,
                );
                (text, json!({ "t": "permissions", "permissions": perms }))
            }
        };
        let ask = Ask {
            id: io.next_ask_id(),
            kind: AskKind::Permission,
            text: truncate(&text, 400),
            options: vec!["Allow".into(), "Always allow".into(), "Deny".into()],
            steps: Vec::new(),
            reason: str_of("reason").map(str::to_string),
            risky: method == "item/commandExecution/requestApproval",
        };
        io.ask(ask, id.clone(), data);
    }

    fn question_ask(&mut self, id: &Value, p: &Value, io: &Io<'_>) {
        let questions: Vec<Value> = p
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|q| {
                let options: Vec<Value> = q
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|o| o.iter().filter_map(|x| x.get("label").cloned()).collect())
                    .unwrap_or_default();
                json!({
                    "id": q.get("id").cloned().unwrap_or(Value::Null),
                    "header": q.get("header").cloned().unwrap_or(Value::Null),
                    "question": q.get("question").cloned().unwrap_or(Value::Null),
                    "options": options,
                })
            })
            .collect();
        if questions.is_empty() {
            io.respond(id, json!({ "answers": {} }));
            return;
        }
        let data = json!({ "t": "input", "questions": questions, "index": 0, "answers": {} });
        io.ask(self.question(io, &data), id.clone(), data);
    }

    /// The ask for the question at `data.index`.
    fn question(&self, io: &Io<'_>, data: &Value) -> Ask {
        let i = data["index"].as_u64().unwrap_or(0) as usize;
        let q = &data["questions"][i];
        let options = q["options"]
            .as_array()
            .map(|o| {
                o.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Ask {
            id: io.next_ask_id(),
            kind: AskKind::Question,
            text: truncate(q["question"].as_str().unwrap_or(""), 400),
            options,
            steps: Vec::new(),
            reason: q["header"].as_str().map(str::to_string),
            risky: false,
        }
    }
}

/// Lines for one change of a `fileChange` item: the tool line and a short diff.
fn file_change_lines(change: &Value, out: &mut Vec<RunEvent>) {
    let path = change.get("path").and_then(Value::as_str).unwrap_or("");
    let kind = change.pointer("/kind/type").and_then(Value::as_str);
    let label = match kind {
        Some("add") => "Write",
        Some("delete") => "Delete",
        _ => "Edit",
    };
    out.push(RunEvent::Line(Line::tool(label, path)));
    let diff = change.get("diff").and_then(Value::as_str).unwrap_or("");
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for l in diff.lines() {
        if kind == Some("add") {
            new.push(l);
        } else if kind == Some("delete") {
            old.push(l);
        } else if l.starts_with("+++") || l.starts_with("---") || l.starts_with("@@") {
        } else if let Some(rest) = l.strip_prefix('+') {
            new.push(rest);
        } else if let Some(rest) = l.strip_prefix('-') {
            old.push(rest);
        }
    }
    diff_lines(&old.join("\n"), &new.join("\n"), out);
}

impl Dialect for Codex {
    fn label(&self) -> &'static str {
        "codex"
    }

    fn program(&self) -> &'static str {
        "codex"
    }

    fn args(&self) -> Vec<String> {
        vec!["app-server".into()]
    }

    fn jsonrpc_header(&self) -> bool {
        false
    }

    fn open(&mut self, io: &Io<'_>) {
        io.request(
            INIT,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "aivory",
                    "title": "AIVory Code",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "experimentalApi": false },
            }),
        );
    }

    fn response(&mut self, tag: Tag, reply: Result<&Value, &Value>, io: &Io<'_>) {
        let message = |e: &Value| {
            e.get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string()
        };
        match (tag, reply) {
            (INIT, Ok(_)) => {
                io.notify("initialized", None);
                let mut p = self.thread_params();
                match (&self.opts.resume, self.opts.fork) {
                    (Some(id), true) => {
                        p.insert("threadId".into(), json!(id));
                        io.request(THREAD, "thread/fork", Value::Object(p));
                    }
                    (Some(id), false) => {
                        p.insert("threadId".into(), json!(id));
                        io.request(THREAD, "thread/resume", Value::Object(p));
                    }
                    (None, _) => {
                        io.request(THREAD, "thread/start", Value::Object(p));
                    }
                }
            }
            (INIT | THREAD, Err(e)) => {
                io.fail(format!("codex could not start the session: {}", message(e)));
            }
            (THREAD, Ok(r)) => {
                let Some(id) = r.pointer("/thread/id").and_then(Value::as_str) else {
                    io.fail("codex did not return a thread id".into());
                    return;
                };
                self.thread = Some(id.to_string());
                io.started(id);
                io.ready();
            }
            (TURN, Ok(r)) => {
                if let Some(id) = r.pointer("/turn/id").and_then(Value::as_str) {
                    self.turn = Some(id.to_string());
                }
            }
            (TURN, Err(e)) => {
                io.line(Line::error(truncate(&message(e), MAX_LINE_CHARS)));
                self.turn = None;
                io.turn_ended(0.0, self.context_pct());
            }
            (INTERRUPT, Err(e)) => {
                io.line(Line::error(truncate(&message(e), MAX_LINE_CHARS)));
            }
            _ => {}
        }
    }

    fn notification(&mut self, method: &str, p: &Value, io: &Io<'_>) {
        match method {
            "item/started" | "item/completed" => {
                if let Some(item) = p.get("item") {
                    self.item(item, method == "item/started", io);
                }
            }
            "turn/started" => {
                if let Some(id) = p.pointer("/turn/id").and_then(Value::as_str) {
                    self.turn = Some(id.to_string());
                }
            }
            "turn/completed" => {
                if let Some(turn) = p.get("turn") {
                    self.turn_completed(turn, io);
                }
            }
            "thread/tokenUsage/updated" => {
                let n = |k: &str| p.pointer(k).and_then(Value::as_u64);
                if let Some(used) = n("/tokenUsage/last/inputTokens") {
                    self.used_tokens = used;
                }
                if let Some(window) = n("/tokenUsage/modelContextWindow") {
                    self.window = window;
                }
            }
            "error" => {
                let will_retry = p.get("willRetry").and_then(Value::as_bool).unwrap_or(false);
                if !will_retry {
                    let why = p
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("The turn failed.");
                    io.line(Line::error(truncate(why, MAX_LINE_CHARS)));
                    self.turn_error = true;
                }
            }
            "serverRequest/resolved" => {
                if let Some(id) = p.get("requestId") {
                    io.forget_request(id);
                }
            }
            _ => {}
        }
    }

    fn request(&mut self, id: &Value, method: &str, p: &Value, io: &Io<'_>) {
        match method {
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval" => self.approval_ask(id, method, p, io),
            "item/tool/requestUserInput" => self.question_ask(id, p, io),
            "mcpServer/elicitation/request" => {
                io.line(Line::meta(
                    "An MCP server asked for input. AIVory declined.",
                ));
                io.respond(id, json!({ "action": "decline" }));
            }
            // Never hand a login token to a client request, and never run client tools.
            _ => io.respond_error(id, -32601, "AIVory does not handle this request"),
        }
    }

    fn start_turn(&mut self, text: &str, io: &Io<'_>) {
        let Some(thread) = self.thread.clone() else {
            io.line(Line::error("The Codex thread is not open."));
            io.turn_ended(0.0, 0);
            return;
        };
        self.turn_error = false;
        io.request(
            TURN,
            "turn/start",
            json!({
                "threadId": thread,
                "input": [{ "type": "text", "text": text, "text_elements": [] }],
            }),
        );
    }

    fn interrupt(&mut self, io: &Io<'_>) {
        if let (Some(thread), Some(turn)) = (&self.thread, &self.turn) {
            io.request(
                INTERRUPT,
                "turn/interrupt",
                json!({ "threadId": thread, "turnId": turn }),
            );
        }
    }

    fn answer(&mut self, slot: AskSlot, answer: Answer, io: &Io<'_>) {
        match slot.data["t"].as_str() {
            Some("decision") => {
                let decision = match (answer.allow, answer.always) {
                    (true, true) => "acceptForSession",
                    (true, false) => "accept",
                    (false, _) => "decline",
                };
                io.respond(&slot.rpc_id, json!({ "decision": decision }));
            }
            Some("permissions") => {
                let result = if answer.allow {
                    json!({
                        "permissions": slot.data["permissions"],
                        "scope": if answer.always { "session" } else { "turn" },
                    })
                } else {
                    json!({ "permissions": {} })
                };
                io.respond(&slot.rpc_id, result);
            }
            Some("input") => {
                let mut data = slot.data.clone();
                let i = data["index"].as_u64().unwrap_or(0) as usize;
                let qid = data["questions"][i]["id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let given = if answer.allow {
                    answer.message.into_iter().collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                data["answers"][&qid] = json!({ "answers": given });
                let total = data["questions"].as_array().map_or(0, Vec::len);
                if answer.allow && i + 1 < total {
                    data["index"] = json!(i + 1);
                    let ask = self.question(io, &data);
                    io.ask(ask, slot.rpc_id, data);
                } else {
                    io.respond(&slot.rpc_id, json!({ "answers": data["answers"] }));
                }
            }
            _ => io.respond_error(&slot.rpc_id, -32603, "unknown ask"),
        }
    }

    fn cancel_result(&self, slot: &AskSlot) -> Value {
        match slot.data["t"].as_str() {
            Some("decision") => json!({ "decision": "cancel" }),
            Some("permissions") => json!({ "permissions": {} }),
            _ => json!({ "answers": {} }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_change_lines_show_a_short_diff() {
        let mut out = Vec::new();
        file_change_lines(
            &json!({"path": "a.rs", "kind": {"type": "update"},
                "diff": "--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-let a = 1;\n+let a = 2;"}),
            &mut out,
        );
        let texts: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                RunEvent::Line(l) => Some(l.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["a.rs", "let a = 1;", "let a = 2;"]);
    }
}
