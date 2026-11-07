//! Queries over a stack of segments: definition, references, symbols, outline, file stub.
//!
//! A [`View`] is the list of segments that one query reads, the newest first: the session delta
//! (when the query has a session), then the segments of the snapshot. A path that a newer
//! segment lists, or removes with a tombstone, hides the same path in all older segments.
//!
//! Results are compact: a path, a line from 1, and the line text. A page holds 20 results by
//! default. [`Page::render`] writes the format of the agent tools:
//!
//! ```text
//! references(sym: "Invoice::total")  →  12 results
//!   src/billing/invoice.rs:88   pub fn total(&self) -> Money {
//!   src/api/invoices.rs:41      let t = inv.total();
//!   ... (10 more, use page=2)
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aivory_syntax::{DeclKind, Language, OutlineItem, Stub, Visibility};
use fst::automaton::{Automaton, Str, Subsequence};
use fst::{IntoStreamer, Streamer};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::segment::Segment;

/// The longest line text in a result, in characters.
const MAX_TEXT: usize = 160;
/// The most names that a symbol search reads per segment.
const SYMBOL_CAP: usize = 2_000;
/// A symbol search reads at least this many results, also for page 1.
const SYMBOL_MIN: usize = 100;

/// Which page of a result list to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageReq {
    /// The page number from 1.
    pub page: usize,
    pub per_page: usize,
}

impl Default for PageReq {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: 20,
        }
    }
}

impl PageReq {
    pub fn new(page: usize, per_page: usize) -> Self {
        Self {
            page: page.max(1),
            per_page: per_page.max(1),
        }
    }

    fn range(&self, total: usize) -> std::ops::Range<usize> {
        let start = ((self.page.max(1) - 1) * self.per_page.max(1)).min(total);
        start..(start + self.per_page.max(1)).min(total)
    }
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    /// The number of results on all pages.
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
    pub items: Vec<T>,
    /// True when `total` is a lower bound: the search stopped after it had enough results for
    /// this page (symbol search).
    pub truncated: bool,
}

impl<T> Page<T> {
    fn empty(req: PageReq) -> Self {
        Self {
            total: 0,
            page: req.page.max(1),
            per_page: req.per_page.max(1),
            items: Vec::new(),
            truncated: false,
        }
    }

    /// The number of results after this page.
    pub fn remaining(&self) -> usize {
        self.total
            .saturating_sub((self.page.max(1) - 1) * self.per_page + self.items.len())
    }
}

/// A result that has a place and a line of text.
pub trait CompactHit {
    fn path(&self) -> &str;
    /// The line from 1.
    fn line(&self) -> u32;
    fn text(&self) -> &str;
}

impl<T: CompactHit> Page<T> {
    /// The compact text for an agent: a title line, one line per result, and a hint for the
    /// next page.
    pub fn render(&self, title: &str) -> String {
        let places: Vec<String> = self
            .items
            .iter()
            .map(|h| format!("{}:{}", h.path(), h.line()))
            .collect();
        let width = places.iter().map(String::len).max().unwrap_or(0);
        let mut out = format!(
            "{title}  →  {}{} result{}\n",
            self.total,
            if self.truncated { "+" } else { "" },
            if self.total == 1 && !self.truncated {
                ""
            } else {
                "s"
            }
        );
        for (place, hit) in places.iter().zip(&self.items) {
            out.push_str(&format!("  {place:<width$}   {}\n", hit.text()));
        }
        let rest = self.remaining();
        if rest > 0 {
            out.push_str(&format!(
                "  ... ({rest} more, use page={})\n",
                self.page.max(1) + 1
            ));
        }
        out
    }
}

