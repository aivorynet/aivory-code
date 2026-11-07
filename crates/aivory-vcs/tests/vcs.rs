//! Integration tests with temporary Git repositories (synthetic data only).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use aivory_vcs::{
    ChangeStatus, GitVcs, Ledger, LogOptions, PublishRequest, SnapshotRequest, TimelineEvent, Turn,
    TurnRef, TurnTool, Vcs, WorkspaceRequest, blame_by_prompt, file_timeline, finish_turn,
    revert_turn,
};
use rusqlite::Connection;
use tempfile::TempDir;

// ---- Helpers ------------------------------------------------------------------------------

fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_env(dir, args, &[])
}

/// A new repository on branch `main` with a local identity.
fn init_repo() -> TempDir {
    let tmp = tempfile::tempdir().expect("temp dir");
    let d = tmp.path();
    git(d, &["init", "-q", "-b", "main", "."]);
    git(d, &["config", "user.name", "Test User"]);
    git(d, &["config", "user.email", "test@example.invalid"]);
    git(d, &["config", "core.autocrlf", "false"]);
    git(d, &["config", "commit.gpgsign", "false"]);
    tmp
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("create dirs");
    }
    fs::write(p, content).expect("write file");
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).expect("read file")
}

/// Commit all tracked changes and new files at a fixed time.
fn commit_all(dir: &Path, time: i64, msgs: &[&str]) -> String {
    git(dir, &["add", "-A"]);
    let date = format!("@{time} +0000");
    let env = [
        ("GIT_AUTHOR_DATE", date.as_str()),
        ("GIT_COMMITTER_DATE", date.as_str()),
    ];
    let mut args = vec!["commit", "-q"];
    for m in msgs {
        args.extend(["-m", m]);
    }
    git_env(dir, &args, &env);
    git(dir, &["rev-parse", "HEAD"])
}

fn turn(session: &str, n: u32, prompt: &str, ended_at: i64) -> Turn {
    Turn {
        session_id: session.to_owned(),
        n,
        provider: "claude-code".to_owned(),
        agent: Some("builder".to_owned()),
        model: Some("claude-opus-5-5".to_owned()),
        prompt: prompt.to_owned(),
        prompt_source: "human".to_owned(),
        started_at: ended_at - 10,
        ended_at,
        cost_usd: Some(0.25),
        ..Turn::default()
    }
}

fn snap_req(session: &str, n: u32) -> SnapshotRequest {
    SnapshotRequest {
        session: session.to_owned(),
        n,
        provider: "claude-code".to_owned(),
        model: "claude-opus-5-5".to_owned(),
        ..SnapshotRequest::default()
    }
}

fn mem_db() -> Connection {
    Connection::open_in_memory().expect("open sqlite")
}

/// Everything about the user state that a snapshot must not change.
#[derive(Debug, PartialEq)]
struct UserState {
    status: String,
    head: String,
    branches: String,
    index_bytes: Vec<u8>,
    files: Vec<(String, String)>,
}

fn user_state(dir: &Path, files: &[&str]) -> UserState {
    let status = git(dir, &["status", "--porcelain", "-uall"]);
    UserState {
        status,
        head: git(dir, &["rev-parse", "HEAD"]),
        branches: git(dir, &["for-each-ref", "refs/heads"]),
        index_bytes: fs::read(dir.join(".git").join("index")).unwrap_or_default(),
        files: files
            .iter()
            .map(|f| {
                (
                    (*f).to_owned(),
                    fs::read_to_string(dir.join(f)).unwrap_or_default(),
                )
            })
            .collect(),
    }
}

// ---- Snapshot -----------------------------------------------------------------------------

