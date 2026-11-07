//! One language server process: JSON-RPC over stdio.
//!
//! A reader thread reads the messages of the server. It sends the responses to the waiting
//! request, stores `publishDiagnostics`, and answers the requests of the server (`workspace/
//! configuration`, `client/registerCapability`, `window/workDoneProgress/create`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lsp_types::{Diagnostic, SemanticTokensLegend, ServerCapabilities};
use serde_json::{Value, json};

use super::uri::path_to_uri;
use crate::error::{Error, Result};

type Reply = std::result::Result<Value, String>;

#[derive(Default)]
struct DiagStore {
    /// uri -> (number of publishes, last diagnostics)
    map: HashMap<String, (u64, Vec<Diagnostic>)>,
}

struct Shared {
    pending: Mutex<HashMap<i64, Sender<Reply>>>,
    diags: Mutex<DiagStore>,
    diag_changed: Condvar,
    alive: AtomicBool,
}

pub(crate) struct Client {
    child: Child,
    writer: Arc<Mutex<ChildStdin>>,
    shared: Arc<Shared>,
    next_id: AtomicI64,
    pub pid: u32,
    pub capabilities: ServerCapabilities,
    pub legend: Option<SemanticTokensLegend>,
    request_timeout: Duration,
}

fn write_message(w: &Mutex<ChildStdin>, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)?;
    let mut w = w.lock().unwrap_or_else(|e| e.into_inner());
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

fn read_message(r: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; length.unwrap_or(0)];
    r.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn reader_loop(stdout: impl Read, shared: Arc<Shared>, writer: Arc<Mutex<ChildStdin>>) {
    let mut r = BufReader::new(stdout);
    while let Ok(Some(msg)) = read_message(&mut r) {
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id");
        match (method, id) {
            (Some(method), Some(id)) => {
                // A request of the server. Answer at once.
                let result = match method {
                    "workspace/configuration" => {
                        let n = msg["params"]["items"].as_array().map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; n])
                    }
                    "workspace/workspaceFolders" => Value::Null,
                    _ => Value::Null,
                };
                let _ = write_message(
                    &writer,
                    &json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            (Some("textDocument/publishDiagnostics"), None) => {
                let p = &msg["params"];
                if let (Some(uri), Ok(items)) = (
                    p["uri"].as_str(),
                    serde_json::from_value::<Vec<Diagnostic>>(p["diagnostics"].clone()),
                ) {
                    let mut store = shared.diags.lock().unwrap_or_else(|e| e.into_inner());
                    let entry = store.map.entry(uri.to_owned()).or_default();
                    entry.0 += 1;
                    entry.1 = items;
                    shared.diag_changed.notify_all();
                }
            }
            (Some(_), None) => {}
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { continue };
                let tx = shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                if let Some(tx) = tx {
                    let reply = match msg.get("error") {
                        Some(err) => Err(err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_owned()),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(reply);
                }
            }
            (None, None) => {}
        }
    }
    shared.alive.store(false, Ordering::SeqCst);
    // Wake every waiting request: the senders drop and `recv` fails.
    shared
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    shared.diag_changed.notify_all();
}

/// How to start a server.
pub(crate) struct Launch<'a> {
    pub exe: &'a Path,
    pub args: &'a [String],
    pub root: &'a Path,
    pub init_options: Option<&'a Value>,
    pub start_timeout: Duration,
    pub request_timeout: Duration,
}

fn client_capabilities() -> Value {
    let kinds = [
        "namespace",
        "type",
        "class",
        "enum",
        "interface",
        "struct",
        "typeParameter",
        "parameter",
        "variable",
        "property",
        "enumMember",
        "event",
        "function",
        "method",
        "macro",
        "keyword",
        "modifier",
        "comment",
        "string",
        "number",
        "regexp",
        "operator",
    ];
    let modifiers = [
        "declaration",
        "definition",
        "readonly",
        "static",
        "deprecated",
        "abstract",
        "async",
        "modification",
        "documentation",
        "defaultLibrary",
    ];
    json!({
        "general": {"positionEncodings": ["utf-16"]},
        "workspace": {"workspaceFolders": true, "configuration": true, "applyEdit": false},
        "window": {"workDoneProgress": true},
        "textDocument": {
            "synchronization": {"didSave": false, "dynamicRegistration": false},
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "definition": {"linkSupport": false},
            "references": {},
            "rename": {"prepareSupport": false},
            "publishDiagnostics": {"relatedInformation": false},
            "semanticTokens": {
                "requests": {"full": true},
                "tokenTypes": kinds,
                "tokenModifiers": modifiers,
                "formats": ["relative"]
            },
            "codeAction": {
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": [
                    "quickfix", "refactor", "refactor.extract", "refactor.inline",
                    "refactor.rewrite", "source", "source.organizeImports"
                ]}},
                "resolveSupport": {"properties": ["edit"]},
                "isPreferredSupport": true
            }
        }
    })
}

