//! The war room (`Main.dc.html`): sessions, codebases, the pulse chart, autopilot, status bar.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Ask, ComposerPrefs, Line, SessionState, UnixMs};

/// Lines of a pane tail in a snapshot. `session/history` pages the rest.
pub const PANE_TAIL_LINES: usize = 40;

/// Samples of the pulse chart: one per minute, the last hour.
pub const ACTIVITY_SAMPLES: usize = 60;

/// One repository with its parts from `.aivory/map.toml`. The id is the short name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Codebase {
    pub id: String,
    pub path: String,
    pub branch: String,
    pub worktrees: u32,
    /// The daemon that runs the sessions of this codebase.
    pub daemon: String,
    pub languages: Vec<String>,
    /// In map order. The Sankey orders its part column by this list.
    pub parts: Vec<Part>,
}

impl Codebase {
    pub fn part_names(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().map(|p| p.name.as_str())
    }
}

/// A named slice of a codebase, for example `payment` or `ui`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub name: String,
    /// Glob patterns relative to the repository root.
    pub paths: Vec<String>,
}

/// One session as the war room shows it: a pane in the grid, or a chip in the quiet section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pane {
    pub id: String,
    pub title: String,
    pub codebase: String,
    pub part: String,
    pub provider: String,
    pub model: String,
    pub state: SessionState,
    /// When the session entered `state`. The meta reads `waits N min`, `idle N min` or `N min`.
    pub since: UnixMs,
    /// USD.
    pub cost: f64,
    /// Tokens in and out of the session so far, without prompt-cache reads (the war room
    /// Overview). 0 when unknown.
    #[serde(default, skip_serializing_if = "crate::common::is_zero")]
    pub tokens: u64,
    /// aivoryd runs the agent process: it can answer an ask, pause and stop. A session that
    /// runs in a terminal is `false`; its asks are answered there.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub controlled: bool,
    pub context_pct: u8,
    /// The task prompt.
    pub prompt: String,
    /// Set for [`SessionState::NeedsYou`] and [`SessionState::Error`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The subagent switcher (the main agent is implicit).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<Subagent>,
    /// The last lines of the main transcript, oldest first.
    pub tail: Vec<Line>,
    /// Lines in the full main transcript. `tail` holds the last of them.
    pub lines_total: u32,
    pub composer: ComposerPrefs,
    /// The task that started this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl Pane {
    /// Sankey flow value: the main agent plus its subagents.
    pub fn agents(&self) -> u32 {
        1 + self.subagents.len() as u32
    }

    /// Whole minutes in the current state at `now`.
    pub fn minutes(&self, now: UnixMs) -> i64 {
        (now - self.since).max(0) / crate::common::MINUTE_MS
    }
}

/// A subagent of a session. `session/history` with `subagent` gives its transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subagent {
    pub id: String,
    pub name: String,
    pub state: SessionState,
    /// What the main agent asked it to do.
    pub task: String,
}

/// One minute of the pulse chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySample {
    /// Start of the minute.
    pub at: UnixMs,
    /// Agents that work (upper half, ink).
    pub active: u32,
    /// Agents that wait for the user, errors included (lower half, orange).
    pub waiting: u32,
    /// Idle agents (lower half, grey).
    pub idle: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum QueueOrder {
    Priority,
    OldestFirst,
    SmallestFirst,
}

/// The autopilot rules (war-room popover and Settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Autopilot {
    pub on: bool,
    /// 5-120, step 5.
    pub park_after_min: u32,
    /// 1-64.
    pub max_running: u32,
    /// 1-16.
    pub max_per_codebase: u32,
    pub start_queued: bool,
    pub order: QueueOrder,
}

impl Autopilot {
    pub const PARK_AFTER_MIN: (u32, u32, u32) = (5, 120, 5);
    pub const MAX_RUNNING: (u32, u32) = (1, 64);
    pub const MAX_PER_CODEBASE: (u32, u32) = (1, 16);

    /// Bring every value into its range. `park_after_min` snaps to the step.
    pub fn clamp(&mut self) {
        let (lo, hi, step) = Self::PARK_AFTER_MIN;
        self.park_after_min = (self.park_after_min.clamp(lo, hi) / step) * step;
        self.max_running = self
            .max_running
            .clamp(Self::MAX_RUNNING.0, Self::MAX_RUNNING.1);
        self.max_per_codebase = self
            .max_per_codebase
            .clamp(Self::MAX_PER_CODEBASE.0, Self::MAX_PER_CODEBASE.1);
    }
}

impl Default for Autopilot {
    fn default() -> Self {
        Self {
            on: true,
            park_after_min: 15,
            max_running: 14,
            max_per_codebase: 4,
            start_queued: true,
            order: QueueOrder::Priority,
        }
    }
}

