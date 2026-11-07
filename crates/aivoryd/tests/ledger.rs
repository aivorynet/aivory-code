//! The turn ledger, the task order, and the fork at a turn, through the real daemon binary and
//! a fake CLI (`fake-agent` of `aivory-agents`, as `codex` and `gemini`). Synthetic data only:
//! the repo, the home folder, and the cache folder are scratch folders.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aivory_protocol::rpc::code;
use aivory_protocol::{Pane, SessionState, WarRoom};
use aivory_vcs::{Ledger, Turn};
use aivoryd::{Endpoint, client};
use rusqlite::Connection;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(90);
/// The id that the fake Codex gives to every thread.
const FAKE_THREAD: &str = "fake-thread";
const NATIVE_CODEX: &str = "99999999-9999-4999-8999-999999999998";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("aivoryd-ledger-{}-{name}", std::process::id()));
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

/// A scratch git repo with one commit, a scratch home with one synthetic native Codex session
/// in the repo (so that the scanner shows the codebase), and a folder with the fake agent as
/// `codex` and `gemini`.
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
    let name = format!("ledger-{name}-{}", std::process::id());
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

fn call(d: &Daemon, method: &str, params: Value) -> Value {
    client::call(&d.endpoint, method, params, &mut |_| {}).expect("call the daemon")
}

