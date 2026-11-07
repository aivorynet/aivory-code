//! A minimal MCP server on `127.0.0.1` for one agent run.
//!
//! The server speaks the streamable HTTP transport of MCP (JSON-RPC over `POST /mcp`). It
//! serves one tool, `permission`, which Claude Code calls through
//! `--permission-prompt-tool mcp__aivory__permission`. The call stays open until the user
//! answers; the server then returns the allow or deny result that Claude expects.
//!
//! Each server has a random port and a random bearer token. A request without the token gets
//! `401`. The token is not a login token: it only protects this local port.

mod http;
mod permission;
mod tools;

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use aivory_protocol::Ask;
use serde_json::{Value, json};

pub use permission::Answer;
pub(crate) use permission::truncate;
use permission::{Request, allow_payload, deny_payload};
pub use tools::{AgentTools, CODE_TOOLS, set_default_tools};

/// The name of the server in the MCP config of Claude.
pub const SERVER_NAME: &str = "aivory";
/// The tool name to pass to `--permission-prompt-tool`.
pub const PERMISSION_TOOL: &str = "mcp__aivory__permission";

const KEEPALIVE: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_DENY: &str = "The user denied this request in the AIVory war room.";

enum Reply {
    Answer(Answer),
    Cancel(String),
}

type AskSink = Box<dyn Fn(Ask) -> bool + Send + Sync>;

struct Inner {
    token: String,
    port: u16,
    on_ask: AskSink,
    pending: Mutex<HashMap<String, Sender<Reply>>>,
    /// Rules from "always allow" answers. See `permission::rule_key`.
    always: Mutex<HashSet<String>>,
    /// The provider of the code tools of this server (else the default of the process).
    tools: Mutex<Option<Arc<dyn AgentTools>>>,
    /// The working folder of the run, for the code tools.
    cwd: Mutex<Option<PathBuf>>,
    /// The native session id of the run, for the coordination tools.
    session: Mutex<Option<String>>,
    next: AtomicU64,
    stop: AtomicBool,
}

/// The server. Dropping it stops the listener and denies all open asks.
pub struct McpServer {
    inner: Arc<Inner>,
    accept: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("port", &self.inner.port)
            .finish_non_exhaustive()
    }
}

