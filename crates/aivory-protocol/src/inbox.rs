//! The inbox (`Inbox.dc.html`, phone Focus and Approval).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{AgentRef, AskKind, Gate, Line, UnixMs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum InboxKind {
    Permission,
    Plan,
    Question,
    Error,
    Review,
    Conflict,
}

impl InboxKind {
    pub fn filter(self) -> InboxFilter {
        match self {
            Self::Permission | Self::Plan | Self::Question => InboxFilter::Answers,
            Self::Error => InboxFilter::Errors,
            Self::Review => InboxFilter::Reviews,
            Self::Conflict => InboxFilter::Conflicts,
        }
    }

    /// The label of a kind pill: `Permission`, `Plan`, ...
    pub fn label(self) -> &'static str {
        match self {
            Self::Permission => "Permission",
            Self::Plan => "Plan",
            Self::Question => "Question",
            Self::Error => "Error",
            Self::Review => "Review",
            Self::Conflict => "Conflict",
        }
    }

    /// The head of the ask card.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Permission => AskKind::Permission.label(),
            Self::Plan => AskKind::Plan.label(),
            Self::Question => AskKind::Question.label(),
            Self::Error => AskKind::Error.label(),
            Self::Review => "Ready for review",
            Self::Conflict => "Two sessions edit the same code",
        }
    }
}

impl From<AskKind> for InboxKind {
    fn from(k: AskKind) -> Self {
        match k {
            AskKind::Permission => Self::Permission,
            AskKind::Plan => Self::Plan,
            AskKind::Question => Self::Question,
            AskKind::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum InboxFilter {
    All,
    Answers,
    Errors,
    Reviews,
    Conflicts,
}

impl InboxFilter {
    pub fn matches(self, kind: InboxKind) -> bool {
        self == Self::All || kind.filter() == self
    }
}

/// One side of a conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub title: String,
    pub lines: Vec<Line>,
}

/// One thing that needs the user. The first option is the primary button.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub id: String,
    pub kind: InboxKind,
    /// The session that asks (for a conflict: the session that found it).
    pub session: String,
    /// The ask of that session, if the item answers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
    pub title: String,
    pub codebase: String,
    pub part: String,
    /// One agent, or two for a conflict.
    pub agents: Vec<AgentRef>,
    /// When the item opened.
    pub since: UnixMs,
    /// USD.
    pub cost: f64,
    pub context_pct: u8,
    /// One line for the list card.
    pub preview: String,
    /// The text of the ask card.
    pub text: String,
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<Gate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sides: Vec<ConflictSide>,
    pub prompt: String,
    pub files: Vec<String>,
    /// The transcript tail of the session, oldest first.
    pub tail: Vec<Line>,
    /// Set when the item is done: `allowed`, `denied`, `approved`, `merged`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// Result of `inbox/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxList {
    /// Open items, in list order.
    pub items: Vec<InboxItem>,
    /// Items done in this daemon run, newest first.
    pub done: Vec<InboxItem>,
}
