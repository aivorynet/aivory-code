//! The local store: SQLite in WAL mode, one file (`<data>/aivory/state.db`, ADR 0018).
//!
//! # Threads
//!
//! One [`Store`] is one connection. The daemon owns one writer: every write method takes
//! `&mut self`, so the compiler allows one writer at a time. A reader opens its own handle with
//! [`Store::open_read_only`]. In WAL mode the readers do not block the writer.
//!
//! # Schema
//!
//! `schema_version` holds one row, the number of the last migration that ran. A migration is one
//! entry of `MIGRATIONS`; entry `n` has version `n + 1`. Never edit a migration that shipped.
//! Add a new entry at the end instead. A file with a newer version than this build supports is
//! refused ([`StoreError::TooNew`]).
//!
//! | Table | Key | Content |
//! |---|---|---|
//! | `tasks` | `id` | the [`Task`] as JSON, plus the columns the queue query needs |
//! | `inbox_results` | `id` | the answered [`InboxItem`] as JSON, its result and the time |
//! | `settings` | `key` | one JSON value per top-level key of [`Settings`] |
//! | `activity_samples` | `at` | one [`ActivitySample`] per minute |
//! | `devices` | `id` | a paired [`Device`] |

use std::path::Path;
use std::time::Duration;

use aivory_protocol::{
    ActivitySample, Autopilot, Device, InboxItem, Settings, Task, TaskColumn, UnixMs,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[cfg(test)]
mod tests;

/// Errors of the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The file was written by a newer build.
    #[error("database version {found} is newer than the supported version {supported}")]
    TooNew { found: u32, supported: u32 },
    /// A read-only handle cannot migrate the file.
    #[error("database version {found} is older than {supported}; the daemon must migrate it")]
    TooOld { found: u32, supported: u32 },
    #[error("invalid input: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// The migrations in order. Entry `n` brings the file to version `n + 1`.
const MIGRATIONS: &[&str] = &[
    // 1: tasks, answered inbox items, settings, pulse-chart samples.
    "CREATE TABLE tasks (
        seq          INTEGER PRIMARY KEY AUTOINCREMENT,
        id           TEXT NOT NULL UNIQUE,
        board_column TEXT NOT NULL,
        codebase     TEXT NOT NULL,
        started      INTEGER,
        body         TEXT NOT NULL
    );
    CREATE INDEX tasks_by_column ON tasks (board_column);
    CREATE TABLE inbox_results (
        id          TEXT PRIMARY KEY,
        answered_at INTEGER NOT NULL,
        result      TEXT NOT NULL,
        body        TEXT NOT NULL
    );
    CREATE INDEX inbox_results_by_time ON inbox_results (answered_at);
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE activity_samples (
        at      INTEGER PRIMARY KEY,
        active  INTEGER NOT NULL,
        waiting INTEGER NOT NULL,
        idle    INTEGER NOT NULL
    );",
    // 2: paired devices.
    "CREATE TABLE devices (
        id        TEXT PRIMARY KEY,
        name      TEXT NOT NULL,
        os        TEXT NOT NULL,
        paired    INTEGER NOT NULL,
        last_seen INTEGER NOT NULL,
        online    INTEGER NOT NULL
    );",
];

/// The schema version of this build: the number of migrations.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// An inbox item that the user answered, with the time of the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct AnsweredItem {
    /// The item as it was answered. `item.result` is always set.
    pub item: InboxItem,
    pub answered: UnixMs,
}

