use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_syntax::Language;

use super::*;

fn far() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn parse(path: &str, src: &str) -> ParsedFile {
    ParsedFile::parse(Arc::new(FileCtx::new(
        path,
        src.to_owned(),
        Language::detect(path),
    )))
    .unwrap()
}

fn scan_with(rules: &RuleSet, path: &str, src: &str) -> Vec<SarifResult> {
    let file = parse(path, src);
    let mut out = Vec::new();
    assert!(rules.scan_file(&file, far(), &mut out));
    out
}

fn scan(path: &str, src: &str) -> Vec<SarifResult> {
    scan_with(&RuleSet::builtin(), path, src)
}

fn ids(results: &[SarifResult]) -> Vec<&str> {
    results.iter().map(|r| r.rule_id.as_str()).collect()
}

fn rust_fn_with_lines(name: &str, lines: usize) -> String {
    let mut s = format!("fn {name}() {{\n");
    for i in 0..lines.saturating_sub(2) {
        s.push_str(&format!("    let v{i} = {i};\n"));
    }
    s.push_str("}\n");
    s
}

#[test]
fn builtin_yaml_pack_loads_without_errors() {
    let rules = RuleSet::builtin();
    assert!(rules.errors().is_empty(), "{:?}", rules.errors());
    assert!(rules.rule_count() >= 20);
}

#[test]
fn long_function_over_80_lines() {
    let long = rust_fn_with_lines("big", 90);
    let short = rust_fn_with_lines("small", 10);
    let r = scan("src/a.rs", &format!("{short}\n{long}"));
    let hits: Vec<_> = r
        .iter()
        .filter(|r| r.rule_id == RULE_LONG_FUNCTION)
        .collect();
    assert_eq!(hits.len(), 1, "{r:?}");
    assert!(hits[0].message.text.contains("`big`"));
    assert!(hits[0].message.text.contains("90 lines"));
    // exactly 80 lines is fine
    let ok = scan("src/a.rs", &rust_fn_with_lines("edge", 80));
    assert!(!ids(&ok).contains(&RULE_LONG_FUNCTION));
}

#[test]
fn long_function_in_python_and_js() {
    let mut py = String::from("def big():\n");
    for i in 0..85 {
        py.push_str(&format!("    v{i} = {i}\n"));
    }
    assert!(ids(&scan("a.py", &py)).contains(&RULE_LONG_FUNCTION));
    let mut js = String::from("const big = () => {\n");
    for i in 0..85 {
        js.push_str(&format!("  const v{i} = {i};\n"));
    }
    js.push_str("};\n");
    let r = scan("a.js", &js);
    let hit = r.iter().find(|r| r.rule_id == RULE_LONG_FUNCTION).unwrap();
    assert!(hit.message.text.contains("`big`"), "{}", hit.message.text);
}

#[test]
fn deep_nesting_over_4() {
    let deep = "fn f(a: bool) {\n if a { if a { if a { if a { if a { println!(); } } } } }\n}\n";
    let r = scan("a.rs", deep);
    let hit = r.iter().find(|r| r.rule_id == RULE_DEEP_NESTING).unwrap();
    assert!(
        hit.message.text.contains("5 levels"),
        "{}",
        hit.message.text
    );
    let four = "fn f(a: bool) {\n if a { if a { if a { if a { println!(); } } } }\n}\n";
    assert!(!ids(&scan("a.rs", four)).contains(&RULE_DEEP_NESTING));
}

