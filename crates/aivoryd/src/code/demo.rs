//! `aivoryd serve --demo`: the code service on the synthetic files of
//! [`aivory_protocol::demo`]. The files go to a scratch folder, and the real engines (syntax
//! tree, index) answer `code/*` on them. The ledger and the findings come from the demo data,
//! because the scratch folder has no git history.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use aivory_protocol::code::{
    DiagnosticsParams, DiagnosticsResult, FileBlame, LedgerTurn, RevertParams, RevertResult,
    Timeline, TimelineEvent, TurnFileStat, TurnList,
};
use aivory_protocol::demo::{self, Demo};
use aivory_protocol::rpc::{RpcError, code};

use super::resolve::{Resolver, SessionCtx, StaticResolver};
use super::state::{CodeCore, core_for};

static NEXT: AtomicU32 = AtomicU32::new(1);

#[derive(Debug)]
pub struct DemoCode {
    dir: PathBuf,
    core: Arc<CodeCore>,
    resolver: Arc<StaticResolver>,
    /// The turns that `ledger/revert` added to the demo ledger of the session `a01c`.
    reverts: Mutex<Vec<LedgerTurn>>,
}

impl DemoCode {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "aivoryd-demo-code-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let mut roots: Vec<(String, PathBuf)> = Vec::new();
        for f in demo::code_files() {
            let root = dir.join(f.codebase);
            let path = root.join(f.path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&path, f.text) {
                eprintln!("aivoryd: demo file {}: {e}", path.display());
            }
            if !roots.iter().any(|(c, _)| c == f.codebase) {
                roots.push((f.codebase.to_owned(), root));
            }
        }
        let sessions = Demo::new()
            .war_room
            .sessions
            .iter()
            .map(|p| {
                let workdir = roots
                    .iter()
                    .find(|(c, _)| *c == p.codebase)
                    .map(|(_, r)| r.clone());
                (
                    p.id.clone(),
                    SessionCtx {
                        native: p.id.clone(),
                        codebase: Some(p.codebase.clone()),
                        workdir,
                    },
                )
            })
            .collect();
        let resolver = Arc::new(StaticResolver {
            codebases: roots,
            sessions,
        });
        Self {
            core: core_for(&dir.join("aivory.db")),
            dir,
            resolver,
            reverts: Mutex::new(Vec::new()),
        }
    }

    pub fn core(&self) -> Arc<CodeCore> {
        Arc::clone(&self.core)
    }

    pub fn resolver(&self) -> Arc<dyn Resolver> {
        self.resolver.clone()
    }

    pub fn turns(&self, session: &str) -> Result<TurnList, RpcError> {
        if self.resolver.session(session).is_none() {
            return Err(RpcError::new(
                code::SESSION_NOT_FOUND,
                format!("no session '{session}'"),
            ));
        }
        if session == demo::LEDGER_SESSION {
            let mut list = demo::ledger_turns();
            list.turns.extend(self.reverts().iter().cloned());
            return Ok(list);
        }
        Ok(TurnList {
            session: session.to_owned(),
            turns: Vec::new(),
        })
    }

    fn reverts(&self) -> std::sync::MutexGuard<'_, Vec<LedgerTurn>> {
        self.reverts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Simulate `ledger/revert`: the files return to the state before the turn, and the ledger
    /// gets a new turn that holds the opposite change. Only the session `a01c` has turns.
    pub fn revert(&self, p: &RevertParams) -> Result<RevertResult, RpcError> {
        let list = self.turns(&p.session)?;
        if p.turn == 0 {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "turn 0 is the baseline; there is nothing to undo",
            ));
        }
        let Some(turn) = list.turns.iter().find(|t| t.n == p.turn).cloned() else {
            return Err(RpcError::new(
                code::INVALID_STATE,
                format!("the session has no turn {}", p.turn),
            ));
        };
        let mut reverts = self.reverts();
        let n = list.turns.last().map_or(1, |t| t.n + 1);
        let at = turn.ended_at.max(turn.started_at) + 60_000 * i64::from(n);
        reverts.push(LedgerTurn {
            n,
            prompt: format!("Revert turn {}", p.turn),
            provider: "aivoryd".to_owned(),
            model: None,
            started_at: at,
            ended_at: at,
            cost: None,
            tokens_in: None,
            tokens_out: None,
            snapshot: true,
            files: turn
                .files
                .iter()
                .map(|f| TurnFileStat {
                    path: f.path.clone(),
                    added: f.removed,
                    removed: f.added,
                })
                .collect(),
            diagnostics_added: None,
            diagnostics_removed: None,
        });
        Ok(RevertResult {
            session: p.session.clone(),
            turn: p.turn,
            restored: p.turn - 1,
            snapshot: format!("{:012x}", 0xd3a0_0000_0000_u64 + u64::from(n) * 0x1f3),
        })
    }

    pub fn blame(&self, path: &str) -> Result<FileBlame, RpcError> {
        let blame = demo::ledger_blame();
        if blame.path == path {
            return Ok(blame);
        }
        // A file that no turn touched: one hunk with no turn.
        let text = demo::code_files()
            .into_iter()
            .find(|f| f.path == path)
            .ok_or_else(|| RpcError::not_found("file", path))?;
        Ok(FileBlame {
            path: path.to_owned(),
            hunks: vec![aivory_protocol::code::BlameHunk {
                line: 1,
                count: u32::try_from(text.text.lines().count()).unwrap_or(0),
                commit: None,
                turn: None,
            }],
        })
    }

    pub fn timeline(&self, path: &str) -> Result<Timeline, RpcError> {
        let timeline = demo::ledger_timeline();
        if timeline.path == path {
            return Ok(timeline);
        }
        let known = demo::code_files().iter().any(|f| f.path == path);
        if !known {
            return Err(RpcError::not_found("file", path));
        }
        Ok(Timeline {
            path: path.to_owned(),
            events: Vec::<TimelineEvent>::new(),
        })
    }

    pub fn diagnostics(&self, p: &DiagnosticsParams, path: Option<&str>) -> DiagnosticsResult {
        let mut result = demo::ledger_diagnostics();
        let ours =
            p.session.as_deref() == Some(demo::LEDGER_SESSION) && p.turn.is_none_or(|n| n == 3);
        if !ours {
            return DiagnosticsResult {
                turn: p.turn,
                rows: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
                notes: vec!["the demo has findings only for session a01c, turn 3".into()],
            };
        }
        if let Some(path) = path {
            result.rows.retain(|f| f.path == path);
            result.added.retain(|f| f.path == path);
            result.removed.retain(|f| f.path == path);
        }
        result
    }
}

impl Drop for DemoCode {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
