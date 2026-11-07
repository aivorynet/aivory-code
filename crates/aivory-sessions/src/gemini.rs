//! Gemini store: `<root>/tmp/<project-name|hash>/chats/session-<ts>-<id8>.{json,jsonl}`.
//!
//! Two on-disk formats exist:
//! - `.json`: one object `{sessionId, projectHash, startTime, lastUpdated, messages[]}` (older CLI).
//! - `.jsonl` (CLI 0.49): a header line `{sessionId, projectHash, startTime, lastUpdated, kind}`,
//!   then message records (`{id, timestamp, type, content, ...}`; a repeated `id` replaces the
//!   earlier record), `{"$set": {...}}` metadata updates, and `{"$rewindTo": id}` records.
//!
//! The working directory is not in the session file. It comes from `<project>/.project_root`
//! or from the reverse map of `<root>/projects.json`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use crate::jsonl::read_head_tail;
use crate::model::{Provider, SessionSummary};
use crate::store::{FileRole, Parsed, SessionFile, SessionStore, mtime_ms};
use crate::text::{clean_title, clean_title_lenient, content_text};
use crate::timeutil::parse_rfc3339_ms;

const HEAD_BYTES: usize = 128 * 1024;
const TAIL_BYTES: usize = 64 * 1024;
/// A `.json` file up to this size is parsed whole. A larger one gets header fields only.
const WHOLE_JSON_MAX: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct GeminiStore {
    root: PathBuf,
}

impl GeminiStore {
    /// `root` is the Gemini home directory (normally `~/.gemini`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn project_names(&self) -> HashMap<String, String> {
        // projects.json: {"projects": {"<cwd>": "<dir name>"}}. Keep only unique names.
        let mut by_name: HashMap<String, Option<String>> = HashMap::new();
        if let Ok(bytes) = fs::read(self.root.join("projects.json"))
            && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
            && let Some(map) = v.get("projects").and_then(Value::as_object)
        {
            for (cwd, name) in map {
                if let Some(name) = name.as_str() {
                    by_name
                        .entry(name.to_string())
                        .and_modify(|e| *e = None)
                        .or_insert_with(|| Some(cwd.clone()));
                }
            }
        }
        by_name
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect()
    }
}

#[derive(Deserialize, Default)]
struct JsonFile {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "startTime")]
    start_time: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize, Default)]
struct Message {
    #[serde(rename = "type")]
    kind: Option<String>,
    content: Option<Value>,
    model: Option<String>,
}

impl Message {
    fn user_title(&self) -> Option<String> {
        if self.kind.as_deref() != Some("user") {
            return None;
        }
        let text = content_text(self.content.as_ref()?);
        if text.trim_start().starts_with("<session_context>") {
            return None;
        }
        clean_title(&text).or_else(|| clean_title_lenient(&text))
    }
}

impl SessionStore for GeminiStore {
    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    fn enumerate(&self) -> Vec<SessionFile> {
        let Ok(dir) = fs::read_dir(self.root.join("tmp")) else {
            return Vec::new();
        };
        let names = self.project_names();
        let projects: Vec<PathBuf> = dir
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        projects
            .par_iter()
            .flat_map_iter(|p| scan_project(p, &names))
            .collect()
    }

    fn parse(&self, file: &SessionFile) -> Option<Parsed> {
        let is_jsonl = file.path.extension().and_then(|e| e.to_str()) == Some("jsonl");
        let s = if is_jsonl {
            parse_jsonl(file)?
        } else {
            parse_json(file)?
        };
        Some(Parsed::Session(s))
    }
}

fn scan_project(dir: &Path, names: &HashMap<String, String>) -> Vec<SessionFile> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir.join("chats")) else {
        return out;
    };
    let cwd_hint = fs::read_to_string(dir.join(".project_root"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| names.get(n).cloned())
        });
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("session-") || !(name.ends_with(".json") || name.ends_with(".jsonl")) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        out.push(SessionFile {
            path: entry.path(),
            mtime_ms: mtime_ms(&meta),
            size: meta.len(),
            role: FileRole::Session,
            cwd_hint: cwd_hint.clone(),
        });
    }
    out
}

fn parse_json(file: &SessionFile) -> Option<SessionSummary> {
    let (id, created, title, model) = if file.size <= WHOLE_JSON_MAX {
        let bytes = fs::read(&file.path).ok()?;
        let parsed: JsonFile = serde_json::from_slice(&bytes).ok()?;
        let title = parsed
            .summary
            .as_deref()
            .and_then(clean_title_lenient)
            .or_else(|| parsed.messages.iter().find_map(Message::user_title));
        let model = parsed.messages.iter().rev().find_map(|m| m.model.clone());
        (
            parsed.session_id?,
            parsed.start_time.as_deref().and_then(parse_rfc3339_ms),
            title,
            model,
        )
    } else {
        let ht = read_head_tail(&file.path, file.size, 4096, 0).ok()?;
        let text = String::from_utf8_lossy(&ht.head);
        let id = string_after_key(&text, "sessionId")?;
        (
            id,
            string_after_key(&text, "startTime")
                .as_deref()
                .and_then(parse_rfc3339_ms),
            None,
            None,
        )
    };
    Some(summary(file, id, created, title, model))
}

fn parse_jsonl(file: &SessionFile) -> Option<SessionSummary> {
    let ht = read_head_tail(&file.path, file.size, HEAD_BYTES, TAIL_BYTES).ok()?;
    let mut header: Option<Value> = None;
    let mut summary_text = None;
    let mut title = None;
    let mut model = None;
    for raw in ht.head_lines().chain(ht.tail_lines()) {
        let Ok(v) = serde_json::from_slice::<Value>(raw) else {
            continue;
        };
        if header.is_none() {
            if v.get("sessionId").is_some() {
                if v.get("kind")
                    .and_then(Value::as_str)
                    .is_some_and(|k| k != "main")
                {
                    return None; // sub-agent chat file, not a resumable session
                }
                header = Some(v);
            }
            continue;
        }
        if let Some(set) = v.get("$set") {
            if let Some(s) = set.get("summary").and_then(Value::as_str) {
                summary_text = clean_title_lenient(s);
            }
            continue;
        }
        let Ok(msg) = serde_json::from_value::<Message>(v) else {
            continue;
        };
        if title.is_none() {
            title = msg.user_title();
        }
        if msg.kind.as_deref() == Some("gemini")
            && let Some(m) = msg.model
        {
            model = Some(m);
        }
    }
    let header = header?;
    let id = header.get("sessionId")?.as_str()?.to_string();
    let created = header
        .get("startTime")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_ms);
    Some(summary(file, id, created, summary_text.or(title), model))
}

fn summary(
    file: &SessionFile,
    id: String,
    created: Option<i64>,
    title: Option<String>,
    model: Option<String>,
) -> SessionSummary {
    SessionSummary {
        provider: Provider::Gemini,
        id,
        cwd: None,
        title,
        created,
        updated: file.mtime_ms,
        git_branch: None,
        cli_version: None,
        model,
        path: file.path.clone(),
        sidechains: Vec::new(),
    }
}

/// Find `"key": "value"` in raw JSON text without a full parse.
fn string_after_key(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let rest = &text[text.find(&needle)? + needle.len()..];
    let rest = rest
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_string_after_key() {
        let t = r#"{ "sessionId": "abc-1", "startTime" : "2026-01-01T00:00:00Z", "messages": ["#;
        assert_eq!(string_after_key(t, "sessionId").as_deref(), Some("abc-1"));
        assert_eq!(
            string_after_key(t, "startTime").as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(string_after_key(t, "missing"), None);
    }
}
