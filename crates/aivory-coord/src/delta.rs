//! The semantic delta of a turn.
//!
//! [`compute_delta`] compares the stubs of the changed files before and after a turn and lists
//! the symbols that changed. [`delta_from_snapshots`] reads the two file versions from the
//! turn snapshots of `aivory-vcs`.
//!
//! Rules for one symbol (a declaration):
//!
//! - Two declarations with the same kind and qualified name are the same symbol. When a name
//!   occurs more than once (overloads), the pairs with the same signature match first.
//! - Different signature text (white space aside): [`Change::Signature`].
//! - Same signature, different body: [`Change::Body`]. The body hash covers the declaration
//!   text without the member declarations, with white space folded. A container does not
//!   change when only a member changes.
//! - A symbol only in the new stub: [`Change::Added`]. Only in the old stub: [`Change::Removed`].
//! - A removed and an added symbol with the same kind, container, and signature shape (the
//!   signature with the name replaced) form one [`Change::Renamed`].
//! - A function or a method that is a test (see [`is_test`]) also gives a [`TestDelta`].
//!
//! Line numbers in ranges start at 0, like the ranges of `aivory-syntax`.

use std::collections::BTreeMap;
use std::path::Path;

use aivory_syntax::{DeclKind, Declaration, Language, Stub};
use aivory_vcs::{ChangeStatus, Vcs};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::keys;

/// What changed in a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// The symbol is new.
    Added,
    /// The symbol is gone.
    Removed,
    /// The symbol has a new name; the old key is in `old_key`.
    Renamed,
    /// The signature changed (the riskiest kind).
    Signature,
    /// The body changed, the signature did not.
    Body,
}

impl Change {
    /// True when a caller of the symbol can break (signature, removal, rename).
    pub fn breaks_callers(self) -> bool {
        matches!(self, Self::Signature | Self::Removed | Self::Renamed)
    }
}

/// A line range, both ends inclusive, lines from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    /// First line.
    pub start: u32,
    /// Last line.
    pub end: u32,
}

impl LineRange {
    /// True when the two ranges share a line.
    pub fn overlaps(&self, other: &LineRange) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// One changed symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedSymbol {
    /// The symbol key (`sym:<language>:<qualified name>`). The new key after a rename.
    pub key: String,
    /// What changed.
    pub change: Change,
    /// The key before a rename.
    pub old_key: Option<String>,
    /// The file of the symbol.
    pub path: String,
    /// The lines of the declaration in the old version. `None` for an added symbol.
    pub base_range: Option<LineRange>,
    /// The lines of the declaration in the new version. `None` for a removed symbol.
    pub new_range: Option<LineRange>,
}

/// The state of a test in a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestState {
    /// The turn added the test.
    Added,
    /// The turn removed the test.
    Removed,
    /// The test fails after the turn. The daemon sets it from a test run; [`compute_delta`]
    /// never sets it.
    Failing,
}

/// A test in a delta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestDelta {
    /// The test key (`test:<qualified name>`).
    pub key: String,
    /// The state.
    pub state: TestState,
}

/// The line counts of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDelta {
    /// The path key (`path:<file>`).
    pub key: String,
    /// Added lines.
    pub lines_added: u64,
    /// Removed lines.
    pub lines_removed: u64,
}

/// The semantic delta of one turn. It is the plaintext of the `turn.delta` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDelta {
    /// The session id.
    pub session: String,
    /// The turn number.
    pub turn: u32,
    /// The changed symbols.
    pub changed_symbols: Vec<ChangedSymbol>,
    /// The tests that were added, removed, or fail.
    pub tests: Vec<TestDelta>,
    /// The changed files.
    pub files: Vec<FileDelta>,
    /// The ref of the turn snapshot (`refs/aivory/turns/<session>/<n>`), when there is one.
    pub wip_ref: Option<String>,
}

impl TurnDelta {
    /// All routing keys of the delta: path, module, symbol, and test keys, without duplicates.
    /// The transport maps them with a [`keys::KeyMapper`].
    pub fn routing_keys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |k: String| {
            if !out.contains(&k) {
                out.push(k);
            }
        };
        for f in &self.files {
            push(f.key.clone());
            if let Some(p) = f.key.strip_prefix("path:") {
                push(keys::module_of(p));
            }
        }
        for s in &self.changed_symbols {
            push(s.key.clone());
            if let Some(o) = &s.old_key {
                push(o.clone());
            }
        }
        for t in &self.tests {
            push(t.key.clone());
        }
        out
    }
}

