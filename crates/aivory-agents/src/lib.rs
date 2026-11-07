//! Drivers that run the user's own, logged-in agent CLI and map its output to [`AgentEvent`].
//!
//! The crate never handles credentials. It spawns the native binary (`claude`, `codex`,
//! `gemini`) in the session working directory and reads the JSON lines that the binary prints.
//!
//! Parsers are tolerant: a line that is not JSON, an unknown event type, or an unknown item
//! type becomes [`AgentEvent::Raw`], never an error.

mod command;
pub mod detect;
mod error;
mod event;
pub mod live;
pub mod mcp;
mod parse;
mod run;

pub use command::{CommandSpec, ResumeRequest, build_command, resolve_program};
pub use error::AgentError;
pub use event::{AgentEvent, Usage};
pub use parse::{ClaudeParser, CodexParser, EventParser, GeminiParser, parser_for};
pub use run::{RunOptions, RunOutcome, run_resume};
