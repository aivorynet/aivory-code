//! A live run over JSON-RPC on stdio: the engine for Codex (`codex app-server`) and Gemini
//! (`gemini --acp`).
//!
//! Both CLIs keep one process and one session for many turns, and both ask the client for
//! approvals as JSON-RPC requests that the client answers. This module owns the parts that
//! the two share: the child process, the request ids, the queue of user turns, the open asks,
//! and the end of the process. A [`Dialect`] owns the parts that differ: the handshake, the
//! mapping of notifications to [`RunEvent`]s, and the shape of the answers.
//!
//! The engine runs one turn at a time. A [`RpcRun::send`] during a turn waits in a queue and
//! starts when the turn ends, as the Claude run does.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use aivory_protocol::{Ask, Line, SessionState};
use serde_json::{Value, json};

use super::{
    Input, LiveError, RunEvent, STDERR_KEEP_BYTES, StartOptions, Trace, is_shell_script, lock,
    no_window, open_trace, read_tail, send_event, write_loop,
};
use crate::command::resolve_program;
use crate::error::AgentError;
use crate::mcp::Answer;

/// Names a request, so that the dialect knows which response it reads.
pub(super) type Tag = &'static str;

/// An ask that waits for the user, and the JSON-RPC request that waits for the CLI side.
#[derive(Debug, Clone)]
pub(super) struct AskSlot {
    /// The id of the server request to answer.
    pub rpc_id: Value,
    /// What the dialect needs to build the response.
    pub data: Value,
}

