//! The JSON-RPC 2.0 surface (newline-delimited JSON on a named pipe or a Unix socket).
//! Protobuf (ADR 0005) is the later wire format; the types stay the same.
//!
//! The war room is a feed: `warroom/subscribe` returns a snapshot (or the missed deltas for a
//! known cursor), then the daemon sends `warroom/delta` notifications with increasing `seq`.
//! A client applies them with [`WarRoom::apply`]. After a reconnect it subscribes again with
//! its last `seq` as the cursor.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::actions::{
    AcpAdded, AddAcpParams, AddPartParams, DaemonRef, DaemonScaleParams, ImportReport,
    ProjectNewParams, ProviderRef, RepoAddParams,
};
use crate::code::{
    BlameParams, CodeFile, CodeFileParams, CodePosParams, DiagnosticsParams, DiagnosticsResult,
    FileBlame, Hover, LedgerTurnsParams, PlacePage, RenameParams, RenamePreview, RevertParams,
    RevertResult, SymbolPage, SymbolsParams, Timeline, TimelineParams, TurnList,
};
use crate::common::{ComposerPrefs, Effort, Line, Mode, UnixMs};
use crate::coord::{
    BlockOverrideParams, BlockOverrideResult, ClaimList, ClaimsListParams, MailList,
    MailListParams, RadarList, RadarListParams, RadarPingParams,
};
use crate::inbox::{InboxFilter, InboxItem, InboxList};
use crate::repos::{MapFile, RepoDetail, RepoList};
use crate::session::{HistoryPage, SessionDiff, SessionTrace};
use crate::settings::{DevicePushParams, Pairing, Settings, SettingsView};
use crate::tasks::{Playbook, Task, TaskColumn, TaskList};
use crate::warroom::Board;
use crate::warroom::{
    ACTIVITY_SAMPLES, ActivitySample, Autopilot, Codebase, LinkState, PANE_TAIL_LINES, Pane,
    QueueOrder, UsageWindow, WarRoom,
};

pub mod code {
    //! JSON-RPC error codes.
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const SERVER_ERROR: i64 = -32000;
    pub const SESSION_NOT_FOUND: i64 = -32001;
    pub const SESSION_AMBIGUOUS: i64 = -32002;
    pub const SESSION_HAS_NO_CWD: i64 = -32003;
    /// The method needs the live war-room engine, which this daemon does not run.
    pub const NOT_AVAILABLE: i64 = -32004;
    /// No inbox item, task, codebase or device with this id.
    pub const NOT_FOUND: i64 = -32005;
    /// The answer is for an ask that is no longer open.
    pub const STALE_ASK: i64 = -32006;
    /// The session cannot do this in its state, for example resume a working session.
    pub const INVALID_STATE: i64 = -32007;
}

/// The `error` member of a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn not_found(what: &str, id: &str) -> Self {
        Self::new(code::NOT_FOUND, format!("no {what} '{id}'"))
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// A method: its name, parameter type and result type.
pub trait Method {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned + JsonSchema;
    type Result: Serialize + DeserializeOwned + JsonSchema;
}

/// The request line for method `M`.
pub fn request<M: Method>(id: u64, params: &M::Params) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": M::NAME, "params": params })
}

/// The typed result of a response object for method `M`.
pub fn parse_response<M: Method>(response: &Value) -> Result<M::Result, RpcError> {
    if let Some(error) = response.get("error") {
        return Err(serde_json::from_value(error.clone()).unwrap_or_else(|_| {
            RpcError::new(code::SERVER_ERROR, format!("malformed error: {error}"))
        }));
    }
    let result = response.get("result").cloned().unwrap_or(Value::Null);
    serde_json::from_value(result)
        .map_err(|e| RpcError::new(code::SERVER_ERROR, format!("unexpected result: {e}")))
}

/// Parameters of a method without parameters. `null` and `{}` are both valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ack {
    pub ok: bool,
}