/// The old and the new text of one file. `None` means that the file does not exist on that side.
#[derive(Debug, Clone, Default)]
pub struct FileVersions {
    /// The path, relative to the repo root.
    pub path: String,
    /// The text before the turn.
    pub before: Option<String>,
    /// The text after the turn.
    pub after: Option<String>,
    /// The line counts (added, removed), when the caller knows them from a diff. `None` makes
    /// [`compute_delta`] count them.
    pub lines: Option<(u64, u64)>,
}

/// Build the delta of a turn from the old and the new text of the changed files. A file with
/// no known language, or with a parse error, appears in `files` but gives no symbols.
pub fn compute_delta(
    session: &str,
    turn: u32,
    wip_ref: Option<&str>,
    files: &[FileVersions],
) -> TurnDelta {
    let mut delta = TurnDelta {
        session: session.to_owned(),
        turn,
        changed_symbols: Vec::new(),
        tests: Vec::new(),
        files: Vec::new(),
        wip_ref: wip_ref.map(str::to_owned),
    };
    for f in files {
        if f.before == f.after {
            continue;
        }
        let path = f.path.replace('\\', "/");
        let (lines_added, lines_removed) = f
            .lines
            .unwrap_or_else(|| line_counts(f.before.as_deref(), f.after.as_deref()));
        delta.files.push(FileDelta {
            key: keys::path(&path),
            lines_added,
            lines_removed,
        });
        let Some(lang) = Language::detect(&path) else {
            continue;
        };
        // A missing side is an empty symbol list. A parse error makes the file give no symbols.
        let side = |text: &Option<String>| -> Option<Vec<Sym>> {
            match text {
                None => Some(Vec::new()),
                Some(t) => Stub::parse(lang, t)
                    .ok()
                    .map(|stub| symbols(lang, &stub, t)),
            }
        };
        let (Some(b_syms), Some(a_syms)) = (side(&f.before), side(&f.after)) else {
            continue;
        };
        diff_symbols(lang, &path, &b_syms, &a_syms, &mut delta);
    }
    delta
}

/// Build the delta of turn `turn` of `session` from the snapshots of `aivory-vcs`. The old
/// version is the parent of the turn snapshot: the previous snapshot of the session, or the
/// base commit that the first snapshot named as its parent. A snapshot without a parent (the
/// baseline of turn 0) counts against the empty tree. Returns `None` when the session has no
/// snapshot for the turn (the turn changed nothing).
pub fn delta_from_snapshots(
    vcs: &dyn Vcs,
    repo: &Path,
    session: &str,
    turn: u32,
) -> Result<Option<TurnDelta>> {
    let snaps = vcs.list_snapshots(repo, session)?;
    let Some(current) = snaps.iter().find(|s| s.n == turn) else {
        return Ok(None);
    };
    let previous = current.parent.as_deref();
    let changes = vcs.diff_trees(repo, previous, &current.commit)?;
    let mut files = Vec::new();
    for c in changes.iter().filter(|c| !c.binary) {
        let before = match previous {
            Some(p) if c.status != ChangeStatus::Added => read_text(vcs, repo, p, &c.path),
            _ => None,
        };
        let after = if c.status == ChangeStatus::Deleted {
            None
        } else {
            read_text(vcs, repo, &current.commit, &c.path)
        };
        files.push(FileVersions {
            path: c.path.clone(),
            before,
            after,
            lines: Some((c.added, c.removed)),
        });
    }
    Ok(Some(compute_delta(
        session,
        turn,
        Some(&current.ref_name),
        &files,
    )))
}

fn read_text(vcs: &dyn Vcs, repo: &Path, rev: &str, path: &str) -> Option<String> {
    String::from_utf8(vcs.show_file(repo, rev, path).ok()?).ok()
}

// ---------------------------------------------------------------------------------------------
// Symbols

/// One declaration with the data that the comparison needs.
struct Sym {
    name: String,
    kind: DeclKind,
    qualified: String,
    container: String,
    signature: String,
    shape: String,
    body_hash: String,
    lines: LineRange,
    is_test: bool,
}

fn symbols(lang: Language, stub: &Stub, text: &str) -> Vec<Sym> {
    let decls = &stub.declarations;
    let qualified: Vec<String> = (0..decls.len()).map(|i| qualified_name(decls, i)).collect();
    decls
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let container = d
                .parent
                .and_then(|p| qualified.get(p as usize))
                .cloned()
                .unwrap_or_default();
            let signature = fold_ws(&d.signature);
            Sym {
                name: d.name.clone(),
                kind: d.kind,
                qualified: qualified[i].clone(),
                container,
                shape: shape(&signature, &d.name),
                signature,
                body_hash: body_hash(decls, i, text),
                lines: LineRange {
                    start: d.range.start_line,
                    end: d.range.end_line,
                },
                is_test: is_test(lang, d, text),
            }
        })
        .collect()
}

