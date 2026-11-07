//! The conflict radar.
//!
//! [`Coord::radar`] compares one session with every other session that has a live editing
//! claim. A pair matches in one of three tiers, from cheap to expensive:
//!
//! 1. **Path:** both sessions have editing claims on the same file.
//! 2. **Symbol:** both sessions have editing claims on the same symbol or test key, or the
//!    declaration ranges that they changed overlap in one file (ranges of the old version).
//! 3. **Dependency:** one session changed the signature of S (or removed or renamed it), and
//!    the workspace of the other session references S. The reference check reads the index of
//!    the base plus the delta of the other session ([`RefSource`]).
//!
//! Only matched pairs get a trial merge (`git merge-tree`) between the latest snapshots of the
//! two sessions. The result is cached by the pair of commit ids.
//!
//! The risk score:
//!
//! | Result | Score |
//! |---|---|
//! | Text conflict | 1.0 |
//! | Clean merge, tier 3 match (signature change used by the other side) | 0.7 |
//! | Clean merge, tier 2 match (shared symbols changed) | 0.4 |
//! | Clean merge, tier 1 match only | 0.1 |
//!
//! When no trial merge can run (a session has no snapshot, or the snapshots have no common
//! history), the score is the clean-merge score of the tier, and [`RadarHit::trial_error`]
//! says why. Snapshots of two sessions need a common ancestor: the first snapshot of a session
//! must use the base commit as its parent (`SnapshotRequest::parent`).

use std::collections::BTreeSet;
use std::path::Path;

use aivory_index::{Index, PageReq};
use aivory_vcs::Vcs;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::coord::{Coord, Footprint};
use crate::error::{Error, Result};
use crate::keys;
use crate::mailbox::{Ping, PingKind};

/// Tier of a path match.
pub const TIER_PATH: u8 = 1;
/// Tier of a symbol match.
pub const TIER_SYMBOL: u8 = 2;
/// Tier of a dependency match.
pub const TIER_DEPENDENCY: u8 = 3;

/// The workspace references of a name. Tier 3 of the radar uses it.
pub trait RefSource {
    /// The paths in which the workspace of `session` (base plus delta) references `name`.
    /// `name` is a bare name, without a qualifier.
    fn references(&self, session: &str, name: &str) -> Result<Vec<String>>;
}

impl RefSource for Index {
    fn references(&self, session: &str, name: &str) -> Result<Vec<String>> {
        let page = Index::references(self, Some(session), name, PageReq::new(1, 200))
            .map_err(|e| Error::Index(e.to_string()))?;
        let mut paths: Vec<String> = page.items.into_iter().map(|h| h.path).collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// A [`RefSource`] with no references. It turns tier 3 off.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRefs;

impl RefSource for NoRefs {
    fn references(&self, _session: &str, _name: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// The things that the radar reads besides the database.
#[derive(Clone, Copy)]
pub struct RadarEnv<'a> {
    /// The version control backend (snapshots and trial merge).
    pub vcs: &'a dyn Vcs,
    /// The repository folder that holds the snapshot refs of all sessions.
    pub repo: &'a Path,
    /// The reference index.
    pub refs: &'a dyn RefSource,
}

impl std::fmt::Debug for RadarEnv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadarEnv")
            .field("repo", &self.repo)
            .finish_non_exhaustive()
    }
}

/// A matched pair of sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadarHit {
    /// The first session (the lower id).
    pub a: String,
    /// The second session.
    pub b: String,
    /// The highest tier that matched (1, 2, or 3).
    pub tier: u8,
    /// The keys that caused the match, sorted.
    pub keys: Vec<String>,
    /// The risk score, 0.1 to 1.0.
    pub score: f32,
    /// The files with a text conflict.
    pub conflict_files: Vec<String>,
    /// True when a trial merge decided the score.
    pub trial: bool,
    /// Why no trial merge ran, when `trial` is false.
    pub trial_error: Option<String>,
}

/// The matches of a pair, before the trial merge.
#[derive(Default)]
struct Matches {
    path: BTreeSet<String>,
    symbol: BTreeSet<String>,
    dependency: BTreeSet<String>,
}

