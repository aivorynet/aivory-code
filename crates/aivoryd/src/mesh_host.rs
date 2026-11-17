//! The daemon side of the machine mesh (ADR 0026, steps M2 and M6).
//!
//! - [`DaemonHost`] gives the mesh the work state of this machine for each ping, and runs the
//!   calls of peers (`mesh/call`) on the local surface as [`Caller::Peer`].
//! - [`route`] sends a local request to a peer: `session/new` and `session/dispatch` with a
//!   `node` (`auto` or a peer id), and every request whose `id` names a remote session
//!   (`@<node8>/<id>`).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use aivory_protocol::WarRoom;
use aivory_protocol::mesh::{NodeCodebase, NodeState, VcsSyncRow, VcsSyncView};
use serde_json::{Value, json};

use crate::mesh::{Mesh, MeshHost};
use crate::outbox::Outbox;
use crate::remote::{Caller, as_caller};
use crate::rpc::Daemon;

/// The prefix of a session id that lives on a peer: `@<first 8 hex of the node id>/<id>`.
pub const REMOTE_PREFIX: char = '@';

const PROVIDERS: [&str; 3] = ["claude", "codex", "gemini"];

/// How often the war room mirrors the sessions of the online peers.
const ROOM_MIRROR_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

/// The mesh host over the daemon.
#[derive(Debug)]
pub struct DaemonHost {
    daemon: Weak<Daemon>,
    /// Codebase path -> identity (a repository identity does not change).
    identities: Mutex<HashMap<String, (String, Option<String>)>>,
    /// The installed providers, found once.
    providers: Vec<String>,
    /// The auto-sync rows by codebase (step M4).
    vcs: Mutex<Vec<VcsSyncRow>>,
    vcs_interval: std::time::Duration,
}

impl DaemonHost {
    pub fn new(daemon: &Arc<Daemon>) -> Arc<Self> {
        let providers = PROVIDERS
            .iter()
            .filter(|p| aivory_agents::resolve_program(Path::new(p)).is_ok())
            .map(|p| (*p).to_string())
            .collect();
        Arc::new(Self {
            daemon: Arc::downgrade(daemon),
            identities: Mutex::new(HashMap::new()),
            providers,
            vcs: Mutex::new(Vec::new()),
            vcs_interval: vcs_interval(),
        })
    }

    /// Run the auto-sync loop (step M4) on `handle`: every interval each codebase checkout
    /// updates from its VCS, fast-forward only. A codebase with a running session waits, so an
    /// agent never sees its files change during a turn. `AIVORY_VCS_SYNC=<seconds>` sets the
    /// interval (default 120, 0 turns it off).
    pub fn start_autosync(self: &Arc<Self>, handle: &tokio::runtime::Handle) {
        if self.vcs_interval.is_zero() {
            return;
        }
        let host = Arc::clone(self);
        handle.spawn(async move {
            loop {
                tokio::time::sleep(host.vcs_interval).await;
                let worker = Arc::clone(&host);
                let _ = tokio::task::spawn_blocking(move || worker.sync_once()).await;
            }
        });
    }

    /// Mirror the sessions of the online peers into the war room (every 3 s): the phone and
    /// the desktop then show all machines of the mesh. A peer that goes offline loses its
    /// sessions here at the next round.
    pub fn start_room_mirror(self: &Arc<Self>, mesh: &Arc<Mesh>, handle: &tokio::runtime::Handle) {
        let host = Arc::clone(self);
        let mesh = Arc::clone(mesh);
        handle.spawn(async move {
            loop {
                tokio::time::sleep(ROOM_MIRROR_EVERY).await;
                let worker = Arc::clone(&host);
                let mesh = Arc::clone(&mesh);
                let _ = tokio::task::spawn_blocking(move || worker.mirror_once(&mesh)).await;
            }
        });
    }

