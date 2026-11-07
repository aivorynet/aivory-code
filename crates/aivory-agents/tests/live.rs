//! `LiveRun` against the fake `claude` (tests/support/fake_claude.rs). No real CLI runs here.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use aivory_agents::AgentError;
use aivory_agents::live::{LiveError, LiveRun, RunEvent, StartOptions};
use aivory_agents::mcp::Answer;
use aivory_protocol::{Ask, AskKind, Line, LineKind, SessionState};

const WAIT: Duration = Duration::from_secs(20);

fn fake() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-claude"))
}

fn start_with(prompt: &str, model: Option<&str>) -> (LiveRun, Receiver<RunEvent>) {
    LiveRun::start(StartOptions {
        cwd: std::env::temp_dir(),
        prompt: prompt.into(),
        model: model.map(str::to_string),
        permission_mode: Some("default".into()),
        binary: Some(fake()),
        ..StartOptions::default()
    })
    .expect("start")
}

fn start(prompt: &str) -> (LiveRun, Receiver<RunEvent>) {
    start_with(prompt, None)
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

fn agent_texts(events: &[RunEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::Line(l) if l.kind == LineKind::Agent => Some(l.text.as_str()),
            _ => None,
        })
        .collect()
}

fn has_line(events: &[RunEvent], kind: LineKind, text: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(e, RunEvent::Line(l) if l.kind == kind && l.text == text))
}

fn no_failure(events: &[RunEvent]) {
    assert!(
        !events.iter().any(|e| matches!(e, RunEvent::Failed(_))),
        "unexpected failure in {events:#?}"
    );
}

#[test]
fn multi_turn_send_keeps_one_process_and_one_session() {
    let (run, rx) = start("say one");
    let first = turn_end(&rx);
    assert_eq!(
        first[0],
        RunEvent::Line(Line::user("say one")),
        "the prompt is the first line"
    );
    assert!(first.contains(&RunEvent::Started {
        session_id: "fake-session".into()
    }));
    assert_eq!(agent_texts(&first), ["one"]);
    // 100 input tokens of a 1000 token window.
    assert_eq!(
        first.last(),
        Some(&RunEvent::TurnEnded {
            cost_usd: 0.01,
            context_pct: 10,
            tokens: 0
        })
    );
    assert_eq!(run.session_id().as_deref(), Some("fake-session"));
    assert_eq!(run.state(), SessionState::Idle);

    run.send("say two").unwrap();
    let second = turn_end(&rx);
    assert_eq!(second[0], RunEvent::Line(Line::user("say two")));
    assert_eq!(agent_texts(&second), ["two"]);
    // The init event of the second turn does not give a second `Started`.
    assert!(!second.iter().any(|e| matches!(e, RunEvent::Started { .. })));
    assert_eq!(
        second.last(),
        Some(&RunEvent::TurnEnded {
            cost_usd: 0.02,
            context_pct: 10,
            tokens: 0
        })
    );

    run.send("say three").unwrap();
    assert_eq!(agent_texts(&turn_end(&rx)), ["three"]);
    run.kill();
}

#[test]
fn permission_allow_runs_the_tool() {
    let (run, rx) = start("run echo hi");
    let (seen, ask) = next_ask(&rx);
    assert!(has_line(&seen, LineKind::Tool, "echo hi"));
    assert_eq!(ask.kind, AskKind::Permission);
    assert_eq!(ask.text, "echo hi");
    assert_eq!(ask.options, ["Allow", "Always allow", "Deny"]);
    assert_eq!(run.state(), SessionState::NeedsYou);

    run.answer(&ask.id, Answer::allow()).unwrap();
    let rest = turn_end(&rx);
    assert!(has_line(&rest, LineKind::Output, "ran echo hi"));
    assert_eq!(agent_texts(&rest), ["ok"]);
    assert_eq!(run.state(), SessionState::Idle);
    // A late second answer to the same ask finds nothing.
    assert!(matches!(
        run.answer(&ask.id, Answer::allow()),
        Err(LiveError::UnknownAsk(_))
    ));
    run.kill();
}

