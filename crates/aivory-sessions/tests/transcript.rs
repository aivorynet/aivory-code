//! Fixture tests for `transcript`: line kinds, paged history, state rules, sub-agents, and
//! malformed input. The fixtures in `tests/fixtures/transcript` contain synthetic data only.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aivory_protocol::{AskKind, Line, SessionState};
use aivory_sessions::Provider;
use aivory_sessions::timeutil::parse_rfc3339_ms;
use aivory_sessions::transcript::{
    IDLE_WINDOW_MS, WORKING_WINDOW_MS, derive_state, history, normalize, subagent_path, subagents,
    tail,
};

fn fx(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("transcript")
        .join(rel)
}

/// Unix ms of `2026-03-01T10:00:00Z` plus `sec` seconds, the clock of the fixtures.
fn t(sec: i64) -> i64 {
    parse_rfc3339_ms("2026-03-01T10:00:00Z").unwrap() + sec * 1000
}

fn rows(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|l| match &l.label {
            Some(label) => format!("{} [{label}] {}", l.kind.code(), l.text),
            None => format!("{} {}", l.kind.code(), l.text),
        })
        .collect()
}

fn lines_of(provider: Provider, rel: &str) -> Vec<String> {
    rows(&normalize(provider, &fx(rel)).unwrap())
}

fn kinds(lines: &[Line]) -> String {
    let mut seen: Vec<char> = lines.iter().map(|l| l.kind.code()).collect();
    seen.sort_unstable();
    seen.dedup();
    seen.into_iter().collect()
}

fn state(
    provider: Provider,
    rel: &str,
    now: i64,
    mtime: i64,
) -> aivory_sessions::transcript::Derived {
    derive_state(provider, &fx(rel), now, mtime).unwrap()
}

fn fs_mtime(path: &Path) -> i64 {
    let d = fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from(d.as_millis()).unwrap()
}

const TURN_TIME: &str = "2026-03-01 10:00";

// ---- Claude: lines --------------------------------------------------------------------------