#[test]
fn snapshot_leaves_user_state_alone() {
    let repo = init_repo();
    let d = repo.path();
    write(d, ".gitignore", "*.log\n");
    write(d, "tracked.txt", "one\n");
    write(d, "ignored.log", "noise\n");
    commit_all(d, 1000, &["base"]);
    git(d, &["branch", "side"]);
    // A mixed user state: unstaged edit, staged file, untracked file, ignored file.
    write(d, "tracked.txt", "one\ntwo\n");
    write(d, "staged.txt", "staged\n");
    git(d, &["add", "staged.txt"]);
    write(d, "untracked.txt", "new\n");
    write(d, "ignored.log", "more noise\n");

    let files = ["tracked.txt", "staged.txt", "untracked.txt", "ignored.log"];
    let before = user_state(d, &files);
    let vcs = GitVcs::new();
    let snap = vcs
        .snapshot(d, &snap_req("S-1", 0))
        .unwrap()
        .expect("snapshot");
    let after = user_state(d, &files);
    assert_eq!(before, after, "snapshot changed the user state");

    // The snapshot has the work state, without ignored files.
    let listing = git(d, &["ls-tree", "-r", "--name-only", &snap.commit]);
    let names: Vec<&str> = listing.lines().collect();
    assert!(names.contains(&"tracked.txt"));
    assert!(names.contains(&"staged.txt"));
    assert!(names.contains(&"untracked.txt"));
    assert!(!names.contains(&"ignored.log"));
    assert_eq!(
        String::from_utf8(vcs.show_file(d, &snap.commit, "tracked.txt").unwrap()).unwrap(),
        "one\ntwo\n"
    );
    // The ref exists, and the commit has the trailers.
    assert_eq!(
        git(d, &["rev-parse", "refs/aivory/turns/S-1/0"]),
        snap.commit
    );
    let msg = git(d, &["log", "-1", "--format=%B", &snap.commit]);
    for t in [
        "AIVory-Session: S-1",
        "AIVory-Turn: 0",
        "AIVory-Provider: claude-code",
        "AIVory-Model: claude-opus-5-5",
    ] {
        assert!(msg.contains(t), "missing trailer {t}");
    }
}

#[test]
fn snapshot_chain_of_three_turns() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    write(d, "a.txt", "one\n");
    let s0 = vcs.snapshot(d, &snap_req("S-1", 0)).unwrap().unwrap();
    assert_eq!(s0.parent, None);
    write(d, "a.txt", "one\ntwo\n");
    write(d, "b.txt", "b\n");
    let s1 = vcs.snapshot(d, &snap_req("S-1", 1)).unwrap().unwrap();
    write(d, "c.txt", "c\n");
    let s2 = vcs.snapshot(d, &snap_req("S-1", 2)).unwrap().unwrap();
    fs::remove_file(d.join("b.txt")).unwrap();
    let s3 = vcs.snapshot(d, &snap_req("S-1", 3)).unwrap().unwrap();

    assert_eq!(s1.parent.as_deref(), Some(s0.commit.as_str()));
    assert_eq!(s2.parent.as_deref(), Some(s1.commit.as_str()));
    assert_eq!(s3.parent.as_deref(), Some(s2.commit.as_str()));
    let list = vcs.list_snapshots(d, "S-1").unwrap();
    assert_eq!(list.iter().map(|s| s.n).collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert_eq!(list[3].commit, s3.commit);
    // A session with a similar name stays separate.
    assert!(vcs.list_snapshots(d, "S-10").unwrap().is_empty());

    let ch = vcs.diff_trees(d, Some(&s0.commit), &s1.commit).unwrap();
    assert_eq!(ch.len(), 2);
    let a = ch.iter().find(|c| c.path == "a.txt").unwrap();
    assert_eq!(
        (a.status, a.added, a.removed),
        (ChangeStatus::Modified, 1, 0)
    );
    let b = ch.iter().find(|c| c.path == "b.txt").unwrap();
    assert_eq!(b.status, ChangeStatus::Added);
    assert!(b.blob_after.is_some());
    let del = vcs.diff_trees(d, Some(&s2.commit), &s3.commit).unwrap();
    assert_eq!(del.len(), 1);
    assert_eq!((del[0].status, del[0].removed), (ChangeStatus::Deleted, 1));
    assert_eq!(del[0].blob_after, None);
    // Without a base, the first snapshot is all additions.
    let all = vcs.diff_trees(d, None, &s0.commit).unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn unchanged_turn_is_skipped() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    write(d, "a.txt", "one\n");
    let s0 = vcs.snapshot(d, &snap_req("S-1", 0)).unwrap().unwrap();
    assert!(vcs.snapshot(d, &snap_req("S-1", 1)).unwrap().is_none());
    assert!(vcs.list_snapshots(d, "S-1").unwrap().len() == 1);
    write(d, "a.txt", "two\n");
    let s2 = vcs.snapshot(d, &snap_req("S-1", 2)).unwrap().unwrap();
    assert_eq!(s2.parent.as_deref(), Some(s0.commit.as_str()));
    // Reuse of a turn number is an error, and a bad session id is refused.
    assert!(vcs.snapshot(d, &snap_req("S-1", 2)).is_err());
    assert!(vcs.snapshot(d, &snap_req("../x", 3)).is_err());
}

