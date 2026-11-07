//! A fake `codex` and a fake `gemini` for the tests. One program, two protocols.
//!
//! - `app-server` in the arguments: the app-server of Codex 0.124.0 (JSON-RPC lines, no
//!   `jsonrpc` field). The shapes follow `codex app-server generate-json-schema`.
//! - `--acp` in the arguments: the Agent Client Protocol as Gemini 0.49.0 speaks it.
//!
//! The text of a user turn picks the behavior:
//!
//! - `say <text>`: answer with `<text>`.
//! - `run <cmd>`: ask for permission to run `<cmd>`; then report what happened.
//! - `edit <path>`: ask for permission to change `<path>`; then report what happened.
//! - `question`: ask a question (Codex only).
//! - `config`: answer with the approval policy, the sandbox, and the mode that the client set.
//! - `slow`: print `working` and wait for an interrupt.
//! - `fail`: end the turn with an error.
//! - `exit <code>`: write `boom` to stderr and exit with that code.
//!
//! `--model needauth` makes `session/new` demand `authenticate` first (Gemini).
//! `--model hang` never answers the request that opens the session (Codex: the model of
//! `thread/start`).

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::thread;

use serde_json::{Value, json};

fn out(v: &Value, header: bool) {
    let mut v = v.clone();
    if header {
        v["jsonrpc"] = json!("2.0");
    }
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

/// What the fake waits for.
#[derive(Debug)]
enum Wait {
    Nothing,
    /// A `slow` turn waits for an interrupt.
    Slow,
    /// The answer to a server request with this id, for the command or path.
    Ask {
        id: i64,
        what: String,
    },
    Question {
        id: i64,
    },
}

struct Fake {
    codex: bool,
    needs_auth: bool,
    /// `--model hang` (Gemini): never answer `session/new`.
    hang: bool,
    authenticated: bool,
    wait: Wait,
    /// The id of the client request `turn/start` or `session/prompt` that waits for its end.
    turn_request: Option<Value>,
    turn: u64,
    next_server_id: i64,
    policy: String,
    sandbox: String,
    mode: String,
    session: String,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("fake-agent 0.0.0");
        return;
    }
    let codex = args.iter().any(|a| a == "app-server");
    let needs_auth = args
        .windows(2)
        .any(|w| w[0] == "--model" && w[1] == "needauth");
    let mut fake = Fake {
        codex,
        needs_auth,
        hang: args.windows(2).any(|w| w[0] == "--model" && w[1] == "hang"),
        authenticated: false,
        wait: Wait::Nothing,
        turn_request: None,
        turn: 0,
        next_server_id: 1000,
        policy: String::new(),
        sandbox: String::new(),
        mode: args
            .windows(2)
            .find(|w| w[0] == "--approval-mode")
            .map(|w| w[1].clone())
            .unwrap_or_default(),
        session: String::new(),
    };
    let (tx, rx) = mpsc::channel::<Value>();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(v);
            }
        }
    });
    while let Ok(v) = rx.recv() {
        fake.on_message(&v);
    }
}

impl Fake {
    fn send(&self, v: &Value) {
        out(v, !self.codex);
    }

    fn respond(&self, id: &Value, result: Value) {
        self.send(&json!({ "id": id, "result": result }));
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(&json!({ "method": method, "params": params }));
    }

    fn on_message(&mut self, v: &Value) {
        let method = v.get("method").and_then(Value::as_str);
        let id = v.get("id").cloned();
        match (method, id) {
            (Some(m), Some(id)) => self.on_request(m, &id, v.get("params").unwrap_or(&Value::Null)),
            (Some(m), None) => self.on_notification(m),
            (None, Some(id)) => self.on_response(&id, v),
            (None, None) => {}
        }
    }

