//! Local coordination through the real daemon binary and two fake CLIs (`fake-agent` of
//! `aivory-agents`, as `codex` and `gemini`) in one scratch git repo: claims, the conflict
//! radar with a trial merge, the conflict item of the inbox, the mail in front of the next
//! input, and the block that denies an edit ask. Synthetic data only: the repo, the home
//! folder, and the cache folder are scratch folders.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aivory_agents::mcp::McpServer;
use aivory_coord::{Ping, TestState};
use aivoryd::coord::{CoordTools, Effect, hub_for};
use aivoryd::ledger::{BaselineJob, Job, LedgerWorker, TurnJob, last_turn, with_ledger};

use aivory_protocol::coord::{ClaimList, ClaimState, MailList, PingKind, RadarList};
use aivory_protocol::{InboxKind, InboxList, Pane, SessionState, WarRoom};
use aivory_vcs::{Ledger, Turn};
use aivoryd::{Endpoint, client};
use rusqlite::Connection;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(90);
/// The native ids that the fake CLIs give to their sessions.
const CODEX: &str = "fake-thread";
const GEMINI: &str = "fake-gemini";
const NATIVE_CODEX: &str = "99999999-9999-4999-8999-999999999997";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aivoryd-coord-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn db(&self) -> PathBuf {
        self.0.join("cache").join("aivory.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Daemon {
    child: Child,
    endpoint: Endpoint,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Build the fake agent next to the test binary, in the same target folder and profile.
fn build_fake_agent() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
    let target = profile_dir.parent().unwrap();
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args([
        "build",
        "-p",
        "aivory-agents",
        "--bin",
        "fake-agent",
        "--target-dir",
    ])
    .arg(target)
    .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
    if profile_dir.file_name().is_some_and(|n| n == "release") {
        cmd.arg("--release");
    }
    let status = cmd.status().expect("run cargo");
    assert!(status.success(), "cargo cannot build the fake agent");
    let fake = profile_dir.join(format!("fake-agent{}", std::env::consts::EXE_SUFFIX));
    assert!(fake.is_file(), "no fake agent at {}", fake.display());
    fake
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stderr(Stdio::null())
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A scratch git repo with one commit (`a.txt`), a scratch home with one synthetic native
/// Codex session in the repo (so that the scanner shows the codebase), and a folder with the
/// fake agent as `codex` and `gemini`.
fn setup(scratch: &Scratch, fake: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let root = &scratch.0;
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_out(&repo, &["init", "-q"]);
    fs::write(repo.join("a.txt"), "x\n").unwrap();
    git_out(&repo, &["add", "a.txt"]);
    git_out(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "first",
        ],
    );

    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["codex", "gemini"] {
        fs::copy(
            fake,
            bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
        )
        .unwrap();
    }

    let home = root.join("home");
    let dir = home.join(".codex/sessions/2026/10/22");
    fs::create_dir_all(&dir).unwrap();
    let lines = [
        json!({"timestamp": "2026-10-22T10:00:00.000Z", "type": "session_meta", "payload": {
            "id": NATIVE_CODEX, "timestamp": "2026-10-22T10:00:00.000Z",
            "cwd": repo.display().to_string(), "originator": "codex_exec",
            "cli_version": "0.0.1-test", "source": "exec", "model_provider": "openai"}}),
        json!({"timestamp": "2026-10-22T10:00:01.000Z", "type": "event_msg", "payload": {
            "type": "user_message", "message": "Synthetic prompt"}}),
        json!({"timestamp": "2026-10-22T10:00:02.000Z", "type": "event_msg", "payload": {
            "type": "agent_message", "message": "Synthetic answer"}}),
    ];
    let text: Vec<String> = lines.iter().map(Value::to_string).collect();
    fs::write(
        dir.join(format!("rollout-2026-10-22T10-00-00-{NATIVE_CODEX}.jsonl")),
        text.join("\n") + "\n",
    )
    .unwrap();
    (repo, home, bin)
}

fn start_daemon(name: &str, scratch: &Scratch, repo: &Path, home: &Path, bin: &Path) -> Daemon {
    let name = format!("coord-{name}-{}", std::process::id());
    let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_aivoryd"))
        .args(["serve", "--endpoint", &name, "--scope"])
        .arg(repo)
        .env("PATH", path)
        .env("USERPROFILE", home)
        .env("HOME", home)
        .env("AIVORY_CACHE_DIR", scratch.0.join("cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start aivoryd");
    Daemon {
        child,
        endpoint: Endpoint::new(&name).unwrap(),
    }
}

/// Something that has an endpoint: the daemon of a test, or a demo server.
trait Ep {
    fn endpoint(&self) -> &Endpoint;
}

impl Ep for Daemon {
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl Ep for Endpoint {
    fn endpoint(&self) -> &Endpoint {
        self
    }
}

fn call(d: &impl Ep, method: &str, params: Value) -> Value {
    client::call(d.endpoint(), method, params, &mut |_| {}).expect("call the daemon")
}

fn ok(d: &impl Ep, method: &str, params: Value) -> Value {
    let r = call(d, method, params);
    assert!(r.get("error").is_none(), "{method} failed: {r}");
    r["result"].clone()
}

fn typed<T: serde::de::DeserializeOwned>(d: &impl Ep, method: &str, params: Value) -> T {
    serde_json::from_value(ok(d, method, params)).unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn snapshot(d: &Daemon) -> Option<WarRoom> {
    let r = client::call(&d.endpoint, "warroom/snapshot", Value::Null, &mut |_| {}).ok()?;
    serde_json::from_value(r.get("result")?.clone()).ok()
}

/// Poll `pick` until it returns a value.
fn wait_until<T>(what: &str, pick: impl Fn() -> Option<T>) -> T {
    let begin = Instant::now();
    loop {
        if let Some(v) = pick() {
            return v;
        }
        assert!(begin.elapsed() < WAIT, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for<T>(d: &Daemon, what: &str, pick: impl Fn(&WarRoom) -> Option<T>) -> T {
    wait_until(what, || snapshot(d).and_then(|w| pick(&w)))
}

fn pane_of(w: &WarRoom, id: &str) -> Option<Pane> {
    w.sessions.iter().find(|p| p.id == id).cloned()
}

fn codebase_of(d: &Daemon) -> String {
    wait_for(d, "the codebase", |w| {
        w.codebases.first().map(|c| c.id.clone())
    })
}

fn new_session(d: &Daemon, codebase: &str, provider: &str, prompt: &str) -> String {
    let r = ok(
        d,
        "session/new",
        json!({
            "codebase": codebase, "prompt": prompt,
            "composer": {"mode": "edit", "provider": provider, "model": "", "effort": "high"},
        }),
    );
    r["id"].as_str().unwrap().to_string()
}

fn turns(scratch: &Scratch, session: &str) -> Option<Vec<Turn>> {
    let conn = Connection::open(scratch.db()).ok()?;
    conn.busy_timeout(Duration::from_secs(5)).ok()?;
    let has_table: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'turn'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    if has_table == 0 {
        return Some(Vec::new());
    }
    Ledger::new(&conn).ok()?.list_turns(session).ok()
}

fn wait_for_turn(scratch: &Scratch, session: &str, n: u32) -> Vec<Turn> {
    wait_until(&format!("turn {n} of {session} in the ledger"), || {
        turns(scratch, session).filter(|t| t.iter().any(|x| x.n == n))
    })
}

fn is_idle(w: &WarRoom, id: &str) -> Option<Pane> {
    pane_of(w, id).filter(|p| p.state == SessionState::Idle)
}

/// A slow turn: the test changes `a.txt` while the fake waits, then the pause ends the turn.
/// The turn snapshot holds the change.
fn edit_in_a_turn(
    d: &Daemon,
    scratch: &Scratch,
    repo: &Path,
    codebase: &str,
    provider: &str,
    native: &str,
    content: &str,
) -> String {
    let run = new_session(d, codebase, provider, "slow");
    wait_for(d, "the slow turn", |w| {
        pane_of(w, &run).filter(|p| p.tail.iter().any(|l| l.text == "working"))
    });
    let t = wait_for_turn(scratch, native, 0);
    assert!(t[0].snapshot_ref.is_some(), "the baseline has a snapshot");
    fs::write(repo.join("a.txt"), content).unwrap();
    ok(d, "session/pause", json!({"id": run}));
    wait_for(d, "the end of the turn", |w| is_idle(w, &run));
    wait_for_turn(scratch, native, 1);
    run
}

#[test]
fn two_sessions_that_edit_one_file_get_a_claim_overlap_a_radar_hit_and_a_conflict_item() {
    let fake = build_fake_agent();
    let scratch = Scratch::new("radar");
    let (repo, home, bin) = setup(&scratch, &fake);
    let d = start_daemon("radar", &scratch, &repo, &home, &bin);
    let codebase = codebase_of(&d);

    let a = edit_in_a_turn(&d, &scratch, &repo, &codebase, "codex", CODEX, "from A\n");
    let b = edit_in_a_turn(&d, &scratch, &repo, &codebase, "gemini", GEMINI, "from B\n");

    // Both sessions hold an editing claim on the file: a claim overlap.
    let claims = wait_until("two claims on a.txt", || {
        let list: ClaimList = typed(&d, "claims/list", json!({"codebase": codebase}));
        let on_file: Vec<_> = list
            .claims
            .iter()
            .filter(|c| c.key == "path:a.txt" && c.state == ClaimState::Editing)
            .cloned()
            .collect();
        (on_file.len() == 2).then_some(on_file)
    });
    let mut owners: Vec<&str> = claims.iter().map(|c| c.session.as_str()).collect();
    owners.sort_unstable();
    let mut want = [a.as_str(), b.as_str()];
    want.sort_unstable();
    assert_eq!(owners, want, "the claims use the ids of the war room");
    assert!(claims.iter().all(|c| c.path.as_deref() == Some("a.txt")));
    assert!(claims.iter().all(|c| c.lease_s > 500 && c.lease_s <= 600));
    let only_a: ClaimList = typed(&d, "claims/list", json!({"session": a}));
    assert!(only_a.claims.iter().all(|c| c.session == a));
    let other: ClaimList = typed(&d, "claims/list", json!({"codebase": "nope"}));
    assert!(other.claims.is_empty());

    // The radar: tier 1 (same path), and the trial merge finds the text conflict: 1.0.
    let radar = wait_until("the radar hit", || {
        let r: RadarList = typed(&d, "radar/list", json!({"codebase": codebase}));
        (!r.hits.is_empty()).then_some(r)
    });
    assert_eq!(radar.hits.len(), 1);
    let hit = &radar.hits[0];
    assert_eq!((hit.tier, hit.score), (1, 1.0));
    assert!(hit.trial, "a trial merge decided: {:?}", hit.trial_error);
    assert_eq!(hit.conflict_files, ["a.txt"]);
    assert_eq!(hit.keys, ["path:a.txt"]);
    assert_eq!(hit.codebase, codebase);
    let mut ids = [hit.a.as_str(), hit.b.as_str()];
    ids.sort_unstable();
    assert_eq!(ids, want);
    assert_eq!(radar.sessions.len(), 2);

    // The conflict item of the inbox has the hunks of the trial merge.
    let inbox = wait_until("the conflict item", || {
        let list: InboxList = typed(&d, "inbox/list", json!({"filter": "conflicts"}));
        (!list.items.is_empty()).then_some(list)
    });
    let item = &inbox.items[0];
    assert_eq!(item.kind, InboxKind::Conflict);
    assert_eq!(item.agents.len(), 2);
    assert_eq!(item.files, ["a.txt"]);
    assert_eq!(item.sides.len(), 2);
    let side_text = |i: usize| {
        item.sides[i]
            .lines
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (s0, s1) = (side_text(0), side_text(1));
    assert!(s0.contains("from A") || s0.contains("from B"), "{s0}");
    assert!(s1.contains("from A") || s1.contains("from B"), "{s1}");
    assert_ne!(s0, s1);
    // The count of open items follows the conflict, so that the push worker reads the inbox.
    let w = snapshot(&d).unwrap();
    assert!(w.inbox_open >= 1);

    // The radar sent an FYI ping to both sessions.
    let mail_a: MailList = typed(&d, "mail/list", json!({"session": a}));
    assert!(
        mail_a.items.iter().any(|m| m.kind == PingKind::Fyi
            && m.text.contains("also changes path:a.txt")
            && m.delivered_at.is_none()),
        "{mail_a:?}"
    );
    let mail_b: MailList = typed(&d, "mail/list", json!({"session": b}));
    assert_eq!(mail_b.items.len(), 1);

    // The Files tab shows the claim. (The tree lists the tracked file a.txt.)
    let repo_view = ok(&d, "repos/get", json!({"id": codebase}));
    let file = repo_view["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "a.txt")
        .unwrap_or_else(|| panic!("no a.txt in {repo_view}"))
        .clone();
    assert!(
        file["claimedBy"] == a.as_str() || file["claimedBy"] == b.as_str(),
        "{file}"
    );

    // The mail arrives in front of the next input: the fake gets "> ..." as the first word and
    // does not know the command (it would answer "two" to a plain "say two").
    ok(&d, "session/send", json!({"id": a, "text": "say two"}));
    let pane = wait_for(&d, "the second turn of A", |w| {
        let p = is_idle(w, &a)?;
        p.tail
            .iter()
            .any(|l| l.text == "unknown command")
            .then_some(p)
    });
    assert!(!pane.tail.iter().any(|l| l.text == "two"));
    let mail_a: MailList = typed(&d, "mail/list", json!({"session": a}));
    assert!(
        mail_a.items.iter().all(|m| m.delivered_at.is_some()),
        "the mail is delivered: {mail_a:?}"
    );
    // Nothing is left to deliver: the next input goes out as it is.
    ok(&d, "session/send", json!({"id": a, "text": "say three"}));
    wait_for(&d, "the third turn of A", |w| {
        is_idle(w, &a).filter(|p| p.tail.iter().any(|l| l.text == "three"))
    });

    // "Dismiss" closes the item.
    let done = ok(
        &d,
        "inbox/answer",
        json!({"id": item.id, "option": "Dismiss"}),
    );
    assert_eq!(done["result"], "dismissed");
    let after: InboxList = typed(&d, "inbox/list", json!({"filter": "conflicts"}));
    assert!(after.items.is_empty());
}

#[test]
fn a_block_denies_the_edit_ask_until_a_human_overrides_it() {
    let fake = build_fake_agent();
    let scratch = Scratch::new("block");
    let (repo, home, bin) = setup(&scratch, &fake);
    let d = start_daemon("block", &scratch, &repo, &home, &bin);
    let codebase = codebase_of(&d);

    let run = new_session(&d, &codebase, "codex", "say hi");
    wait_for(&d, "the first turn", |w| {
        is_idle(w, &run).filter(|p| p.tail.iter().any(|l| l.text == "hi"))
    });
    // The first message of a session registers it; a ping to an unknown session fails.
    let r = call(
        &d,
        "radar/ping",
        json!({"to": "run-99", "kind": "fyi", "text": "x"}),
    );
    assert!(r.get("error").is_some(), "{r}");
    let r = call(
        &d,
        "radar/ping",
        json!({"to": run, "kind": "block", "text": "stop"}),
    );
    assert!(
        r["error"]["message"].as_str().unwrap().contains("key"),
        "{r}"
    );

    // A human blocks the edits of the session on a.txt. A block is a ping too: it arrives at
    // the next turn boundary, and the pause holds from now on.
    ok(
        &d,
        "radar/ping",
        json!({"to": run, "kind": "block", "text": "Wait until the refactor lands.", "keys": ["a.txt"]}),
    );
    let mail: MailList = typed(&d, "mail/list", json!({"session": run}));
    assert_eq!(mail.items.len(), 1);
    assert_eq!(mail.items[0].kind, PingKind::Block);
    assert_eq!(mail.items[0].key, "path:a.txt");
    assert_eq!(mail.items[0].from, "you");

    // The block arrives as the prefix of this input (the fake does not know the command).
    ok(&d, "session/send", json!({"id": run, "text": "say two"}));
    wait_for(&d, "the second turn", |w| {
        is_idle(w, &run).filter(|p| p.tail.iter().any(|l| l.text == "unknown command"))
    });

    // The agent asks to edit a.txt: the daemon denies at once, and the pane never asks the human.
    ok(&d, "session/send", json!({"id": run, "text": "edit a.txt"}));
    let pane = wait_for(&d, "the denied edit", |w| {
        let p = is_idle(w, &run)?;
        p.tail
            .iter()
            .any(|l| l.text.starts_with("edit paused by a block"))
            .then_some(p)
    });
    assert!(pane.ask.is_none());
    let inbox: InboxList = typed(&d, "inbox/list", json!({}));
    assert!(inbox.items.iter().all(|i| i.session != run), "{inbox:?}");

    // A command that does not name the file is not an edit of it: it still asks the human.
    ok(&d, "session/send", json!({"id": run, "text": "edit b.txt"}));
    wait_for(&d, "the ask for b.txt", |w| {
        pane_of(w, &run).filter(|p| p.state == SessionState::NeedsYou)
    });
    let ask = pane_of(&snapshot(&d).unwrap(), &run).unwrap().ask.unwrap();
    ok(
        &d,
        "session/answer",
        json!({"id": run, "ask": ask.id, "option": "Deny"}),
    );
    wait_for(&d, "the end of the turn", |w| is_idle(w, &run));

    // The human overrides: the same edit asks the human again.
    let freed = ok(
        &d,
        "block/override",
        json!({"session": run, "keys": ["a.txt"]}),
    );
    assert_eq!(freed["resolved"], 1);
    ok(&d, "session/send", json!({"id": run, "text": "edit a.txt"}));
    wait_for(&d, "the ask for a.txt after the override", |w| {
        pane_of(w, &run).filter(|p| p.state == SessionState::NeedsYou)
    });
    let mail: MailList = typed(&d, "mail/list", json!({"session": run}));
    assert!(
        mail.items
            .iter()
            .all(|m| m.kind != PingKind::Block || m.resolved)
    );
}

// ---------------------------------------------------------------------------------------------
// The hub on the real ledger worker: a main checkout and a linked worktree

const BASE: &str = "\
pub fn helper(x: u32) -> u32 {
    x + 1
}

pub fn run() -> u32 {
    helper(1)
}
";

fn git_c(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn turn_job(session: &str, workdir: &Path, n: u32) -> Job {
    Job::Turn(TurnJob {
        session: session.into(),
        workdir: workdir.to_path_buf(),
        provider: "claude".into(),
        model: "opus-5.5".into(),
        n: Some(n),
        prompt: "change it".into(),
        prompt_source: "human",
        started_ms: 1_792_593_000_000,
        ended_ms: 1_792_593_030_000,
        ..TurnJob::default()
    })
}

fn wait_cond(what: &str, ok: impl Fn() -> bool) {
    let begin = Instant::now();
    while !ok() {
        assert!(begin.elapsed() < WAIT, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn ledger_has(db: &Path, session: &str, n: u32) -> bool {
    with_ledger(db, |l| {
        Ok(l.list_turns(session)
            .map_err(|e| e.to_string())?
            .iter()
            .any(|t| t.n == n))
    })
    .unwrap_or(false)
}

#[test]
fn the_turns_of_two_checkouts_feed_the_hub_with_a_signature_hit_pings_and_a_block() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("proj");
    fs::create_dir_all(repo.join("src")).unwrap();
    git_c(&repo, &["init", "-q"]);
    fs::write(repo.join("src/lib.rs"), BASE).unwrap();
    git_c(&repo, &["add", "."]);
    git_c(&repo, &["commit", "-q", "-m", "first"]);
    let wt = dir.path().join("wt");
    git_c(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-branch",
            wt.to_str().unwrap(),
        ],
    );

    let db = dir.path().join("data").join("aivory.db");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    let hub = hub_for(&db);
    let effects: Arc<Mutex<Vec<Effect>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&effects);
    hub.set_sink(Arc::new(move |e| {
        sink.lock().unwrap().push(e);
        true
    }));
    let worker = LedgerWorker::start(db.clone());
    for (session, workdir) in [("S-1", &repo), ("S-2", &wt)] {
        worker.send(Job::Baseline(BaselineJob {
            session: session.into(),
            workdir: workdir.clone(),
            provider: "claude".into(),
            model: "opus-5.5".into(),
        }));
    }
    wait_cond("the baselines", || {
        ledger_has(&db, "S-1", 0) && ledger_has(&db, "S-2", 0)
    });

    // S-1 changes the signature of `helper` in the main checkout. A test run reported a failing
    // test for the turn (the daemon marks it in the delta).
    fs::write(
        repo.join("src/lib.rs"),
        BASE.replace("helper(x: u32)", "helper(x: u32, y: u32)")
            .replace("helper(1)", "helper(1, 2)"),
    )
    .unwrap();
    with_ledger(&db, |l| {
        l.record_op("S-1", "test_failed", Some(1), 1, Some("pays_once"))
            .map_err(|e| e.to_string())
    })
    .unwrap();
    worker.send(turn_job("S-1", &repo, 1));
    wait_cond("turn 1 of S-1", || {
        last_turn(&db, "S-1") >= 1 && !hub.claims().is_empty()
    });
    assert!(hub.hits().is_empty(), "one session has no pair");
    let claims = hub.claims();
    assert!(claims.iter().all(|c| c.session == "S-1"));
    assert!(claims.iter().any(|c| c.key == "path:src/lib.rs"));
    let sym = claims
        .iter()
        .find(|c| c.key == "sym:rust:helper")
        .unwrap_or_else(|| panic!("no symbol claim in {claims:?}"));
    assert_eq!(sym.path.as_deref(), Some("src/lib.rs"));
    assert_eq!(sym.range.map(|r| r.0), Some(1), "lines count from 1");
    // The failing test is in the stored delta, and its key is a claim.
    let coord = hub.repo_of("S-1").unwrap().open().unwrap();
    let deltas = coord.deltas_of("S-1").unwrap();
    assert!(
        deltas[0]
            .tests
            .iter()
            .any(|t| t.key == "test:pays_once" && t.state == TestState::Failing),
        "{:?}",
        deltas[0].tests
    );
    assert!(claims.iter().any(|c| c.key == "test:pays_once"));

    // S-2 works in the linked worktree and appends a function: a clean merge, and the base of
    // S-2 calls `helper`, so tier 3 (a dependency) scores 0.7.
    let mut text = BASE.to_string();
    text.push_str("\npub fn extra() -> u32 {\n    helper(3)\n}\n");
    fs::write(wt.join("src/lib.rs"), text).unwrap();
    worker.send(turn_job("S-2", &wt, 1));
    wait_cond("the radar hit", || !hub.hits().is_empty());
    let hits = hub.hits();
    assert_eq!(hits.len(), 1);
    let hit = &hits[0].hit;
    assert_eq!((hit.a.as_str(), hit.b.as_str()), ("S-1", "S-2"));
    assert_eq!((hit.tier, hit.score), (3, 0.7));
    assert!(hit.trial, "the trial merge ran: {:?}", hit.trial_error);
    assert!(hit.conflict_files.is_empty());
    assert!(
        hit.keys.contains(&"sym:rust:helper".to_string()),
        "{:?}",
        hit.keys
    );
    // The worktree and the main checkout share one coordination database.
    assert_eq!(
        hub.repo_of("S-1").unwrap().db(),
        hub.repo_of("S-2").unwrap().db()
    );
    // 0.7 opens an inbox item through the sink.
    let got = effects.lock().unwrap().clone();
    assert!(
        got.iter()
            .any(|e| matches!(e, Effect::Conflict(c) if c.hit.score == 0.7 && c.hunks.is_empty())),
        "{got:?}"
    );

    // FYI pings from the radar wait in both mailboxes. The delivery is quoted data with the
    // sender, once.
    let mail = hub.take_for_turn("S-1", &repo).expect("mail for S-1");
    assert!(mail.contains("S-2"), "{mail}");
    assert!(mail.contains("also changes"), "{mail}");
    assert!(hub.take_for_turn("S-1", &repo).is_none());
    assert_eq!(hub.mailbox("S-2").len(), 1);

    // The renewal of the claims.
    let before = coord.claims_of("S-1").unwrap()[0].expires_ms;
    std::thread::sleep(Duration::from_millis(30));
    hub.maintain(&HashSet::from(["S-1".to_string()]));
    let after = coord.claims_of("S-1").unwrap()[0].expires_ms;
    assert!(after > before, "the heartbeat renews the lease");
    assert_eq!(hub.hits().len(), 1, "both sessions still edit");

    // A block on a file pauses the edit asks of S-1 on it, and only on it.
    hub.send_ping(&Ping {
        from: "you".into(),
        to: "S-1".into(),
        kind: aivory_coord::PingKind::Block,
        key: "path:src/lib.rs".into(),
        text: "Wait for the merge of S-2.".into(),
        deadline_ms: None,
    })
    .unwrap();
    let ask = format!("{}/src/lib.rs", repo.display());
    let reason = hub.block_reason("S-1", &ask).unwrap();
    assert!(
        reason.contains("you") && reason.contains("override"),
        "{reason}"
    );
    assert!(hub.block_reason("S-1", "src/other.rs").is_none());
    assert!(
        hub.block_reason("S-2", "src/lib.rs").is_none(),
        "only the receiver is paused"
    );
    assert_eq!(
        hub.override_blocks("S-1", &["path:src/other.rs".into()])
            .unwrap(),
        0
    );
    assert_eq!(hub.override_blocks("S-1", &[]).unwrap(), 1);
    assert!(hub.block_reason("S-1", "src/lib.rs").is_none());

    // The agent tools: the server knows the session and the folder of its run.
    let server = McpServer::start(|_| false).unwrap();
    server.set_tools(Arc::new(CoordTools::new(Arc::clone(&hub))));
    server.set_cwd(&repo);
    server.set_session("S-1");
    let (port, token) = (server.port(), server.token().to_string());
    let listed = mcp(
        port,
        &token,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let tools = listed["result"]["tools"].as_array().unwrap();
    let ping_def = tools
        .iter()
        .find(|t| t["name"] == "ping")
        .expect("the ping tool");
    assert!(
        ping_def["description"]
            .as_str()
            .unwrap()
            .contains("information, not a permission"),
        "the tool text says that pings are information"
    );
    assert!(tools.iter().any(|t| t["name"] == "claim"));
    assert!(tools.iter().any(|t| t["name"] == "subscribe"));

    // The permission tool lets the coordination tools pass without an ask (the server of this
    // test would deny an ask at once).
    let allowed = mcp(
        port,
        &token,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "permission",
            "arguments": {"tool_name": "mcp__aivory__claim", "input": {"keys": ["src/x.rs"]}}}}),
    );
    assert!(
        allowed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"allow\""),
        "{allowed}"
    );

    let (text, error) = tool(
        port,
        &token,
        "claim",
        json!({"keys": ["src/lib.rs", "sym:rust:Money"]}),
    );
    assert!(!error, "{text}");
    assert!(text.contains("claimed 2 key(s)"), "{text}");
    assert!(
        text.contains("S-2"),
        "the other session on src/lib.rs is named: {text}"
    );
    let (text, error) = tool(
        port,
        &token,
        "claim",
        json!({"keys": ["a.rs"], "state": "editing"}),
    );
    assert!(error && text.contains("intent"), "{text}");

    let (text, error) = tool(
        port,
        &token,
        "ping",
        json!({"to": "S-2", "kind": "question", "text": "Do you keep `extra`?", "key": "src/lib.rs"}),
    );
    assert!(!error, "{text}");
    let (_, error) = tool(
        port,
        &token,
        "ping",
        json!({"to": "S-2", "kind": "block", "text": "stop"}),
    );
    assert!(error, "an agent cannot send a block");
    let (_, error) = tool(port, &token, "ping", json!({"to": "S-1", "text": "me"}));
    assert!(error, "no ping to yourself");
    let (_, error) = tool(port, &token, "ping", json!({"to": "S-404", "text": "who"}));
    assert!(error, "an unknown session");
    let waiting = hub.take_for_turn("S-2", &wt).unwrap();
    assert!(
        waiting.contains("S-1") && waiting.contains("Do you keep"),
        "{waiting}"
    );
    assert!(waiting.contains("QUESTION"), "{waiting}");

    let (text, error) = tool(
        port,
        &token,
        "subscribe",
        json!({"key": "sym:*:Money", "change": "signature"}),
    );
    assert!(!error && text.contains("subscribed"), "{text}");
    let subs = hub
        .repo_of("S-1")
        .unwrap()
        .open()
        .unwrap()
        .subscriptions("S-1")
        .unwrap();
    assert_eq!(subs.len(), 1);

    // Without a session the tools refuse.
    let anonymous = McpServer::start(|_| false).unwrap();
    anonymous.set_tools(Arc::new(CoordTools::new(Arc::clone(&hub))));
    let (text, error) = tool(
        anonymous.port(),
        anonymous.token(),
        "claim",
        json!({"keys": ["src/lib.rs"]}),
    );
    assert!(error && text.contains("session"), "{text}");
}

fn mcp(port: u16, token: &str, body: &Value) -> Value {
    let body = body.to_string();
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let json = resp.split_once("\r\n\r\n").unwrap().1;
    // The permission tool answers as an event stream: the data line holds the message.
    let json = json
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap_or(json);
    serde_json::from_str(json).unwrap()
}

fn tool(port: u16, token: &str, name: &str, args: Value) -> (String, bool) {
    let v = mcp(
        port,
        token,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}),
    );
    (
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        v["result"]["isError"].as_bool().unwrap_or(true),
    )
}

// ---------------------------------------------------------------------------------------------
// The demo answers

#[test]
fn the_demo_answers_every_coordination_method_like_the_radar_frame() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../aivory-sessions/tests/fixtures");
    let config = aivoryd::Config {
        roots: aivory_sessions::StoreRoots {
            claude: base.join("claude"),
            codex: base.join("codex"),
            gemini: base.join("gemini"),
        },
        cache_file: None,
    };
    let endpoint = Endpoint::new(&format!("coord-demo-{}", std::process::id())).unwrap();
    let server = aivoryd::Server::bind(
        Arc::new(aivoryd::Daemon::with_demo(config)),
        endpoint.clone(),
    )
    .expect("bind");
    let handle = std::thread::spawn(move || server.run());
    let d = endpoint;

    // The radar frame: five sessions, four pairs with the scores 1.0, 0.7, 0.4, 0.1.
    let radar: RadarList = typed(&d, "radar/list", json!({}));
    let scores: Vec<f32> = radar.hits.iter().map(|h| h.score).collect();
    assert_eq!(scores, [1.0, 0.7, 0.4, 0.1]);
    let ids: Vec<&str> = radar.sessions.iter().map(|s| s.id.as_str()).collect();
    for want in ["7f3a", "8a22", "k9d4", "m2e5", "8d07"] {
        assert!(ids.contains(&want), "{want} in {ids:?}");
    }
    assert_eq!(radar.hits[0].conflict_files, ["src/payment/charge.rs"]);
    assert_eq!(
        (radar.hits[1].tier, radar.hits[2].tier, radar.hits[3].tier),
        (3, 2, 1)
    );
    let none: RadarList = typed(&d, "radar/list", json!({"codebase": "nope"}));
    assert!(none.hits.is_empty() && none.sessions.is_empty());

    let claims: ClaimList = typed(&d, "claims/list", json!({}));
    assert_eq!(claims.claims.len(), 10);
    assert!(claims.claims.iter().any(|c| c.state == ClaimState::Intent));
    let mine: ClaimList = typed(&d, "claims/list", json!({"session": "7f3a"}));
    assert_eq!(mine.claims.len(), 3);
    let other_repo: ClaimList = typed(&d, "claims/list", json!({"codebase": "nope"}));
    assert!(other_repo.claims.is_empty());

    let mail: MailList = typed(&d, "mail/list", json!({"session": "7f3a"}));
    assert_eq!(mail.items.len(), 2);
    let empty: MailList = typed(&d, "mail/list", json!({"session": "8d07"}));
    assert!(empty.items.is_empty());

    // A ping, a block (with the alias `mail/send`), and the override change the mailbox.
    ok(
        &d,
        "radar/ping",
        json!({"to": "8d07", "kind": "fyi", "text": "Hello."}),
    );
    let r = call(
        &d,
        "radar/ping",
        json!({"to": "8d07", "kind": "block", "text": "Stop."}),
    );
    assert_eq!(
        r["error"]["code"],
        aivory_protocol::rpc::code::INVALID_PARAMS
    );
    ok(
        &d,
        "mail/send",
        json!({"from": "8a22", "to": "8d07", "kind": "block", "text": "Stop.", "keys": ["path:db.rs"]}),
    );
    let mail: MailList = typed(&d, "mail/list", json!({"session": "8d07"}));
    assert_eq!(mail.items.len(), 2);
    assert_eq!(mail.items[1].kind, PingKind::Block);
    assert_eq!(mail.items[1].from, "8a22");
    let freed = ok(&d, "block/override", json!({"session": "8d07"}));
    assert_eq!(freed["resolved"], 1);
    let mail: MailList = typed(&d, "mail/list", json!({"session": "8d07"}));
    assert!(mail.items[1].resolved);

    ok(&d, "daemon/shutdown", Value::Null);
    let _ = handle.join();
}
