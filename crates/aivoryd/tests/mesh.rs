//! The machine mesh (ADR 0026) in one process: two daemons with the iroh link on localhost
//! and relays off. Invite and join, both sides store the other one, both dial and stay
//! online, `peers/check` names the direct path, a code works once, and a removed peer is
//! refused.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_link::endpoint::RelayChoice;
use aivory_protocol::mesh::{PeerList, PeerPathKind};
use aivory_sessions::StoreRoots;
use aivoryd::iroh_link::{IrohLink, IrohOptions};
use aivoryd::mesh::Mesh;
use aivoryd::remote::Pairings;
use aivoryd::{Config, Daemon};

const WAIT: Duration = Duration::from_secs(30);

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

struct Machine {
    dir: PathBuf,
    mesh: Arc<Mesh>,
    link: IrohLink,
    daemon: Arc<Daemon>,
    host: Arc<aivoryd::mesh_host::DaemonHost>,
}

impl Machine {
    fn start(name: &str) -> Self {
        Self::start_with(name, false)
    }

    /// A machine with the live backend (the real war room over the fixture sessions).
    fn start_live(name: &str) -> Self {
        Self::start_with(name, true)
    }

    fn start_with(name: &str, live: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("aivory-mesh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pairings = Arc::new(Pairings::load_with(Some(dir.join("devices.json")), None));
        let mesh = Mesh::load(Some(dir.join("peers.json")));
        let files = aivoryd::mesh_sync::FileSync::new(Arc::clone(&mesh), dir.join("mirrors"));
        let base = if live {
            Daemon::with_live(config(), aivoryd::live::LiveOptions::default())
        } else {
            Daemon::with_demo(config())
        };
        let daemon = Arc::new(
            base.with_pairings(Arc::clone(&pairings))
                .with_mesh(Arc::clone(&mesh))
                .with_files(Arc::clone(&files)),
        );
        let host = aivoryd::mesh_host::DaemonHost::new(&daemon);
        mesh.set_host(Arc::clone(&host) as Arc<dyn aivoryd::mesh::MeshHost>);
        let link = IrohLink::start(
            Arc::clone(&daemon),
            pairings,
            IrohOptions {
                secret_file: Some(dir.join("iroh.key")),
                relay: RelayChoice::Disabled,
                bind: Some("127.0.0.1:0".parse().unwrap()),
                mesh: Some(Arc::clone(&mesh)),
            },
        )
        .expect("the iroh link starts");
        files.start();
        Self {
            dir,
            mesh,
            link,
            daemon,
            host,
        }
    }

    /// One request on the local surface, as the desktop app sends it.
    fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let line =
            serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
        let (out, _rx) = aivoryd::outbox::Outbox::new(64);
        self.daemon
            .handle_line(&line.to_string(), &out)
            .expect("a response")
    }

    fn node(&self) -> String {
        self.link.node_id().to_string()
    }

