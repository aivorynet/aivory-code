//! A live Claude session that the daemon owns.
//!
//! [`LiveRun::start`] spawns the user's own `claude` binary with
//! `-p --input-format stream-json --output-format stream-json`. The process keeps its stdin
//! open, so that [`LiveRun::send`] can add one user turn after another.
//!
//! Permission requests, plan approvals, and questions do not block on a terminal. Claude calls
//! the `permission` tool of a local [`McpServer`] (`--permission-prompt-tool`). Each call becomes
//! a [`RunEvent::Ask`]. The call stays open until [`LiveRun::answer`] arrives. An ask lives
//! until it is answered, until the turn ends, or until the process exits.
//!
//! The crate never reads or stores login data. Claude uses its own login. The only secret is
//! the random token of the local MCP port, which goes to the child in an environment variable.
//!
//! For debugging, set `AIVORY_LIVE_TRACE=<file>`: the run then appends every raw line of the
//! child stdout and stdin to that file.
//!
//! Codex and Gemini run through [`RpcRun`] (JSON-RPC on stdio, see `rpc.rs`): `codex app-server`
//! and `gemini --acp`. [`AnyRun`] is one handle for all three providers.

mod acp;
mod any;
mod codex;
mod rpc;
mod stream;

pub use any::AnyRun;
pub use rpc::RpcRun;

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use aivory_protocol::{Ask, Line, SessionState};
use serde_json::{Value, json};

use crate::command::resolve_program;
use crate::error::AgentError;
use crate::mcp::{Answer, McpServer, PERMISSION_TOOL};

use stream::Mapper;

const TOKEN_ENV: &str = "AIVORY_MCP_TOKEN";
const TRACE_ENV: &str = "AIVORY_LIVE_TRACE";
const STDERR_KEEP_BYTES: usize = 8 * 1024;

/// How to start a run.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// Working directory of the session.
    pub cwd: PathBuf,
    /// The first user turn.
    pub prompt: String,
    /// Passed to `--model`.
    pub model: Option<String>,
    /// Passed to `--permission-mode`, for example `default`, `plan`, or `acceptEdits`.
    pub permission_mode: Option<String>,
    /// Use this binary instead of `claude` from PATH.
    pub binary: Option<PathBuf>,
    /// Passed to `--session-id`: the id of the new session, so that the caller knows it before
    /// the first turn. It must be a UUID. With `resume`, Claude forks the old session into
    /// this new id (`--fork-session`).
    pub session_id: Option<String>,
    /// Passed to `--resume`: continue this native session. `prompt` is the next user turn.
    /// Without `session_id`, the run keeps the id of the old session.
    pub resume: Option<String>,
    /// Codex only: the sandbox mode (`read-only`, `workspace-write`, `danger-full-access`).
    /// For Codex, `permission_mode` is the approval policy (`untrusted`, `on-request`, `never`).
    /// For Gemini, `permission_mode` is the approval mode (`default`, `auto_edit`, `plan`).
    pub sandbox: Option<String>,
    /// Codex only: extra developer instructions for the thread (for example "plan only").
    pub instructions: Option<String>,
    /// Codex only, with `resume`: fork the native thread (`thread/fork`) instead of resuming it.
    pub fork: bool,
    /// Codex and Gemini: how long the CLI may take to open the session before the run fails.
    /// The default is 90 seconds.
    pub open_timeout: Option<Duration>,
}

/// What a run reports. The channel closes after [`RunEvent::Exited`].
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// Claude accepted the session. It arrives with the first turn.
    Started { session_id: String },
    /// A transcript line. The user turns are in the transcript too: the first prompt and every
    /// [`LiveRun::send`].
    Line(Line),
    /// The agent waits for an answer. Call [`LiveRun::answer`] with the id of the ask.
    Ask(Ask),
    /// A turn ended. The process stays alive and waits for the next turn.
    /// `cost_usd` is the cost of the session so far. `context_pct` is the share of the context
    /// window in use (0 to 100).
    /// `tokens` counts the tokens of the session so far without prompt-cache reads (0 when
    /// the CLI does not tell them).
    TurnEnded {
        cost_usd: f64,
        context_pct: u8,
        tokens: u64,
    },
    /// The process exited. This is the last event. `None` means that a signal ended it.
    Exited { code: Option<i32> },
    /// The run broke: the process exited with an error that nobody asked for. `Exited` follows.
    Failed(String),
}

