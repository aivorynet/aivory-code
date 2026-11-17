//! The machine mesh: this daemon and the daemons of the same user on other machines
//! (ADR 0026). This file holds step M1: the peer link.
//!
//! - **Trust.** `peers.json` next to `aivory.db` keeps the paired machines by node id (the
//!   iroh public key). The QUIC handshake proves the key, so a peer needs no token.
//! - **Pairing.** `peers/invite` gives a one-time link (`aivory://peer?...`, 10 minutes).
//!   `peers/join` on the other machine dials with the code of the link. Both sides store the
//!   other key: the inviter from the hello, the joiner from the handshake.
//! - **Connections.** The iroh endpoint of `serve --iroh` accepts the ALPN
//!   [`aivory_link::MESH_ALPN`] next to the phone link. Each daemon also dials each peer and
//!   keeps that connection open (reconnect with backoff). The first stream carries
//!   `mesh/hello`; each later request has a stream of its own.
//! - **Addresses.** A hello and each `mesh/ping` (every 15 s) carry the current direct
//!   addresses and the home relay of the sender. The receiver stores them, so the next dial
//!   finds the peer without a lookup service.
//! - **Scope.** A peer reaches only the `mesh/*` methods of this file, never the JSON-RPC
//!   surface of the local client or of a phone.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use aivory_link::endpoint::addr_of;
use aivory_link::frame::{read_frame, write_frame};
use aivory_link::hello::{
    MESH_HELLO_METHOD, MESH_PING_METHOD, MeshHello, MeshHelloReply, UNAUTHORIZED,
};
use aivory_link::iroh::Endpoint;
use aivory_link::iroh::endpoint::{Connection, RecvStream, SendStream};
use aivory_link::{MESH_ALPN, PathKind, PeerLink, path_of};
use aivory_protocol::UnixMs;
use aivory_protocol::mesh::{
    MeshInfo, NodeState, PEER_METHODS, Peer, PeerInvite, PeerList, PeerPathKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::time::timeout;

use crate::remote::{now_ms, random_hex};

const INVITE_MINUTES: i64 = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
const PING_EVERY: Duration = Duration::from_secs(15);
const RECHECK: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const CLOSE_BYE: u32 = 0;
const CLOSE_REMOVED: u32 = 2;

/// The method on a mesh connection that runs one method of [`PEER_METHODS`] (step M6).
pub const MESH_CALL_METHOD: &str = "mesh/call";

/// What the mesh needs from the daemon: the work state for the ping, and the calls of peers.
pub trait MeshHost: Send + Sync {
    /// The work state of this machine (step M2).
    fn state(&self) -> NodeState;
    /// Run one method of [`PEER_METHODS`] as the peer `peer`. The error is a JSON-RPC code and
    /// a message.
    fn call(&self, peer: &str, method: &str, params: Value) -> Result<Value, (i64, String)>;
    /// The auto-sync state of the codebases (step M4).
    fn vcs(&self) -> aivory_protocol::mesh::VcsSyncView {
        aivory_protocol::mesh::VcsSyncView::default()
    }
}

/// The receiver of the file methods of peers (`mesh/put`, `mesh/del`, step M5).
pub trait MeshFiles: Send + Sync {
    fn handle(&self, peer: &str, method: &str, params: Value) -> Result<Value, String>;
}

/// The paired machines and the state of their connections.
pub struct Mesh {
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
    link: OnceLock<LinkHandle>,
    host: OnceLock<Arc<dyn MeshHost>>,
    files: OnceLock<Arc<dyn MeshFiles>>,
}

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mesh")
            .field("file", &self.file)
            .field("attached", &self.link.get().is_some())
            .finish_non_exhaustive()
    }
}

struct LinkHandle {
    endpoint: Endpoint,
    handle: Handle,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    /// Node id -> peer (only the stored fields; the live fields are blank).
    peers: BTreeMap<String, Peer>,
}

