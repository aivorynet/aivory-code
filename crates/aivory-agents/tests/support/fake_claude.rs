//! A fake `claude` for the tests. It speaks the stream-json protocol of Claude Code 2.1.306
//! and calls the permission tool of the MCP server over HTTP, as the real CLI does.
//!
//! The text of a user turn picks the behavior:
//!
//! - `say <text>`: answer with `<text>`.
//! - `run <cmd>`: ask for permission to run `<cmd>`; then report what happened.
//! - `plan`: call `ExitPlanMode`. `question`: call `AskUserQuestion`.
//! - `garbage`: print malformed lines, then answer `after garbage`.
//! - `slow`: print `working` and wait for an interrupt.
//! - `exit <code>`: write `boom` to stderr and exit with that code.
//!
//! `--model badtoken` makes the MCP calls use a wrong token.

use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

static INTERRUPT: AtomicBool = AtomicBool::new(false);
static SESSION_ID: OnceLock<String> = OnceLock::new();

fn session() -> &'static str {
    SESSION_ID.get().map_or("fake-session", String::as_str)
}

fn out(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

struct Mcp {
    port: u16,
    token: String,
}

/// Send one POST and read the whole answer. Returns the status and the body.
/// With `abort_on_interrupt`, a pending interrupt drops the connection.
fn post(mcp: &Mcp, body: &Value, abort_on_interrupt: bool) -> Option<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", mcp.port)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(100))).ok()?;
    let body = body.to_string();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{body}",
        mcp.port,
        mcp.token,
        body.len()
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut all = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                all.extend_from_slice(&chunk[..n]);
                if abort_on_interrupt && INTERRUPT.load(Ordering::SeqCst) {
                    return None;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if abort_on_interrupt && INTERRUPT.load(Ordering::SeqCst) {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    let text = String::from_utf8_lossy(&all).into_owned();
    let status = text.split(' ').nth(1)?.parse().ok()?;
    let body = text.split_once("\r\n\r\n").map_or("", |x| x.1).to_string();
    Some((status, body))
}

/// The first JSON-RPC message of a body: plain JSON, or the first `data:` line of an event stream.
fn rpc_json(body: &str) -> Option<Value> {
    let text = body
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap_or(body);
    serde_json::from_str(text).ok()
}

/// The handshake that Claude does at start: discover (unknown), initialize, initialized, list.
fn handshake(mcp: &Mcp) -> Result<(), String> {
    let discover = json!({"jsonrpc": "2.0", "id": "server-discover-probe-1", "method": "server/discover", "params": {}});
    let (status, body) = post(mcp, &discover, false).ok_or("no answer to discover")?;
    if status == 401 {
        return Err("401".into());
    }
    let v = rpc_json(&body).ok_or("bad discover answer")?;
    if v["error"]["code"] != -32601 {
        return Err("discover should fail with -32601".into());
    }
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "fake-claude", "version": "0"}}});
    let (_, body) = post(mcp, &init, false).ok_or("no answer to initialize")?;
    let v = rpc_json(&body).ok_or("bad initialize answer")?;
    if v["result"]["serverInfo"]["name"] != "aivory" {
        return Err("unexpected server".into());
    }
    let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let (status, _) = post(mcp, &note, false).ok_or("no answer to initialized")?;
    if status != 202 {
        return Err(format!("initialized gave {status}"));
    }
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let (_, body) = post(mcp, &list, false).ok_or("no answer to tools/list")?;
    let v = rpc_json(&body).ok_or("bad tools/list answer")?;
    if v["result"]["tools"][0]["name"] != "permission" {
        return Err("the permission tool is missing".into());
    }
    Ok(())
}

enum Permission {
    Allow(Value),
    Deny(String),
    /// The call failed: the status, or `None` when the turn was interrupted.
    Failed(Option<u16>),
}