/// One connection to the state database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open or create the database file, switch it to WAL mode and run the missing migrations.
    /// Missing parent directories are created.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path)?;
        configure(&conn, true)?;
        migrate(&mut conn, SCHEMA_VERSION)?;
        Ok(Self { conn })
    }

    /// A private in-memory database (no WAL), migrated to the latest version.
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn, false)?;
        migrate(&mut conn, SCHEMA_VERSION)?;
        Ok(Self { conn })
    }

    /// A read-only handle for a UI-side reader. The file must exist and the daemon must have
    /// migrated it. Every write method on this handle fails with a SQLite error.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure(&conn, false)?;
        let found = read_version(&conn)?;
        if found < SCHEMA_VERSION {
            return Err(StoreError::TooOld {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found > SCHEMA_VERSION {
            return Err(StoreError::TooNew {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(Self { conn })
    }

    /// The version in `schema_version`.
    pub fn schema_version(&self) -> Result<u32> {
        read_version(&self.conn)
    }

    /// The journal mode of the connection: `wal` for a file, `memory` for [`Store::open_in_memory`].
    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))?)
    }

    // ---- tasks -------------------------------------------------------------------------

    /// Insert a task, or replace the task with the same id. A replaced task keeps its place in
    /// the creation order.
    pub fn put_task(&mut self, task: &Task) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tasks (id, board_column, codebase, started, body)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET
                board_column = excluded.board_column,
                codebase     = excluded.codebase,
                started      = excluded.started,
                body         = excluded.body",
            params![
                task.id,
                tag(&task.column)?,
                task.codebase,
                task.started,
                serde_json::to_string(task)?
            ],
        )?;
        Ok(())
    }

    pub fn task(&self, id: &str) -> Result<Option<Task>> {
        let body: Option<String> = self
            .conn
            .query_row("SELECT body FROM tasks WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into))
            .transpose()
    }

    /// All tasks, in creation order (oldest first).
    pub fn tasks(&self) -> Result<Vec<Task>> {
        self.query_json("SELECT body FROM tasks ORDER BY seq", [])
    }

    /// The tasks that wait for an autopilot slot: column `planned`, not started. In creation
    /// order, which is the age order that [`crate::autopilot::plan`] expects.
    pub fn queued_tasks(&self) -> Result<Vec<Task>> {
        self.query_json(
            "SELECT body FROM tasks
             WHERE board_column = ?1 AND started IS NULL ORDER BY seq",
            [tag(&TaskColumn::Planned)?],
        )
    }

    /// Returns `true` when a task was deleted.
    pub fn delete_task(&mut self, id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id])? > 0)
    }

    // ---- inbox results -----------------------------------------------------------------

    /// Save an answered inbox item. `item.result` must be set. The same id replaces the old row.
    pub fn put_inbox_result(&mut self, item: &InboxItem, answered: UnixMs) -> Result<()> {
        let Some(result) = item.result.as_deref() else {
            return Err(StoreError::Invalid(format!(
                "inbox item '{}' has no result",
                item.id
            )));
        };
        self.conn.execute(
            "INSERT INTO inbox_results (id, answered_at, result, body) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET
                answered_at = excluded.answered_at,
                result      = excluded.result,
                body        = excluded.body",
            params![item.id, answered, result, serde_json::to_string(item)?],
        )?;
        Ok(())
    }

    pub fn inbox_result(&self, id: &str) -> Result<Option<AnsweredItem>> {
        let row: Option<(String, UnixMs)> = self
            .conn
            .query_row(
                "SELECT body, answered_at FROM inbox_results WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(body, answered)| answered_item(&body, answered))
            .transpose()
    }

    /// The newest answered items first, at most `limit`.
    pub fn inbox_results(&self, limit: usize) -> Result<Vec<AnsweredItem>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT body, answered_at FROM inbox_results
             ORDER BY answered_at DESC, id LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, UnixMs>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (body, answered) = row?;
            out.push(answered_item(&body, answered)?);
        }
        Ok(out)
    }

    pub fn delete_inbox_result(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM inbox_results WHERE id = ?1", [id])?
            > 0)
    }

    /// Delete the items answered before `before`. Returns the number of deleted rows.
    pub fn prune_inbox_results(&mut self, before: UnixMs) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM inbox_results WHERE answered_at < ?1", [before])?)
    }

    // ---- settings ----------------------------------------------------------------------

    /// Set one raw key. [`Store::save_settings`] uses the top-level keys of [`Settings`]:
    /// `providers`, `autopilot`, `display`, `notify` and `quietHours`.
    pub fn set_setting(&mut self, key: &str, value: &Value) -> Result<()> {
        put_setting(&self.conn, key, value)
    }

    pub fn setting(&self, key: &str) -> Result<Option<Value>> {
        let text: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        text.map(|t| serde_json::from_str(&t).map_err(Into::into))
            .transpose()
    }

    pub fn delete_setting(&mut self, key: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM settings WHERE key = ?1", [key])?
            > 0)
    }

    /// Write every top-level key of `settings` in one transaction.
    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        let Value::Object(map) = serde_json::to_value(settings)? else {
            return Err(StoreError::Invalid("settings are not an object".into()));
        };
        let tx = self.conn.transaction()?;
        for (key, value) in &map {
            put_setting(&tx, key, value)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The saved settings. A key that is not saved (or a field that a newer build added) comes
    /// from `defaults`, so an empty database returns `defaults`.
    pub fn settings(&self, defaults: &Settings) -> Result<Settings> {
        let mut doc = serde_json::to_value(defaults)?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut stored = serde_json::Map::new();
        for row in rows {
            let (key, text) = row?;
            stored.insert(key, serde_json::from_str(&text)?);
        }
        merge(&mut doc, Value::Object(stored));
        Ok(serde_json::from_value(doc)?)
    }

    /// Change one value by its dotted key, as [`Settings::set_key`] does (for example
    /// `autopilot.parkAfterMin`), and save the result. Returns the new settings. A bad key or
    /// value returns [`StoreError::Invalid`] and changes nothing.
    pub fn apply_setting(
        &mut self,
        defaults: &Settings,
        key: &str,
        value: Value,
    ) -> Result<Settings> {
        let mut next = self.settings(defaults)?;
        next.set_key(key, value).map_err(StoreError::Invalid)?;
        self.save_settings(&next)?;
        Ok(next)
    }

    /// The saved autopilot rules, or [`Autopilot::default`] when none are saved.
    pub fn autopilot(&self) -> Result<Autopilot> {
        let default = Autopilot::default();
        let Some(stored) = self.setting("autopilot")? else {
            return Ok(default);
        };
        let mut doc = serde_json::to_value(&default)?;
        merge(&mut doc, stored);
        Ok(serde_json::from_value(doc)?)
    }

    // ---- activity samples --------------------------------------------------------------

    /// Insert a sample, or replace the sample of the same minute (`at`).
    pub fn put_activity(&mut self, sample: &ActivitySample) -> Result<()> {
        self.conn.execute(
            "INSERT INTO activity_samples (at, active, waiting, idle) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (at) DO UPDATE SET
                active  = excluded.active,
                waiting = excluded.waiting,
                idle    = excluded.idle",
            params![sample.at, sample.active, sample.waiting, sample.idle],
        )?;
        Ok(())
    }

    /// The samples with `at > after`, oldest first.
    pub fn activity_since(&self, after: UnixMs) -> Result<Vec<ActivitySample>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT at, active, waiting, idle FROM activity_samples WHERE at > ?1 ORDER BY at",
        )?;
        let rows = stmt.query_map([after], |r| {
            Ok(ActivitySample {
                at: r.get(0)?,
                active: r.get(1)?,
                waiting: r.get(2)?,
                idle: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Delete the samples with `at <= up_to`. Returns the number of deleted rows.
    pub fn prune_activity(&mut self, up_to: UnixMs) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM activity_samples WHERE at <= ?1", [up_to])?)
    }

    // ---- devices -----------------------------------------------------------------------

    /// Insert a device, or replace the device with the same id.
    pub fn put_device(&mut self, device: &Device) -> Result<()> {
        self.conn.execute(
            "INSERT INTO devices (id, name, os, paired, last_seen, online)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (id) DO UPDATE SET
                name      = excluded.name,
                os        = excluded.os,
                paired    = excluded.paired,
                last_seen = excluded.last_seen,
                online    = excluded.online",
            params![
                device.id,
                device.name,
                device.os,
                device.paired,
                device.last_seen,
                device.online
            ],
        )?;
        Ok(())
    }

    pub fn device(&self, id: &str) -> Result<Option<Device>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, os, paired, last_seen, online FROM devices WHERE id = ?1",
                [id],
                device_row,
            )
            .optional()?)
    }

    /// All devices, the oldest pairing first.
    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, os, paired, last_seen, online FROM devices ORDER BY paired, id",
        )?;
        let rows = stmt.query_map([], device_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn delete_device(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM devices WHERE id = ?1", [id])?
            > 0)
    }

    // ---- helpers -----------------------------------------------------------------------

    /// Run a query whose first column is a JSON body and parse each row.
    fn query_json<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params, |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }
}

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: r.get(0)?,
        name: r.get(1)?,
        os: r.get(2)?,
        paired: r.get(3)?,
        last_seen: r.get(4)?,
        online: r.get(5)?,
    })
}

