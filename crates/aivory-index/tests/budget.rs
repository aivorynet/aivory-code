//! Time and memory budgets from `docs/design/03-code-intelligence.md`. Run them in release
//! mode: `cargo test --release -p aivory-index -- --ignored --nocapture`.
//!
//! Budgets: a 100,000-file repo indexes in 3 minutes cold (so 20,000 files in 36 seconds),
//! and navigation from the index takes at most 30 ms.

use std::path::Path;
use std::time::{Duration, Instant};

use aivory_index::{Index, IndexOptions, PageReq};

const FILES: usize = 20_000;
const PER_DIR: usize = 100;

/// A synthetic Rust file. File `n` calls functions of the files `n + 1` and `n + 7`, and it
/// has the common names `new`, `total`, and `Item`.
fn synthetic(n: usize) -> String {
    format!(
        "//! Module {n}.
use crate::m{a}::f{a}_run;

/// Item number {n}.
pub struct Item{n} {{
    pub a: u32,
    pub b: String,
}}

impl Item{n} {{
    pub fn new(a: u32) -> Self {{
        let b = format!(\"item {{}}\", a);
        Self {{ a, b }}
    }}

    pub fn total(&self, other: &Item{n}) -> u32 {{
        self.a + other.a * 2 // doubled
    }}
}}

pub fn f{n}_run(x: u32) -> u32 {{
    let item = Item{n}::new(x);
    let next = f{a}_run(item.a) + f{b}_run(x);
    item.total(&item) + next
}}

pub fn f{n}_helper(items: &[Item{n}]) -> u32 {{
    items.iter().map(|i| i.a).sum()
}}
",
        a = (n + 1) % FILES,
        b = (n + 7) % FILES,
    )
}

fn make_repo(root: &Path) {
    for n in 0..FILES {
        let dir = root.join(format!("d{}", n / PER_DIR));
        if n % PER_DIR == 0 {
            std::fs::create_dir_all(&dir).unwrap();
        }
        std::fs::write(dir.join(format!("m{n}.rs")), synthetic(n)).unwrap();
    }
}

/// The peak working set of this process in KiB (best effort).
fn peak_memory_kib() -> Option<u64> {
    #[cfg(windows)]
    {
        let script = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok()
            .map(|b| b / 1024)
    }
    #[cfg(not(windows))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> Duration {
    median(
        (0..runs)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(f());
                t.elapsed()
            })
            .collect(),
    )
}

#[test]
#[ignore = "time budget; run in release mode"]
fn cold_index_20k_files() {
    let work = tempfile::tempdir().unwrap();
    let repo = work.path().join("repo");
    let started = Instant::now();
    make_repo(&repo);
    let bytes: u64 = (0..FILES)
        .step_by(97)
        .map(|n| synthetic(n).len() as u64)
        .sum::<u64>()
        * 97;
    eprintln!(
        "synthetic repo: {FILES} files, about {} MiB, made in {:?}",
        bytes / (1024 * 1024),
        started.elapsed()
    );

    let index = Index::open(work.path().join("index")).unwrap();
    let opts = IndexOptions::default();

    // Cold: nothing in the stub store, nothing in the index.
    let cold = index.refresh(&repo, "cold", &opts).unwrap();
    let stats = index.stats();
    eprintln!(
        "cold index:        {:?} ({} parsed, {} segments, {} MiB on disk, {} MiB resident maps)",
        cold.elapsed,
        cold.parsed,
        stats.segments,
        stats.segment_bytes / (1024 * 1024),
        stats.resident_bytes / (1024 * 1024)
    );
    assert_eq!((cold.parsed, cold.files), (FILES, FILES));
    assert!(
        cold.elapsed < Duration::from_secs(36),
        "cold index: {:?}",
        cold.elapsed
    );
    if let Some(kib) = peak_memory_kib() {
        eprintln!("peak memory after the cold index: {} MiB", kib / 1024);
    }

    // The stub store is warm: a second snapshot of the same files parses nothing.
    let warm = index.refresh(&repo, "warm", &opts).unwrap();
    eprintln!(
        "warm store index:  {:?} ({} from store)",
        warm.elapsed, warm.from_store
    );
    assert_eq!((warm.parsed, warm.from_store), (0, FILES));

    // Nothing changed: the refresh compares and writes nothing.
    let same = index.refresh(&repo, "warm", &opts).unwrap();
    eprintln!("refresh, no change: {:?}", same.elapsed);
    assert_eq!((same.unchanged, same.segments_written), (FILES, 0));

    // Ten changed files: only they are parsed.
    for n in 0..10 {
        let path = repo.join(format!("d0/m{n}.rs"));
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!("pub fn added_{n}() {{}}\n"));
        std::fs::write(path, text).unwrap();
    }
    let inc = index.refresh(&repo, "warm", &opts).unwrap();
    eprintln!(
        "refresh, 10 edits:  {:?} ({} parsed)",
        inc.elapsed, inc.parsed
    );
    assert_eq!(inc.parsed, 10);

    // Queries on the snapshot.
    index.set_current("warm").unwrap();
    let page = PageReq::default();
    let def = time(21, || index.definition(None, "f1234_run", page).unwrap());
    let refs = time(21, || index.references(None, "f1234_run", page).unwrap());
    let common = time(21, || {
        index
            .references(None, "total", PageReq::new(1, 20))
            .unwrap()
    });
    let sym = time(21, || index.symbols(None, "f12", None, page).unwrap());
    let fuzzy = time(21, || index.symbols(None, "i7r", None, page).unwrap());
    let outline = time(21, || index.outline(None, "d12/m1234.rs").unwrap());
    eprintln!("definition:         {def:?}");
    eprintln!("references:         {refs:?} (a rare name)");
    eprintln!("references:         {common:?} (a name in all {FILES} files)");
    eprintln!("symbols (prefix):   {sym:?}");
    eprintln!("symbols (fuzzy):    {fuzzy:?}");
    eprintln!("outline:            {outline:?}");
    assert_eq!(index.definition(None, "f1234_run", page).unwrap().total, 1);
    assert_eq!(index.references(None, "f1234_run", page).unwrap().total, 2);
    for (name, t) in [
        ("definition", def),
        ("references", refs),
        ("symbols", sym),
        ("outline", outline),
    ] {
        assert!(t < Duration::from_millis(30), "{name}: {t:?}");
    }

    // A session delta of 100 files.
    let changed: Vec<String> = (100..200).map(|n| format!("d1/m{n}.rs")).collect();
    let t = Instant::now();
    let report = index
        .session_update("s", None, &repo, &changed, &[])
        .unwrap();
    eprintln!(
        "session delta, 100 files: {:?} ({} files)",
        t.elapsed(),
        report.files
    );
    let in_session = time(21, || {
        index.references(Some("s"), "f1234_run", page).unwrap()
    });
    eprintln!("references in a session: {in_session:?}");

    if let Some(kib) = peak_memory_kib() {
        eprintln!("peak memory at the end: {} MiB", kib / 1024);
    }
}