impl Ack {
    pub const OK: Self = Self { ok: true };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    pub name: String,
    pub version: String,
    /// The transport, `jsonrpc-2.0/ndjson`.
    pub protocol: String,
    /// [`crate::PROTOCOL_VERSION`] of the daemon.
    pub contract: String,
    pub pid: u32,
    /// The daemon serves the demo data (`aivoryd serve --demo`).
    pub demo: bool,
    /// The name of the machine. Empty from an older daemon.
    #[serde(default)]
    pub host: String,
    /// The working folder of the daemon process: where "Open shell" starts when no codebase is
    /// selected. Empty from an older daemon.
    #[serde(default)]
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    /// The last `seq` the client applied. Omit it for a fresh snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u64>,
    /// The epoch of that cursor ([`WarRoom::epoch`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
}

/// Result of `warroom/subscribe`. Either `snapshot` is set, or `deltas` holds everything after
/// the given cursor (same epoch). Notifications that follow have `seq > cursor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub epoch: u64,
    pub cursor: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WarRoom>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deltas: Vec<DeltaNote>,
}

/// Parameters of the `warroom/delta` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeltaNote {
    pub seq: u64,
    pub delta: Delta,
}

/// One change of the war-room view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Delta {
    /// Add a session or replace it (same id).
    Session {
        session: Box<Pane>,
    },
    SessionRemoved {
        id: String,
    },
    /// New transcript lines. Main lines go to the pane tail; subagent lines only count.
    Lines {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent: Option<String>,
        lines: Vec<Line>,
        /// The new transcript length.
        total: u32,
    },
    /// A new pulse sample, or a new value for the current minute (same `at`).
    Activity {
        sample: ActivitySample,
    },
    Autopilot {
        autopilot: Autopilot,
    },
    Counts {
        queued: u32,
        inbox_open: u32,
    },
    Usage {
        usage: Vec<UsageWindow>,
    },
    Links {
        hub: LinkState,
        relay: LinkState,
    },
    /// Add a codebase or replace it (same id).
    Codebase {
        codebase: Codebase,
    },
    /// A codebase that no longer shows (`repos/remove` of a pinned codebase).
    CodebaseRemoved {
        id: String,
    },
    /// Daemon time, so that the clocks of the clients stay right.
    Tick {
        now: UnixMs,
    },
    /// The new board. The daemon sends it after every change of a session. A client that does
    /// not know this delta keeps its old board.
    Board {
        board: Board,
    },
}

