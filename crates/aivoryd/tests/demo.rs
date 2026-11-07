//! `aivoryd serve --demo` over the local socket: snapshot, subscription with deltas, answers,
//! catch-up after a reconnect.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use aivory_protocol::rpc::{self, Subscription, code};
use aivory_protocol::{SessionState, WarRoom};
use aivory_sessions::StoreRoots;
use aivoryd::endpoint::{self, Connection};
use aivoryd::{Config, Daemon, Endpoint, Server, client};
use serde_json::{Value, json};

fn config() -> Config {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../aivory-sessions/tests/fixtures");
    Config {
        roots: StoreRoots {
            claude: base.join("claude"),
            codex: base.join("codex"),
            gemini: base.join("gemini"),
        },
        cache_file: None,
    }
}

fn start(name: &str, daemon: Daemon) -> (Endpoint, thread::JoinHandle<std::io::Result<()>>) {
    let endpoint = Endpoint::new(&format!("test-{name}-{}", std::process::id())).unwrap();
    let server = Server::bind(Arc::new(daemon), endpoint.clone()).expect("bind");
    (endpoint, thread::spawn(move || server.run()))
}

fn call(endpoint: &Endpoint, method: &str, params: Value) -> Value {
    client::call(endpoint, method, params, &mut |_| {}).unwrap()
}

/// A connection that stays open, like a UI.
struct Subscriber {
    conn: Connection,
    line: String,
}