    fn on_request(&mut self, m: &str, id: &Value, p: &Value) {
        match m {
            "initialize" if self.codex => self.respond(
                id,
                json!({"userAgent": "fake", "codexHome": "x", "platformFamily": "x", "platformOs": "x"}),
            ),
            "initialize" => self.respond(
                id,
                json!({"protocolVersion": 1,
                    "agentCapabilities": {"loadSession": true},
                    "authMethods": [{"id": "oauth-personal", "name": "Login"}]}),
            ),
            "thread/start" if p["model"] == "hang" => {}
            "session/new" if self.hang => {}
            "thread/start" | "thread/resume" | "thread/fork" => {
                self.policy = p["approvalPolicy"].as_str().unwrap_or("").into();
                self.sandbox = p["sandbox"].as_str().unwrap_or("").into();
                self.session = match m {
                    "thread/start" => "fake-thread".into(),
                    "thread/resume" => p["threadId"].as_str().unwrap_or("").into(),
                    _ => format!("fork-of-{}", p["threadId"].as_str().unwrap_or("")),
                };
                self.respond(id, json!({"thread": {"id": self.session}}));
            }
            "authenticate" => {
                self.authenticated = true;
                self.respond(id, json!({}));
            }
            "session/new" if self.needs_auth && !self.authenticated => self.send(
                &json!({"id": id, "error": {"code": -32000, "message": "Authentication required"}}),
            ),
            "session/new" => {
                self.session = "fake-gemini".into();
                if self.mode.is_empty() {
                    self.mode = "default".into();
                }
                self.respond(
                    id,
                    json!({"sessionId": self.session,
                        "modes": {"currentModeId": "default", "availableModes": [
                            {"id": "default"}, {"id": "autoEdit"}, {"id": "plan"}, {"id": "yolo"}]}}),
                );
            }
            "session/load" => {
                self.session = p["sessionId"].as_str().unwrap_or("").into();
                let upd = |u: Value| {
                    self.notify("session/update", json!({"sessionId": self.session, "update": u}));
                };
                upd(json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "old prompt"}}));
                upd(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "old answer"}}));
                self.respond(id, json!({}));
            }
            "session/set_mode" => {
                self.mode = match p["modeId"].as_str().unwrap_or("") {
                    "autoEdit" => "auto_edit".into(),
                    other => other.into(),
                };
                self.respond(id, json!({}));
            }
            "turn/start" => {
                self.turn += 1;
                self.respond(id, json!({"turn": {"id": self.turn_id(), "status": "inProgress", "items": []}}));
                self.notify(
                    "turn/started",
                    json!({"threadId": self.session, "turn": {"id": self.turn_id(), "status": "inProgress", "items": []}}),
                );
                let text = p["input"][0]["text"].as_str().unwrap_or("").to_string();
                self.run_turn(&text);
            }
            "session/prompt" => {
                self.turn += 1;
                self.turn_request = Some(id.clone());
                let text = p["prompt"][0]["text"].as_str().unwrap_or("").to_string();
                self.run_turn(&text);
            }
            "turn/interrupt" => {
                self.respond(id, json!({}));
                self.interrupted();
            }
            _ => self.send(&json!({"id": id, "error": {"code": -32601, "message": "unknown"}})),
        }
    }

    fn on_notification(&mut self, m: &str) {
        if m == "session/cancel" {
            self.interrupted();
        }
    }

    fn turn_id(&self) -> String {
        format!("turn-{}", self.turn)
    }

    fn interrupted(&mut self) {
        if matches!(self.wait, Wait::Slow) {
            self.wait = Wait::Nothing;
            self.end_turn("interrupted");
        }
    }

    /// Text the agent says.
    fn say(&self, text: &str) {
        if self.codex {
            let item =
                json!({"type": "agentMessage", "id": format!("m{}", self.turn), "text": text});
            self.notify(
                "item/started",
                json!({"threadId": self.session, "turnId": self.turn_id(), "item": item}),
            );
            self.notify(
                "item/completed",
                json!({"threadId": self.session, "turnId": self.turn_id(), "item": item}),
            );
        } else {
            // Two chunks: the client joins them.
            let (a, b) = text.split_at(text.len() / 2);
            for part in [a, b] {
                self.notify(
                    "session/update",
                    json!({"sessionId": self.session, "update": {
                        "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": part}}}),
                );
            }
        }
    }

    /// Finish the turn. `how` is `completed`, `interrupted`, or `failed`.
    fn end_turn(&mut self, how: &str) {
        self.wait = Wait::Nothing;
        if self.codex {
            self.notify(
                "thread/tokenUsage/updated",
                json!({"threadId": self.session, "turnId": self.turn_id(), "tokenUsage": {
                    "last": {"inputTokens": 100, "cachedInputTokens": 0, "outputTokens": 5, "reasoningOutputTokens": 0, "totalTokens": 105},
                    "total": {"inputTokens": 100, "cachedInputTokens": 0, "outputTokens": 5, "reasoningOutputTokens": 0, "totalTokens": 105},
                    "modelContextWindow": 1000}}),
            );
            let error = (how == "failed").then(|| json!({"message": "The model broke."}));
            self.notify(
                "turn/completed",
                json!({"threadId": self.session, "turn": {"id": self.turn_id(), "status": how, "items": [], "error": error}}),
            );
        } else if let Some(id) = self.turn_request.take() {
            self.notify(
                "session/update",
                json!({"sessionId": self.session, "update": {"sessionUpdate": "usage_update", "used": 100, "size": 1000}}),
            );
            if how == "failed" {
                self.send(
                    &json!({"id": id, "error": {"code": -32603, "message": "The model broke."}}),
                );
            } else {
                let reason = if how == "interrupted" {
                    "cancelled"
                } else {
                    "end_turn"
                };
                self.respond(&id, json!({"stopReason": reason}));
            }
        }
    }

    fn run_turn(&mut self, text: &str) {
        let (verb, rest) = text.split_once(' ').unwrap_or((text, ""));
        match verb {
            "say" => {
                self.say(rest);
                self.end_turn("completed");
            }
            "config" => {
                let t = format!(
                    "policy={} sandbox={} mode={}",
                    self.policy, self.sandbox, self.mode
                );
                self.say(&t);
                self.end_turn("completed");
            }
            "slow" => {
                // The blank line ends a paragraph, so that the client shows the text at once.
                self.say(
                    "working

",
                );
                self.wait = Wait::Slow;
            }
            "fail" => {
                if self.codex {
                    self.notify(
                        "error",
                        json!({"threadId": self.session, "turnId": self.turn_id(), "willRetry": false,
                            "error": {"message": "The model broke."}}),
                    );
                }
                self.end_turn("failed");
            }
            "exit" => {
                eprintln!("boom");
                std::process::exit(rest.trim().parse().unwrap_or(1));
            }
            "run" | "edit" => self.ask_permission(verb, rest),
            "question" if self.codex => {
                let id = self.server_id();
                self.send(&json!({"id": id, "method": "item/tool/requestUserInput", "params": {
                    "threadId": self.session, "turnId": self.turn_id(), "itemId": "q1",
                    "questions": [
                        {"id": "color", "header": "Color", "question": "Which color?", "isOther": false, "isSecret": false,
                         "options": [{"label": "Red", "description": "r"}, {"label": "Blue", "description": "b"}]},
                        {"id": "size", "header": "Size", "question": "Which size?", "isOther": true, "isSecret": false,
                         "options": null}]}}));
                self.wait = Wait::Question { id };
            }
            _ => {
                self.say("unknown command");
                self.end_turn("completed");
            }
        }
    }

    fn server_id(&mut self) -> i64 {
        self.next_server_id += 1;
        self.next_server_id
    }

    fn ask_permission(&mut self, verb: &str, what: &str) {
        let id = self.server_id();
        if self.codex {
            if verb == "run" {
                self.send(
                    &json!({"id": id, "method": "item/commandExecution/requestApproval", "params": {
                    "threadId": self.session, "turnId": self.turn_id(), "itemId": "c1",
                    "command": what, "reason": "The agent needs it."}}),
                );
            } else {
                let item = json!({"type": "fileChange", "id": "f1", "status": "inProgress", "changes": [
                    {"path": what, "kind": {"type": "update"}, "diff": "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new"}]});
                self.notify(
                    "item/started",
                    json!({"threadId": self.session, "turnId": self.turn_id(), "item": item}),
                );
                self.send(
                    &json!({"id": id, "method": "item/fileChange/requestApproval", "params": {
                    "threadId": self.session, "turnId": self.turn_id(), "itemId": "f1"}}),
                );
            }
        } else {
            let kind = if verb == "run" { "execute" } else { "edit" };
            self.notify(
                "session/update",
                json!({"sessionId": self.session, "update": {
                    "sessionUpdate": "tool_call", "toolCallId": "t1", "title": what, "kind": kind, "status": "pending",
                    "content": if verb == "edit" {
                        json!([{"type": "diff", "path": what, "oldText": "old", "newText": "new"}])
                    } else {
                        json!([])
                    }}}),
            );
            self.send(
                &json!({"id": id, "method": "session/request_permission", "params": {
                "sessionId": self.session,
                "toolCall": {"toolCallId": "t1", "title": what, "kind": kind},
                "options": [
                    {"optionId": "o1", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "o2", "name": "Allow always", "kind": "allow_always"},
                    {"optionId": "o3", "name": "Reject", "kind": "reject_once"}]}}),
            );
        }
        self.wait = Wait::Ask {
            id,
            what: format!("{verb} {what}"),
        };
    }

    fn on_response(&mut self, id: &Value, v: &Value) {
        let id = id.as_i64().unwrap_or(-1);
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        match std::mem::replace(&mut self.wait, Wait::Nothing) {
            Wait::Ask { id: want, what } if want == id => self.after_ask(&what, &result),
            Wait::Question { id: want } if want == id => {
                let t = format!("answers: {}", result["answers"]);
                self.say(&t);
                self.end_turn("completed");
            }
            other => self.wait = other,
        }
    }

    fn after_ask(&mut self, what: &str, result: &Value) {
        let (verb, rest) = what.split_once(' ').unwrap_or((what, ""));
        // Codex: `{decision}`. ACP: `{outcome: {outcome, optionId}}`.
        let (allowed, cancelled, always) = if self.codex {
            let d = result["decision"].as_str().unwrap_or("");
            (
                d == "accept" || d == "acceptForSession",
                d == "cancel",
                d == "acceptForSession",
            )
        } else {
            let o = &result["outcome"];
            let picked = o["optionId"].as_str().unwrap_or("");
            (
                picked == "o1" || picked == "o2",
                o["outcome"] == "cancelled",
                picked == "o2",
            )
        };
        if cancelled {
            self.end_turn("interrupted");
            return;
        }
        let note = if always { " (always)" } else { "" };
        if self.codex {
            let item = if verb == "run" {
                json!({"type": "commandExecution", "id": "c1", "command": rest, "commandActions": [],
                    "status": if allowed { "completed" } else { "declined" },
                    "aggregatedOutput": if allowed { format!("ran {rest}") } else { String::new() },
                    "exitCode": if allowed { json!(0) } else { Value::Null }})
            } else {
                json!({"type": "fileChange", "id": "f1", "changes": [],
                    "status": if allowed { "completed" } else { "declined" }})
            };
            if verb == "run" {
                let mut started = item.clone();
                started["status"] = json!("inProgress");
                self.notify(
                    "item/started",
                    json!({"threadId": self.session, "turnId": self.turn_id(), "item": started}),
                );
            }
            self.notify(
                "item/completed",
                json!({"threadId": self.session, "turnId": self.turn_id(), "item": item}),
            );
        } else {
            let (status, text) = if allowed {
                ("completed", format!("ran {rest}"))
            } else {
                ("failed", "denied".to_string())
            };
            self.notify(
                "session/update",
                json!({"sessionId": self.session, "update": {
                    "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": status,
                    "content": [{"type": "content", "content": {"type": "text", "text": text}}]}}),
            );
        }
        self.say(&if allowed {
            format!("ok{note}")
        } else {
            "denied".to_string()
        });
        self.end_turn("completed");
    }
}
