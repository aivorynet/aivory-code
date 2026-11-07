//! Local coordination: claims, the conflict radar, and the mailbox of the sessions of one
//! daemon (`docs/design/04-coordination-sync.md`, README section 8 "Repos -> Radar").
//!
//! - A claim is a lease of a session on a routing key. `claims/list` shows the live claims.
//!   The Files tab shows the `claimedBy` pill from them.
//! - A radar hit is a pair of sessions that change the same code. `radar/list` shows the pairs
//!   with the risk score of the trial merge (`git merge-tree`).
//! - A ping is a message from a session or a human to a session. The daemon delivers it at the
//!   next turn boundary as quoted data. A ping never grants a permission (ADR 0019).
//!
//! A routing key is a plain string: `path:src/a.rs`, `sym:rust:Invoice::total`,
//! `test:tests::pays_once`, or `mod:src/billing`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{SessionState, UnixMs};

/// The state of a claim. It only moves up: intent, editing, landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ClaimState {
    /// The plan of the session will touch the key.
    Intent,
    /// The session changed the key.
    Editing,
    /// The change is on the target branch. It shows no heat.
    Landed,
}

/// The lines of a changed declaration, from 1, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRange {
    pub from: u32,
    pub to: u32,
}

/// A lease of a session on a routing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    /// The session id as the war room shows it.
    pub session: String,
    pub codebase: String,
    pub key: String,
    pub state: ClaimState,
    /// The file of the key (a path key, or the file of a symbol key when the daemon knows it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The lines of the symbol, when the key is a symbol key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ClaimRange>,
    /// Seconds until the lease ends. A heartbeat of the daemon renews it.
    pub lease_s: u32,
}

/// Parameters of `claims/list`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimsListParams {
    /// Only the claims of the sessions of this codebase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<String>,
    /// Only the claims of this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// Result of `claims/list`: the live claims, ordered by key and session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimList {
    pub claims: Vec<Claim>,
}

/// A pair of sessions that change the same code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarHit {
    /// The first session (the lower id).
    pub a: String,
    pub b: String,
    pub codebase: String,
    /// The highest tier that matched: 1 same path, 2 same symbol, 3 dependency.
    pub tier: u8,
    /// The keys that caused the match, sorted.
    pub keys: Vec<String>,
    /// The risk: 0.1 clean merge with no shared symbol, 0.4 shared symbols, 0.7 a signature
    /// that the other side uses, 1.0 a text conflict.
    pub score: f32,
    /// The files with a text conflict (the trial merge).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_files: Vec<String>,
    /// True when a trial merge decided the score.
    pub trial: bool,
    /// Why no trial merge ran, when `trial` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_error: Option<String>,
    /// When the radar found the pair.
    pub at: UnixMs,
    /// What a human did about the pair. `None` for a new pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<HitStatus>,
}

/// What a human did about a radar hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HitStatus {
    /// A ping went to one of the sessions (`radar/ping`).
    Pinged,
    /// One of the sessions paused (`session/pause`).
    Paused,
}

/// A session in the radar graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarNode {
    pub id: String,
    pub provider: String,
    pub part: String,
    /// The owner of the session: `you`, or a team member in the team scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub state: SessionState,
}

/// Parameters of `radar/list`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<String>,
}

/// Result of `radar/list`: the sessions that appear in a hit, and the hits, highest risk first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarList {
    pub sessions: Vec<RadarNode>,
    pub hits: Vec<RadarHit>,
}

/// The kind of a ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PingKind {
    /// Information, shown at the next turn boundary.
    Fyi,
    /// A question with a deadline.
    Question,
    /// A request for work. The daemon makes a task.
    Request,
    /// "Stop changing X until Y lands". The daemon denies the edits of the receiver on the
    /// keys until a human overrides it with `block/override`. Only a human sends a block.
    Block,
}

/// Parameters of `radar/ping` (and its alias `mail/send`). A ping without `from` comes from the
/// human at the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RadarPingParams {
    /// The sender: a session id. Omit it for a human.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The receiver session.
    pub to: String,
    pub kind: PingKind,
    pub text: String,
    /// The routing keys that the ping is about. Pings with the same key and kind in 10 minutes
    /// merge into one. A block needs at least one key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
}

/// Parameters of `mail/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MailListParams {
    pub session: String,
}

/// One item of the mailbox of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MailItem {
    pub id: i64,
    /// The senders, comma separated (a merged ping has several).
    pub from: String,
    pub kind: PingKind,
    pub key: String,
    pub text: String,
    pub created_at: UnixMs,
    /// When the daemon put the item in front of an input. Empty while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<UnixMs>,
    /// A question that has an answer, a request that has a task, or a block that a human
    /// overrode.
    pub resolved: bool,
    /// The number of pings that merged into this item.
    pub count: u32,
    /// True for the digest of the pings over the hourly limit.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub digest: bool,
}

/// Result of `mail/list`: the mailbox of the session, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MailList {
    pub session: String,
    pub items: Vec<MailItem>,
}

/// Parameters of `block/override`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlockOverrideParams {
    pub session: String,
    /// The blocked keys to free. Empty frees every block of the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
}

/// Result of `block/override`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlockOverrideResult {
    /// The number of blocks that the call closed.
    pub resolved: u32,
}
