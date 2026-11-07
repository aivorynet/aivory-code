//! Conflict radar tests with temporary git repositories and real snapshot refs.

use std::fs;
use std::path::Path;
use std::process::Command;

use aivory_coord::{
    ClaimState, Coord, ManualClock, NoRefs, RadarEnv, RadarHit, TIER_DEPENDENCY, TIER_PATH,
    TIER_SYMBOL, TurnDelta, delta_from_snapshots,
};
use aivory_index::{Index, IndexOptions};
use aivory_vcs::{GitVcs, SnapshotRequest, Vcs};
use rusqlite::Connection;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

struct World {
    repo: TempDir,
    vcs: GitVcs,
    coord: Coord,
}

impl World {
    fn new(files: &[(&str, &str)]) -> Self {
        let repo = tempfile::tempdir().unwrap();
        let d = repo.path();
        git(d, &["init", "-q", "-b", "main", "."]);
        git(d, &["config", "user.name", "Test User"]);
        git(d, &["config", "user.email", "test@example.invalid"]);
        git(d, &["config", "core.autocrlf", "false"]);
        git(d, &["config", "commit.gpgsign", "false"]);
        for (p, c) in files {
            write(d, p, c);
        }
        git(d, &["add", "-A"]);
        git(d, &["commit", "-q", "-m", "base"]);
        let coord = Coord::with_clock(
            Connection::open_in_memory().unwrap(),
            ManualClock::new(10_000_000),
        )
        .unwrap();
        Self {
            repo,
            vcs: GitVcs::new(),
            coord,
        }
    }

    fn path(&self) -> &Path {
        self.repo.path()
    }

    /// Apply the edits, take the turn-1 snapshot of the session (its parent is the base
    /// commit), build the delta from the snapshots, restore the working tree, and return the
    /// delta. `during` runs while the edits are on disk.
    fn edit(&self, session: &str, edits: &[(&str, &str)], during: impl FnOnce(&Path)) -> TurnDelta {
        let d = self.path();
        for (p, c) in edits {
            write(d, p, c);
        }
        let snap = self
            .vcs
            .snapshot(
                d,
                &SnapshotRequest {
                    session: session.to_owned(),
                    n: 1,
                    provider: "test".to_owned(),
                    model: "test".to_owned(),
                    parent: Some("HEAD".to_owned()),
                    ..SnapshotRequest::default()
                },
            )
            .unwrap()
            .expect("snapshot");
        assert_eq!(snap.ref_name, format!("refs/aivory/turns/{session}/1"));
        let delta = delta_from_snapshots(&self.vcs, d, session, 1)
            .unwrap()
            .expect("delta");
        during(d);
        git(d, &["checkout", "-q", "--", "."]);
        delta
    }

    fn env<'a>(&'a self, refs: &'a dyn aivory_coord::RefSource) -> RadarEnv<'a> {
        RadarEnv {
            vcs: &self.vcs,
            repo: self.path(),
            refs,
        }
    }
}

const SHARED: &str = "\
pub fn top() -> i64 {
    1
}

pub fn mid() -> i64 {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    a + b + c + d
}

pub fn bottom() -> i64 {
    3
}
";

fn hit_for<'a>(hits: &'a [RadarHit], a: &str, b: &str) -> &'a RadarHit {
    hits.iter()
        .find(|h| h.a == a && h.b == b)
        .unwrap_or_else(|| panic!("no hit for {a} {b}: {hits:?}"))
}

