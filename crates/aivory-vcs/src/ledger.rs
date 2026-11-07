//! The turn ledger: the "prompt to change" log in SQLite.
//!
//! The caller owns the [`Connection`]. The ledger creates its own tables (`turn`, `turn_file`,
//! `turn_tool`, `ledger_op`) and a `ledger_meta` table with the schema version. All times are
//! Unix seconds.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{Result, VcsError};
use crate::types::FileChange;

/// The highest ledger schema version that this crate writes and reads.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS turn (
  id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, n INTEGER NOT NULL, task_id TEXT,
  provider TEXT NOT NULL, agent TEXT, model TEXT,
  prompt TEXT NOT NULL, prompt_source TEXT NOT NULL,
  started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
  snapshot_ref TEXT, parent_snapshot TEXT,
  usage_in INTEGER, usage_out INTEGER, cost_usd REAL,
  tests_passed INTEGER, tests_failed INTEGER,
  diagnostics_added INTEGER, diagnostics_removed INTEGER,
  UNIQUE (session_id, n)
);
CREATE TABLE IF NOT EXISTS turn_file (
  turn_id INTEGER NOT NULL, path TEXT NOT NULL, added INTEGER NOT NULL, removed INTEGER NOT NULL,
  blob_after TEXT
);
CREATE INDEX IF NOT EXISTS turn_file_by_turn ON turn_file (turn_id);
CREATE INDEX IF NOT EXISTS turn_file_by_path ON turn_file (path);
CREATE TABLE IF NOT EXISTS turn_tool (
  turn_id INTEGER NOT NULL, seq INTEGER NOT NULL, tool TEXT NOT NULL, input_digest TEXT,
  files TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS turn_tool_by_turn ON turn_tool (turn_id);
CREATE TABLE IF NOT EXISTS ledger_op (
  id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, kind TEXT NOT NULL, turn_n INTEGER,
  at INTEGER NOT NULL, detail TEXT
);
CREATE INDEX IF NOT EXISTS ledger_op_by_session ON ledger_op (session_id, id);
";

/// One file that a turn changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFile {
    /// Path relative to the repository root, with `/` separators.
    pub path: String,
    /// Added lines.
    pub added: i64,
    /// Removed lines.
    pub removed: i64,
    /// Blob id of the file after the turn. `None` when the turn deleted the file.
    pub blob_after: Option<String>,
}

impl From<&FileChange> for TurnFile {
    fn from(c: &FileChange) -> Self {
        Self {
            path: c.path.clone(),
            added: i64::try_from(c.added).unwrap_or(i64::MAX),
            removed: i64::try_from(c.removed).unwrap_or(i64::MAX),
            blob_after: c.blob_after.clone(),
        }
    }
}

/// One tool call of a turn. Only agents with hooks or tool events fill these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnTool {
    /// Order inside the turn.
    pub seq: i64,
    /// Tool name.
    pub tool: String,
    /// Digest of the tool input (never the input itself).
    pub input_digest: Option<String>,
    /// Files that the tool call changed.
    pub files: Vec<String>,
}

/// One ledger entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Turn {
    /// Row id. The ledger sets it. [`Ledger::record_turn`] ignores the input value.
    pub id: i64,
    /// Session id, for example `S-88`.
    pub session_id: String,
    /// Turn number inside the session. Turn 0 is the baseline snapshot.
    pub n: u32,
    /// Task id, when the session works on a task.
    pub task_id: Option<String>,
    /// Provider, for example `claude-code`.
    pub provider: String,
    /// Agent name.
    pub agent: Option<String>,
    /// Model id.
    pub model: Option<String>,
    /// Prompt text.
    pub prompt: String,
    /// Prompt origin: `human`, `agent`, `playbook`, or `ping`.
    pub prompt_source: String,
    /// Start time (Unix seconds).
    pub started_at: i64,
    /// End time (Unix seconds).
    pub ended_at: i64,
    /// Snapshot commit of the turn end. `None` when the turn changed nothing.
    pub snapshot_ref: Option<String>,
    /// Snapshot commit before the turn.
    pub parent_snapshot: Option<String>,
    /// Input tokens.
    pub usage_in: Option<i64>,
    /// Output tokens.
    pub usage_out: Option<i64>,
    /// Cost in USD.
    pub cost_usd: Option<f64>,
    /// Passed tests.
    pub tests_passed: Option<i64>,
    /// Failed tests.
    pub tests_failed: Option<i64>,
    /// Diagnostics that the turn added.
    pub diagnostics_added: Option<i64>,
    /// Diagnostics that the turn removed.
    pub diagnostics_removed: Option<i64>,
    /// Files that the turn changed (from the snapshot diff).
    pub files: Vec<TurnFile>,
    /// Tool calls of the turn.
    pub tools: Vec<TurnTool>,
}

