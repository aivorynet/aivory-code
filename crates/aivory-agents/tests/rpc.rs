//! `RpcRun` (Codex app-server and Gemini ACP) against the fake agent
//! (tests/support/fake_agent.rs). No real CLI runs here.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use aivory_agents::AgentError;
use aivory_agents::live::{AnyRun, LiveError, RpcRun, RunEvent, StartOptions};
use aivory_agents::mcp::Answer;
use aivory_protocol::{Ask, AskKind, Line, LineKind, SessionState};

const WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy)]
enum Cli {
    Codex,
    Gemini,
}

fn fake() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-agent"))
}

fn options(prompt: &str) -> StartOptions {
    StartOptions {
        cwd: std::env::temp_dir(),
        prompt: prompt.into(),
        binary: Some(fake()),
        ..StartOptions::default()
    }
}

fn start_with(cli: Cli, opts: StartOptions) -> (RpcRun, Receiver<RunEvent>) {
    match cli {
        Cli::Codex => RpcRun::start_codex(opts),
        Cli::Gemini => RpcRun::start_gemini(opts),
    }
    .expect("start")
}

fn start(cli: Cli, prompt: &str) -> (RpcRun, Receiver<RunEvent>) {
    start_with(cli, options(prompt))
}

/// Collect events until `stop` accepts one. The accepted event is the last in the list.
fn until(rx: &Receiver<RunEvent>, stop: impl Fn(&RunEvent) -> bool) -> Vec<RunEvent> {
    let begin = Instant::now();
    let mut seen = Vec::new();
    loop {
        let left = WAIT.saturating_sub(begin.elapsed());
        let e = rx
            .recv_timeout(left)
            .unwrap_or_else(|e| panic!("no event ({e}); seen so far: {seen:#?}"));
        let done = stop(&e);
        seen.push(e);
        if done {
            return seen;
        }
    }
}

fn turn_end(rx: &Receiver<RunEvent>) -> Vec<RunEvent> {
    until(rx, |e| matches!(e, RunEvent::TurnEnded { .. }))
}

fn next_ask(rx: &Receiver<RunEvent>) -> (Vec<RunEvent>, Ask) {
    let seen = until(rx, |e| matches!(e, RunEvent::Ask(_)));
    let Some(RunEvent::Ask(ask)) = seen.last().cloned() else {
        unreachable!()
    };
    (seen, ask)
}

fn texts(events: &[RunEvent], kind: LineKind) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::Line(l) if l.kind == kind => Some(l.text.as_str()),
            _ => None,
        })
        .collect()
}

fn agent_texts(events: &[RunEvent]) -> Vec<&str> {
    texts(events, LineKind::Agent)
}

fn has_line(events: &[RunEvent], kind: LineKind, text: &str) -> bool {
    texts(events, kind).contains(&text)
}

fn no_failure(events: &[RunEvent]) {
    assert!(
        !events.iter().any(|e| matches!(e, RunEvent::Failed(_))),
        "unexpected failure in {events:#?}"
    );
}

const BOTH: [Cli; 2] = [Cli::Codex, Cli::Gemini];

#[test]
fn multi_turn_keeps_one_process_and_one_session() {
    for (cli, id) in [(Cli::Codex, "fake-thread"), (Cli::Gemini, "fake-gemini")] {
        let (run, rx) = start(cli, "say one");
        let first = turn_end(&rx);
        assert_eq!(first[0], RunEvent::Line(Line::user("say one")));
        assert!(first.contains(&RunEvent::Started {
            session_id: id.into()
        }));
        assert_eq!(agent_texts(&first), ["one"]);
        // 100 tokens of a 1000 token window. No cost: the CLIs do not tell it.
        assert_eq!(
            first.last(),
            Some(&RunEvent::TurnEnded {
                cost_usd: 0.0,
                context_pct: 10,
                tokens: 0
            })
        );
        assert_eq!(run.session_id().as_deref(), Some(id));
        assert_eq!(run.state(), SessionState::Idle);

        run.send("say two").unwrap();
        let second = turn_end(&rx);
        assert_eq!(second[0], RunEvent::Line(Line::user("say two")));
        assert_eq!(agent_texts(&second), ["two"]);
        assert!(!second.iter().any(|e| matches!(e, RunEvent::Started { .. })));
        run.kill();
    }
}