/// A place where a name is defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefHit {
    pub path: String,
    /// The line of the name, from 1.
    pub line: u32,
    /// The byte column of the name, from 1.
    pub col: u32,
    pub name: String,
    pub kind: DeclKind,
    pub visibility: Visibility,
    /// The names of the enclosing declarations and the name, for example `Invoice::total`.
    pub qualified: String,
    /// The declaration header on one line.
    pub signature: String,
    /// True when the hit matches all qualifiers of the query (always true for a bare name).
    pub exact: bool,
}

impl CompactHit for DefHit {
    fn path(&self) -> &str {
        &self.path
    }
    fn line(&self) -> u32 {
        self.line
    }
    fn text(&self) -> &str {
        &self.signature
    }
}

/// A place where a name is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefHit {
    pub path: String,
    /// The line, from 1.
    pub line: u32,
    /// The byte column, from 1.
    pub col: u32,
    pub start_byte: u32,
    pub end_byte: u32,
    /// The text of the line, trimmed. Empty when the file is not readable or has changed.
    pub text: String,
}

impl CompactHit for RefHit {
    fn path(&self) -> &str {
        &self.path
    }
    fn line(&self) -> u32 {
        self.line
    }
    fn text(&self) -> &str {
        &self.text
    }
}

/// A result of the symbol search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolHit {
    pub path: String,
    pub line: u32,
    pub name: String,
    pub kind: DeclKind,
    pub container: Option<String>,
    pub signature: String,
}

impl CompactHit for SymbolHit {
    fn path(&self) -> &str {
        &self.path
    }
    fn line(&self) -> u32 {
        self.line
    }
    fn text(&self) -> &str {
        &self.signature
    }
}

/// One segment in a view, with the folder that gives the text of reference lines.
#[derive(Debug, Clone)]
pub struct Layer {
    pub segment: Arc<Segment>,
    pub root: Option<PathBuf>,
}

/// The segments that a query reads, newest first.
#[derive(Debug, Clone, Default)]
pub struct View {
    layers: Vec<Layer>,
}