#[test]
fn tier1_same_path_clean_merge_scores_low() {
    let w = World::new(&[("src/shared.rs", SHARED)]);
    let a = w.edit(
        "S-A",
        &[("src/shared.rs", &SHARED.replace("    1\n", "    10\n"))],
        |_| {},
    );
    let b = w.edit(
        "S-B",
        &[("src/shared.rs", &SHARED.replace("    3\n}", "    30\n}"))],
        |_| {},
    );
    assert_eq!(a.changed_symbols.len(), 1, "{:?}", a.changed_symbols);
    let env = w.env(&NoRefs);
    assert!(
        w.coord
            .record_delta(&a, Some(&env))
            .unwrap()
            .hits
            .is_empty()
    );
    let out = w.coord.record_delta(&b, Some(&env)).unwrap();
    assert_eq!(out.hits.len(), 1, "{:?}", out.hits);
    let h = hit_for(&out.hits, "S-A", "S-B");
    assert_eq!(h.tier, TIER_PATH);
    assert_eq!(h.keys, ["path:src/shared.rs"]);
    assert!((h.score - 0.1).abs() < 1e-6, "{h:?}");
    assert!(h.trial && h.conflict_files.is_empty(), "{h:?}");

    // The radar sent an FYI ping to both sessions.
    let ta = w.coord.take_for_turn("S-A").unwrap().expect("ping to A");
    let tb = w.coord.take_for_turn("S-B").unwrap().expect("ping to B");
    assert!(ta.contains("from S-B") && ta.contains("tier 1"), "{ta}");
    assert!(tb.contains("from S-A") && tb.contains("risk 0.1"), "{tb}");
}

#[test]
fn tier2_same_symbol_clean_merge_scores_medium() {
    let w = World::new(&[("src/shared.rs", SHARED)]);
    let a = w.edit(
        "S-A",
        &[("src/shared.rs", &SHARED.replace("let a = 1", "let a = 10"))],
        |_| {},
    );
    let b = w.edit(
        "S-B",
        &[("src/shared.rs", &SHARED.replace("let d = 4", "let d = 40"))],
        |_| {},
    );
    let env = w.env(&NoRefs);
    w.coord.record_delta(&a, Some(&env)).unwrap();
    let out = w.coord.record_delta(&b, Some(&env)).unwrap();
    let h = hit_for(&out.hits, "S-A", "S-B");
    assert_eq!(h.tier, TIER_SYMBOL);
    assert!(h.keys.contains(&"sym:rust:mid".to_owned()), "{h:?}");
    assert!((h.score - 0.4).abs() < 1e-6, "{h:?}");
    assert!(h.trial && h.conflict_files.is_empty());
}

#[test]
fn tier2_after_a_rename() {
    let w = World::new(&[("src/shared.rs", SHARED)]);
    // A renames `mid`; B changes the body of `mid`. The old name stays claimed.
    let a = w.edit(
        "S-A",
        &[("src/shared.rs", &SHARED.replace("fn mid()", "fn middle()"))],
        |_| {},
    );
    let b = w.edit(
        "S-B",
        &[("src/shared.rs", &SHARED.replace("let d = 4", "let d = 40"))],
        |_| {},
    );
    let env = w.env(&NoRefs);
    w.coord.record_delta(&a, Some(&env)).unwrap();
    let out = w.coord.record_delta(&b, Some(&env)).unwrap();
    let h = hit_for(&out.hits, "S-A", "S-B");
    assert_eq!(h.tier, TIER_SYMBOL, "{h:?}");
    assert!((h.score - 0.4).abs() < 1e-6, "{h:?}");
}

#[test]
fn text_conflict_scores_one_and_the_cache_answers_twice() {
    let w = World::new(&[("src/shared.rs", SHARED)]);
    let a = w.edit(
        "S-A",
        &[("src/shared.rs", &SHARED.replace("let b = 2", "let b = 20"))],
        |_| {},
    );
    let b = w.edit(
        "S-B",
        &[("src/shared.rs", &SHARED.replace("let b = 2", "let b = 21"))],
        |_| {},
    );
    let env = w.env(&NoRefs);
    w.coord.record_delta(&a, Some(&env)).unwrap();
    let out = w.coord.record_delta(&b, Some(&env)).unwrap();
    let h = hit_for(&out.hits, "S-A", "S-B");
    assert!((h.score - 1.0).abs() < 1e-6, "{h:?}");
    assert_eq!(h.conflict_files, ["src/shared.rs"]);
    assert!(h.tier >= TIER_SYMBOL);
    assert!(h.trial_error.is_none());
    let stats = w.coord.merge_stats();
    assert_eq!(stats.runs, 1);

    // The same pair of commits: the cache answers. The ping text names the conflict.
    let again = w.coord.radar("S-A", &env).unwrap();
    assert_eq!(again[0].conflict_files, ["src/shared.rs"]);
    let stats = w.coord.merge_stats();
    assert_eq!((stats.runs, stats.cache_hits), (1, 1));
    let text = w.coord.take_for_turn("S-A").unwrap().unwrap();
    assert!(text.contains("text conflict in src/shared.rs"), "{text}");
}

