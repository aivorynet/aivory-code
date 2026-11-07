use aivory_sessions::Provider;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Token counts of one turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens served from the prompt cache.
    pub cached_input_tokens: u64,
    pub reasoning_tokens: u64,
}

/// One normalised event of an agent turn. The same enum serves all providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The CLI accepted the session.
    Started {
        session_id: Option<String>,
        model: Option<String>,
        cwd: Option<String>,
    },
    /// A piece of the assistant answer. Claude streams real deltas; Codex and Gemini send whole messages.
    TextDelta { text: String },
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
    },
    /// The agent asked to use a tool that needs approval. `outcome` is set when the CLI already decided.
    PermissionRequest {
        id: String,
        tool: String,
        input: Value,
        outcome: Option<String>,
    },
    /// The turn ended well.
    TurnCompleted {
        usage: Option<Usage>,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
    },
    /// A failure. `fatal` is true when the turn cannot continue.
    Error { message: String, fatal: bool },
    /// An event that the parser does not map. `kind` is the CLI event type, `data` the original JSON.
    Raw {
        provider: Provider,
        kind: String,
        data: Value,
    },
}

impl AgentEvent {
    pub(crate) fn raw(provider: Provider, kind: impl Into<String>, data: Value) -> Self {
        AgentEvent::Raw {
            provider,
            kind: kind.into(),
            data,
        }
    }

    /// `true` for the events that end a turn.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentEvent::TurnCompleted { .. } | AgentEvent::Error { fatal: true, .. }
        )
    }
}
