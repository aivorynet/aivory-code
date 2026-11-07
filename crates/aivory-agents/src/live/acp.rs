//! The Gemini dialect: `gemini --acp` (CLI 0.49.0), the Agent Client Protocol on stdio.
//!
//! ACP is JSON-RPC 2.0 with one JSON object per line. The client sends `initialize`, then
//! `session/new` (or `session/load` to resume), then `session/prompt` for each user turn. The
//! agent streams `session/update` notifications and asks for approvals with the request
//! `session/request_permission`, which the client answers with
//! `{outcome: {outcome: "selected", optionId}}` or `{outcome: {outcome: "cancelled"}}`.
//! `session/cancel` (a notification) stops the turn that runs.
//!
//! Gemini may demand `authenticate` before `session/new` (error code -32000). The client then
//! names a method id and the CLI uses its own stored login. The client never sees a token.

use std::collections::HashMap;

use aivory_protocol::{Ask, AskKind, Line};
use serde_json::{Map, Value, json};

use super::StartOptions;
use super::rpc::{AskSlot, Dialect, Io, Tag};
use super::stream::{MAX_LINE_CHARS, diff_lines, summarize};
use crate::mcp::{Answer, truncate};

const INIT: Tag = "initialize";
const AUTH: Tag = "authenticate";
const NEW: Tag = "session/new";
const LOAD: Tag = "session/load";
const MODE: Tag = "session/set_mode";
const PROMPT: Tag = "session/prompt";

const AUTH_REQUIRED: i64 = -32000;
/// The window of a Gemini model when the CLI does not tell it.
const DEFAULT_WINDOW: u64 = 1_048_576;
const MAX_REMEMBERED: usize = 256;

pub(super) struct Acp {
    opts: StartOptions,
    session: Option<String>,
    load_supported: bool,
    auth_methods: Vec<String>,
    authenticated: bool,
    /// `session/load` replays the old transcript as updates. Drop them.
    replaying: bool,
    /// The streamed text of the agent message that is not complete yet.
    message: String,
    /// Tool call id -> (kind, diff shown).
    tools: HashMap<String, (String, bool)>,
    used: u64,
    window: u64,
}

impl Acp {
    pub fn new(opts: &StartOptions) -> Self {
        Self {
            opts: opts.clone(),
            session: None,
            load_supported: false,
            auth_methods: Vec::new(),
            authenticated: false,
            replaying: false,
            message: String::new(),
            tools: HashMap::new(),
            used: 0,
            window: 0,
        }
    }

    fn cwd(&self) -> String {
        self.opts.cwd.display().to_string()
    }

    fn open_session(&mut self, io: &Io<'_>) {
        let cwd = self.cwd();
        let params = |extra: Option<&str>| {
            let mut p = json!({ "cwd": cwd, "mcpServers": [] });
            if let Some(id) = extra {
                p["sessionId"] = json!(id);
            }
            p
        };
        match self.opts.resume.clone() {
            Some(id) if self.load_supported => {
                self.replaying = true;
                io.request(LOAD, "session/load", params(Some(&id)));
            }
            Some(_) => io.fail("this Gemini CLI cannot load a session over ACP".into()),
            None => {
                io.request(NEW, "session/new", params(None));
            }
        }
    }

    /// The auth method to name when Gemini asks for `authenticate`.
    fn auth_method(&self) -> Option<String> {
        let has = |id: &str| self.auth_methods.iter().any(|m| m == id);
        let env = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        let want = if env("GEMINI_API_KEY") {
            "gemini-api-key"
        } else if env("GOOGLE_GENAI_USE_VERTEXAI") {
            "vertex-ai"
        } else {
            "oauth-personal"
        };
        if has(want) {
            Some(want.to_string())
        } else {
            self.auth_methods.first().cloned()
        }
    }