impl Subscriber {
    fn open(endpoint: &Endpoint, params: Value) -> (Self, Subscription) {
        let mut conn = endpoint::connect(endpoint).unwrap();
        let req =
            json!({ "jsonrpc": "2.0", "id": 1, "method": "warroom/subscribe", "params": params });
        writeln!(conn.writer, "{req}").unwrap();
        conn.writer.flush().unwrap();
        let mut s = Self {
            conn,
            line: String::new(),
        };
        let response = s.next();
        assert_eq!(response["id"], 1, "the response comes first: {response}");
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
}

#[test]
fn demo_feed_answers_and_catch_up() {
    let (endpoint, server) = start("demo", Daemon::with_demo(config()));

    let info = call(&endpoint, "daemon/info", Value::Null);
    assert_eq!(info["result"]["demo"], true);
    assert_eq!(
        info["result"]["contract"],
        aivory_protocol::PROTOCOL_VERSION
    );

    let snapshot: WarRoom =
        serde_json::from_value(call(&endpoint, "warroom/snapshot", json!({}))["result"].clone())
            .unwrap();
    assert_eq!(snapshot.sessions.len(), 24);
    assert_eq!(snapshot.counts().needs_you, 3);

    let (mut ui, sub) = Subscriber::open(&endpoint, json!({}));
    let mut view = sub.snapshot.expect("a fresh subscription gets a snapshot");
    assert_eq!(sub.epoch, view.epoch);

    // A stale ask id is refused.
    let stale = call(
        &endpoint,
        "session/answer",
        json!({ "id": "7f3a", "ask": "old", "option": "Allow" }),
    );
    assert_eq!(stale["error"]["code"], code::STALE_ASK);

    // Answer the permission; the open UI sees the session go from needs you to working.
    let ok = call(
        &endpoint,
        "session/answer",
        json!({ "id": "7f3a", "ask": "7f3a-ask-1", "option": "Allow" }),
    );
    assert_eq!(ok["result"]["ok"], true, "{ok}");
    for _ in 0..3 {
        let note = ui.next();
        assert_eq!(note["method"], rpc::WARROOM_DELTA);
        let note: rpc::DeltaNote = serde_json::from_value(note["params"].clone()).unwrap();
        view.apply(note.seq, &note.delta);
    }
    let pane = view.session("7f3a").unwrap();
    assert_eq!(pane.state, SessionState::Working);
    assert!(pane.ask.is_none());
    assert_eq!(
        pane.tail.last().unwrap().text,
        "OK. I continue with: allow."
    );
    assert_eq!(view.inbox_open, 5);
    assert_eq!(view.counts().needs_you, 2);

    // The inbox moved the item to Done.
    let inbox = call(&endpoint, "inbox/list", json!({}));
    assert_eq!(inbox["result"]["items"].as_array().unwrap().len(), 5);
    assert_eq!(inbox["result"]["done"][0]["result"], "allowed");

    // Inbox answer of the plan; the history pages hold the answer.
    let item = call(
        &endpoint,
        "inbox/answer",
        json!({ "id": "in-c218", "option": "Approve" }),
    );
    assert_eq!(item["result"]["result"], "approved");
    let page = call(
        &endpoint,
        "session/history",
        json!({ "id": "c218", "limit": 2 }),
    );
    assert_eq!(page["result"]["lines"][0]["text"], "Approve");
    assert_eq!(
        page["result"]["start"],
        page["result"]["total"].as_u64().unwrap() - 2
    );

    // A UI that reconnects with its cursor gets only the missed deltas.
    let (_, catch_up) = Subscriber::open(
        &endpoint,
        json!({ "cursor": view.cursor, "epoch": view.epoch }),
    );
    assert!(catch_up.snapshot.is_none());
    for note in &catch_up.deltas {
        view.apply(note.seq, &note.delta);
    }
    assert_eq!(view.cursor, catch_up.cursor);
    assert_eq!(view.session("c218").unwrap().state, SessionState::Working);
    // A cursor of another epoch gets a snapshot.
    let (_, fresh) = Subscriber::open(&endpoint, json!({ "cursor": 1, "epoch": 7 }));
    assert!(fresh.snapshot.is_some());

    // Session controls, a new task, settings.
    assert_eq!(
        call(&endpoint, "session/pause", json!({ "id": "a01c" }))["result"]["ok"],
        true
    );
    let again = call(&endpoint, "session/pause", json!({ "id": "a01c" }));
    assert_eq!(again["error"]["code"], code::INVALID_STATE);
    assert_eq!(
        call(&endpoint, "session/resume", json!({ "id": "o2c4" }))["result"]["ok"],
        true
    );
    let task = call(
        &endpoint,
        "tasks/create",
        json!({ "title": "Retry budget", "codebase": "billing", "part": "payment", "playbook": "fix", "prompt": "cap the retries", "when": "now" }),
    );
    let new_session = task["result"]["sessions"][0]["session"]
        .as_str()
        .unwrap()
        .to_string();
    let spans = call(&endpoint, "session/spans", json!({ "id": new_session }));
    assert_eq!(spans["result"]["spans"][0]["kind"], "run");
    let settings = call(
        &endpoint,
        "settings/set",
        json!({ "key": "autopilot.maxRunning", "value": 20 }),
    );
    assert_eq!(settings["result"]["autopilot"]["maxRunning"], 20);
    assert_eq!(
        call(&endpoint, "autopilot/get", json!({}))["result"]["maxRunning"],
        20
    );
    let a01c = call(&endpoint, "session/spans", json!({ "id": "a01c" }));
    assert_eq!(a01c["result"]["spans"].as_array().unwrap().len(), 21);
    assert_eq!(
        call(&endpoint, "repos/get", json!({ "id": "nope" }))["error"]["code"],
        code::NOT_FOUND
    );

    assert_eq!(
        call(&endpoint, "daemon/shutdown", Value::Null)["result"]["ok"],
        true
    );
    server.join().unwrap().unwrap();
}

#[test]
fn every_method_is_served_in_demo_mode() {
    let (endpoint, server) = start("methods", Daemon::with_demo(config()));
    for &name in rpc::METHODS {
        if name == "daemon/shutdown" || name == "warroom/subscribe" {
            continue;
        }
        let r = call(&endpoint, name, json!({}));
        assert_ne!(r["error"]["code"], code::METHOD_NOT_FOUND, "{name}");
    }
    call(&endpoint, "daemon/shutdown", Value::Null);
    server.join().unwrap().unwrap();
}

#[test]
fn war_room_needs_demo_mode_for_now() {
    let (endpoint, server) = start("live", Daemon::new(config()));
    let r = call(&endpoint, "warroom/snapshot", json!({}));
    assert_eq!(r["error"]["code"], code::NOT_AVAILABLE);
    assert_eq!(
        call(&endpoint, "daemon/info", Value::Null)["result"]["demo"],
        false
    );
    call(&endpoint, "daemon/shutdown", Value::Null);
    server.join().unwrap().unwrap();
}