// ---- Blame by prompt ----------------------------------------------------------------------

#[test]
fn blame_by_prompt_across_committed_and_uncommitted_lines() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    let db = mem_db();
    let ledger = Ledger::new(&db).unwrap();

    write(d, "a.txt", "base1\nbase2\n");
    let commit_a = commit_all(d, 1000, &["initial"]);
    finish_turn(&vcs, &ledger, d, turn("S-1", 0, "baseline", 1500)).unwrap();
    write(d, "a.txt", "base1\nbase2\nai1\n");
    finish_turn(&vcs, &ledger, d, turn("S-1", 1, "add ai1", 2000)).unwrap();
    write(d, "a.txt", "base1\nbase2\nai1\nai2\n");
    finish_turn(&vcs, &ledger, d, turn("S-1", 2, "add ai2", 3000)).unwrap();
    // The user edits one line by hand, and commits the work with the trailer.
    write(d, "a.txt", "base1\nbase2 by hand\nai1\nai2\n");
    let commit_b = commit_all(d, 3500, &["feature", "AIVory-Turns: S-1/1,S-1/2"]);
    // A later turn stays uncommitted. The user also types a line after the snapshot.
    write(d, "a.txt", "base1\nbase2 by hand\nai1\nai2\nai3\n");
    finish_turn(&vcs, &ledger, d, turn("S-1", 3, "add ai3", 4000)).unwrap();
    write(d, "a.txt", "base1\nbase2 by hand\nai1\nai2\nai3\nhand3\n");

    let lines = blame_by_prompt(&vcs, &ledger, d, "a.txt", None, Some("S-1")).unwrap();
    assert_eq!(lines.len(), 6);
    let l = &lines;
    // base1: old commit, no turn.
    assert_eq!(l[0].commit, commit_a);
    assert!(l[0].turn_ref.is_none() && l[0].commit_turns.is_empty());
    // base2 by hand: commit B names turns, but the line has no exact turn.
    assert_eq!(l[1].commit, commit_b);
    assert_eq!(l[1].commit_turns.len(), 2);
    assert!(l[1].turn_ref.is_none());
    // ai1 and ai2: commit B, exact turn and prompt from the ledger.
    assert_eq!(l[2].commit, commit_b);
    assert_eq!(
        l[2].turn_ref,
        Some(TurnRef {
            session: "S-1".into(),
            n: 1
        })
    );
    let t1 = l[2].turn.as_ref().expect("turn 1 in ledger");
    assert_eq!((t1.prompt.as_str(), t1.cost_usd), ("add ai1", Some(0.25)));
    assert_eq!(t1.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(l[3].turn_ref.as_ref().map(|t| t.n), Some(2));
    assert_eq!(l[3].turn.as_ref().unwrap().prompt, "add ai2");
    // ai3: uncommitted, from the live snapshot chain.
    assert!(l[4].uncommitted);
    assert_eq!(l[4].turn_ref.as_ref().map(|t| t.n), Some(3));
    assert_eq!(l[4].turn.as_ref().unwrap().prompt, "add ai3");
    // hand3: uncommitted and after the last snapshot: no turn.
    assert!(l[5].uncommitted && l[5].turn_ref.is_none());

    // A range returns only those lines.
    let part = blame_by_prompt(&vcs, &ledger, d, "a.txt", Some((3, 4)), Some("S-1")).unwrap();
    assert_eq!(part.iter().map(|p| p.line).collect::<Vec<_>>(), [3, 4]);
    // Without a session, an uncommitted line has no turn.
    let no_session = blame_by_prompt(&vcs, &ledger, d, "a.txt", Some((5, 5)), None).unwrap();
    assert!(no_session[0].uncommitted && no_session[0].turn_ref.is_none());

    // A new file that Git does not know: all lines are uncommitted, and nothing fails.
    write(d, "new.txt", "x\ny\n");
    let new = blame_by_prompt(&vcs, &ledger, d, "new.txt", None, Some("S-1")).unwrap();
    assert_eq!(new.len(), 2);
    assert!(new.iter().all(|n| n.uncommitted && n.turn_ref.is_none()));
}

