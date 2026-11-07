//! Semantic delta tests on synthetic source text.

use aivory_coord::{
    Change, ChangedSymbol, FileVersions, TestState, TurnDelta, compute_delta, keys, line_counts,
};

fn one(path: &str, before: Option<&str>, after: Option<&str>) -> TurnDelta {
    compute_delta(
        "S-1",
        1,
        Some("refs/aivory/turns/S-1/1"),
        &[FileVersions {
            path: path.to_owned(),
            before: before.map(str::to_owned),
            after: after.map(str::to_owned),
            lines: None,
        }],
    )
}

fn find<'a>(d: &'a TurnDelta, key: &str) -> Option<&'a ChangedSymbol> {
    d.changed_symbols.iter().find(|s| s.key == key)
}

const BEFORE: &str = "\
pub struct Money(i64);

impl Money {
    pub fn add(&self, other: &Money) -> Money {
        Money(self.0 + other.0)
    }

    pub fn old_name(&self, scale: i64) -> i64 {
        self.0 * scale
    }

    pub fn keep(&self) -> i64 {
        self.0
    }

    pub fn tweak(&self) -> i64 {
        self.0
    }
}

pub fn price(x: i64) -> i64 {
    x
}

pub fn gone() {}
";

const AFTER: &str = "\
pub struct Money(i64);

impl Money {
    pub fn add(&self, other: &Money) -> Money {
        Money(self.0 + other.0)
    }

    pub fn new_name(&self, scale: i64) -> i64 {
        self.0 * scale
    }

    pub fn keep(&self) -> i64 {
        self.0
    }

    pub fn tweak(&self) -> i64 {
        self.0 + 1
    }
}

pub fn price(x: i64, rate: i64) -> i64 {
    x
}

pub fn fresh(flag: bool) {}

#[cfg(test)]
mod tests {
    #[test]
    fn total_rounds() {}
}
";

#[test]
fn rename_signature_body_added_removed_and_test() {
    let d = one("src/billing/money.rs", Some(BEFORE), Some(AFTER));

    let renamed = find(&d, "sym:rust:Money::new_name").expect("rename");
    assert_eq!(renamed.change, Change::Renamed);
    assert_eq!(renamed.old_key.as_deref(), Some("sym:rust:Money::old_name"));
    assert!(find(&d, "sym:rust:Money::old_name").is_none());

    assert_eq!(
        find(&d, "sym:rust:price").map(|s| s.change),
        Some(Change::Signature)
    );
    assert_eq!(
        find(&d, "sym:rust:Money::tweak").map(|s| s.change),
        Some(Change::Body)
    );
    assert_eq!(
        find(&d, "sym:rust:gone").map(|s| s.change),
        Some(Change::Removed)
    );
    assert_eq!(
        find(&d, "sym:rust:fresh").map(|s| s.change),
        Some(Change::Added)
    );
    // Unchanged symbols and the container of a changed member give no entry.
    assert!(find(&d, "sym:rust:Money::keep").is_none());
    assert!(find(&d, "sym:rust:Money::add").is_none());
    assert!(find(&d, "sym:rust:Money").is_none());

    assert_eq!(d.tests.len(), 1, "{:?}", d.tests);
    assert_eq!(d.tests[0].key, "test:tests::total_rounds");
    assert_eq!(d.tests[0].state, TestState::Added);

    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].key, "path:src/billing/money.rs");
    assert!(d.files[0].lines_added > 0 && d.files[0].lines_removed > 0);
    assert_eq!(d.wip_ref.as_deref(), Some("refs/aivory/turns/S-1/1"));
}

#[test]
fn body_range_and_white_space() {
    let before = "fn a() {\n    1\n}\n";
    // Only white space changes: no delta entry, but the file counts as changed.
    let d = one("a.rs", Some(before), Some("fn a() {\n  1\n\n}\n"));
    assert!(d.changed_symbols.is_empty(), "{:?}", d.changed_symbols);
    assert_eq!(d.files.len(), 1);

    let d = one("a.rs", Some(before), Some("fn a() {\n    2\n}\n"));
    let s = find(&d, "sym:rust:a").expect("body change");
    assert_eq!(s.change, Change::Body);
    assert_eq!(s.base_range.map(|r| (r.start, r.end)), Some((0, 2)));
}

#[test]
fn removed_test_and_other_languages() {
    let before = "def test_one():\n    pass\n\ndef helper(a):\n    pass\n";
    let after = "def helper(a, b):\n    pass\n";
    let d = one("tests/test_money.py", Some(before), Some(after));
    assert_eq!(d.tests.len(), 1);
    assert_eq!(d.tests[0].state, TestState::Removed);
    assert_eq!(d.tests[0].key, "test:test_one");
    assert_eq!(
        find(&d, "sym:python:helper").map(|s| s.change),
        Some(Change::Signature)
    );

    // A deleted file removes all of its symbols; a new file adds them.
    let d = one("x.go", Some("package x\n\nfunc TestA() {}\n"), None);
    assert_eq!(
        find(&d, "sym:go:TestA").map(|s| s.change),
        Some(Change::Removed)
    );
    assert_eq!(d.tests[0].state, TestState::Removed);
    let d = one("x.go", None, Some("package x\n\nfunc TestA() {}\n"));
    assert_eq!(d.tests[0].state, TestState::Added);
}

#[test]
fn unknown_language_and_identical_text() {
    let d = one("notes.xyz", Some("a\n"), Some("a\nb\n"));
    assert!(d.changed_symbols.is_empty());
    assert_eq!(d.files[0].lines_added, 1);
    assert_eq!(d.files[0].lines_removed, 0);
    let d = one("a.rs", Some("fn a() {}\n"), Some("fn a() {}\n"));
    assert!(d.files.is_empty());
}

#[test]
fn routing_keys_and_serde() {
    let d = one("src/billing/money.rs", Some(BEFORE), Some(AFTER));
    let k = d.routing_keys();
    assert!(k.contains(&"path:src/billing/money.rs".to_owned()));
    assert!(k.contains(&"mod:src/billing".to_owned()));
    assert!(k.contains(&"sym:rust:price".to_owned()));
    assert!(k.contains(&"sym:rust:Money::old_name".to_owned()));
    assert!(k.iter().all(|x| keys::kind_of(x).is_some()));
    let json = serde_json::to_string(&d).unwrap();
    assert!(json.contains("\"change\":\"signature\""));
    assert_eq!(serde_json::from_str::<TurnDelta>(&json).unwrap(), d);
}

#[test]
fn line_count_diff() {
    assert_eq!(line_counts(Some("a\nb\nc\n"), Some("a\nx\nc\n")), (1, 1));
    assert_eq!(line_counts(None, Some("a\nb\n")), (2, 0));
    assert_eq!(line_counts(Some("a\nb\n"), None), (0, 2));
    assert_eq!(line_counts(Some("a\nb\nc\nd\n"), Some("b\nd\ne\n")), (1, 2));
}

#[test]
fn key_helpers() {
    assert_eq!(keys::path(".\\src\\a.rs"), "path:src/a.rs");
    assert_eq!(keys::module_of("a.rs"), "mod:.");
    assert_eq!(keys::module_of("src/a/b.rs"), "mod:src/a");
    assert!(keys::matches("sym:*:Money", "sym:rust:Money"));
    assert!(!keys::matches("sym:*:Money", "sym:rust:Cash"));
    assert!(keys::matches("mod:src/billing", "path:src/billing/a.rs"));
    assert!(!keys::matches("mod:src/bill", "path:src/billing/a.rs"));
    assert_eq!(keys::last_name("Money::add"), "add");
}