/// A request that cannot go to the process.
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("the agent process is not running")]
    NotRunning,
    #[error("no open ask has the id {0}")]
    UnknownAsk(String),
}

struct Shared {
    /// Lines for the stdin of the child. `None` after the process ended or was closed.
    input: Mutex<Option<Sender<String>>>,
    child: Mutex<Child>,
    /// The program is a `.cmd` shim: killing it needs `taskkill /T`.
    shim: bool,
    mcp: McpServer,
    /// Taken at the end, so that the receiver sees the channel close after `Exited`.
    events: Arc<Mutex<Option<Sender<RunEvent>>>>,
    session_id: Mutex<Option<String>>,
    /// The id that the caller asked for, known before the init event.
    requested_id: Option<String>,
    /// User turns sent minus results received.
    open_turns: AtomicUsize,
    killed: AtomicBool,
    /// The state after the process exited.
    exited: Mutex<Option<SessionState>>,
    control_seq: AtomicU64,
    config_file: PathBuf,
}

/// A handle to a running session. Dropping it kills the process.
pub struct LiveRun {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for LiveRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveRun")
            .field("session_id", &self.session_id())
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl LiveRun {
    /// Spawn the process, send the first prompt, and return the handle and the event channel.
    pub fn start(opts: StartOptions) -> Result<(Self, Receiver<RunEvent>), AgentError> {
        if !opts.cwd.is_dir() {
            return Err(AgentError::CwdMissing(opts.cwd));
        }
        if let Some(id) = &opts.session_id
            && !is_uuid(id)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("the session id must be a UUID: {id}"),
            )
            .into());
        }
        let requested_id = opts
            .session_id
            .clone()
            .or_else(|| opts.resume.clone().filter(|r| is_uuid(r)));
        let wanted = opts
            .binary
            .clone()
            .unwrap_or_else(|| PathBuf::from("claude"));
        let program = unwrap_npm_shim(resolve_program(&wanted)?);
        let shim = is_shell_script(&program);

        let (tx, rx) = mpsc::channel::<RunEvent>();
        let events = Arc::new(Mutex::new(Some(tx)));
        let ask_events = Arc::clone(&events);
        let mcp = McpServer::start(move |ask| send_event(&ask_events, RunEvent::Ask(ask)))?;
        // The code and coordination tools pick the codebase from the folder, and know the sender.
        mcp.set_cwd(&opts.cwd);
        if let Some(id) = &requested_id {
            mcp.set_session(id.clone());
        }

        let config_file = std::env::temp_dir().join(format!(
            "aivory-mcp-{}-{}.json",
            std::process::id(),
            mcp.port()
        ));
        fs::write(&config_file, mcp.config_json(TOKEN_ENV))?;

        let mut cmd = Command::new(&program);
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--mcp-config",
        ])
        .arg(&config_file)
        .args(["--permission-prompt-tool", PERMISSION_TOOL]);
        if let Some(id) = &opts.resume {
            cmd.args(["--resume", id]);
        }
        if let Some(id) = &opts.session_id {
            cmd.args(["--session-id", id]);
            if opts.resume.is_some() {
                // Claude refuses `--session-id` with `--resume` unless the session forks.
                cmd.arg("--fork-session");
            }
        }
        if let Some(mode) = &opts.permission_mode {
            cmd.args(["--permission-mode", mode]);
        }
        if let Some(model) = &opts.model {
            cmd.args(["--model", model]);
        }
        cmd.current_dir(&opts.cwd)
            .env(TOKEN_ENV, mcp.token())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_window(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(source) => {
                let _ = fs::remove_file(&config_file);
                return Err(AgentError::Spawn {
                    program: program.display().to_string(),
                    source,
                });
            }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
            let _ = child.kill();
            let _ = fs::remove_file(&config_file);
            return Err(std::io::Error::other("a pipe of the child is missing").into());
        };

        let (input_tx, input_rx) = mpsc::channel::<String>();
        let trace = open_trace();
        let shared = Arc::new(Shared {
            input: Mutex::new(Some(input_tx)),
            child: Mutex::new(child),
            shim,
            mcp,
            events,
            session_id: Mutex::new(None),
            requested_id,
            open_turns: AtomicUsize::new(0),
            killed: AtomicBool::new(false),
            exited: Mutex::new(None),
            control_seq: AtomicU64::new(1),
            config_file,
        });

        let writer_trace = trace.clone();
        thread::Builder::new()
            .name("aivory-live-stdin".into())
            .spawn(move || write_loop(stdin, &input_rx, writer_trace.as_ref()))?;
        let tail = Arc::new(Mutex::new(String::new()));
        let stderr_tail = Arc::clone(&tail);
        thread::Builder::new()
            .name("aivory-live-stderr".into())
            .spawn(move || read_tail(stderr, STDERR_KEEP_BYTES, &stderr_tail))?;
        let reader_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("aivory-live-stdout".into())
            .spawn(move || read_loop(&reader_shared, stdout, trace.as_ref(), &tail))?;

        let run = Self { shared };
        if run.send(&opts.prompt).is_err() {
            return Err(std::io::Error::other("the agent process closed its input").into());
        }
        Ok((run, rx))
    }

    /// The session id: the one from the init event once it arrived, else the one that
    /// `StartOptions` asked for (`session_id`, or the `resume` id).
    pub fn session_id(&self) -> Option<String> {
        lock(&self.shared.session_id)
            .clone()
            .or_else(|| self.shared.requested_id.clone())
    }

    /// The state for the war room: needs you, working, idle, or the final state.
    pub fn state(&self) -> SessionState {
        if let Some(s) = *lock(&self.shared.exited) {
            return s;
        }
        if self.shared.mcp.pending() > 0 {
            SessionState::NeedsYou
        } else if self.shared.open_turns.load(Ordering::SeqCst) > 0 {
            SessionState::Working
        } else {
            SessionState::Idle
        }
    }

    /// Send the next user turn. The text also appears as a user line in the transcript.
    pub fn send(&self, text: &str) -> Result<(), LiveError> {
        if lock(&self.shared.exited).is_some() {
            return Err(LiveError::NotRunning);
        }
        let message = json!({
            "type": "user",
            "message": { "role": "user", "content": text },
        });
        self.shared.open_turns.fetch_add(1, Ordering::SeqCst);
        if let Err(e) = self.push(message.to_string()) {
            self.shared.open_turns.fetch_sub(1, Ordering::SeqCst);
            return Err(e);
        }
        send_event(&self.shared.events, RunEvent::Line(Line::user(text)));
        Ok(())
    }

    /// Answer an open ask.
    pub fn answer(&self, ask_id: &str, answer: Answer) -> Result<(), LiveError> {
        if self.shared.mcp.answer(ask_id, answer) {
            Ok(())
        } else {
            Err(LiveError::UnknownAsk(ask_id.to_string()))
        }
    }

    /// Stop the turn that runs. The process stays alive for the next turn. Open asks are denied.
    pub fn interrupt(&self) -> Result<(), LiveError> {
        let seq = self.shared.control_seq.fetch_add(1, Ordering::SeqCst);
        let request = json!({
            "type": "control_request",
            "request_id": format!("aivory-{seq}"),
            "request": { "subtype": "interrupt" },
        });
        self.push(request.to_string())?;
        self.shared.mcp.cancel_all("The user interrupted the turn.");
        Ok(())
    }

    /// End the process now (its whole process tree on Windows). `Exited` follows.
    pub fn kill(&self) {
        self.shared.kill();
    }

    fn push(&self, line: String) -> Result<(), LiveError> {
        match lock(&self.shared.input).as_ref() {
            Some(tx) => tx.send(line).map_err(|_| LiveError::NotRunning),
            None => Err(LiveError::NotRunning),
        }
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        self.shared.kill();
    }
}