fn permission(mcp: &Mcp, tool: &str, input: &Value) -> Permission {
    let call = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": {"name": "permission", "arguments": {"tool_name": tool, "input": input, "tool_use_id": "toolu_fake"}}});
    let Some((status, body)) = post(mcp, &call, true) else {
        return Permission::Failed(None);
    };
    if status != 200 {
        return Permission::Failed(Some(status));
    }
    let behavior = rpc_json(&body)
        .and_then(|v| {
            v["result"]["content"][0]["text"]
                .as_str()
                .map(str::to_string)
        })
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    match behavior {
        Some(b) if b["behavior"] == "allow" => Permission::Allow(b["updatedInput"].clone()),
        Some(b) if b["behavior"] == "deny" => {
            Permission::Deny(b["message"].as_str().unwrap_or("").to_string())
        }
        _ => Permission::Failed(Some(500)),
    }
}

struct State {
    turns: u32,
    cost: f64,
}

fn assistant(content: Value) {
    out(
        &json!({"type": "assistant", "message": {"role": "assistant", "content": content,
        "usage": {"input_tokens": 100, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
        "parent_tool_use_id": null, "session_id": session()}),
    );
}

fn say(text: &str) {
    assistant(json!([{"type": "text", "text": text}]));
}

fn tool_use(name: &str, input: &Value) {
    assistant(json!([{"type": "tool_use", "id": "toolu_fake", "name": name, "input": input}]));
}

fn tool_result(text: &str, is_error: bool) {
    out(
        &json!({"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_fake", "content": text, "is_error": is_error}]},
        "parent_tool_use_id": null, "session_id": session()}),
    );
}

fn result(state: &mut State, interrupted: bool) {
    state.cost += 0.01;
    let (subtype, reason) = if interrupted {
        ("error_during_execution", "aborted_streaming")
    } else {
        ("success", "completed")
    };
    out(
        &json!({"type": "result", "subtype": subtype, "is_error": interrupted,
        "terminal_reason": reason, "session_id": session(), "total_cost_usd": state.cost,
        "modelUsage": {"fake": {"contextWindow": 1000}}, "permission_denials": []}),
    );
}