#[test]
fn else_if_chain_does_not_nest() {
    let mut s = String::from("fn f(a: i32) {\n if a == 0 { }\n");
    for i in 1..8 {
        s.push_str(&format!(" else if a == {i} {{ }}\n"));
    }
    s.push_str("}\n");
    assert!(!ids(&scan("a.rs", &s)).contains(&RULE_DEEP_NESTING));
    // JavaScript: `else if` has an else_clause.
    let js = "function f(a) { if (a == 0) {} else if (a == 1) {} else if (a == 2) {} else if (a == 3) {} else if (a == 4) {} else if (a == 5) {} }\n";
    assert!(!ids(&scan("a.js", js)).contains(&RULE_DEEP_NESTING));
    // Java: `alternative` field.
    let java = "class A { void f(int a) { if (a == 0) {} else if (a == 1) {} else if (a == 2) {} else if (a == 3) {} else if (a == 4) {} else if (a == 5) {} } }\n";
    assert!(!ids(&scan("A.java", java)).contains(&RULE_DEEP_NESTING));
}

#[test]
fn nesting_is_per_function() {
    // A nested function starts at depth 0.
    let src =
        "fn outer(a: bool) {\n if a { if a {\n fn inner(b: bool) { if b { if b { } } }\n } }\n}\n";
    assert!(!ids(&scan("a.rs", src)).contains(&RULE_DEEP_NESTING));
}

#[test]
fn high_cyclomatic_complexity_over_15() {
    let mut s = String::from("fn f(a: i32) {\n");
    for i in 0..15 {
        s.push_str(&format!("    if a == {i} {{ }}\n"));
    }
    s.push_str("}\n");
    // 15 ifs -> complexity 16 > 15
    let r = scan("a.rs", &s);
    let hit = r.iter().find(|r| r.rule_id == RULE_COMPLEXITY).unwrap();
    assert!(
        hit.message.text.contains("complexity of 16"),
        "{}",
        hit.message.text
    );
    // 14 ifs -> complexity 15: fine
    let mut t = String::from("fn f(a: i32) {\n");
    for i in 0..14 {
        t.push_str(&format!("    if a == {i} {{ }}\n"));
    }
    t.push_str("}\n");
    assert!(!ids(&scan("a.rs", &t)).contains(&RULE_COMPLEXITY));
}

#[test]
fn complexity_counts_operators_and_match_arms() {
    let mut s = String::from("fn f(a: i32, b: bool) -> i32 {\n match a {\n");
    for i in 0..10 {
        s.push_str(&format!("  {i} => 1,\n"));
    }
    s.push_str("  _ => 0,\n }\n");
    s.push_str(" ; if b && a > 0 || a < -5 && b { 1 } else { 2 }\n}\n");
    // 1 + 11 arms + if + && + || + && = 16
    let r = scan("a.rs", &s);
    let hit = r.iter().find(|r| r.rule_id == RULE_COMPLEXITY).unwrap();
    assert!(
        hit.message.text.contains("complexity of 16"),
        "{}",
        hit.message.text
    );
    // Python `and`/`or`
    let mut py = String::from("def f(a, b):\n");
    for i in 0..8 {
        py.push_str(&format!("    if a == {i} and b or a > 3:\n        pass\n"));
    }
    assert!(ids(&scan("a.py", &py)).contains(&RULE_COMPLEXITY));
}

fn dup_block(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!("    let value{i} = compute({i}) + {i};\n"));
    }
    s
}

#[test]
fn duplicated_blocks_by_subtree_hash() {
    let body = dup_block(14);
    let src = format!(
        "fn a(x: i32) {{\n{body}}}\n\nfn b(y: i32) {{\n{body}}}\n\nfn c() {{\n    let z = 1;\n}}\n"
    );
    let rules = RuleSet::builtin();
    let file = parse("a.rs", &src);
    let mut out = Vec::new();
    assert!(rules.scan_duplicates(std::slice::from_ref(&file), far(), &mut out));
    assert_eq!(out.len(), 2, "{out:?}");
    assert!(out.iter().all(|r| r.rule_id == RULE_DUPLICATE));
    assert!(out[0].message.text.contains("also at a.rs:"));
    // Only the outer block is reported, not its statements.
    assert_eq!(out[0].start_line(), Some(1));
}