#[test]
fn permission_deny_sends_the_message_to_the_agent() {
    let (run, rx) = start("run rm -rf x");
    let (_, ask) = next_ask(&rx);
    run.answer(&ask.id, Answer::deny("Not in this folder."))
        .unwrap();
    let rest = turn_end(&rx);
    assert!(has_line(&rest, LineKind::Error, "Not in this folder."));
    assert_eq!(agent_texts(&rest), ["denied: Not in this folder."]);
    run.kill();
}

#[test]
fn always_allow_skips_the_ask_for_the_same_command() {
    let (run, rx) = start("run cargo test");
    let (_, ask) = next_ask(&rx);
    run.answer(&ask.id, Answer::allow_always()).unwrap();
    turn_end(&rx);

    run.send("run cargo test").unwrap();
    let second = turn_end(&rx);
    assert!(!second.iter().any(|e| matches!(e, RunEvent::Ask(_))));
    assert_eq!(agent_texts(&second), ["ok"]);

    // Another command asks again.
    run.send("run cargo clean").unwrap();
    let (_, ask) = next_ask(&rx);
    assert_eq!(ask.text, "cargo clean");
    run.answer(&ask.id, Answer::deny("No.")).unwrap();
    turn_end(&rx);
    run.kill();
}

#[test]
fn plan_and_question_become_asks() {
    let (run, rx) = start("plan");
    let (_, ask) = next_ask(&rx);
    assert_eq!(ask.kind, AskKind::Plan);
    assert_eq!(ask.text, "Cache rows");
    assert_eq!(ask.steps, ["Measure the rows", "Draw the visible rows"]);
    run.answer(&ask.id, Answer::deny("Add a test step."))
        .unwrap();
    assert_eq!(
        agent_texts(&turn_end(&rx)),
        ["plan rejected: Add a test step."]
    );

    run.send("question").unwrap();
    let (_, ask) = next_ask(&rx);
    assert_eq!(ask.kind, AskKind::Question);
    assert_eq!(ask.text, "Which color?");
    assert_eq!(ask.options, ["Red", "Blue"]);
    run.answer(&ask.id, Answer::choice("Blue")).unwrap();
    assert_eq!(
        agent_texts(&turn_end(&rx)),
        ["answers: {\"Which color?\":\"Blue\"}"]
    );
    run.kill();
}