impl McpServer {
    /// Start the server on a random local port.
    ///
    /// `on_ask` receives each ask when a tool call starts. It returns `false` when nobody can
    /// receive the ask; the call is then denied at once.
    pub fn start(on_ask: impl Fn(Ask) -> bool + Send + Sync + 'static) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            token: random_token()?,
            port,
            on_ask: Box::new(on_ask),
            pending: Mutex::new(HashMap::new()),
            always: Mutex::new(HashSet::new()),
            tools: Mutex::new(None),
            cwd: Mutex::new(None),
            session: Mutex::new(None),
            next: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        });
        let shared = Arc::clone(&inner);
        let accept = thread::Builder::new()
            .name("aivory-mcp-accept".into())
            .spawn(move || accept_loop(&listener, &shared))?;
        Ok(Self {
            inner,
            accept: Mutex::new(Some(accept)),
        })
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.inner.port)
    }

    /// Serve the code tools of `tools` on this server, not the default provider of the process.
    pub fn set_tools(&self, tools: Arc<dyn AgentTools>) {
        *lock(&self.inner.tools) = Some(tools);
    }

    /// Tell the server the working folder of its run. The code tools pass it to the provider,
    /// which picks the codebase from it.
    pub fn set_cwd(&self, cwd: impl Into<PathBuf>) {
        *lock(&self.inner.cwd) = Some(cwd.into());
    }

    /// Tell the server the native session id of its run. The provider gets it with each call
    /// ([`AgentTools::call_as`]), so that `claim` and `ping` know the sender.
    pub fn set_session(&self, session: impl Into<String>) {
        *lock(&self.inner.session) = Some(session.into());
    }

    /// The bearer token. Pass it to the agent in an environment variable, not in a file.
    pub fn token(&self) -> &str {
        &self.inner.token
    }

    /// The `--mcp-config` JSON. The token is not inside: Claude expands `${token_env}` in the
    /// header from its environment.
    pub fn config_json(&self, token_env: &str) -> String {
        json!({
            "mcpServers": {
                SERVER_NAME: {
                    "type": "http",
                    "url": self.url(),
                    "headers": { "Authorization": format!("Bearer ${{{token_env}}}") },
                }
            }
        })
        .to_string()
    }

    /// Answer an open ask. Returns `false` when the ask is not open (late or wrong id).
    pub fn answer(&self, ask_id: &str, answer: Answer) -> bool {
        let tx = lock(&self.inner.pending).remove(ask_id);
        tx.is_some_and(|tx| tx.send(Reply::Answer(answer)).is_ok())
    }

    /// Deny all open asks with this reason. The agent sees the reason as the denial message.
    pub fn cancel_all(&self, reason: &str) {
        self.inner.cancel_all(reason);
    }

    /// The number of asks that wait for an answer.
    pub fn pending(&self) -> usize {
        lock(&self.inner.pending).len()
    }

    /// Stop the listener and deny all open asks.
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.inner.cancel_all("The AIVory session ended.");
        // Wake the accept loop, which blocks in `accept`.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.inner.port)),
            Duration::from_secs(1),
        );
        if let Some(t) = lock(&self.accept).take() {
            let _ = t.join();
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn accept_loop(listener: &TcpListener, inner: &Arc<Inner>) {
    for stream in listener.incoming() {
        if inner.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let inner = Arc::clone(inner);
        let _ = thread::Builder::new()
            .name("aivory-mcp-conn".into())
            .spawn(move || handle(&inner, stream));
    }
}

impl Inner {
    fn cancel_all(&self, reason: &str) {
        let drained: Vec<_> = lock(&self.pending).drain().collect();
        for (_, tx) in drained {
            let _ = tx.send(Reply::Cancel(reason.to_string()));
        }
    }

    /// The provider of the code tools: the own one, or the default of the process.
    fn code_tools(&self) -> Option<Arc<dyn AgentTools>> {
        lock(&self.tools).clone().or_else(tools::default_tools)
    }

    fn next_id(&self) -> String {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        format!("{}-{n}", &self.token[..6])
    }

    fn authorized(&self, req: &http::Request) -> bool {
        req.header("authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| constant_time_eq(t.trim().as_bytes(), self.token.as_bytes()))
    }

    /// Only a request for the local address is valid. This stops DNS rebinding.
    fn host_ok(&self, req: &http::Request) -> bool {
        let port = self.port;
        req.header("host").is_some_and(|h| {
            h.eq_ignore_ascii_case(&format!("127.0.0.1:{port}"))
                || h.eq_ignore_ascii_case(&format!("localhost:{port}"))
        })
    }

    /// Show an ask to the user and wait for the answer. `None` means the client went away.
    fn ask(&self, ask: Ask, stream: &mut TcpStream) -> Option<Reply> {
        let (tx, rx) = mpsc::channel();
        let id = ask.id.clone();
        lock(&self.pending).insert(id.clone(), tx);
        if !(self.on_ask)(ask) {
            lock(&self.pending).remove(&id);
            return Some(Reply::Cancel("Nobody can answer the request.".into()));
        }
        loop {
            match rx.recv_timeout(KEEPALIVE) {
                Ok(reply) => return Some(reply),
                Err(RecvTimeoutError::Timeout) => {
                    if self.stop.load(Ordering::SeqCst) {
                        lock(&self.pending).remove(&id);
                        return Some(Reply::Cancel("The AIVory session ended.".into()));
                    }
                    // A comment line keeps the connection alive, and shows that the client left.
                    if http::write_sse(stream, ": keepalive\n\n").is_err() {
                        lock(&self.pending).remove(&id);
                        return None;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Some(Reply::Cancel("The request was cancelled.".into()));
                }
            }
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn handle(inner: &Inner, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let req = match http::read_request(&mut stream) {
        Ok(r) => r,
        Err(http::ReadError::TooLarge) => {
            let _ = http::write_response(&mut stream, 413, None, &[], b"");
            return;
        }
        Err(http::ReadError::Bad(why)) => {
            let _ = http::write_response(&mut stream, 400, Some("text/plain"), &[], why.as_bytes());
            return;
        }
        Err(http::ReadError::Io) => return,
    };
    if !inner.authorized(&req) {
        let _ = http::write_response(
            &mut stream,
            401,
            None,
            &[("WWW-Authenticate", "Bearer")],
            b"",
        );
        return;
    }
    if req.header("origin").is_some() || !inner.host_ok(&req) {
        let _ = http::write_response(&mut stream, 403, None, &[], b"");
        return;
    }
    let path = req.path.split('?').next().unwrap_or_default();
    if path != "/mcp" {
        let _ = http::write_response(&mut stream, 404, None, &[], b"");
        return;
    }
    if req.method != "POST" {
        let _ = http::write_response(&mut stream, 405, None, &[("Allow", "POST")], b"");
        return;
    }
    // The wait for an answer can be long. The read timeout ends here.
    let _ = stream.set_read_timeout(None);
    post(inner, &mut stream, &req.body);
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn reply_json(stream: &mut TcpStream, status: u16, body: &Value) {
    let _ = http::write_response(
        stream,
        status,
        Some("application/json"),
        &[],
        body.to_string().as_bytes(),
    );
}

fn post(inner: &Inner, stream: &mut TcpStream, body: &[u8]) {
    let msg: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            reply_json(stream, 400, &rpc_error(&Value::Null, -32700, "parse error"));
            return;
        }
    };
    if msg.is_array() {
        let err = rpc_error(&Value::Null, -32600, "batches are not supported");
        reply_json(stream, 400, &err);
        return;
    }
    let (Some(method), Some(id)) = (
        msg.get("method").and_then(Value::as_str),
        msg.get("id").filter(|v| !v.is_null()),
    ) else {
        // A notification, or a response of the client. There is nothing to send back.
        let _ = http::write_response(stream, 202, None, &[], b"");
        return;
    };
    let params = msg.get("params").unwrap_or(&Value::Null);
    let reply = match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-03-26");
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => {
            let mut list = vec![tool_definition()];
            if let Some(provider) = inner.code_tools() {
                list.extend(tools::definitions());
                list.extend(provider.extra_tools());
            }
            rpc_result(id, json!({ "tools": list }))
        }
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if name == "permission" {
                permission_call(inner, stream, id, params.get("arguments"));
                return;
            }
            match inner.code_tools() {
                Some(provider) if tools::is_code_tool(name) || tools::serves(&*provider, name) => {
                    code_call(inner, &provider, id, name, params)
                }
                _ => rpc_error(id, -32602, "unknown tool"),
            }
        }
        _ => rpc_error(id, -32601, "method not found"),
    };
    reply_json(stream, 200, &reply);
}

fn tool_definition() -> Value {
    json!({
        "name": "permission",
        "description": "Ask the user of the AIVory war room to approve a tool call. Claude Code calls this tool itself, through --permission-prompt-tool.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "tool_name": { "type": "string", "description": "The tool that needs approval." },
                "input": { "type": "object", "description": "The input of that tool." },
                "tool_use_id": { "type": "string" },
            },
            "required": ["tool_name", "input"],
        },
    })
}

/// A code tool call: the provider answers at once, with text.
fn code_call(
    inner: &Inner,
    provider: &Arc<dyn AgentTools>,
    id: &Value,
    name: &str,
    params: &Value,
) -> Value {
    let bare = name.rsplit("__").next().unwrap_or(name);
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let cwd = lock(&inner.cwd).clone();
    let session = lock(&inner.session).clone();
    let (text, is_error) = match provider.call_as(bare, &args, cwd.as_deref(), session.as_deref()) {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    rpc_result(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
    )
}

fn tool_result(payload: &Value) -> Value {
    json!({ "content": [{ "type": "text", "text": payload.to_string() }] })
}

/// The tool call stays open as a server-sent-events response until the user answers.
fn permission_call(inner: &Inner, stream: &mut TcpStream, id: &Value, args: Option<&Value>) {
    let tool = args
        .and_then(|a| a.get("tool_name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let input = args
        .and_then(|a| a.get("input"))
        .cloned()
        .unwrap_or(Value::Null);
    if tool.is_empty() {
        let payload = deny_payload("The permission request has no tool name.");
        reply_json(stream, 200, &rpc_result(id, tool_result(&payload)));
        return;
    }
    if http::write_sse_head(stream).is_err() {
        return;
    }
    let Some(payload) = decide(inner, stream, tool, &input) else {
        return;
    };
    let event = rpc_result(id, tool_result(&payload));
    let _ = http::write_sse(stream, &format!("event: message\ndata: {event}\n\n"));
}

/// Turn the user answers into the payload of the permission tool.
fn decide(inner: &Inner, stream: &mut TcpStream, tool: &str, input: &Value) -> Option<Value> {
    // The code tools only read, and the coordination tools only send information (claims and
    // pings grant nothing). They pass without an ask.
    if let Some(provider) = inner.code_tools()
        && (tools::is_prefixed_code_tool(tool) || tools::is_prefixed_extra_tool(&*provider, tool))
    {
        return Some(allow_payload(input, None));
    }
    match permission::classify(tool, input) {
        Request::Tool { text, risky } => {
            let key = permission::rule_key(tool, input);
            if lock(&inner.always).contains(&key) {
                return Some(allow_payload(input, None));
            }
            let ask = permission::permission_ask(inner.next_id(), tool, text, risky);
            Some(match answer_of(inner.ask(ask, stream)?) {
                Ok(a) => {
                    if a.always {
                        lock(&inner.always).insert(key);
                    }
                    allow_payload(input, None)
                }
                Err(deny) => deny,
            })
        }
        Request::Plan { title, steps } => {
            let ask = permission::plan_ask(inner.next_id(), title, steps);
            Some(match answer_of(inner.ask(ask, stream)?) {
                Ok(_) => allow_payload(input, None),
                Err(deny) => deny,
            })
        }
        Request::Questions(questions) => {
            let mut answers = Vec::new();
            for q in &questions {
                let ask = permission::question_ask(inner.next_id(), q);
                match answer_of(inner.ask(ask, stream)?) {
                    Ok(a) => {
                        let text = a
                            .message
                            .filter(|m| !m.is_empty())
                            .or_else(|| q.options.first().cloned())
                            .unwrap_or_default();
                        answers.push((q.text.clone(), text));
                    }
                    Err(deny) => return Some(deny),
                }
            }
            Some(allow_payload(input, Some(&answers)))
        }
    }
}

/// `Ok` for an allow, `Err` with the deny payload for a refusal or a cancel.
fn answer_of(reply: Reply) -> Result<Answer, Value> {
    match reply {
        Reply::Answer(a) if a.allow => Ok(a),
        Reply::Answer(a) => Err(deny_payload(a.message.as_deref().unwrap_or(DEFAULT_DENY))),
        Reply::Cancel(why) => Err(deny_payload(&why)),
    }
}

#[cfg(test)]
mod tests;
