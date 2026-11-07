//! `aivoryd serve --demo`: every `code/*` and `ledger/*` method answers on the synthetic demo
//! files, and the code fixtures in `proto/fixtures/` are the answers of the demo daemon. Run
//! with `AIVORY_BLESS=1` to write the fixtures after a change.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use aivory_protocol::code::{
    CodeFile, CodeSource, DiagnosticsResult, FileBlame, Hover, PlacePage, RenamePreview,
    SymbolKind, SymbolPage, Timeline, TurnList,
};
use aivory_protocol::demo;
use aivory_protocol::rpc::code;
use aivory_sessions::StoreRoots;
use aivoryd::{Config, Daemon, Endpoint, Server, client};
use serde_json::{Value, json};

fn config() -> Config {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../aivory-sessions/tests/fixtures");
    Config {
        roots: StoreRoots {
            claude: base.join("claude"),
            codex: base.join("codex"),
            gemini: base.join("gemini"),
        },
        cache_file: None,
    }
}

fn start(name: &str) -> (Endpoint, thread::JoinHandle<std::io::Result<()>>) {
    let endpoint = Endpoint::new(&format!("code-{name}-{}", std::process::id())).unwrap();
    let daemon = Daemon::with_demo(config());
    let server = Server::bind(Arc::new(daemon), endpoint.clone()).expect("bind");
    (endpoint, thread::spawn(move || server.run()))
}

fn call(endpoint: &Endpoint, method: &str, params: Value) -> Value {
    client::call(endpoint, method, params, &mut |_| {}).unwrap()
}

/// The result of a method, and an assert that it is not an error.
fn result(endpoint: &Endpoint, method: &str, params: Value) -> Value {
    let r = call(endpoint, method, params);
    assert!(r.get("error").is_none(), "{method}: {r}");
    r["result"].clone()
}

fn typed<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}

fn proto_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto/fixtures")
}

/// Sort object keys: a workspace build turns on `preserve_order` of serde_json.
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), sorted(&map[k])))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn golden(name: &str, v: &Value) {
    let path = proto_dir().join(name);
    let mut want = serde_json::to_string_pretty(&sorted(v)).unwrap();
    want.push('\n');
    if std::env::var_os("AIVORY_BLESS").is_some() {
        std::fs::write(&path, want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e}. Run the tests with AIVORY_BLESS=1.",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        have == want,
        "{} is out of date. Run the tests with AIVORY_BLESS=1.",
        path.display()
    );
}

const SPAWN: &str = "crates/orch/src/spawn.rs";
const BUDGET: &str = "crates/orch/src/budget.rs";

