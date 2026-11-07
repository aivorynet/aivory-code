//! The read-only code tools of the agents: `references`, `definition`, `symbols`, `outline`,
//! and `diagnostics` (`docs/design/02-orchestration.md`, section "Agent tools").
//!
//! The server only routes the calls. A provider ([`AgentTools`]) answers them in the compact
//! text format of the code index. The daemon sets one provider for the whole process
//! ([`set_default_tools`]). A server can have an own provider ([`super::McpServer::set_tools`])
//! and the working folder of its run ([`super::McpServer::set_cwd`]), which the provider uses to
//! pick the codebase. Without a provider the server lists and serves only `permission`.

use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use serde_json::{Value, json};

/// The names of the code tools.
pub const CODE_TOOLS: [&str; 5] = [
    "references",
    "definition",
    "symbols",
    "outline",
    "diagnostics",
];

/// The prefix that Claude Code gives to the tools of the server `aivory`.
const PREFIX: &str = "mcp__aivory__";

/// Answers the code tools. All tools are read-only: no call changes a file.
pub trait AgentTools: Send + Sync {
    /// Run a tool. `cwd` is the working folder of the run, when the server knows it. The
    /// answer is plain text. An `Err` text goes to the agent as an error result.
    fn call(&self, tool: &str, args: &Value, cwd: Option<&Path>) -> Result<String, String>;

    /// Run a tool for a session. `session` is the native session id of the run, when the server
    /// knows it ([`super::McpServer::set_session`]). The default ignores it.
    fn call_as(
        &self,
        tool: &str,
        args: &Value,
        cwd: Option<&Path>,
        session: Option<&str>,
    ) -> Result<String, String> {
        let _ = session;
        self.call(tool, args, cwd)
    }

    /// The definitions of the tools that the provider serves besides the code tools (the
    /// coordination tools `claim`, `ping`, and `subscribe`). The server lists them and sends
    /// the calls to the provider. The calls pass the permission tool without an ask.
    fn extra_tools(&self) -> Vec<Value> {
        Vec::new()
    }
}

static DEFAULT: RwLock<Option<Arc<dyn AgentTools>>> = RwLock::new(None);

/// Set the provider for every server that has no own provider. `None` removes it.
pub fn set_default_tools(tools: Option<Arc<dyn AgentTools>>) {
    *DEFAULT.write().unwrap_or_else(PoisonError::into_inner) = tools;
}

pub(super) fn default_tools() -> Option<Arc<dyn AgentTools>> {
    DEFAULT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// True for the name of a code tool, with or without the Claude Code prefix.
pub(super) fn is_code_tool(name: &str) -> bool {
    let bare = name.strip_prefix(PREFIX).unwrap_or(name);
    CODE_TOOLS.contains(&bare)
}

/// True for `mcp__aivory__<code tool>`: the permission tool lets these calls pass.
pub(super) fn is_prefixed_code_tool(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .is_some_and(|bare| CODE_TOOLS.contains(&bare))
}

/// True when the provider serves the tool as an extra tool (see [`AgentTools::extra_tools`]).
pub(super) fn serves(provider: &dyn AgentTools, name: &str) -> bool {
    let bare = name.strip_prefix(PREFIX).unwrap_or(name);
    provider
        .extra_tools()
        .iter()
        .any(|d| d.get("name").and_then(Value::as_str) == Some(bare))
}

/// True for `mcp__aivory__<extra tool>` of the provider: the permission tool lets it pass.
pub(super) fn is_prefixed_extra_tool(provider: &dyn AgentTools, name: &str) -> bool {
    name.starts_with(PREFIX) && serves(provider, name)
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// The tool definitions for `tools/list`.
pub(super) fn definitions() -> Vec<Value> {
    let codebase = json!({ "type": "string", "description": "The codebase id. Only needed when the daemon knows more than one codebase and the call cannot tell which." });
    let page = json!({ "type": "integer", "minimum": 1, "description": "The result page, from 1. The result tells when there is a next page." });
    vec![
        json!({
            "name": "references",
            "description": "Find the uses of a name in the code index, with file, line, and the text of the line. Use it instead of grep for a function, type, or constant. The index matches names, not types: a common name has hits of other things.",
            "inputSchema": schema(json!({
                "symbol": { "type": "string", "description": "The name, for example `total` or `Invoice::total`." },
                "page": page, "codebase": codebase,
            }), &["symbol"]),
        }),
        json!({
            "name": "definition",
            "description": "Find where a name is declared: file, line, and the declaration header. A qualified name such as `billing::Invoice::total` narrows the result.",
            "inputSchema": schema(json!({
                "symbol": { "type": "string" }, "page": page, "codebase": codebase,
            }), &["symbol"]),
        }),
        json!({
            "name": "symbols",
            "description": "Search the declarations of the codebase by name: a name, a prefix, or a few letters in order (`inv` finds `Invoice`).",
            "inputSchema": schema(json!({
                "query": { "type": "string" },
                "kind": { "type": "string", "description": "Only this kind: function, method, struct, class, enum, trait, interface, constant, type_alias, module, and others." },
                "page": page, "codebase": codebase,
            }), &["query"]),
        }),
        json!({
            "name": "outline",
            "description": "The structure of one file: its declarations with line numbers. Use it before you read a large file.",
            "inputSchema": schema(json!({
                "path": { "type": "string", "description": "The path relative to the codebase root." },
                "codebase": codebase,
            }), &["path"]),
        }),
        json!({
            "name": "diagnostics",
            "description": "The findings of the syntax check and the rule pack (long functions, deep nesting, risky calls) for one file, or for all files that changed in the checkout.",
            "inputSchema": schema(json!({
                "path": { "type": "string", "description": "One file. Default: all changed files." },
                "codebase": codebase,
            }), &[]),
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_with_and_without_the_prefix() {
        assert!(is_code_tool("references"));
        assert!(is_code_tool("mcp__aivory__outline"));
        assert!(!is_code_tool("permission"));
        assert!(is_prefixed_code_tool("mcp__aivory__symbols"));
        assert!(!is_prefixed_code_tool("symbols"));
        assert!(!is_prefixed_code_tool("mcp__aivory__permission"));
    }

    #[test]
    fn every_tool_has_a_definition() {
        let defs = definitions();
        let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
        assert_eq!(names, CODE_TOOLS);
    }
}
