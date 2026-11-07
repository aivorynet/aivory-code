//! The action methods of the desktop controls on `aivoryd serve --demo`: `repos/add`,
//! `repos/remove`, `project/new`, `map/addPart`, `providers/addAcp`, `settings/importClaude`,
//! `daemon/restart`, `daemon/scale`, `ledger/revert`, `session/fork` at a span, the radar marks,
//! `block/override`, and the task moves. Real folders are scratch folders (tempfile). The Claude
//! folder of the daemon is a scratch folder with a synthetic `settings.json`.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use aivory_protocol::rpc::{Delta, DeltaNote, Subscription, code};
use aivory_protocol::{SessionState, WarRoom};
use aivory_sessions::StoreRoots;
use aivoryd::endpoint::{self, Connection};
use aivoryd::{Config, Daemon, Endpoint, Server, client};
use serde_json::{Value, json};

fn config(claude: &Path) -> Config {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../aivory-sessions/tests/fixtures");
    Config {
        roots: StoreRoots {
            claude: claude.to_path_buf(),
            codex: base.join("codex"),
            gemini: base.join("gemini"),
        },
        cache_file: None,
    }
}

struct Demo {
    endpoint: Endpoint,
    daemon: Arc<Daemon>,
    server: Option<thread::JoinHandle<std::io::Result<()>>>,
    /// The scratch folder: Claude folder, repositories.
    dir: tempfile::TempDir,
}

fn start(name: &str) -> Demo {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    let endpoint = Endpoint::new(&format!("act-{name}-{}", std::process::id())).unwrap();
    let daemon = Arc::new(Daemon::with_demo(config(&claude)));
    let server = Server::bind(Arc::clone(&daemon), endpoint.clone()).expect("bind");
    Demo {
        endpoint,
        daemon,
        server: Some(thread::spawn(move || server.run())),
        dir,
    }
}

impl Demo {
    fn call(&self, method: &str, params: Value) -> Value {
        client::call(&self.endpoint, method, params, &mut |_| {}).unwrap()
    }

    fn ok(&self, method: &str, params: Value) -> Value {
        let r = self.call(method, params);
        assert!(r.get("error").is_none(), "{method}: {r}");
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

    fn claude(&self) -> PathBuf {
        self.dir.path().join("claude")
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = client::call(&self.endpoint, "daemon/shutdown", Value::Null, &mut |_| {});
            let _ = server.join();
        }
    }
}

/// A connection that stays open and reads the deltas.
struct Subscriber {
    conn: Connection,
    line: String,
}

