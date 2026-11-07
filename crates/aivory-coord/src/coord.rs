//! The coordinator: one SQLite connection, a clock, and the policy. The claims live here. The
//! radar is in [`crate::radar`] and the mailbox in [`crate::mailbox`].

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::clock::{Clock, Hlc, HlcClock, SystemClock};
use crate::delta::{Change, ChangedSymbol, TestState, TurnDelta};
use crate::error::{Error, Result};
use crate::keys;
use crate::radar::{RadarEnv, RadarHit};

/// The version of the `coord_*` tables that this crate writes.
pub const SCHEMA_VERSION: i64 = 1;

/// The length of a claim lease in milliseconds (10 minutes).
pub const LEASE_MS: i64 = 10 * 60 * 1000;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS coord_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS coord_claims (
    session    TEXT NOT NULL,
    key        TEXT NOT NULL,
    state      TEXT NOT NULL,
    expires_ms INTEGER NOT NULL,
    hlc        INTEGER NOT NULL,
    PRIMARY KEY (session, key)
);
CREATE INDEX IF NOT EXISTS coord_claims_key ON coord_claims (key);
CREATE TABLE IF NOT EXISTS coord_deltas (
    session TEXT NOT NULL,
    turn    INTEGER NOT NULL,
    hlc     INTEGER NOT NULL,
    json    TEXT NOT NULL,
    PRIMARY KEY (session, turn)
);
CREATE TABLE IF NOT EXISTS coord_mail (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    to_session   TEXT NOT NULL,
    from_session TEXT NOT NULL,
    kind         TEXT NOT NULL,
    key          TEXT NOT NULL,
    text         TEXT NOT NULL,
    deadline_ms  INTEGER,
    created_ms   INTEGER NOT NULL,
    hlc          INTEGER NOT NULL,
    delivered_ms INTEGER,
    resolved     INTEGER NOT NULL DEFAULT 0,
    count        INTEGER NOT NULL DEFAULT 1,
    digest       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS coord_mail_to ON coord_mail (to_session, delivered_ms);
CREATE TABLE IF NOT EXISTS coord_subs (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    session TEXT NOT NULL,
    key     TEXT NOT NULL,
    change  TEXT
);
CREATE TABLE IF NOT EXISTS coord_merges (
    commit_a   TEXT NOT NULL,
    commit_b   TEXT NOT NULL,
    clean      INTEGER NOT NULL,
    conflicts  TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (commit_a, commit_b)
);
";

/// The state of a claim. A claim only moves up: intent, then editing, then landed. A new edit
/// of a landed key makes it editing again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    /// The plan of the session will touch the key.
    Intent,
    /// The session changed the key.
    Editing,
    /// The change is on the target branch.
    Landed,
}

impl ClaimState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Editing => "editing",
            Self::Landed => "landed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "editing" => Self::Editing,
            "landed" => Self::Landed,
            _ => Self::Intent,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Intent => 0,
            Self::Editing => 1,
            Self::Landed => 2,
        }
    }
}

/// A lease of a session on a routing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// The session that holds the claim.
    pub session: String,
    /// The routing key.
    pub key: String,
    /// The state.
    pub state: ClaimState,
    /// The end of the lease (Unix milliseconds).
    pub expires_ms: i64,
    /// The time of the last change of the claim.
    pub hlc: Hlc,
}

/// Settings of the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Policy {
    /// When true, an editing claim on a key that another session holds with an editing claim
    /// gives a [`Gate`]. The claim is still recorded. The daemon decides what a gate does
    /// (for example: the second session must ask first).
    pub gate_editing_overlap: bool,
}

/// An overlap of editing claims that the policy turns into a gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    /// The session that made the new editing claim.
    pub session: String,
    /// The key.
    pub key: String,
    /// The other sessions that hold an editing claim on the key.
    pub holders: Vec<String>,
}