    /// Choose the mode that fits the approval mode of the composer.
    fn set_mode(&mut self, result: &Value, io: &Io<'_>) {
        let Some(wanted) = self.opts.permission_mode.as_deref() else {
            return;
        };
        let candidates: &[&str] = match wanted {
            "plan" => &["plan"],
            "auto_edit" => &["autoEdit", "auto_edit"],
            "yolo" => &["yolo"],
            _ => &["default"],
        };
        let available: Vec<&str> = result
            .pointer("/modes/availableModes")
            .and_then(Value::as_array)
            .map(|m| m.iter().filter_map(|x| x["id"].as_str()).collect())
            .unwrap_or_default();
        let current = result
            .pointer("/modes/currentModeId")
            .and_then(Value::as_str);
        let Some(target) = candidates.iter().find(|c| available.contains(c)) else {
            return;
        };
        if current != Some(*target)
            && let Some(session) = &self.session
        {
            io.request(
                MODE,
                "session/set_mode",
                json!({ "sessionId": session, "modeId": target }),
            );
        }
    }

    fn flush_message(&mut self, io: &Io<'_>) {
        let text = std::mem::take(&mut self.message);
        let text = text.trim();
        if !text.is_empty() {
            io.line(Line::agent(text));
        }
    }

    fn update(&mut self, u: &Value, io: &Io<'_>) {
        if self.replaying {
            return;
        }
        match u.get("sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                if let Some(t) = u.pointer("/content/text").and_then(Value::as_str) {
                    self.message.push_str(t);
                    // A finished paragraph is a line now: a long turn must not stay silent.
                    while let Some(end) = self.message.find(
                        "

",
                    ) {
                        let paragraph: String = self.message.drain(..end + 2).collect();
                        if !paragraph.trim().is_empty() {
                            io.line(Line::agent(paragraph.trim()));
                        }
                    }
                }
            }
            Some("tool_call") => {
                self.flush_message(io);
                self.tool_call(u, io);
            }
            Some("tool_call_update") => {
                self.flush_message(io);
                self.tool_update(u, io);
            }
            Some("usage_update") => {
                let n = |k: &str| u.get(k).and_then(Value::as_u64);
                if let (Some(used), Some(size)) = (n("used"), n("size")) {
                    self.used = used;
                    self.window = size;
                }
            }
            _ => {}
        }
    }

    fn tool_call(&mut self, u: &Value, io: &Io<'_>) {
        let id = u.get("toolCallId").and_then(Value::as_str).unwrap_or("");
        let kind = u.get("kind").and_then(Value::as_str).unwrap_or("other");
        let title = u.get("title").and_then(Value::as_str).unwrap_or("");
        let label = match kind {
            "read" => "Read",
            "edit" => "Edit",
            "delete" => "Delete",
            "move" => "Move",
            "search" => "Search",
            "execute" => "Bash",
            "fetch" => "WebFetch",
            "think" => "Think",
            _ => "Tool",
        };
        io.line(Line::tool(label, truncate(title, MAX_LINE_CHARS)));
        let diff = self.diff_lines(u, io);
        if self.tools.len() >= MAX_REMEMBERED {
            self.tools.clear();
        }
        self.tools.insert(id.to_string(), (kind.to_string(), diff));
    }

    fn tool_update(&mut self, u: &Value, io: &Io<'_>) {
        let id = u.get("toolCallId").and_then(Value::as_str).unwrap_or("");
        let shown = self.tools.get(id).is_some_and(|t| t.1);
        if !shown
            && self.diff_lines(u, io)
            && let Some(t) = self.tools.get_mut(id)
        {
            t.1 = true;
        }
        match u.get("status").and_then(Value::as_str) {
            Some("completed") => {
                let text = content_text(u);
                if !text.trim().is_empty() {
                    io.line(Line::output(summarize(&text)));
                }
                self.tools.remove(id);
            }
            Some("failed") => {
                let text = content_text(u);
                io.line(Line::error(if text.trim().is_empty() {
                    "The tool call failed.".to_string()
                } else {
                    summarize(&text)
                }));
                self.tools.remove(id);
            }
            _ => {}
        }
    }

    /// Show the `diff` content items of a tool call. `true` when it showed any.
    fn diff_lines(&self, u: &Value, io: &Io<'_>) -> bool {
        let mut any = false;
        for c in u
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if c.get("type").and_then(Value::as_str) != Some("diff") {
                continue;
            }
            any = true;
            let text = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
            let mut out = Vec::new();
            diff_lines(text("oldText"), text("newText"), &mut out);
            for e in out {
                io.emit(e);
            }
        }
        any
    }

    fn context_pct(&self) -> u8 {
        let window = if self.window > 0 {
            self.window
        } else {
            DEFAULT_WINDOW
        };
        u8::try_from((self.used.saturating_mul(100) / window).min(100)).unwrap_or(100)
    }

    fn permission_ask(&mut self, id: &Value, p: &Value, io: &Io<'_>) {
        let tool = p.get("toolCall").unwrap_or(&Value::Null);
        let title = tool
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("A tool");
        let kind = tool.get("kind").and_then(Value::as_str).unwrap_or("other");
        let mut ids = Map::new();
        let mut options = Vec::new();
        let plan = kind == "switch_mode";
        for o in p
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(option_id), Some(k)) = (o["optionId"].as_str(), o["kind"].as_str()) else {
                continue;
            };
            let (key, label) = match (k, plan) {
                ("allow_once", false) => ("allow", "Allow"),
                ("allow_always", false) => ("always", "Always allow"),
                ("reject_once" | "reject_always", false) => ("reject", "Deny"),
                ("allow_once", true) => ("allow", "Approve"),
                ("reject_once" | "reject_always", true) => ("reject", "Reject"),
                _ => continue,
            };
            if !ids.contains_key(key) {
                ids.insert(key.to_string(), json!(option_id));
                options.push(label.to_string());
            }
        }
        if plan && ids.contains_key("reject") {
            options.insert(1, "Edit plan".into());
        }
        if options.is_empty() {
            io.respond(id, json!({ "outcome": { "outcome": "cancelled" } }));
            return;
        }
        let ask = Ask {
            id: io.next_ask_id(),
            kind: if plan {
                AskKind::Plan
            } else {
                AskKind::Permission
            },
            text: truncate(title, 400),
            options,
            steps: Vec::new(),
            reason: None,
            risky: kind == "execute",
        };
        io.ask(ask, id.clone(), Value::Object(ids));
    }
}