/// Split a qualified name at `::`, `.`, `#`, and `\`.
fn split_qualified(query: &str) -> Vec<&str> {
    query
        .split([':', '.', '#', '\\'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

fn separator(language: Language) -> &'static str {
    match language {
        Language::Rust | Language::Cpp | Language::Php | Language::Ruby | Language::C => "::",
        _ => ".",
    }
}

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_TEXT {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(MAX_TEXT - 3).collect();
    cut.push_str("...");
    cut
}

impl View {
    /// A view of layers, newest first.
    pub fn new(layers: Vec<Layer>) -> Self {
        Self { layers }
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// True when no newer layer than `index` hides the path.
    fn is_live(&self, index: usize, path: &str) -> bool {
        !self.layers[..index]
            .iter()
            .any(|l| l.segment.find_file(path).is_some() || l.segment.has_tombstone(path))
    }

    /// The number of live files.
    pub fn file_count(&self) -> usize {
        self.layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                l.segment
                    .files()
                    .iter()
                    .filter(|f| self.is_live(i, &f.path))
                    .count()
            })
            .sum()
    }

    /// The live stub of a file.
    pub fn file_stub(&self, path: &str) -> Result<Option<Arc<Stub>>> {
        for layer in &self.layers {
            if let Some(id) = layer.segment.find_file(path) {
                return layer.segment.stub(id).map(Some);
            }
            if layer.segment.has_tombstone(path) {
                return Ok(None);
            }
        }
        Ok(None)
    }

    /// The structure view of a file.
    pub fn outline(&self, path: &str) -> Result<Option<Vec<OutlineItem>>> {
        Ok(self.file_stub(path)?.map(|s| s.outline.clone()))
    }

    /// The paths of all live files, sorted.
    pub fn paths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .layers
            .iter()
            .enumerate()
            .flat_map(|(i, l)| {
                l.segment
                    .files()
                    .iter()
                    .filter(move |f| self.is_live(i, &f.path))
                    .map(|f| f.path.clone())
            })
            .collect();
        out.sort();
        out
    }

    /// Where a name is defined. A qualified query such as `billing::Invoice::total` finds the
    /// declarations named `total` and ranks them by how many of the qualifiers match the
    /// enclosing declarations or the path of the file. When no declaration has the exact
    /// name, the search ignores the case.
    pub fn definition(&self, query: &str, req: PageReq) -> Result<Page<DefHit>> {
        let parts = split_qualified(query);
        let Some((name, quals)) = parts.split_last() else {
            return Ok(Page::empty(req));
        };
        let mut hits = self.definitions(name, quals, false)?;
        if hits.is_empty() {
            hits = self.definitions(&name.to_lowercase(), quals, true)?;
        }
        hits.sort_by(|a, b| {
            let rank = |h: &DefHit| (!h.exact, h.visibility != Visibility::Public);
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line))
        });
        let total = hits.len();
        let range = req.range(total);
        Ok(Page {
            total,
            page: req.page.max(1),
            per_page: req.per_page.max(1),
            items: hits[range].to_vec(),
            truncated: false,
        })
    }

    fn definitions(&self, name: &str, quals: &[&str], lower: bool) -> Result<Vec<DefHit>> {
        let mut hits = Vec::new();
        for (i, layer) in self.layers.iter().enumerate() {
            let seg = &layer.segment;
            for p in seg.decl_postings(lower, name)? {
                let path = &seg.files()[p.file as usize].path;
                if !self.is_live(i, path) {
                    continue;
                }
                let stub = seg.stub(p.file)?;
                let Some(decl) = stub.declarations.get(p.decl as usize) else {
                    continue;
                };
                let mut chain: Vec<&str> = Vec::new();
                let mut parent = decl.parent;
                while let Some(n) = parent {
                    let Some(up) = stub.declarations.get(n as usize) else {
                        break;
                    };
                    chain.push(up.name.as_str());
                    parent = up.parent;
                }
                let exact = qualifiers_match(quals, &chain, path);
                let sep = separator(stub.language);
                let mut qualified = chain.iter().rev().copied().collect::<Vec<_>>().join(sep);
                if !qualified.is_empty() {
                    qualified.push_str(sep);
                }
                qualified.push_str(&decl.name);
                hits.push(DefHit {
                    path: path.clone(),
                    line: decl.name_range.start_line + 1,
                    col: decl.name_range.start_col + 1,
                    name: decl.name.clone(),
                    kind: decl.kind,
                    visibility: decl.visibility,
                    qualified,
                    signature: clip(&decl.signature),
                    exact,
                });
            }
        }
        Ok(hits)
    }

    /// Where a name is used. A qualified name uses its last part. The result lists the
    /// references of the stubs (identifiers that are not declaration names); it does not
    /// resolve them. The text of each line on the page comes from the file in the folder of
    /// the layer.
    pub fn references(&self, name: &str, req: PageReq) -> Result<Page<RefHit>> {
        let parts = split_qualified(name);
        let Some(name) = parts.last() else {
            return Ok(Page::empty(req));
        };
        // (path, start byte, layer, line, col, end byte)
        let mut found: Vec<(&str, u32, usize, u32, u32, u32)> = Vec::new();
        // 0: not known, 1: live, 2: hidden by a newer layer
        let mut live: Vec<Vec<u8>> = self
            .layers
            .iter()
            .map(|l| vec![0; l.segment.files().len()])
            .collect();
        for (i, layer) in self.layers.iter().enumerate() {
            let seg = &layer.segment;
            for p in seg.ref_postings(name)? {
                let path = seg.files()[p.file as usize].path.as_str();
                let state = &mut live[i][p.file as usize];
                if *state == 0 {
                    *state = if self.is_live(i, path) { 1 } else { 2 };
                }
                if *state == 1 {
                    found.push((path, p.start_byte, i, p.line, p.col, p.end_byte));
                }
            }
        }
        found.sort_unstable();
        let total = found.len();
        let range = req.range(total);
        let mut files: HashMap<(usize, String), Option<Vec<u8>>> = HashMap::new();
        let items = found[range]
            .iter()
            .map(|(path, start, layer, line, col, end)| {
                let bytes = files
                    .entry((*layer, path.to_string()))
                    .or_insert_with(|| {
                        let root = self.layers[*layer].root.as_ref()?;
                        std::fs::read(root.join(path)).ok()
                    })
                    .as_deref();
                RefHit {
                    path: path.to_string(),
                    line: line + 1,
                    col: col + 1,
                    start_byte: *start,
                    end_byte: *end,
                    text: bytes
                        .and_then(|b| line_text(b, *start as usize, *end as usize, name))
                        .unwrap_or_default(),
                }
            })
            .collect();
        Ok(Page {
            total,
            page: req.page.max(1),
            per_page: req.per_page.max(1),
            items,
            truncated: false,
        })
    }

    /// Search declarations by name. The search ignores the case and ranks an exact name
    /// first, then names that start with the query, then names that contain the characters of
    /// the query in order (`inv` finds `Invoice`, `tot` and `ttl` find `total`). A `kind`
    /// limits the result to one kind. The search reads at most 2,000 names per segment and stops
    /// after it has enough results for the page (then `truncated` is true and `total` is a
    /// lower bound).
    pub fn symbols(
        &self,
        query: &str,
        kind: Option<DeclKind>,
        req: PageReq,
    ) -> Result<Page<SymbolHit>> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(Page::empty(req));
        }
        // The pages ask for the first `want` results. Names are read in rank order, so a
        // search with thousands of matches reads only the lists of the best names.
        let want = (req.page.max(1) * req.per_page.max(1)).max(SYMBOL_MIN);
        // (class, name, layer, offset of the posting list)
        let mut keys: Vec<(u8, Vec<u8>, usize, u64)> = Vec::new();
        let mut truncated = false;
        for (i, layer) in self.layers.iter().enumerate() {
            let map = layer.segment.names_map(true);
            let mut found = 0;
            if let Some(v) = map.get(q.as_bytes()) {
                keys.push((0, q.as_bytes().to_vec(), i, v));
                found += 1;
            }
            let mut stream = map.search(Str::new(&q).starts_with()).into_stream();
            while let Some((k, v)) = stream.next() {
                if k == q.as_bytes() {
                    continue;
                }
                if found >= SYMBOL_CAP {
                    truncated = true;
                    break;
                }
                keys.push((1, k.to_vec(), i, v));
                found += 1;
            }
            if found < 50 {
                let mut stream = map.search(Subsequence::new(&q)).into_stream();
                while let Some((k, v)) = stream.next() {
                    if k.starts_with(q.as_bytes()) {
                        continue;
                    }
                    if found >= SYMBOL_CAP {
                        truncated = true;
                        break;
                    }
                    keys.push((2, k.to_vec(), i, v));
                    found += 1;
                }
            }
        }
        keys.sort_by(|a, b| (a.0, a.1.len(), &a.1).cmp(&(b.0, b.1.len(), &b.1)));
        // (class, name length, name, path, line, layer, file, declaration)
        type Row = (u8, usize, Vec<u8>, String, u32, usize, u32, u32);
        let mut rows: Vec<Row> = Vec::new();
        for (class, key, i, rel) in keys {
            if rows.len() >= want {
                truncated = true;
                break;
            }
            let seg = &self.layers[i].segment;
            for p in seg.decls_at(rel)? {
                if kind.is_some_and(|k| k != p.kind) {
                    continue;
                }
                let path = &seg.files()[p.file as usize].path;
                if self.is_live(i, path) {
                    let name_len = key.len();
                    let row = (
                        class,
                        name_len,
                        key.clone(),
                        path.clone(),
                        p.line,
                        i,
                        p.file,
                        p.decl,
                    );
                    rows.push(row);
                }
            }
        }
        rows.sort_by(|a, b| (a.0, a.1, &a.2, &a.3, a.4).cmp(&(b.0, b.1, &b.2, &b.3, b.4)));
        let total = rows.len();
        let range = req.range(total);
        let mut items = Vec::with_capacity(range.len());
        for (_, _, _, path, line, layer, file, decl) in &rows[range] {
            let stub = self.layers[*layer].segment.stub(*file)?;
            let Some(d) = stub.declarations.get(*decl as usize) else {
                continue;
            };
            items.push(SymbolHit {
                path: path.clone(),
                line: line + 1,
                name: d.name.clone(),
                kind: d.kind,
                container: d.container.clone(),
                signature: clip(&d.signature),
            });
        }
        Ok(Page {
            total,
            page: req.page.max(1),
            per_page: req.per_page.max(1),
            items,
            truncated,
        })
    }
}