fn ok(d: &Daemon, method: &str, params: Value) -> Value {
    let r = call(d, method, params);
    assert!(r.get("error").is_none(), "{method} failed: {r}");
    r["result"].clone()
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

fn tail_has(p: &Pane, text: &str) -> bool {
    p.tail.iter().any(|l| l.text == text)
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

/// The turns of a session in the ledger, `None` while the database or the tables are missing.
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
    wait_until(&format!("turn {n} in the ledger"), || {
        turns(scratch, session).filter(|t| t.iter().any(|x| x.n == n))
    })
}

/// HEAD, the staged state, the status, and the branches of a repo.
fn repo_state(repo: &Path) -> [String; 4] {
    [
        git_out(repo, &["rev-parse", "HEAD"]),
        git_out(repo, &["ls-files", "-s"]),
        git_out(repo, &["status", "--porcelain"]),
        git_out(repo, &["branch", "--list"]),
    ]
}

#[test]
fn a_live_run_fills_the_ledger_and_a_fork_at_a_turn_starts_in_a_worktree() {
    let fake = build_fake_agent();
    let scratch = Scratch::new("run");
    let (repo, home, bin) = setup(&scratch, &fake);
    let d = start_daemon("run", &scratch, &repo, &home, &bin);
    let codebase = codebase_of(&d);

    // Turn 0 is the baseline at the start. Turn 1 ends after b.txt appeared.
    let run = new_session(&d, &codebase, "codex", "slow");
    wait_for(&d, "the slow turn", |w| {
        pane_of(w, &run).filter(|p| tail_has(p, "working"))
    });
    let t = wait_for_turn(&scratch, FAKE_THREAD, 0);
    assert!(t[0].snapshot_ref.is_some(), "the baseline has a snapshot");
    fs::write(repo.join("b.txt"), "b\n").unwrap();
    ok(&d, "session/pause", json!({"id": run}));
    wait_for(&d, "the end of turn 1", |w| {
        pane_of(w, &run).filter(|p| p.state == SessionState::Idle)
    });
    wait_for_turn(&scratch, FAKE_THREAD, 1);

    // Turn 2.
    fs::write(repo.join("c.txt"), "c\n").unwrap();
    let before = repo_state(&repo);
    ok(&d, "session/send", json!({"id": run, "text": "say two"}));
    wait_for(&d, "the second turn", |w| {
        pane_of(w, &run).filter(|p| tail_has(p, "two") && p.state == SessionState::Idle)
    });
    let all = wait_for_turn(&scratch, FAKE_THREAD, 2);

    // The ledger holds the turns 0, 1, 2.
    assert_eq!(all.iter().map(|t| t.n).collect::<Vec<_>>(), [0, 1, 2]);
    assert!(all.iter().all(|t| t.provider == "codex"));
    assert_eq!(all[1].prompt, "slow");
    assert_eq!(all[2].prompt, "say two");
    assert_eq!(all[1].prompt_source, "human");
    assert!(all[0].files.is_empty());
    let paths = |t: &Turn| t.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&all[1]), ["b.txt"]);
    assert_eq!(paths(&all[2]), ["c.txt"]);
    // Seconds, not milliseconds.
    assert!(all[1].ended_at > 1_700_000_000 && all[1].ended_at < 100_000_000_000);
    assert!(all[1].started_at <= all[1].ended_at);
    // The snapshots are hidden refs. HEAD, the index, the status, and the branches stay.
    for n in 0..=2 {
        let r = format!("refs/aivory/turns/{FAKE_THREAD}/{n}");
        git_out(&repo, &["rev-parse", "--verify", &r]);
    }
    assert_eq!(repo_state(&repo), before);
    assert!(
        repo_state(&repo)[2].contains("b.txt"),
        "the user files stay untracked"
    );

    // Fork at turn 1: a worktree at the snapshot of turn 1 (b.txt, not c.txt) and a run that
    // starts with the packet of the turn 1.
    let r = call(&d, "session/fork", json!({"id": run, "at": "9"}));
    assert_eq!(r["error"]["code"], code::INVALID_STATE, "{r}");
    let fork = ok(&d, "session/fork", json!({"id": run, "at": "1"}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(fork, run);
    let tree = git_out(
        &repo,
        &[
            "rev-parse",
            &format!("refs/aivory/turns/{FAKE_THREAD}/1^{{tree}}"),
        ],
    );
    let work = scratch
        .0
        .join("cache")
        .join("workspaces")
        .join(&codebase)
        .join(&fork);
    assert!(
        work.join("b.txt").is_file(),
        "the worktree has the files of turn 1"
    );
    assert!(
        !work.join("c.txt").exists(),
        "the worktree has no later file"
    );
    assert_eq!(git_out(&work, &["rev-parse", "HEAD^{tree}"]), tree);
    let pane = wait_for(&d, "the fork run", |w| {
        pane_of(w, &fork).filter(|p| tail_has(p, "unknown command"))
    });
    assert_eq!(pane.provider, "codex");
    assert!(pane.title.contains("turn 1"), "{}", pane.title);
    assert!(
        pane.prompt.contains("Turn 1\nPrompt: slow"),
        "{}",
        pane.prompt
    );
    assert!(!pane.prompt.contains("say two"), "{}", pane.prompt);
    assert!(pane.prompt.chars().count() < 8_300);
    // The diff of the fork is the diff of its worktree, which equals the snapshot.
    let diff = ok(&d, "session/diff", json!({"id": fork}));
    assert_eq!(diff["files"].as_array().map(Vec::len), Some(0), "{diff}");
    // The main repo is untouched by the fork.
    assert_eq!(repo_state(&repo), before);

    // Fork at turn 2 with another provider: the worktree has c.txt.
    let other = ok(
        &d,
        "session/fork",
        json!({"id": run, "at": "2", "provider": "gemini", "prompt": "Review it."}),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let work2 = scratch
        .0
        .join("cache")
        .join("workspaces")
        .join(&codebase)
        .join(&other);
    assert!(work2.join("b.txt").is_file() && work2.join("c.txt").is_file());
    let pane = wait_for(&d, "the gemini fork", |w| {
        pane_of(w, &other).filter(|p| tail_has(p, "unknown command"))
    });
    assert_eq!(pane.provider, "gemini");
    assert!(pane.prompt.contains("Turn 2\nPrompt: say two"));
    assert!(pane.prompt.ends_with("Review it."));

    for id in [other, fork, run] {
        ok(&d, "session/stop", json!({"id": id}));
    }
}

#[test]
fn tasks_get_a_creation_time_and_a_priority_in_creation_order() {
    let fake = build_fake_agent();
    let scratch = Scratch::new("tasks");
    let (repo, home, bin) = setup(&scratch, &fake);
    let d = start_daemon("tasks", &scratch, &repo, &home, &bin);
    let codebase = codebase_of(&d);

    let before = now_ms();
    let mut ids = Vec::new();
    for (title, priority) in [
        ("first", Some("low")),
        ("second", Some("urgent")),
        ("third", None),
    ] {
        let mut params = json!({
            "title": title, "codebase": codebase, "part": "root",
            "playbook": "feature", "prompt": "Do it.", "when": "queue",
        });
        if let Some(p) = priority {
            params["priority"] = json!(p);
        }
        let task = ok(&d, "tasks/create", params);
        ids.push(task["id"].as_str().unwrap().to_string());
    }
    let after = now_ms();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "ids: {ids:?}");

    let list = ok(&d, "tasks/list", json!({}))["tasks"].clone();
    let rows: Vec<(String, String, i64)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["title"].as_str().unwrap().to_string(),
                t["priority"].as_str().unwrap_or("normal").to_string(),
                t["createdAt"].as_i64().unwrap(),
            )
        })
        .collect();
    let titles: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(titles, ["first", "second", "third"]);
    let priorities: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(priorities, ["low", "urgent", "normal"]);
    let times: Vec<i64> = rows.iter().map(|r| r.2).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    assert!(
        times.iter().all(|t| *t >= before && *t <= after),
        "{times:?}"
    );
}