impl Coord {
    /// Run the radar for `session`. [`Coord::record_delta`] and [`Coord::claim`] call it when
    /// they get an env. The call sends an FYI ping to both sessions of each hit.
    /// Hits come highest risk first.
    pub fn radar(&self, session: &str, env: &RadarEnv<'_>) -> Result<Vec<RadarHit>> {
        let mine = self.editing_keys(session)?;
        if mine.is_empty() {
            return Ok(Vec::new());
        }
        let my_fp = self.footprint(session)?;
        let mut hits = Vec::new();
        for other in self.editing_sessions()? {
            if other == session {
                continue;
            }
            let theirs = self.editing_keys(&other)?;
            let their_fp = self.footprint(&other)?;
            let m = self.match_pair(session, &mine, &my_fp, &other, &theirs, &their_fp, env)?;
            if let Some(hit) = self.judge(session, &other, m, env)? {
                self.ping_pair(&hit)?;
                hits.push(hit);
            }
        }
        hits.sort_by(|x, y| {
            y.score
                .total_cmp(&x.score)
                .then_with(|| (&x.a, &x.b).cmp(&(&y.a, &y.b)))
        });
        Ok(hits)
    }

    #[allow(clippy::too_many_arguments)]
    fn match_pair(
        &self,
        x: &str,
        kx: &BTreeSet<String>,
        fx: &Footprint,
        y: &str,
        ky: &BTreeSet<String>,
        fy: &Footprint,
        env: &RadarEnv<'_>,
    ) -> Result<Matches> {
        let mut m = Matches::default();
        for k in kx.intersection(ky) {
            match keys::kind_of(k) {
                Some("path") => {
                    m.path.insert(k.clone());
                }
                Some("sym" | "test") => {
                    m.symbol.insert(k.clone());
                }
                _ => {}
            }
        }
        // Declaration ranges of the old version that overlap in one file.
        for sx in fx.symbols.values() {
            for sy in fy.symbols.values() {
                if sx.path != sy.path {
                    continue;
                }
                if let (Some(rx), Some(ry)) = (&sx.base_range, &sy.base_range)
                    && rx.overlaps(ry)
                {
                    m.symbol.insert(sx.key.clone());
                    m.symbol.insert(sy.key.clone());
                }
            }
        }
        // Tier 3, both directions.
        self.dependency(fx, y, env, &mut m.dependency)?;
        self.dependency(fy, x, env, &mut m.dependency)?;
        Ok(m)
    }

    /// Keys of `changed` that break callers and that the workspace of `reader` references.
    fn dependency(
        &self,
        changed: &Footprint,
        reader: &str,
        env: &RadarEnv<'_>,
        out: &mut BTreeSet<String>,
    ) -> Result<()> {
        for s in changed
            .symbols
            .values()
            .filter(|s| s.change.breaks_callers())
        {
            // After a rename the callers use the old name.
            let key = s.old_key.as_deref().unwrap_or(&s.key);
            let Some((_, qualified)) = keys::split_sym(key) else {
                continue;
            };
            let name = keys::last_name(qualified);
            if !env.refs.references(reader, name)?.is_empty() {
                out.insert(s.key.clone());
            }
        }
        Ok(())
    }

    /// Decide tier and score. Runs the trial merge for a matched pair.
    fn judge(&self, x: &str, y: &str, m: Matches, env: &RadarEnv<'_>) -> Result<Option<RadarHit>> {
        let tier = if !m.dependency.is_empty() {
            TIER_DEPENDENCY
        } else if !m.symbol.is_empty() {
            TIER_SYMBOL
        } else if !m.path.is_empty() {
            TIER_PATH
        } else {
            return Ok(None);
        };
        let clean_score = match tier {
            TIER_DEPENDENCY => 0.7,
            TIER_SYMBOL => 0.4,
            _ => 0.1,
        };
        let (a, b) = if x <= y { (x, y) } else { (y, x) };
        let mut keys: BTreeSet<String> = BTreeSet::new();
        keys.extend(m.path);
        keys.extend(m.symbol);
        keys.extend(m.dependency);

        let mut hit = RadarHit {
            a: a.to_owned(),
            b: b.to_owned(),
            tier,
            keys: keys.into_iter().collect(),
            score: clean_score,
            conflict_files: Vec::new(),
            trial: false,
            trial_error: None,
        };
        match self.trial(a, b, env) {
            Ok((clean, conflicts)) => {
                hit.trial = true;
                if !clean {
                    hit.score = 1.0;
                    hit.conflict_files = conflicts;
                }
            }
            Err(e) => hit.trial_error = Some(e),
        }
        Ok(Some(hit))
    }

