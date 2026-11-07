//! Tests of the process driver with a fake CLI script (no real agent is started).

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use aivory_agents::{AgentError, AgentEvent, ResumeRequest, RunOptions, run_resume};
use aivory_sessions::Provider;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Write a script that behaves like the CLI. `body_windows` and `body_unix` are the script lines.
fn fake_cli(tag: &str, body_windows: &str, body_unix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aivory-agents-test-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    if cfg!(windows) {
        let path = dir.join("fake-cli.cmd");
        std::fs::write(&path, format!("@echo off\r\n{body_windows}\r\n")).unwrap();
        path
    } else {
        let path = dir.join("fake-cli.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body_unix}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
}

fn request(program: PathBuf) -> ResumeRequest {
    ResumeRequest {
        provider: Provider::Claude,
        session_id: "sess-claude-1".into(),
        cwd: std::env::temp_dir(),
        prompt: "Reply with OK".into(),
        session_path: None,
        model: None,
        extra_args: Vec::new(),
        program: Some(program),
    }
}

fn collect(req: &ResumeRequest, opts: &RunOptions) -> (Vec<AgentEvent>, aivory_agents::RunOutcome) {
    let mut events = Vec::new();
    let outcome = run_resume(req, opts, &mut |e| {
        events.push(e);
        ControlFlow::Continue(())
    })
    .unwrap();
    (events, outcome)
}

fn quoted(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn streams_events_and_ends_with_turn_completed() {
    let json = fixture("claude-stream.jsonl");
    let cli = fake_cli(
        "ok",
        &format!("type \"{}\"", quoted(&json)),
        &format!("cat '{}'", quoted(&json)),
    );
    let (events, outcome) = collect(&request(cli), &RunOptions::default());
    assert!(matches!(events.first(), Some(AgentEvent::Started { .. })));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted { .. })
    ));
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Raw { .. })),
        "raw events are dropped by default"
    );
    assert!(outcome.completed && !outcome.cancelled);
    assert_eq!(outcome.exit_code, Some(0));

    let cli = fake_cli(
        "raw",
        &format!("type \"{}\"", quoted(&json)),
        &format!("cat '{}'", quoted(&json)),
    );
    let (with_raw, _) = collect(&request(cli), &RunOptions { include_raw: true });
    assert!(with_raw.iter().any(|e| matches!(e, AgentEvent::Raw { .. })));
}

#[test]
fn early_exit_becomes_a_fatal_error_with_the_stderr_tail() {
    let cli = fake_cli(
        "fail",
        "echo something broke 1>&2\r\nexit /b 3",
        "echo something broke >&2\nexit 3",
    );
    let (events, outcome) = collect(&request(cli), &RunOptions::default());
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::Error { message, fatal } => {
            assert!(*fatal);
            assert!(
                message.contains("code 3") && message.contains("something broke"),
                "message was: {message}"
            );
        }
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(!outcome.completed);
    assert_eq!(outcome.exit_code, Some(3));
}

#[test]
fn the_sink_can_cancel_the_run() {
    let json = fixture("claude-stream.jsonl");
    let cli = fake_cli(
        "cancel",
        &format!("type \"{}\"", quoted(&json)),
        &format!("cat '{}'", quoted(&json)),
    );
    let mut seen = 0;
    let outcome = run_resume(&request(cli), &RunOptions::default(), &mut |_| {
        seen += 1;
        ControlFlow::Break(())
    })
    .unwrap();
    assert_eq!(seen, 1);
    assert!(outcome.cancelled && !outcome.completed);
}

#[test]
fn missing_cwd_and_missing_program_are_errors() {
    let mut req = request(PathBuf::from("definitely-not-a-real-cli-name"));
    req.cwd = std::env::temp_dir().join("aivory-no-such-dir");
    assert!(matches!(
        run_resume(
            &req,
            &RunOptions::default(),
            &mut |_| ControlFlow::Continue(())
        ),
        Err(AgentError::CwdMissing(_))
    ));
    req.cwd = std::env::temp_dir();
    assert!(matches!(
        run_resume(
            &req,
            &RunOptions::default(),
            &mut |_| ControlFlow::Continue(())
        ),
        Err(AgentError::ProgramNotFound(_))
    ));
}
