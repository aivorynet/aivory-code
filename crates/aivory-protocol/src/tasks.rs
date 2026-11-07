//! The task board (`Tasks.dc.html`, phone Voice).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{AskKind, Gate, SessionState, UnixMs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TaskColumn {
    Planned,
    Working,
    Waiting,
    Review,
    Done,
}

impl TaskColumn {
    pub const ALL: [Self; 5] = [
        Self::Planned,
        Self::Working,
        Self::Waiting,
        Self::Review,
        Self::Done,
    ];

    /// The columns that a card can move to by hand (drag and drop, `tasks/update`). The daemon
    /// sets `Waiting` itself when a session asks, so no move goes there. A task that never ran
    /// does not go to Review, and a running task does not go back to Planned.
    pub fn moves_to(self) -> &'static [Self] {
        match self {
            Self::Planned => &[Self::Working, Self::Done],
            Self::Working => &[Self::Review, Self::Done],
            Self::Waiting => &[Self::Working, Self::Review, Self::Done],
            Self::Review => &[Self::Working, Self::Planned, Self::Done],
            Self::Done => &[Self::Review, Self::Planned],
        }
    }

    /// True when the card may move from `self` to `to`. Staying in the column is no move.
    pub fn can_move_to(self, to: Self) -> bool {
        self == to || self.moves_to().contains(&to)
    }

    /// The text of the error for a move that `can_move_to` refuses.
    pub fn refusal(self, to: Self) -> String {
        let allowed: Vec<&str> = self.moves_to().iter().map(|c| c.label()).collect();
        format!(
            "a task in {} cannot move to {}; it can move to {}",
            self.label(),
            to.label(),
            allowed.join(", ")
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Working => "Working",
            Self::Waiting => "Waiting",
            Self::Review => "Review",
            Self::Done => "Done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Playbook {
    Feature,
    Fix,
    Port,
}

impl Playbook {
    /// The five steps of the playbook.
    pub fn steps(self) -> [&'static str; 5] {
        match self {
            Self::Feature => ["Plan", "Implement", "Test", "Review", "Merge"],
            Self::Fix => ["Reproduce", "Fix", "Test", "Review", "Merge"],
            Self::Port => ["Inventory", "Port", "Fixtures", "Review", "Merge"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    NotStarted,
    Working,
    /// Waits for the user.
    Waiting,
    Done,
}

/// One step of the playbook stepper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub name: String,
    pub state: StepState,
    /// Who does the step: `main · opus-5.5`, `test-writer`, `you`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Known for done steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// A session of a task, as the task panel lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskSession {
    /// The session to open. Subagent rows open their parent session.
    pub session: String,
    pub title: String,
    /// `main · opus-5.5`, `sub · running`.
    pub role: String,
    pub state: SessionState,
}

/// How soon a queued task starts when the autopilot order is `Priority`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub column: TaskColumn,
    pub title: String,
    pub codebase: String,
    pub part: String,
    pub playbook: Playbook,
    /// Index of the current step, 0-5 (5 = all done).
    pub step: u8,
    /// Why the task waits (the orange wait reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<AskKind>,
    /// The provider of each agent that works on the task (the provider circles).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    pub prompt: String,
    /// Old JSON has no priority: `Normal`.
    #[serde(default)]
    pub priority: TaskPriority,
    /// When the task was created. Old JSON has no value: `0` (the oldest).
    #[serde(default)]
    pub created_at: UnixMs,
    /// `None` while the task is queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<UnixMs>,
    /// USD.
    pub cost: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<TaskSession>,
    pub gates: Vec<Gate>,
    /// Always five, in playbook order.
    pub steps: Vec<Step>,
}

/// Result of `tasks/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    pub tasks: Vec<Task>,
}