#[derive(Default)]
struct Inner {
    saved: Saved,
    /// Open invite codes -> expiry.
    codes: HashMap<String, UnixMs>,
    /// Live state by node id.
    live: HashMap<String, Live>,
    /// Peers with a running dial loop.
    dialing: HashSet<String>,
    me: Me,
}

#[derive(Default, Clone)]
struct Me {
    node: String,
    relay: Option<String>,
    direct: Vec<String>,
}

#[derive(Default)]
struct Live {
    /// The outbound connection of the dial loop.
    conn: Option<Connection>,
    /// Open inbound connections from the peer.
    inbound: u32,
    rtt_ms: Option<u64>,
    last_error: Option<String>,
    /// The work state of the last ping.
    state: Option<NodeState>,
}

impl Mesh {
    /// Load `peers.json`. `None`: keep the peers in memory only (tests).
    pub fn load(file: Option<PathBuf>) -> Arc<Self> {
        let saved = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|text| serde_json::from_str::<Saved>(&text).ok())
            .unwrap_or_default();
        Arc::new(Self {
            file,
            inner: Mutex::new(Inner {
                saved,
                ..Inner::default()
            }),
            link: OnceLock::new(),
            host: OnceLock::new(),
            files: OnceLock::new(),
        })
    }

    /// Give the mesh the iroh endpoint of the daemon and start a dial loop for each peer.
    /// Call it once, from the runtime of the endpoint.
    pub fn attach(self: &Arc<Self>, endpoint: Endpoint, handle: Handle) {
        let node = endpoint.id().to_string();
        self.lock().me.node = node;
        if self.link.set(LinkHandle { endpoint, handle }).is_err() {
            return;
        }
        let ids: Vec<String> = self.lock().saved.peers.keys().cloned().collect();
        for id in ids {
            self.start_dialer(&id);
        }
    }

    /// Give the mesh the daemon: its work state and the calls of peers.
    pub fn set_host(&self, host: Arc<dyn MeshHost>) {
        let _ = self.host.set(host);
    }

    /// `peers/vcs`.
    pub fn vcs(&self) -> aivory_protocol::mesh::VcsSyncView {
        self.host.get().map(|h| h.vcs()).unwrap_or_default()
    }

    /// Give the mesh the receiver of the file methods.
    pub fn set_files(&self, files: Arc<dyn MeshFiles>) {
        let _ = self.files.set(files);
    }

    /// The node id of this daemon (empty before [`Mesh::attach`]).
    pub fn node(&self) -> String {
        self.lock().me.node.clone()
    }

    /// The runtime of the endpoint, after [`Mesh::attach`].
    pub fn runtime(&self) -> Option<Handle> {
        self.link.get().map(|l| l.handle.clone())
    }

    /// The paired peers with an open outbound connection.
    pub fn online(&self) -> Vec<String> {
        let inner = self.lock();
        inner
            .live
            .iter()
            .filter(|(id, l)| inner.saved.peers.contains_key(*id) && l.conn.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// One request to a peer on the open connection (async).
    pub async fn request(&self, id: &str, method: &str, params: Value) -> Result<Value, String> {
        let conn = self
            .lock()
            .live
            .get(id)
            .and_then(|l| l.conn.clone())
            .ok_or_else(|| format!("peer {} is offline", short(id)))?;
        let request = json!({ "jsonrpc": "2.0", "id": 3, "method": method, "params": params });
        call_stream(&conn, &request).await
    }

    /// `peers/call`: run one method of [`PEER_METHODS`] on a peer.
    pub fn call(self: &Arc<Self>, id: &str, method: &str, params: Value) -> Result<Value, String> {
        if !PEER_METHODS.contains(&method) {
            return Err(format!("'{method}' is not a method that a peer runs"));
        }
        let id = self.resolve(id)?;
        let handle = self.attached()?.handle.clone();
        let conn = self
            .lock()
            .live
            .get(&id)
            .and_then(|l| l.conn.clone())
            .ok_or_else(|| format!("peer {} is offline", short(&id)))?;
        let request = json!({
            "jsonrpc": "2.0", "id": 2, "method": MESH_CALL_METHOD,
            "params": { "method": method, "params": params },
        });
        handle.block_on(async move { call_stream(&conn, &request).await })
    }

    /// The best machine for a new session of `provider` in `codebase` (step M6): the online
    /// peer with the most free slots that has the provider and the codebase (by name or by
    /// repository identity). `None`: this machine (it wins a tie).
    pub fn pick(&self, codebase: &str, provider: &str) -> Option<String> {
        let local = self.host.get().map(|h| h.state()).unwrap_or_default();
        let identity = local
            .codebases
            .iter()
            .find(|c| c.name == codebase)
            .map(|c| c.identity.clone())
            .filter(|i| !i.is_empty());
        let fits = |st: &NodeState| {
            st.providers.iter().any(|p| p == provider)
                && st.codebases.iter().any(|c| {
                    c.name == codebase || identity.as_deref().is_some_and(|i| i == c.identity)
                })
        };
        let local_free = if fits(&local) || local.codebases.is_empty() {
            local.free()
        } else {
            0
        };
        let inner = self.lock();
        inner
            .live
            .iter()
            .filter(|(id, l)| inner.saved.peers.contains_key(*id) && l.conn.is_some())
            .filter_map(|(id, l)| l.state.as_ref().map(|st| (id, st)))
            .filter(|(_, st)| fits(st) && st.free() > local_free)
            .max_by_key(|(_, st)| st.free())
            .map(|(id, _)| id.clone())
    }

    /// The node id of a peer from a full id or a unique prefix.
    pub fn resolve_peer(&self, id: &str) -> Result<String, String> {
        self.resolve(id)
    }

    /// The addresses that the hello and the ping name from now on.
    pub fn set_addrs(&self, relay: Option<&str>, direct: &[String]) {
        let mut inner = self.lock();
        if relay.is_some() {
            inner.me.relay = relay.map(str::to_string);
        }
        if !direct.is_empty() {
            inner.me.direct = direct.to_vec();
        }
    }

    /// `peers/invite`.
    pub fn invite(&self) -> Result<PeerInvite, String> {
        self.attached()?;
        let code = format!("AIV-{}", random_hex(3).to_uppercase());
        let expires = now_ms() + INVITE_MINUTES * 60_000;
        let me = {
            let mut inner = self.lock();
            inner.codes.retain(|_, e| *e >= now_ms());
            inner.codes.insert(code.clone(), expires);
            inner.me.clone()
        };
        let link = PeerLink {
            node: me.node,
            relay: me.relay,
            direct: me.direct,
            code: code.clone(),
            name: Some(crate::actions::host_name()),
        };
        Ok(PeerInvite {
            code,
            url: link.to_url(),
            expires,
        })
    }

    /// `peers/join`: dial the inviter with the code of its link and store it as a peer.
    pub fn join(self: &Arc<Self>, link: &str) -> Result<Peer, String> {
        let link = PeerLink::parse(link).map_err(|e| e.to_string())?;
        let handle = self.attached()?.handle.clone();
        if link.node == self.lock().me.node {
            return Err("this is the link of this machine".into());
        }
        let mesh = Arc::clone(self);
        let node = link.node.clone();
        let conn = handle.block_on(async move {
            mesh.dial(
                &link.node,
                link.relay.as_deref(),
                &link.direct,
                Some(link.code.clone()),
            )
            .await
        })?;
        conn.close(CLOSE_BYE.into(), b"joined");
        self.start_dialer(&node);
        self.peer(&node)
            .ok_or_else(|| "the peer is gone".to_string())
    }

    /// `peers/list`.
    pub fn list(&self) -> PeerList {
        let inner = self.lock();
        PeerList {
            node: inner.me.node.clone(),
            peers: inner
                .saved
                .peers
                .values()
                .map(|p| with_live(p, inner.live.get(&p.id)))
                .collect(),
        }
    }

    /// `peers/check`: ping the peer now on the open connection. Without a connection the
    /// result shows the last error.
    pub fn check(self: &Arc<Self>, id: &str) -> Result<Peer, String> {
        let id = self.resolve(id)?;
        let handle = self.attached()?.handle.clone();
        let conn = self.lock().live.get(&id).and_then(|l| l.conn.clone());
        if let Some(conn) = conn {
            let mesh = Arc::clone(self);
            let target = id.clone();
            let result = handle.block_on(async move { mesh.ping(&target, &conn).await });
            if let Err(e) = result {
                self.set_error(&id, e);
            }
        }
        self.peer(&id).ok_or_else(|| "the peer is gone".to_string())
    }

    /// `peers/remove`: forget the peer and close its connections.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let id = self.resolve(id)?;
        let conn = {
            let mut inner = self.lock();
            inner.saved.peers.remove(&id);
            inner.live.remove(&id).and_then(|l| l.conn)
        };
        if let Some(conn) = conn {
            conn.close(CLOSE_REMOVED.into(), b"peer removed");
        }
        self.save();
        Ok(())
    }

    /// Serve one inbound mesh connection: the hello, then one request per stream.
    pub async fn serve_inbound(self: Arc<Self>, conn: Connection) -> io::Result<()> {
        let peer = conn.remote_id().to_string();
        let (mut send, mut recv) = conn.accept_bi().await.map_err(io::Error::other)?;
        let request = read_request(&mut recv).await?;
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let admitted = if request["method"] != MESH_HELLO_METHOD {
            Err(format!("the first message must be {MESH_HELLO_METHOD}"))
        } else {
            serde_json::from_value::<MeshHello>(request["params"].clone())
                .map_err(|e| format!("bad hello: {e}"))
                .and_then(|hello| self.admit(&peer, &hello))
        };
        let response = match admitted {
            Ok(new) => {
                let me = self.lock().me.clone();
                let reply = MeshHelloReply {
                    node: me.node,
                    name: crate::actions::host_name(),
                    os: std::env::consts::OS.to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    direct: me.direct,
                    relay: me.relay,
                    newly_paired: new,
                };
                json!({ "jsonrpc": "2.0", "id": id, "result": reply })
            }
            Err(message) => {
                let error = json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": UNAUTHORIZED, "message": message },
                });
                let _ = write_frame(&mut send, error.to_string().as_bytes()).await;
                finish(&mut send).await;
                conn.close(1u32.into(), b"rejected");
                return Ok(());
            }
        };
        write_frame(&mut send, response.to_string().as_bytes()).await?;
        finish(&mut send).await;
        eprintln!("aivoryd: mesh peer {} connected", short(&peer));
        self.inbound(&peer, 1);
        loop {
            tokio::select! {
                stream = conn.accept_bi() => match stream {
                    Ok((send, recv)) => {
                        let mesh = Arc::clone(&self);
                        let peer = peer.clone();
                        tokio::spawn(async move { mesh.answer(&peer, send, recv).await });
                    }
                    Err(_) => break,
                },
                () = tokio::time::sleep(RECHECK) => {
                    if !self.known(&peer) {
                        conn.close(CLOSE_REMOVED.into(), b"peer removed");
                        break;
                    }
                }
            }
        }
        self.inbound(&peer, -1);
        Ok(())
    }

    fn attached(&self) -> Result<&LinkHandle, String> {
        self.link.get().ok_or_else(|| {
            "the mesh needs the iroh link; start the daemon with `aivoryd serve --iroh`".into()
        })
    }

    /// Check the hello of an inbound connection. `Ok(true)`: the code paired a new peer.
    fn admit(self: &Arc<Self>, peer: &str, hello: &MeshHello) -> Result<bool, String> {
        if hello.key.trim() != peer {
            return Err("the key does not match the connection".into());
        }
        let now = now_ms();
        let new = {
            let mut inner = self.lock();
            let known = inner.saved.peers.contains_key(peer);
            if !known {
                let code = hello.code.as_deref().unwrap_or_default();
                let valid = inner.codes.remove(code).is_some_and(|e| e >= now);
                if !valid {
                    return Err(
                        "unknown machine: join with a new invite link (peers/invite)".into(),
                    );
                }
                inner
                    .saved
                    .peers
                    .insert(peer.to_string(), new_peer(peer, now));
            }
            let p = inner.saved.peers.get_mut(peer).expect("inserted above");
            update(
                p,
                &hello.name,
                &hello.os,
                &hello.version,
                &hello.direct,
                hello.relay.as_deref(),
            );
            !known
        };
        self.save();
        if new {
            eprintln!("aivoryd: mesh paired with '{}'", hello.name);
            self.start_dialer(peer);
        }
        Ok(new)
    }

    /// Answer one request stream of a peer.
    async fn answer(self: Arc<Self>, peer: &str, mut send: SendStream, mut recv: RecvStream) {
        let Ok(request) = read_request(&mut recv).await else {
            return;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let response = match request["method"].as_str().unwrap_or_default() {
            MESH_PING_METHOD => {
                if let Ok(info) = serde_json::from_value::<MeshInfo>(request["params"].clone()) {
                    self.seen(peer, &info);
                }
                json!({ "jsonrpc": "2.0", "id": id, "result": self.info() })
            }
            MESH_CALL_METHOD => {
                let method = request["params"]["method"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let params = request["params"]["params"].clone();
                let result = match self.host.get().cloned() {
                    Some(_) if !PEER_METHODS.contains(&method.as_str()) => Err((
                        -32601,
                        format!("'{method}' is not a method that a peer runs"),
                    )),
                    Some(host) => {
                        let peer = peer.to_string();
                        tokio::task::spawn_blocking(move || host.call(&peer, &method, params))
                            .await
                            .unwrap_or_else(|e| Err((-32603, e.to_string())))
                    }
                    None => Err((-32002, "this daemon takes no calls from peers".to_string())),
                };
                match result {
                    Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
                    Err((code, message)) => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": code, "message": message },
                    }),
                }
            }
            method @ ("mesh/put" | "mesh/del") => {
                let files = self.files.get().cloned();
                let params = request["params"].clone();
                let peer = peer.to_string();
                let method = method.to_string();
                let result = match files {
                    Some(files) => {
                        tokio::task::spawn_blocking(move || files.handle(&peer, &method, params))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    }
                    None => Err("this daemon keeps no mirrors".to_string()),
                };
                match result {
                    Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
                    Err(message) => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32002, "message": message },
                    }),
                }
            }
            method => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("'{method}' is not a mesh method") },
            }),
        };
        let _ = write_frame(&mut send, response.to_string().as_bytes()).await;
        finish(&mut send).await;
    }

    fn info(&self) -> MeshInfo {
        let me = self.lock().me.clone();
        MeshInfo {
            node: me.node,
            name: crate::actions::host_name(),
            os: std::env::consts::OS.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            direct: me.direct,
            relay: me.relay,
            now: now_ms(),
            state: self.host.get().map(|h| h.state()),
        }
    }

    fn hello(&self, code: Option<String>) -> MeshHello {
        let info = self.info();
        MeshHello {
            code,
            key: info.node,
            name: info.name,
            os: info.os,
            version: info.version,
            direct: info.direct,
            relay: info.relay,
        }
    }

    /// Dial a peer, send the hello, and store what the reply says.
    async fn dial(
        self: &Arc<Self>,
        node: &str,
        relay: Option<&str>,
        direct: &[String],
        code: Option<String>,
    ) -> Result<Connection, String> {
        let endpoint = self.attached()?.endpoint.clone();
        let addr = addr_of(node, relay, direct).map_err(|e| e.to_string())?;
        let conn = timeout(CONNECT_TIMEOUT, endpoint.connect(addr, MESH_ALPN))
            .await
            .map_err(|_| "no answer from the peer (timeout)".to_string())?
            .map_err(|e| format!("cannot connect: {e}"))?;
        let joining = code.is_some();
        let request = json!({
            "jsonrpc": "2.0", "id": 0, "method": MESH_HELLO_METHOD,
            "params": self.hello(code),
        });
        let result = call_stream(&conn, &request).await?;
        let reply: MeshHelloReply =
            serde_json::from_value(result).map_err(|e| format!("bad hello reply: {e}"))?;
        let now = now_ms();
        {
            let mut inner = self.lock();
            if !joining && !inner.saved.peers.contains_key(node) {
                conn.close(CLOSE_REMOVED.into(), b"peer removed");
                return Err("the peer was removed".into());
            }
            let p = inner
                .saved
                .peers
                .entry(node.to_string())
                .or_insert_with(|| new_peer(node, now));
            update(
                p,
                &reply.name,
                &reply.os,
                &reply.version,
                &reply.direct,
                reply.relay.as_deref(),
            );
            p.last_seen = Some(now);
        }
        self.save();
        Ok(conn)
    }

    /// One `mesh/ping` on its own stream. It swaps the addresses and measures the time.
    async fn ping(self: &Arc<Self>, id: &str, conn: &Connection) -> Result<(), String> {
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": MESH_PING_METHOD, "params": self.info(),
        });
        let started = Instant::now();
        let result = call_stream(conn, &request).await?;
        let rtt = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let info: MeshInfo =
            serde_json::from_value(result).map_err(|e| format!("bad ping reply: {e}"))?;
        self.seen(id, &info);
        let mut inner = self.lock();
        let live = inner.live.entry(id.to_string()).or_default();
        live.rtt_ms = Some(rtt);
        live.last_error = None;
        Ok(())
    }

    fn start_dialer(self: &Arc<Self>, id: &str) {
        let Some(link) = self.link.get() else {
            return;
        };
        if !self.lock().dialing.insert(id.to_string()) {
            return;
        }
        let mesh = Arc::clone(self);
        let id = id.to_string();
        link.handle.spawn(async move {
            mesh.dial_loop(&id).await;
            mesh.lock().dialing.remove(&id);
        });
    }

    /// Keep one outbound connection to the peer: connect, ping every 15 s, reconnect with
    /// backoff. Ends when the peer is removed.
    async fn dial_loop(self: &Arc<Self>, id: &str) {
        let mut backoff = Duration::from_secs(1);
        loop {
            let Some(target) = self.peer(id) else {
                return;
            };
            match self
                .dial(id, target.relay.as_deref(), &target.direct, None)
                .await
            {
                Ok(conn) => {
                    backoff = Duration::from_secs(1);
                    self.lock().live.entry(id.to_string()).or_default().conn = Some(conn.clone());
                    loop {
                        if let Err(e) = self.ping(id, &conn).await {
                            self.set_error(id, e);
                            break;
                        }
                        tokio::select! {
                            () = tokio::time::sleep(PING_EVERY) => {}
                            _ = conn.closed() => break,
                        }
                        if !self.known(id) {
                            conn.close(CLOSE_REMOVED.into(), b"peer removed");
                            return;
                        }
                    }
                    if let Some(live) = self.lock().live.get_mut(id) {
                        live.conn = None;
                    }
                }
                Err(e) => self.set_error(id, e),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }

    fn seen(&self, id: &str, info: &MeshInfo) {
        let changed = {
            let mut inner = self.lock();
            if inner.saved.peers.contains_key(id) {
                inner.live.entry(id.to_string()).or_default().state = info.state.clone();
            }
            let Some(p) = inner.saved.peers.get_mut(id) else {
                return;
            };
            let before = (p.direct.clone(), p.relay.clone(), p.name.clone());
            update(
                p,
                &info.name,
                &info.os,
                &info.version,
                &info.direct,
                info.relay.as_deref(),
            );
            p.last_seen = Some(now_ms());
            before != (p.direct.clone(), p.relay.clone(), p.name.clone())
        };
        if changed {
            self.save();
        }
    }

    fn inbound(&self, id: &str, delta: i32) {
        let mut inner = self.lock();
        let live = inner.live.entry(id.to_string()).or_default();
        live.inbound = live.inbound.saturating_add_signed(delta);
    }

    fn set_error(&self, id: &str, error: String) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.last_error = Some(error);
            return;
        }
        self.lock().live.insert(
            id.to_string(),
            Live {
                last_error: Some(error),
                ..Live::default()
            },
        );
    }

    fn known(&self, id: &str) -> bool {
        self.lock().saved.peers.contains_key(id)
    }

    fn peer(&self, id: &str) -> Option<Peer> {
        let inner = self.lock();
        inner
            .saved
            .peers
            .get(id)
            .map(|p| with_live(p, inner.live.get(id)))
    }

    /// A full node id or a unique prefix.
    fn resolve(&self, id: &str) -> Result<String, String> {
        let id = id.trim().to_ascii_lowercase();
        let inner = self.lock();
        let hits: Vec<&String> = inner
            .saved
            .peers
            .keys()
            .filter(|k| k.starts_with(&id))
            .collect();
        match hits.as_slice() {
            [one] if !id.is_empty() => Ok((*one).clone()),
            [] | [_] => Err(format!("no peer '{id}'")),
            _ => Err(format!("'{id}' names more than one peer")),
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let text = {
            let inner = self.lock();
            serde_json::to_string_pretty(&inner.saved).unwrap_or_default()
        };
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn new_peer(id: &str, now: UnixMs) -> Peer {
    Peer {
        id: id.to_string(),
        name: String::new(),
        os: String::new(),
        version: String::new(),
        direct: Vec::new(),
        relay: None,
        paired_at: now,
        last_seen: None,
        online: false,
        path: PeerPathKind::None,
        path_addr: String::new(),
        rtt_ms: None,
        last_error: None,
        state: None,
    }
}

/// The stored peer with its live fields.
fn with_live(p: &Peer, live: Option<&Live>) -> Peer {
    let mut out = p.clone();
    let Some(live) = live else {
        return out;
    };
    out.online = live
        .conn
        .as_ref()
        .is_some_and(|c| c.close_reason().is_none())
        || live.inbound > 0;
    if let Some(conn) = &live.conn {
        let path = path_of(conn);
        out.path = match path.kind {
            PathKind::Direct => PeerPathKind::Direct,
            PathKind::Relay => PeerPathKind::Relay,
            PathKind::None => PeerPathKind::None,
        };
        out.path_addr = path.addr;
    }
    out.rtt_ms = live.rtt_ms;
    out.state = live.state.clone();
    out.last_error = if out.online {
        None
    } else {
        live.last_error.clone()
    };
    out
}

fn update(
    p: &mut Peer,
    name: &str,
    os: &str,
    version: &str,
    direct: &[String],
    relay: Option<&str>,
) {
    if !name.is_empty() {
        p.name = name.to_string();
    }
    if !os.is_empty() {
        p.os = os.to_string();
    }
    if !version.is_empty() {
        p.version = version.to_string();
    }
    if !direct.is_empty() {
        p.direct = direct.to_vec();
    }
    if let Some(relay) = relay {
        p.relay = Some(relay.to_string());
    }
}

/// Send one request on a new stream and read the result.
async fn call_stream(conn: &Connection, request: &Value) -> Result<Value, String> {
    let work = async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
        write_frame(&mut send, request.to_string().as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let _ = send.finish();
        read_request(&mut recv).await.map_err(|e| e.to_string())
    };
    let response = timeout(CALL_TIMEOUT, work)
        .await
        .map_err(|_| "no answer from the peer (timeout)".to_string())??;
    if let Some(error) = response.get("error") {
        return Err(error["message"]
            .as_str()
            .unwrap_or("the peer refused")
            .to_string());
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

async fn read_request(recv: &mut RecvStream) -> io::Result<Value> {
    let frame = timeout(CALL_TIMEOUT, read_frame(recv))
        .await
        .map_err(|_| io::Error::other("no message"))??
        .ok_or_else(|| io::Error::other("the stream ended"))?;
    serde_json::from_slice(&frame).map_err(io::Error::other)
}

async fn finish(send: &mut SendStream) {
    if send.finish().is_ok() {
        let _ = timeout(Duration::from_secs(3), send.stopped()).await;
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}