fn answered_item(body: &str, answered: UnixMs) -> Result<AnsweredItem> {
    Ok(AnsweredItem {
        item: serde_json::from_str(body)?,
        answered,
    })
}

fn put_setting(conn: &Connection, key: &str, value: &Value) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

/// The serde string of a unit enum variant, for example `planned`.
fn tag<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        Value::String(s) => Ok(s),
        other => Err(StoreError::Invalid(format!(
            "expected a string tag, got {other}"
        ))),
    }
}

/// Lay `over` on `base`: objects merge key by key, every other value replaces.
fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (key, value) in o {
                match b.get_mut(&key) {
                    Some(slot) => merge(slot, value),
                    None => {
                        b.insert(key, value);
                    }
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

fn configure(conn: &Connection, wal: bool) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    if wal {
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Invalid(format!(
                "the file system does not support WAL (journal mode '{mode}')"
            )));
        }
        // With WAL, NORMAL cannot corrupt the file; a power cut can lose the last commits only.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(())
}

/// The version in `schema_version`; 0 when the table does not exist or has no row.
fn read_version(conn: &Connection) -> Result<u32> {
    let table: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if table.is_none() {
        return Ok(0);
    }
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?)
}

/// Run the migrations after the current version, up to and including `target`. Each migration
/// and its version row commit together.
fn migrate(conn: &mut Connection, target: u32) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")?;
    let current = read_version(conn)?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::TooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index as u32 + 1;
        if version <= current || version > target {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute("DELETE FROM schema_version", [])?;
        tx.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            [version],
        )?;
        tx.commit()?;
    }
    Ok(())
}