#[test]
fn short_or_different_blocks_are_not_duplicates() {
    let body = dup_block(5);
    let src = format!("fn a() {{\n{body}}}\nfn b() {{\n{body}}}\n");
    let rules = RuleSet::builtin();
    let file = parse("a.rs", &src);
    let mut out = Vec::new();
    rules.scan_duplicates(std::slice::from_ref(&file), far(), &mut out);
    assert!(out.is_empty());
    // 14 lines, but the second block differs in one literal.
    let a = dup_block(14);
    let b = a.replace("compute(3)", "compute(30)");
    let src = format!("fn a() {{\n{a}}}\nfn b() {{\n{b}}}\n");
    let file = parse("a.rs", &src);
    rules.scan_duplicates(std::slice::from_ref(&file), far(), &mut out);
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn duplicates_across_files() {
    let body = dup_block(13);
    let f1 = parse("a.rs", &format!("fn a() {{\n{body}}}\n"));
    let f2 = parse("b.rs", &format!("fn other() {{\n{body}}}\n"));
    let mut out = Vec::new();
    RuleSet::builtin().scan_duplicates(&[f1, f2], far(), &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].file(), Some("a.rs"));
    assert!(out[0].message.text.contains("b.rs:"));
}

#[test]
fn risky_unwrap_in_rust_skips_test_code() {
    let src = r#"
fn run(x: Option<i32>) -> i32 {
    x.unwrap()
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        let v = Some(1).unwrap();
        assert_eq!(v, 1);
    }
}

#[test]
fn lone_test() {
    Some(2).unwrap();
}
"#;
    let r = scan("src/lib.rs", src);
    let hits: Vec<_> = r
        .iter()
        .filter(|r| r.rule_id == "aivory/risky-unwrap")
        .collect();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].start_line(), Some(3));
    assert_eq!(hits[0].level, Level::Warning);
    // A file in tests/ is test code.
    let r = scan("crates/x/tests/it.rs", "fn f() { Some(1).unwrap(); }\n");
    assert!(r.iter().all(|r| r.rule_id != "aivory/risky-unwrap"));
}

#[test]
fn risky_apis_in_other_languages() {
    let r = scan(
        "a.js",
        "function f(s) { return eval(s); }\nnew Function('a', 'return a');\n",
    );
    assert_eq!(
        r.iter()
            .filter(|r| r.rule_id == "aivory/risky-eval")
            .count(),
        1
    );
    assert_eq!(
        r.iter()
            .filter(|r| r.rule_id == "aivory/risky-new-function")
            .count(),
        1
    );
    let r = scan("a.ts", "const x = eval(code);\nel.innerHTML = html;\n");
    assert!(ids(&r).contains(&"aivory/risky-eval"));
    assert!(ids(&r).contains(&"aivory/risky-inner-html"));
    let r = scan(
        "a.py",
        "import os\ndef f(s):\n    eval(s)\n    os.system(s)\n    exec(s)\n",
    );
    for id in [
        "aivory/risky-eval",
        "aivory/risky-os-system",
        "aivory/risky-exec",
    ] {
        assert!(ids(&r).contains(&id), "{id} in {r:?}");
    }
    let r = scan(
        "a.c",
        "#include <string.h>\nvoid f(char *d, char *s) { strcpy(d, s); }\n",
    );
    assert!(ids(&r).contains(&"aivory/risky-unbounded-copy"));
    let r = scan(
        "A.java",
        "class A { void f(String s) throws Exception { Runtime.getRuntime().exec(s); } }\n",
    );
    assert!(ids(&r).contains(&"aivory/risky-runtime-exec"));
    let r = scan(
        "a.go",
        "package a\nimport \"unsafe\"\nfunc f(p *int) { _ = unsafe.Pointer(p) }\n",
    );
    assert!(ids(&r).contains(&"aivory/risky-unsafe-pointer"));
    let r = scan(
        "A.cs",
        "class A { void F(string s) { Process.Start(s); } }
",
    );
    assert!(ids(&r).contains(&"aivory/risky-process-start"));
}