impl Subscriber {
    fn open(endpoint: &Endpoint) -> (Self, Subscription) {
        let mut conn = endpoint::connect(endpoint).unwrap();
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "warroom/subscribe", "params": {} });
        writeln!(conn.writer, "{req}").unwrap();
        conn.writer.flush().unwrap();
        let mut s = Self {
            conn,
            line: String::new(),
        };
        let response = s.next();
        (
            s,
            serde_json::from_value(response["result"].clone()).unwrap(),
        )
    }

    fn next(&mut self) -> Value {
        self.line.clear();
        self.conn.reader.read_line(&mut self.line).unwrap();
        serde_json::from_str(&self.line).unwrap()
    }

    /// The next delta.
    fn delta(&mut self) -> Delta {
        let note = self.next();
        serde_json::from_value::<DeltaNote>(note["params"].clone())
            .unwrap()
            .delta
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

/// A scratch git repository with a file.
fn repo(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    root
}

fn text(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn a_repo_is_added_and_removed() {
    let d = start("repos");
    let root = repo(d.dir.path(), "shop");
    let (mut sub, _) = Subscriber::open(&d.endpoint);

    let cb = d.ok("repos/add", json!({ "path": text(&root) }));
    assert_eq!(cb["id"], "shop");
    assert_eq!(
        cb["parts"][0]["name"], "src",
        "the map guess names the parts"
    );
    assert!(matches!(sub.delta(), Delta::Codebase { codebase } if codebase.id == "shop"));
    assert!(d.view().codebase("shop").is_some());
    let listed = d.ok("repos/list", Value::Null);
    assert!(
        listed["codebases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "shop")
    );
    let detail = d.ok("repos/get", json!({ "id": "shop" }));
    assert!(
        detail["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "src/"),
        "{detail}"
    );
    d.ok("repos/map", json!({ "id": "shop" }));
    // The same folder again is the same codebase (also through a subfolder).
    let again = d.ok("repos/add", json!({ "path": text(&root.join("src")) }));
    assert_eq!(again["id"], "shop");
    assert_eq!(
        d.view().codebases.iter().filter(|c| c.id == "shop").count(),
        1
    );
    // The map file was not written.
    assert!(!root.join(".aivory").exists());

    d.ok("repos/remove", json!({ "id": "shop" }));
    assert!(matches!(sub.delta(), Delta::CodebaseRemoved { id } if id == "shop"));
    assert!(d.view().codebase("shop").is_none());
    assert_eq!(
        d.err("repos/remove", json!({ "id": "shop" }))["code"],
        code::NOT_FOUND
    );
    // A codebase of the sample has no pin.
    assert_eq!(
        d.err("repos/remove", json!({ "id": "billing" }))["code"],
        code::INVALID_STATE
    );
}

#[test]
fn a_folder_without_git_is_refused() {
    let d = start("nogit");
    let plain = d.dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let e = d.err("repos/add", json!({ "path": text(&plain) }));
    assert_eq!(e["code"], code::INVALID_PARAMS);
    assert!(e["message"].as_str().unwrap().contains("git"), "{e}");
    let e = d.err(
        "repos/add",
        json!({ "path": text(&d.dir.path().join("missing")) }),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
    assert_eq!(
        d.err("repos/add", json!({ "path": "  " }))["code"],
        code::INVALID_PARAMS
    );
}

#[test]
fn a_new_project_has_git_a_template_and_a_map() {
    let d = start("project");
    let parent = d.dir.path().join("work");
    std::fs::create_dir_all(&parent).unwrap();
    let cb = d.ok(
        "project/new",
        json!({ "name": "tool", "parent": text(&parent), "template": "rust" }),
    );
    assert_eq!(cb["id"], "tool");
    let root = parent.join("tool");
    assert!(root.join(".git").exists());
    for file in [
        "README.md",
        ".gitignore",
        "Cargo.toml",
        "src/main.rs",
        ".aivory/map.toml",
    ] {
        assert!(root.join(file).is_file(), "{file}");
    }
    assert!(d.view().codebase("tool").is_some());
    // The default template is empty.
    d.ok(
        "project/new",
        json!({ "name": "plain", "parent": text(&parent) }),
    );
    assert!(parent.join("plain/README.md").is_file());
    assert!(!parent.join("plain/Cargo.toml").exists());
    d.ok(
        "project/new",
        json!({ "name": "web", "parent": text(&parent), "template": "node" }),
    );
    assert!(parent.join("web/package.json").is_file());
    d.ok(
        "project/new",
        json!({ "name": "snake", "parent": text(&parent), "template": "python" }),
    );
    assert!(parent.join("snake/main.py").is_file());

    // An existing folder with files stays as it is.
    let e = d.err(
        "project/new",
        json!({ "name": "tool", "parent": text(&parent) }),
    );
    assert_eq!(e["code"], code::INVALID_STATE);
    assert!(root.join("Cargo.toml").is_file());
    // An empty folder is fine.
    std::fs::create_dir_all(parent.join("empty")).unwrap();
    d.ok(
        "project/new",
        json!({ "name": "empty", "parent": text(&parent) }),
    );
    // Bad input.
    for name in ["", "a/b", ".."] {
        let e = d.err(
            "project/new",
            json!({ "name": name, "parent": text(&parent) }),
        );
        assert_eq!(e["code"], code::INVALID_PARAMS, "{name:?}");
    }
    let e = d.err(
        "project/new",
        json!({ "name": "x", "parent": text(&parent.join("missing")) }),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
}

#[test]
fn a_part_goes_into_the_map_of_a_real_repo_and_a_sample() {
    let d = start("parts");
    let root = repo(d.dir.path(), "app");
    d.ok("repos/add", json!({ "path": text(&root) }));
    // The repository has a map with a comment of the user.
    std::fs::create_dir_all(root.join(".aivory")).unwrap();
    std::fs::write(
        root.join(".aivory/map.toml"),
        "# mine\nversion = 1\nupdated = \"2026-01-01T00:00:00Z\"\n\n[[part]]\nname = \"core\"\npaths = [\"src/**\"]\n",
    )
    .unwrap();
    let (mut sub, _) = Subscriber::open(&d.endpoint);
    let map = d.ok(
        "map/addPart",
        json!({ "codebase": "app", "name": "docs", "paths": ["docs/**", "*.md"] }),
    );
    assert_eq!(map["parts"].as_array().unwrap().len(), 2);
    let disk = std::fs::read_to_string(root.join(".aivory/map.toml")).unwrap();
    assert!(
        disk.contains("# mine") && disk.contains("name  = \"docs\""),
        "{disk}"
    );
    match sub.delta() {
        Delta::Codebase { codebase } => assert_eq!(codebase.parts.len(), 2),
        other => panic!("{other:?}"),
    }
    let shown = d.ok("repos/map", json!({ "id": "app" }));
    assert_eq!(shown["parts"].as_array().unwrap().len(), 2);

    // Errors write nothing.
    let before = std::fs::read_to_string(root.join(".aivory/map.toml")).unwrap();
    for (name, paths) in [
        ("docs", json!(["x/**"])),
        ("bad", json!(["a["])),
        ("none", json!([])),
        (" ", json!(["a"])),
    ] {
        let e = d.err(
            "map/addPart",
            json!({ "codebase": "app", "name": name, "paths": paths }),
        );
        assert_eq!(e["code"], code::INVALID_PARAMS, "{name:?}");
    }
    assert_eq!(
        std::fs::read_to_string(root.join(".aivory/map.toml")).unwrap(),
        before
    );
    assert_eq!(
        d.err(
            "map/addPart",
            json!({ "codebase": "nope", "name": "x", "paths": ["x"] })
        )["code"],
        code::NOT_FOUND
    );

    // A sample codebase has no folder: only the demo data changes.
    let map = d.ok(
        "map/addPart",
        json!({ "codebase": "billing", "name": "reports", "paths": ["reports/**"] }),
    );
    assert_eq!(
        map["parts"].as_array().unwrap().last().unwrap()["name"],
        "reports"
    );
    assert!(
        d.view()
            .codebase("billing")
            .unwrap()
            .parts
            .iter()
            .any(|p| p.name == "reports")
    );
    let detail = d.ok("repos/get", json!({ "id": "billing" }));
    assert!(
        detail["parts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "reports")
    );
    let e = d.err(
        "map/addPart",
        json!({ "codebase": "billing", "name": "reports", "paths": ["r"] }),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
}

#[test]
fn an_acp_agent_becomes_a_card() {
    let d = start("acp");
    let me = env!("CARGO_BIN_EXE_aivoryd");
    let added = d.ok(
        "providers/addAcp",
        json!({ "name": "My Agent", "command": me, "args": ["--acp"], "env": { "MY_FLAG": "1" } }),
    );
    assert_eq!(added["found"], true);
    assert!(
        added["version"].as_str().unwrap().contains("aivoryd"),
        "{added}"
    );
    assert_eq!(added["provider"]["id"], "acp-my-agent");
    assert_eq!(added["provider"]["status"], "acp");
    assert_eq!(added["provider"]["monogram"], "M");
    let view = d.ok("settings/get", Value::Null);
    let cards = view["providers"].as_array().unwrap();
    assert!(
        cards
            .iter()
            .any(|c| c["id"] == "acp-my-agent" && c["status"] == "acp")
    );
    assert_eq!(view["settings"]["acp"][0]["args"][0], "--acp");
    assert!(
        view["settings"]["providers"]["acp-my-agent"]["enabled"]
            .as_bool()
            .unwrap()
    );

    // A command that does not exist is stored, and the card says so.
    let gone = d.ok(
        "providers/addAcp",
        json!({ "name": "Ghost", "command": "no-such-agent-xyz" }),
    );
    assert_eq!(gone["found"], false);
    assert!(
        gone["provider"]["auth"]
            .as_str()
            .unwrap()
            .contains("not found")
    );
    // A file that exists counts as found, even when it does not run as an agent.
    let file = d.dir.path().join("agent.txt");
    std::fs::write(&file, "x").unwrap();
    let file_agent = d.ok(
        "providers/addAcp",
        json!({ "name": "File", "command": text(&file) }),
    );
    assert_eq!(file_agent["found"], true);

    // Bad input.
    assert_eq!(
        d.err(
            "providers/addAcp",
            json!({ "name": "my agent", "command": me })
        )["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        d.err("providers/addAcp", json!({ "name": "", "command": me }))["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        d.err("providers/addAcp", json!({ "name": "x", "command": "" }))["code"],
        code::INVALID_PARAMS
    );
    let e = d.err(
        "providers/addAcp",
        json!({ "name": "x", "command": me, "env": { "1 bad": "v" } }),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
    // The list has its own method.
    assert_eq!(
        d.err("settings/set", json!({ "key": "acp", "value": [] }))["code"],
        code::INVALID_PARAMS
    );

    d.ok("providers/removeAcp", json!({ "id": "acp-my-agent" }));
    let view = d.ok("settings/get", Value::Null);
    assert!(
        !view["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "acp-my-agent")
    );
    assert_eq!(
        d.err("providers/removeAcp", json!({ "id": "acp-my-agent" }))["code"],
        code::NOT_FOUND
    );
    // The ACP cards of the sample are no agents of the user.
    assert_eq!(
        d.err("providers/removeAcp", json!({ "id": "goose" }))["code"],
        code::NOT_FOUND
    );
}

#[test]
fn the_import_reads_only_the_model_and_the_mode() {
    let d = start("import");
    // No file yet.
    let none = d.ok("settings/importClaude", Value::Null);
    assert_eq!(none["found"], false);
    assert!(none["changes"].as_array().unwrap().is_empty());

    let file = d.claude().join("settings.json");
    let body = json!({
        "model": "claude-sonnet-4-5",
        "permissions": { "defaultMode": "plan", "allow": ["Bash(rm:*)"] },
        "env": { "ANTHROPIC_AUTH_TOKEN": "sk-secret-token" },
        "apiKeyHelper": "echo secret"
    });
    std::fs::write(&file, body.to_string()).unwrap();
    let report = d.ok("settings/importClaude", Value::Null);
    assert_eq!(report["found"], true);
    let changes = report["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2, "{report}");
    assert!(
        changes
            .iter()
            .any(|c| c["key"] == "providers.claude.model" && c["to"] == "claude-sonnet-4-5")
    );
    assert!(
        changes
            .iter()
            .any(|c| c["key"] == "providers.claude.mode" && c["to"] == "plan")
    );
    let whole = report.to_string();
    assert!(
        !whole.contains("sk-secret")
            && !whole.contains("echo secret")
            && !whole.contains("Bash(rm"),
        "{whole}"
    );
    let view = d.ok("settings/get", Value::Null);
    assert_eq!(
        view["settings"]["providers"]["claude"]["model"],
        "claude-sonnet-4-5"
    );
    assert_eq!(view["settings"]["providers"]["claude"]["mode"], "plan");
    // The file of Claude Code stays as it is.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), body.to_string());
    // The same file again changes nothing.
    let again = d.ok("settings/importClaude", Value::Null);
    assert!(again["changes"].as_array().unwrap().is_empty());
    // The bypass mode is not imported.
    std::fs::write(
        &file,
        json!({ "permissions": { "defaultMode": "bypassPermissions" } }).to_string(),
    )
    .unwrap();
    let bypass = d.ok("settings/importClaude", Value::Null);
    assert!(bypass["changes"].as_array().unwrap().is_empty());
    assert!(bypass["skipped"].to_string().contains("bypassPermissions"));
    std::fs::write(&file, "not json").unwrap();
    assert_eq!(d.ok("settings/importClaude", Value::Null)["found"], false);
}

#[test]
fn the_daemon_scales_and_tells_its_host() {
    let d = start("scale");
    let info = d.ok("daemon/info", Value::Null);
    assert!(!info["host"].as_str().unwrap().is_empty());
    assert!(!info["cwd"].as_str().unwrap().is_empty());
    let (mut sub, _) = Subscriber::open(&d.endpoint);
    let a = d.ok(
        "daemon/scale",
        json!({ "daemon": "desktop", "maxRunning": 6 }),
    );
    assert_eq!(a["maxRunning"], 6);
    assert!(matches!(sub.delta(), Delta::Autopilot { autopilot } if autopilot.max_running == 6));
    assert_eq!(d.ok("autopilot/get", Value::Null)["maxRunning"], 6);
    assert_eq!(
        d.err("daemon/scale", json!({ "daemon": "nope", "maxRunning": 6 }))["code"],
        code::NOT_FOUND
    );
    assert_eq!(
        d.err(
            "daemon/scale",
            json!({ "daemon": "desktop", "maxRunning": 0 })
        )["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        d.err(
            "daemon/scale",
            json!({ "daemon": "desktop", "maxRunning": 65 })
        )["code"],
        code::INVALID_PARAMS
    );
}

#[test]
fn a_restart_ends_the_server_and_asks_for_a_new_process() {
    let mut d = start("restart");
    // A remote daemon of the sample answers ok and changes nothing.
    d.ok("daemon/restart", json!({ "daemon": "build-01" }));
    assert!(!d.daemon.is_restart_requested());
    assert_eq!(
        d.err("daemon/restart", json!({ "daemon": "nope" }))["code"],
        code::NOT_FOUND
    );
    d.ok("daemon/info", Value::Null);
    // The local daemon.
    d.ok("daemon/restart", json!({ "daemon": "desktop" }));
    assert!(d.daemon.is_restart_requested());
    assert!(d.daemon.is_shutdown_requested());
    d.server.take().unwrap().join().unwrap().unwrap();
}

#[test]
fn the_demo_revert_adds_a_ledger_turn_and_a_line() {
    let d = start("revert");
    let (mut sub, _) = Subscriber::open(&d.endpoint);
    let before = d.ok("ledger/turns", json!({ "session": "a01c" }));
    assert_eq!(before["turns"].as_array().unwrap().len(), 4);

    let r = d.ok("ledger/revert", json!({ "session": "a01c", "turn": 2 }));
    assert_eq!(r["session"], "a01c");
    assert_eq!(r["turn"], 2);
    assert_eq!(r["restored"], 1);
    assert_eq!(r["snapshot"].as_str().unwrap().len(), 12);
    match sub.delta() {
        Delta::Lines { id, lines, .. } => {
            assert_eq!(id, "a01c");
            assert!(lines[0].text.contains("reverted turn 2"));
        }
        other => panic!("{other:?}"),
    }
    let after = d.ok("ledger/turns", json!({ "session": "a01c" }));
    let turns = after["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 5);
    assert_eq!(turns[4]["n"], 4);
    assert_eq!(turns[4]["prompt"], "Revert turn 2");
    // The revert holds the opposite change of the turn.
    assert_eq!(
        turns[4]["files"][0]["removed"],
        turns[2]["files"][0]["added"]
    );

    assert_eq!(
        d.err("ledger/revert", json!({ "session": "a01c", "turn": 9 }))["code"],
        code::INVALID_STATE
    );
    assert_eq!(
        d.err("ledger/revert", json!({ "session": "a01c", "turn": 0 }))["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        d.err("ledger/revert", json!({ "session": "a7e2", "turn": 1 }))["code"],
        code::INVALID_STATE
    );
    assert_eq!(
        d.err("ledger/revert", json!({ "session": "nope", "turn": 1 }))["code"],
        code::SESSION_NOT_FOUND
    );
}

#[test]
fn a_fork_honours_a_span_and_a_turn() {
    let d = start("fork");
    let old = d.ok("session/history", json!({ "id": "a7e2" }));
    let old_total = old["total"].as_u64().unwrap();

    // The span s3 is turn 2 of the generic trace.
    let by_span = d.ok("session/fork", json!({ "id": "a7e2", "at": "s3" }));
    let id = by_span["id"].as_str().unwrap().to_string();
    let pane = d.view().session(&id).cloned().unwrap();
    assert!(pane.title.contains("turn 2"), "{}", pane.title);
    assert!(pane.prompt.contains("turn 2"));
    // A turn number does the same.
    let by_turn = d.ok("session/fork", json!({ "id": "a7e2", "at": "1" }));
    let turn1 = d
        .view()
        .session(by_turn["id"].as_str().unwrap())
        .cloned()
        .unwrap();
    assert!(turn1.title.contains("turn 1"));
    // Without `at` the fork has no turn.
    let plain = d.ok("session/fork", json!({ "id": "a7e2" }));
    let plain = d
        .view()
        .session(plain["id"].as_str().unwrap())
        .cloned()
        .unwrap();
    assert!(!plain.title.contains("turn"));
    // The history of the fork carries the lines of the old session before its own three.
    let fork_history = d.ok("session/history", json!({ "id": id }));
    assert!(fork_history["total"].as_u64().unwrap() >= 3);
    assert!(fork_history["total"].as_u64().unwrap() <= old_total + 3);
    // The run span has no turn: the fork has none either. An unknown span is an error.
    let run = d.ok("session/fork", json!({ "id": "a7e2", "at": "s1" }));
    let run = d
        .view()
        .session(run["id"].as_str().unwrap())
        .cloned()
        .unwrap();
    assert!(!run.title.contains("turn"));
    assert_eq!(
        d.err("session/fork", json!({ "id": "a7e2", "at": "s99" }))["code"],
        code::INVALID_PARAMS
    );
}

#[test]
fn a_ping_and_a_pause_mark_the_radar() {
    let d = start("radar");
    let list = d.ok("radar/list", json!({}));
    assert!(
        list["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h.get("status").is_none())
    );

    d.ok(
        "radar/ping",
        json!({ "to": "7f3a", "kind": "fyi", "text": "heads up" }),
    );
    let list = d.ok("radar/list", json!({}));
    let hits = list["hits"].as_array().unwrap();
    for hit in hits {
        let mine = hit["a"] == "7f3a" || hit["b"] == "7f3a";
        assert_eq!(hit.get("status").is_some(), mine, "{hit}");
        if mine {
            assert_eq!(hit["status"], "pinged");
        }
    }

    // 8a22 works: it pauses. Its pairs get `paused` and go to the end.
    d.ok("session/pause", json!({ "id": "8a22" }));
    let list = d.ok("radar/list", json!({}));
    let hits = list["hits"].as_array().unwrap();
    let paused: Vec<&Value> = hits.iter().filter(|h| h["status"] == "paused").collect();
    assert_eq!(paused.len(), 2, "{list}");
    assert!(paused.iter().all(|h| h["a"] == "8a22" || h["b"] == "8a22"));
    assert_eq!(hits.last().unwrap()["status"], "paused");
    let node = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "8a22")
        .unwrap();
    assert_eq!(node["state"], "idle");
    // A ping does not undo a pause.
    d.ok(
        "radar/ping",
        json!({ "to": "8a22", "kind": "fyi", "text": "x" }),
    );
    let list = d.ok("radar/list", json!({}));
    assert_eq!(
        list["hits"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["status"] == "paused")
            .count(),
        2
    );

    // The resume takes the pause away.
    d.ok("session/resume", json!({ "id": "8a22" }));
    let list = d.ok("radar/list", json!({}));
    assert!(
        list["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["status"] != "paused")
    );
    let pane = d.view().session("8a22").cloned().unwrap();
    assert_eq!(pane.state, SessionState::Working);
}

#[test]
fn a_block_of_the_sample_can_be_freed() {
    let d = start("block");
    let mail = d.ok("mail/list", json!({ "session": "5e81" }));
    let items = mail["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "block");
    assert_eq!(items[0]["resolved"], false);
    let freed = d.ok("block/override", json!({ "session": "5e81" }));
    assert_eq!(freed["resolved"], 1);
    let mail = d.ok("mail/list", json!({ "session": "5e81" }));
    assert_eq!(mail["items"][0]["resolved"], true);
    assert_eq!(
        d.ok("block/override", json!({ "session": "5e81" }))["resolved"],
        0
    );
    // A new block works too.
    d.ok(
        "radar/ping",
        json!({ "to": "5e81", "kind": "block", "text": "wait", "keys": ["path:a.rs"] }),
    );
    assert_eq!(
        d.ok(
            "block/override",
            json!({ "session": "5e81", "keys": ["path:a.rs"] })
        )["resolved"],
        1
    );
}

#[test]
fn a_card_moves_only_by_the_allowed_moves() {
    let d = start("tasks");
    let tasks = d.ok("tasks/list", json!({}));
    let tasks = tasks["tasks"].as_array().unwrap();
    let planned = tasks.iter().find(|t| t["column"] == "planned").unwrap()["id"].clone();
    let working = tasks.iter().find(|t| t["column"] == "working").unwrap()["id"].clone();

    // Planned does not go to Review, nobody goes to Waiting, Working does not go back.
    let e = d.err("tasks/update", json!({ "id": planned, "column": "review" }));
    assert_eq!(e["code"], code::INVALID_STATE);
    assert!(
        e["message"].as_str().unwrap().contains("Working, Done"),
        "{e}"
    );
    assert_eq!(
        d.err(
            "tasks/update",
            json!({ "id": working, "column": "waiting" })
        )["code"],
        code::INVALID_STATE
    );
    assert_eq!(
        d.err(
            "tasks/update",
            json!({ "id": working, "column": "planned" })
        )["code"],
        code::INVALID_STATE
    );
    // The allowed moves work, and the card keeps its other values.
    let moved = d.ok("tasks/update", json!({ "id": working, "column": "review" }));
    assert_eq!(moved["column"], "review");
    let back = d.ok(
        "tasks/update",
        json!({ "id": working, "column": "working" }),
    );
    assert_eq!(back["column"], "working");
    let done = d.ok("tasks/update", json!({ "id": working, "column": "done" }));
    assert_eq!(done["column"], "done");
    let reopened = d.ok("tasks/update", json!({ "id": working, "column": "review" }));
    assert_eq!(reopened["column"], "review");
    // No move at all is fine.
    d.ok(
        "tasks/update",
        json!({ "id": planned, "column": "planned" }),
    );
    assert_eq!(
        d.err("tasks/update", json!({ "id": "nope", "column": "done" }))["code"],
        code::NOT_FOUND
    );
}

#[test]
fn a_paired_phone_cannot_change_the_machine() {
    use aivoryd::remote::{Caller, as_caller};
    let daemon = Daemon::with_demo(config(Path::new("nowhere")));
    let out = aivoryd::Outbox::new(8).0;
    for method in [
        "repos/add",
        "repos/remove",
        "project/new",
        "map/addPart",
        "providers/addAcp",
        "providers/removeAcp",
        "settings/importClaude",
        "daemon/restart",
    ] {
        let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": {} }).to_string();
        let reply = as_caller(Caller::Device("phone".into()), || {
            daemon.handle_line(&line, &out)
        })
        .unwrap();
        assert_eq!(
            reply["error"]["code"],
            code::NOT_AVAILABLE,
            "{method}: {reply}"
        );
    }
    assert!(!daemon.is_restart_requested());
}