    /// One mirror round: fetch `warroom/snapshot` of each online peer and merge its sessions.
    pub fn mirror_once(&self, mesh: &Arc<Mesh>) {
        let Some(daemon) = self.daemon.upgrade() else {
            return;
        };
        let Some(live) = daemon.live_backend() else {
            return;
        };
        let list = mesh.list();
        let online = mesh.online();
        let mut seen = Vec::new();
        for peer in &list.peers {
            let node8 = peer.id[..8.min(peer.id.len())].to_string();
            if !online.contains(&peer.id) {
                continue;
            }
            match mesh.call(&peer.id, "warroom/snapshot", json!({})) {
                Ok(value) => {
                    if let Ok(war) = serde_json::from_value::<WarRoom>(value) {
                        let name = if peer.name.is_empty() {
                            node8.clone()
                        } else {
                            peer.name.clone()
                        };
                        live.merge_peer(&node8, &name, Some(war.sessions));
                        seen.push(node8);
                    }
                }
                Err(e) => eprintln!("aivoryd: mesh room of {node8}: {e}"),
            }
        }
        for node8 in live.peer_prefixes() {
            if !seen.contains(&node8) {
                live.merge_peer(&node8, "", None);
            }
        }
    }

    /// One auto-sync round over all codebases.
    pub fn sync_once(&self) {
        let Some(daemon) = self.daemon.upgrade() else {
            return;
        };
        let Ok(snapshot) = local_call(&daemon, Caller::Local, "warroom/snapshot", json!({})) else {
            return;
        };
        let Ok(war) = serde_json::from_value::<WarRoom>(snapshot) else {
            return;
        };
        let mut rows = Vec::new();
        for c in &war.codebases {
            let busy = war.sessions.iter().any(|s| {
                s.codebase == c.id
                    && matches!(
                        s.state,
                        aivory_protocol::SessionState::Working
                            | aivory_protocol::SessionState::NeedsYou
                    )
            });
            rows.push(sync_codebase(&c.id, Path::new(&c.path), busy));
        }
        *self.vcs.lock().unwrap_or_else(PoisonError::into_inner) = rows;
    }

    fn identity(&self, path: &str) -> (String, Option<String>) {
        let mut cache = self
            .identities
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache
            .entry(path.to_string())
            .or_insert_with(|| repo_identity(Path::new(path)))
            .clone()
    }
}

impl MeshHost for DaemonHost {
    fn state(&self) -> NodeState {
        let Some(daemon) = self.daemon.upgrade() else {
            return NodeState::default();
        };
        let Ok(snapshot) = local_call(&daemon, Caller::Local, "warroom/snapshot", json!({})) else {
            return NodeState::default();
        };
        let Ok(war) = serde_json::from_value::<WarRoom>(snapshot) else {
            return NodeState::default();
        };
        let running = war
            .sessions
            .iter()
            .filter(|s| {
                matches!(
                    s.state,
                    aivory_protocol::SessionState::Working
                        | aivory_protocol::SessionState::NeedsYou
                )
            })
            .count();
        let look = local_look(&daemon);
        NodeState {
            look,
            running: u32::try_from(running).unwrap_or(u32::MAX),
            max_running: war.autopilot.max_running,
            queued: war.queued,
            providers: self.providers.clone(),
            codebases: war
                .codebases
                .iter()
                .map(|c| {
                    let (identity, remote) = self.identity(&c.path);
                    NodeCodebase {
                        name: c.id.clone(),
                        identity,
                        remote,
                    }
                })
                .collect(),
        }
    }

    fn vcs(&self) -> VcsSyncView {
        VcsSyncView {
            interval_secs: self.vcs_interval.as_secs(),
            rows: self
                .vcs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        }
    }

    fn call(&self, peer: &str, method: &str, params: Value) -> Result<Value, (i64, String)> {
        let daemon = self
            .daemon
            .upgrade()
            .ok_or((-32603, "the daemon stops".to_string()))?;
        local_call(&daemon, Caller::Peer(peer.to_string()), method, params)
    }
}