// ---- Merge --------------------------------------------------------------------------------

#[test]
fn trial_merge_clean_and_conflict() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    write(d, "f.txt", "a\nb\nc\nd\ne\n");
    commit_all(d, 1000, &["base"]);
    git(d, &["checkout", "-q", "-b", "side"]);
    write(d, "f.txt", "a1\nb\nc\nd\ne\n");
    commit_all(d, 2000, &["side edit"]);
    git(d, &["checkout", "-q", "main"]);
    write(d, "f.txt", "a\nb\nc\nd\ne1\n");
    commit_all(d, 3000, &["main edit"]);

    let before = user_state(d, &["f.txt"]);
    let clean = vcs.trial_merge(d, "main", "side").unwrap();
    assert!(clean.clean && clean.conflicts.is_empty());
    let merged = git(d, &["show", &format!("{}:f.txt", clean.tree)]);
    assert_eq!(merged, "a1\nb\nc\nd\ne1");

    // A second branch edits the same line as main: conflict.
    git(d, &["checkout", "-q", "-b", "clash", "HEAD~1"]);
    write(d, "f.txt", "a\nb\nc\nd\ne2\n");
    commit_all(d, 4000, &["clash edit"]);
    git(d, &["checkout", "-q", "main"]);
    let before2 = user_state(d, &["f.txt"]);
    let conflict = vcs.trial_merge(d, "main", "clash").unwrap();
    assert!(!conflict.clean);
    assert_eq!(conflict.conflicts, ["f.txt"]);
    assert!(
        conflict
            .messages
            .iter()
            .any(|m| m.kind.starts_with("CONFLICT")),
        "{:?}",
        conflict.messages
    );
    assert_eq!(before.status, before2.status);
    assert_eq!(
        before2,
        user_state(d, &["f.txt"]),
        "trial merge changed the user state"
    );
    // A bad revision is an error, not a conflict.
    assert!(vcs.trial_merge(d, "main", "no-such-branch").is_err());
}

// ---- Revert -------------------------------------------------------------------------------

#[test]
fn revert_restores_the_snapshot_before_the_turn() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    let db = mem_db();
    let ledger = Ledger::new(&db).unwrap();
    write(d, ".gitignore", "*.log\n");
    write(d, "a.txt", "v0\n");
    write(d, "keep/old.txt", "old\n");
    commit_all(d, 1000, &["base"]);
    let s0 = finish_turn(&vcs, &ledger, d, turn("S-1", 0, "baseline", 1100)).unwrap();
    write(d, "ignored.log", "log\n");

    write(d, "a.txt", "v1\n");
    write(d, "new/deep/b.txt", "b\n");
    fs::remove_file(d.join("keep/old.txt")).unwrap();
    let t1 = finish_turn(&vcs, &ledger, d, turn("S-1", 1, "change", 1200)).unwrap();
    assert_eq!(t1.parent_snapshot, s0.snapshot_ref);
    assert_eq!(t1.files.len(), 3);
    write(d, "a.txt", "v2\n");
    finish_turn(&vcs, &ledger, d, turn("S-1", 2, "change more", 1300)).unwrap();

    // Revert turn 2: back to the state after turn 1.
    revert_turn(&vcs, &ledger, d, "S-1", 2, 2000).unwrap();
    assert_eq!(read(d, "a.txt"), "v1\n");
    assert!(d.join("new/deep/b.txt").exists());
    assert!(!d.join("keep/old.txt").exists());

    // Revert turn 1: back to the baseline. The new folders go away, the old file returns.
    let snap = revert_turn(&vcs, &ledger, d, "S-1", 1, 2100).unwrap();
    assert_eq!(snap.n, 0);
    assert_eq!(read(d, "a.txt"), "v0\n");
    assert_eq!(read(d, "keep/old.txt"), "old\n");
    assert!(!d.join("new").exists());
    assert!(d.join("ignored.log").exists(), "ignored files stay");
    assert!(vcs.diff_workdir(d, &snap.commit).unwrap().is_empty());
    assert_eq!(git(d, &["status", "--porcelain"]), "");

    let ops = ledger.list_ops("S-1").unwrap();
    assert_eq!(ops.len(), 2);
    assert_eq!(
        (ops[0].kind.as_str(), ops[0].turn_n, ops[0].at),
        ("revert", Some(2), 2000)
    );
    // No snapshot exists before the baseline.
    assert!(revert_turn(&vcs, &ledger, d, "S-1", 0, 2200).is_err());
}