    /// The commit of the latest snapshot of a session, or of its last `wip_ref`.
    fn latest_commit(
        &self,
        session: &str,
        env: &RadarEnv<'_>,
    ) -> std::result::Result<String, String> {
        if let Ok(snaps) = env.vcs.list_snapshots(env.repo, session)
            && let Some(last) = snaps.last()
        {
            return Ok(last.commit.clone());
        }
        let wip = self
            .footprint(session)
            .ok()
            .and_then(|f| f.wip_ref)
            .ok_or_else(|| format!("session {session} has no snapshot"))?;
        env.vcs
            .resolve(env.repo, &wip)
            .map_err(|e| format!("cannot resolve {wip}: {e}"))
    }

    /// The trial merge of the latest snapshots of two sessions, from the cache when possible.
    /// Returns (clean, conflicted files), or the reason why no merge ran.
    fn trial(
        &self,
        a: &str,
        b: &str,
        env: &RadarEnv<'_>,
    ) -> std::result::Result<(bool, Vec<String>), String> {
        let ca = self.latest_commit(a, env)?;
        let cb = self.latest_commit(b, env)?;
        let (lo, hi) = if ca <= cb { (ca, cb) } else { (cb, ca) };
        let cached: Option<(bool, String)> = self
            .conn
            .query_row(
                "SELECT clean, conflicts FROM coord_merges WHERE commit_a = ?1 AND commit_b = ?2",
                params![lo, hi],
                |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let mut stats = self.stats.get();
        if let Some((clean, json)) = cached {
            stats.cache_hits += 1;
            self.stats.set(stats);
            let files = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            return Ok((clean, files));
        }
        let out = env
            .vcs
            .trial_merge(env.repo, &lo, &hi)
            .map_err(|e| format!("trial merge failed: {e}"))?;
        stats.runs += 1;
        self.stats.set(stats);
        let json = serde_json::to_string(&out.conflicts).map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO coord_merges (commit_a, commit_b, clean, conflicts, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lo, hi, i64::from(out.clean), json, self.now()],
            )
            .map_err(|e| e.to_string())?;
        Ok((out.clean, out.conflicts))
    }

    /// Send an FYI ping to both sessions of a hit. The ping to one session names the other
    /// session as the sender. The text has only keys, tier, and score, never file content.
    fn ping_pair(&self, hit: &RadarHit) -> Result<()> {
        let key = hit.keys.first().cloned().unwrap_or_default();
        let shown: Vec<&str> = hit.keys.iter().take(3).map(String::as_str).collect();
        let more = hit.keys.len().saturating_sub(shown.len());
        let mut keys_text = shown.join(", ");
        if more > 0 {
            keys_text.push_str(&format!(" and {more} more"));
        }
        let conflict = if hit.conflict_files.is_empty() {
            String::new()
        } else {
            format!("; text conflict in {}", hit.conflict_files.join(", "))
        };
        for (to, from) in [(&hit.a, &hit.b), (&hit.b, &hit.a)] {
            self.send(&Ping {
                from: from.clone(),
                to: to.clone(),
                kind: PingKind::Fyi,
                key: key.clone(),
                text: format!(
                    "{from} also changes {keys_text} (radar tier {}, risk {:.1}{conflict})",
                    hit.tier, hit.score
                ),
                deadline_ms: None,
            })?;
        }
        Ok(())
    }
}