impl Client {
    /// Start the process and run the `initialize` handshake.
    pub fn start(launch: Launch<'_>) -> Result<Client> {
        let mut cmd = Command::new(launch.exe);
        cmd.args(launch.args)
            .current_dir(launch.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Server(format!("cannot start {}: {e}", launch.exe.display())))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Server("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Server("no stdout".into()))?;
        let writer = Arc::new(Mutex::new(stdin));
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            diags: Mutex::new(DiagStore::default()),
            diag_changed: Condvar::new(),
            alive: AtomicBool::new(true),
        });
        let (s2, w2) = (shared.clone(), writer.clone());
        std::thread::Builder::new()
            .name("lsp-reader".into())
            .spawn(move || reader_loop(stdout, s2, w2))?;
        let pid = child.id();
        let mut client = Client {
            child,
            writer,
            shared,
            next_id: AtomicI64::new(1),
            pid,
            capabilities: ServerCapabilities::default(),
            legend: None,
            request_timeout: launch.request_timeout,
        };
        let root_uri = path_to_uri(launch.root);
        let name = launch
            .root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("root");
        let init = client.request_with(
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": {"name": "aivory"},
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": name}],
                "capabilities": client_capabilities(),
                "initializationOptions": launch.init_options.cloned().unwrap_or(Value::Null),
            }),
            launch.start_timeout,
        )?;
        let caps = init.get("capabilities").cloned().unwrap_or(Value::Null);
        client.capabilities = serde_json::from_value(caps.clone()).unwrap_or_default();
        client.legend = caps
            .pointer("/semanticTokensProvider/legend")
            .and_then(|l| serde_json::from_value(l.clone()).ok());
        client.notify("initialized", json!({}))?;
        Ok(client)
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        write_message(
            &self.writer,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
        .map_err(|e| Error::Server(format!("cannot write to the server: {e}")))
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_with(method, params, self.request_timeout)
    }

    fn request_with(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        if let Err(e) = write_message(
            &self.writer,
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        ) {
            self.shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(Error::Server(format!("cannot write to the server: {e}")));
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(message)) => Err(Error::Request {
                method: method.to_owned(),
                message,
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                let _ = self.notify("$/cancelRequest", json!({"id": id}));
                Err(Error::Timeout(method.to_owned()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(Error::Server(format!(
                "the server exited during `{method}`"
            ))),
        }
    }

    /// The number of `publishDiagnostics` messages seen for a document.
    pub fn diag_epoch(&self, uri: &str) -> u64 {
        let store = self.shared.diags.lock().unwrap_or_else(|e| e.into_inner());
        store.map.get(uri).map_or(0, |e| e.0)
    }

    /// The last diagnostics of a document.
    pub fn diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        let store = self.shared.diags.lock().unwrap_or_else(|e| e.into_inner());
        store.map.get(uri).map(|e| e.1.clone()).unwrap_or_default()
    }

    /// Wait until every document has a publish newer than its baseline, or until `timeout`.
    /// Returns true when all documents have one.
    pub fn wait_diagnostics(&self, baselines: &[(String, u64)], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut store = self.shared.diags.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let done = baselines
                .iter()
                .all(|(uri, base)| store.map.get(uri).is_some_and(|e| e.0 > *base));
            if done {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || !self.is_alive() {
                return false;
            }
            store = self
                .shared
                .diag_changed
                .wait_timeout(store, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Stop the server: `shutdown`, `exit`, then a kill if the process stays alive.
    pub fn shutdown(mut self) {
        if self.is_alive() {
            let _ = self.request_with("shutdown", Value::Null, Duration::from_secs(2));
            let _ = self.notify("exit", Value::Null);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