impl WarRoom {
    /// Apply one delta and move the cursor to `seq`.
    pub fn apply(&mut self, seq: u64, delta: &Delta) {
        match delta {
            Delta::Session { session } => match self.session_mut(&session.id) {
                Some(s) => *s = (**session).clone(),
                None => self.sessions.push((**session).clone()),
            },
            Delta::SessionRemoved { id } => self.sessions.retain(|s| &s.id != id),
            Delta::Lines {
                id,
                subagent,
                lines,
                total,
            } => {
                if let Some(s) = self.session_mut(id)
                    && subagent.is_none()
                {
                    s.tail.extend(lines.iter().cloned());
                    let extra = s.tail.len().saturating_sub(PANE_TAIL_LINES);
                    s.tail.drain(..extra);
                    s.lines_total = *total;
                }
            }
            Delta::Activity { sample } => {
                match self.activity.last_mut() {
                    Some(last) if last.at == sample.at => *last = *sample,
                    _ => self.activity.push(*sample),
                }
                let extra = self.activity.len().saturating_sub(ACTIVITY_SAMPLES);
                self.activity.drain(..extra);
            }
            Delta::Autopilot { autopilot } => self.autopilot = autopilot.clone(),
            Delta::Counts { queued, inbox_open } => {
                self.queued = *queued;
                self.inbox_open = *inbox_open;
            }
            Delta::Usage { usage } => self.usage = usage.clone(),
            Delta::Links { hub, relay } => {
                self.hub = *hub;
                self.relay = *relay;
            }
            Delta::Codebase { codebase } => {
                match self.codebases.iter_mut().find(|c| c.id == codebase.id) {
                    Some(c) => *c = codebase.clone(),
                    None => self.codebases.push(codebase.clone()),
                }
            }
            Delta::CodebaseRemoved { id } => self.codebases.retain(|c| &c.id != id),
            Delta::Tick { now } => self.now = *now,
            Delta::Board { board } => self.board = board.clone(),
        }
        self.cursor = seq;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SendParams {
    pub id: String,
    pub text: String,
    /// Send to this subagent instead of the main agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
    /// Paths of attached files and screen captures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// New mode, model or effort for this turn. A new provider needs `session/handoff`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<ComposerPrefs>,
}

/// Answer an ask: pick an option, or type an own answer, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnswerParams {
    pub id: String,
    /// The [`crate::Ask::id`] the answer is for.
    pub ask: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    pub codebase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub prompt: String,
    pub composer: ComposerPrefs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// Start work from one line, or queue it (`session/dispatch`).
///
/// Rule: the daemon takes `codebase` (default: the first codebase), `part` (default: the first
/// part) and `agent` (the provider, default `claude`). When the autopilot has a free slot for
/// the codebase ([`crate::WarRoom::has_free_slot`]) the daemon starts a session now and the
/// result holds `session`. Else it queues a task, whatever the autopilot `startQueued` value is,
/// and the result holds `task`. [`DispatchParams::from_line`] reads the line
/// `@codebase words --agent provider`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DispatchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The model of the session (default: the provider setting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort of the session (default: the provider setting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    pub prompt: String,
}

impl DispatchParams {
    /// Read a dispatch line. The first word `@codebase` names the codebase. `--agent x` names
    /// the provider, anywhere in the line. The other words are the prompt. `None` when no
    /// prompt is left.
    pub fn from_line(line: &str) -> Option<Self> {
        let mut codebase = None;
        let mut agent = None;
        let mut prompt: Vec<&str> = Vec::new();
        let mut words = line.split_whitespace();
        while let Some(w) = words.next() {
            if w == "--agent" {
                agent = words.next().map(str::to_string);
            } else if let Some(name) = w.strip_prefix('@').filter(|n| !n.is_empty())
                && codebase.is_none()
                && prompt.is_empty()
            {
                codebase = Some(name.to_string());
            } else {
                prompt.push(w);
            }
        }
        if prompt.is_empty() {
            return None;
        }
        Some(Self {
            codebase,
            part: None,
            agent,
            model: None,
            effort: None,
            prompt: prompt.join(" "),
        })
    }
}

/// The result of `session/dispatch`: `session` when it started, `task` when it queued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dispatched {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// Set the mode of the composer of a session. It applies from the next turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModeParams {
    pub id: String,
    pub mode: Mode,
}

/// Continue a session with another provider. The daemon starts a new session with a summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandoffParams {
    pub id: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Fork a session: a new session that starts with the conversation of the old one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForkParams {
    pub id: String,
    /// The span to fork at. A daemon that cannot fork at a span forks at the end. A turn
    /// number (`"3"`) forks at the end of that turn of the turn ledger: the new session gets a
    /// worktree at the snapshot of the turn and a handoff packet of the turns 1 to 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The provider of a fork at a turn. Default: the provider of the old session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The first turn of the fork. The daemon picks a neutral one when it is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
    /// Return the lines before this index. Omit it for the newest lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u32>,
    /// Default 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Change some autopilot rules. Missing fields keep their value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AutopilotPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub park_after_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_running: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_codebase: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_queued: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<QueueOrder>,
}

