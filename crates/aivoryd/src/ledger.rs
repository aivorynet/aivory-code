//! The turn ledger of the daemon (`aivory-vcs`), on the state database `aivory.db`.
//!
//! Git work takes time and the run event pumps hold the state lock. So the pumps and the
//! scanner only send a [`Job`] to one worker thread. The worker runs the jobs in order, opens a
//! connection for each job, and calls [`aivory_vcs::finish_turn`]. A failed job prints one line
//! and changes nothing else.
//!
//! The ledger keys a session by its native id (the session id of the CLI). Times in the ledger
//! are Unix seconds. The protocol uses milliseconds.
//!
//! After a turn the worker tells the code service ([`crate::code::turn_ended`]), which updates the
//! index delta of the session and runs the diagnostics pipeline on the files of the turn.
//!
//! The last agent text of a turn goes to the ledger as an operation of kind
//! [`AGENT_TEXT_OP`], because a turn row has no field for it. A fork at a turn reads it back for
//! the handoff packet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use aivory_vcs::{BASELINE_TURN, GitVcs, Ledger, Snapshot, Turn, Vcs, finish_turn};
use rusqlite::Connection;

/// The kind of the ledger operation that holds the last agent text of a turn.
pub const AGENT_TEXT_OP: &str = "agent_text";
/// Longest prompt that the ledger keeps, in characters.
const PROMPT_CHARS: usize = 20_000;
/// Longest agent text that the ledger keeps, in characters.
const AGENT_TEXT_CHARS: usize = 4_000;
const BUSY: Duration = Duration::from_secs(5);

/// A turn that ended.
#[derive(Debug, Clone, Default)]
pub struct TurnJob {
    /// Native session id.
    pub session: String,
    /// The repository folder (the snapshot covers the whole repository).
    pub workdir: PathBuf,
    pub provider: String,
    pub model: String,
    /// Turn number. `None` takes the number after the last turn of the session.
    pub n: Option<u32>,
    pub prompt: String,
    /// `human`, `playbook`, or `native` (a turn that the scanner found in a transcript).
    pub prompt_source: &'static str,
    /// Milliseconds.
    pub started_ms: i64,
    /// Milliseconds.
    pub ended_ms: i64,
    pub cost_usd: Option<f64>,
    pub usage_in: Option<i64>,
    pub usage_out: Option<i64>,
    /// The last text of the agent in the turn.
    pub agent_text: String,
    pub task: Option<String>,
}