#[test]
fn a_ledger_setting_turns_the_snapshots_off() {
    let fake = build_fake_agent();
    let scratch = Scratch::new("off");
    let (repo, home, bin) = setup(&scratch, &fake);
    let d = start_daemon("off", &scratch, &repo, &home, &bin);
    let codebase = codebase_of(&d);
    ok(
        &d,
        "settings/set",
        json!({"key": "ledger.snapshots", "value": false}),
    );
    let run = new_session(&d, &codebase, "codex", "say hi");
    wait_for(&d, "the turn end", |w| {
        pane_of(w, &run).filter(|p| tail_has(p, "hi") && p.state == SessionState::Idle)
    });
    std::thread::sleep(Duration::from_millis(1500));
    assert!(turns(&scratch, FAKE_THREAD).unwrap_or_default().is_empty());
    let refs = git_out(&repo, &["for-each-ref", "refs/aivory"]);
    assert!(refs.is_empty(), "{refs}");
    // The relay setting is a plain string key.
    let s = ok(
        &d,
        "settings/set",
        json!({"key": "remote.relay", "value": "none"}),
    );
    assert_eq!(s["remote"]["relay"], "none");
    ok(&d, "session/stop", json!({"id": run}));
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// `2026-10-22T10:00:00.000Z` for Unix seconds.
fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[test]
fn native_sessions_get_a_cost_usage_windows_and_ledger_turns() {
    let fake = build_fake_agent();
    let scratch = Scratch::new("native");
    let (repo, home, bin) = setup(&scratch, &fake);
    // Replace the setup transcript by one with a model, token counts and a turn end.
    let file = fs::read_dir(home.join(".codex/sessions/2026/10/22"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let now = now_ms() / 1000;
    let at = |back: i64| rfc3339(now - back);
    let turn = |back: i64, prompt: &str, tokens: bool| {
        let mut lines = vec![
            json!({"timestamp": at(back), "type": "event_msg", "payload": {
                "type": "user_message", "message": prompt}}),
            json!({"timestamp": at(back - 1), "type": "event_msg", "payload": {
                "type": "agent_message", "message": format!("answer to {prompt}")}}),
        ];
        if tokens {
            lines.push(
                json!({"timestamp": at(back - 2), "type": "event_msg", "payload": {
                "type": "token_count", "info": {"last_token_usage": {
                    "input_tokens": 1_000_000, "cached_input_tokens": 800_000,
                    "output_tokens": 100_000}}}}),
            );
        }
        lines.push(
            json!({"timestamp": at(back - 3), "type": "event_msg", "payload": {
            "type": "task_complete", "turn_id": "t"}}),
        );
        lines
    };
    let head = [
        json!({"timestamp": at(60), "type": "session_meta", "payload": {
            "id": NATIVE_CODEX, "timestamp": at(60), "cwd": repo.display().to_string(),
            "originator": "codex_exec", "cli_version": "0.0.1-test", "source": "exec",
            "model_provider": "openai"}}),
        json!({"timestamp": at(59), "type": "turn_context", "payload": {
            "model": "gpt-5.3-codex", "cwd": repo.display().to_string()}}),
    ];
    let write = |extra: &[Value]| {
        let text: Vec<String> = head.iter().chain(extra).map(Value::to_string).collect();
        fs::write(
            &file,
            text.join(
                "
",
            ) + "
",
        )
        .unwrap();
    };
    let first = turn(50, "first prompt", true);
    write(&first);
    let d = start_daemon("native", &scratch, &repo, &home, &bin);

    // Cost: 200k plain input, 800k cache read, 100k output.
    let price = aivory_engine::PriceTable::builtin()
        .lookup("codex", "gpt-5.3-codex")
        .unwrap();
    let expected =
        (200_000.0 * price.input + 800_000.0 * price.cache_read + 100_000.0 * price.output)
            / 1_000_000.0;
    let pane = wait_for(&d, "the native session", |w| {
        pane_of(w, NATIVE_CODEX).filter(|p| p.cost > 0.0)
    });
    assert!(
        (pane.cost - expected).abs() < 1e-9,
        "{} vs {expected}",
        pane.cost
    );

    // Usage: 300k of 2M tokens in the 5 h window, of 20M in the week.
    let usage = wait_for(&d, "the usage windows", |w| {
        (!w.usage.is_empty()).then(|| w.usage.clone())
    });
    let pct = |label: &str| {
        usage
            .iter()
            .find(|u| u.provider == "codex" && u.label == label)
            .map(|u| (u.pct, u.estimate))
    };
    assert_eq!(pct("5 h window"), Some((15, true)), "{usage:?}");
    assert_eq!(pct("Week"), Some((2, true)), "{usage:?}");

    // A new turn ends in the transcript: the ledger records it as a native turn. No turn was
    // recorded for the one that was there at the start.
    assert!(turns(&scratch, NATIVE_CODEX).unwrap_or_default().is_empty());
    fs::write(
        repo.join("n.txt"),
        "n
",
    )
    .unwrap();
    let both: Vec<Value> = first
        .into_iter()
        .chain(turn(5, "second prompt", false))
        .collect();
    write(&both);
    let all = wait_for_turn(&scratch, NATIVE_CODEX, 2);
    assert_eq!(all.iter().map(|t| t.n).collect::<Vec<_>>(), [0, 2]);
    assert_eq!(all[1].prompt, "second prompt");
    assert_eq!(all[1].prompt_source, "native");
    assert_eq!(all[1].provider, "codex");
    assert!(
        all[1].files.is_empty(),
        "the baseline is the state at detection"
    );
    git_out(
        &repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/aivory/turns/{NATIVE_CODEX}/0"),
        ],
    );
}
