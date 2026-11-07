//! One handle for a Claude, Codex, or Gemini run.

use std::sync::mpsc::Receiver;

use aivory_protocol::SessionState;

use super::{LiveError, LiveRun, RpcRun, RunEvent, StartOptions};
use crate::error::AgentError;
use crate::mcp::Answer;

/// A live run of any provider. The daemon keeps one of these per pane.
#[derive(Debug)]
pub enum AnyRun {
    Claude(LiveRun),
    Rpc(RpcRun),
}

impl AnyRun {
    /// Start a run for `provider`: `claude`, `codex`, or `gemini`.
    pub fn start(
        provider: &str,
        opts: StartOptions,
    ) -> Result<(Self, Receiver<RunEvent>), AgentError> {
        match provider {
            "claude" => LiveRun::start(opts).map(|(r, rx)| (Self::Claude(r), rx)),
            "codex" => RpcRun::start_codex(opts).map(|(r, rx)| (Self::Rpc(r), rx)),
            "gemini" => RpcRun::start_gemini(opts).map(|(r, rx)| (Self::Rpc(r), rx)),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("no live runs for '{other}'"),
            )
            .into()),
        }
    }

    /// Whether the provider has live runs.
    pub fn supports(provider: &str) -> bool {
        matches!(provider, "claude" | "codex" | "gemini")
    }

    pub fn session_id(&self) -> Option<String> {
        match self {
            Self::Claude(r) => r.session_id(),
            Self::Rpc(r) => r.session_id(),
        }
    }

    pub fn state(&self) -> SessionState {
        match self {
            Self::Claude(r) => r.state(),
            Self::Rpc(r) => r.state(),
        }
    }

    pub fn send(&self, text: &str) -> Result<(), LiveError> {
        match self {
            Self::Claude(r) => r.send(text),
            Self::Rpc(r) => r.send(text),
        }
    }

    pub fn answer(&self, ask_id: &str, answer: Answer) -> Result<(), LiveError> {
        match self {
            Self::Claude(r) => r.answer(ask_id, answer),
            Self::Rpc(r) => r.answer(ask_id, answer),
        }
    }

    pub fn interrupt(&self) -> Result<(), LiveError> {
        match self {
            Self::Claude(r) => r.interrupt(),
            Self::Rpc(r) => r.interrupt(),
        }
    }

    pub fn kill(&self) {
        match self {
            Self::Claude(r) => r.kill(),
            Self::Rpc(r) => r.kill(),
        }
    }
}
