//! Codex store: `<root>/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`.
//!
//! The first line is `session_meta` with `payload{id, cwd, cli_version, model_provider, git}`.
//! The model comes from the first `turn_context` line, the title from the first
//! `event_msg` of type `user_message`.

use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use crate::jsonl::read_head_tail;
use crate::model::{Provider, SessionSummary};
use crate::store::{FileRole, Parsed, SessionFile, SessionStore, mtime_ms};
use crate::text::clean_title_lenient;
use crate::timeutil::parse_rfc3339_ms;

/// The first line (`session_meta`) holds the base instructions and is 10-25 KB.
const HEAD_BYTES: usize = 128 * 1024;
const TAIL_BYTES: usize = 32 * 1024;
const MAX_DEPTH: u8 = 6;

#[derive(Debug, Clone)]
pub struct CodexStore {
    root: PathBuf,
}

impl CodexStore {
    /// `root` is the Codex home directory (normally `~/.codex`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<Value>,
}

impl SessionStore for CodexStore {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn enumerate(&self) -> Vec<SessionFile> {
        let Ok(dir) = fs::read_dir(self.root.join("sessions")) else {
            return Vec::new();
        };
        let tops: Vec<PathBuf> = dir.filter_map(Result::ok).map(|e| e.path()).collect();
        tops.par_iter()
            .flat_map_iter(|p| {
                let mut out = Vec::new();
                walk(p, 0, &mut out);
                out
            })
            .collect()
    }

    fn parse(&self, file: &SessionFile) -> Option<Parsed> {
        let ht = read_head_tail(&file.path, file.size, HEAD_BYTES, TAIL_BYTES).ok()?;
        let mut id = None;
        let mut cwd = None;
        let mut created = None;
        let mut cli_version = None;
        let mut branch = None;
        let mut model = None;
        let mut mode = None;
        let mut effort = None;
        let mut title = None;
        for raw in ht.head_lines().chain(ht.tail_lines()) {
            let Ok(line) = serde_json::from_slice::<Line>(raw) else {
                continue;
            };
            let Some(payload) = line.payload else {
                continue;
            };
            match line.kind.as_deref() {
                Some("session_meta") if id.is_none() => {
                    id = payload
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    cwd = str_field(&payload, "cwd");
                    cli_version = str_field(&payload, "cli_version");
                    branch = payload.get("git").and_then(|g| str_field(g, "branch"));
                    created = payload
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .or(line.timestamp.as_deref())
                        .and_then(parse_rfc3339_ms);
                }
                Some("turn_context") => {
                    if let Some(m) = str_field(&payload, "model") {
                        model = Some(m);
                    }
                    if let Some(e) = str_field(&payload, "effort") {
                        effort = Some(e);
                    }
                    let sandbox = payload.get("sandbox_policy").and_then(|s| {
                        s.as_str()
                            .map(str::to_string)
                            .or_else(|| str_field(s, "type"))
                    });
                    if let (Some(a), Some(s)) = (str_field(&payload, "approval_policy"), sandbox) {
                        mode = Some(format!("{a}/{s}"));
                    }
                }
                Some("event_msg")
                    if title.is_none()
                        && payload.get("type").and_then(Value::as_str) == Some("user_message") =>
                {
                    title = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .and_then(clean_title_lenient);
                }
                _ => {}
            }
        }
        let id = id.or_else(|| id_from_file_name(&file.path))?;
        Some(Parsed::Session(SessionSummary {
            provider: Provider::Codex,
            id,
            cwd,
            title,
            created,
            updated: file.mtime_ms,
            git_branch: branch,
            cli_version,
            model,
            mode,
            effort,
            path: file.path.clone(),
            sidechains: Vec::new(),
        }))
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `rollout-2026-10-21T14-12-26-01a15e60-8596-7fe2-a84e-3b00fdf8a146` -> the trailing UUID.
fn id_from_file_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let tail = stem.get(stem.len().checked_sub(36)?..)?;
    (tail.matches('-').count() == 4).then(|| tail.to_string())
}

fn walk(dir: &Path, depth: u8, out: &mut Vec<SessionFile>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(&entry.path(), depth + 1, out);
        } else if file_type.is_file() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                let Ok(meta) = entry.metadata() else { continue };
                out.push(SessionFile {
                    path: entry.path(),
                    mtime_ms: mtime_ms(&meta),
                    size: meta.len(),
                    role: FileRole::Session,
                    cwd_hint: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_uuid_from_rollout_name() {
        let p = Path::new("rollout-2026-10-21T14-12-26-01a15e60-8596-7fe2-a84e-3b00fdf8a146.jsonl");
        assert_eq!(
            id_from_file_name(p).as_deref(),
            Some("01a15e60-8596-7fe2-a84e-3b00fdf8a146")
        );
        assert_eq!(id_from_file_name(Path::new("short.jsonl")), None);
    }
}