/// An operation on the ledger that is not a turn, for example a revert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerOp {
    /// Row id.
    pub id: i64,
    /// Session id.
    pub session_id: String,
    /// Operation kind, for example `revert`.
    pub kind: String,
    /// Turn number that the operation affects.
    pub turn_n: Option<u32>,
    /// Time (Unix seconds).
    pub at: i64,
    /// Free text.
    pub detail: Option<String>,
}

/// Handle on the ledger tables of a caller-owned connection.
#[derive(Debug, Clone, Copy)]
pub struct Ledger<'c> {
    conn: &'c Connection,
}

const TURN_COLUMNS: &str = "id, session_id, n, task_id, provider, agent, model, prompt, \
    prompt_source, started_at, ended_at, snapshot_ref, parent_snapshot, usage_in, usage_out, \
    cost_usd, tests_passed, tests_failed, diagnostics_added, diagnostics_removed";

fn turn_from_row(row: &Row<'_>) -> rusqlite::Result<Turn> {
    let n: i64 = row.get(2)?;
    Ok(Turn {
        id: row.get(0)?,
        session_id: row.get(1)?,
        n: u32::try_from(n).unwrap_or(u32::MAX),
        task_id: row.get(3)?,
        provider: row.get(4)?,
        agent: row.get(5)?,
        model: row.get(6)?,
        prompt: row.get(7)?,
        prompt_source: row.get(8)?,
        started_at: row.get(9)?,
        ended_at: row.get(10)?,
        snapshot_ref: row.get(11)?,
        parent_snapshot: row.get(12)?,
        usage_in: row.get(13)?,
        usage_out: row.get(14)?,
        cost_usd: row.get(15)?,
        tests_passed: row.get(16)?,
        tests_failed: row.get(17)?,
        diagnostics_added: row.get(18)?,
        diagnostics_removed: row.get(19)?,
        files: Vec::new(),
        tools: Vec::new(),
    })
}

impl<'c> Ledger<'c> {
    /// Create the tables when they are missing and check the schema version.
    pub fn new(conn: &'c Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ledger_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        )?;
        let found: Option<String> = conn
            .query_row(
                "SELECT value FROM ledger_meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match found {
            Some(v) => {
                let v: i64 = v
                    .parse()
                    .map_err(|_| VcsError::Parse(format!("ledger schema version `{v}`")))?;
                if v > SCHEMA_VERSION {
                    return Err(VcsError::SchemaTooNew {
                        found: v,
                        supported: SCHEMA_VERSION,
                    });
                }
                // Older versions: no migration exists yet.
            }
            None => {
                let tx = conn.unchecked_transaction()?;
                tx.execute_batch(SCHEMA)?;
                tx.execute(
                    // Two connections can both find no version; the schema is idempotent.
                    "INSERT OR IGNORE INTO ledger_meta (key, value) VALUES ('schema_version', ?1)",
                    [SCHEMA_VERSION.to_string()],
                )?;
                tx.commit()?;
            }
        }
        Ok(Self { conn })
    }

