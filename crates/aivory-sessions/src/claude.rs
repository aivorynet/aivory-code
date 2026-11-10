//! Claude Code store: `<root>/projects/<cwd-slug>/<sessionId>.jsonl`.
//!
//! Sub-agent files live in `<cwd-slug>/<sessionId>/subagents/agent-*.jsonl`. Older versions
//! wrote `agent-*.jsonl` next to the main files; the `sessionId` of their first line names the parent.

use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use crate::jsonl::{HeadTail, read_head_tail};
use crate::model::{Provider, SessionSummary};
use crate::store::{FileRole, Parsed, SessionFile, SessionStore, mtime_ms};
use crate::text::{clean_title, clean_title_lenient, content_text};
use crate::timeutil::parse_rfc3339_ms;

const HEAD_BYTES: usize = 64 * 1024;
const TAIL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ClaudeStore {
    root: PathBuf,
}

impl ClaudeStore {
    /// `root` is the Claude home directory (normally `~/.claude`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }
}

#[derive(Deserialize, Default)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    version: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    #[serde(rename = "lastPrompt")]
    last_prompt: Option<Value>,
    #[serde(rename = "permissionMode")]
    permission_mode: Option<String>,
    effort: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize, Default)]
struct Message {
    model: Option<String>,
    content: Option<Value>,
}

#[derive(Default)]
struct Collector {
    seen_message: bool,
    cwd: Option<String>,
    created: Option<i64>,
    first_user: Option<String>,
    first_user_lenient: Option<String>,
    custom_title: Option<String>,
    ai_title: Option<String>,
    last_prompt: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    effort: Option<String>,
    git_branch: Option<String>,
    version: Option<String>,
}

impl Collector {
    fn feed(&mut self, raw: &[u8]) {
        let Ok(line) = serde_json::from_slice::<Line>(raw) else {
            return;
        };
        if self.cwd.is_none() {
            self.cwd = line.cwd.clone().filter(|c| !c.is_empty());
        }
        if self.created.is_none() {
            self.created = line.timestamp.as_deref().and_then(parse_rfc3339_ms);
        }
        if line.git_branch.is_some() {
            self.git_branch = line.git_branch.clone();
        }
        if line.version.is_some() {
            self.version = line.version.clone();
        }
        if line.permission_mode.is_some() {
            self.mode = line.permission_mode.clone();
        }
        if line.effort.is_some() {
            self.effort = line.effort.clone();
        }
        match line.kind.as_deref() {
            Some("user") => {
                self.seen_message = true;
                let meta = line.is_meta.unwrap_or(false) || line.is_sidechain.unwrap_or(false);
                if !meta
                    && self.first_user.is_none()
                    && let Some(content) = line.message.as_ref().and_then(|m| m.content.as_ref())
                {
                    let text = content_text(content);
                    self.first_user = clean_title(&text);
                    if self.first_user_lenient.is_none() {
                        self.first_user_lenient = clean_title_lenient(&text);
                    }
                }
            }
            Some("assistant") => {
                self.seen_message = true;
                if let Some(model) = line.message.and_then(|m| m.model)
                    && model != "<synthetic>"
                {
                    self.model = Some(model);
                }
            }
            Some("ai-title") => self.ai_title = line.ai_title.or(self.ai_title.take()),
            Some("custom-title") => {
                self.custom_title = line.custom_title.or(self.custom_title.take())
            }
            Some("last-prompt") => {
                if let Some(Value::String(s)) = line.last_prompt {
                    self.last_prompt = clean_title(&s).or(self.last_prompt.take());
                }
            }
            _ => {}
        }
    }
}

impl SessionStore for ClaudeStore {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn enumerate(&self) -> Vec<SessionFile> {
        let Ok(dir) = fs::read_dir(self.projects_dir()) else {
            return Vec::new();
        };
        let project_dirs: Vec<PathBuf> = dir
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        project_dirs
            .par_iter()
            .flat_map_iter(|p| scan_project_dir(p))
            .collect()
    }

    fn parse(&self, file: &SessionFile) -> Option<Parsed> {
        if let FileRole::Sidechain { parent_id } = &file.role {
            let parent = match parent_id {
                Some(p) => p.clone(),
                None => {
                    let ht = read_head_tail(&file.path, file.size, 8 * 1024, 0).ok()?;
                    let first = ht.head_lines().next()?;
                    serde_json::from_slice::<Line>(first).ok()?.session_id?
                }
            };
            return Some(Parsed::Sidechain { parent_id: parent });
        }
        let ht: HeadTail = read_head_tail(&file.path, file.size, HEAD_BYTES, TAIL_BYTES).ok()?;
        let mut c = Collector::default();
        ht.head_lines()
            .chain(ht.tail_lines())
            .for_each(|l| c.feed(l));
        if !c.seen_message {
            return None;
        }
        let id = file.path.file_stem()?.to_str()?.to_string();
        Some(Parsed::Session(SessionSummary {
            provider: Provider::Claude,
            id,
            cwd: c.cwd,
            title: c
                .custom_title
                .or(c.ai_title)
                .or(c.last_prompt)
                .or(c.first_user)
                .or(c.first_user_lenient),
            created: c.created,
            updated: file.mtime_ms,
            git_branch: c.git_branch,
            cli_version: c.version,
            model: c.model,
            mode: c.mode,
            effort: c.effort,
            path: file.path.clone(),
            sidechains: Vec::new(),
        }))
    }
}

fn scan_project_dir(dir: &Path) -> Vec<SessionFile> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if file_type.is_file() && name.ends_with(".jsonl") {
            let Ok(meta) = entry.metadata() else { continue };
            let role = if name.starts_with("agent-") {
                FileRole::Sidechain { parent_id: None }
            } else {
                FileRole::Session
            };
            out.push(SessionFile {
                path: entry.path(),
                mtime_ms: mtime_ms(&meta),
                size: meta.len(),
                role,
                cwd_hint: None,
            });
        } else if file_type.is_dir() {
            let Ok(subs) = fs::read_dir(entry.path().join("subagents")) else {
                continue;
            };
            for sub in subs.filter_map(Result::ok) {
                let sub_name = sub.file_name();
                let Some(sub_name) = sub_name.to_str() else {
                    continue;
                };
                if sub_name.ends_with(".jsonl") {
                    out.push(SessionFile {
                        path: sub.path(),
                        mtime_ms: 0,
                        size: 0,
                        role: FileRole::Sidechain {
                            parent_id: Some(name.to_string()),
                        },
                        cwd_hint: None,
                    });
                }
            }
        }
    }
    out
}
