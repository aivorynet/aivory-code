use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use aivory_protocol::AskKind;

use super::*;

fn server() -> (McpServer, Receiver<Ask>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let s = McpServer::start(move |ask| lock(&tx).send(ask).is_ok()).unwrap();
    (s, rx)
}

/// Send one request and read the whole response (the server closes the connection).
fn http(port: u16, token: Option<&str>, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(t) = token {
        req.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let status = resp.split(' ').nth(1).unwrap().parse().unwrap();
    let body = resp
        .split_once("\r\n\r\n")
        .map(|x| x.1)
        .unwrap_or("")
        .to_string();
    (status, body)
}

/// The JSON of the first `data:` line of an event stream, or of a plain JSON body.
fn json_of(body: &str) -> Value {
    let text = body
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap_or(body);
    serde_json::from_str(text).unwrap()
}

fn rpc(method: &str, id: u64, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn call_permission(tool: &str, input: &Value) -> String {
    rpc(
        "tools/call",
        7,
        json!({"name": "permission", "arguments": {"tool_name": tool, "input": input, "tool_use_id": "toolu_1"}}),
    )
}

/// The `behavior` JSON inside the tool result.
fn behavior(body: &str) -> Value {
    let v = json_of(body);
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

#[test]
fn bad_or_missing_token_gets_401() {
    let (s, _rx) = server();
    let body = rpc("initialize", 1, json!({"protocolVersion": "2025-11-25"}));
    assert_eq!(http(s.port(), Some("wrong"), "POST", "/mcp", &body).0, 401);
    assert_eq!(http(s.port(), None, "POST", "/mcp", &body).0, 401);
    assert_eq!(
        http(
            s.port(),
            Some(&format!("{}x", s.token())),
            "POST",
            "/mcp",
            &body
        )
        .0,
        401
    );
    assert_eq!(
        http(s.port(), Some(s.token()), "POST", "/mcp", &body).0,
        200
    );
}

#[test]
fn token_is_random_per_server() {
    let (a, _ra) = server();
    let (b, _rb) = server();
    assert_eq!(a.token().len(), 64);
    assert_ne!(a.token(), b.token());
    assert_ne!(a.port(), b.port());
}

#[test]
fn initialize_and_list_tools() {
    let (s, _rx) = server();
    let t = Some(s.token());
    let init = rpc("initialize", 1, json!({"protocolVersion": "2025-11-25"}));
    let (status, body) = http(s.port(), t, "POST", "/mcp", &init);
    assert_eq!(status, 200);
    let v = json_of(&body);
    assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(v["result"]["serverInfo"]["name"], "aivory");

    let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
    assert_eq!(http(s.port(), t, "POST", "/mcp", &note).0, 202);

    let (_, body) = http(
        s.port(),
        t,
        "POST",
        "/mcp",
        &rpc("tools/list", 2, json!({})),
    );
    let v = json_of(&body);
    assert_eq!(v["result"]["tools"][0]["name"], "permission");
}

#[test]
fn unknown_method_and_wrong_path_and_get() {
    let (s, _rx) = server();
    let t = Some(s.token());
    let (status, body) = http(
        s.port(),
        t,
        "POST",
        "/mcp",
        &rpc("server/discover", 1, json!({})),
    );
    assert_eq!(status, 200);
    assert_eq!(json_of(&body)["error"]["code"], -32601);
    assert_eq!(http(s.port(), t, "POST", "/other", "{}").0, 404);
    assert_eq!(http(s.port(), t, "GET", "/mcp", "").0, 405);
    assert_eq!(http(s.port(), t, "POST", "/mcp", "{not json").0, 400);
}

#[test]
fn permission_allow_returns_the_original_input() {
    let (s, rx) = server();
    let port = s.port();
    let token = s.token().to_string();
    let input = json!({"command": "echo hi"});
    let body = call_permission("Bash", &input);
    let client = thread::spawn(move || http(port, Some(&token), "POST", "/mcp", &body));
    let ask = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(ask.kind, AskKind::Permission);
    assert_eq!(ask.text, "echo hi");
    assert_eq!(ask.options, ["Allow", "Always allow", "Deny"]);
    assert_eq!(s.pending(), 1);
    assert!(s.answer(&ask.id, Answer::allow()));
    // A second answer to the same ask finds nothing.
    assert!(!s.answer(&ask.id, Answer::allow()));
    let (status, body) = client.join().unwrap();
    assert_eq!(status, 200);
    let b = behavior(&body);
    assert_eq!(b["behavior"], "allow");
    assert_eq!(b["updatedInput"], input);
    assert!(b.get("updatedPermissions").is_none());
    assert_eq!(s.pending(), 0);
}

#[test]
fn permission_deny_carries_the_message() {
    let (s, rx) = server();
    let port = s.port();
    let token = s.token().to_string();
    let body = call_permission("Edit", &json!({"file_path": "src/a.rs"}));
    let client = thread::spawn(move || http(port, Some(&token), "POST", "/mcp", &body));
    let ask = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(ask.text, "src/a.rs");
    assert!(s.answer(&ask.id, Answer::deny("Use the other file.")));
    let b = behavior(&client.join().unwrap().1);
    assert_eq!(b["behavior"], "deny");
    assert_eq!(b["message"], "Use the other file.");
}

#[test]
fn always_allows_the_same_call_later_without_an_ask() {
    let (s, rx) = server();
    let port = s.port();
    let token = s.token().to_string();
    let first = call_permission("Bash", &json!({"command": "cargo test"}));
    let t = token.clone();
    let client = thread::spawn(move || http(port, Some(&t), "POST", "/mcp", &first));
    let ask = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    s.answer(&ask.id, Answer::allow_always());
    assert_eq!(behavior(&client.join().unwrap().1)["behavior"], "allow");

    // The same command: allowed at once, no new ask.
    let again = call_permission("Bash", &json!({"command": "cargo test"}));
    let (_, body) = http(port, Some(&token), "POST", "/mcp", &again);
    assert_eq!(behavior(&body)["behavior"], "allow");
    assert!(rx.try_recv().is_err());

    // Another command still asks.
    let other = call_permission("Bash", &json!({"command": "rm -rf x"}));
    let t = token.clone();
    let client = thread::spawn(move || http(port, Some(&t), "POST", "/mcp", &other));
    let ask = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(ask.text, "rm -rf x");
    s.answer(&ask.id, Answer::deny("No."));
    assert_eq!(behavior(&client.join().unwrap().1)["behavior"], "deny");
}

#[test]
fn plan_becomes_a_plan_ask_with_steps() {
    let (s, rx) = server();
    let port = s.port();
    let token = s.token().to_string();
    let plan = "# Cache the grid\n1. Measure rows\n2. Draw visible rows";
    let body = call_permission("ExitPlanMode", &json!({"plan": plan}));
    let client = thread::spawn(move || http(port, Some(&token), "POST", "/mcp", &body));
    let ask = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(ask.kind, AskKind::Plan);
    assert_eq!(ask.text, "Cache the grid");
    assert_eq!(ask.steps, ["Measure rows", "Draw visible rows"]);
    s.answer(&ask.id, Answer::deny("Add a test step."));
    let b = behavior(&client.join().unwrap().1);
    assert_eq!(b["behavior"], "deny");
    assert_eq!(b["message"], "Add a test step.");
}

#[test]
fn questions_are_asked_one_by_one() {
    let (s, rx) = server();
    let port = s.port();
    let token = s.token().to_string();
    let input = json!({"questions": [
        {"question": "Which color?", "header": "Color", "multiSelect": false,
         "options": [{"label": "Red", "description": ""}, {"label": "Blue", "description": ""}]},
        {"question": "Which size?", "header": "Size", "multiSelect": false,
         "options": [{"label": "S", "description": ""}, {"label": "L", "description": ""}]},
    ]});
    let body = call_permission("AskUserQuestion", &input);
    let client = thread::spawn(move || http(port, Some(&token), "POST", "/mcp", &body));
    let first = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(first.kind, AskKind::Question);
    assert_eq!(first.text, "Which color?");
    assert_eq!(first.options, ["Red", "Blue"]);
    s.answer(&first.id, Answer::choice("Blue"));
    let second = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(second.text, "Which size?");
    assert_ne!(first.id, second.id);
    // No message: the first option wins.
    s.answer(&second.id, Answer::allow());
    let b = behavior(&client.join().unwrap().1);
    assert_eq!(b["behavior"], "allow");
    assert_eq!(b["updatedInput"]["answers"]["Which color?"], "Blue");
    assert_eq!(b["updatedInput"]["answers"]["Which size?"], "S");
}

#[test]
fn cancel_all_denies_open_calls() {
    let (s, rx) = server();
    let port = s.port();
    let token = s.token().to_string();
    let body = call_permission("Bash", &json!({"command": "sleep 99"}));
    let client = thread::spawn(move || http(port, Some(&token), "POST", "/mcp", &body));
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    s.cancel_all("Interrupted.");
    let b = behavior(&client.join().unwrap().1);
    assert_eq!(b["behavior"], "deny");
    assert_eq!(b["message"], "Interrupted.");
    assert_eq!(s.pending(), 0);
}

#[test]
fn drop_stops_the_listener() {
    let (s, _rx) = server();
    let port = s.port();
    drop(s);
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn config_names_the_token_variable_not_the_token() {
    let (s, _rx) = server();
    let cfg: Value = serde_json::from_str(&s.config_json("AIVORY_MCP_TOKEN")).unwrap();
    let server = &cfg["mcpServers"]["aivory"];
    assert_eq!(server["type"], "http");
    assert_eq!(server["url"], s.url());
    assert_eq!(
        server["headers"]["Authorization"],
        "Bearer ${AIVORY_MCP_TOKEN}"
    );
    assert!(!cfg.to_string().contains(s.token()));
}

/// A provider for the tests: it answers `references` with a fixed text and records the call.
struct FakeTools {
    seen: Mutex<Vec<(String, Value, Option<std::path::PathBuf>)>>,
}

impl AgentTools for FakeTools {
    fn call(
        &self,
        tool: &str,
        args: &Value,
        cwd: Option<&std::path::Path>,
    ) -> Result<String, String> {
        lock(&self.seen).push((tool.to_string(), args.clone(), cwd.map(Into::into)));
        match tool {
            "references" => Ok("references  ->  1 result\n  src/a.rs:3   inv.total()\n".into()),
            _ => Err("not here".into()),
        }
    }
}

fn tool_call(name: &str, args: &Value) -> String {
    rpc("tools/call", 9, json!({"name": name, "arguments": args}))
}

#[test]
fn code_tools_are_listed_only_with_a_provider() {
    let (s, _rx) = server();
    let t = Some(s.token());
    let listed = |s: &McpServer| -> Vec<String> {
        let (_, body) = http(
            s.port(),
            Some(s.token()),
            "POST",
            "/mcp",
            &rpc("tools/list", 2, json!({})),
        );
        json_of(&body)["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(listed(&s), ["permission"]);
    // A call of a code tool without a provider is an unknown tool.
    let (_, body) = http(
        s.port(),
        t,
        "POST",
        "/mcp",
        &tool_call("references", &json!({"symbol": "x"})),
    );
    assert_eq!(json_of(&body)["error"]["code"], -32602);

    s.set_tools(Arc::new(FakeTools {
        seen: Mutex::new(Vec::new()),
    }));
    assert_eq!(
        listed(&s),
        [
            "permission",
            "references",
            "definition",
            "symbols",
            "outline",
            "diagnostics"
        ]
    );
}

#[test]
fn the_references_tool_returns_the_text_of_the_provider() {
    let (s, rx) = server();
    let fake = Arc::new(FakeTools {
        seen: Mutex::new(Vec::new()),
    });
    s.set_tools(fake.clone());
    s.set_cwd("/work/repo");
    let args = json!({"symbol": "Invoice::total", "page": 2});
    // Both the bare name and the name that Claude Code uses work.
    for name in ["references", "mcp__aivory__references"] {
        let (status, body) = http(
            s.port(),
            Some(s.token()),
            "POST",
            "/mcp",
            &tool_call(name, &args),
        );
        assert_eq!(status, 200);
        let v = json_of(&body);
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("references  ->  1 result"), "{text}");
    }
    let seen = lock(&fake.seen);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, "references");
    assert_eq!(seen[0].1, args);
    assert_eq!(
        seen[0].2.as_deref(),
        Some(std::path::Path::new("/work/repo"))
    );
    // The tool did not ask the user.
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_failed_code_tool_is_an_error_result() {
    let (s, _rx) = server();
    s.set_tools(Arc::new(FakeTools {
        seen: Mutex::new(Vec::new()),
    }));
    let (_, body) = http(
        s.port(),
        Some(s.token()),
        "POST",
        "/mcp",
        &tool_call("outline", &json!({"path": "a.rs"})),
    );
    let v = json_of(&body);
    assert_eq!(v["result"]["isError"], true);
    assert_eq!(v["result"]["content"][0]["text"], "not here");
}

#[test]
fn the_permission_tool_lets_code_tools_pass_without_an_ask() {
    let (s, rx) = server();
    s.set_tools(Arc::new(FakeTools {
        seen: Mutex::new(Vec::new()),
    }));
    let input = json!({"symbol": "x"});
    let body = call_permission("mcp__aivory__references", &input);
    let (_, resp) = http(s.port(), Some(s.token()), "POST", "/mcp", &body);
    let b = behavior(&resp);
    assert_eq!(b["behavior"], "allow");
    assert_eq!(b["updatedInput"], input);
    assert!(rx.try_recv().is_err());
    // Another tool of the same server still asks.
    let body = call_permission("mcp__aivory__other", &input);
    let port = s.port();
    let token = s.token().to_string();
    let client = thread::spawn(move || http(port, Some(&token), "POST", "/mcp", &body));
    let ask = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    s.answer(&ask.id, Answer::deny("No."));
    assert_eq!(behavior(&client.join().unwrap().1)["behavior"], "deny");
}
