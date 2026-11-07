//! Features on top of the [`Vcs`] trait and the [`Ledger`]: end of a turn, blame by prompt,
//! the file timeline, and the revert of a turn. They work with every backend.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::{Result, VcsError};
use crate::ledger::{Ledger, Turn, TurnFile};
use crate::types::{BASELINE_TURN, CommitInfo, LogOptions, Snapshot, SnapshotRequest, TurnRef};
use crate::vcs::Vcs;

/// End a turn: snapshot the folder, read the file changes from the snapshot diff, and write the
/// turn to the ledger.
///
/// The caller fills the turn data (session, number, provider, model, prompt, times, usage).
/// This function sets `snapshot_ref`, `parent_snapshot`, and `files`. When the turn changed
/// nothing, `snapshot_ref` stays `None`, and the turn has no files. The baseline turn (0) has
/// no files, too.
pub fn finish_turn(
    vcs: &dyn Vcs,
    ledger: &Ledger<'_>,
    workdir: &Path,
    mut turn: Turn,
) -> Result<Turn> {
    let previous = vcs.snapshot_before(workdir, &turn.session_id, turn.n)?;
    // The first snapshot of a session has the base commit (`HEAD`) as its parent, so that the
    // snapshots of two sessions of one repository share history and `trial_merge` works. A
    // repository without a commit has no base: the snapshot has no parent then.
    let parent = match &previous {
        Some(s) => Some(s.commit.clone()),
        None => vcs.resolve(workdir, "HEAD").ok(),
    };
    let req = SnapshotRequest {
        session: turn.session_id.clone(),
        n: turn.n,
        provider: turn.provider.clone(),
        model: turn.model.clone().unwrap_or_default(),
        time: (turn.ended_at > 0).then_some(turn.ended_at),
        parent,
    };
    match vcs.snapshot(workdir, &req)? {
        Some(snap) => {
            // The baseline only records the start state: it changed no file.
            turn.files = if turn.n == BASELINE_TURN {
                Vec::new()
            } else {
                vcs.diff_trees(workdir, snap.parent.as_deref(), &snap.commit)?
                    .iter()
                    .map(TurnFile::from)
                    .collect()
            };
            turn.parent_snapshot = snap.parent;
            turn.snapshot_ref = Some(snap.commit);
        }
        None => {
            turn.parent_snapshot = previous.map(|s| s.commit);
            turn.snapshot_ref = None;
            turn.files.clear();
        }
    }
    ledger.record_turn(&turn)
}

/// Revert a turn: restore the snapshot before turn `n` into `workdir`, and record the revert in
/// the ledger. This also drops the changes of every later turn, because the folder returns to
/// the earlier state. The caller decides when to call it. Returns the restored snapshot.
pub fn revert_turn(
    vcs: &dyn Vcs,
    ledger: &Ledger<'_>,
    workdir: &Path,
    session: &str,
    n: u32,
    now: i64,
) -> Result<Snapshot> {
    let snap = vcs
        .snapshot_before(workdir, session, n)?
        .ok_or_else(|| VcsError::NotFound(format!("snapshot before turn {session}/{n}")))?;
    vcs.restore(workdir, &snap.commit)?;
    let detail = format!("restored snapshot {} (turn {})", snap.commit, snap.n);
    ledger.record_op(session, "revert", Some(n), now, Some(&detail))?;
    Ok(snap)
}

/// One line of a blame by prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptBlameLine {
    /// Line number in the working tree (1-based).
    pub line: u32,
    /// Content of the line.
    pub content: String,
    /// Commit of the line. All zeros when the line is not committed.
    pub commit: String,
    /// True when the line is not committed.
    pub uncommitted: bool,
    /// Summary of the commit.
    pub summary: String,
    /// Commit time (Unix seconds). 0 for an uncommitted line.
    pub time: i64,
    /// Turns that the `AIVory-Turns` trailer of the commit names.
    pub commit_turns: Vec<TurnRef>,
    /// The exact turn that wrote the line, from the snapshot chain. `None` when the line did not
    /// come from a turn, or when a human edited it after the last snapshot.
    pub turn_ref: Option<TurnRef>,
    /// The ledger entry of `turn_ref`: prompt, model, time, cost.
    pub turn: Option<Turn>,
}