// ---- Timeline -----------------------------------------------------------------------------

#[test]
fn timeline_mixes_commits_and_turns_in_time_order() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    let db = mem_db();
    let ledger = Ledger::new(&db).unwrap();
    write(d, "a.txt", "1\n");
    write(d, "other.txt", "o\n");
    commit_all(d, 1000, &["commit A"]);
    finish_turn(&vcs, &ledger, d, turn("S-1", 0, "baseline", 1100)).unwrap();
    write(d, "a.txt", "2\n");
    finish_turn(&vcs, &ledger, d, turn("S-1", 1, "turn one", 2000)).unwrap();
    write(d, "a.txt", "3\n");
    commit_all(d, 3000, &["commit B"]);
    write(d, "a.txt", "4\n");
    finish_turn(&vcs, &ledger, d, turn("S-1", 2, "turn two", 4000)).unwrap();
    // This turn does not touch a.txt.
    write(d, "other.txt", "o2\n");
    finish_turn(&vcs, &ledger, d, turn("S-1", 3, "other file", 4500)).unwrap();
    // A commit at the same second as turn 2: the turn comes first.
    write(d, "a.txt", "5\n");
    commit_all(d, 4000, &["commit C"]);

    let tl = file_timeline(&vcs, &ledger, d, "a.txt", None).unwrap();
    let labels: Vec<String> = tl
        .iter()
        .map(|e| match e {
            TimelineEvent::Commit(c) => c.subject.clone(),
            TimelineEvent::Turn(t) => format!("turn {}", t.n),
        })
        .collect();
    assert_eq!(
        labels,
        ["commit A", "turn 1", "commit B", "turn 2", "commit C"]
    );
    assert!(tl.windows(2).all(|w| w[0].time() <= w[1].time()));
    // The baseline turn 0 lists no file.
    let other = file_timeline(&vcs, &ledger, d, "other.txt", Some("S-1")).unwrap();
    assert!(
        other
            .iter()
            .any(|e| matches!(e, TimelineEvent::Turn(t) if t.n == 3))
    );
}

// ---- Ledger -------------------------------------------------------------------------------

