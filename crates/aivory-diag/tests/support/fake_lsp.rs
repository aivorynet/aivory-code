//! A fake language server for the tests of the LSP pool. It speaks LSP over stdio.
//!
//! - `initialize` answers the capabilities. `shutdown` answers null. `exit` ends the process.
//! - `didOpen`, `didChange`, and `didClose` update a table of documents. Each open or change
//!   publishes diagnostics: one warning per line with `FIXME`. A close publishes an empty list.
//! - `hover` answers the text of the line at the position (so a test sees the overlay content).
//! - `definition`, `references`, `rename`, `semanticTokens/full`, and `codeAction` answer fixed
//!   shapes.
//! - After `initialized` the server sends two requests of its own (`client/registerCapability`
//!   and `workspace/configuration`) and the client must answer them.
//! - With `--log <file>` each received method is appended to that file as `method uri`.
//! - `--sleep <seconds>` sleeps and exits, for the tests of the command runner.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};

use serde_json::{Value, json};

fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((n, v)) = line.split_once(':')
            && n.eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0u8; length?];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(out: &mut impl Write, v: &Value) {
    let body = serde_json::to_vec(v).unwrap();
    write!(out, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    out.write_all(&body).unwrap();
    out.flush().unwrap();
}

fn publish(out: &mut impl Write, uri: &str, text: &str) {
    let diags: Vec<Value> = text
        .lines()
        .enumerate()
        .filter_map(|(i, l)| {
            l.find("FIXME").map(|c| {
                json!({"range": {"start": {"line": i, "character": c},
                                 "end": {"line": i, "character": c + 5}},
                       "severity": 2, "code": "fixme", "source": "fake-lsp",
                       "message": "FIXME found"})
            })
        })
        .collect();
    send(
        out,
        &json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                "params": {"uri": uri, "diagnostics": diags}}),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sleep") {
        let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
        std::thread::sleep(std::time::Duration::from_secs(secs));
        return;
    }
    let log_path = args
        .iter()
        .position(|a| a == "--log")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let log = |method: &str, uri: &str| {
        if let Some(p) = &log_path
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
        {
            let _ = writeln!(f, "{method} {uri}");
        }
    };
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut docs: HashMap<String, String> = HashMap::new();
    while let Some(msg) = read_message(&mut input) {
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            continue; // a response to our own request
        };
        let id = msg.get("id").cloned();
        let p = &msg["params"];
        let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_owned();
        log(method, &uri);
        let result = match method {
            "initialize" => json!({"capabilities": {
                "textDocumentSync": 1,
                "hoverProvider": true,
                "definitionProvider": true,
                "referencesProvider": true,
                "renameProvider": true,
                "codeActionProvider": true,
                "semanticTokensProvider": {
                    "legend": {"tokenTypes": ["function", "variable"],
                               "tokenModifiers": ["declaration", "readonly"]},
                    "full": true}
            }, "serverInfo": {"name": "fake-lsp"}}),
            "initialized" => {
                send(
                    &mut out,
                    &json!({"jsonrpc": "2.0", "id": 9001,
                    "method": "client/registerCapability", "params": {"registrations": []}}),
                );
                send(
                    &mut out,
                    &json!({"jsonrpc": "2.0", "id": 9002,
                    "method": "workspace/configuration", "params": {"items": [{"section": "x"}]}}),
                );
                continue;
            }
            "textDocument/didOpen" => {
                let doc = &p["textDocument"];
                let text = doc["text"].as_str().unwrap_or("").to_owned();
                publish(&mut out, doc["uri"].as_str().unwrap_or(""), &text);
                docs.insert(doc["uri"].as_str().unwrap_or("").to_owned(), text);
                continue;
            }
            "textDocument/didChange" => {
                let text = p["contentChanges"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                publish(&mut out, &uri, &text);
                docs.insert(uri.clone(), text);
                continue;
            }
            "textDocument/didClose" => {
                docs.remove(&uri);
                publish(&mut out, &uri, "");
                continue;
            }
            "textDocument/hover" => {
                let line = p["position"]["line"].as_u64().unwrap_or(0) as usize;
                let text = docs
                    .get(&uri)
                    .and_then(|t| t.lines().nth(line))
                    .unwrap_or("<no document>");
                json!({"contents": {"kind": "markdown", "value": format!("line: {text}")}})
            }
            "textDocument/definition" => json!({"uri": uri,
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}}),
            "textDocument/references" => json!([
                {"uri": uri, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}},
                {"uri": uri, "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 7}}}
            ]),
            "textDocument/rename" => json!({"changes": {uri.clone(): [
                {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}},
                 "newText": p["newName"]}
            ]}}),
            "textDocument/semanticTokens/full" => json!({"data": [0, 0, 3, 0, 1, 1, 2, 4, 1, 0]}),
            "textDocument/codeAction" => json!([
                {"title": "Remove FIXME", "kind": "quickfix", "isPreferred": true,
                 "edit": {"changes": {uri.clone(): [
                    {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                     "newText": ""}]}}},
                {"title": "Run command", "command": "fake.run", "arguments": []}
            ]),
            "shutdown" => Value::Null,
            "exit" => return,
            _ => {
                if let Some(id) = id {
                    send(
                        &mut out,
                        &json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": format!("unknown method {method}")}}),
                    );
                }
                continue;
            }
        };
        if let Some(id) = id {
            send(
                &mut out,
                &json!({"jsonrpc": "2.0", "id": id, "result": result}),
            );
        }
    }
}