#[test]
fn clean_code_has_no_findings() {
    let r = scan("a.rs", "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n");
    assert!(r.is_empty(), "{r:?}");
}

#[test]
fn findings_keep_their_fingerprint_when_a_block_moves() {
    let src = "fn run(x: Option<i32>) -> i32 {\n    x.unwrap()\n}\n";
    let moved = format!("// a comment\n// another comment\n\n{src}");
    let a = scan("src/lib.rs", src);
    let b = scan("src/lib.rs", &moved);
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_eq!(a[0].start_line(), Some(2));
    assert_eq!(b[0].start_line(), Some(5));
    assert_eq!(a[0].fingerprint(), b[0].fingerprint());
    assert!(a[0].fingerprint().is_some());
}

#[test]
fn yaml_rules_load_from_the_repo_folder() {
    let dir = tempfile::tempdir().unwrap();
    let rules_dir = dir.path().join(".aivory").join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join("team.yml"),
        "id: team/no-println\nlanguage: rust\nseverity: error\nmessage: 'Use the logger, not println! (found $ARG).'\nrule:\n  pattern: println!($ARG)\n---\nid: team/todo-comment\nlanguage: python\nseverity: hint\nmessage: Resolve the TODO.\nrule:\n  kind: comment\n  regex: TODO\n",
    )
    .unwrap();
    std::fs::write(
        rules_dir.join("broken.yml"),
        "id: x\nlanguage: cobol\nrule: {pattern: a}\n",
    )
    .unwrap();
    std::fs::write(rules_dir.join("notes.txt"), "ignored").unwrap();
    let rules = RuleSet::load(dir.path());
    assert_eq!(rules.errors().len(), 1);
    assert_eq!(rules.errors()[0].file, "broken.yml");
    assert!(rules.errors()[0].message.contains("cobol") || !rules.errors()[0].message.is_empty());

    let r = scan_with(
        &rules,
        "src/main.rs",
        "fn main() {\n    println!(\"hi\");\n}\n",
    );
    let hit = r.iter().find(|r| r.rule_id == "team/no-println").unwrap();
    assert_eq!(hit.level, Level::Error);
    assert_eq!(hit.start_line(), Some(2));
    assert!(
        hit.message.text.contains("found \"hi\""),
        "{}",
        hit.message.text
    );
    let r = scan_with(&rules, "x.py", "x = 1  # TODO later\n");
    let hit = r.iter().find(|r| r.rule_id == "team/todo-comment").unwrap();
    assert_eq!(hit.level, Level::Note);
    // The rule is for Rust only.
    let r = scan_with(&rules, "x.py", "println!(1)\n");
    assert!(r.iter().all(|r| r.rule_id != "team/no-println"));
}

#[test]
fn yaml_rule_files_glob_is_honored() {
    let yaml = "id: team/no-dbg\nlanguage: rust\nseverity: warning\nmessage: no dbg\nrule:\n  pattern: dbg!($A)\nignores:\n  - '**/tests/**'\n";
    let rules = RuleSet::from_yaml(vec![("g.yml".into(), yaml.into())]);
    assert!(rules.errors().is_empty(), "{:?}", rules.errors());
    let src = "fn f() { dbg!(1); }\n";
    assert!(ids(&scan_with(&rules, "src/a.rs", src)).contains(&"team/no-dbg"));
    assert!(!ids(&scan_with(&rules, "x/tests/a.rs", src)).contains(&"team/no-dbg"));
}

#[test]
fn an_expired_deadline_stops_the_scan() {
    let file = parse("a.rs", &rust_fn_with_lines("big", 90));
    let mut out = Vec::new();
    let done = RuleSet::builtin().scan_file(&file, Instant::now(), &mut out);
    assert!(!done);
    assert!(out.is_empty());
}