/// Whether an error message of Gemini asks the client to authenticate. Other -32000 errors
/// (for example an account that the CLI no longer supports) fail the start at once: an
/// `authenticate` call could open a browser for nothing.
fn wants_login(message: &str) -> bool {
    let m = message.to_lowercase();
    ["authenticat", "log in", "login", "sign in"]
        .iter()
        .any(|k| m.contains(k))
}

/// The text blocks of the `content` of a tool call update.
fn content_text(u: &Value) -> String {
    u.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("content"))
        .filter_map(|c| c.pointer("/content/text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Dialect for Acp {
    fn label(&self) -> &'static str {
        "gemini"
    }

    fn program(&self) -> &'static str {
        "gemini"
    }

    fn args(&self) -> Vec<String> {
        let mut a = vec!["--acp".to_string()];
        if let Some(mode) = &self.opts.permission_mode {
            a.push("--approval-mode".into());
            a.push(mode.clone());
        }
        if let Some(model) = &self.opts.model {
            a.push("--model".into());
            a.push(model.clone());
        }
        a
    }

    fn jsonrpc_header(&self) -> bool {
        true
    }

    fn open(&mut self, io: &Io<'_>) {
        io.request(
            INIT,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false,
                },
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
        let code = |e: &Value| e.get("code").and_then(Value::as_i64);
        match (tag, reply) {
            (INIT, Ok(r)) => {
                self.load_supported = r
                    .pointer("/agentCapabilities/loadSession")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.auth_methods = r
                    .get("authMethods")
                    .and_then(Value::as_array)
                    .map(|m| {
                        m.iter()
                            .filter_map(|x| x["id"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                self.open_session(io);
            }
            (NEW | LOAD, Err(e))
                if code(e) == Some(AUTH_REQUIRED)
                    && !self.authenticated
                    && wants_login(&message(e)) =>
            {
                self.replaying = false;
                match self.auth_method() {
                    Some(method) => {
                        self.authenticated = true;
                        io.request(AUTH, "authenticate", json!({ "methodId": method }));
                    }
                    None => io.fail("gemini needs a login: run `gemini` once and sign in".into()),
                }
            }
            (AUTH, Ok(_)) => self.open_session(io),
            (INIT | AUTH | NEW | LOAD, Err(e)) => {
                io.fail(format!(
                    "gemini could not start the session: {}",
                    message(e)
                ));
            }
            (NEW, Ok(r)) => {
                let Some(id) = r.get("sessionId").and_then(Value::as_str) else {
                    io.fail("gemini did not return a session id".into());
                    return;
                };
                self.session = Some(id.to_string());
                io.started(id);
                self.set_mode(r, io);
                io.ready();
            }
            (LOAD, Ok(r)) => {
                self.replaying = false;
                self.session = self.opts.resume.clone();
                if let Some(id) = &self.session {
                    io.started(id);
                }
                self.set_mode(r, io);
                io.ready();
            }
            (PROMPT, Ok(r)) => {
                self.flush_message(io);
                match r.get("stopReason").and_then(Value::as_str) {
                    Some("cancelled") => io.line(Line::meta("Interrupted")),
                    Some("max_tokens") => io.line(Line::error("The turn hit the token limit.")),
                    Some("max_turn_requests") => {
                        io.line(Line::error("The turn hit the limit of model requests."));
                    }
                    Some("refusal") => io.line(Line::error("The model refused the request.")),
                    _ => {}
                }
                if let Some(t) = r
                    .pointer("/_meta/quota/token_count/input_tokens")
                    .and_then(Value::as_u64)
                    && self.window == 0
                {
                    self.used = t;
                }
                io.turn_ended(0.0, self.context_pct());
            }
            (PROMPT, Err(e)) => {
                self.flush_message(io);
                io.line(Line::error(truncate(&message(e), MAX_LINE_CHARS)));
                io.turn_ended(0.0, self.context_pct());
            }
            _ => {}
        }
    }

    fn notification(&mut self, method: &str, p: &Value, io: &Io<'_>) {
        if method == "session/update"
            && let Some(u) = p.get("update")
        {
            self.update(u, io);
        }
    }

    fn request(&mut self, id: &Value, method: &str, p: &Value, io: &Io<'_>) {
        match method {
            "session/request_permission" => self.permission_ask(id, p, io),
            _ => io.respond_error(id, -32601, "AIVory does not handle this request"),
        }
    }

    fn start_turn(&mut self, text: &str, io: &Io<'_>) {
        let Some(session) = self.session.clone() else {
            io.line(Line::error("The Gemini session is not open."));
            io.turn_ended(0.0, 0);
            return;
        };
        io.request(
            PROMPT,
            "session/prompt",
            json!({ "sessionId": session, "prompt": [{ "type": "text", "text": text }] }),
        );
    }

    fn interrupt(&mut self, io: &Io<'_>) {
        if let Some(session) = &self.session {
            io.notify("session/cancel", Some(json!({ "sessionId": session })));
        }
    }

    fn answer(&mut self, slot: AskSlot, answer: Answer, io: &Io<'_>) {
        let pick = |key: &str| slot.data.get(key).cloned();
        let chosen = match (answer.allow, answer.always) {
            (true, true) => pick("always").or_else(|| pick("allow")),
            (true, false) => pick("allow"),
            (false, _) => pick("reject"),
        };
        let outcome = match chosen {
            Some(option_id) => json!({ "outcome": "selected", "optionId": option_id }),
            None => json!({ "outcome": "cancelled" }),
        };
        io.respond(&slot.rpc_id, json!({ "outcome": outcome }));
    }

    fn cancel_result(&self, _: &AskSlot) -> Value {
        json!({ "outcome": { "outcome": "cancelled" } })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_text_joins_text_blocks() {
        let u = json!({"content": [
            {"type": "content", "content": {"type": "text", "text": "a"}},
            {"type": "diff", "path": "x", "newText": "y"},
            {"type": "content", "content": {"type": "text", "text": "b"}},
        ]});
        assert_eq!(content_text(&u), "a\nb");
    }

    #[test]
    fn only_a_login_message_asks_for_authenticate() {
        assert!(wants_login("Authentication required."));
        assert!(!wants_login(
            "This client is no longer supported for Gemini Code Assist."
        ));
    }

    #[test]
    fn the_args_carry_the_approval_mode_and_model() {
        let acp = Acp::new(&StartOptions {
            permission_mode: Some("plan".into()),
            model: Some("gemini-x".into()),
            ..StartOptions::default()
        });
        assert_eq!(
            acp.args(),
            ["--acp", "--approval-mode", "plan", "--model", "gemini-x"]
        );
    }
}