/// Cached data of one blame run.
struct Ctx<'a> {
    vcs: &'a dyn Vcs,
    ledger: &'a Ledger<'a>,
    repo: &'a Path,
    commit_turns: HashMap<String, Vec<TurnRef>>,
    snapshots: HashMap<String, Vec<Snapshot>>,
    /// `(contents id, session, snapshot commit)` -> line number -> turn number.
    chains: HashMap<(String, String, String), Option<HashMap<u32, u32>>>,
    turns: HashMap<TurnRef, Option<Turn>>,
}

impl Ctx<'_> {
    fn commit_turns(&mut self, commit: &str) -> Vec<TurnRef> {
        if let Some(t) = self.commit_turns.get(commit) {
            return t.clone();
        }
        let opts = LogOptions {
            rev: Some(commit.to_owned()),
            path: None,
            max_count: Some(1),
        };
        let turns = self
            .vcs
            .log(self.repo, &opts)
            .ok()
            .and_then(|l| l.into_iter().next())
            .map(|c: CommitInfo| c.turns)
            .unwrap_or_default();
        self.commit_turns.insert(commit.to_owned(), turns.clone());
        turns
    }

    fn snapshots(&mut self, session: &str) -> &[Snapshot] {
        if !self.snapshots.contains_key(session) {
            let snaps = self
                .vcs
                .list_snapshots(self.repo, session)
                .unwrap_or_default();
            self.snapshots.insert(session.to_owned(), snaps);
        }
        &self.snapshots[session]
    }

    /// Turn number of `line` in a blame of `contents` on top of the snapshot `upto`.
    fn chain_turn(
        &mut self,
        contents_id: &str,
        contents: impl FnOnce() -> Option<Vec<u8>>,
        path: &str,
        session: &str,
        upto: &Snapshot,
        line: u32,
    ) -> Option<u32> {
        let key = (
            contents_id.to_owned(),
            session.to_owned(),
            upto.commit.clone(),
        );
        if !self.chains.contains_key(&key) {
            let chain = contents().and_then(|bytes| {
                let blamed = self
                    .vcs
                    .blame_contents(self.repo, &upto.commit, path, &bytes)
                    .ok()?;
                let by_commit: HashMap<&str, u32> = self
                    .snapshots
                    .get(session)?
                    .iter()
                    .map(|s| (s.commit.as_str(), s.n))
                    .collect();
                Some(
                    blamed
                        .iter()
                        .filter(|l| !l.uncommitted)
                        .filter_map(|l| by_commit.get(l.commit.as_str()).map(|n| (l.line, *n)))
                        .collect::<HashMap<u32, u32>>(),
                )
            });
            self.chains.insert(key.clone(), chain);
        }
        self.chains.get(&key)?.as_ref()?.get(&line).copied()
    }

    fn turn(&mut self, r: &TurnRef) -> Option<Turn> {
        if !self.turns.contains_key(r) {
            let t = self.ledger.get_turn(&r.session, r.n).ok().flatten();
            self.turns.insert(r.clone(), t);
        }
        self.turns[r].clone()
    }
}

