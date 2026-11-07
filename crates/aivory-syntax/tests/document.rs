//! Tests of `Document`: incremental parse, edits, and syntax errors.

use aivory_syntax::{Document, Error, Language, SyntaxErrorKind, syntax_errors};

const SOURCE: &str = "fn alpha() -> u32 {\n    let x = 1;\n    x + 2\n}\n\nfn beta(s: &str) -> usize {\n    s.len()\n}\n";

#[test]
fn edit_matches_fresh_parse() {
    let mut doc = Document::new(Language::Rust, SOURCE).unwrap();
    assert!(doc.syntax_errors().is_empty());
    // Insert a line, delete a range, replace one token, and add multi-byte text.
    let steps: [(usize, usize, &str); 5] = [
        (
            SOURCE.find("    x + 2").unwrap(),
            SOURCE.find("    x + 2").unwrap(),
            "    let y = 3;\n",
        ),
        (0, 3, "pub fn"),
        (20, 23, ""),
        (5, 5, "\u{e9}t\u{e9}_"),
        (0, 0, "// h\u{fc}llo\n"),
    ];
    for (start, end, text) in steps {
        doc.edit(start, end, text).unwrap();
        let fresh = Document::new(Language::Rust, doc.source().to_owned()).unwrap();
        assert_eq!(
            doc.to_sexp(),
            fresh.to_sexp(),
            "after edit {start}..{end} {text:?}"
        );
        assert_eq!(doc.syntax_errors(), fresh.syntax_errors());
        assert_eq!(doc.stub().unwrap(), fresh.stub().unwrap());
    }
}

#[test]
fn many_small_edits_match_fresh_parse() {
    let mut doc = Document::new(
        Language::Python,
        "def f(a):\n    return a\n\nclass C:\n    pass\n",
    )
    .unwrap();
    let mut seed = 12345u32;
    let mut next = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as usize
    };
    let pieces = ["x", " ", "\n", "(", ")", "def ", "\u{3b1}", ":", "    "];
    for _ in 0..200 {
        let len = doc.source().len();
        let mut start = next() % (len + 1);
        let mut end = (start + next() % 4).min(len);
        while !doc.source().is_char_boundary(start) {
            start -= 1;
        }
        while !doc.source().is_char_boundary(end) {
            end += 1;
        }
        doc.edit(start, end, pieces[next() % pieces.len()]).unwrap();
        let fresh = Document::new(Language::Python, doc.source().to_owned()).unwrap();
        assert_eq!(doc.to_sexp(), fresh.to_sexp());
    }
}

#[test]
fn replace_text_finds_the_changed_part() {
    let mut doc = Document::new(Language::Rust, SOURCE).unwrap();
    let new = SOURCE.replace("x + 2", "x + 20 + \u{e9}");
    doc.replace_text(&new).unwrap();
    assert_eq!(doc.source(), new);
    let fresh = Document::new(Language::Rust, new.clone()).unwrap();
    assert_eq!(doc.to_sexp(), fresh.to_sexp());
    // The same text again, a longer text, and a shorter text.
    doc.replace_text(&new).unwrap();
    doc.replace_text("").unwrap();
    doc.replace_text(SOURCE).unwrap();
    assert_eq!(doc.source(), SOURCE);
    assert!(doc.syntax_errors().is_empty());
}

#[test]
fn bad_edits_are_errors() {
    let mut doc = Document::new(Language::Rust, "fn \u{e9}() {}").unwrap();
    assert!(matches!(doc.edit(5, 2, ""), Err(Error::BadEdit)));
    assert!(matches!(doc.edit(0, 999, ""), Err(Error::BadEdit)));
    // Byte 4 is inside the two-byte character.
    assert!(matches!(doc.edit(4, 5, ""), Err(Error::BadEdit)));
    assert_eq!(doc.source(), "fn \u{e9}() {}");
}