/// True when all qualifiers match the enclosing declarations (nearest first, in order, gaps
/// allowed) or a component of the path.
fn qualifiers_match(quals: &[&str], chain: &[&str], path: &str) -> bool {
    if quals.is_empty() {
        return true;
    }
    let lower_path: Vec<String> = path
        .to_lowercase()
        .split(['/', '.'])
        .map(str::to_owned)
        .collect();
    let mut next = 0;
    quals.iter().rev().all(|q| {
        if matches!(*q, "crate" | "self" | "super") {
            return true;
        }
        if let Some(p) = chain[next..].iter().position(|a| a == q) {
            next += p + 1;
            true
        } else {
            lower_path.iter().any(|c| *c == q.to_lowercase())
        }
    })
}

/// The trimmed text of the line around `start..end`, when the bytes there equal `name`.
fn line_text(bytes: &[u8], start: usize, end: usize, name: &str) -> Option<String> {
    if bytes.get(start..end)? != name.as_bytes() {
        return None;
    }
    let from = bytes[..start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let to = bytes[end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |i| end + i);
    Some(clip(&String::from_utf8_lossy(&bytes[from..to])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_split_and_match() {
        assert_eq!(
            split_qualified("billing::Invoice::total"),
            ["billing", "Invoice", "total"]
        );
        assert_eq!(split_qualified("a.b#c"), ["a", "b", "c"]);
        assert!(qualifiers_match(&["Invoice"], &["Invoice"], "src/a.rs"));
        assert!(qualifiers_match(
            &["billing", "Invoice"],
            &["Invoice"],
            "src/billing/a.rs"
        ));
        assert!(!qualifiers_match(&["Other"], &["Invoice"], "src/a.rs"));
        assert!(qualifiers_match(
            &["crate", "Invoice"],
            &["Invoice"],
            "a.rs"
        ));
    }

    #[test]
    fn line_text_checks_the_bytes() {
        let src = b"let a = 1;\n    foo(bar);\nend\n";
        let start = 15;
        assert_eq!(
            line_text(src, start, start + 3, "foo").unwrap(),
            "foo(bar);"
        );
        assert!(line_text(src, start, start + 3, "bar").is_none());
        assert!(line_text(src, 100, 103, "foo").is_none());
    }

    #[test]
    fn page_render() {
        let hits: Vec<RefHit> = (1..=2)
            .map(|n| RefHit {
                path: format!("src/f{n}.rs"),
                line: n * 10,
                col: 1,
                start_byte: 0,
                end_byte: 1,
                text: "x();".into(),
            })
            .collect();
        let page = Page {
            total: 12,
            page: 1,
            per_page: 2,
            items: hits,
            truncated: false,
        };
        let text = page.render("references(sym: \"x\")");
        assert!(text.starts_with("references(sym: \"x\")  →  12 results\n"));
        assert!(text.contains("  src/f1.rs:10   x();\n"));
        assert!(text.ends_with("  ... (10 more, use page=2)\n"));
    }
}
