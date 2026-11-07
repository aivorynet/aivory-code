//! Machine resources (`resources/memory`) and the organizer agent (`organizer/start`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::UnixMs;

/// The RAM of the machine and of the agent processes (`resources/memory`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub total_mb: u32,
    pub used_mb: u32,
    /// The RAM of all agent processes (claude, codex, gemini and their node helpers).
    pub agents_mb: u32,
    /// Oldest first, one sample each few seconds, about 30 minutes.
    pub samples: Vec<MemSample>,
    /// The agent processes that use the most RAM, largest first.
    pub top: Vec<AgentMem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemSample {
    pub at: UnixMs,
    pub used_mb: u32,
    pub agents_mb: u32,
}

/// One agent process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentMem {
    pub pid: u32,
    /// The program: `claude`, `codex`, `gemini` or `node`.
    pub name: String,
    pub mb: u32,
}

/// What the organizer agent may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OrganizerMode {
    /// It acts: pauses, parks, stops and resumes sessions, sets the autopilot, queues tasks.
    FullAuto,
    /// It watches RAM and sessions and gives feedback; it does not act.
    #[default]
    MemoryAndSessions,
    /// It watches the sessions only (stuck, waiting, conflicts) and gives feedback.
    FeedbackOnly,
    /// It watches RAM only and warns when it runs low.
    MemoryOnly,
}

impl OrganizerMode {
    pub const ALL: [OrganizerMode; 4] = [
        Self::FullAuto,
        Self::MemoryAndSessions,
        Self::FeedbackOnly,
        Self::MemoryOnly,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FullAuto => "Full auto",
            Self::MemoryAndSessions => "Memory + sessions",
            Self::FeedbackOnly => "Feedback only",
            Self::MemoryOnly => "Memory only",
        }
    }
}

/// `organizer/start`: start the organizer agent as a session of aivoryd.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizerStartParams {
    #[serde(default)]
    pub mode: OrganizerMode,
    /// The codebase whose folder the agent runs in (default: the first codebase).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_read_as_camel_case() {
        let p: OrganizerStartParams = serde_json::from_str(r#"{"mode":"fullAuto"}"#).unwrap();
        assert_eq!(p.mode, OrganizerMode::FullAuto);
        assert_eq!(OrganizerMode::ALL.len(), 4);
        assert_eq!(OrganizerMode::default(), OrganizerMode::MemoryAndSessions);
    }
}