impl Shared {
    fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        self.mcp.cancel_all("The AIVory session was stopped.");
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

    fn emit(&self, event: RunEvent) {
        send_event(&self.events, event);
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
}

fn read_loop(
    shared: &Shared,
    stdout: impl Read,
    trace: Option<&Trace>,
    stderr_tail: &Mutex<String>,
) {
    let mut mapper = Mapper::default();
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
        for event in mapper.map(&value) {
            match &event {
                RunEvent::Started { session_id } => {
                    // Claude prints the init event again for each turn. Report it once.
                    let mut known = lock(&shared.session_id);
                    if known.as_deref() == Some(session_id.as_str()) {
                        continue;
                    }
                    *known = Some(session_id.clone());
                    shared.mcp.set_session(session_id.clone());
                }
                RunEvent::TurnEnded { .. } => {
                    // A turn that ends leaves no ask open.
                    let _ =
                        shared
                            .open_turns
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
                    shared.mcp.cancel_all("The turn ended.");
                }
                _ => {}
            }
            shared.emit(event);
        }
    }

    let status = shared.reap();
    let code = status.and_then(|s| s.code());
    let killed = shared.killed.load(Ordering::SeqCst);
    // Give the stderr thread a moment to read the last bytes.
    thread::sleep(Duration::from_millis(30));
    let tail = lock(stderr_tail).trim().to_string();
    let final_state = if killed {
        SessionState::Parked
    } else if code == Some(0) {
        SessionState::Done
    } else {
        SessionState::Error
    };
    if !killed && code != Some(0) {
        let mut message = format!(
            "claude exited with code {}",
            code.map_or_else(|| "none".to_string(), |c| c.to_string())
        );
        if !tail.is_empty() {
            message.push_str(": ");
            message.push_str(&tail);
        }
        shared.emit(RunEvent::Failed(message));
    }
    lock(&shared.input).take();
    shared.mcp.shutdown();
    let _ = fs::remove_file(&shared.config_file);
    *lock(&shared.exited) = Some(final_state);
    shared.emit(RunEvent::Exited { code });
    // `Exited` is the last event: close the channel.
    lock(&shared.events).take();
}

