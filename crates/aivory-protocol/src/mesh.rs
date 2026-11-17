//! The machine mesh: daemons of one user on more machines (ADR 0026).
//!
//! - `peers/invite` on machine A gives a one-time link `aivory://peer?...`.
//! - `peers/join` on machine B takes the link, dials A over iroh (ALPN `aivory/mesh/1`), and
//!   both daemons store the key of the other one.
//! - `peers/list` and `peers/check` show each peer with its network path (direct or relay) and
//!   the round-trip time. `peers/remove` forgets a peer on this machine.
//!
//! The `peers/*` methods are for the local client only. A peer daemon never reaches them: a
//! mesh connection has its own small method set (`mesh/*`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::UnixMs;

/// Result of `peers/invite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeerInvite {
    /// The one-time code in the link (`AIV-xxxxxx`).
    pub code: String,
    /// `aivory://peer?node=&relay=&direct=&code=&name=`. Give it to the other machine.
    pub url: String,
    pub expires: UnixMs,
}

/// Params of `peers/join`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeerJoinParams {
    /// The link of `peers/invite` of the other machine.
    pub link: String,
}

/// Params of `peers/check` and `peers/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeerRef {
    /// The node id of the peer (64 hex characters). A unique prefix is enough.
    pub id: String,
}

/// The kind of the network path to a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PeerPathKind {
    /// A direct IP path (same network, or hole punched).
    Direct,
    /// Through the relay.
    Relay,
    /// No connection.
    #[default]
    None,
}

/// One paired machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// The iroh node id of the peer daemon (64 hex characters).
    pub id: String,
    /// The machine name that the peer gave.
    pub name: String,
    #[serde(default)]
    pub os: String,
    /// The version of the peer daemon.
    #[serde(default)]
    pub version: String,
    /// The direct addresses that the peer gave on its last connect (`ip:port`).
    #[serde(default)]
    pub direct: Vec<String>,
    /// The home relay of the peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    pub paired_at: UnixMs,
    /// The last answer of the peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<UnixMs>,
    /// `true` while this daemon has an open connection to the peer.
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub path: PeerPathKind,
    /// The remote address of the path: `ip:port` or the relay URL. Empty: no path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path_addr: String,
    /// The round-trip time of the last ping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<u64>,
    /// Why the last connect or ping failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// What the peer reported on its last ping: slots, providers, codebases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<NodeState>,
}

/// The work state of one machine, sent with each `mesh/ping` (step M2).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeState {
    /// Sessions that work or wait for the user now.
    pub running: u32,
    /// The autopilot limit of running sessions.
    pub max_running: u32,
    /// Queued tasks.
    pub queued: u32,
    /// The agent CLIs installed on the machine (`claude`, `codex`, `gemini`).
    #[serde(default)]
    pub providers: Vec<String>,
    /// The codebases of the machine.
    #[serde(default)]
    pub codebases: Vec<NodeCodebase>,
    /// The look (`display.look`) of the machine, as `settings/get` gives it. Empty: an older
    /// peer that does not send it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub look: String,
}

impl NodeState {
    /// Free session slots.
    pub fn free(&self) -> u32 {
        self.max_running.saturating_sub(self.running)
    }
}

/// One codebase of a machine.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeCodebase {
    pub name: String,
    /// The repository identity (the same repo on two machines has the same identity), for
    /// example `git:<root commit>`. Empty: not a repository, or unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub identity: String,
    /// The remote URL of the repository, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

/// Params of `peers/call`: run one method of the JSON-RPC surface on a peer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeerCallParams {
    /// The node id of the peer, or a unique prefix.
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Result of `peers/list`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PeerList {
    /// The node id of this daemon. Empty: the daemon runs without iroh.
    pub node: String,
    pub peers: Vec<Peer>,
}

/// One machine of the mesh for a phone (`machines/list`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    /// The node id (`local` when the daemon has no node id).
    pub id: String,
    /// The name that `Pane.machine` uses for the sessions of this machine.
    pub name: String,
    /// The `display.look` of the machine. Empty: not known yet.
    pub look: String,
    /// This daemon.
    pub local: bool,
    pub online: bool,
}

/// Result of `machines/list`: this machine first, then one entry per mesh peer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MachineList {
    pub machines: Vec<Machine>,
}

/// Result of `mesh/ping`, peer to peer: who the machine is now.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeshInfo {
    pub node: String,
    pub name: String,
    pub os: String,
    pub version: String,
    /// The current direct addresses of the daemon (`ip:port`).
    #[serde(default)]
    pub direct: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    /// The sender's clock (Unix ms).
    pub now: UnixMs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<NodeState>,
}

/// The methods of the local surface that a peer may call (`mesh/call`, step M6). The
/// methods that change the machine stay local (`LOCAL_ONLY` in the daemon).
pub const PEER_METHODS: &[&str] = &[
    "daemon/info",
    "warroom/snapshot",
    "session/new",
    "session/dispatch",
    "session/send",
    "session/answer",
    "session/stop",
    "session/pause",
    "session/resume",
    "session/history",
    "session/diff",
    "tasks/list",
    "tasks/create",
    "tasks/start",
    "repos/list",
    "claims/list",
    "radar/list",
];

/// Params of `peers/share`: push a folder live to peers (step M5). The machine that shares is
/// the only writer; the peers keep a read-only mirror.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareParams {
    /// The folder to share (absolute).
    pub path: String,
    /// The name of the mirror on the peers. Default: the folder name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The peers (node ids or prefixes). Empty: all paired machines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
}

/// One folder that this machine shares.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    /// The peers. Empty: all paired machines.
    #[serde(default)]
    pub to: Vec<String>,
    /// Files in the share (ignored files left out).
    pub files: u32,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan: Option<UnixMs>,
    /// The last WIP commit of the share in its VCS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// One read-only mirror of a folder of a peer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MirrorInfo {
    pub peer: String,
    pub name: String,
    /// The local folder of the mirror.
    pub path: String,
    pub files: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<UnixMs>,
}

/// Params of `peers/unshare`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareRef {
    pub id: String,
}

/// Result of `peers/shares`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharesView {
    pub shares: Vec<ShareInfo>,
    pub mirrors: Vec<MirrorInfo>,
}

/// The auto-sync state of one codebase (step M4).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VcsSyncRow {
    pub codebase: String,
    pub path: String,
    /// `git`, `hg`, `jj`, `svn`, `p4`. Empty: no VCS found.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<UnixMs>,
    /// The last update brought new commits.
    #[serde(default)]
    pub changed: bool,
    /// Why the last update did not run or failed (dirty checkout, a session works, conflict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Result of `peers/vcs`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VcsSyncView {
    /// Seconds between two updates. 0: auto-sync is off.
    pub interval_secs: u64,
    pub rows: Vec<VcsSyncRow>,
}
