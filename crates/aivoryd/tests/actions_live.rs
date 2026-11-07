//! The action methods of the desktop controls on a live `aivoryd serve` (the real binary), with
//! a scratch home, a scratch state folder, and a scratch Claude folder. No real session, no
//! login, and no file of the user is touched. The repositories are scratch git repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aivory_protocol::WarRoom;
use aivory_protocol::rpc::code;
use aivoryd::{Endpoint, client};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(60);
const NATIVE_CODEX: &str = "99999999-9999-4999-8999-999999999999";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aivoryd-act-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("home")).unwrap();
        fs::create_dir_all(dir.join("claude")).unwrap();
        fs::create_dir_all(dir.join("work")).unwrap();
        Self(dir)
    }

    fn work(&self) -> PathBuf {
        self.0.join("work")
    }

    fn claude(&self) -> PathBuf {
        self.0.join("claude")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Live {
    child: Child,
    endpoint: Endpoint,
}

impl Drop for Live {
    fn drop(&mut self) {
        // A restart leaves a new process that this child does not own: ask it to end.
        let _ = client::call(&self.endpoint, "daemon/shutdown", Value::Null, &mut |_| {});
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(scratch: &Scratch, name: &str) -> Live {
    let name = format!("act-{name}-{}", std::process::id());
    let home = scratch.0.join("home");
    let child = Command::new(env!("CARGO_BIN_EXE_aivoryd"))
        .args(["serve", "--endpoint", &name, "--scope"])
        .arg(scratch.0.join("no-sessions-here"))
        .env("USERPROFILE", &home)
        .env("HOME", &home)
        // A git repository above the scratch folder (a home folder under git) stays out of the test.
        .env("GIT_CEILING_DIRECTORIES", &scratch.0)
        .env("AIVORY_CLAUDE_HOME", scratch.claude())
        .env("AIVORY_CACHE_DIR", scratch.0.join("cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start aivoryd");
    let live = Live {
        child,
        endpoint: Endpoint::new(&name).unwrap(),
    };
    let begin = Instant::now();
    while client::call(&live.endpoint, "daemon/info", Value::Null, &mut |_| {}).is_err() {
        assert!(begin.elapsed() < WAIT, "the daemon does not start");
        std::thread::sleep(Duration::from_millis(100));
    }
    live
}

impl Live {
    fn call(&self, method: &str, params: Value) -> Value {
        client::call(&self.endpoint, method, params, &mut |_| {}).expect("call the daemon")
    }

    fn ok(&self, method: &str, params: Value) -> Value {
        let r = self.call(method, params);
        assert!(r.get("error").is_none(), "{method} failed: {r}");
        r["result"].clone()
    }

    fn err(&self, method: &str, params: Value) -> Value {
        let r = self.call(method, params);
        assert!(r.get("error").is_some(), "{method} should fail: {r}");
        r["error"].clone()
    }

    fn view(&self) -> WarRoom {
        serde_json::from_value(self.ok("warroom/snapshot", Value::Null)).unwrap()
    }

    fn wait_for(&self, what: &str, ok: impl Fn(&WarRoom) -> bool) -> WarRoom {
        let begin = Instant::now();
        loop {
            let view = self.view();
            if ok(&view) {
                return view;
            }
            assert!(begin.elapsed() < WAIT, "timeout: {what}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn repo(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::write(root.join("lib/a.rs"), "// a\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    root
}

fn text(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn a_pinned_repo_shows_without_a_session_and_survives_a_restart() {
    let s = Scratch::new("pin");
    let root = repo(&s.work(), "shop");
    {
        let d = start(&s, "pin1");
        let info = d.ok("daemon/info", Value::Null);
        assert!(!info["host"].as_str().unwrap().is_empty());
        assert!(!info["cwd"].as_str().unwrap().is_empty());

        let plain = s.work().join("plain");
        fs::create_dir_all(&plain).unwrap();
        let e = d.err("repos/add", json!({ "path": text(&plain) }));
        assert_eq!(e["code"], code::INVALID_PARAMS);
        assert!(e["message"].as_str().unwrap().contains("git"));

        let cb = d.ok("repos/add", json!({ "path": text(&root) }));
        assert_eq!(cb["id"], "shop");
        assert_eq!(cb["daemon"], "this machine");
        assert!(
            d.view().codebase("shop").is_some(),
            "the view has it at once"
        );
        // The scanner keeps it in the view, with its id and its parts.
        std::thread::sleep(Duration::from_millis(4500));
        let view = d.view();
        assert_eq!(view.codebases.iter().filter(|c| c.id == "shop").count(), 1);
        d.ok("repos/get", json!({ "id": "shop" }));
        // The same folder again: the same codebase.
        assert_eq!(
            d.ok("repos/add", json!({ "path": text(&root.join("lib")) }))["id"],
            "shop"
        );
        assert!(!root.join(".aivory").exists(), "no write to the repository");
    }
    // A new daemon on the same state folder finds the pin.
    let d = start(&s, "pin2");
    d.wait_for("the pinned codebase", |v| v.codebase("shop").is_some());
    d.ok("repos/remove", json!({ "id": "shop" }));
    assert!(d.view().codebase("shop").is_none());
    std::thread::sleep(Duration::from_millis(4500));
    assert!(
        d.view().codebase("shop").is_none(),
        "the scanner does not bring it back"
    );
    assert_eq!(
        d.err("repos/remove", json!({ "id": "shop" }))["code"],
        code::NOT_FOUND
    );
    assert!(
        d.ok("settings/get", Value::Null)["settings"]["pinned"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_new_project_a_part_an_acp_agent_an_import_and_a_scale() {
    let s = Scratch::new("misc");
    let d = start(&s, "misc");

    // project/new
    let cb = d.ok(
        "project/new",
        json!({ "name": "tool", "parent": text(&s.work()), "template": "node" }),
    );
    assert_eq!(cb["id"], "tool");
    let root = s.work().join("tool");
    for file in [
        "README.md",
        ".gitignore",
        "package.json",
        "index.js",
        ".aivory/map.toml",
    ] {
        assert!(root.join(file).is_file(), "{file}");
    }
    assert!(root.join(".git").exists());
    assert!(d.view().codebase("tool").is_some());
    assert_eq!(
        d.err(
            "project/new",
            json!({ "name": "tool", "parent": text(&s.work()) })
        )["code"],
        code::INVALID_STATE
    );
    assert_eq!(
        d.err(
            "project/new",
            json!({ "name": "a/b", "parent": text(&s.work()) })
        )["code"],
        code::INVALID_PARAMS
    );

    // map/addPart: the other parts and the comments stay.
    let map_file = root.join(".aivory/map.toml");
    let mut map = fs::read_to_string(&map_file).unwrap();
    map.push_str("\n# a note of the user\n");
    fs::write(&map_file, &map).unwrap();
    let new_map = d.ok(
        "map/addPart",
        json!({ "codebase": "tool", "name": "docs", "paths": ["docs/**"] }),
    );
    assert!(
        new_map["parts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "docs")
    );
    let disk = fs::read_to_string(&map_file).unwrap();
    assert!(
        disk.contains("# a note of the user") && disk.contains("name  = \"docs\""),
        "{disk}"
    );
    assert!(
        d.view()
            .codebase("tool")
            .unwrap()
            .parts
            .iter()
            .any(|p| p.name == "docs")
    );
    // The scanner reads the changed map again, and keeps the part.
    std::thread::sleep(Duration::from_millis(4500));
    assert!(
        d.view()
            .codebase("tool")
            .unwrap()
            .parts
            .iter()
            .any(|p| p.name == "docs")
    );
    assert!(
        d.ok("repos/map", json!({ "id": "tool" }))["parts"]
            .to_string()
            .contains("docs")
    );
    assert_eq!(
        d.err(
            "map/addPart",
            json!({ "codebase": "tool", "name": "docs", "paths": ["x"] })
        )["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        d.err(
            "map/addPart",
            json!({ "codebase": "tool", "name": "z", "paths": ["a["] })
        )["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        d.err(
            "map/addPart",
            json!({ "codebase": "nope", "name": "z", "paths": ["a"] })
        )["code"],
        code::NOT_FOUND
    );
    // A repository without a map gets one on the first part.
    let bare = repo(&s.work(), "bare");
    d.ok("repos/add", json!({ "path": text(&bare) }));
    assert!(!bare.join(".aivory").exists());
    d.ok(
        "map/addPart",
        json!({ "codebase": "bare", "name": "extra", "paths": ["extra/**"] }),
    );
    assert!(bare.join(".aivory/map.toml").is_file());

    // providers/addAcp
    let added = d.ok(
        "providers/addAcp",
        json!({ "name": "Mine", "command": env!("CARGO_BIN_EXE_aivoryd") }),
    );
    assert_eq!(added["found"], true);
    assert!(added["version"].as_str().unwrap().contains("aivoryd"));
    let view = d.ok("settings/get", Value::Null);
    assert!(
        view["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "acp-mine" && c["status"] == "acp"),
        "{view}"
    );
    let ghost = d.ok(
        "providers/addAcp",
        json!({ "name": "Ghost", "command": "no-such-agent-xyz" }),
    );
    assert_eq!(ghost["found"], false);
    assert_eq!(
        d.err(
            "providers/addAcp",
            json!({ "name": "mine", "command": "x" })
        )["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        d.err("settings/set", json!({ "key": "pinned", "value": [] }))["code"],
        code::INVALID_PARAMS
    );
    d.ok("providers/removeAcp", json!({ "id": "acp-ghost" }));
    assert_eq!(
        d.err("providers/removeAcp", json!({ "id": "acp-ghost" }))["code"],
        code::NOT_FOUND
    );

    // settings/importClaude reads only the model and the mode.
    let file = s.claude().join("settings.json");
    let body = json!({
        "model": "opus",
        "permissions": { "defaultMode": "acceptEdits" },
        "env": { "ANTHROPIC_AUTH_TOKEN": "sk-secret-token" }
    })
    .to_string();
    fs::write(&file, &body).unwrap();
    let report = d.ok("settings/importClaude", Value::Null);
    assert_eq!(report["found"], true);
    assert!(!report.to_string().contains("sk-secret"));
    let view = d.ok("settings/get", Value::Null);
    assert_eq!(view["settings"]["providers"]["claude"]["mode"], "auto");
    assert_eq!(view["settings"]["providers"]["claude"]["model"], "opus");
    assert_eq!(fs::read_to_string(&file).unwrap(), body);

    // daemon/scale
    let a = d.ok(
        "daemon/scale",
        json!({ "daemon": "local", "maxRunning": 3 }),
    );
    assert_eq!(a["maxRunning"], 3);
    assert_eq!(d.view().autopilot.max_running, 3);
    assert_eq!(
        d.err("daemon/scale", json!({ "daemon": "x", "maxRunning": 3 }))["code"],
        code::NOT_FOUND
    );
    assert_eq!(
        d.err(
            "daemon/scale",
            json!({ "daemon": "local", "maxRunning": 0 })
        )["code"],
        code::INVALID_PARAMS
    );
}

#[test]
fn a_card_moves_by_the_allowed_moves() {
    let s = Scratch::new("tasks");
    let d = start(&s, "tasks");
    let root = repo(&s.work(), "board");
    d.ok("repos/add", json!({ "path": text(&root) }));
    let task = d.ok(
        "tasks/create",
        json!({
            "title": "T", "codebase": "board", "part": "lib", "playbook": "feature",
            "prompt": "p", "when": "queue"
        }),
    );
    let id = task["id"].clone();
    assert_eq!(task["column"], "planned");
    let e = d.err("tasks/update", json!({ "id": id, "column": "review" }));
    assert_eq!(e["code"], code::INVALID_STATE);
    assert!(e["message"].as_str().unwrap().contains("Working, Done"));
    assert_eq!(
        d.err("tasks/update", json!({ "id": id, "column": "waiting" }))["code"],
        code::INVALID_STATE
    );
    let done = d.ok("tasks/update", json!({ "id": id, "column": "done" }));
    assert_eq!(done["column"], "done");
    assert_eq!(
        d.err("tasks/update", json!({ "id": id, "column": "working" }))["code"],
        code::INVALID_STATE
    );
    let back = d.ok("tasks/update", json!({ "id": id, "column": "planned" }));
    assert_eq!(back["column"], "planned");
    assert_eq!(
        d.err("tasks/update", json!({ "id": "nope", "column": "done" }))["code"],
        code::NOT_FOUND
    );
}

#[test]
fn a_restart_starts_a_new_process_with_the_same_arguments() {
    let s = Scratch::new("restart");
    let d = start(&s, "restart");
    let before = d.ok("daemon/info", Value::Null)["pid"].as_u64().unwrap();
    let root = repo(&s.work(), "keep");
    d.ok("repos/add", json!({ "path": text(&root) }));
    assert_eq!(
        d.err("daemon/restart", json!({ "daemon": "nope" }))["code"],
        code::NOT_FOUND
    );
    d.ok("daemon/restart", json!({ "daemon": "local" }));
    // The new process takes over the same endpoint.
    let begin = Instant::now();
    let after = loop {
        if let Ok(r) = client::call(&d.endpoint, "daemon/info", Value::Null, &mut |_| {})
            && let Some(pid) = r["result"]["pid"].as_u64()
            && pid != before
        {
            break pid;
        }
        assert!(begin.elapsed() < WAIT, "no new daemon");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_ne!(after, before);
    // The pin is in the state folder: the new daemon shows it.
    d.wait_for("the pin after the restart", |v| {
        v.codebase("keep").is_some()
    });
    // The `Live` guard ends the new daemon with `daemon/shutdown`.
}

#[test]
fn a_fork_at_a_span_finds_its_turn() {
    let s = Scratch::new("fork");
    let work = s.work();
    let repo = repo(&work, "forked");
    let home = s.0.join("home");
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
    let body: Vec<String> = lines.iter().map(Value::to_string).collect();
    fs::write(
        dir.join(format!("rollout-2026-10-22T10-00-00-{NATIVE_CODEX}.jsonl")),
        body.join("\n") + "\n",
    )
    .unwrap();
    let name = format!("act-fork-{}", std::process::id());
    let child = Command::new(env!("CARGO_BIN_EXE_aivoryd"))
        .args(["serve", "--endpoint", &name, "--scope"])
        .arg(&repo)
        .env("USERPROFILE", &home)
        .env("HOME", &home)
        .env("AIVORY_CLAUDE_HOME", s.claude())
        .env("AIVORY_CACHE_DIR", s.0.join("cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start aivoryd");
    let d = Live {
        child,
        endpoint: Endpoint::new(&name).unwrap(),
    };
    let begin = Instant::now();
    while client::call(&d.endpoint, "daemon/info", Value::Null, &mut |_| {}).is_err() {
        assert!(begin.elapsed() < WAIT, "the daemon does not start");
        std::thread::sleep(Duration::from_millis(100));
    }
    d.wait_for("the Codex session", |v| v.session(NATIVE_CODEX).is_some());
    let trace = d.ok("session/spans", json!({ "id": NATIVE_CODEX }));
    let turn = trace["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sp| sp["kind"] == "turn")
        .expect("a turn span")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // An unknown span is a bad parameter.
    let e = d.err("session/fork", json!({ "id": NATIVE_CODEX, "at": "s999" }));
    assert_eq!(e["code"], code::INVALID_PARAMS);
    assert!(e["message"].as_str().unwrap().contains("span"));
    // A real span maps to a turn, and the fork at a turn takes its way: this scratch session
    // has no ledger turn, so the answer is about the turn, not about the span.
    let e = d.err("session/fork", json!({ "id": NATIVE_CODEX, "at": turn }));
    assert_ne!(e["code"], code::INVALID_PARAMS, "{e}");
    assert!(
        e["code"] == code::INVALID_STATE || e["code"] == code::NOT_AVAILABLE,
        "{e}"
    );
    let by_number = d.err("session/fork", json!({ "id": NATIVE_CODEX, "at": "1" }));
    assert_eq!(
        by_number["code"], e["code"],
        "a span and its turn number take the same way"
    );
}