    /// Wait until `ok` holds for the peer list.
    fn wait(&self, what: &str, ok: impl Fn(&PeerList) -> bool) -> PeerList {
        let start = Instant::now();
        loop {
            let list = self.mesh.list();
            if ok(&list) {
                return list;
            }
            assert!(start.elapsed() < WAIT, "{what}: {list:#?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn two_machines_pair_link_and_unpair() {
    let a = Machine::start("a");
    let b = Machine::start("b");
    let c = Machine::start("c");

    let invite = a.mesh.invite().expect("invite");
    assert!(
        invite.url.starts_with("aivory://peer?node="),
        "{}",
        invite.url
    );
    assert!(invite.url.contains("direct=127.0.0.1"), "{}", invite.url);

    // B joins: B stores A at once, A stores B from the hello.
    let peer = b.mesh.join(&invite.url).expect("join");
    assert_eq!(peer.id, a.node());
    assert!(!peer.name.is_empty());
    assert!(a.mesh.list().peers.iter().any(|p| p.id == b.node()));

    // A code works once; an unknown machine is refused.
    let refused = c.mesh.join(&invite.url).unwrap_err();
    assert!(refused.contains("unknown machine"), "{refused}");
    assert!(b.mesh.join(&b.mesh.invite().unwrap().url).is_err());

    // Both sides dial and stay online.
    let online = |id: String| move |l: &PeerList| l.peers.iter().any(|p| p.id == id && p.online);
    b.wait("B sees A online", online(a.node()));
    a.wait("A sees B online", online(b.node()));

    // `peers/check` pings now: a direct path on localhost, with a round-trip time.
    let checked = b.mesh.check(&a.node()[..8]).expect("check");
    assert!(checked.online);
    assert_eq!(checked.path, PeerPathKind::Direct);
    assert!(checked.rtt_ms.is_some());
    assert!(checked.direct.iter().any(|d| d.starts_with("127.0.0.1:")));

    // The pairing survives a restart: it is in peers.json.
    let saved = std::fs::read_to_string(a.dir.join("peers.json")).unwrap();
    assert!(saved.contains(&b.node()));

    // A forgets B: B is refused from now on.
    a.mesh.remove(&b.node()).expect("remove");
    assert!(a.mesh.list().peers.is_empty());
    b.wait("B sees A refuse", |l| {
        l.peers.iter().any(|p| {
            p.id == a.node()
                && !p.online
                && p.last_error
                    .as_deref()
                    .is_some_and(|e| e.contains("unknown machine"))
        })
    });
    assert!(a.mesh.list().peers.is_empty(), "A does not take B back");
}

#[test]
fn a_peer_reports_its_state_and_runs_work() {
    let a = Machine::start("ra");
    let b = Machine::start("rb");
    let invite = a.mesh.invite().expect("invite");
    b.mesh.join(&invite.url).expect("join");
    let a_node = a.node();
    let b_node = b.node();

    // M2: each ping carries the work state of the machine.
    let list = b.wait("B has the state of A", |l| {
        l.peers.iter().any(|p| p.id == a_node && p.state.is_some())
    });
    let state = list.peers[0].state.clone().unwrap();
    assert!(state.max_running > 0, "{state:?}");
    assert!(!state.codebases.is_empty(), "{state:?}");

    // M6: a call on the peer, through the local surface.
    let info = b.rpc(
        "peers/call",
        serde_json::json!({ "id": &a_node[..8], "method": "daemon/info", "params": {} }),
    );
    assert_eq!(info["result"]["name"], "aivoryd", "{info}");

    // A peer cannot reach a method that changes the machine.
    let refused = b.rpc(
        "peers/call",
        serde_json::json!({ "id": a_node, "method": "repos/add", "params": { "path": "/" } }),
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a method"),
        "{refused}"
    );
    let direct = b.mesh.call(&a_node, "settings/get", serde_json::json!({}));
    assert!(direct.is_err());

    // Dispatch on the peer: the ids come back with the node prefix and route back.
    let codebase = state.codebases[0].name.clone();
    let sent = b.rpc(
        "session/dispatch",
        serde_json::json!({ "codebase": codebase, "prompt": "remote work", "node": &a_node[..8] }),
    );
    let result = &sent["result"];
    assert_eq!(result["node"], a_node.as_str(), "{sent}");
    let id = result["session"]
        .as_str()
        .or_else(|| result["task"].as_str())
        .unwrap()
        .to_string();
    assert!(id.starts_with(&format!("@{}/", &a_node[..8])), "{id}");
    if result["session"].is_string() {
        let history = b.rpc("session/history", serde_json::json!({ "id": id }));
        assert!(history.get("result").is_some(), "{history}");
    }
    let _ = b_node;
}

#[test]
fn a_shared_folder_mirrors_live_on_the_peer() {
    let a = Machine::start("fa");
    let b = Machine::start("fb");
    let invite = a.mesh.invite().expect("invite");
    b.mesh.join(&invite.url).expect("join");
    let b_node = b.node();
    a.wait("A sees B online", |l| {
        l.peers.iter().any(|p| p.id == b_node && p.online)
    });

    // A plain folder (no VCS): every file syncs, except the skipped folders.
    let work = a.dir.join("work");
    std::fs::create_dir_all(work.join("src")).unwrap();
    std::fs::create_dir_all(work.join("target")).unwrap();
    std::fs::write(work.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(work.join("target/big.bin"), "skip me").unwrap();
    let big: Vec<u8> = (0..5_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(work.join("data.bin"), &big).unwrap();

    let shared = a.rpc(
        "peers/share",
        serde_json::json!({ "path": work.to_string_lossy(), "name": "work" }),
    );
    assert!(shared.get("result").is_some(), "{shared}");

    let mirror = b.dir.join("mirrors").join(&a.node()[..8]).join("work");
    let wait_file = |rel: &str, want: &[u8]| {
        let start = Instant::now();
        loop {
            if std::fs::read(mirror.join(rel)).is_ok_and(|got| got == want) {
                return;
            }
            assert!(start.elapsed() < WAIT, "{rel} did not arrive");
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    wait_file("src/main.rs", b"fn main() {}\n");
    wait_file("data.bin", &big);
    assert!(!mirror.join("target/big.bin").exists());

    // A change and a delete follow.
    std::fs::write(work.join("src/main.rs"), "fn main() { run(); }\n").unwrap();
    wait_file("src/main.rs", b"fn main() { run(); }\n");
    std::fs::remove_file(work.join("data.bin")).unwrap();
    let start = Instant::now();
    while mirror.join("data.bin").exists() {
        assert!(start.elapsed() < WAIT, "the delete did not arrive");
        std::thread::sleep(Duration::from_millis(200));
    }

    let view = b.rpc("peers/shares", serde_json::json!({}));
    assert_eq!(view["result"]["mirrors"][0]["name"], "work", "{view}");
}

/// The war room of a machine shows the sessions of its online peers: ids `@<node8>/<id>`,
/// `machine` = the peer name. A peer's own copies of other machines are not mirrored back, and
/// an offline peer's sessions leave.
#[test]
fn the_war_room_shows_the_sessions_of_the_peers() {
    let a = Machine::start_live("wa");
    let b = Machine::start("wb");
    let invite = a.mesh.invite().expect("invite");
    b.mesh.join(&invite.url).expect("join");
    let b_node = b.node();
    a.wait("A has B online", |_| a.mesh.online().contains(&b_node));
    let node8 = &b_node[..8];
    let prefix = format!("@{node8}/");
    let sessions = |m: &Machine| -> Vec<serde_json::Value> {
        let snap = m.rpc("warroom/snapshot", serde_json::json!({}));
        snap["result"]["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let b_count = sessions(&b).len();
    assert!(b_count > 0, "the demo machine has sessions");

    a.host.mirror_once(&a.mesh);
    let all = sessions(&a);
    let remote: Vec<&serde_json::Value> = all
        .iter()
        .filter(|p| p["id"].as_str().unwrap_or_default().starts_with(&prefix))
        .collect();
    assert_eq!(remote.len(), b_count, "{all:#?}");
    assert!(
        remote.iter().all(|p| p["machine"].is_string()),
        "{remote:#?}"
    );
    let board = a.rpc("warroom/snapshot", serde_json::json!({}))["result"]["board"]["asks"].clone();
    assert!(
        board
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id.as_str().unwrap().starts_with(&prefix)),
        "the asks of the peer join the board: {board}"
    );

    // A pane that is itself a copy of another machine never comes back.
    let live = a.daemon.live_backend().expect("live");
    let mut copy: aivory_protocol::Pane = serde_json::from_value(remote[0].clone()).unwrap();
    copy.id = "@ffffffff/x".to_string();
    live.merge_peer(node8, "wb", Some(vec![copy]));
    assert!(
        sessions(&a)
            .iter()
            .all(|p| !p["id"].as_str().unwrap().starts_with(&prefix)),
        "only copies: the peer has no own sessions now"
    );

    // Offline: the sessions of the peer leave.
    a.host.mirror_once(&a.mesh);
    assert_eq!(
        sessions(&a)
            .iter()
            .filter(|p| p["id"].as_str().unwrap().starts_with(&prefix))
            .count(),
        b_count
    );
    live.merge_peer(node8, "", None);
    assert!(sessions(&a).iter().all(|p| p["machine"].is_null()));
}

/// `machines/list`: this machine first, then each peer with the look that it sent in its ping.
#[test]
fn machines_list_shows_the_look_of_each_machine() {
    let a = Machine::start("la");
    let b = Machine::start("lb");
    let alone = a.rpc("machines/list", serde_json::json!({}));
    let list = alone["result"]["machines"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{alone}");
    assert_eq!(list[0]["local"], true);
    assert_eq!(list[0]["online"], true);
    assert_eq!(list[0]["id"], a.node());
    assert!(!list[0]["name"].as_str().unwrap().is_empty());

    let invite = a.mesh.invite().expect("invite");
    b.mesh.join(&invite.url).expect("join");
    let b_node = b.node();
    a.wait("A has B online", |_| a.mesh.online().contains(&b_node));

    let set = b.rpc(
        "settings/set",
        serde_json::json!({ "key": "display.look", "value": "holo" }),
    );
    assert!(set["error"].is_null(), "{set}");
    let own = b.rpc("settings/get", serde_json::json!({}))["result"]["settings"]["display"]["look"]
        .clone();
    assert_eq!(own, "holo");

    let start = Instant::now();
    loop {
        a.rpc("peers/check", serde_json::json!({ "id": b_node }));
        let r = a.rpc("machines/list", serde_json::json!({}));
        let list = r["result"]["machines"].as_array().unwrap().clone();
        assert_eq!(list.len(), 2, "{r}");
        assert_eq!(list[0]["local"], true);
        if list[1]["look"] == "holo" {
            assert_eq!(list[1]["id"], b_node);
            assert_eq!(list[1]["online"], true);
            assert_eq!(list[1]["local"], false);
            break;
        }
        assert!(start.elapsed() < WAIT, "{r}");
        std::thread::sleep(Duration::from_millis(200));
    }
}