    /// Read the schema version of the database.
    pub fn schema_version(&self) -> Result<i64> {
        let v: String = self.conn.query_row(
            "SELECT value FROM ledger_meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        v.parse()
            .map_err(|_| VcsError::Parse(format!("ledger schema version `{v}`")))
    }

    /// Store a turn with its files and tool calls. Returns the stored turn with its row id.
    /// A second turn with the same session and number is an error.
    pub fn record_turn(&self, turn: &Turn) -> Result<Turn> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO turn (session_id, n, task_id, provider, agent, model, prompt, \
             prompt_source, started_at, ended_at, snapshot_ref, parent_snapshot, usage_in, \
             usage_out, cost_usd, tests_passed, tests_failed, diagnostics_added, \
             diagnostics_removed) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
             ?18, ?19)",
            params![
                turn.session_id,
                turn.n,
                turn.task_id,
                turn.provider,
                turn.agent,
                turn.model,
                turn.prompt,
                turn.prompt_source,
                turn.started_at,
                turn.ended_at,
                turn.snapshot_ref,
                turn.parent_snapshot,
                turn.usage_in,
                turn.usage_out,
                turn.cost_usd,
                turn.tests_passed,
                turn.tests_failed,
                turn.diagnostics_added,
                turn.diagnostics_removed,
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut ins = tx.prepare(
                "INSERT INTO turn_file (turn_id, path, added, removed, blob_after) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for f in &turn.files {
                ins.execute(params![id, f.path, f.added, f.removed, f.blob_after])?;
            }
            let mut ins = tx.prepare(
                "INSERT INTO turn_tool (turn_id, seq, tool, input_digest, files) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for t in &turn.tools {
                ins.execute(params![
                    id,
                    t.seq,
                    t.tool,
                    t.input_digest,
                    t.files.join("\n")
                ])?;
            }
        }
        tx.commit()?;
        let mut stored = turn.clone();
        stored.id = id;
        Ok(stored)
    }

    /// Get one turn of a session.
    pub fn get_turn(&self, session_id: &str, n: u32) -> Result<Option<Turn>> {
        let sql = format!("SELECT {TURN_COLUMNS} FROM turn WHERE session_id = ?1 AND n = ?2");
        let turn = self
            .conn
            .query_row(&sql, params![session_id, n], turn_from_row)
            .optional()?;
        match turn {
            Some(mut t) => {
                self.load_children(&mut t)?;
                Ok(Some(t))
            }
            None => Ok(None),
        }
    }

    /// List the turns of a session, ordered by turn number.
    pub fn list_turns(&self, session_id: &str) -> Result<Vec<Turn>> {
        let sql = format!("SELECT {TURN_COLUMNS} FROM turn WHERE session_id = ?1 ORDER BY n");
        self.query_turns(&sql, params![session_id])
    }

    /// List the turns (of one session, or of all sessions) that changed a file, oldest first.
    pub fn turns_for_file(&self, path: &str, session_id: Option<&str>) -> Result<Vec<Turn>> {
        let sql = format!(
            "SELECT {TURN_COLUMNS} FROM turn WHERE id IN \
             (SELECT turn_id FROM turn_file WHERE path = ?1) \
             AND (?2 IS NULL OR session_id = ?2) ORDER BY ended_at, id"
        );
        self.query_turns(&sql, params![path, session_id])
    }

    fn query_turns(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Turn>> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut turns = stmt
            .query_map(p, turn_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for t in &mut turns {
            self.load_children(t)?;
        }
        Ok(turns)
    }

    fn load_children(&self, t: &mut Turn) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT path, added, removed, blob_after FROM turn_file WHERE turn_id = ?1 \
             ORDER BY rowid",
        )?;
        t.files = stmt
            .query_map([t.id], |r| {
                Ok(TurnFile {
                    path: r.get(0)?,
                    added: r.get(1)?,
                    removed: r.get(2)?,
                    blob_after: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT seq, tool, input_digest, files FROM turn_tool WHERE turn_id = ?1 \
             ORDER BY seq, rowid",
        )?;
        t.tools = stmt
            .query_map([t.id], |r| {
                let files: String = r.get(3)?;
                Ok(TurnTool {
                    seq: r.get(0)?,
                    tool: r.get(1)?,
                    input_digest: r.get(2)?,
                    files: files
                        .split('\n')
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(())
    }

    /// Record an operation (for example a revert) in the ledger.
    pub fn record_op(
        &self,
        session_id: &str,
        kind: &str,
        turn_n: Option<u32>,
        at: i64,
        detail: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO ledger_op (session_id, kind, turn_n, at, detail) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, kind, turn_n, at, detail],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List the operations of a session in the order of recording.
    pub fn list_ops(&self, session_id: &str) -> Result<Vec<LedgerOp>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, kind, turn_n, at, detail FROM ledger_op \
             WHERE session_id = ?1 ORDER BY id",
        )?;
        let ops = stmt
            .query_map([session_id], |r| {
                let n: Option<i64> = r.get(3)?;
                Ok(LedgerOp {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    kind: r.get(2)?,
                    turn_n: n.and_then(|n| u32::try_from(n).ok()),
                    at: r.get(4)?,
                    detail: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ops)
    }
}