/// Send an event. `false` means that nobody receives events any more.
fn send_event(events: &Mutex<Option<Sender<RunEvent>>>, event: RunEvent) -> bool {
    lock(events)
        .as_ref()
        .is_some_and(|tx| tx.send(event).is_ok())
}

fn write_loop(mut stdin: impl Write, lines: &Receiver<String>, trace: Option<&Trace>) {
    while let Ok(line) = lines.recv() {
        if let Some(t) = trace {
            t.write(">", &line);
        }
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .is_err()
        {
            break;
        }
    }
    // `stdin` drops here and closes the pipe.
}

/// Keep the last `keep` bytes of a stream as text.
fn read_tail(mut r: impl Read, keep: usize, out: &Mutex<String>) {
    let mut all: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = r.read(&mut chunk) {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&chunk[..n]);
        if all.len() > keep * 2 {
            all.drain(..all.len() - keep);
        }
        let start = all.len().saturating_sub(keep);
        *lock(out) = String::from_utf8_lossy(&all[start..]).into_owned();
    }
}

#[derive(Debug, Clone)]
struct Trace(Arc<Mutex<fs::File>>);

impl Trace {
    fn write(&self, direction: &str, line: &str) {
        let _ = writeln!(lock(&self.0), "{direction} {line}");
    }
}

fn open_trace() -> Option<Trace> {
    let path = std::env::var_os(TRACE_ENV)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    Some(Trace(Arc::new(Mutex::new(file))))
}

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// `8-4-4-4-12` hex digits.
fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_shell_script(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// The npm shim `claude.cmd` only starts `claude.exe`. Start the exe itself: the arguments
/// then need no `cmd.exe` quoting, and `kill` reaches the real process.
fn unwrap_npm_shim(path: PathBuf) -> PathBuf {
    if cfg!(windows)
        && is_shell_script(&path)
        && path
            .file_stem()
            .is_some_and(|s| s.eq_ignore_ascii_case("claude"))
        && let Some(dir) = path.parent()
    {
        let exe = dir
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin")
            .join("claude.exe");
        if exe.is_file() {
            return exe;
        }
    }
    path
}