/// The names of the enclosing declarations and the name, joined with `::`.
fn qualified_name(decls: &[Declaration], index: usize) -> String {
    let mut names = Vec::new();
    let mut at = Some(index);
    while let Some(i) = at {
        let Some(d) = decls.get(i) else { break };
        names.push(d.name.as_str());
        at = d.parent.map(|p| p as usize).filter(|p| *p < i);
    }
    names.reverse();
    names.join("::")
}

fn fold_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The signature with each whole-word occurrence of the name replaced by `_`.
fn shape(signature: &str, name: &str) -> String {
    let mut out = String::with_capacity(signature.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            out.push_str(if word == name { "_" } else { word });
            word.clear();
        }
    };
    for c in signature.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The hash of the declaration text without its member declarations, white space folded.
fn body_hash(decls: &[Declaration], index: usize, text: &str) -> String {
    let d = &decls[index];
    let (start, end) = (d.range.start_byte as usize, d.range.end_byte as usize);
    let mut holes: Vec<(usize, usize)> = decls
        .iter()
        .filter(|c| c.parent == Some(index as u32))
        .map(|c| (c.range.start_byte as usize, c.range.end_byte as usize))
        .collect();
    holes.sort_unstable();
    let mut folded = String::new();
    let mut piece = |from: usize, to: usize| {
        if let Some(s) = text.get(from..to) {
            folded.push_str(&fold_ws(s));
            folded.push(' ');
        }
    };
    let mut at = start;
    for (hs, he) in holes {
        if hs >= at && he <= end {
            piece(at, hs);
            at = he;
        }
    }
    piece(at, end);
    blake3::hash(folded.as_bytes()).to_hex().to_string()
}

/// True when a declaration is a test of its language.
///
/// - Rust: a function with a `#[test]` style attribute (`#[test]`, `#[tokio::test]`, ...).
/// - Python: a function or method named `test_*`.
/// - Go: a function named `Test*`.
/// - Java, Kotlin, C#, PHP: a method with `@Test`, `[Test]`, `[Fact]`, `[Theory]`, or
///   `[TestMethod]`; PHP also a method named `test*`.
///
/// Other languages give no tests.
pub fn is_test(lang: Language, d: &Declaration, text: &str) -> bool {
    if !matches!(d.kind, DeclKind::Function | DeclKind::Method) {
        return false;
    }
    let head = header_text(d, text);
    let name = d.name.as_str();
    match lang {
        Language::Rust => head.lines().any(|l| {
            let l = l.trim();
            l.starts_with("#[") && (l.contains("test]") || l.contains("test("))
        }),
        Language::Python => name.starts_with("test_"),
        Language::Go => name.starts_with("Test"),
        Language::Java | Language::Kotlin => ["@Test", "@ParameterizedTest", "@RepeatedTest"]
            .iter()
            .any(|a| head.contains(a)),
        Language::CSharp => ["[Test]", "[Fact]", "[Theory]", "[TestMethod]", "[Test("]
            .iter()
            .any(|a| head.contains(a)),
        Language::Php => name.starts_with("test") || head.contains("@test"),
        _ => false,
    }
}

/// The attribute and comment lines above a declaration, then the text up to its name.
fn header_text(d: &Declaration, text: &str) -> String {
    let start = d.range.start_byte as usize;
    let mut above: Vec<&str> = Vec::new();
    let before = text.get(..start).unwrap_or("");
    // The text before `start` ends inside the line of the declaration; skip that partial line.
    let mut lines = before.rsplit('\n');
    lines.next();
    for l in lines.take(8) {
        let t = l.trim();
        if t.starts_with("#[") || t.starts_with('@') || t.starts_with('[') || t.starts_with("//") {
            above.push(l);
        } else {
            break;
        }
    }
    above.reverse();
    let name_start = (d.name_range.start_byte as usize).max(start);
    let own = text.get(start..name_start).unwrap_or("");
    format!("{}\n{}", above.join("\n"), own)
}

