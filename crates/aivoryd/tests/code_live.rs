//! The code service through the real daemon binary: a scratch git repo with a synthetic native
//! Codex session (so that the scanner shows the codebase) and two turns that the test writes
//! into the ledger of the daemon. Then `code/file`, `code/references`, and `ledger/blame` over
//! the local socket.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aivory_protocol::WarRoom;
use aivory_protocol::code::{CodeFile, FileBlame, PlacePage, TurnList};
use aivory_protocol::rpc::code;
use aivory_vcs::{GitVcs, Ledger, Turn, finish_turn};
use aivoryd::{Endpoint, client};
use rusqlite::Connection;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(90);
const NATIVE: &str = "99999999-9999-4999-8999-999999999997";

const BASE: &str = "pub fn helper(x: u32) -> u32 {\n    x + 1\n}\n";
const ADDED: &str = "\npub fn twice(x: u32) -> u32 {\n    helper(helper(x))\n}\n";
const MORE: &str = "\npub fn thrice(x: u32) -> u32 {\n    helper(twice(x))\n}\n";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("aivoryd-codelive-{}-{name}", std::process::id()));
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

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn append(path: &Path, text: &str) {
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

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

fn setup(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let repo = scratch.0.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("src/lib.rs"), BASE).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);

    let home = scratch.0.join("home");
    let dir = home.join(".codex/sessions/2026/10/22");
    fs::create_dir_all(&dir).unwrap();
    let lines = [
        json!({"timestamp": "2026-10-22T10:00:00.000Z", "type": "session_meta", "payload": {
            "id": NATIVE, "timestamp": "2026-10-22T10:00:00.000Z",
            "cwd": repo.display().to_string(), "originator": "codex_exec",
            "cli_version": "0.0.1-test", "source": "exec", "model_provider": "openai"}}),
        json!({"timestamp": "2026-10-22T10:00:01.000Z", "type": "event_msg", "payload": {
            "type": "user_message", "message": "Synthetic prompt"}}),
        json!({"timestamp": "2026-10-22T10:00:02.000Z", "type": "event_msg", "payload": {
            "type": "agent_message", "message": "Synthetic answer"}}),
    ];
    let text: Vec<String> = lines.iter().map(Value::to_string).collect();
    fs::write(
        dir.join(format!("rollout-2026-10-22T10-00-00-{NATIVE}.jsonl")),
        text.join("\n") + "\n",
    )
    .unwrap();
    (repo, home)
}

