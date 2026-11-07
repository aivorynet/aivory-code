//! Tests on synthetic repos in temporary folders. No real session or user file is read.

use std::path::Path;
use std::process::Command;

use aivory_index::{ContentId, Index, IndexOptions, MergePolicy, PageReq};
use aivory_syntax::DeclKind;

const INVOICE: &str = "pub struct Invoice {
    pub amount: u64,
}

impl Invoice {
    pub fn total(&self) -> u64 {
        self.amount
    }
}

pub fn make_invoice() -> Invoice {
    Invoice { amount: 1 }
}
";

const INVOICES_API: &str = "use crate::billing::invoice::make_invoice;

pub fn show() -> u64 {
    let inv = make_invoice();
    inv.total()
}
";

const CART: &str = "pub struct Cart;

impl Cart {
    pub fn total(&self) -> u64 {
        0
    }
}
";

const TOOLS_PY: &str = "def make_invoice_text(n):
    return str(n)
";

/// The files of the fixture repo.
const FILES: [(&str, &str); 4] = [
    ("src/billing/invoice.rs", INVOICE),
    ("src/api/invoices.rs", INVOICES_API),
    ("src/cart.rs", CART),
    ("tools/text.py", TOOLS_PY),
];

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn fixture(root: &Path) {
    for (rel, content) in FILES {
        write(root, rel, content);
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_repo(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["checkout", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
}

fn opts() -> IndexOptions {
    IndexOptions::default()
}

fn open() -> (tempfile::TempDir, Index) {
    let dir = tempfile::tempdir().unwrap();
    let index = Index::open(dir.path().join("index")).unwrap();
    (dir, index)
}

fn paths(page: &aivory_index::Page<impl aivory_index::CompactHit>) -> Vec<String> {
    page.items.iter().map(|h| h.path().to_owned()).collect()
}

#[test]
fn same_content_is_parsed_once() {
    let (_keep, index) = open();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    fixture(a.path());
    fixture(b.path());
    let first = index.refresh(a.path(), "a", &opts()).unwrap();
    assert_eq!((first.parsed, first.files), (4, 4));
    // A second checkout with equal files, under another snapshot name: nothing is parsed.
    let second = index.refresh(b.path(), "b", &opts()).unwrap();
    assert_eq!((second.parsed, second.from_store, second.files), (0, 4, 4));
    // An unchanged repo: nothing is written.
    let again = index.refresh(a.path(), "a", &opts()).unwrap();
    assert_eq!(
        (again.parsed, again.unchanged, again.segments_written),
        (0, 4, 0)
    );
    assert_eq!(index.snapshot_names(), ["a", "b"]);
}

#[test]
fn git_blob_ids_branches_and_worktrees() {
    let (_keep, index) = open();
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());
    git_repo(repo.path());
    let first = index.refresh(repo.path(), "main", &opts()).unwrap();
    assert_eq!((first.parsed, first.git_ids), (4, 4));
    let stub = index.file_stub(None, "src/cart.rs").unwrap().unwrap();
    assert_eq!(stub.hash, blake3_hex(CART));
    let entry = index.snapshot("main").unwrap().segments[0]
        .files()
        .iter()
        .find(|f| f.path == "src/cart.rs")
        .cloned()
        .unwrap();
    assert!(matches!(entry.content, ContentId::Git(_)));

    // A worktree at the same commit shares all stubs.
    let wt_parent = tempfile::tempdir().unwrap();
    let wt = wt_parent.path().join("wt");
    git(
        repo.path(),
        &["worktree", "add", "-q", "-b", "other", wt.to_str().unwrap()],
    );
    let in_wt = index.refresh(&wt, "wt", &opts()).unwrap();
    assert_eq!((in_wt.parsed, in_wt.from_store, in_wt.git_ids), (0, 4, 4));

    // A branch with one changed file: fork, refresh, one parse.
    git(repo.path(), &["checkout", "-q", "-b", "feature"]);
    write(
        repo.path(),
        "src/cart.rs",
        &format!("{CART}pub fn extra() {{}}\n"),
    );
    git(repo.path(), &["commit", "-q", "-am", "extra"]);
    index.fork_snapshot("main", "feature").unwrap();
    let feature = index.refresh(repo.path(), "feature", &opts()).unwrap();
    assert_eq!(
        (feature.parsed, feature.unchanged, feature.git_ids),
        (1, 3, 4)
    );

    // An uncommitted change has a blake3 id and is found.
    write(
        repo.path(),
        "src/cart.rs",
        &format!("{CART}pub fn dirty() {{}}\n"),
    );
    let dirty = index.refresh(repo.path(), "feature", &opts()).unwrap();
    assert_eq!((dirty.parsed, dirty.git_ids), (1, 3));
    let hits = index.definition(None, "dirty", PageReq::default()).unwrap();
    assert_eq!(hits.total, 0, "the current snapshot is main");
    index.set_current("feature").unwrap();
    assert_eq!(
        index
            .definition(None, "dirty", PageReq::default())
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        index
            .definition(None, "extra", PageReq::default())
            .unwrap()
            .total,
        0,
        "the committed version of the file is replaced"
    );
}

fn blake3_hex(text: &str) -> String {
    match ContentId::of_bytes(text.as_bytes()) {
        ContentId::Blake3(h) | ContentId::Git(h) => h,
    }
}

#[test]
fn definition_and_references_across_files() {
    let (_keep, index) = open();
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());
    index.refresh(repo.path(), "main", &opts()).unwrap();

    let make = index
        .definition(None, "make_invoice", PageReq::default())
        .unwrap();
    assert_eq!(paths(&make), ["src/billing/invoice.rs"]);
    assert_eq!(make.items[0].kind, DeclKind::Function);
    assert_eq!(make.items[0].line, 11);
    assert!(make.items[0].signature.contains("fn make_invoice"));

    // Two methods are named `total`. The qualified query ranks the exact one first.
    let total = index
        .definition(None, "billing::Invoice::total", PageReq::default())
        .unwrap();
    assert_eq!(total.total, 2);
    assert_eq!(total.items[0].path, "src/billing/invoice.rs");
    assert!(total.items[0].exact && !total.items[1].exact);
    assert_eq!(total.items[0].qualified, "Invoice::total");
    let cart = index
        .definition(None, "Cart::total", PageReq::default())
        .unwrap();
    assert_eq!(cart.items[0].path, "src/cart.rs");
    // Without an exact match the search ignores the case.
    let lower = index
        .definition(None, "MAKE_INVOICE", PageReq::default())
        .unwrap();
    assert_eq!(lower.total, 1);

    // References: the call, with the line text. (An import is not a reference.)
    let refs = index
        .references(None, "make_invoice", PageReq::default())
        .unwrap();
    assert_eq!(paths(&refs), ["src/api/invoices.rs"]);
    assert_eq!(refs.items[0].line, 4);
    assert_eq!(refs.items[0].text, "let inv = make_invoice();");
    let calls = index
        .references(None, "Invoice::total", PageReq::default())
        .unwrap();
    assert_eq!(paths(&calls), ["src/api/invoices.rs"]);
    assert_eq!(calls.items[0].text, "inv.total()");

    // The compact text for an agent.
    let text = refs.render("references(sym: \"make_invoice\")");
    assert!(
        text.starts_with("references(sym: \"make_invoice\")  →  1 result\n"),
        "{text}"
    );
    assert!(
        text.contains("src/api/invoices.rs:4   let inv = make_invoice();"),
        "{text}"
    );

    // Paging.
    let page2 = index
        .references(None, "Invoice", PageReq::new(2, 1))
        .unwrap();
    assert!(page2.total >= 2, "{page2:?}");
    assert_eq!((page2.items.len(), page2.page), (1, 2));
    assert_eq!(page2.remaining(), page2.total - 2);
    assert!(
        index
            .references(None, "make_invoice", PageReq::new(2, 1))
            .unwrap()
            .items
            .is_empty()
    );

    // Outline and stub.
    let outline = index.outline(None, "src/cart.rs").unwrap().unwrap();
    assert!(outline.iter().any(|i| i.name == "Cart"));
    assert!(index.file_stub(None, "nope.rs").unwrap().is_none());
}

#[test]
fn symbol_search_ranks_exact_prefix_fuzzy() {
    let (_keep, index) = open();
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());
    write(
        repo.path(),
        "src/more.rs",
        "pub fn invoice() {}\npub fn invoice_list() {}\npub fn open_invoice_now() {}\n",
    );
    index.refresh(repo.path(), "main", &opts()).unwrap();

    let all = index
        .symbols(None, "invoice", None, PageReq::default())
        .unwrap();
    let names: Vec<&str> = all.items.iter().map(|h| h.name.as_str()).collect();
    // Exact (case-insensitive) first: `invoice` and `Invoice` (struct and impl), then prefix
    // matches, then the fuzzy `open_invoice_now` and `make_invoice`.
    assert!(names[..3].iter().all(|n| n.eq_ignore_ascii_case("invoice")));
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(pos("invoice_list") < pos("make_invoice"));
    assert!(pos("Invoice") < pos("invoice_list"));
    assert!(names.contains(&"open_invoice_now"));
    assert!(!names.contains(&"total"));

    // Subsequence: `mkinv` finds `make_invoice`.
    let fuzzy = index
        .symbols(None, "mkinv", None, PageReq::default())
        .unwrap();
    assert!(fuzzy.items.iter().any(|h| h.name == "make_invoice"));
    // Kind filter.
    let structs = index
        .symbols(None, "invoice", Some(DeclKind::Struct), PageReq::default())
        .unwrap();
    assert!(structs.items.iter().all(|h| h.kind == DeclKind::Struct));
    assert_eq!(structs.total, 1);
    // A method has its container.
    let total = index
        .symbols(None, "total", Some(DeclKind::Method), PageReq::default())
        .unwrap();
    assert_eq!(total.total, 2);
    assert!(
        total
            .items
            .iter()
            .any(|h| h.container.as_deref() == Some("Cart"))
    );
    assert!(
        index
            .symbols(None, "  ", None, PageReq::default())
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn session_delta_overrides_base_and_tombstone_hides() {
    let (_keep, index) = open();
    let base = tempfile::tempdir().unwrap();
    fixture(base.path());
    index.refresh(base.path(), "main", &opts()).unwrap();

    // The session works in its own worktree: it renames `Cart` to `Basket`, adds a file, and
    // deletes the Python file.
    let wt = tempfile::tempdir().unwrap();
    fixture(wt.path());
    write(wt.path(), "src/cart.rs", &CART.replace("Cart", "Basket"));
    write(
        wt.path(),
        "src/new.rs",
        "pub fn fresh() { make_invoice(); }\n",
    );
    std::fs::remove_file(wt.path().join("tools/text.py")).unwrap();
    let report = index
        .session_update(
            "s1",
            None,
            wt.path(),
            &["src/cart.rs".into(), "src/new.rs".into()],
            &["tools/text.py".into()],
        )
        .unwrap();
    assert_eq!((report.parsed, report.files, report.tombstones), (2, 2, 1));

    let def = |s: Option<&str>, q: &str| index.definition(s, q, PageReq::default()).unwrap();
    // The base is unchanged for a query without a session.
    assert_eq!(def(None, "Cart").total, 2); // struct and impl
    assert_eq!(def(None, "Basket").total, 0);
    assert_eq!(def(None, "fresh").total, 0);
    assert_eq!(def(None, "make_invoice_text").total, 1);
    // The session sees its own files.
    assert_eq!(
        def(Some("s1"), "Cart").total,
        0,
        "the delta overrides the base file"
    );
    assert_eq!(def(Some("s1"), "Basket").total, 2);
    assert_eq!(def(Some("s1"), "fresh").total, 1);
    // The tombstone hides the removed file; other files stay.
    assert_eq!(def(Some("s1"), "make_invoice_text").total, 0);
    assert!(
        index
            .file_stub(Some("s1"), "tools/text.py")
            .unwrap()
            .is_none()
    );
    assert!(index.file_stub(None, "tools/text.py").unwrap().is_some());
    assert_eq!(def(Some("s1"), "make_invoice").total, 1);
    // An unknown session reads the base.
    assert_eq!(def(Some("nobody"), "Cart").total, 2);

    // References merge the base and the delta: the call in the new file shows up.
    let refs = index
        .references(Some("s1"), "make_invoice", PageReq::default())
        .unwrap();
    assert_eq!(paths(&refs), ["src/api/invoices.rs", "src/new.rs"]);
    assert_eq!(refs.items[1].text, "pub fn fresh() { make_invoice(); }");
    assert_eq!(
        index
            .references(None, "make_invoice", PageReq::default())
            .unwrap()
            .total,
        1
    );

    // A second update keeps the first one; an undone change falls back to the base.
    std::fs::write(wt.path().join("src/cart.rs"), CART).unwrap();
    index
        .session_update("s1", None, wt.path(), &["src/cart.rs".into()], &[])
        .unwrap();
    assert_eq!(def(Some("s1"), "Cart").total, 2);
    assert_eq!(def(Some("s1"), "fresh").total, 1);
    assert_eq!(index.session_paths("s1").unwrap().1, ["tools/text.py"]);

    // The radar: who uses `make_invoice` in a delta?
    let other = tempfile::tempdir().unwrap();
    fixture(other.path());
    write(
        other.path(),
        "src/api/invoices.rs",
        "pub fn show() { make_invoice(); }\n",
    );
    index
        .session_update(
            "s2",
            None,
            other.path(),
            &["src/api/invoices.rs".into()],
            &[],
        )
        .unwrap();
    let uses = index.sessions_touching("make_invoice").unwrap();
    assert_eq!(
        uses.iter().map(|u| u.session.as_str()).collect::<Vec<_>>(),
        ["s1", "s2"]
    );
    assert_eq!(uses[1].references.len(), 1);
    assert!(
        index
            .sessions_touching("Cart")
            .unwrap()
            .iter()
            .all(|u| u.session == "s1")
    );

    // After the session ends, its delta is gone.
    assert!(index.session_drop("s1"));
    assert_eq!(def(Some("s1"), "fresh").total, 0);
    assert_eq!(index.sessions(), ["s2"]);
}

#[test]
fn incremental_update_removal_and_tombstone_in_snapshot() {
    let (_keep, index) = open();
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());
    index.refresh(repo.path(), "main", &opts()).unwrap();
    assert_eq!(index.view(None).unwrap().file_count(), 4);

    // update_files: one changed, one removed, one new.
    write(repo.path(), "src/cart.rs", "pub struct Basket;\n");
    std::fs::remove_file(repo.path().join("tools/text.py")).unwrap();
    write(repo.path(), "src/new.rs", "pub fn fresh() {}\n");
    let paths_changed: Vec<String> = ["src/cart.rs", "tools/text.py", "src/new.rs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let report = index
        .update_files(repo.path(), "main", &paths_changed, &opts())
        .unwrap();
    assert_eq!((report.parsed, report.removed), (2, 1));
    assert_eq!(index.view(None).unwrap().file_count(), 4);
    let def = |q: &str| index.definition(None, q, PageReq::default()).unwrap().total;
    assert_eq!(
        (
            def("Cart"),
            def("Basket"),
            def("fresh"),
            def("make_invoice_text")
        ),
        (0, 1, 1, 0)
    );
    assert!(index.file_stub(None, "tools/text.py").unwrap().is_none());

    // refresh: a deleted file gets a tombstone; unchanged files are not parsed again.
    std::fs::remove_file(repo.path().join("src/new.rs")).unwrap();
    let refresh = index.refresh(repo.path(), "main", &opts()).unwrap();
    assert_eq!(
        (refresh.parsed, refresh.removed, refresh.unchanged),
        (0, 1, 3)
    );
    assert_eq!(def("fresh"), 0);

    // Merge: the same answers with fewer segments, and no tombstones left.
    let before = index.snapshot("main").unwrap().segments.len();
    assert!(before >= 3);
    let merged = index
        .merge(
            "main",
            &MergePolicy {
                max_segments: 1,
                max_merged_files: 1000,
            },
        )
        .unwrap();
    assert_eq!((merged.segments_before, merged.segments_after), (before, 1));
    assert_eq!(
        (
            def("Cart"),
            def("Basket"),
            def("fresh"),
            def("make_invoice")
        ),
        (0, 1, 0, 1)
    );
    assert!(index.file_stub(None, "tools/text.py").unwrap().is_none());
    assert_eq!(index.view(None).unwrap().file_count(), 3);
    let seg = &index.snapshot("main").unwrap().segments[0];
    assert!(seg.tombstones().is_empty());
    // Old segment files go away with gc.
    assert!(index.gc().unwrap() >= before);
    assert_eq!(index.stats().segments, 1);
}

#[test]
fn background_merge_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());
    {
        let index = Index::open(dir.path().join("index")).unwrap();
        let small = IndexOptions {
            batch_files: 1,
            max_segments: 100,
            ..IndexOptions::default()
        };
        index.refresh(repo.path(), "feature/x", &small).unwrap();
        assert_eq!(index.snapshot("feature/x").unwrap().segments.len(), 4);
        let handle = index.spawn_merge(
            "feature/x",
            MergePolicy {
                max_segments: 2,
                max_merged_files: 100,
            },
        );
        let report = handle.join().unwrap().unwrap();
        assert_eq!(report.segments_after, 2);
        assert_eq!(index.current().as_deref(), Some("feature/x"));
    }
    // A new process opens the same folder.
    let index = Index::open(dir.path().join("index")).unwrap();
    assert_eq!(index.snapshot_names(), ["feature/x"]);
    assert_eq!(index.current().as_deref(), Some("feature/x"));
    assert_eq!(
        index
            .definition(None, "make_invoice", PageReq::default())
            .unwrap()
            .total,
        1
    );
    let refs = index
        .references(None, "make_invoice", PageReq::default())
        .unwrap();
    assert_eq!(refs.items[0].text, "let inv = make_invoice();");
    assert!(index.set_current("nope").is_err());
    index.delete_snapshot("feature/x").unwrap();
    assert!(index.snapshot_names().is_empty());
    assert_eq!(
        index
            .definition(None, "make_invoice", PageReq::default())
            .unwrap()
            .total,
        0
    );
}

