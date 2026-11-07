//! The code tools of the agents on the MCP endpoint: `references`, `definition`, `symbols`,
//! `outline`, `diagnostics`. All of them only read. The answers are compact text:
//! [`aivory_index::Page::render`] for the index queries.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aivory_agents::mcp::AgentTools;
use aivory_index::PageReq;
use aivory_syntax::{DeclKind, Language, OutlineItem, Stub};
use serde_json::Value;

use super::diag;
use super::resolve::{Resolver, repo_root};
use super::service::safe_rel;
use super::state::CodeCore;

/// Results on one page.
const PER_PAGE: usize = 20;
/// Findings in one answer.
const MAX_FINDINGS: usize = 50;
/// How long a call waits for the first refresh of an index.
const INDEX_WAIT: Duration = Duration::from_secs(10);
/// The largest file that `outline` reads.
const MAX_FILE: u64 = 2 << 20;

#[derive(Debug)]
pub struct CodeTools {
    core: Arc<CodeCore>,
    resolver: Arc<dyn Resolver>,
}

impl CodeTools {
    pub fn new(core: Arc<CodeCore>, resolver: Arc<dyn Resolver>) -> Self {
        Self { core, resolver }
    }

    /// The codebase of a call: named in the arguments, found from the folder of the run, or the
    /// only one.
    fn context(&self, args: &Value, cwd: Option<&Path>) -> Result<(String, PathBuf), String> {
        let all = self.resolver.codebases();
        let names = || {
            all.iter()
                .map(|(c, _)| c.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(id) = args.get("codebase").and_then(Value::as_str) {
            return all
                .iter()
                .find(|(c, _)| c == id)
                .cloned()
                .ok_or_else(|| format!("unknown codebase '{id}'; known: {}", names()));
        }
        if let Some(cwd) = cwd {
            let repo = repo_root(cwd);
            let found = all
                .iter()
                .filter(|(_, root)| cwd.starts_with(root) || repo == *root)
                .max_by_key(|(_, root)| root.components().count());
            if let Some(found) = found {
                return Ok(found.clone());
            }
        }
        match all.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err("the daemon knows no codebase".into()),
            _ => Err(format!("give the argument `codebase`: one of {}", names())),
        }
    }
}

fn text_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("the argument `{name}` is missing"))
}

fn page_arg(args: &Value) -> usize {
    args.get("page")
        .and_then(Value::as_u64)
        .map_or(1, |p| usize::try_from(p).unwrap_or(1).max(1))
}

impl AgentTools for CodeTools {
    fn call(&self, tool: &str, args: &Value, cwd: Option<&Path>) -> Result<String, String> {
        let (codebase, root) = self.context(args, cwd)?;
        match tool {
            "outline" => outline(&root, text_arg(args, "path")?),
            "diagnostics" => {
                let path = args.get("path").and_then(Value::as_str);
                diagnostics(&root, path)
            }
            "references" | "definition" | "symbols" => {
                let entry = self.core.entry(&root, &codebase)?;
                let ready = entry.wait_ready(INDEX_WAIT);
                let req = PageReq::new(page_arg(args), PER_PAGE);
                let mut text = match tool {
                    "references" => {
                        let sym = text_arg(args, "symbol")?;
                        entry
                            .index
                            .references(None, sym, req)
                            .map_err(|e| e.to_string())?
                            .render(&format!("references(sym: \"{sym}\")"))
                    }
                    "definition" => {
                        let sym = text_arg(args, "symbol")?;
                        entry
                            .index
                            .definition(None, sym, req)
                            .map_err(|e| e.to_string())?
                            .render(&format!("definition(sym: \"{sym}\")"))
                    }
                    _ => {
                        let query = text_arg(args, "query")?;
                        let kind = match args.get("kind").and_then(Value::as_str) {
                            Some(k) => Some(
                                serde_json::from_value::<DeclKind>(Value::String(k.to_owned()))
                                    .map_err(|_| format!("unknown kind '{k}'"))?,
                            ),
                            None => None,
                        };
                        entry
                            .index
                            .symbols(None, query, kind, req)
                            .map_err(|e| e.to_string())?
                            .render(&format!("symbols(query: \"{query}\")"))
                    }
                };
                if !ready {
                    text.push_str("  (the index is still building; the list can be incomplete)\n");
                }
                Ok(text)
            }
            other => Err(format!("unknown tool '{other}'")),
        }
    }
}

fn outline(root: &Path, path: &str) -> Result<String, String> {
    let rel = safe_rel(path).map_err(|e| e.message)?;
    let file = root.join(&rel);
    let real = std::fs::canonicalize(&file).map_err(|_| format!("no file '{rel}'"))?;
    let base = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    if !real.starts_with(&base) || !real.is_file() {
        return Err(format!("'{rel}' is not a file inside the codebase"));
    }
    if std::fs::metadata(&real).map_err(|e| e.to_string())?.len() > MAX_FILE {
        return Err(format!("'{rel}' is too large"));
    }
    let language = Language::detect(&rel).ok_or_else(|| format!("no grammar for '{rel}'"))?;
    let text = std::fs::read_to_string(&real).map_err(|_| format!("'{rel}' is not a text file"))?;
    let stub = Stub::parse(language, &text).map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    for item in &stub.outline {
        outline_lines(item, 1, &mut lines);
    }
    let mut out = format!(
        "outline(path: \"{rel}\")  →  {} item{}\n",
        lines.len(),
        if lines.len() == 1 { "" } else { "s" }
    );
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    Ok(out)
}

fn outline_lines(item: &OutlineItem, depth: usize, out: &mut Vec<String>) {
    let kind = serde_json::to_value(item.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    out.push(format!(
        "{}{kind} {}  :{}",
        "  ".repeat(depth),
        item.name,
        item.name_range.start_line + 1
    ));
    for child in &item.children {
        outline_lines(child, depth + 1, out);
    }
}

fn diagnostics(root: &Path, path: Option<&str>) -> Result<String, String> {
    let (title, files) = match path {
        Some(p) => {
            let rel = safe_rel(p).map_err(|e| e.message)?;
            (format!("diagnostics(path: \"{rel}\")"), vec![rel])
        }
        None => ("diagnostics()".to_owned(), diag::changed_paths(root)),
    };
    let (rows, notes) = diag::quick(root, &files);
    let mut out = format!(
        "{title}  →  {} finding{}\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
    let level = |l: aivory_protocol::code::FindingLevel| {
        serde_json::to_value(l)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    for f in rows.iter().take(MAX_FINDINGS) {
        out.push_str(&format!(
            "  {}:{}:{}   {} {}  {}\n",
            f.path,
            f.line,
            f.col + 1,
            level(f.level),
            f.rule,
            f.message
        ));
    }
    if rows.len() > MAX_FINDINGS {
        out.push_str(&format!("  ... ({} more)\n", rows.len() - MAX_FINDINGS));
    }
    for n in notes
        .iter()
        .filter(|n| n.contains("truncated") || n.contains("cut"))
    {
        out.push_str(&format!("  note: {n}\n"));
    }
    Ok(out)
}