#[test]
fn ledger_round_trip() {
    let db = mem_db();
    let ledger = Ledger::new(&db).unwrap();
    assert_eq!(ledger.schema_version().unwrap(), 1);
    // A second open keeps the data and the version.
    let mut t = turn("S-9", 1, "do it", 500);
    t.task_id = Some("T-4".into());
    t.snapshot_ref = Some("abc".into());
    t.parent_snapshot = Some("def".into());
    t.usage_in = Some(100);
    t.usage_out = Some(50);
    t.tests_passed = Some(7);
    t.tests_failed = Some(1);
    t.diagnostics_added = Some(2);
    t.diagnostics_removed = Some(3);
    t.files = vec![
        aivory_vcs::TurnFile {
            path: "src/a.rs".into(),
            added: 4,
            removed: 1,
            blob_after: Some("b1".into()),
        },
        aivory_vcs::TurnFile {
            path: "gone.rs".into(),
            added: 0,
            removed: 9,
            blob_after: None,
        },
    ];
    t.tools = vec![TurnTool {
        seq: 1,
        tool: "Edit".into(),
        input_digest: Some("d1".into()),
        files: vec!["src/a.rs".into()],
    }];
    let stored = ledger.record_turn(&t).unwrap();
    assert!(stored.id > 0);
    let again = Ledger::new(&db).unwrap();
    let got = again.get_turn("S-9", 1).unwrap().expect("turn");
    assert_eq!(got, stored);
    assert!(again.get_turn("S-9", 2).unwrap().is_none());

    ledger.record_turn(&turn("S-9", 2, "next", 600)).unwrap();
    ledger
        .record_turn(&turn("S-8", 1, "other session", 550))
        .unwrap();
    let list = ledger.list_turns("S-9").unwrap();
    assert_eq!(list.iter().map(|t| t.n).collect::<Vec<_>>(), [1, 2]);
    let by_file = ledger.turns_for_file("src/a.rs", None).unwrap();
    assert_eq!(by_file.len(), 1);
    assert!(
        ledger
            .turns_for_file("src/a.rs", Some("S-8"))
            .unwrap()
            .is_empty()
    );
    // A turn number can be used once per session.
    assert!(ledger.record_turn(&turn("S-9", 2, "again", 700)).is_err());
    // A schema from a newer version is refused.
    db.execute(
        "UPDATE ledger_meta SET value = '99' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    assert!(Ledger::new(&db).is_err());
}

// ---- Publish ------------------------------------------------------------------------------

#[test]
fn publish_builds_a_commit_with_trailers() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    let db = mem_db();
    let ledger = Ledger::new(&db).unwrap();
    write(d, "a.txt", "base\n");
    let base = commit_all(d, 1000, &["base"]);
    finish_turn(&vcs, &ledger, d, turn("S-1", 0, "baseline", 1100)).unwrap();
    write(d, "a.txt", "base\none\n");
    let t1 = finish_turn(&vcs, &ledger, d, turn("S-1", 1, "one", 1200)).unwrap();
    write(d, "a.txt", "base\none\ntwo\n");
    let mut t2 = turn("S-1", 2, "two", 1300);
    t2.provider = "gemini-cli".into();
    t2.model = Some("gemini-3-pro".into());
    let t2 = finish_turn(&vcs, &ledger, d, t2).unwrap();

    let before = user_state(d, &["a.txt"]);
    let req = PublishRequest {
        branch: "aivory/S-1".into(),
        subject: "Add two lines".into(),
        body: Some("Agent text.".into()),
        task_id: Some("T-4".into()),
        turns: vec![t1.clone(), t2.clone()],
        author: Some(("Agent Author".into(), "agent@example.invalid".into())),
        notes: true,
        ..PublishRequest::default()
    };
    let out = vcs.publish_turns(d, &req).unwrap().expect("commit");
    assert_eq!(out.parent.as_deref(), Some(base.as_str()));
    assert_eq!(out.ref_name, "refs/heads/aivory/S-1");
    assert_eq!(git(d, &["rev-parse", "refs/heads/aivory/S-1"]), out.commit);
    let tree_of_snapshot = git(
        d,
        &[
            "rev-parse",
            &format!("{}^{{tree}}", t2.snapshot_ref.clone().unwrap()),
        ],
    );
    assert_eq!(out.tree, tree_of_snapshot);
    let msg = git(d, &["log", "-1", "--format=%B", &out.commit]);
    assert!(msg.starts_with("Add two lines\n\nAgent text."), "{msg}");
    assert!(msg.contains("AIVory-Task: T-4"));
    assert!(msg.contains("AIVory-Turns: S-1/1,S-1/2"));
    assert!(msg.contains("AIVory-Agents: claude-code/claude-opus-5-5, gemini-cli/gemini-3-pro"));
    assert_eq!(
        git(d, &["log", "-1", "--format=%an", &out.commit]),
        "Agent Author"
    );
    let note = git(d, &["notes", "--ref=aivory", "show", &out.commit]);
    assert!(note.contains("S-1/2 gemini-cli gemini-3-pro"));
    // The commit trailer reads back as turns, and the user state is the same.
    let log = vcs
        .log(
            d,
            &LogOptions {
                rev: Some("aivory/S-1".into()),
                ..LogOptions::default()
            },
        )
        .unwrap();
    assert_eq!(log[0].turns.len(), 2);
    let after = user_state(d, &["a.txt"]);
    assert_eq!(
        (
            &before.status,
            &before.head,
            &before.index_bytes,
            &before.files
        ),
        (&after.status, &after.head, &after.index_bytes, &after.files)
    );
    assert!(after.branches.contains("refs/heads/aivory/S-1"));
    // The same content again: nothing to publish.
    assert!(vcs.publish_turns(d, &req).unwrap().is_none());

    // Publish on the checked out branch: the index follows, the status is clean.
    let on_main = PublishRequest {
        branch: "main".into(),
        subject: "Land".into(),
        turns: vec![t1, t2],
        sync_checkout: Some(d.to_path_buf()),
        ..PublishRequest::default()
    };
    assert!(vcs.publish_turns(d, &on_main).unwrap().is_some());
    assert_eq!(git(d, &["status", "--porcelain"]), "");
    assert_eq!(read(d, "a.txt"), "base\none\ntwo\n");
}