#[test]
fn the_demo_answers_every_code_method() {
    let (e, server) = start("methods");

    // code/file: Rust.
    let v = result(
        &e,
        "code/file",
        json!({"codebase": "aivory-code", "path": BUDGET, "lines": {"from": 1, "to": 30}}),
    );
    let file: CodeFile = typed(&v);
    let demo_text = demo::code_files()
        .into_iter()
        .find(|f| f.path == BUDGET)
        .unwrap()
        .text;
    assert_eq!(
        (file.language.as_str(), file.text.as_str()),
        ("rust", demo_text)
    );
    assert_eq!(file.total_lines, 30);
    assert!(!file.highlights.is_empty());
    let outline: Vec<&str> = file.outline.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(outline, ["Budget", "Budget", "BudgetError"]);
    assert!(
        file.folding
            .iter()
            .any(|f| f.start_line == 1 && f.end_line == 3)
    );
    assert!(file.diagnostics.is_empty());
    golden("demo-code-file.json", &v);

    // code/file: TypeScript.
    let v = result(
        &e,
        "code/file",
        json!({"codebase": "aivory-guard", "path": "web/src/findings.ts"}),
    );
    let ts: CodeFile = typed(&v);
    assert_eq!(ts.language, "typescript");
    assert!(ts.outline.iter().any(|o| o.name == "countByDirectory"));
    golden("demo-code-file-ts.json", &v);

    // code/definition, references, hover on `check` in spawn.rs (line 11).
    let at = json!({"codebase": "aivory-code", "path": SPAWN, "line": 11, "col": 21});
    let v = result(&e, "code/definition", at.clone());
    let def: PlacePage = typed(&v);
    assert_eq!(def.name, "check");
    assert_eq!(def.source, CodeSource::Index);
    assert_eq!(def.items.len(), 1);
    assert_eq!(
        (
            def.items[0].path.as_str(),
            def.items[0].line,
            def.items[0].col
        ),
        (BUDGET, 18, 11)
    );
    assert_eq!(def.items[0].container.as_deref(), Some("Budget"));
    golden("demo-code-definition.json", &v);

    let v = result(&e, "code/references", at.clone());
    let refs: PlacePage = typed(&v);
    assert_eq!(refs.total, 1);
    assert_eq!(
        (refs.items[0].path.as_str(), refs.items[0].line),
        (SPAWN, 11)
    );
    assert!(refs.items[0].preview.contains("self.budget.check(cb)?"));
    assert!(!refs.indexing);
    golden("demo-code-references.json", &v);

    let v = result(&e, "code/hover", at);
    let hover: Hover = typed(&v);
    assert_eq!(hover.name, "check");
    assert!(hover.text.contains("pub fn check(&self, cb: &Codebase)"));
    assert_eq!(hover.kind, Some(SymbolKind::Method));
    golden("demo-code-hover.json", &v);

    // code/symbols, in Rust and in TypeScript files of different codebases.
    let v = result(
        &e,
        "code/symbols",
        json!({"codebase": "aivory-code", "query": "budget"}),
    );
    let symbols: SymbolPage = typed(&v);
    assert!(
        symbols
            .items
            .iter()
            .any(|s| s.name == "Budget" && s.kind == SymbolKind::Struct)
    );
    assert!(symbols.items.iter().any(|s| s.name == "BudgetError"));
    assert!(!symbols.indexing);
    golden("demo-code-symbols.json", &v);
    let ts: SymbolPage = typed(&result(
        &e,
        "code/symbols",
        json!({"codebase": "aivory-guard", "query": "count"}),
    ));
    assert_eq!(ts.items[0].name, "countByDirectory");

    // code/rename: a preview, never applied.
    let v = result(
        &e,
        "code/rename",
        json!({"codebase": "aivory-code", "path": BUDGET, "line": 7, "col": 10, "newName": "max_kids"}),
    );
    let rename: RenamePreview = typed(&v);
    assert_eq!(rename.old_name, "max_children");
    assert!(!rename.exact && rename.source == CodeSource::Index);
    assert_eq!(rename.files.len(), 1);
    assert_eq!(rename.files[0].edits[0].line, 7);
    assert!(rename.files[0].edits.len() >= 3);
    golden("demo-code-rename.json", &v);
    let again = result(
        &e,
        "code/file",
        json!({"codebase": "aivory-code", "path": BUDGET}),
    );
    assert!(typed::<CodeFile>(&again).text.contains("max_children"));

    // code/diagnostics.
    let v = result(
        &e,
        "code/diagnostics",
        json!({"codebase": "aivory-code", "session": "a01c"}),
    );
    let diag: DiagnosticsResult = typed(&v);
    assert_eq!(diag.turn, Some(3));
    assert_eq!((diag.rows.len(), diag.removed.len()), (2, 1));
    assert_eq!(v, serde_json::to_value(demo::ledger_diagnostics()).unwrap());
    let other = result(
        &e,
        "code/diagnostics",
        json!({"codebase": "aivory-code", "session": "a7e2"}),
    );
    assert!(typed::<DiagnosticsResult>(&other).rows.is_empty());

    // ledger/*: the demo ledger of session a01c, the same values as the protocol fixtures.
    let v = result(&e, "ledger/turns", json!({"session": "a01c"}));
    assert_eq!(typed::<TurnList>(&v).turns.len(), 4);
    assert_eq!(v, serde_json::to_value(demo::ledger_turns()).unwrap());
    let v = result(
        &e,
        "ledger/blame",
        json!({"codebase": "aivory-code", "path": BUDGET, "session": "a01c"}),
    );
    assert_eq!(typed::<FileBlame>(&v).hunks.len(), 4);
    assert_eq!(v, serde_json::to_value(demo::ledger_blame()).unwrap());
    let v = result(
        &e,
        "ledger/timeline",
        json!({"codebase": "aivory-code", "path": BUDGET}),
    );
    assert_eq!(typed::<Timeline>(&v).events.len(), 4);
    let empty = result(&e, "ledger/turns", json!({"session": "a7e2"}));
    assert!(typed::<TurnList>(&empty).turns.is_empty());

    // Errors.
    let code_of = |m: &str, p: Value| call(&e, m, p)["error"]["code"].clone();
    assert_eq!(
        code_of("ledger/revert", json!({"session": "a01c", "turn": 9})),
        code::INVALID_STATE
    );
    assert_eq!(
        code_of("code/file", json!({"codebase": "nope", "path": "a.rs"})),
        code::NOT_FOUND
    );
    assert_eq!(
        code_of(
            "code/file",
            json!({"codebase": "aivory-code", "path": "../a.rs"})
        ),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of("ledger/turns", json!({"session": "ghost"})),
        code::SESSION_NOT_FOUND
    );
    assert_eq!(code_of("code/file", json!({})), code::INVALID_PARAMS);

    call(&e, "daemon/shutdown", Value::Null);
    server.join().unwrap().unwrap();
}

#[test]
fn a_daemon_without_a_war_room_has_no_code_service() {
    let endpoint = Endpoint::new(&format!("code-none-{}", std::process::id())).unwrap();
    let server = Server::bind(Arc::new(Daemon::new(config())), endpoint.clone()).expect("bind");
    let handle = thread::spawn(move || server.run());
    let r = call(
        &endpoint,
        "code/file",
        json!({"codebase": "a", "path": "b"}),
    );
    assert_eq!(r["error"]["code"], code::NOT_AVAILABLE);
    call(&endpoint, "daemon/shutdown", Value::Null);
    handle.join().unwrap().unwrap();
}

/// The code fixtures parse as their types and serialize back unchanged.
#[test]
fn the_code_fixtures_round_trip() {
    fn check<T: serde::Serialize + serde::de::DeserializeOwned>(name: &str) {
        let text = std::fs::read_to_string(proto_dir().join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}. Run the tests with AIVORY_BLESS=1."));
        let v: Value = serde_json::from_str(&text).unwrap();
        let t: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            sorted(&serde_json::to_value(t).unwrap()),
            sorted(&v),
            "{name}"
        );
    }
    if std::env::var_os("AIVORY_BLESS").is_some() {
        return; // the other test writes the files in this run
    }
    check::<CodeFile>("demo-code-file.json");
    check::<CodeFile>("demo-code-file-ts.json");
    check::<PlacePage>("demo-code-definition.json");
    check::<PlacePage>("demo-code-references.json");
    check::<Hover>("demo-code-hover.json");
    check::<SymbolPage>("demo-code-symbols.json");
    check::<RenamePreview>("demo-code-rename.json");
}
