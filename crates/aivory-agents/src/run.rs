use std::io::{BufRead, BufReader, Read, Write};
use std::ops::ControlFlow;
use std::process::{Command, Stdio};
use std::thread;

use crate::command::{ResumeRequest, build_command, resolve_program};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::parse::parser_for;

const STDERR_KEEP_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Pass `Raw` events to the sink. They are dropped by default.
    pub include_raw: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    pub exit_code: Option<i32>,
    /// A `TurnCompleted` event was sent.
    pub completed: bool,
    /// The sink asked to stop, so the child was killed.
    pub cancelled: bool,
    /// Events passed to the sink.
    pub events: usize,
}

/// Resume a session and stream its events to `sink`.
///
/// The run always ends with a terminal event unless the sink cancels it: `TurnCompleted`
/// on success, or a fatal `Error` (for example when the CLI exits early; the end of its
/// stderr is added to the message).
pub fn run_resume(
    req: &ResumeRequest,
    opts: &RunOptions,
    sink: &mut dyn FnMut(AgentEvent) -> ControlFlow<()>,
) -> Result<RunOutcome, AgentError> {
    let spec = build_command(req)?;
    if !spec.cwd.is_dir() {
        return Err(AgentError::CwdMissing(spec.cwd));
    }
    let program = resolve_program(&spec.program)?;
    let mut cmd = Command::new(&program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|source| AgentError::Spawn {
        program: program.display().to_string(),
        source,
    })?;

    // Write the prompt, then close stdin so that the CLI starts.
    let stdin_thread = child.stdin.take().map(|mut stdin| {
        let text = spec.stdin.clone();
        thread::spawn(move || {
            if let Some(text) = text {
                let _ = stdin.write_all(text.as_bytes());
            }
        })
    });
    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| thread::spawn(move || read_tail(stderr, STDERR_KEEP_BYTES)));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout pipe"))?;

    let mut parser = parser_for(req.provider);
    let mut outcome = RunOutcome::default();
    let mut ended = false;
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    'lines: loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        for event in parser.parse_line(&line) {
            ended |= event.is_terminal();
            outcome.completed |= matches!(event, AgentEvent::TurnCompleted { .. });
            if matches!(event, AgentEvent::Raw { .. }) && !opts.include_raw {
                continue;
            }
            outcome.events += 1;
            if sink(event).is_break() {
                outcome.cancelled = true;
                let _ = child.kill();
                break 'lines;
            }
        }
    }

    let status = child.wait()?;
    outcome.exit_code = status.code();
    if let Some(t) = stdin_thread {
        let _ = t.join();
    }
    let stderr_tail = stderr_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();
    if !ended && !outcome.cancelled {
        let tail = stderr_tail.trim();
        let mut message = format!(
            "{} exited with code {} before the turn completed",
            req.provider,
            status
                .code()
                .map_or_else(|| "none".to_string(), |c| c.to_string())
        );
        if !tail.is_empty() {
            message.push_str(": ");
            message.push_str(tail);
        }
        outcome.events += 1;
        let _ = sink(AgentEvent::Error {
            message,
            fatal: true,
        });
    }
    Ok(outcome)
}

/// Read a stream to its end and keep the last `keep` bytes as text.
fn read_tail(mut r: impl Read, keep: usize) -> String {
    let mut all = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = r.read(&mut chunk) {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&chunk[..n]);
        if all.len() > keep * 2 {
            all.drain(..all.len() - keep);
        }
    }
    let start = all.len().saturating_sub(keep);
    String::from_utf8_lossy(&all[start..]).into_owned()
}