// ---- Workspaces ---------------------------------------------------------------------------

#[test]
fn workspace_create_snapshot_and_remove() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    write(d, "src/lib.rs", "fn a() {}\n");
    write(d, "docs/readme.md", "docs\n");
    commit_all(d, 1000, &["base"]);
    let root = tempfile::tempdir().unwrap();

    let ws = vcs
        .create_workspace(
            d,
            &WorkspaceRequest {
                root: root.path().join("workspaces"),
                name: "S-1".into(),
                branch: Some("aivory/S-1".into()),
                create_branch: true,
                start_point: None,
                sparse: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(ws.path, root.path().join("workspaces").join("S-1"));
    assert_eq!(ws.branch.as_deref(), Some("aivory/S-1"));
    assert!(ws.path.join("docs/readme.md").exists());

    // A snapshot inside the workspace leaves the main folder and its branch alone.
    let main_before = user_state(d, &["src/lib.rs"]);
    write(&ws.path, "src/lib.rs", "fn b() {}\n");
    let ws_before = user_state(&ws.path, &["src/lib.rs"]);
    let snap = vcs
        .snapshot(&ws.path, &snap_req("S-1", 1))
        .unwrap()
        .unwrap();
    assert_eq!(ws_before, user_state(&ws.path, &["src/lib.rs"]));
    assert_eq!(main_before, user_state(d, &["src/lib.rs"]));
    // The ref lives in the shared repository.
    assert_eq!(
        git(d, &["rev-parse", "refs/aivory/turns/S-1/1"]),
        snap.commit
    );

    vcs.remove_workspace(d, &ws.path, true).unwrap();
    assert!(!ws.path.exists());

    // A sparse workspace holds only the named directories.
    let sparse = vcs
        .create_workspace(
            d,
            &WorkspaceRequest {
                root: root.path().join("workspaces"),
                name: "S-2".into(),
                branch: None,
                create_branch: false,
                start_point: Some("main".into()),
                sparse: vec!["src".into()],
            },
        )
        .unwrap();
    assert!(sparse.path.join("src/lib.rs").exists());
    assert!(!sparse.path.join("docs").exists());
    assert_eq!(sparse.branch, None);
    assert!(
        vcs.create_workspace(
            d,
            &WorkspaceRequest {
                root: root.path().to_path_buf(),
                name: "../escape".into(),
                branch: None,
                create_branch: false,
                start_point: None,
                sparse: Vec::new(),
            },
        )
        .is_err()
    );
}

// ---- Read and remote ----------------------------------------------------------------------

#[test]
fn read_operations() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    assert!(
        vcs.log(d, &LogOptions::default()).unwrap().is_empty(),
        "no commit yet"
    );
    write(d, "a.txt", "one\n");
    write(d, "b.txt", "bee\n");
    let first = commit_all(d, 1000, &["first", "body text"]);
    write(d, "a.txt", "one\ntwo\n");
    let second = commit_all(d, 2000, &["second"]);

    let log = vcs.log(d, &LogOptions::default()).unwrap();
    assert_eq!(
        log.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [second.as_str(), first.as_str()]
    );
    assert_eq!(
        (log[1].subject.as_str(), log[1].body.as_str()),
        ("first", "body text")
    );
    assert_eq!(
        (log[1].author_name.as_str(), log[1].time),
        ("Test User", 1000)
    );
    assert_eq!(log[0].parents, std::slice::from_ref(&first));
    let b_log = vcs
        .log(
            d,
            &LogOptions {
                path: Some("b.txt".into()),
                ..LogOptions::default()
            },
        )
        .unwrap();
    assert_eq!(b_log.len(), 1);
    let limited = vcs
        .log(
            d,
            &LogOptions {
                max_count: Some(1),
                ..LogOptions::default()
            },
        )
        .unwrap();
    assert_eq!(limited.len(), 1);

    assert_eq!(vcs.show_file(d, &first, "a.txt").unwrap(), b"one\n");
    assert_eq!(vcs.resolve(d, "HEAD").unwrap(), second);
    assert!(vcs.resolve(d, "nope").is_err());
    assert!(
        vcs.resolve(d, "--output=x").is_err(),
        "option-like revision"
    );
    let refs = vcs.refs(d, Some("refs/heads/")).unwrap();
    assert_eq!(
        (refs[0].name.as_str(), refs[0].id.as_str()),
        ("refs/heads/main", second.as_str())
    );

    write(d, "c.txt", "new\n");
    write(d, "a.txt", "changed\n");
    let status = vcs.status(d).unwrap();
    assert!(status.iter().any(|s| s.path == "c.txt" && s.index == '?'));
    assert!(
        status
            .iter()
            .any(|s| s.path == "a.txt" && s.worktree == 'M')
    );
    let wd = vcs.diff_workdir(d, "HEAD").unwrap();
    assert!(
        wd.iter()
            .any(|c| c.path == "c.txt" && c.status == ChangeStatus::Added)
    );
    let patch = vcs
        .diff_patch(d, Some(&first), &second, Some("a.txt"))
        .unwrap();
    assert!(patch.contains("+two"));
    let blame = vcs.blame(d, None, "b.txt", None).unwrap();
    assert_eq!(
        (blame[0].commit.as_str(), blame[0].content.as_str()),
        (first.as_str(), "bee")
    );
    let at_rev = vcs.blame(d, Some(&second), "a.txt", Some((2, 2))).unwrap();
    assert_eq!(
        (at_rev[0].commit.as_str(), at_rev[0].summary.as_str()),
        (second.as_str(), "second")
    );
}