#[test]
fn turns_sent_during_a_turn_wait_for_their_turn() {
    for cli in BOTH {
        let (run, rx) = start(cli, "slow");
        until(
            &rx,
            |e| matches!(e, RunEvent::Line(l) if l.text == "working"),
        );
        run.send("say queued").unwrap();
        assert_eq!(run.state(), SessionState::Working);
        run.interrupt().unwrap();
        let first = turn_end(&rx);
        assert!(has_line(&first, LineKind::Meta, "Interrupted"));
        // The queued turn starts after the interrupted turn.
        let second = turn_end(&rx);
        assert_eq!(agent_texts(&second), ["queued"]);
        assert_eq!(run.state(), SessionState::Idle);
        run.kill();
    }
}

#[test]
fn permission_allow_runs_the_tool() {
    for cli in BOTH {
        let (run, rx) = start(cli, "run echo hi");
        let (seen, ask) = next_ask(&rx);
        // Gemini shows the tool line before the ask. Codex shows it when the command starts.
        assert_eq!(
            has_line(&seen, LineKind::Tool, "echo hi"),
            matches!(cli, Cli::Gemini)
        );
        assert_eq!(ask.kind, AskKind::Permission);
        assert_eq!(ask.text, "echo hi");
        assert_eq!(ask.options, ["Allow", "Always allow", "Deny"]);
        assert!(ask.risky);
        assert_eq!(run.state(), SessionState::NeedsYou);

        run.answer(&ask.id, Answer::allow()).unwrap();
        let rest = turn_end(&rx);
        assert!(has_line(&rest, LineKind::Output, "ran echo hi"));
        assert_eq!(
            has_line(&rest, LineKind::Tool, "echo hi"),
            matches!(cli, Cli::Codex)
        );
        assert_eq!(agent_texts(&rest), ["ok"]);
        assert_eq!(run.state(), SessionState::Idle);
        // A late second answer to the same ask finds nothing.
        assert!(matches!(
            run.answer(&ask.id, Answer::allow()),
            Err(LiveError::UnknownAsk(_))
        ));
        run.kill();
    }
}

#[test]
fn always_allow_is_a_session_grant() {
    for cli in BOTH {
        let (run, rx) = start(cli, "run cargo test");
        let (_, ask) = next_ask(&rx);
        run.answer(&ask.id, Answer::allow_always()).unwrap();
        assert_eq!(agent_texts(&turn_end(&rx)), ["ok (always)"]);
        run.kill();
    }
}

#[test]
fn permission_deny_tells_the_agent() {
    for cli in BOTH {
        let (run, rx) = start(cli, "run rm -rf x");
        let (_, ask) = next_ask(&rx);
        run.answer(&ask.id, Answer::deny("Not here.")).unwrap();
        let rest = turn_end(&rx);
        assert_eq!(agent_texts(&rest), ["denied"]);
        assert!(texts(&rest, LineKind::Error).len() == 1, "{rest:#?}");
        run.kill();
    }
}

#[test]
fn an_edit_ask_shows_the_diff() {
    for cli in BOTH {
        let (run, rx) = start(cli, "edit src/a.rs");
        let (seen, ask) = next_ask(&rx);
        assert_eq!(ask.kind, AskKind::Permission);
        assert_eq!(ask.text, "src/a.rs");
        assert!(has_line(&seen, LineKind::Tool, "src/a.rs"));
        let removed = texts(&seen, LineKind::Removed);
        let added = texts(&seen, LineKind::Added);
        assert_eq!((removed, added), (vec!["old"], vec!["new"]));
        run.answer(&ask.id, Answer::allow()).unwrap();
        turn_end(&rx);
        run.kill();
    }
}