/// Blame by prompt: for each line of a file, the commit, and the turn that wrote the line.
///
/// 1. A normal blame of the working tree gives the commit of each line.
/// 2. When the commit has an `AIVory-Turns` trailer, a blame over the snapshot chain of the
///    named sessions gives the exact turn. The chain blame compares the committed file with the
///    last named snapshot, so a line that a human edited has no turn.
/// 3. For an uncommitted line, `session` names the live session. The chain blame compares the
///    working tree with the latest snapshot of that session.
///
/// A line from the baseline snapshot (turn 0) has no turn: the session did not write it.
pub fn blame_by_prompt(
    vcs: &dyn Vcs,
    ledger: &Ledger<'_>,
    repo: &Path,
    path: &str,
    range: Option<(u32, u32)>,
    session: Option<&str>,
) -> Result<Vec<PromptBlameLine>> {
    let lines = vcs.blame(repo, None, path, range)?;
    let mut ctx = Ctx {
        vcs,
        ledger,
        repo,
        commit_turns: HashMap::new(),
        snapshots: HashMap::new(),
        chains: HashMap::new(),
        turns: HashMap::new(),
    };
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        let mut commit_turns = Vec::new();
        let mut found: Option<TurnRef> = None;
        if l.uncommitted {
            if let Some(session) = session {
                let latest = ctx.snapshots(session).last().cloned();
                if let Some(upto) = latest {
                    let n = ctx.chain_turn(
                        "workdir",
                        || fs::read(repo.join(path)).ok(),
                        path,
                        session,
                        &upto,
                        l.line,
                    );
                    found = n.map(|n| TurnRef {
                        session: session.to_owned(),
                        n,
                    });
                }
            }
        } else {
            commit_turns = ctx.commit_turns(&l.commit);
            let mut sessions: Vec<&str> = Vec::new();
            for t in &commit_turns {
                if !sessions.contains(&t.session.as_str()) {
                    sessions.push(&t.session);
                }
            }
            for s in sessions {
                let max_n = commit_turns
                    .iter()
                    .filter(|t| t.session == s)
                    .map(|t| t.n)
                    .max()
                    .unwrap_or(0);
                let upto = ctx
                    .snapshots(s)
                    .iter()
                    .rev()
                    .find(|x| x.n <= max_n)
                    .cloned();
                let Some(upto) = upto else { continue };
                let n = ctx.chain_turn(
                    &l.commit,
                    || vcs.show_file(repo, &l.commit, &l.orig_path).ok(),
                    &l.orig_path,
                    s,
                    &upto,
                    l.orig_line,
                );
                if let Some(n) = n {
                    found = Some(TurnRef {
                        session: s.to_owned(),
                        n,
                    });
                    break;
                }
            }
        }
        let turn_ref = found.filter(|t| t.n != BASELINE_TURN);
        let turn = turn_ref.as_ref().and_then(|r| ctx.turn(r));
        out.push(PromptBlameLine {
            line: l.line,
            content: l.content,
            commit: l.commit,
            uncommitted: l.uncommitted,
            summary: l.summary,
            time: l.time,
            commit_turns,
            turn_ref,
            turn,
        });
    }
    Ok(out)
}

/// One point on the timeline of a file.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineEvent {
    /// A commit of the VCS history.
    Commit(CommitInfo),
    /// A turn that changed the file. The turn can belong to a live session.
    Turn(Box<Turn>),
}

impl TimelineEvent {
    /// Time of the event (Unix seconds). A turn counts at its end.
    pub fn time(&self) -> i64 {
        match self {
            Self::Commit(c) => c.time,
            Self::Turn(t) => t.ended_at,
        }
    }
}

/// The timeline of a file: commits and turns in time order, oldest first. At equal times, a turn
/// comes before a commit, because a commit publishes earlier turns.
pub fn file_timeline(
    vcs: &dyn Vcs,
    ledger: &Ledger<'_>,
    repo: &Path,
    path: &str,
    session: Option<&str>,
) -> Result<Vec<TimelineEvent>> {
    let path = path.replace('\\', "/");
    let opts = LogOptions {
        rev: None,
        path: Some(path.clone()),
        max_count: None,
    };
    let mut events: Vec<TimelineEvent> = vcs
        .log(repo, &opts)?
        .into_iter()
        .rev()
        .map(TimelineEvent::Commit)
        .collect();
    events.extend(
        ledger
            .turns_for_file(&path, session)?
            .into_iter()
            .map(|t| TimelineEvent::Turn(Box::new(t))),
    );
    // Stable sort: equal keys keep the order (commits oldest first, then turns by end time).
    events.sort_by_key(|e| {
        let (rank, n) = match e {
            TimelineEvent::Turn(t) => (0u8, t.n),
            TimelineEvent::Commit(_) => (1, 0),
        };
        (e.time(), rank, n)
    });
    Ok(events)
}