#[test]
fn gitignore_and_limits() {
    let (_keep, index) = open();
    let repo = tempfile::tempdir().unwrap();
    fixture(repo.path());
    write(repo.path(), ".gitignore", "target/\n*.gen.rs\n");
    write(repo.path(), "target/debug/junk.rs", "fn junk() {}\n");
    write(repo.path(), "src/a.gen.rs", "fn generated() {}\n");
    write(repo.path(), ".git/hooks/x.rs", "fn hook() {}\n");
    write(
        repo.path(),
        "src/big.rs",
        &format!("fn big() {{}}\n{}", "// pad\n".repeat(1000)),
    );
    std::fs::write(repo.path().join("src/binary.rs"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
    write(repo.path(), "README.md", "# no language\n");
    let report = index
        .refresh(
            repo.path(),
            "main",
            &IndexOptions {
                max_file_bytes: 4000,
                ..IndexOptions::default()
            },
        )
        .unwrap();
    // The four fixture files; `big.rs` and `binary.rs` are skipped.
    assert_eq!((report.files, report.skipped), (4, 2));
    let def = |q: &str| index.definition(None, q, PageReq::default()).unwrap().total;
    assert_eq!(
        (def("junk"), def("generated"), def("hook"), def("big")),
        (0, 0, 0, 0)
    );
}