#[test]
fn remote_push_fetch_and_list() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    write(d, "a.txt", "one\n");
    let head = commit_all(d, 1000, &["first"]);
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare", "-b", "main", "."]);
    let url: PathBuf = remote.path().to_path_buf();
    git(d, &["remote", "add", "origin", url.to_str().unwrap()]);

    vcs.push_ref(d, "origin", "refs/heads/main:refs/heads/main")
        .unwrap();
    let remote_refs = vcs.list_remote_refs(d, "origin").unwrap();
    assert!(
        remote_refs
            .iter()
            .any(|r| r.name == "refs/heads/main" && r.id == head)
    );
    vcs.fetch(d, "origin").unwrap();
    assert_eq!(git(d, &["rev-parse", "origin/main"]), head);
    assert!(vcs.push_ref(d, "origin", "--delete").is_err());
}

#[test]
fn the_baseline_of_every_session_has_the_base_commit_as_parent() {
    let repo = init_repo();
    let d = repo.path();
    let vcs = GitVcs::new();
    let db = mem_db();
    let ledger = Ledger::new(&db).unwrap();
    write(d, "a.txt", "line\n");
    let head = commit_all(d, 1000, &["base"]);

    // The baseline exists although the tree equals the base commit.
    let a0 = finish_turn(&vcs, &ledger, d, turn("S-A", 0, "baseline", 1100)).unwrap();
    let b0 = finish_turn(&vcs, &ledger, d, turn("S-B", 0, "baseline", 1100)).unwrap();
    assert!(a0.snapshot_ref.is_some() && b0.snapshot_ref.is_some());
    assert_eq!(a0.parent_snapshot.as_deref(), Some(head.as_str()));
    assert_eq!(b0.parent_snapshot.as_deref(), Some(head.as_str()));

    // Turn 1 builds on the baseline of its own session.
    write(d, "a.txt", "line A\n");
    let a1 = finish_turn(&vcs, &ledger, d, turn("S-A", 1, "edit", 1200)).unwrap();
    assert_eq!(a1.parent_snapshot, a0.snapshot_ref);
    write(d, "a.txt", "line B\n");
    let b1 = finish_turn(&vcs, &ledger, d, turn("S-B", 1, "edit", 1300)).unwrap();
    assert_eq!(b1.parent_snapshot, b0.snapshot_ref);

    // The two sessions share the base commit, so the trial merge runs and finds the conflict.
    let merge = vcs
        .trial_merge(
            d,
            a1.snapshot_ref.as_deref().unwrap(),
            b1.snapshot_ref.as_deref().unwrap(),
        )
        .unwrap();
    assert!(!merge.clean);
    assert_eq!(merge.conflicts, ["a.txt"]);
}
