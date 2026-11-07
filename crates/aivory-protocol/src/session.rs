//! The session deep dive (`Session.dc.html`): spans, DAG, diff, paged history.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Line, SessionState, UnixMs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SpanKind {
    Run,
    Turn,
    Llm,
    Tool,
    Agent,
}

/// One bar of the waterfall and one entry of the span tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub id: String,
    /// `None` for the run span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub kind: SpanKind,
    pub name: String,
    /// `main` or the subagent name.
    pub agent: String,
    pub model: String,
    pub start: UnixMs,
    /// `None` while the span runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<UnixMs>,
    pub state: SessionState,
    /// Input tokens, including the cache reads and cache writes of the call.
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// The part of `tokens_in` that the prompt cache served. `None` when the transcript does
    /// not tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_cache_read: Option<u64>,
    /// The part of `tokens_in` that wrote the prompt cache (Claude only). `None` when the
    /// transcript does not tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_cache_write: Option<u64>,
    /// USD.
    pub cost: f64,
    /// The tool id for a tool span, for example `bash` or `file_edit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub input: String,
    pub output: String,
}

/// A card of the DAG tab. [`crate::layout::dag`] places the cards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DagNode {
    pub id: String,
    pub title: String,
    pub agent: String,
    /// Done, working (`running`), needs you (`waiting`) or parked (`planned`).
    pub state: SessionState,
    /// Known for done and running nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The span that a click selects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DagEdge {
    pub from: String,
    pub to: String,
}

/// The stats strip of the deep dive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub subagents: u32,
    pub turns: u32,
    pub tool_calls: u32,
    /// USD.
    pub cost: f64,
    pub context_pct: u8,
}

/// Result of `session/spans`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionTrace {
    pub session: String,
    pub stats: SessionStats,
    /// Parents before children, siblings by start time.
    pub spans: Vec<Span>,
    pub nodes: Vec<DagNode>,
    pub edges: Vec<DagEdge>,
}

impl SessionTrace {
    /// The turn number (from 1, in start order) of the turn that holds a span: the span itself
    /// when it is a turn span, else its nearest turn ancestor. `None` when the span is unknown
    /// or has no turn (the run span). The number matches the turn number of the ledger.
    pub fn turn_of_span(&self, span: &str) -> Option<u32> {
        let by_id = |id: &str| self.spans.iter().find(|s| s.id == id);
        let mut at = by_id(span)?;
        let mut hops = 0;
        while at.kind != SpanKind::Turn {
            at = by_id(at.parent.as_deref()?)?;
            hops += 1;
            if hops > 64 {
                return None;
            }
        }
        let mut turns: Vec<&Span> = self
            .spans
            .iter()
            .filter(|s| s.kind == SpanKind::Turn && s.agent == at.agent)
            .collect();
        turns.sort_by_key(|s| s.start);
        let n = turns.iter().position(|s| s.id == at.id)?;
        u32::try_from(n + 1).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DiffLineKind {
    /// `@@ -1,12 +1,46 @@`
    Hunk,
    Context,
    Added,
    Removed,
}

/// Which prompt wrote a line (phone Diff screen, "blame by prompt").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Blame {
    pub turn: u32,
    pub prompt: String,
    pub model: String,
    pub session: String,
    pub at: UnixMs,
    /// USD.
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<u32>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blame: Option<Blame>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub lines: Vec<DiffLine>,
}

/// Result of `session/diff`: the diff of the session worktree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionDiff {
    pub session: String,
    pub files: Vec<DiffFile>,
}

/// Result of `session/history`: lines `start .. start + lines.len()` of `total`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<String>,
    pub start: u32,
    pub total: u32,
    pub lines: Vec<Line>,
}
