//! Live Codex and Gemini runs through `aivoryd serve`, with a fake CLI in place of the real
//! ones. The fake (`fake-agent` of `aivory-agents`) speaks `codex app-server` and `gemini --acp`.
//!
//! The test starts the real daemon binary with a scratch home folder, a scratch cache folder,
//! and a PATH that finds the fake as `codex` and `gemini`. No real session and no login is
//! touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aivory_protocol::rpc::code;
use aivory_protocol::{Pane, SessionState, WarRoom};
use aivoryd::{Endpoint, client};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(60);
const NATIVE_CODEX: &str = "99999999-9999-4999-8999-999999999999";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("aivoryd-live-runs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
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

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A scratch git repo, a scratch home with one synthetic native Codex session in the repo, and
/// a folder with the fake agent as `codex` and `gemini`.
fn setup(scratch: &Scratch, fake: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let root = &scratch.0;
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("a.txt"), "x").unwrap();

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

fn start_daemon(scratch: &Scratch, repo: &Path, home: &Path, bin: &Path) -> Daemon {
    let name = format!("live-runs-{}", std::process::id());
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

/// Poll the war room until `pick` finds a pane.
fn wait_for<T>(d: &Daemon, what: &str, pick: impl Fn(&WarRoom) -> Option<T>) -> T {
    let begin = Instant::now();
    loop {
        if let Some(pane) = snapshot(d).and_then(|w| pick(&w)) {
            return pane;
        }
        assert!(begin.elapsed() < WAIT, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pane_of(w: &WarRoom, id: &str) -> Option<Pane> {
    w.sessions.iter().find(|p| p.id == id).cloned()
}

fn tail_has(p: &Pane, text: &str) -> bool {
    p.tail.iter().any(|l| l.text == text)
}

fn new_session(d: &Daemon, codebase: &str, provider: &str, mode: &str, prompt: &str) -> String {
    let r = ok(
        d,
        "session/new",
        json!({
            "codebase": codebase, "prompt": prompt,
            "composer": {"mode": mode, "provider": provider, "model": "", "effort": "high"},
        }),
    );
    r["id"].as_str().unwrap().to_string()
}

#[test]
fn codex_and_gemini_runs_start_ask_answer_fork_and_stop() {
    let fake = build_fake_agent();
    let scratch = Scratch::new();
    let (repo, home, bin) = setup(&scratch, &fake);
    let d = start_daemon(&scratch, &repo, &home, &bin);

    // The scanner shows the repo and the native Codex session.
    let begin = Instant::now();
    let codebase = loop {
        if let Some(cb) = snapshot(&d).and_then(|w| w.codebases.first().map(|c| c.id.clone())) {
            break cb;
        }
        assert!(begin.elapsed() < WAIT, "timeout: no codebase");
        std::thread::sleep(Duration::from_millis(100));
    };
    wait_for(&d, "the native session", |w| pane_of(w, NATIVE_CODEX));

    // Codex: a new run asks for permission, the answer runs the command.
    let run = new_session(&d, &codebase, "codex", "edit", "run echo hi");
    let pane = wait_for(&d, "a codex ask", |w| {
        pane_of(w, &run).filter(|p| p.state == SessionState::NeedsYou && p.ask.is_some())
    });
    assert_eq!(pane.provider, "codex");
    let ask = pane.ask.clone().unwrap();
    assert_eq!(ask.text, "echo hi");
    assert_eq!(ask.options, ["Allow", "Always allow", "Deny"]);
    assert!(
        tail_has(&pane, "codex session fake-thread"),
        "{:?}",
        pane.tail
    );
    ok(
        &d,
        "session/answer",
        json!({"id": run, "ask": ask.id, "option": "Allow"}),
    );
    let pane = wait_for(&d, "the codex turn end", |w| {
        pane_of(w, &run).filter(|p| p.state == SessionState::Idle && tail_has(p, "ok"))
    });
    assert!(tail_has(&pane, "ran echo hi"));
    assert_eq!(pane.context_pct, 10);

    // A second turn, then a fork. The fork has its own pane and the fork thread id.
    ok(&d, "session/send", json!({"id": run, "text": "say again"}));
    wait_for(&d, "the second codex turn", |w| {
        pane_of(w, &run).filter(|p| tail_has(p, "again") && p.state == SessionState::Idle)
    });
    let fork = ok(&d, "session/fork", json!({"id": run}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(fork, run);
    let fork_pane = wait_for(&d, "the codex fork", |w| {
        pane_of(w, &fork).filter(|p| tail_has(p, "codex session fork-of-fake-thread"))
    });
    assert_eq!(fork_pane.provider, "codex");

    // Stop ends the fork.
    ok(&d, "session/stop", json!({"id": fork}));
    wait_for(&d, "the fork to leave", |w| {
        pane_of(w, &fork).is_none().then_some(())
    });

    // Resume the native Codex session: the pane keeps its id and becomes a run.
    ok(
        &d,
        "session/send",
        json!({"id": NATIVE_CODEX, "text": "say resumed"}),
    );
    wait_for(&d, "the resumed codex run", |w| {
        pane_of(w, NATIVE_CODEX).filter(|p| tail_has(p, "resumed") && p.state == SessionState::Idle)
    });
    ok(&d, "session/park", json!({"id": NATIVE_CODEX}));
    ok(&d, "session/stop", json!({"id": run}));

    // Gemini: a plan mode run asks, pause cancels the ask, fork is not available.
    let g = new_session(&d, &codebase, "gemini", "plan", "run echo hi");
    let pane = wait_for(&d, "a gemini ask", |w| {
        pane_of(w, &g).filter(|p| p.state == SessionState::NeedsYou && p.ask.is_some())
    });
    assert_eq!(pane.provider, "gemini");
    assert!(
        tail_has(&pane, "gemini session fake-gemini"),
        "{:?}",
        pane.tail
    );
    ok(&d, "session/pause", json!({"id": g}));
    wait_for(&d, "the gemini pause", |w| {
        pane_of(w, &g).filter(|p| p.state == SessionState::Idle && p.ask.is_none())
    });
    ok(&d, "session/send", json!({"id": g, "text": "config"}));
    // Plan -> `--approval-mode plan`.
    wait_for(&d, "the gemini mode", |w| {
        pane_of(w, &g).filter(|p| tail_has(p, "policy= sandbox= mode=plan"))
    });
    let r = call(&d, "session/fork", json!({"id": g}));
    assert_eq!(r["error"]["code"], code::NOT_AVAILABLE, "{r}");
    ok(&d, "session/stop", json!({"id": g}));
    wait_for(&d, "the gemini run to leave", |w| {
        pane_of(w, &g).is_none().then_some(())
    });

    // A provider without live runs.
    let r = call(
        &d,
        "session/new",
        json!({"codebase": codebase, "prompt": "x",
            "composer": {"mode": "edit", "provider": "opencode", "model": "", "effort": "high"}}),
    );
    assert_eq!(r["error"]["code"], code::NOT_AVAILABLE, "{r}");
}