#[test]
fn unmatched_pairs_get_no_trial_merge() {
    let w = World::new(&[("src/one.rs", "fn a() {}\n"), ("src/two.rs", "fn b() {}\n")]);
    let a = w.edit("S-A", &[("src/one.rs", "fn a() { 1; }\n")], |_| {});
    let b = w.edit("S-B", &[("src/two.rs", "fn b() { 2; }\n")], |_| {});
    let env = w.env(&NoRefs);
    w.coord.record_delta(&a, Some(&env)).unwrap();
    let out = w.coord.record_delta(&b, Some(&env)).unwrap();
    assert!(out.hits.is_empty(), "{:?}", out.hits);
    assert_eq!(w.coord.merge_stats().runs, 0);
    assert!(w.coord.take_for_turn("S-A").unwrap().is_none());
}

#[test]
fn tier3_signature_change_used_by_the_other_session() {
    let money = "pub fn price(x: i64) -> i64 {\n    x\n}\n";
    let cart = "pub fn total() -> i64 {\n    price(1)\n}\n\npub fn other() -> i64 {\n    7\n}\n";
    let w = World::new(&[("src/money.rs", money), ("src/cart.rs", cart)]);

    // The index of the base commit.
    let index_dir = tempfile::tempdir().unwrap();
    let index = Index::open(index_dir.path()).unwrap();
    index
        .refresh(w.path(), "main", &IndexOptions::default())
        .unwrap();

    // S-B edits `other` in cart.rs. Its delta in the index holds cart.rs.
    let b = w.edit(
        "S-B",
        &[("src/cart.rs", &cart.replace("    7\n", "    8\n"))],
        |root| {
            index
                .session_update("S-B", None, root, &["src/cart.rs".to_owned()], &[])
                .unwrap();
        },
    );
    // S-A changes the signature of `price`.
    let a = w.edit(
        "S-A",
        &[(
            "src/money.rs",
            "pub fn price(x: i64, rate: i64) -> i64 {\n    x\n}\n",
        )],
        |_| {},
    );

    let env = w.env(&index);
    assert!(
        w.coord
            .record_delta(&b, Some(&env))
            .unwrap()
            .hits
            .is_empty()
    );
    let out = w.coord.record_delta(&a, Some(&env)).unwrap();
    let h = hit_for(&out.hits, "S-A", "S-B");
    assert_eq!(h.tier, TIER_DEPENDENCY, "{h:?}");
    assert_eq!(h.keys, ["sym:rust:price"]);
    assert!((h.score - 0.7).abs() < 1e-6, "{h:?}");
    assert!(h.trial && h.conflict_files.is_empty());

    // Without the index references, the same pair does not match at all.
    let none = w.coord.radar("S-A", &w.env(&NoRefs)).unwrap();
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn a_session_without_a_snapshot_scores_by_tier() {
    let w = World::new(&[("src/shared.rs", SHARED)]);
    let a = w.edit(
        "S-A",
        &[("src/shared.rs", &SHARED.replace("    1\n", "    10\n"))],
        |_| {},
    );
    let env = w.env(&NoRefs);
    w.coord.record_delta(&a, Some(&env)).unwrap();
    // S-C claims the file by hand and has no snapshot.
    let out = w
        .coord
        .claim(
            "S-C",
            &["path:src/shared.rs".to_owned()],
            ClaimState::Editing,
            Some(&env),
        )
        .unwrap();
    let h = hit_for(&out.hits, "S-A", "S-C");
    assert_eq!(h.tier, TIER_PATH);
    assert!(!h.trial);
    assert!(h.trial_error.as_deref().unwrap().contains("S-C"), "{h:?}");
    assert!((h.score - 0.1).abs() < 1e-6);
}