/// The `display.look` of this machine, as `settings/get` gives it. Empty: unknown.
fn local_look(daemon: &Daemon) -> String {
    local_call(daemon, Caller::Local, "settings/get", json!({}))
        .ok()
        .and_then(|v| {
            v["settings"]["display"]["look"]
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Run one request on the local surface as `caller`.
fn local_call(
    daemon: &Daemon,
    caller: Caller,
    method: &str,
    params: Value,
) -> Result<Value, (i64, String)> {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (out, _rx) = Outbox::new(64);
    let response = as_caller(caller, || daemon.handle_line(&line.to_string(), &out))
        .ok_or((-32603, "no response".to_string()))?;
    if let Some(error) = response.get("error") {
        return Err((
            error["code"].as_i64().unwrap_or(-32603),
            error["message"].as_str().unwrap_or("error").to_string(),
        ));
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

/// Send the request to a peer when it names one. `None`: run it here.
pub fn route(mesh: &Arc<Mesh>, method: &str, params: &Value) -> Option<Result<Value, String>> {
    // A remote session id: `@<node8>/<id>`.
    if let Some(id) = params.get("id").and_then(Value::as_str)
        && let Some((node, rest)) = split_remote(id)
    {
        let mut params = params.clone();
        params["id"] = json!(rest);
        return Some(
            mesh.resolve_peer(node)
                .and_then(|peer| mesh.call(&peer, method, params)),
        );
    }
    if !matches!(method, "session/new" | "session/dispatch") {
        return None;
    }
    let node = params.get("node").and_then(Value::as_str)?.trim();
    let peer = match node {
        "" | "local" => None,
        "auto" => {
            let codebase = params["codebase"].as_str().unwrap_or_default();
            let provider = params["composer"]["provider"]
                .as_str()
                .or_else(|| params["agent"].as_str())
                .unwrap_or("claude");
            mesh.pick(codebase, provider)
        }
        id => match mesh.resolve_peer(id) {
            Ok(peer) => Some(peer),
            Err(e) => return Some(Err(e)),
        },
    };
    // `None`: this machine runs it; the local handlers ignore `node`.
    let peer = peer?;
    let mut params = params.clone();
    if let Some(map) = params.as_object_mut() {
        map.remove("node");
    }
    Some(mesh.call(&peer, method, params).map(|mut result| {
        let short = &peer[..8.min(peer.len())];
        for key in ["id", "session", "task"] {
            if let Some(id) = result.get(key).and_then(Value::as_str) {
                result[key] = json!(format!("{REMOTE_PREFIX}{short}/{id}"));
            }
        }
        result["node"] = json!(peer);
        result
    }))
}

fn vcs_interval() -> std::time::Duration {
    let secs = std::env::var("AIVORY_VCS_SYNC")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(120);
    std::time::Duration::from_secs(secs)
}

/// Update one checkout from its VCS, fast-forward only.
pub fn sync_codebase(codebase: &str, path: &Path, busy: bool) -> VcsSyncRow {
    let mut row = VcsSyncRow {
        codebase: codebase.to_string(),
        path: path.to_string_lossy().into_owned(),
        last: Some(crate::remote::now_ms()),
        ..VcsSyncRow::default()
    };
    let vcs = crate::mesh_sync::own_vcs_root(path)
        .then(|| aivory_vcs::detect(path))
        .flatten();
    let Some(vcs) = vcs else {
        row.note = Some("no VCS in this folder".into());
        return row;
    };
    row.kind = kind_name(vcs.kind()).to_string();
    if busy {
        row.note = Some("a session works here; the update waits".into());
        return row;
    }
    match vcs.is_clean(path) {
        Ok(true) => {}
        Ok(false) => {
            row.note = Some("local changes; no update".into());
            return row;
        }
        Err(e) => {
            row.note = Some(e.to_string());
            return row;
        }
    }
    match vcs.update(path) {
        Ok(changed) => row.changed = changed,
        Err(e) => row.note = Some(e.to_string()),
    }
    row
}

fn kind_name(kind: aivory_vcs::VcsKind) -> &'static str {
    match kind {
        aivory_vcs::VcsKind::Git => "git",
        aivory_vcs::VcsKind::Hg => "hg",
        aivory_vcs::VcsKind::Jj => "jj",
        aivory_vcs::VcsKind::Svn => "svn",
        aivory_vcs::VcsKind::P4 => "p4",
    }
}

/// `@<node8>/<id>` -> (`<node8>`, `<id>`).
pub fn split_remote(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(REMOTE_PREFIX)?.split_once('/')
}

/// The identity of the repository at `path`: `git:<root commit>` with the `origin` URL.
fn repo_identity(path: &Path) -> (String, Option<String>) {
    if !crate::mesh_sync::own_vcs_root(path) {
        return (String::new(), None);
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let root = git(&["rev-list", "--max-parents=0", "HEAD"])
        .and_then(|out| out.lines().last().map(str::to_string));
    let remote = git(&["remote", "get-url", "origin"]);
    (root.map(|r| format!("git:{r}")).unwrap_or_default(), remote)
}