/// What a call that records a change returns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// The radar hits (empty without a [`RadarEnv`]), highest risk first.
    pub hits: Vec<RadarHit>,
    /// The gates (empty unless [`Policy::gate_editing_overlap`] is set).
    pub gates: Vec<Gate>,
}

/// The symbols that a session changed in all of its live deltas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    /// The changed symbols by key. A later `Body` change does not hide an earlier stronger
    /// change, and the base range is the one of the first turn that touched the symbol.
    pub symbols: BTreeMap<String, ChangedSymbol>,
    /// The snapshot ref of the latest delta.
    pub wip_ref: Option<String>,
}

/// Counters of the trial merges, for tests and for the daemon status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeStats {
    /// Trial merges that ran `git merge-tree`.
    pub runs: u64,
    /// Trial merges that the cache answered.
    pub cache_hits: u64,
}

/// The local coordinator of one daemon. It is synchronous and not `Sync`: the daemon keeps it
/// behind a mutex. All state is in the `coord_*` tables of the connection.
#[derive(Debug)]
pub struct Coord {
    pub(crate) conn: Connection,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) hlc: HlcClock,
    pub(crate) policy: Policy,
    pub(crate) stats: Cell<MergeStats>,
}

impl Coord {
    /// Use `conn` with the system clock. The call creates the `coord_*` tables when needed.
    pub fn new(conn: Connection) -> Result<Self> {
        Self::with_clock(conn, SystemClock)
    }

