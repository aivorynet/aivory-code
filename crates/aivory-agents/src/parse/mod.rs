//! Tolerant line parsers: one per provider, one shared [`EventParser`] trait.

mod claude;
mod codex;
mod gemini;

use aivory_sessions::Provider;
use serde_json::Value;

use crate::event::AgentEvent;

pub use claude::ClaudeParser;
pub use codex::CodexParser;
pub use gemini::GeminiParser;

/// Turns the stdout lines of one CLI run into [`AgentEvent`]s. A parser may keep state.
pub trait EventParser: Send {
    /// Map one stdout line to zero or more events. Never fails: odd input becomes `Raw`.
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent>;
}

/// The parser for a provider.
pub fn parser_for(provider: Provider) -> Box<dyn EventParser> {
    match provider {
        Provider::Claude => Box::new(ClaudeParser::default()),
        Provider::Codex => Box::new(CodexParser::default()),
        Provider::Gemini => Box::new(GeminiParser),
    }
}

/// Parse a line as a JSON object, or give back the `Raw` event for it.
pub(crate) fn json_line(provider: Provider, line: &str) -> Result<Value, Vec<AgentEvent>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(Vec::new());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(other) => Err(vec![AgentEvent::raw(provider, "non_object", other)]),
        Err(_) => Err(vec![AgentEvent::raw(
            provider,
            "non_json",
            Value::String(trimmed.to_string()),
        )]),
    }
}

pub(crate) fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn u64_of(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Flatten a tool output (string, list of parts, or any JSON) to text.
pub(crate) fn output_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(parts) if parts.iter().all(|p| p.get("text").is_some()) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}