#[test]
fn claude_session_maps_every_line_kind() {
    let lines = normalize(Provider::Claude, &fx("claude/session.jsonl")).unwrap();
    assert_eq!(kinds(&lines), "aemopstux", "all nine kinds appear");
    let want: Vec<String> = [
        "x session start · C:\\synthetic\\app · main",
        &format!("x [TURN 1] {TURN_TIME}"),
        "u Fix the bug in the parser",
        "x model · claude-test-1",
        "a I will look at the parser.",
        "a Then I fix it.",
        "t [Bash] cargo test",
        "o line1",
        "o line2",
        "o line3",
        "o … 2 more lines",
        "t [Read] src/parser.rs",
        "o file ok",
        "t [Grep] fn parse",
        "e grep: boom",
        "t [Edit] src/parser.rs",
        "m let y = 2;",
        "p let y = 3;",
        "p let z = 4;",
        "t [Write] notes.txt",
        "p alpha",
        "p beta",
        "s [Explore] Find usages",
        "o Found 3 usages",
        "a Done. All tests pass.",
        &format!("x [TURN 2] {TURN_TIME}"),
        "u Now add a test",
        &format!("x [TURN 3] {TURN_TIME}"),
        "u also run clippy",
        "a On it.",
        "x interrupted",
        "x context compacted",
        "x model · claude-test-2",
        "a Resumed after compaction.",
        "e API Error: 500 overloaded",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(rows(&lines), want);
}

#[test]
fn claude_skips_meta_noise_and_malformed_lines() {
    // The fixture holds a truncated JSON line and a non-JSON line. Both are skipped.
    let all = lines_of(Provider::Claude, "claude/session.jsonl").join("\n");
    for noise in [
        "request noise",
        "hidden thoughts",
        "Caveat",
        "machine text",
        "task-notification",
        "Not a real answer",
        "Summary",
        "updated successfully",
        "File created",
        "\u{1b}",
    ] {
        assert!(!all.contains(noise), "noise leaked: {noise}");
    }
}

#[test]
fn line_wire_format_has_label_only_when_set() {
    let lines = normalize(Provider::Claude, &fx("claude/session.jsonl")).unwrap();
    let tool = lines.iter().find(|l| l.text == "cargo test").unwrap();
    let json = serde_json::to_value(tool).unwrap();
    assert_eq!(json["kind"], "tool");
    assert_eq!(json["label"], "Bash");
    let user = lines.iter().find(|l| l.kind.code() == 'u').unwrap();
    assert!(serde_json::to_value(user).unwrap().get("label").is_none());
}

// ---- Codex / Gemini: lines ------------------------------------------------------------------

#[test]
fn codex_session_maps_every_line_kind() {
    let lines = normalize(Provider::Codex, &fx("codex/session.jsonl")).unwrap();
    assert_eq!(kinds(&lines), "aemopstux");
    let want: Vec<String> = [
        "x session start · C:\\synthetic\\app · main",
        "x model · gpt-test-1",
        &format!("x [TURN 1] {TURN_TIME}"),
        "u List the files and fix the bug",
        "t [shell_command] ls -la",
        "o a",
        "o b",
        "o c",
        "o … 2 more lines",
        "t [shell_command] false",
        "e command failed",
        "t [apply_patch] src/a.rs",
        "m old line",
        "p new line",
        "s [explorer] Scan the repo",
        "o agent started",
        "t [web_search] rust serde",
        "a I listed the files.",
        "a The bug is fixed.",
        "e stream disconnected",
        "x turn aborted",
        "x model · gpt-test-2",
        &format!("x [TURN 2] {TURN_TIME}"),
        "u Try again",
        "x context compacted",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(rows(&lines), want);
}

#[test]
fn gemini_document_maps_every_line_kind() {
    let lines = normalize(Provider::Gemini, &fx("gemini/session.json")).unwrap();
    assert_eq!(kinds(&lines), "aemopstux");
    let want: Vec<String> = [
        &format!("x session start · {TURN_TIME}"),
        "x Using model gemini-test",
        &format!("x [TURN 1] {TURN_TIME}"),
        "u Fix the parser bug",
        "x model · gemini-test-1",
        "a Let me read the file.",
        "a Then I fix it.",
        "t [read_file] src/parser.rs",
        "o line1",
        "o line2",
        "o line3",
        "o … 2 more lines",
        "t [replace] src/parser.rs",
        "m let y = 2;",
        "p let y = 3;",
        "p let z = 4;",
        "t [write_file] notes.txt",
        "p alpha",
        "p beta",
        "t [run_shell_command] cargo test",
        "e exit code 101",
        "s [cli_help] How do I configure hooks?",
        "o See the docs.",
        "t [write_todos] ",
        "o Todos updated",
        "a All done. Tests are green.",
        &format!("x [TURN 2] {TURN_TIME}"),
        "u Now add a test",
        "x model · gemini-test-2",
        "a Switching model.",
        "e Quota exceeded",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(rows(&lines), want);
}

#[test]
fn gemini_jsonl_replaces_rewinds_and_skips_bad_lines() {
    let got = lines_of(Provider::Gemini, "gemini/session.jsonl");
    let want: Vec<String> = [
        &format!("x session start · {TURN_TIME}"),
        &format!("x [TURN 1] {TURN_TIME}"),
        "u First question",
        "x model · gemini-test-1",
        "a Checking.",
        "t [run_shell_command] ls",
        "o a.txt",
        &format!("x [TURN 2] {TURN_TIME}"),
        "u Second question",
        "a Answer to the second question.",
        &format!("x [TURN 3] {TURN_TIME}"),
        "u Third question again",
        "a Final answer.",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(got, want);
}

#[test]
fn invalid_gemini_document_is_an_error_and_missing_file_too() {
    let dir = std::env::temp_dir().join(format!("aivory-transcript-bad-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("session-bad.json");
    fs::write(&bad, "{ \"messages\": [ ").unwrap();
    assert!(normalize(Provider::Gemini, &bad).is_err());
    assert!(normalize(Provider::Claude, &dir.join("missing.jsonl")).is_err());
    fs::remove_dir_all(&dir).ok();
}

// ---- tail and history -----------------------------------------------------------------------

#[test]
fn tail_and_history_page_the_full_transcript() {
    let lines = normalize(Provider::Claude, &fx("claude/session.jsonl")).unwrap();
    let total = u32::try_from(lines.len()).unwrap();
    assert_eq!(tail(&lines, 3), &lines[lines.len() - 3..]);
    assert_eq!(tail(&lines, 10_000).len(), lines.len());
    assert!(tail(&lines, 0).is_empty());

    let first = history("sess", None, &lines, 0, 10);
    assert_eq!(
        (first.start, first.total, first.lines.len()),
        (0, total, 10)
    );
    assert_eq!(first.session, "sess");
    assert_eq!(first.subagent, None);

    // Page through the history and get the same lines back.
    let mut joined = Vec::new();
    let mut start = 0;
    loop {
        let page = history("sess", Some("a1"), &lines, start, 7);
        assert_eq!(page.total, total);
        assert_eq!(page.start, start);
        assert_eq!(page.subagent.as_deref(), Some("a1"));
        if page.lines.is_empty() {
            break;
        }
        start += u32::try_from(page.lines.len()).unwrap();
        joined.extend(page.lines);
    }
    assert_eq!(joined, lines);

    let past = history("sess", None, &lines, total + 50, 5);
    assert_eq!((past.start, past.total), (total, total));
    assert!(past.lines.is_empty());
    let last = history("sess", None, &lines, total - 2, 100);
    assert_eq!(last.lines.len(), 2);
    assert_eq!(history("s", None, &lines, 3, 0).lines.len(), 0);
}

// ---- state: Claude --------------------------------------------------------------------------

#[test]
fn claude_error_state_wins_over_everything() {
    let d = state(Provider::Claude, "claude/state_error.jsonl", t(5), t(4));
    assert_eq!(d.state, SessionState::Error);
    assert_eq!(d.since, t(4));
    let ask = d.ask.unwrap();
    assert_eq!(ask.kind, AskKind::Error);
    assert!(
        ask.text.starts_with("API Error: rate limited"),
        "{}",
        ask.text
    );
    // An error stays an error when the file is old.
    let old = state(
        Provider::Claude,
        "claude/state_error.jsonl",
        t(4) + 3 * IDLE_WINDOW_MS,
        t(4),
    );
    assert_eq!(old.state, SessionState::Error);
    // The API error entry of the full fixture is the last entry too.
    let full = state(Provider::Claude, "claude/session.jsonl", t(30), t(29));
    assert_eq!((full.state, full.since), (SessionState::Error, t(29)));
}

#[test]
fn claude_permission_question_and_plan_asks() {
    let d = state(
        Provider::Claude,
        "claude/state_permission.jsonl",
        t(4),
        t(3),
    );
    assert_eq!(d.state, SessionState::NeedsYou);
    assert_eq!(d.since, t(3));
    let ask = d.ask.unwrap();
    assert_eq!(ask.kind, AskKind::Permission);
    assert_eq!(ask.text, "rm -rf build");
    assert_eq!(ask.reason.as_deref(), Some("Remove build output"));
    assert_eq!(ask.options, ["Allow", "Deny"]);
    assert_eq!(ask.id, "t1");
    // The ask outranks the age rules.
    let old = state(
        Provider::Claude,
        "claude/state_permission.jsonl",
        t(3) + 5 * IDLE_WINDOW_MS,
        t(3),
    );
    assert_eq!(old.state, SessionState::NeedsYou);

    let q = state(Provider::Claude, "claude/state_question.jsonl", t(3), t(2))
        .ask
        .unwrap();
    assert_eq!(q.kind, AskKind::Question);
    assert_eq!(q.text, "Which database?");
    assert_eq!(q.options, ["Postgres", "SQLite"]);

    let p = state(Provider::Claude, "claude/state_plan.jsonl", t(3), t(2))
        .ask
        .unwrap();
    assert_eq!(p.kind, AskKind::Plan);
    assert_eq!(p.text, "Add caching");
    assert_eq!(p.steps, ["Add a cache struct", "Wire it in", "Write tests"]);
}

#[test]
fn claude_working_idle_and_parked_by_age() {
    let last = t(3);
    // The last entry is a tool result: the turn did not end.
    for rel in [
        "claude/state_running.jsonl",
        "claude/state_prompt.jsonl",
        "claude/state_thinking.jsonl",
    ] {
        let mtime = match rel {
            "claude/state_prompt.jsonl" => t(1),
            "claude/state_thinking.jsonl" => t(2),
            _ => last,
        };
        let fresh = state(Provider::Claude, rel, mtime + WORKING_WINDOW_MS - 1, mtime);
        assert_eq!(fresh.state, SessionState::Working, "{rel}");
        assert!(fresh.ask.is_none());
        assert_eq!(fresh.since, mtime);
        let stale = state(Provider::Claude, rel, mtime + WORKING_WINDOW_MS, mtime);
        assert_eq!(stale.state, SessionState::Parked, "{rel}");
    }
    // The turn ended with a final text.
    let mtime = t(2);
    for age in [0, WORKING_WINDOW_MS + 1, IDLE_WINDOW_MS - 1] {
        let d = state(
            Provider::Claude,
            "claude/state_done.jsonl",
            mtime + age,
            mtime,
        );
        assert_eq!(d.state, SessionState::Idle, "age {age}");
    }
    let old = state(
        Provider::Claude,
        "claude/state_done.jsonl",
        mtime + IDLE_WINDOW_MS,
        mtime,
    );
    assert_eq!(old.state, SessionState::Parked);
    // The user stopped the turn: it ended.
    let stopped = state(
        Provider::Claude,
        "claude/state_interrupted.jsonl",
        t(3) + 1000,
        t(3),
    );
    assert_eq!(stopped.state, SessionState::Idle);
}

#[test]
fn claude_permission_modes_skip_the_ask() {
    let d = state(Provider::Claude, "claude/state_bypass.jsonl", t(3), t(2));
    assert_eq!(d.state, SessionState::Working);
    assert!(d.ask.is_none());
    // acceptEdits runs the Edit call, but the Bash call still asks.
    let a = state(
        Provider::Claude,
        "claude/state_accept_edits.jsonl",
        t(4),
        t(3),
    );
    assert_eq!(a.state, SessionState::NeedsYou);
    assert_eq!(a.ask.unwrap().text, "cargo fmt");
}

#[test]
fn claude_auto_mode_on_the_user_entry_skips_the_ask() {
    // The mode sits on the user entry, not on a `permission-mode` entry; a running tool of an
    // `auto` session is work, not a permission request.
    let d = state(Provider::Claude, "claude/state_auto_user.jsonl", t(3), t(2));
    assert_eq!(d.state, SessionState::Working);
    assert!(d.ask.is_none());
}

#[test]
fn claude_waiting_on_a_subagent_is_not_an_ask() {
    let none = state(Provider::Claude, "claude/waiting_nosub.jsonl", t(3), t(2));
    assert_eq!(none.state, SessionState::Working);
    assert!(none.ask.is_none());
    assert_eq!(
        state(
            Provider::Claude,
            "claude/waiting_nosub.jsonl",
            t(2) + WORKING_WINDOW_MS,
            t(2)
        )
        .state,
        SessionState::Parked
    );

    // The session file is old, but the sub-agent file changed a moment ago.
    let sub_mtime = fs_mtime(&fx("claude/waiting_subagent/subagents/agent-w1.jsonl"));
    let busy = state(
        Provider::Claude,
        "claude/waiting_subagent.jsonl",
        sub_mtime + 5_000,
        t(2),
    );
    assert_eq!(busy.state, SessionState::Working);
    let quiet = state(
        Provider::Claude,
        "claude/waiting_subagent.jsonl",
        sub_mtime + 10 * 60_000,
        t(2),
    );
    assert_eq!(quiet.state, SessionState::Parked);
}

// ---- state: Codex ---------------------------------------------------------------------------

#[test]
fn codex_states() {
    let e = state(Provider::Codex, "codex/state_error.jsonl", t(6), t(5));
    assert_eq!(e.state, SessionState::Error, "an error that ended the turn");
    assert_eq!(e.ask.unwrap().text, "unexpected status 500");
    assert_eq!(e.since, t(5));

    let p = state(Provider::Codex, "codex/state_permission.jsonl", t(5), t(4));
    assert_eq!(p.state, SessionState::NeedsYou);
    let ask = p.ask.unwrap();
    assert_eq!(
        (ask.kind, ask.text.as_str()),
        (AskKind::Permission, "Remove-Item -Recurse build")
    );

    let never = state(Provider::Codex, "codex/state_never.jsonl", t(5), t(4));
    assert_eq!(
        never.state,
        SessionState::Working,
        "approval policy never asks nothing"
    );

    let q = state(Provider::Codex, "codex/state_question.jsonl", t(5), t(4))
        .ask
        .unwrap();
    assert_eq!(
        (q.kind, q.text.as_str()),
        (AskKind::Question, "Which store?")
    );
    assert_eq!(q.options, ["Postgres", "SQLite"]);

    let plan = state(Provider::Codex, "codex/state_plan.jsonl", t(6), t(5));
    assert_eq!(plan.state, SessionState::NeedsYou);
    let ask = plan.ask.unwrap();
    assert_eq!((ask.kind, ask.text.as_str()), (AskKind::Plan, "Cache plan"));
    assert_eq!(ask.steps, ["Add a cache", "Wire it in"]);

    let run = state(
        Provider::Codex,
        "codex/state_running.jsonl",
        t(5) + 10_000,
        t(5),
    );
    assert_eq!(run.state, SessionState::Working);
    let stale = state(
        Provider::Codex,
        "codex/state_running.jsonl",
        t(5) + WORKING_WINDOW_MS,
        t(5),
    );
    assert_eq!(stale.state, SessionState::Parked);

    let done = state(
        Provider::Codex,
        "codex/state_done.jsonl",
        t(5) + 60_000,
        t(5),
    );
    assert_eq!(done.state, SessionState::Idle);
    assert_eq!(done.since, t(5));
    let old = state(
        Provider::Codex,
        "codex/state_done.jsonl",
        t(5) + IDLE_WINDOW_MS,
        t(5),
    );
    assert_eq!(old.state, SessionState::Parked);

    let aborted = state(
        Provider::Codex,
        "codex/state_aborted.jsonl",
        t(5) + 1000,
        t(5),
    );
    assert_eq!(aborted.state, SessionState::Idle, "an aborted turn is over");
}

// ---- state: Gemini --------------------------------------------------------------------------

#[test]
fn gemini_states() {
    let e = state(Provider::Gemini, "gemini/state_error.json", t(3), t(2));
    assert_eq!(e.state, SessionState::Error);
    assert!(e.ask.unwrap().text.starts_with("Quota exceeded"));
    assert_eq!(e.since, t(2));

    let p = state(Provider::Gemini, "gemini/state_permission.json", t(3), t(2));
    assert_eq!(p.state, SessionState::NeedsYou);
    let ask = p.ask.unwrap();
    assert_eq!(
        (ask.kind, ask.text.as_str()),
        (AskKind::Permission, "git push")
    );
    assert_eq!(ask.reason.as_deref(), Some("Push"));

    let q = state(Provider::Gemini, "gemini/state_question.json", t(3), t(2))
        .ask
        .unwrap();
    assert_eq!(
        (q.kind, q.text.as_str()),
        (AskKind::Question, "Which store?")
    );
    assert_eq!(q.options, ["Postgres", "SQLite"]);

    let plan = state(Provider::Gemini, "gemini/state_plan.json", t(3), t(2))
        .ask
        .unwrap();
    assert_eq!(
        (plan.kind, plan.text.as_str()),
        (AskKind::Plan, "Cache plan")
    );
    assert_eq!(plan.steps, ["Add a cache", "Wire it in"]);

    for rel in [
        "gemini/state_running.json",
        "gemini/state_tool_done.json",
        "gemini/state_subagent.json",
    ] {
        let mtime = t(2);
        let fresh = state(Provider::Gemini, rel, mtime + 1000, mtime);
        assert_eq!(fresh.state, SessionState::Working, "{rel}");
        assert!(fresh.ask.is_none(), "{rel}");
        assert_eq!(
            state(Provider::Gemini, rel, mtime + WORKING_WINDOW_MS, mtime).state,
            SessionState::Parked,
            "{rel}"
        );
    }

    let done = state(
        Provider::Gemini,
        "gemini/state_done.json",
        t(2) + 120_000,
        t(2),
    );
    assert_eq!(done.state, SessionState::Idle);
    // The jsonl format ends with a final answer too.
    let jsonl = state(Provider::Gemini, "gemini/session.jsonl", t(9) + 1000, t(9));
    assert_eq!((jsonl.state, jsonl.since), (SessionState::Idle, t(9)));
}

// ---- sub-agents -----------------------------------------------------------------------------

fn tree_session() -> PathBuf {
    fx("claude/projects/C--synthetic-app/aaaaaaaa-0000-4000-8000-000000000001.jsonl")
}

#[test]
fn claude_subagents_come_from_the_subagents_folder() {
    let session = tree_session();
    let dir = session.with_extension("").join("subagents");
    let newest = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| fs_mtime(&e.path()))
        .max()
        .unwrap();

    let subs = subagents(&session, newest + 5_000);
    let summary: Vec<(&str, &str, SessionState, &str)> = subs
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_str(), s.state, s.task.as_str()))
        .collect();
    // Order of the start times: a2, a3, a1, a4.
    assert_eq!(
        summary,
        [
            (
                "a2",
                "general-purpose",
                SessionState::Working,
                "Write tests"
            ),
            ("a3", "Plan", SessionState::NeedsYou, "Plan the change"),
            ("a1", "Explore", SessionState::Done, "Find usages"),
            ("a4", "Explore", SessionState::Error, "Check docs"),
        ]
    );

    let later = subagents(&session, newest + 10 * 60_000);
    assert_eq!(
        later[0].state,
        SessionState::Parked,
        "a quiet sub-agent that did not finish"
    );
    assert_eq!(later[2].state, SessionState::Done);
}

#[test]
fn claude_subagent_transcript_normalizes_like_a_session() {
    let session = tree_session();
    let path = subagent_path(&session, "a1").expect("sub-agent file");
    assert!(path.ends_with("agent-a1.jsonl"));
    let got = rows(&normalize(Provider::Claude, &path).unwrap());
    let want: Vec<String> = [
        "x session start · C:\\synthetic\\app · main",
        &format!("x [TURN 1] {TURN_TIME}"),
        "u Find all usages of parse",
        "x model · claude-test-1",
        "t [Grep] parse",
        "o src/a.rs:1: parse",
        "o src/b.rs:2: parse",
        "a Found 2 usages.",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(got, want);
    // The state of a sub-agent file works through derive_state too.
    let d = derive_state(Provider::Claude, &path, t(40), t(33)).unwrap();
    assert_eq!((d.state, d.since), (SessionState::Idle, t(33)));

    assert!(subagent_path(&session, "missing").is_none());
    assert!(subagent_path(&session, "..\\..\\x").is_none());
    assert!(subagent_path(&session, "").is_none());
}

#[test]
fn claude_old_layout_subagents_are_matched_by_session_id() {
    let session = fx("claude/projects/C--synthetic-app/bbbbbbbb-0000-4000-8000-000000000002.jsonl");
    let subs = subagents(&session, t(100));
    assert_eq!(
        subs.len(),
        1,
        "the agent file of another session is left out"
    );
    assert_eq!(subs[0].id, "old1");
    assert_eq!(subs[0].name, "agent", "no meta file: default name");
    assert!(subagent_path(&session, "old1").is_some());
    assert!(
        subagent_path(&session, "other").is_some(),
        "the file exists, but it is not listed"
    );
    let none = fx("claude/session.jsonl");
    assert!(subagents(&none, t(100)).is_empty());
}

// ---- large and odd files --------------------------------------------------------------------

struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aivory-transcript-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir.join(name))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        fs::remove_file(&self.0).ok();
    }
}

fn claude_user(ts: i64, text: &str) -> String {
    format!(
        r#"{{"type":"user","timestamp":"2026-03-01T10:{:02}:{:02}.000Z","message":{{"role":"user","content":"{text}"}}}}"#,
        ts / 60,
        ts % 60
    )
}

#[test]
fn state_reads_the_tail_even_when_the_last_entry_is_far_from_the_end() {
    let file = TempFile::new("far.jsonl");
    let mut out = fs::File::create(&file.0).unwrap();
    writeln!(out, "{}", claude_user(1, "start")).unwrap();
    writeln!(
        out,
        r#"{{"type":"assistant","timestamp":"2026-03-01T10:00:02.000Z","message":{{"role":"assistant","model":"m","stop_reason":"end_turn","content":[{{"type":"text","text":"Final."}}]}}}}"#
    )
    .unwrap();
    // 600 KB of bookkeeping lines after the last conversation entry. The first tail window
    // of 256 KB sees none of the conversation.
    let noise = format!(
        r#"{{"type":"api-request-blob","message":{{"role":"system","content":[{{"type":"text","text":"{}"}}]}}}}"#,
        "n".repeat(1000)
    );
    for _ in 0..600 {
        writeln!(out, "{noise}").unwrap();
    }
    drop(out);
    let d = derive_state(Provider::Claude, &file.0, t(10), t(9)).unwrap();
    assert_eq!((d.state, d.since), (SessionState::Idle, t(2)));
}

#[test]
fn a_line_longer_than_the_first_window_does_not_break_state_or_normalize() {
    let file = TempFile::new("big.jsonl");
    let mut out = fs::File::create(&file.0).unwrap();
    writeln!(out, "{}", claude_user(1, "Read the log")).unwrap();
    writeln!(
        out,
        r#"{{"type":"assistant","timestamp":"2026-03-01T10:00:02.000Z","message":{{"role":"assistant","model":"m","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"r1","name":"Read","input":{{"file_path":"big.log"}}}}]}}}}"#
    )
    .unwrap();
    writeln!(
        out,
        r#"{{"type":"user","timestamp":"2026-03-01T10:00:03.000Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"r1","content":"{}"}}]}}}}"#,
        "x".repeat(300_000)
    )
    .unwrap();
    writeln!(
        out,
        r#"{{"type":"assistant","timestamp":"2026-03-01T10:00:04.000Z","message":{{"role":"assistant","model":"m","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"b1","name":"Bash","input":{{"command":"tail big.log"}}}}]}}}}"#
    )
    .unwrap();
    drop(out);
    let d = derive_state(Provider::Claude, &file.0, t(5), t(4)).unwrap();
    assert_eq!(d.state, SessionState::NeedsYou);
    assert_eq!(d.ask.unwrap().text, "tail big.log");
    assert_eq!(d.since, t(4));

    let lines = normalize(Provider::Claude, &file.0).unwrap();
    let out_lines: Vec<&Line> = lines.iter().filter(|l| l.kind.code() == 'o').collect();
    assert_eq!(out_lines.len(), 1);
    assert!(
        out_lines[0].text.chars().count() <= 240,
        "long output rows are cut"
    );
}

#[test]
fn crlf_and_missing_final_newline_are_fine() {
    let file = TempFile::new("crlf.jsonl");
    let text = format!(
        "{}\r\n{}",
        claude_user(1, "Hello"),
        r#"{"type":"assistant","timestamp":"2026-03-01T10:00:02.000Z","message":{"role":"assistant","model":"m","stop_reason":"end_turn","content":[{"type":"text","text":"Hi"}]}}"#
    );
    fs::write(&file.0, text).unwrap();
    let lines = normalize(Provider::Claude, &file.0).unwrap();
    assert_eq!(
        rows(&lines).last().map(String::as_str),
        Some("a Hi"),
        "the last line without a line break is read"
    );
    assert_eq!(
        derive_state(Provider::Claude, &file.0, t(3), t(2))
            .unwrap()
            .state,
        SessionState::Idle
    );
}

#[test]
fn empty_and_unreadable_files_give_parked_or_an_error() {
    let file = TempFile::new("empty.jsonl");
    fs::write(&file.0, "").unwrap();
    for provider in Provider::ALL {
        assert!(normalize(provider, &file.0).map_or(true, |l| l.len() <= 1));
        let d = derive_state(provider, &file.0, t(10), t(5));
        // An empty Gemini `.jsonl` or Claude file has no entry: parked, since = mtime.
        let d = d.unwrap();
        assert_eq!((d.state, d.since), (SessionState::Parked, t(5)));
    }
    assert!(
        derive_state(
            Provider::Claude,
            &file.0.with_file_name("nope.jsonl"),
            t(1),
            t(1)
        )
        .is_err()
    );
}