    /// Use `conn` with a given clock.
    pub fn with_clock(conn: Connection, clock: impl Clock + 'static) -> Result<Self> {
        init_schema(&conn)?;
        let last: i64 = conn.query_row(
            "SELECT MAX(
                COALESCE((SELECT MAX(hlc) FROM coord_claims), 0),
                COALESCE((SELECT MAX(hlc) FROM coord_deltas), 0),
                COALESCE((SELECT MAX(hlc) FROM coord_mail), 0))",
            [],
            |r| r.get(0),
        )?;
        Ok(Self {
            conn,
            clock: Arc::new(clock),
            hlc: HlcClock::starting_after(Hlc(u64::try_from(last).unwrap_or(0))),
            policy: Policy::default(),
            stats: Cell::new(MergeStats::default()),
        })
    }

    /// The connection, for the owner of the database.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Give back the connection.
    pub fn into_connection(self) -> Connection {
        self.conn
    }

    /// Set the policy.
    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
    }

    /// The policy.
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// The counters of the trial merges.
    pub fn merge_stats(&self) -> MergeStats {
        self.stats.get()
    }

    pub(crate) fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    pub(crate) fn tick(&self) -> Hlc {
        self.hlc.tick(self.now())
    }

    /// Merge an HLC value from another daemon. The next event is later than `remote`.
    pub fn observe(&self, remote: Hlc) {
        self.hlc.observe(remote);
    }

    // -----------------------------------------------------------------------------------------
    // Claims

    /// Claim keys for a session with a lease of [`LEASE_MS`]. A claim on a key that the session
    /// holds already renews the lease and keeps the higher state. A claim never blocks.
    /// With an `env`, the radar runs for the session afterwards.
    pub fn claim(
        &self,
        session: &str,
        keys: &[String],
        state: ClaimState,
        env: Option<&RadarEnv<'_>>,
    ) -> Result<Outcome> {
        check_session(session)?;
        for k in keys {
            if keys::kind_of(k).is_none() {
                return Err(Error::Invalid(format!("bad routing key `{k}`")));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut gates = Vec::new();
        for k in keys {
            if let Some(g) = self.upsert_claim(session, k, state, false)? {
                gates.push(g);
            }
        }
        tx.commit()?;
        self.finish(session, gates, env)
    }

    /// Record the delta of a turn. The call
    ///
    /// 1. stores the delta,
    /// 2. makes (or renews) editing claims on the path, symbol, and test keys of the delta,
    /// 3. sends an FYI ping to each session that subscribed to a changed key,
    /// 4. runs the radar when `env` is given, and sends FYI pings to both sessions of each hit.
    pub fn record_delta(&self, delta: &TurnDelta, env: Option<&RadarEnv<'_>>) -> Result<Outcome> {
        check_session(&delta.session)?;
        let json = serde_json::to_string(delta)?;
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT OR REPLACE INTO coord_deltas (session, turn, hlc, json) VALUES (?1, ?2, ?3, ?4)",
            params![delta.session, delta.turn, self.tick().0 as i64, json],
        )?;
        let mut claim_keys: BTreeSet<String> = BTreeSet::new();
        claim_keys.extend(delta.files.iter().map(|f| f.key.clone()));
        for s in &delta.changed_symbols {
            claim_keys.insert(s.key.clone());
            claim_keys.extend(s.old_key.clone());
        }
        claim_keys.extend(delta.tests.iter().map(|t| t.key.clone()));
        let mut gates = Vec::new();
        for k in &claim_keys {
            if let Some(g) = self.upsert_claim(&delta.session, k, ClaimState::Editing, true)? {
                gates.push(g);
            }
        }
        tx.commit()?;
        self.notify_subscribers(delta)?;
        self.finish(&delta.session, gates, env)
    }

    fn finish(
        &self,
        session: &str,
        mut gates: Vec<Gate>,
        env: Option<&RadarEnv<'_>>,
    ) -> Result<Outcome> {
        if !self.policy.gate_editing_overlap {
            gates.clear();
        }
        let hits = match env {
            Some(env) => self.radar(session, env)?,
            None => Vec::new(),
        };
        Ok(Outcome { hits, gates })
    }

    /// Write one claim. `force` lets the state go down (a new edit of a landed key).
    /// Returns a gate when the new state is editing and other sessions hold editing claims.
    fn upsert_claim(
        &self,
        session: &str,
        key: &str,
        state: ClaimState,
        force: bool,
    ) -> Result<Option<Gate>> {
        let now = self.now();
        let old: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT state, expires_ms FROM coord_claims WHERE session = ?1 AND key = ?2",
                params![session, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let new_state = match old {
            Some((s, exp)) if exp > now && !force => {
                let o = ClaimState::parse(&s);
                if o.rank() > state.rank() { o } else { state }
            }
            _ => state,
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO coord_claims (session, key, state, expires_ms, hlc)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session,
                key,
                new_state.as_str(),
                now + LEASE_MS,
                self.tick().0 as i64
            ],
        )?;
        if new_state != ClaimState::Editing {
            return Ok(None);
        }
        let holders = self.other_holders(session, key, ClaimState::Editing)?;
        Ok((!holders.is_empty()).then(|| Gate {
            session: session.to_owned(),
            key: key.to_owned(),
            holders,
        }))
    }

    fn other_holders(&self, session: &str, key: &str, state: ClaimState) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT session FROM coord_claims
             WHERE key = ?1 AND session <> ?2 AND state = ?3 AND expires_ms > ?4
             ORDER BY session",
        )?;
        let rows = stmt.query_map(params![key, session, state.as_str(), self.now()], |r| {
            r.get(0)
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Renew the lease of every intent and editing claim of the session. A claim that expired
    /// is not renewed (the session must claim it again). Landed claims are not renewed.
    /// Returns the number of renewed claims. The daemon calls this while the session lives.
    pub fn heartbeat(&self, session: &str) -> Result<usize> {
        let now = self.now();
        Ok(self.conn.execute(
            "UPDATE coord_claims SET expires_ms = ?2, hlc = ?3
             WHERE session = ?1 AND state <> 'landed' AND expires_ms > ?4",
            params![session, now + LEASE_MS, self.tick().0 as i64, now],
        )?)
    }

    /// The change of the session is on the target branch. Editing claims become landed (they
    /// stay visible for one lease), and the stored deltas go away.
    /// Returns the number of claims that became landed.
    pub fn land(&self, session: &str) -> Result<usize> {
        let now = self.now();
        let n = self.conn.execute(
            "UPDATE coord_claims SET state = 'landed', expires_ms = ?2, hlc = ?3
             WHERE session = ?1 AND state = 'editing' AND expires_ms > ?4",
            params![session, now + LEASE_MS, self.tick().0 as i64, now],
        )?;
        self.conn
            .execute("DELETE FROM coord_deltas WHERE session = ?1", [session])?;
        Ok(n)
    }

    /// The session ended. Delete its claims and deltas. The mailbox and the subscriptions stay.
    pub fn release(&self, session: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM coord_claims WHERE session = ?1", [session])?;
        self.conn
            .execute("DELETE FROM coord_deltas WHERE session = ?1", [session])?;
        Ok(())
    }

    /// Delete expired claims, the deltas of sessions without a claim, and merge cache rows
    /// older than one day. Returns the number of deleted claims. The queries ignore expired
    /// claims already; this call only frees space.
    pub fn sweep(&self) -> Result<usize> {
        let now = self.now();
        let n = self
            .conn
            .execute("DELETE FROM coord_claims WHERE expires_ms <= ?1", [now])?;
        self.conn.execute(
            "DELETE FROM coord_deltas WHERE session NOT IN (SELECT session FROM coord_claims)",
            [],
        )?;
        self.conn.execute(
            "DELETE FROM coord_merges WHERE created_ms < ?1",
            [now - 24 * 60 * 60 * 1000],
        )?;
        Ok(n)
    }

    /// The live claims of one session (the lease has not ended), ordered by key.
    pub fn claims_of(&self, session: &str) -> Result<Vec<Claim>> {
        self.query_claims(
            "SELECT session, key, state, expires_ms, hlc FROM coord_claims
             WHERE session = ?1 AND expires_ms > ?2 ORDER BY key",
            params![session, self.now()],
        )
    }

    /// The live claims on one key, ordered by session.
    pub fn claims_on(&self, key: &str) -> Result<Vec<Claim>> {
        self.query_claims(
            "SELECT session, key, state, expires_ms, hlc FROM coord_claims
             WHERE key = ?1 AND expires_ms > ?2 ORDER BY session",
            params![key, self.now()],
        )
    }

    fn query_claims(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Claim>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map(p, |r| {
            Ok(Claim {
                session: r.get(0)?,
                key: r.get(1)?,
                state: ClaimState::parse(&r.get::<_, String>(2)?),
                expires_ms: r.get(3)?,
                hlc: Hlc(r.get::<_, i64>(4)? as u64),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The sessions with a live editing claim, ordered by id.
    pub fn editing_sessions(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT session FROM coord_claims
             WHERE state = 'editing' AND expires_ms > ?1 ORDER BY session",
        )?;
        let rows = stmt.query_map([self.now()], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The keys of the live editing claims of a session.
    pub(crate) fn editing_keys(&self, session: &str) -> Result<BTreeSet<String>> {
        Ok(self
            .claims_of(session)?
            .into_iter()
            .filter(|c| c.state == ClaimState::Editing)
            .map(|c| c.key)
            .collect())
    }

    /// The stored deltas of a session, oldest turn first.
    pub fn deltas_of(&self, session: &str) -> Result<Vec<TurnDelta>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT json FROM coord_deltas WHERE session = ?1 ORDER BY turn")?;
        let rows = stmt.query_map([session], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            out.push(serde_json::from_str(&json?)?);
        }
        Ok(out)
    }

    /// The symbols that the session changed in its stored deltas.
    pub fn footprint(&self, session: &str) -> Result<Footprint> {
        let mut fp = Footprint::default();
        for d in self.deltas_of(session)? {
            fp.wip_ref = d.wip_ref.clone().or(fp.wip_ref);
            for s in d.changed_symbols {
                match fp.symbols.get_mut(&s.key) {
                    Some(old) => {
                        let base = old.base_range;
                        if s.change != Change::Body {
                            *old = s;
                        } else {
                            old.new_range = s.new_range;
                        }
                        old.base_range = base.or(old.base_range);
                    }
                    None => {
                        fp.symbols.insert(s.key.clone(), s);
                    }
                }
            }
        }
        Ok(fp)
    }

    // -----------------------------------------------------------------------------------------
    // Subscriptions fire on a delta

    fn notify_subscribers(&self, delta: &TurnDelta) -> Result<()> {
        // (key, old key, change, what happened)
        let mut events: Vec<(String, Option<String>, Option<Change>, String)> = Vec::new();
        for s in &delta.changed_symbols {
            events.push((
                s.key.clone(),
                s.old_key.clone(),
                Some(s.change),
                describe(&delta.session, &s.key, s.old_key.as_deref(), Some(s.change)),
            ));
        }
        for t in &delta.tests {
            let change = match t.state {
                TestState::Added => Some(Change::Added),
                TestState::Removed => Some(Change::Removed),
                TestState::Failing => None,
            };
            let what = match t.state {
                TestState::Failing => {
                    format!("{} made the test `{}` fail", delta.session, name_of(&t.key))
                }
                _ => describe(&delta.session, &t.key, None, change),
            };
            events.push((t.key.clone(), None, change, what));
        }
        for f in &delta.files {
            events.push((
                f.key.clone(),
                None,
                None,
                format!("{} changed the file `{}`", delta.session, name_of(&f.key)),
            ));
        }
        let subs = self.all_subscriptions()?;
        for sub in subs.iter().filter(|s| s.session != delta.session) {
            for (key, old, change, what) in &events {
                let key_hit = keys::matches(&sub.key, key)
                    || old.as_deref().is_some_and(|o| keys::matches(&sub.key, o));
                let change_hit = sub.change.is_none_or(|c| Some(c) == *change);
                if key_hit && change_hit {
                    self.send(&crate::mailbox::Ping {
                        from: delta.session.clone(),
                        to: sub.session.clone(),
                        kind: crate::mailbox::PingKind::Fyi,
                        key: sub.key.clone(),
                        text: what.clone(),
                        deadline_ms: None,
                    })?;
                }
            }
        }
        Ok(())
    }
}

/// The name part of a key (the text after the kind, and after the language for a symbol).
pub(crate) fn name_of(key: &str) -> &str {
    match keys::split_sym(key) {
        Some((_, q)) => q,
        None => key.split_once(':').map_or(key, |(_, rest)| rest),
    }
}

/// A short statement about a change, for a ping.
pub(crate) fn describe(
    session: &str,
    key: &str,
    old: Option<&str>,
    change: Option<Change>,
) -> String {
    let name = name_of(key);
    match change {
        Some(Change::Added) => format!("{session} added `{name}`"),
        Some(Change::Removed) => format!("{session} removed `{name}`"),
        Some(Change::Renamed) => format!(
            "{session} renamed `{}` to `{name}`",
            old.map_or("?", name_of)
        ),
        Some(Change::Signature) => format!("{session} changed the signature of `{name}`"),
        Some(Change::Body) => format!("{session} changed the body of `{name}`"),
        None => format!("{session} changed `{name}`"),
    }
}

fn check_session(session: &str) -> Result<()> {
    if session.trim().is_empty() {
        return Err(Error::Invalid("empty session id".to_owned()));
    }
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    let found: Option<i64> = conn
        .query_row(
            "SELECT value FROM coord_meta WHERE key = 'schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.parse().ok());
    match found {
        Some(v) if v > SCHEMA_VERSION => Err(Error::SchemaTooNew {
            found: v,
            supported: SCHEMA_VERSION,
        }),
        Some(_) => Ok(()),
        None => {
            conn.execute(
                "INSERT INTO coord_meta (key, value) VALUES ('schema_version', ?1)",
                [SCHEMA_VERSION.to_string()],
            )?;
            Ok(())
        }
    }
}
