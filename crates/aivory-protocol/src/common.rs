//! Types that several screens share: session state, transcript lines, asks, composer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch, UTC.
pub type UnixMs = i64;

/// Milliseconds in one minute.
pub const MINUTE_MS: i64 = 60_000;

/// The known providers, in the order of the provider menus.
pub const PROVIDERS: [&str; 5] = ["claude", "codex", "gemini", "opencode", "goose"];

/// The one status model of all screens. The declaration order is the display order
/// (needs you first). The design calls the states `q e w i p d`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SessionState {
    /// Waits for an answer (`q`): permission, plan or question.
    NeedsYou,
    /// Stopped on an error (`e`).
    Error,
    /// Runs (`w`).
    Working,
    /// Live, but waits for a task (`i`).
    Idle,
    /// Stopped and resumable (`p`). On the task board: planned.
    Parked,
    /// Finished (`d`).
    Done,
}

impl SessionState {
    pub const ALL: [Self; 6] = [
        Self::NeedsYou,
        Self::Error,
        Self::Working,
        Self::Idle,
        Self::Parked,
        Self::Done,
    ];

    /// The one-letter code of the design (`q e w i p d`).
    pub fn code(self) -> char {
        match self {
            Self::NeedsYou => 'q',
            Self::Error => 'e',
            Self::Working => 'w',
            Self::Idle => 'i',
            Self::Parked => 'p',
            Self::Done => 'd',
        }
    }

    pub fn from_code(code: char) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.code() == code)
    }

    /// The label of the design: `needs you`, `error`, `working`, `idle`, `parked`, `done`.
    pub fn label(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs you",
            Self::Error => "error",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Parked => "parked",
            Self::Done => "done",
        }
    }

    /// Live sessions get a pane in the grid. Parked and done sessions go to the quiet section.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            Self::NeedsYou | Self::Error | Self::Working | Self::Idle
        )
    }

    pub fn needs_attention(self) -> bool {
        matches!(self, Self::NeedsYou | Self::Error)
    }
}

/// What a session in [`SessionState::NeedsYou`] or [`SessionState::Error`] waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AskKind {
    Permission,
    Plan,
    Question,
    Error,
}

impl AskKind {
    /// The label of the ask box: `Permission required`, `Plan approval`, ...
    pub fn label(self) -> &'static str {
        match self {
            Self::Permission => "Permission required",
            Self::Plan => "Plan approval",
            Self::Question => "Question",
            Self::Error => "Stopped on an error",
        }
    }

    /// The short tag of a card: `needs permission`, `plan to approve`, ...
    pub fn tag(self) -> &'static str {
        match self {
            Self::Permission => "needs permission",
            Self::Plan => "plan to approve",
            Self::Question => "question",
            Self::Error => "error",
        }
    }
}

/// A question to the user. The first option is the primary button.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// Changes with every new ask, so a late answer cannot hit the next ask.
    pub id: String,
    pub kind: AskKind,
    /// The command for a permission (show it in mono), else the question.
    pub text: String,
    pub options: Vec<String>,
    /// The steps of a plan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    /// Why the agent asks (phone Approval screen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A risky permission needs a biometric confirmation on the phone.
    #[serde(default, skip_serializing_if = "is_false")]
    pub risky: bool,
}

/// The kind of a transcript line. The design calls them `x u a t o p m s e h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    /// `x`: meta line (session header, idle marker).
    Meta,
    /// `u`: a user prompt, shown as `› text` on a grey band.
    User,
    /// `a`: agent text.
    Agent,
    /// `t`: a tool call, shown as `● Label  text` (bold label, green first letter).
    Tool,
    /// `o`: tool output, shown as `  └ text`.
    Output,
    /// `p`: an added line, shown as `+ text` (green).
    Added,
    /// `m`: a removed line, shown as `- text` (red).
    Removed,
    /// `s`: subagent activity, shown as `↳ label  text` (purple).
    Subagent,
    /// `e`: an error line (red).
    Error,
    /// `h`: a turn divider, shown as `label · text` (deep dive transcript).
    Turn,
}

impl LineKind {
    pub fn code(self) -> char {
        match self {
            Self::Meta => 'x',
            Self::User => 'u',
            Self::Agent => 'a',
            Self::Tool => 't',
            Self::Output => 'o',
            Self::Added => 'p',
            Self::Removed => 'm',
            Self::Subagent => 's',
            Self::Error => 'e',
            Self::Turn => 'h',
        }
    }
}

/// One transcript line. Clients add the decoration (`› `, `  └ `, `● `, `↳ `, `+ `, `- `).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub kind: LineKind,
    /// The bold head: the tool name, the subagent name, or `TURN n`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub text: String,
}

impl Line {
    pub fn new(kind: LineKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            label: None,
            text: text.into(),
        }
    }

    pub fn labeled(kind: LineKind, label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind,
            label: Some(label.into()),
            text: text.into(),
        }
    }

    pub fn meta(text: impl Into<String>) -> Self {
        Self::new(LineKind::Meta, text)
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(LineKind::User, text)
    }
    pub fn agent(text: impl Into<String>) -> Self {
        Self::new(LineKind::Agent, text)
    }
    pub fn tool(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::labeled(LineKind::Tool, name, detail)
    }
    pub fn output(text: impl Into<String>) -> Self {
        Self::new(LineKind::Output, text)
    }
    pub fn added(text: impl Into<String>) -> Self {
        Self::new(LineKind::Added, text)
    }
    pub fn removed(text: impl Into<String>) -> Self {
        Self::new(LineKind::Removed, text)
    }
    pub fn subagent(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self::labeled(LineKind::Subagent, name, text)
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self::new(LineKind::Error, text)
    }
    pub fn turn(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self::labeled(LineKind::Turn, label, text)
    }
}

/// Permission mode of the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Plan,
    Edit,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
}

/// The dropdowns of a composer. A provider change is a handoff to a new session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComposerPrefs {
    pub mode: Mode,
    pub provider: String,
    pub model: String,
    pub effort: Effort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GateState {
    /// `✓`
    Ok,
    /// `○`
    Pending,
    /// `✕`
    Failed,
}

/// A check before a merge: tests, Guard, build, review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    pub label: String,
    pub state: GateState,
}

impl Gate {
    pub fn new(label: impl Into<String>, state: GateState) -> Self {
        Self {
            label: label.into(),
            state,
        }
    }
}

/// A provider and a model, for example `claude · opus-5.5`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentRef {
    pub provider: String,
    pub model: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_zero(n: &u64) -> bool {
    *n == 0
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}