/// A provider usage window, for example `claude 5 h window 62%`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub provider: String,
    /// `5 h window`, `Week`, `Daily quota`, `Monthly budget $60`.
    pub label: String,
    pub pct: u8,
    /// Tokens counted in the window (0 when unknown).
    #[serde(default, skip_serializing_if = "crate::common::is_zero")]
    pub used: u64,
    /// When the window frees up: the oldest counted call leaves it. `None` when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<UnixMs>,
    /// The value comes from local transcripts, not from the provider. Show "estimate".
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub estimate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LinkState {
    Connected,
    Connecting,
    Offline,
    /// Not configured.
    Off,
}

/// The complete war-room view. `warroom/snapshot` returns it; `warroom/subscribe` keeps it
/// current with [`crate::rpc::Delta`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WarRoom {
    /// Changes when the daemon restarts. A cursor is valid only with its epoch.
    pub epoch: u64,
    /// The sequence number of the last delta in this view.
    pub cursor: u64,
    /// Daemon time of the view.
    pub now: UnixMs,
    pub codebases: Vec<Codebase>,
    /// Live, parked and done sessions, in a stable order.
    pub sessions: Vec<Pane>,
    /// Oldest first, [`ACTIVITY_SAMPLES`] at most.
    pub activity: Vec<ActivitySample>,
    pub autopilot: Autopilot,
    /// Tasks that wait for a free autopilot slot.
    pub queued: u32,
    /// Open inbox items (the rail badge).
    pub inbox_open: u32,
    pub usage: Vec<UsageWindow>,
    pub hub: LinkState,
    pub relay: LinkState,
    /// The gates of the board and the order of the asks. The daemon computes it
    /// ([`WarRoom::compute_board`]) and keeps it current with [`crate::rpc::Delta::Board`].
    /// Old JSON has no board: empty.
    #[serde(default)]
    pub board: Board,
}

/// One gate of the board: the count and the session ids.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardGate {
    pub count: u32,
    pub ids: Vec<String>,
}

/// The computed board of the war room. Every display shows these gates; none computes them.
///
/// Gate rule, for live sessions only (`Working`, `NeedsYou`, `Error`, `Idle`):
/// `NeedsYou` goes to `needs_you`, `Working` to `working`, `Error` to `blocked`, `Idle` to
/// `idle`. Parked and done sessions are in no gate. In a gate the sessions with an open ask
/// come first, then the longest wait (the oldest `since`), then the id.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub needs_you: BoardGate,
    pub working: BoardGate,
    pub blocked: BoardGate,
    pub idle: BoardGate,
    /// The sessions with an open ask, longest wait first.
    pub asks: Vec<String>,
}

/// Counts for the header pills and the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WarRoomCounts {
    pub needs_you: u32,
    pub error: u32,
    pub working: u32,
    pub idle: u32,
    /// Live sessions (`N running` in the status bar).
    pub live: u32,
    /// Parked and done sessions.
    pub quiet: u32,
    /// Live agents, subagents included.
    pub agents: u32,
}

impl WarRoom {
    /// The board of the current sessions (see [`Board`] for the gate rule).
    pub fn compute_board(&self) -> Board {
        let mut live: Vec<&Pane> = self.sessions.iter().filter(|s| s.state.is_live()).collect();
        live.sort_by(|a, b| {
            (a.ask.is_none(), a.since, &a.id).cmp(&(b.ask.is_none(), b.since, &b.id))
        });
        let gate = |state: SessionState| {
            let ids: Vec<String> = live
                .iter()
                .filter(|s| s.state == state)
                .map(|s| s.id.clone())
                .collect();
            BoardGate {
                count: ids.len() as u32,
                ids,
            }
        };
        let mut asks: Vec<&Pane> = live.iter().copied().filter(|s| s.ask.is_some()).collect();
        asks.sort_by(|a, b| (a.since, &a.id).cmp(&(b.since, &b.id)));
        Board {
            needs_you: gate(SessionState::NeedsYou),
            working: gate(SessionState::Working),
            blocked: gate(SessionState::Error),
            idle: gate(SessionState::Idle),
            asks: asks.iter().map(|s| s.id.clone()).collect(),
        }
    }

    /// True when a free autopilot slot exists for a new session in `codebase`: fewer running
    /// sessions (`Working` or `NeedsYou`) than `maxRunning` in all, and than `maxPerCodebase`
    /// in the codebase.
    pub fn has_free_slot(&self, codebase: &str) -> bool {
        let running = |s: &&Pane| matches!(s.state, SessionState::Working | SessionState::NeedsYou);
        let all = self.sessions.iter().filter(running).count() as u32;
        let own = self
            .sessions
            .iter()
            .filter(running)
            .filter(|s| s.codebase == codebase)
            .count() as u32;
        all < self.autopilot.max_running && own < self.autopilot.max_per_codebase
    }