#[test]
fn interrupt_ends_the_turn_and_the_process_takes_the_next_one() {
    let (run, rx) = start("slow");
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

#[test]
fn interrupt_with_an_open_ask_clears_the_ask() {
    let (run, rx) = start("run sleep 99");
    let (_, ask) = next_ask(&rx);
    run.interrupt().unwrap();
    // The turn ends, either interrupted or with a denied tool call.
    turn_end(&rx);
    assert!(matches!(
        run.answer(&ask.id, Answer::allow()),
        Err(LiveError::UnknownAsk(_))
    ));
    assert_eq!(run.state(), SessionState::Idle);
    run.kill();
}

#[test]
fn kill_ends_the_process_without_a_failure() {
    let (run, rx) = start("slow");
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

#[test]
fn kill_denies_the_open_ask() {
    let (run, rx) = start("run echo hi");
    let (_, ask) = next_ask(&rx);
    run.kill();
    until(&rx, |e| matches!(e, RunEvent::Exited { .. }));
    assert!(run.answer(&ask.id, Answer::allow()).is_err());
}

#[test]
fn a_wrong_token_gets_401_and_no_ask() {
    let (run, rx) = start_with("run echo hi", Some("badtoken"));
    let seen = turn_end(&rx);
    assert!(!seen.iter().any(|e| matches!(e, RunEvent::Ask(_))));
    assert_eq!(agent_texts(&seen), ["permission call failed: 401"]);
    run.kill();
}

#[test]
fn malformed_output_lines_are_skipped() {
    let (run, rx) = start("garbage");
    let seen = turn_end(&rx);
    no_failure(&seen);
    assert_eq!(agent_texts(&seen), ["after garbage"]);
    // The run goes on after the bad lines.
    run.send("say next").unwrap();
    assert_eq!(agent_texts(&turn_end(&rx)), ["next"]);
    run.kill();
}

#[test]
fn an_unexpected_exit_gives_failed_and_exited() {
    let (run, rx) = start("exit 3");
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

#[test]
fn start_reports_a_missing_binary_and_folder() {
    let missing = LiveRun::start(StartOptions {
        cwd: std::env::temp_dir(),
        prompt: "x".into(),
        binary: Some(PathBuf::from("no-such-dir").join("claude")),
        ..StartOptions::default()
    });
    assert!(matches!(missing, Err(AgentError::ProgramNotFound(_))));
    let no_dir = LiveRun::start(StartOptions {
        cwd: std::env::temp_dir().join("aivory-no-such-folder"),
        prompt: "x".into(),
        binary: Some(fake()),
        ..StartOptions::default()
    });
    assert!(matches!(no_dir, Err(AgentError::CwdMissing(_))));
}

#[test]
fn dropping_the_handle_kills_the_process() {
    let (run, rx) = start("slow");
    until(
        &rx,
        |e| matches!(e, RunEvent::Line(l) if l.text == "working"),
    );
    drop(run);
    let rest = until(&rx, |e| matches!(e, RunEvent::Exited { .. }));
    no_failure(&rest);
}

const UUID_A: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const UUID_B: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

fn start_opts(opts: StartOptions) -> Result<(LiveRun, Receiver<RunEvent>), AgentError> {
    LiveRun::start(StartOptions {
        cwd: std::env::temp_dir(),
        binary: Some(fake()),
        ..opts
    })
}

#[test]
fn session_id_is_known_before_the_first_turn() {
    let (run, rx) = start_opts(StartOptions {
        prompt: "say hi".into(),
        session_id: Some(UUID_A.into()),
        ..StartOptions::default()
    })
    .unwrap();
    assert_eq!(run.session_id().as_deref(), Some(UUID_A));
    let seen = turn_end(&rx);
    assert!(seen.contains(&RunEvent::Started {
        session_id: UUID_A.into()
    }));
    run.kill();
}

#[test]
fn resume_continues_a_native_session_with_the_prompt_as_next_turn() {
    let (run, rx) = start_opts(StartOptions {
        prompt: "say again".into(),
        resume: Some(UUID_A.into()),
        ..StartOptions::default()
    })
    .unwrap();
    assert_eq!(run.session_id().as_deref(), Some(UUID_A));
    let seen = turn_end(&rx);
    assert!(seen.contains(&RunEvent::Started {
        session_id: UUID_A.into()
    }));
    assert_eq!(agent_texts(&seen), ["again"]);
    run.kill();
}

#[test]
fn resume_with_a_new_session_id_forks() {
    // The fake exits with code 2 unless `--fork-session` comes with both flags.
    let (run, rx) = start_opts(StartOptions {
        prompt: "say forked".into(),
        resume: Some(UUID_A.into()),
        session_id: Some(UUID_B.into()),
        ..StartOptions::default()
    })
    .unwrap();
    assert_eq!(run.session_id().as_deref(), Some(UUID_B));
    let seen = turn_end(&rx);
    assert!(seen.contains(&RunEvent::Started {
        session_id: UUID_B.into()
    }));
    run.kill();
}

#[test]
fn a_session_id_that_is_not_a_uuid_is_refused() {
    let r = start_opts(StartOptions {
        prompt: "x".into(),
        session_id: Some("not-a-uuid".into()),
        ..StartOptions::default()
    });
    assert!(matches!(r, Err(AgentError::Io(_))));
}
