//! The viewport highlight: spans for a byte range of a parsed tree.

use aivory_syntax::{Document, HighlightSpan, Language, TokenKind, highlight};

const SOURCE: &str = r#"// a comment
use std::fmt;

/// Doc line.
pub struct Item {
    pub a: u32,
}

impl Item {
    pub fn total(&self, n: u32) -> u32 {
        let s = "text"; // trailing
        self.a + n * 2
    }
}
"#;

/// The spans of `full` cut to `range`.
fn clip(full: &[HighlightSpan], range: std::ops::Range<usize>) -> Vec<HighlightSpan> {
    full.iter()
        .filter(|s| (s.start_byte as usize) < range.end && (s.end_byte as usize) > range.start)
        .map(|s| HighlightSpan {
            start_byte: s.start_byte.max(range.start as u32),
            end_byte: s.end_byte.min(range.end as u32),
            kind: s.kind,
        })
        .collect()
}

#[test]
fn whole_range_matches_full_highlight_on_token_kinds() {
    let doc = Document::new(Language::Rust, SOURCE).unwrap();
    let viewport = doc.highlights_in(0..SOURCE.len()).unwrap();
    let full = highlight(Language::Rust, SOURCE).unwrap();
    // Every byte that the viewport highlight marks has the same kind in the full highlight.
    let kind_at = |spans: &[HighlightSpan], at: u32| {
        spans
            .iter()
            .find(|s| s.start_byte <= at && at < s.end_byte)
            .map(|s| s.kind)
    };
    let mut same = 0usize;
    let mut total = 0usize;
    for at in 0..SOURCE.len() as u32 {
        total += 1;
        if kind_at(&viewport, at) == kind_at(&full, at) {
            same += 1;
        }
    }
    assert!(same * 100 >= total * 98, "{same} of {total} bytes agree");
}

#[test]
fn range_is_cut_and_sorted() {
    let doc = Document::new(Language::Rust, SOURCE).unwrap();
    let from = SOURCE.find("pub struct").unwrap();
    let to = SOURCE.find("impl").unwrap();
    let spans = doc.highlights_in(from..to).unwrap();
    assert!(!spans.is_empty());
    assert!(
        spans
            .iter()
            .all(|s| s.start_byte as usize >= from && s.end_byte as usize <= to)
    );
    assert!(spans.windows(2).all(|w| w[0].end_byte <= w[1].start_byte));
    assert!(spans.iter().any(|s| s.kind == TokenKind::Keyword));
    assert_eq!(
        spans.first().map(|s| (s.start_byte as usize, s.kind)),
        Some((from, TokenKind::Keyword))
    );
    // A range inside the full highlight gives the same kinds as the clipped full highlight.
    let full = clip(&highlight(Language::Rust, SOURCE).unwrap(), from..to);
    let kinds = |v: &[HighlightSpan]| v.iter().map(|s| s.kind).collect::<Vec<_>>();
    assert_eq!(kinds(&spans), kinds(&full));
}

#[test]
fn lines_and_clamping() {
    let doc = Document::new(Language::Rust, SOURCE).unwrap();
    // Line 0 is the comment.
    let spans = doc.highlights_in_lines(0, 1).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].kind, TokenKind::Comment);
    assert_eq!(spans[0].start_byte, 0);
    // Past the end of the text.
    assert!(
        doc.highlights_in(SOURCE.len() + 10..SOURCE.len() + 20)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        doc.highlights_in(0..SOURCE.len() + 100).unwrap(),
        doc.highlights_in(0..SOURCE.len()).unwrap()
    );
    assert!(doc.highlights_in_lines(500, 600).unwrap().is_empty());
}

#[test]
fn works_after_an_edit() {
    let mut doc = Document::new(Language::Rust, SOURCE).unwrap();
    let at = SOURCE.find("n * 2").unwrap();
    doc.edit(at, at + 1, "m").unwrap();
    let spans = doc.highlights_in(at..at + 5).unwrap();
    assert!(spans.iter().any(|s| s.kind == TokenKind::Number));
}