#[test]
fn errors_appear_and_disappear_with_edits() {
    let mut doc = Document::new(Language::Rust, "fn main() {\n    let x = 1;\n}\n").unwrap();
    assert!(doc.syntax_errors().is_empty());
    let semi = doc.source().find(';').unwrap();
    doc.edit(semi, semi + 1, "").unwrap();
    let errors = doc.syntax_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].kind, SyntaxErrorKind::Missing);
    assert_eq!(errors[0].message, "missing `;`");
    assert_eq!(errors[0].range.start_line, 1);
    assert_eq!(errors[0].range.start_byte, errors[0].range.end_byte);
    doc.edit(semi, semi, ";").unwrap();
    assert!(doc.syntax_errors().is_empty());
}

#[test]
fn unexpected_token_message() {
    let errors = syntax_errors(Language::Rust, "fn main() { let = ; }\n").unwrap();
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .any(|e| e.message.starts_with("unexpected") || e.message.starts_with("missing")),
        "{errors:?}"
    );
    let python = syntax_errors(Language::Python, "x = (1 +\n").unwrap();
    assert!(!python.is_empty(), "{python:?}");
    // Errors are sorted by position and lie inside the text.
    let many = syntax_errors(Language::JavaScript, "let a = ;\nlet b = ;\nfunction ( {\n").unwrap();
    assert!(many.len() >= 2);
    assert!(
        many.windows(2)
            .all(|w| w[0].range.start_byte <= w[1].range.start_byte)
    );
}

#[test]
fn highlight_token_kinds() {
    use aivory_syntax::TokenKind::*;
    let src =
        "#[derive(Debug)]\nfn main() {\n    let s = \"a\"; // c\n    println!(\"{}\", 1 + 2);\n}\n";
    let doc = Document::new(Language::Rust, src).unwrap();
    let spans = doc.highlights().unwrap();
    let kind_of = |needle: &str| {
        let at = src.find(needle).unwrap() as u32;
        spans
            .iter()
            .find(|s| s.start_byte <= at && at < s.end_byte)
            .map(|s| s.kind)
    };
    assert_eq!(kind_of("fn"), Some(Keyword));
    assert_eq!(kind_of("main"), Some(Function));
    assert_eq!(kind_of("\"a\""), Some(String));
    assert_eq!(kind_of("// c"), Some(Comment));
    assert_eq!(kind_of("1 +"), Some(Number));
    assert_eq!(kind_of("+ 2"), Some(Operator));
    assert_eq!(kind_of("derive"), Some(Attribute));
    assert_eq!(kind_of("println"), Some(Macro));
    assert_eq!(kind_of("Debug"), Some(Type));
}

#[test]
fn utf8_columns_are_bytes() {
    let src = "// \u{e9}\u{e9}\nfn f() {}\n";
    let stub = Document::new(Language::Rust, src).unwrap().stub().unwrap();
    let f = stub.declarations.iter().find(|d| d.name == "f").unwrap();
    assert_eq!(f.name_range.start_line, 1);
    assert_eq!(f.name_range.start_col, 3);
    assert_eq!(&src[f.name_range.bytes()], "f");
}

#[test]
fn doc_comment_range() {
    let src = "/// One.\n/// Two.\n#[derive(Debug)]\nstruct A;\n\nlet_x(); // trailing\nfn b() {}\n\n// plain\nfn c() {}\n";
    let stub = Document::new(Language::Rust, src).unwrap().stub().unwrap();
    let a = stub.declarations.iter().find(|d| d.name == "A").unwrap();
    let doc = a.doc.expect("doc of A");
    assert_eq!(&src[doc.bytes()], "/// One.\n/// Two.");
    assert_eq!((doc.start_line, doc.end_line), (0, 1));
    // A trailing comment and a plain comment are not doc comments.
    assert!(
        stub.declarations
            .iter()
            .find(|d| d.name == "b")
            .unwrap()
            .doc
            .is_none()
    );
    assert!(
        stub.declarations
            .iter()
            .find(|d| d.name == "c")
            .unwrap()
            .doc
            .is_none()
    );
}