    pub fn session(&self, id: &str) -> Option<&Pane> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn session_mut(&mut self, id: &str) -> Option<&mut Pane> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn codebase(&self, id: &str) -> Option<&Codebase> {
        self.codebases.iter().find(|c| c.id == id)
    }

    pub fn counts(&self) -> WarRoomCounts {
        let mut c = WarRoomCounts::default();
        for s in &self.sessions {
            match s.state {
                SessionState::NeedsYou => c.needs_you += 1,
                SessionState::Error => c.error += 1,
                SessionState::Working => c.working += 1,
                SessionState::Idle => c.idle += 1,
                SessionState::Parked | SessionState::Done => c.quiet += 1,
            }
            if s.state.is_live() {
                c.live += 1;
                c.agents += s.agents();
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{Ask, AskKind, Effort, Mode};

    fn pane(id: &str, state: SessionState, since: i64, ask: bool) -> Pane {
        Pane {
            id: id.into(),
            title: id.into(),
            codebase: "cb".into(),
            part: "p".into(),
            provider: "claude".into(),
            model: "m".into(),
            state,
            since,
            cost: 0.0,
            tokens: 0,
            controlled: true,
            context_pct: 0,
            prompt: String::new(),
            ask: ask.then(|| Ask {
                id: format!("ask-{id}"),
                kind: AskKind::Question,
                text: "q".into(),
                options: vec![],
                steps: vec![],
                reason: None,
                risky: false,
            }),
            subagents: vec![],
            tail: vec![],
            lines_total: 0,
            composer: ComposerPrefs {
                mode: Mode::Edit,
                provider: "claude".into(),
                model: "m".into(),
                effort: Effort::High,
            },
            task: None,
        }
    }

    fn room(sessions: Vec<Pane>) -> WarRoom {
        WarRoom {
            epoch: 1,
            cursor: 0,
            now: 0,
            codebases: vec![],
            sessions,
            activity: vec![],
            autopilot: Autopilot::default(),
            queued: 0,
            inbox_open: 0,
            usage: vec![],
            hub: LinkState::Off,
            relay: LinkState::Off,
            board: Board::default(),
        }
    }

    #[test]
    fn gates_follow_the_state() {
        let w = room(vec![
            pane("a", SessionState::Working, 5, false),
            pane("b", SessionState::NeedsYou, 9, true),
            pane("c", SessionState::Error, 3, false),
            pane("d", SessionState::Idle, 1, false),
            pane("e", SessionState::Parked, 0, false),
            pane("f", SessionState::Done, 0, false),
        ]);
        let b = w.compute_board();
        assert_eq!(b.needs_you.ids, ["b"]);
        assert_eq!(b.working.count, 1);
        assert_eq!(b.blocked.ids, ["c"]);
        assert_eq!(b.idle.ids, ["d"]);
        assert_eq!(b.asks, ["b"]);
    }

    #[test]
    fn asks_come_longest_wait_first() {
        let w = room(vec![
            pane("new", SessionState::NeedsYou, 90, true),
            pane("old", SessionState::NeedsYou, 10, true),
            pane("mid", SessionState::Error, 50, true),
        ]);
        assert_eq!(w.compute_board().asks, ["old", "mid", "new"]);
    }

    #[test]
    fn a_gate_puts_asks_before_the_rest() {
        let w = room(vec![
            pane("w1", SessionState::Working, 1, false),
            pane("w2", SessionState::Working, 5, true),
            pane("w3", SessionState::Working, 3, false),
        ]);
        assert_eq!(w.compute_board().working.ids, ["w2", "w1", "w3"]);
    }

    #[test]
    fn an_empty_room_has_an_empty_board() {
        assert_eq!(room(vec![]).compute_board(), Board::default());
    }

    #[test]
    fn old_json_has_no_board() {
        let mut v = serde_json::to_value(room(vec![])).unwrap();
        v.as_object_mut().unwrap().remove("board");
        let w: WarRoom = serde_json::from_value(v).unwrap();
        assert_eq!(w.board, Board::default());
    }

    #[test]
    fn a_free_slot_needs_room_in_total_and_per_codebase() {
        let mut w = room(vec![
            pane("a", SessionState::Working, 1, false),
            pane("b", SessionState::NeedsYou, 1, true),
            pane("c", SessionState::Idle, 1, false),
        ]);
        assert!(w.has_free_slot("cb"));
        w.autopilot.max_per_codebase = 2;
        assert!(!w.has_free_slot("cb"));
        assert!(w.has_free_slot("other"));
        w.autopilot.max_running = 2;
        assert!(!w.has_free_slot("other"));
    }
}