/// The protocol of one CLI.
pub(super) trait Dialect: Send {
    /// The name in messages (`codex`, `gemini`).
    fn label(&self) -> &'static str;
    /// The binary name on PATH.
    fn program(&self) -> &'static str;
    /// The arguments of the child process.
    fn args(&self) -> Vec<String>;
    /// Whether messages carry `"jsonrpc": "2.0"`. Codex leaves it out.
    fn jsonrpc_header(&self) -> bool;
    /// The process started: send the first request.
    fn open(&mut self, io: &Io<'_>);
    /// The response to a request of this dialect.
    fn response(&mut self, tag: Tag, reply: Result<&Value, &Value>, io: &Io<'_>);
    fn notification(&mut self, method: &str, params: &Value, io: &Io<'_>);
    /// A request from the CLI. Answer it with `io.respond` now or later (an ask).
    fn request(&mut self, id: &Value, method: &str, params: &Value, io: &Io<'_>);
    /// Start one user turn. End it with `io.turn_ended`.
    fn start_turn(&mut self, input: &Input, io: &Io<'_>);
    /// Stop the turn that runs.
    fn interrupt(&mut self, io: &Io<'_>);
    /// Turn the answer of the user into the response (or the next ask).
    fn answer(&mut self, slot: AskSlot, answer: Answer, io: &Io<'_>);
    /// The response for an ask that nobody answers: the turn ended or was interrupted.
    fn cancel_result(&self, slot: &AskSlot) -> Value;
}

pub(super) struct Core {
    dialect: Mutex<Box<dyn Dialect>>,
    label: &'static str,
    header: bool,
    /// Lines for the stdin of the child. `None` after the process ended or was closed.
    input: Mutex<Option<Sender<String>>>,
    child: Mutex<Child>,
    /// The program is a `.cmd` shim: killing it needs `taskkill /T`.
    shim: bool,
    events: Arc<Mutex<Option<Sender<RunEvent>>>>,
    session_id: Mutex<Option<String>>,
    requested_id: Option<String>,
    /// User turns sent minus turns ended.
    open_turns: AtomicUsize,
    /// A user turn started and did not end yet.
    turn_active: AtomicBool,
    /// The handshake is done: turns may start.
    ready: AtomicBool,
    queue: Mutex<VecDeque<Input>>,
    asks: Mutex<HashMap<String, AskSlot>>,
    pending: Mutex<HashMap<u64, Tag>>,
    ended: Mutex<Option<(f64, u8)>>,
    next_id: AtomicU64,
    next_ask: AtomicU64,
    killed: AtomicBool,
    failed: AtomicBool,
    exited: Mutex<Option<SessionState>>,
}

/// What a dialect may do while it handles one message.
pub(super) struct Io<'a>(&'a Core);

impl Io<'_> {
    /// Send a request. The response arrives at [`Dialect::response`] with the same `tag`.
    pub fn request(&self, tag: Tag, method: &str, params: Value) -> u64 {
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        lock(&self.0.pending).insert(id, tag);
        self.0
            .write(json!({ "id": id, "method": method, "params": params }));
        id
    }

    pub fn notify(&self, method: &str, params: Option<Value>) {
        let mut m = json!({ "method": method });
        if let Some(p) = params {
            m["params"] = p;
        }
        self.0.write(m);
    }

    pub fn respond(&self, id: &Value, result: Value) {
        self.0.write(json!({ "id": id, "result": result }));
    }

    pub fn respond_error(&self, id: &Value, code: i64, message: &str) {
        self.0
            .write(json!({ "id": id, "error": { "code": code, "message": message } }));
    }

    pub fn line(&self, line: Line) {
        send_event(&self.0.events, RunEvent::Line(line));
    }

    pub fn emit(&self, event: RunEvent) {
        send_event(&self.0.events, event);
    }

    /// The native session id is known. Report it once.
    pub fn started(&self, id: &str) {
        let mut known = lock(&self.0.session_id);
        if known.as_deref() == Some(id) {
            return;
        }
        *known = Some(id.to_string());
        drop(known);
        send_event(
            &self.0.events,
            RunEvent::Started {
                session_id: id.to_string(),
            },
        );
    }

    /// The handshake is done. Queued user turns may start.
    pub fn ready(&self) {
        self.0.ready.store(true, Ordering::SeqCst);
    }

    /// The turn that runs ended. `cost_usd` is 0 when the CLI does not tell it.
    pub fn turn_ended(&self, cost_usd: f64, context_pct: u8) {
        *lock(&self.0.ended) = Some((cost_usd, context_pct));
    }

    /// The run cannot go on: report it and end the process.
    pub fn fail(&self, message: String) {
        self.0.failed.store(true, Ordering::SeqCst);
        send_event(&self.0.events, RunEvent::Failed(message));
        self.0.kill();
    }

    /// Open an ask. `rpc_id` is the id of the request that waits for the answer.
    pub fn ask(&self, ask: Ask, rpc_id: Value, data: Value) {
        lock(&self.0.asks).insert(ask.id.clone(), AskSlot { rpc_id, data });
        send_event(&self.0.events, RunEvent::Ask(ask));
    }

    pub fn next_ask_id(&self) -> String {
        format!(
            "{}-ask-{}",
            self.0.label,
            self.0.next_ask.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// The CLI says that a request is resolved: forget the ask that belongs to it.
    pub fn forget_request(&self, rpc_id: &Value) {
        lock(&self.0.asks).retain(|_, s| &s.rpc_id != rpc_id);
    }
}

impl Core {
    fn write(&self, mut message: Value) {
        if self.header {
            message["jsonrpc"] = json!("2.0");
        }
        if let Some(tx) = lock(&self.input).as_ref() {
            let _ = tx.send(message.to_string());
        }
    }

    fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        // Closing the input ends the writer thread and the stdin of the child.
        lock(&self.input).take();
        let mut child = lock(&self.child);
        if self.shim && cfg!(windows) {
            let mut tk = Command::new("taskkill");
            tk.args(["/PID", &child.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            no_window(&mut tk);
            if tk.status().is_ok_and(|s| s.success()) {
                return;
            }
        }
        let _ = child.kill();
    }

    /// Wait for the child to end. A child that closed stdout but lives on gets killed.
    fn reap(&self) -> Option<ExitStatus> {
        for _ in 0..300 {
            match lock(&self.child).try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {}
                Err(_) => return None,
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.kill();
        lock(&self.child).wait().ok()
    }

    /// Answer every open ask with the cancel result of the dialect.
    fn cancel_asks(&self, d: &dyn Dialect) {
        let slots: Vec<AskSlot> = lock(&self.asks).drain().map(|(_, s)| s).collect();
        for slot in slots {
            let result = d.cancel_result(&slot);
            Io(self).respond(&slot.rpc_id, result);
        }
    }

    /// After each message: close an ended turn, then start the next queued turn.
    fn settle(&self, d: &mut dyn Dialect) {
        loop {
            let ended = lock(&self.ended).take();
            if let Some((cost_usd, context_pct)) = ended {
                self.cancel_asks(d);
                self.turn_active.store(false, Ordering::SeqCst);
                let _ = self
                    .open_turns
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
                send_event(
                    &self.events,
                    RunEvent::TurnEnded {
                        cost_usd,
                        context_pct,
                        tokens: 0,
                    },
                );
                continue;
            }
            if self.ready.load(Ordering::SeqCst) && !self.turn_active.load(Ordering::SeqCst) {
                let next = lock(&self.queue).pop_front();
                if let Some(input) = next {
                    self.turn_active.store(true, Ordering::SeqCst);
                    d.start_turn(&input, &Io(self));
                    continue;
                }
            }
            break;
        }
    }

    fn handle(&self, v: &Value, d: &mut dyn Dialect) {
        let io = Io(self);
        let params = v.get("params").unwrap_or(&Value::Null);
        let method = v.get("method").and_then(Value::as_str);
        let id = v.get("id").filter(|i| !i.is_null());
        match (method, id) {
            (Some(m), Some(id)) => d.request(id, m, params, &io),
            (Some(m), None) => d.notification(m, params, &io),
            (None, Some(id)) => {
                let tag = id
                    .as_u64()
                    .and_then(|n| lock(&self.pending).remove(&n))
                    .unwrap_or("");
                match (v.get("result"), v.get("error")) {
                    (_, Some(e)) => d.response(tag, Err(e), &io),
                    (Some(r), None) => d.response(tag, Ok(r), &io),
                    (None, None) => d.response(tag, Ok(&Value::Null), &io),
                }
            }
            (None, None) => {}
        }
        self.settle(d);
    }
}

/// A handle to a running Codex or Gemini session. Dropping it kills the process.
pub struct RpcRun {
    core: Arc<Core>,
}

impl std::fmt::Debug for RpcRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcRun")
            .field("label", &self.core.label)
            .field("session_id", &self.session_id())
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl RpcRun {
    /// Start `codex app-server` and the first turn.
    pub fn start_codex(opts: StartOptions) -> Result<(Self, Receiver<RunEvent>), AgentError> {
        Self::start(Box::new(super::codex::Codex::new(&opts)), opts)
    }

    /// Start `gemini --acp` and the first turn.
    pub fn start_gemini(opts: StartOptions) -> Result<(Self, Receiver<RunEvent>), AgentError> {
        if opts.fork {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Gemini CLI cannot fork a session",
            )
            .into());
        }
        Self::start(Box::new(super::acp::Acp::new(&opts)), opts)
    }

    fn start(
        dialect: Box<dyn Dialect>,
        opts: StartOptions,
    ) -> Result<(Self, Receiver<RunEvent>), AgentError> {
        if !opts.cwd.is_dir() {
            return Err(AgentError::CwdMissing(opts.cwd));
        }
        let label = dialect.label();
        let wanted = opts
            .binary
            .clone()
            .unwrap_or_else(|| PathBuf::from(dialect.program()));
        let program = resolve_program(&wanted)?;
        let shim = is_shell_script(&program);
        let args = dialect.args();

        let mut cmd = Command::new(&program);
        cmd.args(args)
            .current_dir(&opts.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|source| AgentError::Spawn {
            program: program.display().to_string(),
            source,
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(std::io::Error::other("a pipe of the child is missing").into());
        };

        let (tx, rx) = mpsc::channel::<RunEvent>();
        let (input_tx, input_rx) = mpsc::channel::<String>();
        let trace = open_trace();
        let header = dialect.jsonrpc_header();
        let core = Arc::new(Core {
            label,
            header,
            dialect: Mutex::new(dialect),
            input: Mutex::new(Some(input_tx)),
            child: Mutex::new(child),
            shim,
            events: Arc::new(Mutex::new(Some(tx))),
            session_id: Mutex::new(None),
            requested_id: opts.resume.clone().filter(|_| !opts.fork),
            open_turns: AtomicUsize::new(0),
            turn_active: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            asks: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            ended: Mutex::new(None),
            next_id: AtomicU64::new(1),
            next_ask: AtomicU64::new(1),
            killed: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            exited: Mutex::new(None),
        });

        let writer_trace = trace.clone();
        thread::Builder::new()
            .name(format!("aivory-{label}-stdin"))
            .spawn(move || write_loop(stdin, &input_rx, writer_trace.as_ref()))?;
        let tail = Arc::new(Mutex::new(String::new()));
        let stderr_tail = Arc::clone(&tail);
        thread::Builder::new()
            .name(format!("aivory-{label}-stderr"))
            .spawn(move || read_tail(stderr, STDERR_KEEP_BYTES, &stderr_tail))?;

        // The opening request goes out before the reader thread starts, so that no response
        // can overtake the dialect state.
        lock(&core.dialect).open(&Io(&core));
        let reader = Arc::clone(&core);
        thread::Builder::new()
            .name(format!("aivory-{label}-stdout"))
            .spawn(move || read_loop(&reader, stdout, trace.as_ref(), &tail))?;

        // A CLI that never opens its session (a broken login can do this) must not leave the
        // pane at "working" for ever.
        let watch = Arc::clone(&core);
        let limit = opts.open_timeout.unwrap_or(OPEN_TIMEOUT);
        thread::Builder::new()
            .name(format!("aivory-{label}-watch"))
            .spawn(move || watch_open(&watch, limit))?;

        let run = Self { core };
        if run.send_input(&Input::first(&opts)).is_err() {
            return Err(std::io::Error::other("the agent process closed its input").into());
        }
        Ok((run, rx))
    }

    /// The native session id: the one the CLI reported, else the one that `resume` named.
    pub fn session_id(&self) -> Option<String> {
        lock(&self.core.session_id)
            .clone()
            .or_else(|| self.core.requested_id.clone())
    }

    /// The state for the war room: needs you, working, idle, or the final state.
    pub fn state(&self) -> SessionState {
        if let Some(s) = *lock(&self.core.exited) {
            return s;
        }
        if !lock(&self.core.asks).is_empty() {
            SessionState::NeedsYou
        } else if self.core.open_turns.load(Ordering::SeqCst) > 0 {
            SessionState::Working
        } else {
            SessionState::Idle
        }
    }

    /// Send the next user turn. The text also appears as a user line in the transcript.
    pub fn send(&self, text: &str) -> Result<(), LiveError> {
        self.send_input(&Input::text(text))
    }

    /// Send the next user turn with its images, and with a new mode, model, or effort for
    /// this turn and the turns after it.
    pub fn send_input(&self, input: &Input) -> Result<(), LiveError> {
        if lock(&self.core.exited).is_some() || lock(&self.core.input).is_none() {
            return Err(LiveError::NotRunning);
        }
        self.core.open_turns.fetch_add(1, Ordering::SeqCst);
        lock(&self.core.queue).push_back(input.clone());
        for line in input.lines(&[]) {
            send_event(&self.core.events, RunEvent::Line(line));
        }
        let mut d = lock(&self.core.dialect);
        self.core.settle(&mut **d);
        Ok(())
    }

    /// Answer an open ask.
    pub fn answer(&self, ask_id: &str, answer: Answer) -> Result<(), LiveError> {
        let slot = lock(&self.core.asks)
            .remove(ask_id)
            .ok_or_else(|| LiveError::UnknownAsk(ask_id.to_string()))?;
        let mut d = lock(&self.core.dialect);
        d.answer(slot, answer, &Io(&self.core));
        self.core.settle(&mut **d);
        Ok(())
    }

    /// Stop the turn that runs. The process stays alive for the next turn. Open asks are
    /// cancelled.
    pub fn interrupt(&self) -> Result<(), LiveError> {
        if lock(&self.core.input).is_none() {
            return Err(LiveError::NotRunning);
        }
        let mut d = lock(&self.core.dialect);
        d.interrupt(&Io(&self.core));
        self.core.cancel_asks(&**d);
        Ok(())
    }

    /// End the process now (its whole process tree on Windows). `Exited` follows.
    pub fn kill(&self) {
        self.core.kill();
    }
}

impl Drop for RpcRun {
    fn drop(&mut self) {
        self.core.kill();
    }
}

const OPEN_TIMEOUT: Duration = Duration::from_secs(90);

/// Fail the run when the handshake does not end within `limit`.
fn watch_open(core: &Core, limit: Duration) {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < limit {
        thread::sleep(step);
        waited += step;
        if core.ready.load(Ordering::SeqCst)
            || core.killed.load(Ordering::SeqCst)
            || lock(&core.exited).is_some()
        {
            return;
        }
    }
    core.failed.store(true, Ordering::SeqCst);
    send_event(
        &core.events,
        RunEvent::Failed(format!(
            "{} did not open a session within {} s",
            core.label,
            limit.as_secs().max(1)
        )),
    );
    core.kill();
}

fn read_loop(core: &Core, stdout: impl Read, trace: Option<&Trace>, stderr_tail: &Mutex<String>) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&buf);
        if let Some(t) = trace {
            t.write("<", text.trim_end());
        }
        // A line that is not a JSON object is skipped, never an error.
        let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
            continue;
        };
        if !value.is_object() {
            continue;
        }
        let mut d = lock(&core.dialect);
        core.handle(&value, &mut **d);
    }

    let status = core.reap();
    let code = status.and_then(|s| s.code());
    let killed = core.killed.load(Ordering::SeqCst);
    let failed = core.failed.load(Ordering::SeqCst);
    // Give the stderr thread a moment to read the last bytes.
    thread::sleep(Duration::from_millis(30));
    let tail = lock(stderr_tail).trim().to_string();
    let final_state = if failed {
        SessionState::Error
    } else if killed {
        SessionState::Parked
    } else if code == Some(0) {
        SessionState::Done
    } else {
        SessionState::Error
    };
    if !killed && code != Some(0) {
        let mut message = format!(
            "{} exited with code {}",
            core.label,
            code.map_or_else(|| "none".to_string(), |c| c.to_string())
        );
        if !tail.is_empty() {
            message.push_str(": ");
            message.push_str(&tail);
        }
        send_event(&core.events, RunEvent::Failed(message));
    }
    lock(&core.input).take();
    lock(&core.asks).clear();
    *lock(&core.exited) = Some(final_state);
    send_event(&core.events, RunEvent::Exited { code });
    // `Exited` is the last event: close the channel.
    lock(&core.events).take();
}