#[test]
fn codex_questions_are_asked_one_by_one() {
    let (run, rx) = start(Cli::Codex, "question");
    let (_, first) = next_ask(&rx);
    assert_eq!(first.kind, AskKind::Question);
    assert_eq!(first.text, "Which color?");
    assert_eq!(first.options, ["Red", "Blue"]);
    run.answer(&first.id, Answer::choice("Blue")).unwrap();
    let (_, second) = next_ask(&rx);
    assert_eq!(second.text, "Which size?");
    assert_ne!(second.id, first.id);
    assert_eq!(run.state(), SessionState::NeedsYou);
    run.answer(&second.id, Answer::choice("large")).unwrap();
    assert_eq!(
        agent_texts(&turn_end(&rx)),
        [r#"answers: {"color":{"answers":["Blue"]},"size":{"answers":["large"]}}"#]
    );
    run.kill();
}

#[test]
fn interrupt_ends_the_turn_and_the_process_takes_the_next_one() {
    for cli in BOTH {
        let (run, rx) = start(cli, "slow");
        until(
            &rx,
            |e| matches!(e, RunEvent::Line(l) if l.text == "working"),
        );
        assert_eq!(run.state(), SessionState::Working);
        run.interrupt().unwrap();
        let seen = turn_end(&rx);
        assert!(has_line(&seen, LineKind::Meta, "Interrupted"));
        assert_eq!(run.state(), SessionState::Idle);

        run.send("say still here").unwrap();
        assert_eq!(agent_texts(&turn_end(&rx)), ["still here"]);
        run.kill();
    }
}

#[test]
fn interrupt_with_an_open_ask_cancels_it() {
    for cli in BOTH {
        let (run, rx) = start(cli, "run sleep 99");
        let (_, ask) = next_ask(&rx);
        run.interrupt().unwrap();
        let rest = turn_end(&rx);
        no_failure(&rest);
        assert!(matches!(
            run.answer(&ask.id, Answer::allow()),
            Err(LiveError::UnknownAsk(_))
        ));
        assert_eq!(run.state(), SessionState::Idle);
        run.kill();
    }
}

#[test]
fn kill_ends_the_process_without_a_failure() {
    for cli in BOTH {
        let (run, rx) = start(cli, "slow");
        until(
            &rx,
            |e| matches!(e, RunEvent::Line(l) if l.text == "working"),
        );
        run.kill();
        let rest = until(&rx, |e| matches!(e, RunEvent::Exited { .. }));
        no_failure(&rest);
        assert_eq!(run.state(), SessionState::Parked);
        assert!(matches!(run.send("say x"), Err(LiveError::NotRunning)));
        assert!(matches!(run.interrupt(), Err(LiveError::NotRunning)));
        // The channel closes after `Exited`.
        assert!(rx.recv_timeout(WAIT).is_err());
    }
}

#[test]
fn a_failed_turn_shows_one_error_line_and_ends() {
    for cli in BOTH {
        let (run, rx) = start(cli, "fail");
        let seen = turn_end(&rx);
        assert_eq!(texts(&seen, LineKind::Error), ["The model broke."]);
        // The run goes on.
        run.send("say next").unwrap();
        assert_eq!(agent_texts(&turn_end(&rx)), ["next"]);
        run.kill();
    }
}

#[test]
fn an_unexpected_exit_gives_failed_and_exited() {
    for cli in BOTH {
        let (run, rx) = start(cli, "exit 3");
        let seen = until(&rx, |e| matches!(e, RunEvent::Exited { .. }));
        assert_eq!(seen.last(), Some(&RunEvent::Exited { code: Some(3) }));
        let failed = seen
            .iter()
            .find_map(|e| match e {
                RunEvent::Failed(m) => Some(m.clone()),
                _ => None,
            })
            .expect("a failure event");
        assert!(failed.contains("code 3"), "{failed}");
        assert!(failed.contains("boom"), "{failed}");
        assert_eq!(run.state(), SessionState::Error);
    }
}

#[test]
fn codex_gets_the_approval_policy_and_the_sandbox() {
    let (run, rx) = start_with(
        Cli::Codex,
        StartOptions {
            permission_mode: Some("untrusted".into()),
            sandbox: Some("workspace-write".into()),
            ..options("config")
        },
    );
    assert_eq!(
        agent_texts(&turn_end(&rx)),
        ["policy=untrusted sandbox=workspace-write mode="]
    );
    run.kill();
}

#[test]
fn gemini_gets_the_approval_mode_twice() {
    // The flag sets the first mode. `session/set_mode` follows when the session differs.
    let (run, rx) = start_with(
        Cli::Gemini,
        StartOptions {
            permission_mode: Some("auto_edit".into()),
            ..options("config")
        },
    );
    assert_eq!(
        agent_texts(&turn_end(&rx)),
        ["policy= sandbox= mode=auto_edit"]
    );
    run.kill();
}

#[test]
fn codex_resume_and_fork_use_the_thread_commands() {
    let (run, rx) = start_with(
        Cli::Codex,
        StartOptions {
            resume: Some("old-thread".into()),
            ..options("say hi")
        },
    );
    assert_eq!(run.session_id().as_deref(), Some("old-thread"));
    let seen = turn_end(&rx);
    assert!(seen.contains(&RunEvent::Started {
        session_id: "old-thread".into()
    }));
    run.kill();

    let (run, rx) = start_with(
        Cli::Codex,
        StartOptions {
            resume: Some("old-thread".into()),
            fork: true,
            ..options("say hi")
        },
    );
    let seen = turn_end(&rx);
    assert!(seen.contains(&RunEvent::Started {
        session_id: "fork-of-old-thread".into()
    }));
    assert_eq!(run.session_id().as_deref(), Some("fork-of-old-thread"));
    run.kill();
}

#[test]
fn gemini_resume_loads_the_session_without_replaying_it() {
    let (run, rx) = start_with(
        Cli::Gemini,
        StartOptions {
            resume: Some("old-id".into()),
            ..options("say hi")
        },
    );
    let seen = turn_end(&rx);
    assert!(seen.contains(&RunEvent::Started {
        session_id: "old-id".into()
    }));
    // The old messages of `session/load` are not in the transcript of the run.
    assert_eq!(agent_texts(&seen), ["hi"]);
    run.kill();
}

#[test]
fn gemini_cannot_fork() {
    let r = RpcRun::start_gemini(StartOptions {
        resume: Some("old-id".into()),
        fork: true,
        ..options("say hi")
    });
    assert!(matches!(r, Err(AgentError::Io(_))));
}

#[test]
fn gemini_authenticates_when_the_session_asks_for_it() {
    let (run, rx) = start_with(
        Cli::Gemini,
        StartOptions {
            model: Some("needauth".into()),
            ..options("say hi")
        },
    );
    let seen = turn_end(&rx);
    no_failure(&seen);
    assert_eq!(agent_texts(&seen), ["hi"]);
    run.kill();
}

#[test]
fn a_cli_that_never_opens_its_session_fails_the_run() {
    for cli in BOTH {
        let (run, rx) = start_with(
            cli,
            StartOptions {
                model: Some("hang".into()),
                open_timeout: Some(Duration::from_millis(500)),
                ..options("say hi")
            },
        );
        let seen = until(&rx, |e| matches!(e, RunEvent::Exited { .. }));
        let failed = seen
            .iter()
            .find_map(|e| match e {
                RunEvent::Failed(m) => Some(m.clone()),
                _ => None,
            })
            .expect("a failure event");
        assert!(failed.contains("did not open a session"), "{failed}");
        assert_eq!(run.state(), SessionState::Error);
    }
}

#[test]
fn any_run_picks_the_driver_by_provider() {
    let (run, rx) = AnyRun::start("codex", options("say hi")).unwrap();
    assert_eq!(agent_texts(&turn_end(&rx)), ["hi"]);
    assert_eq!(run.session_id().as_deref(), Some("fake-thread"));
    run.kill();
    assert!(AnyRun::supports("gemini"));
    assert!(!AnyRun::supports("opencode"));
    assert!(AnyRun::start("opencode", options("x")).is_err());
}

#[test]
fn start_reports_a_missing_binary_and_folder() {
    for cli in BOTH {
        let missing = match cli {
            Cli::Codex => RpcRun::start_codex(StartOptions {
                binary: Some(PathBuf::from("no-such-dir").join("codex")),
                ..options("x")
            }),
            Cli::Gemini => RpcRun::start_gemini(StartOptions {
                binary: Some(PathBuf::from("no-such-dir").join("gemini")),
                ..options("x")
            }),
        };
        assert!(matches!(missing, Err(AgentError::ProgramNotFound(_))));
        let no_dir = match cli {
            Cli::Codex => RpcRun::start_codex(StartOptions {
                cwd: std::env::temp_dir().join("aivory-no-such-folder"),
                ..options("x")
            }),
            Cli::Gemini => RpcRun::start_gemini(StartOptions {
                cwd: std::env::temp_dir().join("aivory-no-such-folder"),
                ..options("x")
            }),
        };
        assert!(matches!(no_dir, Err(AgentError::CwdMissing(_))));
    }
}

#[test]
fn dropping_the_handle_kills_the_process() {
    for cli in BOTH {
        let (run, rx) = start(cli, "slow");
        until(
            &rx,
            |e| matches!(e, RunEvent::Line(l) if l.text == "working"),
        );
        drop(run);
        until(&rx, |e| matches!(e, RunEvent::Exited { .. }));
    }
}