/// A baseline snapshot (turn 0) at the session start.
#[derive(Debug, Clone)]
pub struct BaselineJob {
    pub session: String,
    pub workdir: PathBuf,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub enum Job {
    /// Skipped when the session has a turn already.
    Baseline(BaselineJob),
    Turn(TurnJob),
}

/// The worker thread that writes the ledger.
#[derive(Debug, Clone)]
pub struct LedgerWorker {
    db: PathBuf,
    tx: Sender<Job>,
}

impl LedgerWorker {
    /// Start the worker for the database file `db`. The thread ends when every handle is gone.
    pub fn start(db: PathBuf) -> Self {
        let (tx, rx) = channel::<Job>();
        let path = db.clone();
        let spawned = thread::Builder::new()
            .name("aivoryd-ledger".into())
            .spawn(move || {
                let vcs = GitVcs::new();
                while let Ok(job) = rx.recv() {
                    if let Err(e) = run_job(&path, &vcs, job) {
                        eprintln!("aivoryd: ledger: {e}");
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("aivoryd: cannot start the ledger thread: {e}");
        }
        Self { db, tx }
    }

    /// Queue a job. A job that cannot go out (the thread is gone) is dropped.
    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    pub fn db(&self) -> &Path {
        &self.db
    }
}

/// True when `dir` is a git checkout (a folder with `.git`, a file for a worktree).
pub fn is_git_checkout(dir: &Path) -> bool {
    dir.join(".git").exists()
}

fn open(db: &Path) -> Result<Connection, String> {
    let conn = Connection::open(db).map_err(|e| e.to_string())?;
    conn.busy_timeout(BUSY).map_err(|e| e.to_string())?;
    Ok(conn)
}

/// `Ledger::new` creates the tables on the first call. Two threads that open the same new
/// database at once would both insert the schema version, so this lock orders them.
static SCHEMA_LOCK: Mutex<()> = Mutex::new(());

/// Open the ledger on `conn`. Use this call, not `Ledger::new`, in the daemon.
pub fn open_ledger(conn: &Connection) -> Result<Ledger<'_>, aivory_vcs::VcsError> {
    let _guard = SCHEMA_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    Ledger::new(conn)
}

/// Run `f` on the ledger of the database file `db`.
pub fn with_ledger<T>(
    db: &Path,
    f: impl FnOnce(&Ledger<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let conn = open(db)?;
    let ledger = open_ledger(&conn).map_err(|e| e.to_string())?;
    f(&ledger)
}

fn run_job(db: &Path, vcs: &GitVcs, job: Job) -> Result<(), String> {
    match job {
        Job::Baseline(b) => with_ledger(db, |ledger| {
            if !ledger
                .list_turns(&b.session)
                .map_err(|e| e.to_string())?
                .is_empty()
            {
                return Ok(());
            }
            let turn = Turn {
                session_id: b.session,
                n: BASELINE_TURN,
                provider: b.provider,
                model: Some(b.model).filter(|m| !m.is_empty()),
                prompt_source: "human".into(),
                started_at: now_secs(),
                ended_at: now_secs(),
                ..Turn::default()
            };
            finish_turn(vcs, ledger, &b.workdir, turn).map_err(|e| e.to_string())?;
            Ok(())
        }),
        Job::Turn(t) => with_ledger(db, |ledger| {
            let last = ledger
                .list_turns(&t.session)
                .map_err(|e| e.to_string())?
                .last()
                .map(|x| x.n);
            let n = t.n.unwrap_or_else(|| last.map_or(1, |l| l + 1));
            if last.is_some_and(|l| l >= n) {
                return Ok(()); // the ledger has this turn already
            }
            if last.is_none() && n > BASELINE_TURN {
                // The session was found after it started: the state now is the baseline. The
                // first turn then shows no change, not the whole repository.
                let baseline = Turn {
                    session_id: t.session.clone(),
                    n: BASELINE_TURN,
                    provider: t.provider.clone(),
                    model: Some(t.model.clone()).filter(|m| !m.is_empty()),
                    prompt_source: "native".into(),
                    started_at: now_secs(),
                    ended_at: now_secs(),
                    ..Turn::default()
                };
                finish_turn(vcs, ledger, &t.workdir, baseline).map_err(|e| e.to_string())?;
            }
            let turn = Turn {
                session_id: t.session.clone(),
                n,
                task_id: t.task,
                provider: t.provider,
                model: Some(t.model).filter(|m| !m.is_empty()),
                prompt: cut(&t.prompt, PROMPT_CHARS),
                prompt_source: t.prompt_source.into(),
                started_at: t.started_ms / 1000,
                ended_at: (t.ended_ms / 1000).max(t.started_ms / 1000),
                usage_in: t.usage_in,
                usage_out: t.usage_out,
                cost_usd: t.cost_usd,
                ..Turn::default()
            };
            let recorded = finish_turn(vcs, ledger, &t.workdir, turn).map_err(|e| e.to_string())?;
            // The code service updates the session delta of the index and runs the diagnostics
            // on the files of the turn. It works in the background.
            crate::code::turn_ended(
                db,
                crate::code::TurnEnd {
                    session: t.session.clone(),
                    n,
                    workdir: t.workdir.clone(),
                    files: recorded.files.iter().map(|f| f.path.clone()).collect(),
                    parent_snapshot: recorded.parent_snapshot.clone(),
                    snapshot: recorded.snapshot_ref.clone(),
                    ended_ms: t.ended_ms,
                },
            );
            if !t.agent_text.trim().is_empty() {
                let text = cut(&t.agent_text, AGENT_TEXT_CHARS);
                ledger
                    .record_op(&t.session, AGENT_TEXT_OP, Some(n), now_secs(), Some(&text))
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        }),
    }
}

/// What a fork at a turn needs from the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnText {
    pub n: u32,
    pub prompt: String,
    /// The last agent text of the turn. Empty when the ledger has none.
    pub agent: String,
}

/// The turns 1 to `upto` of a session with their texts, and the snapshot at the end of turn
/// `upto`: the snapshot of that turn, or the latest earlier one (a turn that changed nothing
/// has no snapshot of its own).
pub fn turns_upto(
    db: &Path,
    vcs: &dyn Vcs,
    repo: &Path,
    session: &str,
    upto: u32,
) -> Result<(Vec<TurnText>, Snapshot), String> {
    let (turns, mut texts) = with_ledger(db, |ledger| {
        let turns = ledger.list_turns(session).map_err(|e| e.to_string())?;
        let mut texts: BTreeMap<u32, String> = BTreeMap::new();
        for op in ledger.list_ops(session).map_err(|e| e.to_string())? {
            if let (AGENT_TEXT_OP, Some(n), Some(detail)) = (op.kind.as_str(), op.turn_n, op.detail)
            {
                texts.insert(n, detail);
            }
        }
        Ok((turns, texts))
    })?;
    if !turns.iter().any(|t| t.n == upto) {
        return Err(format!(
            "the ledger has no turn {upto} of session {session}"
        ));
    }
    let snapshot = vcs
        .list_snapshots(repo, session)
        .map_err(|e| e.to_string())?
        .into_iter()
        .rev()
        .find(|s| s.n <= upto)
        .ok_or_else(|| format!("no snapshot at or before turn {upto} of session {session}"))?;
    let texts = turns
        .into_iter()
        .filter(|t| t.n >= 1 && t.n <= upto)
        .map(|t| TurnText {
            n: t.n,
            prompt: t.prompt,
            agent: texts.remove(&t.n).unwrap_or_default(),
        })
        .collect();
    Ok((texts, snapshot))
}

/// The highest turn number of a session, 0 when the ledger has none.
pub fn last_turn(db: &Path, session: &str) -> u32 {
    with_ledger(db, |l| {
        Ok(l.list_turns(session)
            .map_err(|e| e.to_string())?
            .last()
            .map_or(0, |t| t.n))
    })
    .unwrap_or(0)
}

/// `text`, cut to `max` characters.
pub fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => text[..i].to_string(),
        None => text.to_string(),
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_keeps_whole_characters() {
        assert_eq!(cut("abcdef", 3), "abc");
        assert_eq!(cut("äöüß", 2), "äö");
        assert_eq!(cut("ab", 5), "ab");
    }
}