fn turn(mcp: &Mcp, state: &mut State, text: &str) {
    state.turns += 1;
    INTERRUPT.store(false, Ordering::SeqCst);
    out(
        &json!({"type": "system", "subtype": "init", "session_id": session(), "model": "fake",
        "permissionMode": "default", "cwd": "."}),
    );
    let mut interrupted = false;
    let (verb, rest) = text.split_once(' ').unwrap_or((text, ""));
    match verb {
        "say" => say(rest),
        "run" => {
            let input = json!({"command": rest});
            tool_use("Bash", &input);
            match permission(mcp, "Bash", &input) {
                Permission::Allow(_) => {
                    tool_result(&format!("ran {rest}"), false);
                    say("ok");
                }
                Permission::Deny(message) => {
                    tool_result(&message, true);
                    say(&format!("denied: {message}"));
                }
                Permission::Failed(Some(status)) => {
                    say(&format!("permission call failed: {status}"))
                }
                Permission::Failed(None) => interrupted = true,
            }
        }
        "plan" => {
            let input =
                json!({"plan": "# Cache rows\n1. Measure the rows\n2. Draw the visible rows"});
            tool_use("ExitPlanMode", &input);
            match permission(mcp, "ExitPlanMode", &input) {
                Permission::Allow(_) => say("plan approved"),
                Permission::Deny(message) => say(&format!("plan rejected: {message}")),
                Permission::Failed(Some(status)) => {
                    say(&format!("permission call failed: {status}"))
                }
                Permission::Failed(None) => interrupted = true,
            }
        }
        "question" => {
            let input = json!({"questions": [{"question": "Which color?", "header": "Color",
                "multiSelect": false, "options": [
                    {"label": "Red", "description": "r"}, {"label": "Blue", "description": "b"}]}]});
            tool_use("AskUserQuestion", &input);
            match permission(mcp, "AskUserQuestion", &input) {
                Permission::Allow(updated) => say(&format!("answers: {}", updated["answers"])),
                Permission::Deny(message) => say(&format!("question refused: {message}")),
                Permission::Failed(Some(status)) => {
                    say(&format!("permission call failed: {status}"))
                }
                Permission::Failed(None) => interrupted = true,
            }
        }
        "garbage" => {
            for line in [
                "{\"type\": \"assistant\", \"message\": {\"content\": [",
                "this is not json",
                "[1, 2, 3]",
                "{\"type\": \"mystery\", \"x\": 1}",
                "",
            ] {
                let mut o = std::io::stdout().lock();
                let _ = writeln!(o, "{line}");
                let _ = o.flush();
            }
            say("after garbage");
        }
        "slow" => {
            say("working");
            while !INTERRUPT.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(20));
            }
            interrupted = true;
        }
        "exit" => {
            eprintln!("boom");
            std::process::exit(rest.trim().parse().unwrap_or(1));
        }
        _ => say(&format!("unknown turn: {text}")),
    }
    result(state, interrupted);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let need = |flag: &str| {
        if !args.iter().any(|a| a == flag) {
            eprintln!("fake-claude: missing {flag}");
            std::process::exit(2);
        }
    };
    need("-p");
    need("--verbose");
    if arg_after(&args, "--input-format").as_deref() != Some("stream-json")
        || arg_after(&args, "--output-format").as_deref() != Some("stream-json")
        || arg_after(&args, "--permission-prompt-tool").as_deref()
            != Some("mcp__aivory__permission")
    {
        eprintln!("fake-claude: wrong arguments {args:?}");
        std::process::exit(2);
    }
    // Like Claude: `--session-id` with `--resume` needs `--fork-session`.
    let session_id = arg_after(&args, "--session-id");
    let resume = arg_after(&args, "--resume");
    if session_id.is_some() && resume.is_some() && !args.iter().any(|a| a == "--fork-session") {
        eprintln!("fake-claude: --session-id with --resume needs --fork-session");
        std::process::exit(2);
    }
    if let Some(id) = session_id.or(resume) {
        let _ = SESSION_ID.set(id);
    }
    let config_path = arg_after(&args, "--mcp-config").unwrap_or_default();
    let config: Value = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| {
            eprintln!("fake-claude: cannot read the MCP config");
            std::process::exit(2);
        });
    let server = &config["mcpServers"]["aivory"];
    let port = server["url"]
        .as_str()
        .and_then(|u| u.rsplit(':').next())
        .and_then(|p| p.split('/').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("fake-claude: bad MCP url");
            std::process::exit(2);
        });
    // Claude expands `${VAR}` in the header from its environment.
    let header = server["headers"]["Authorization"]
        .as_str()
        .unwrap_or_default();
    let env_name = header
        .split("${")
        .nth(1)
        .and_then(|r| r.split('}').next())
        .unwrap_or_default();
    let mut token = std::env::var(env_name).unwrap_or_default();
    if arg_after(&args, "--model").as_deref() == Some("badtoken") {
        token.push('x');
    }
    let mcp = Mcp { port, token };
    let handshake_ok = handshake(&mcp);
    if let Err(e) = &handshake_ok
        && e != "401"
    {
        eprintln!("fake-claude: MCP handshake failed: {e}");
        std::process::exit(3);
    }

    let (tx, rx) = mpsc::channel::<String>();
    let worker = thread::spawn(move || {
        let mut state = State {
            turns: 0,
            cost: 0.0,
        };
        while let Ok(text) = rx.recv() {
            turn(&mcp, &mut state, &text);
        }
    });
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("user") => {
                let text = v["message"]["content"].as_str().unwrap_or_default();
                let _ = tx.send(text.to_string());
            }
            Some("control_request") if v["request"]["subtype"] == "interrupt" => {
                INTERRUPT.store(true, Ordering::SeqCst);
                out(
                    &json!({"type": "control_response", "response": {"subtype": "success",
                    "request_id": v["request_id"], "response": {"still_queued": []}}}),
                );
            }
            _ => {}
        }
    }
    // Stdin closed: finish the open turn, then exit.
    drop(tx);
    let _ = worker.join();
}