impl Autopilot {
    /// Apply a patch, then clamp.
    pub fn patch(&mut self, p: &AutopilotPatch) {
        if let Some(v) = p.on {
            self.on = v;
        }
        if let Some(v) = p.park_after_min {
            self.park_after_min = v;
        }
        if let Some(v) = p.max_running {
            self.max_running = v;
        }
        if let Some(v) = p.max_per_codebase {
            self.max_per_codebase = v;
        }
        if let Some(v) = p.start_queued {
            self.start_queued = v;
        }
        if let Some(v) = p.order {
            self.order = v;
        }
        self.clamp();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxListParams {
    /// Default: all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<InboxFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxAnswerParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxSnoozeParams {
    /// Omit it to snooze all open items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StartWhen {
    Now,
    /// Autopilot starts it when a slot is free.
    Queue,
    /// Start in plan mode and wait for the plan approval.
    PlanOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreateParams {
    pub title: String,
    pub codebase: String,
    pub part: String,
    pub playbook: Playbook,
    pub prompt: String,
    pub when: StartWhen,
    /// Default: the provider defaults of Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// Default: `Normal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<crate::tasks::TaskPriority>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<TaskColumn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodebaseRef {
    pub id: String,
}

/// Set one settings value by its dotted key (see [`Settings::set_key`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSetParams {
    pub key: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRef {
    pub id: String,
}

macro_rules! methods {
    ($($(#[doc = $doc:literal])* $marker:ident = $name:literal, $params:ty => $result:ty;)*) => {
        /// Marker types, one per method, for typed clients and servers.
        pub mod method {
            use super::*;
            $(
                $(#[doc = $doc])*
                #[derive(Debug, Clone, Copy)]
                pub struct $marker;
                impl Method for $marker {
                    const NAME: &'static str = $name;
                    type Params = $params;
                    type Result = $result;
                }
            )*
        }

        /// Every method name of the surface.
        pub const METHODS: &[&str] = &[$($name),*];

        pub(crate) fn method_schemas(
            generator: &mut schemars::SchemaGenerator,
        ) -> Vec<(&'static str, schemars::Schema, schemars::Schema)> {
            vec![$(($name, generator.subschema_for::<$params>(), generator.subschema_for::<$result>())),*]
        }
    };
}

methods! {
    DaemonInfoMethod = "daemon/info", Empty => DaemonInfo;
    /// Stop the daemon.
    DaemonShutdown = "daemon/shutdown", Empty => Ack;
    /// Restart the local daemon: the daemon starts a new process with the same arguments and
    /// ends. The clients connect again and subscribe with their cursor and epoch (the epoch
    /// changes, so they get a fresh snapshot).
    DaemonRestart = "daemon/restart", DaemonRef => Ack;
    /// Set the `maxRunning` value of the autopilot for a daemon.
    DaemonScale = "daemon/scale", DaemonScaleParams => Autopilot;
    WarroomSnapshot = "warroom/snapshot", Empty => WarRoom;
    WarroomSubscribe = "warroom/subscribe", SubscribeParams => Subscription;
    /// Send a message to the session, or to one of its subagents.
    SessionSend = "session/send", SendParams => Ack;
    SessionAnswer = "session/answer", AnswerParams => Ack;
    /// Interrupt the current turn. The session stays live.
    SessionPause = "session/pause", SessionRef => Ack;
    /// Kill the agent process.
    SessionStop = "session/stop", SessionRef => Ack;
    /// Stop the session and keep it resumable.
    SessionPark = "session/park", SessionRef => Ack;
    SessionResume = "session/resume", SessionRef => Ack;
    SessionNew = "session/new", NewSessionParams => Created;
    /// Start a session from one line of work, or queue it. See [`DispatchParams`] for the rule.
    SessionDispatch = "session/dispatch", DispatchParams => Dispatched;
    /// The RAM of the machine and of the agent processes, with a 30-minute history.
    ResourcesMemory = "resources/memory", Empty => crate::resources::Memory;
    /// Start the organizer agent: a session of aivoryd that watches RAM and the other
    /// sessions, and acts or gives feedback by its mode.
    OrganizerStart = "organizer/start", crate::resources::OrganizerStartParams => Created;
    /// Set the permission mode of the composer of a session. The daemon stores it in
    /// `composer.mode` of the pane; it applies from the next turn. The change reaches the
    /// subscribers as a `session` delta.
    SessionMode = "session/mode", ModeParams => Ack;
    SessionHandoff = "session/handoff", HandoffParams => Created;
    SessionFork = "session/fork", ForkParams => Created;
    SessionHistory = "session/history", HistoryParams => HistoryPage;
    SessionSpans = "session/spans", SessionRef => SessionTrace;
    SessionDiffMethod = "session/diff", SessionRef => SessionDiff;
    AutopilotGet = "autopilot/get", Empty => Autopilot;
    AutopilotSet = "autopilot/set", AutopilotPatch => Autopilot;
    InboxListMethod = "inbox/list", InboxListParams => InboxList;
    /// Answer an item. The result is the item with its `result`.
    InboxAnswer = "inbox/answer", InboxAnswerParams => InboxItem;
    InboxSnooze = "inbox/snooze", InboxSnoozeParams => Ack;
    TasksList = "tasks/list", TaskListParams => TaskList;
    TasksCreate = "tasks/create", TaskCreateParams => Task;
    TasksUpdate = "tasks/update", TaskUpdateParams => Task;
    ReposList = "repos/list", Empty => RepoList;
    ReposGet = "repos/get", CodebaseRef => RepoDetail;
    ReposMap = "repos/map", CodebaseRef => MapFile;
    /// Register a folder of a git repository as a codebase. The daemon scans it without a
    /// session and keeps it over a restart. A folder that is no git repository: `INVALID_PARAMS`.
    ReposAdd = "repos/add", RepoAddParams => Codebase;
    /// Unregister a codebase that `repos/add` or `project/new` registered. A codebase that only
    /// sessions show answers `INVALID_STATE`. Nothing on the disk changes.
    ReposRemove = "repos/remove", CodebaseRef => Ack;
    /// Create a folder with `git init`, the files of a template and `.aivory/map.toml`, and
    /// register it. A folder that exists and is not empty: `INVALID_STATE`.
    ProjectNew = "project/new", ProjectNewParams => Codebase;
    /// Add a `[[part]]` to `.aivory/map.toml` of the codebase. The daemon keeps the other parts
    /// and the comments, and creates the file when it is missing. The result is the new map.
    MapAddPart = "map/addPart", AddPartParams => MapFile;
    /// Add an ACP agent. The daemon runs `<command> --version`, or checks that the file exists.
    ProvidersAddAcp = "providers/addAcp", AddAcpParams => AcpAdded;
    ProvidersRemoveAcp = "providers/removeAcp", ProviderRef => Ack;
    /// Read only `model` and `permissions.defaultMode` of `~/.claude/settings.json` and set
    /// `providers.claude.model` and `providers.claude.mode`.
    SettingsImportClaude = "settings/importClaude", Empty => ImportReport;
    SettingsGet = "settings/get", Empty => SettingsView;
    SettingsSet = "settings/set", SettingsSetParams => Settings;
    /// Start a pairing. The phone scans the QR code of `url`.
    DevicesPair = "devices/pair", Empty => Pairing;
    DevicesRemove = "devices/remove", DeviceRef => Ack;
    /// A paired device registers the push target for the sealed push of the daemon.
    DevicesPush = "devices/push", DevicePushParams => Ack;
    /// A file with its highlights for the visible lines, outline, folding, and findings.
    CodeFileMethod = "code/file", CodeFileParams => CodeFile;
    /// Where the name under the cursor is defined (index first, a language server when it runs).
    CodeDefinition = "code/definition", CodePosParams => PlacePage;
    /// Where the name under the cursor is used.
    CodeReferences = "code/references", CodePosParams => PlacePage;
    CodeHover = "code/hover", CodePosParams => Hover;
    CodeSymbols = "code/symbols", SymbolsParams => SymbolPage;
    /// The findings of a turn (SARIF rows) and the change against the state before the turn.
    CodeDiagnostics = "code/diagnostics", DiagnosticsParams => DiagnosticsResult;
    /// The edits of a rename as a preview. The daemon never applies them.
    CodeRename = "code/rename", RenameParams => RenamePreview;
    LedgerTurns = "ledger/turns", LedgerTurnsParams => TurnList;
    /// Blame by prompt: the commit and the turn of every line of a file.
    LedgerBlame = "ledger/blame", BlameParams => FileBlame;
    /// The commits and the turns that changed a file, oldest first.
    LedgerTimeline = "ledger/timeline", TimelineParams => Timeline;
    /// Restore the snapshot before a turn into the worktree of the session. A session in the
    /// main checkout of the user answers `INVALID_STATE`.
    LedgerRevert = "ledger/revert", RevertParams => RevertResult;
    /// The live claims of the sessions of this daemon (the lease has not ended).
    ClaimsList = "claims/list", ClaimsListParams => ClaimList;
    /// The pairs of sessions that change the same code, with the risk score of the trial merge.
    RadarListMethod = "radar/list", RadarListParams => RadarList;
    /// Send a ping to a session. The daemon delivers it at the next turn boundary.
    RadarPing = "radar/ping", RadarPingParams => Ack;
    /// The same as `radar/ping`: the "Send ping" button of the design.
    MailSend = "mail/send", RadarPingParams => Ack;
    /// The mailbox of a session.
    MailListMethod = "mail/list", MailListParams => MailList;
    /// A human frees a session from a block: its edits on the keys are allowed again.
    BlockOverride = "block/override", BlockOverrideParams => BlockOverrideResult;
    /// Download a theme package into a staging folder and check it. The result is a job; the
    /// finished job holds an `Inspected` (see `crate::theme`) with the token for `theme/add`.
    ThemeInspect = "theme/inspect", crate::theme::ThemeSourceParams => crate::theme::JobRef;
    /// Commit a staged theme (the token of `theme/inspect`) to the themes folder.
    ThemeAdd = "theme/add", crate::theme::ThemeAddParams => crate::theme::InstalledTheme;
    ThemeListMethod = "theme/list", Empty => crate::theme::ThemeList;
    /// Remove an installed theme. The current theme falls back: `display.customTheme` clears.
    ThemeRemove = "theme/remove", crate::theme::ThemeRef => crate::theme::RemoveResult;
    /// Ask the source of each installed theme for a newer version. A job with an `UpdateList`.
    ThemeCheckUpdates = "theme/checkUpdates", Empty => crate::theme::JobRef;
    /// Update one theme at file level. A job with an `UpdateResult`.
    ThemeUpdate = "theme/update", crate::theme::ThemeRef => crate::theme::JobRef;
    /// Local edits and upstream changes of one theme. A job with a `ThemeDiff`.
    ThemeDiffMethod = "theme/diff", crate::theme::ThemeRef => crate::theme::JobRef;
    /// One installed theme for one display class: tokens, layouts, parts and strings. Read-only.
    ThemeGet = "theme/get", crate::theme::ThemeGetParams => crate::theme::ThemeBundle;
    /// One file of `assets/` or `previews/` of an installed theme, as base64. Read-only.
    ThemeAssetMethod = "theme/asset", crate::theme::ThemeAssetParams => crate::theme::ThemeAsset;
    /// The state of a job that another `theme/*` or `registry/*` method started.
    ThemeJob = "theme/job", crate::theme::JobRef => crate::theme::JobStatus;
    RegistryListMethod = "registry/list", Empty => crate::theme::RegistryList;
    RegistryAdd = "registry/add", crate::theme::RegistryAddParams => crate::theme::RegistryEntry;
    RegistryRemove = "registry/remove", crate::theme::RegistryRef => Ack;
    /// Download the index of one registry or of all. A job with a `RegistryList`.
    RegistryRefresh = "registry/refresh", crate::theme::RegistryRefreshParams => crate::theme::JobRef;
}

/// The notification of the war-room feed.
pub const WARROOM_DELTA: &str = "warroom/delta";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dispatch_line_has_a_codebase_an_agent_and_a_prompt() {
        let d = DispatchParams::from_line("@aivory fix the build --agent codex").unwrap();
        assert_eq!(d.codebase.as_deref(), Some("aivory"));
        assert_eq!(d.agent.as_deref(), Some("codex"));
        assert_eq!(d.prompt, "fix the build");
        assert!(DispatchParams::from_line("@aivory").is_none());
        assert!(DispatchParams::from_line("   ").is_none());
    }

    #[test]
    fn only_the_first_word_names_the_codebase() {
        let d = DispatchParams::from_line("fix @aivory now").unwrap();
        assert_eq!(d.codebase, None);
        assert_eq!(d.prompt, "fix @aivory now");
        let d = DispatchParams::from_line("--agent gemini @cb do it").unwrap();
        assert_eq!(d.codebase.as_deref(), Some("cb"));
        assert_eq!(d.agent.as_deref(), Some("gemini"));
        assert_eq!(DispatchParams::from_line("@ x").unwrap().prompt, "@ x");
    }

    #[test]
    fn a_delta_with_a_board_applies() {
        let json = serde_json::json!({ "type": "board", "board": { "needsYou": { "count": 1, "ids": ["a"] },
            "working": { "count": 0, "ids": [] }, "blocked": { "count": 0, "ids": [] },
            "idle": { "count": 0, "ids": [] }, "asks": ["a"] } });
        let d: Delta = serde_json::from_value(json).unwrap();
        assert!(matches!(d, Delta::Board { .. }));
    }
}