fn start_daemon(scratch: &Scratch, repo: &Path, home: &Path) -> Daemon {
    let name = format!("codelive-{}", std::process::id());
    let child = Command::new(env!("CARGO_BIN_EXE_aivoryd"))
        .args(["serve", "--endpoint", &name, "--scope"])
        .arg(repo)
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

fn call(d: &Daemon, method: &str, params: Value) -> Value {
    client::call(&d.endpoint, method, params, &mut |_| {}).expect("call the daemon")
}

fn ok<T: serde::de::DeserializeOwned>(d: &Daemon, method: &str, params: Value) -> T {
    let r = call(d, method, params);
    assert!(r.get("error").is_none(), "{method} failed: {r}");
    serde_json::from_value(r["result"].clone()).unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn record(db: &Path, repo: &Path, n: u32, prompt: &str) {
    let conn = Connection::open(db).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    let ledger = Ledger::new(&conn).unwrap();
    let turn = Turn {
        session_id: NATIVE.into(),
        n,
        provider: "codex".into(),
        model: Some("gpt-test".into()),
        prompt: prompt.into(),
        prompt_source: "human".into(),
        started_at: 1_792_593_000 + i64::from(n) * 60,
        ended_at: 1_792_593_030 + i64::from(n) * 60,
        cost_usd: Some(0.25),
        ..Turn::default()
    };
    finish_turn(&GitVcs::new(), &ledger, repo, turn).unwrap();
}

#[test]
fn the_daemon_serves_code_and_the_ledger_for_a_scanned_session() {
    let scratch = Scratch::new("main");
    let (repo, home) = setup(&scratch);
    let d = start_daemon(&scratch, &repo, &home);

    // The scanner shows the codebase of the repo.
    let codebase = wait_until("the codebase", || {
        let r = client::call(&d.endpoint, "warroom/snapshot", Value::Null, &mut |_| {}).ok()?;
        let w: WarRoom = serde_json::from_value(r.get("result")?.clone()).ok()?;
        w.codebases.first().map(|c| c.id.clone())
    });
    // The daemon creates the database at its start.
    wait_until("the database", || scratch.db().exists().then_some(()));

    // Turn 0 is the baseline, turn 1 adds `twice`, turn 2 adds `thrice`.
    record(&scratch.db(), &repo, 0, "");
    append(&repo.join("src/lib.rs"), ADDED);
    record(&scratch.db(), &repo, 1, "add twice");
    append(&repo.join("src/lib.rs"), MORE);
    record(&scratch.db(), &repo, 2, "add thrice");

    let file: CodeFile = ok(
        &d,
        "code/file",
        json!({"codebase": codebase, "path": "src/lib.rs", "lines": {"from": 1, "to": 2}}),
    );
    assert_eq!(file.language, "rust");
    assert_eq!(file.text, format!("{BASE}{ADDED}{MORE}"));
    assert_eq!(file.outline.len(), 3);
    assert!(file.highlights.iter().all(|h| h.line <= 2) && !file.highlights.is_empty());

    // `helper` in `twice` (line 6): the definition and the uses, from the index of the daemon.
    let refs: PlacePage = ok(
        &d,
        "code/references",
        json!({"codebase": codebase, "path": "src/lib.rs", "line": 6, "col": 6}),
    );
    let lines: Vec<u32> = refs.items.iter().map(|p| p.line).collect();
    assert_eq!(refs.name, "helper");
    assert!(lines.contains(&6) && lines.contains(&10), "{lines:?}");
    let def: PlacePage = ok(
        &d,
        "code/definition",
        json!({"codebase": codebase, "path": "src/lib.rs", "line": 6, "col": 6}),
    );
    assert_eq!(
        (def.items[0].path.as_str(), def.items[0].line),
        ("src/lib.rs", 1)
    );

    // Blame by prompt: the session is found by its native id.
    let blame: FileBlame = ok(
        &d,
        "ledger/blame",
        json!({"codebase": codebase, "path": "src/lib.rs", "session": NATIVE}),
    );
    let turns: Vec<Option<u32>> = blame
        .hunks
        .iter()
        .map(|h| h.turn.as_ref().map(|t| t.n))
        .collect();
    assert_eq!(turns, [None, Some(1), Some(2)], "{:?}", blame.hunks);
    assert_eq!(blame.hunks[1].turn.as_ref().unwrap().prompt, "add twice");
    assert_eq!(blame.hunks[0].commit.as_ref().unwrap().summary, "first");

    let list: TurnList = ok(&d, "ledger/turns", json!({"session": NATIVE}));
    assert_eq!(list.turns.len(), 3);
    assert_eq!(list.turns[1].files[0].path, "src/lib.rs");

    // The session works in the main checkout of the user: no revert.
    let r = call(&d, "ledger/revert", json!({"session": NATIVE, "turn": 2}));
    assert_eq!(r["error"]["code"], code::INVALID_STATE, "{r}");
    assert!(
        fs::read_to_string(repo.join("src/lib.rs"))
            .unwrap()
            .contains("thrice")
    );

    // Errors keep their codes through the socket.
    let r = call(&d, "code/file", json!({"codebase": "nope", "path": "a.rs"}));
    assert_eq!(r["error"]["code"], code::NOT_FOUND);
    let r = call(
        &d,
        "code/file",
        json!({"codebase": codebase, "path": "../x"}),
    );
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    let r = call(&d, "ledger/turns", json!({"session": "ghost"}));
    assert_eq!(r["error"]["code"], code::SESSION_NOT_FOUND);
}