fn diff_symbols(lang: Language, path: &str, before: &[Sym], after: &[Sym], out: &mut TurnDelta) {
    let key_of = |s: &Sym| keys::sym(lang.name(), &s.qualified);
    let mut matched_after = vec![false; after.len()];
    let mut matched_before = vec![false; before.len()];

    // Group by (qualified name, kind). The debug name of the kind orders the groups.
    type Group = (Vec<usize>, Vec<usize>);
    let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
    for (i, s) in before.iter().enumerate() {
        groups
            .entry((s.qualified.clone(), format!("{:?}", s.kind)))
            .or_default()
            .0
            .push(i);
    }
    for (i, s) in after.iter().enumerate() {
        groups
            .entry((s.qualified.clone(), format!("{:?}", s.kind)))
            .or_default()
            .1
            .push(i);
    }
    // Pair inside a group. Equal signatures pair first, then the rest in order.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (bs, as_) in groups.values() {
        let mut b_left: Vec<usize> = Vec::new();
        let mut a_left: Vec<usize> = as_.clone();
        for &b in bs {
            if let Some(pos) = a_left
                .iter()
                .position(|&a| after[a].signature == before[b].signature)
            {
                pairs.push((b, a_left.remove(pos)));
            } else {
                b_left.push(b);
            }
        }
        pairs.extend(b_left.into_iter().zip(a_left));
    }
    pairs.sort_unstable();
    for (b, a) in pairs {
        matched_before[b] = true;
        matched_after[a] = true;
        let (sb, sa) = (&before[b], &after[a]);
        let change = if sb.signature != sa.signature {
            Change::Signature
        } else if sb.body_hash != sa.body_hash {
            Change::Body
        } else {
            continue;
        };
        out.changed_symbols.push(ChangedSymbol {
            key: key_of(sa),
            change,
            old_key: None,
            path: path.to_owned(),
            base_range: Some(sb.lines),
            new_range: Some(sa.lines),
        });
    }

    // Rename: an unmatched old symbol and an unmatched new one with the same shape.
    for (b, sb) in before.iter().enumerate() {
        if matched_before[b] {
            continue;
        }
        let found = after.iter().enumerate().position(|(a, sa)| {
            !matched_after[a]
                && sa.kind == sb.kind
                && sa.container == sb.container
                && sa.shape == sb.shape
                && sa.name != sb.name
        });
        if let Some(a) = found {
            matched_before[b] = true;
            matched_after[a] = true;
            out.changed_symbols.push(ChangedSymbol {
                key: key_of(&after[a]),
                change: Change::Renamed,
                old_key: Some(key_of(sb)),
                path: path.to_owned(),
                base_range: Some(sb.lines),
                new_range: Some(after[a].lines),
            });
        }
    }
    for (b, s) in before.iter().enumerate() {
        if !matched_before[b] {
            out.changed_symbols.push(ChangedSymbol {
                key: key_of(s),
                change: Change::Removed,
                old_key: None,
                path: path.to_owned(),
                base_range: Some(s.lines),
                new_range: None,
            });
        }
    }
    for (a, s) in after.iter().enumerate() {
        if !matched_after[a] {
            out.changed_symbols.push(ChangedSymbol {
                key: key_of(s),
                change: Change::Added,
                old_key: None,
                path: path.to_owned(),
                base_range: None,
                new_range: Some(s.lines),
            });
        }
    }

    // Tests: the test keys present on one side only.
    let tests = |syms: &[Sym]| -> Vec<String> {
        syms.iter()
            .filter(|s| s.is_test)
            .map(|s| keys::test(&s.qualified))
            .collect()
    };
    let (tb, ta) = (tests(before), tests(after));
    for k in ta.iter().filter(|k| !tb.contains(k)) {
        out.tests.push(TestDelta {
            key: k.clone(),
            state: TestState::Added,
        });
    }
    for k in tb.iter().filter(|k| !ta.contains(k)) {
        out.tests.push(TestDelta {
            key: k.clone(),
            state: TestState::Removed,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Line counts

/// Count the added and removed lines between two texts (a longest common subsequence of lines).
/// A text pair with more than 4 million line pairs after trimming gets an upper bound.
pub fn line_counts(before: Option<&str>, after: Option<&str>) -> (u64, u64) {
    let b: Vec<&str> = before.map_or_else(Vec::new, |t| t.lines().collect());
    let a: Vec<&str> = after.map_or_else(Vec::new, |t| t.lines().collect());
    let prefix = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
    let (b, a) = (&b[prefix..], &a[prefix..]);
    let suffix = b
        .iter()
        .rev()
        .zip(a.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (b, a) = (&b[..b.len() - suffix], &a[..a.len() - suffix]);
    if b.is_empty() || a.is_empty() || b.len().saturating_mul(a.len()) > 4_000_000 {
        return (a.len() as u64, b.len() as u64);
    }
    let mut prev = vec![0u32; a.len() + 1];
    let mut cur = vec![0u32; a.len() + 1];
    for x in b {
        for (j, y) in a.iter().enumerate() {
            cur[j + 1] = if x == y {
                prev[j] + 1
            } else {
                prev[j + 1].max(cur[j])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let lcs = u64::from(prev[a.len()]);
    (a.len() as u64 - lcs, b.len() as u64 - lcs)
}
