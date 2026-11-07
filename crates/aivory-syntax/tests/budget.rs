//! Time budgets from `docs/design/03-code-intelligence.md`. Run them in release mode:
//! `cargo test --release -p aivory-syntax -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use aivory_syntax::{Document, Language, Stub, highlight};

/// A synthetic Rust file of at least `lines` lines.
fn synthetic_rust(lines: usize) -> String {
    const BLOCK: &str = r#"/// Item number NN.
pub struct ItemNN {
    pub a: u32,
    pub b: String,
}

impl ItemNN {
    pub fn new(a: u32) -> Self {
        let b = format!("item {}", a);
        Self { a, b }
    }

    pub fn total(&self, other: &ItemNN) -> u32 {
        self.a + other.a * 2 // doubled
    }
}

"#;
    let per_block = BLOCK.lines().count();
    (0..lines.div_ceil(per_block))
        .map(|n| BLOCK.replace("NN", &n.to_string()))
        .collect()
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
#[ignore = "time budget; run in release mode"]
fn rust_10k_lines() {
    let source = synthetic_rust(10_000);
    let lines = source.lines().count();
    assert!((10_000..11_500).contains(&lines), "{lines} lines");
    eprintln!("source: {lines} lines, {} KiB", source.len() / 1024);

    // Warm up: the first call compiles the queries of the language.
    let warm = Document::new(Language::Rust, "fn a() {}").unwrap();
    warm.highlights().unwrap();
    warm.stub().unwrap();

    // Full parse, full highlight (the highlighter parses on its own), parse plus highlight, and
    // the stub of a parsed document. Each runs 7 times; the report shows the median.
    let mut doc = Document::new(Language::Rust, source.clone()).unwrap();
    assert!(doc.syntax_errors().is_empty());
    let (mut parse, mut hl, mut parse_hl, mut stub_time) = (vec![], vec![], vec![], vec![]);
    for _ in 0..7 {
        let t = Instant::now();
        let d = Document::new(Language::Rust, source.clone()).unwrap();
        parse.push(t.elapsed());

        let t = Instant::now();
        let spans = highlight(Language::Rust, &source).unwrap();
        hl.push(t.elapsed());
        assert!(spans.len() > 10_000);

        let t = Instant::now();
        let d2 = Document::new(Language::Rust, source.clone()).unwrap();
        let _ = d2.highlights().unwrap();
        parse_hl.push(t.elapsed());

        let t = Instant::now();
        let stub = d.stub().unwrap();
        stub_time.push(t.elapsed());
        assert!(stub.declarations.len() > 3_000);
    }
    let (parse, hl, parse_hl, stub_time) = (
        median(parse),
        median(hl),
        median(parse_hl),
        median(stub_time),
    );

    // Incremental reparse after a one-line edit in the middle of the file.
    let needle = "other.a * 2";
    let mid = source.len() / 2;
    let pos = mid + source[mid..].find(needle).unwrap() + needle.len() - 1;
    let mut times = Vec::new();
    for i in 0..51 {
        let digit = if i % 2 == 0 { "3" } else { "2" };
        let t = Instant::now();
        doc.edit(pos, pos + 1, digit).unwrap();
        times.push(t.elapsed());
    }
    let inc = median(times.clone());
    let worst = *times.iter().max().unwrap();
    assert!(doc.syntax_errors().is_empty());

    // The incremental result equals a fresh parse.
    let fresh = Document::new(Language::Rust, doc.source().to_owned()).unwrap();
    assert_eq!(doc.to_sexp(), fresh.to_sexp());
    assert_eq!(
        Stub::parse(Language::Rust, doc.source()).unwrap(),
        doc.stub().unwrap()
    );

    // Viewport highlight: 200 visible lines in the middle of the file, on the parsed tree.
    let doc_view = Document::new(Language::Rust, source.clone()).unwrap();
    let mut view_times = Vec::new();
    for i in 0..51 {
        let first = 4_000 + i * 7;
        let t = Instant::now();
        let spans = doc_view.highlights_in_lines(first, first + 200).unwrap();
        view_times.push(t.elapsed());
        assert!(spans.len() > 200);
    }
    let view = median(view_times.clone());
    let view_worst = *view_times.iter().max().unwrap();

    eprintln!("full parse:                 {parse:?}");
    eprintln!("full highlight:             {hl:?}");
    eprintln!("parse + highlight:          {parse_hl:?}");
    eprintln!("stub (no parse):            {stub_time:?}");
    eprintln!("incremental reparse median: {inc:?} (worst {worst:?})");
    eprintln!("viewport highlight (200 l): {view:?} (worst {view_worst:?})");
    assert!(
        inc < Duration::from_millis(8),
        "incremental reparse: {inc:?}"
    );
    assert!(
        view < Duration::from_millis(8),
        "viewport highlight: {view:?}"
    );
    assert!(
        parse_hl < Duration::from_millis(250),
        "parse + highlight: {parse_hl:?}"
    );
}
