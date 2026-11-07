//! Codebase ids and session ids to folders. The service asks a [`Resolver`]; the live daemon
//! answers from the war-room view and the session catalog.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use aivory_protocol::{RepoList, WarRoom};
use serde_json::Value;

use crate::config::Config;
use crate::live::LiveBackend;
use crate::outbox::Outbox;

/// What the service knows about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCtx {
    /// The id in the ledger (the native id of the CLI).
    pub native: String,
    /// The codebase of the session.
    pub codebase: Option<String>,
    /// The folder that the ledger snapshots cover: the repository root of the session folder.
    pub workdir: Option<PathBuf>,
}

pub trait Resolver: Send + Sync + std::fmt::Debug {
    /// The codebases: id and root folder.
    fn codebases(&self) -> Vec<(String, PathBuf)>;
    /// The session with this id (a pane id or a native id).
    fn session(&self, id: &str) -> Option<SessionCtx>;

    fn codebase_root(&self, id: &str) -> Option<PathBuf> {
        self.codebases()
            .into_iter()
            .find(|(c, _)| c == id)
            .map(|(_, p)| p)
    }
}

/// The repository root of a session folder: the first folder up with a `.git` entry.
pub fn repo_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// A resolver with fixed answers (tests).
#[derive(Debug, Default, Clone)]
pub struct StaticResolver {
    pub codebases: Vec<(String, PathBuf)>,
    pub sessions: Vec<(String, SessionCtx)>,
}

impl Resolver for StaticResolver {
    fn codebases(&self) -> Vec<(String, PathBuf)> {
        self.codebases.clone()
    }

    fn session(&self, id: &str) -> Option<SessionCtx> {
        self.sessions
            .iter()
            .find(|(s, _)| s == id)
            .map(|(_, c)| c.clone())
    }
}

const CACHE_FOR: Duration = Duration::from_secs(3);

/// The resolver of a live daemon.
pub struct LiveResolver {
    live: Arc<LiveBackend>,
    config: Config,
    cache: Mutex<Option<(Instant, WarRoom)>>,
}

impl std::fmt::Debug for LiveResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveResolver").finish_non_exhaustive()
    }
}

impl LiveResolver {
    pub fn new(live: Arc<LiveBackend>, config: Config) -> Self {
        Self {
            live,
            config,
            cache: Mutex::new(None),
        }
    }

    fn view(&self) -> Option<WarRoom> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, view)) = cache.as_ref()
            && at.elapsed() < CACHE_FOR
        {
            return Some(view.clone());
        }
        let (out, _rx) = Outbox::new(1);
        let value = self
            .live
            .dispatch("warroom/snapshot", Value::Null, &Value::Null, &out)
            .ok()??;
        let view: WarRoom = serde_json::from_value(value).ok()?;
        *cache = Some((Instant::now(), view.clone()));
        Some(view)
    }
}

impl Resolver for LiveResolver {
    fn codebases(&self) -> Vec<(String, PathBuf)> {
        let (out, _rx) = Outbox::new(1);
        let Ok(Some(value)) = self
            .live
            .dispatch("repos/list", Value::Null, &Value::Null, &out)
        else {
            return Vec::new();
        };
        serde_json::from_value::<RepoList>(value)
            .map(|l| {
                l.codebases
                    .into_iter()
                    .map(|c| (c.id, PathBuf::from(c.path)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn session(&self, id: &str) -> Option<SessionCtx> {
        let summary = self.config.catalog().find(id, None).ok();
        let pane = self
            .view()
            .and_then(|v| v.sessions.into_iter().find(|p| p.id == id));
        if summary.is_none() && pane.is_none() {
            return None;
        }
        let cwd = summary
            .as_ref()
            .and_then(|s| s.cwd.as_deref())
            .map(PathBuf::from);
        let workdir = cwd.as_deref().map(repo_root);
        let codebase = pane.map(|p| p.codebase).or_else(|| {
            let dir = workdir.as_ref()?;
            self.codebases()
                .into_iter()
                .filter(|(_, root)| dir.starts_with(root))
                .max_by_key(|(_, root)| root.components().count())
                .map(|(c, _)| c)
        });
        Some(SessionCtx {
            native: summary.map_or_else(|| id.to_owned(), |s| s.id),
            codebase,
            workdir,
        })
    }
}
