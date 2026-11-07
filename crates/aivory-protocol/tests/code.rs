//! Round trips of the `code/*` and `ledger/*` types, and the demo values of the ledger.

use std::fmt::Debug;

use aivory_protocol::code::*;
use aivory_protocol::demo;
use aivory_protocol::rpc::{self, Method, method};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Serialize, parse again, and compare. Returns the JSON.
fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(v: &T) -> Value {
    let json = serde_json::to_value(v).unwrap();
    let back: T = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(&back, v);
    json
}

fn place() -> Place {
    Place {
        path: "crates/orch/src/budget.rs".into(),
        line: 18,
        col: 11,
        len: Some(5),
        preview: "pub fn check(&self, cb: &Codebase) -> Result<(), BudgetError> {".into(),
        name: Some("check".into()),
        kind: Some(SymbolKind::Method),
        container: Some("Budget".into()),
    }
}

fn finding() -> Finding {
    Finding {
        rule: "aivory/risky-unwrap".into(),
        level: FindingLevel::Warning,
        message: "m".into(),
        path: "a.rs".into(),
        line: 3,
        col: 4,
        end_line: Some(3),
        end_col: Some(9),
        tool: "aivory-rules".into(),
        fingerprint: "ab12".into(),
    }
}

#[test]
fn code_file_round_trips_with_camel_case_words() {
    let params = CodeFileParams {
        codebase: "aivory-code".into(),
        path: "a.rs".into(),
        session: Some("a01c".into()),
        rev: Some("turn:2".into()),
        lines: Some(LineRange { from: 1, to: 50 }),
    };
    let json = round_trip(&params);
    assert_eq!(json["lines"], json!({"from": 1, "to": 50}));
    // The optional fields stay out of the JSON.
    let bare = CodeFileParams {
        session: None,
        rev: None,
        lines: None,
        ..params
    };
    assert_eq!(
        round_trip(&bare),
        json!({"codebase": "aivory-code", "path": "a.rs"})
    );

    let file = CodeFile {
        path: "a.rs".into(),
        language: "rust".into(),
        text: "fn a() {}\n".into(),
        total_lines: 1,
        truncated: false,
        highlighted: LineRange { from: 1, to: 1 },
        highlights: vec![Highlight {
            line: 1,
            start: 0,
            end: 2,
            kind: TokenKind::Keyword,
        }],
        outline: vec![OutlineNode {
            name: "a".into(),
            kind: SymbolKind::Function,
            line: 1,
            start_line: 1,
            end_line: 1,
            children: Vec::new(),
        }],
        folding: vec![Fold {
            start_line: 1,
            end_line: 3,
            kind: FoldKind::Imports,
        }],
        diagnostics: vec![finding()],
    };
    let json = round_trip(&file);
    assert_eq!(json["highlights"][0]["kind"], "keyword");
    assert_eq!(json["outline"][0]["kind"], "function");
    assert_eq!(json["folding"][0]["kind"], "imports");
    assert_eq!(json["diagnostics"][0]["level"], "warning");
    assert_eq!(json["totalLines"], 1);
    assert!(json.get("truncated").is_none());
}

#[test]
fn position_results_round_trip() {
    let params = CodePosParams {
        codebase: "c".into(),
        path: "a.rs".into(),
        line: 18,
        col: 11,
        session: None,
        page: Some(2),
        per_page: Some(50),
    };
    assert_eq!(round_trip(&params)["perPage"], 50);
    let page = PlacePage {
        name: "check".into(),
        items: vec![place()],
        total: 41,
        page: 1,
        per_page: 20,
        truncated: true,
        source: CodeSource::Lsp,
        indexing: true,
    };
    let json = round_trip(&page);
    assert_eq!(json["source"], "lsp");
    assert_eq!(json["items"][0]["kind"], "method");
    assert_eq!(json["indexing"], true);
    let hover = Hover {
        name: "check".into(),
        text: "```rust\npub fn check\n```".into(),
        signature: Some("pub fn check".into()),
        kind: Some(SymbolKind::Method),
        range: Some(Highlight {
            line: 18,
            start: 11,
            end: 16,
            kind: TokenKind::Function,
        }),
        definitions: vec![place()],
        source: CodeSource::Index,
        indexing: false,
    };
    assert_eq!(round_trip(&hover)["source"], "index");
}

#[test]
fn symbols_diagnostics_and_rename_round_trip() {
    round_trip(&SymbolsParams {
        codebase: "c".into(),
        query: "inv".into(),
        kind: Some(SymbolKind::TypeAlias),
        session: None,
        page: None,
        per_page: None,
    });
    let page = SymbolPage {
        items: vec![Symbol {
            name: "Invoice".into(),
            kind: SymbolKind::Struct,
            path: "a.rs".into(),
            line: 4,
            container: None,
            signature: "pub struct Invoice".into(),
        }],
        total: 1,
        page: 1,
        per_page: 20,
        truncated: false,
        indexing: false,
    };
    assert_eq!(round_trip(&page)["items"][0]["kind"], "struct");
    round_trip(&DiagnosticsParams {
        codebase: "c".into(),
        session: Some("s".into()),
        turn: Some(3),
        path: None,
    });
    let result = DiagnosticsResult {
        turn: Some(3),
        rows: vec![finding()],
        added: vec![finding()],
        removed: Vec::new(),
        notes: vec!["linters are off".into()],
    };
    let json = round_trip(&result);
    assert!(json["removed"].as_array().unwrap().is_empty());
    let preview = RenamePreview {
        old_name: "a".into(),
        new_name: "b".into(),
        files: vec![FileEdits {
            path: "a.rs".into(),
            edits: vec![TextEdit {
                line: 1,
                col: 3,
                end_line: 1,
                end_col: 4,
                new_text: "b".into(),
            }],
        }],
        exact: false,
        source: CodeSource::Index,
    };
    let json = round_trip(&preview);
    assert_eq!(json["files"][0]["edits"][0]["newText"], "b");
    assert_eq!(
        round_trip(&RenameParams {
            codebase: "c".into(),
            path: "a.rs".into(),
            line: 1,
            col: 3,
            new_name: "b".into(),
            session: None,
        })["newName"],
        "b"
    );
}

#[test]
fn ledger_types_round_trip() {
    assert_eq!(
        round_trip(&LedgerTurnsParams {
            session: "s".into()
        }),
        json!({"session": "s"})
    );
    let turns = demo::ledger_turns();
    let json = round_trip(&turns);
    assert!(json["turns"][1]["startedAt"].is_i64());
    assert_eq!(json["turns"][0]["snapshot"], true);

    round_trip(&BlameParams {
        codebase: "c".into(),
        path: "a.rs".into(),
        session: Some("s".into()),
        lines: Some(LineRange { from: 1, to: 9 }),
    });
    let blame = demo::ledger_blame();
    let json = round_trip(&blame);
    // A committed hunk has a commit and no turn; a turn hunk has no commit.
    assert!(json["hunks"][0].get("turn").is_none());
    assert!(json["hunks"][1].get("commit").is_none());
    assert_eq!(json["hunks"][1]["turn"]["n"], 1);

    round_trip(&TimelineParams {
        codebase: "c".into(),
        path: "a.rs".into(),
        session: None,
    });
    let timeline = demo::ledger_timeline();
    let json = round_trip(&timeline);
    assert_eq!(json["events"][0]["type"], "commit");
    assert_eq!(json["events"][1]["type"], "turn");
    assert_eq!(json["events"][1]["session"], "a01c");

    assert_eq!(
        round_trip(&RevertParams {
            session: "s".into(),
            turn: 2
        }),
        json!({"session": "s", "turn": 2})
    );
    round_trip(&RevertResult {
        session: "s".into(),
        turn: 2,
        restored: 1,
        snapshot: "0123456789ab".into(),
    });
}

#[test]
fn demo_prompts_obey_the_limit_and_the_blame_covers_the_file() {
    for t in &demo::ledger_turns().turns {
        assert!(t.prompt.chars().count() <= PROMPT_CHARS);
    }
    let file = demo::code_files()
        .into_iter()
        .find(|f| f.path == "crates/orch/src/budget.rs")
        .unwrap();
    let lines = file.text.lines().count() as u32;
    let mut next = 1;
    for h in demo::ledger_blame().hunks {
        assert_eq!(h.line, next, "hunks are in order, without a gap");
        next += h.count;
    }
    assert_eq!(next - 1, lines);
}

#[test]
fn the_methods_are_in_the_surface() {
    assert_eq!(method::CodeFileMethod::NAME, "code/file");
    assert_eq!(method::LedgerRevert::NAME, "ledger/revert");
    for name in [
        "code/file",
        "code/definition",
        "code/references",
        "code/hover",
        "code/symbols",
        "code/diagnostics",
        "code/rename",
        "ledger/turns",
        "ledger/blame",
        "ledger/timeline",
        "ledger/revert",
    ] {
        assert!(rpc::METHODS.contains(&name), "{name}");
    }
    let req = rpc::request::<method::CodeReferences>(
        7,
        &CodePosParams {
            codebase: "c".into(),
            path: "a.rs".into(),
            line: 1,
            col: 0,
            session: None,
            page: None,
            per_page: None,
        },
    );
    assert_eq!(req["method"], "code/references");
}

/// The ledger fixtures parse as their types.
#[test]
fn ledger_fixtures_parse() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto/fixtures");
    let read = |name: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap()
    };
    let turns: TurnList = serde_json::from_value(read("demo-ledger-turns.json")).unwrap();
    assert_eq!(turns.turns.len(), 4);
    let blame: FileBlame = serde_json::from_value(read("demo-ledger-blame.json")).unwrap();
    assert_eq!(blame.hunks.len(), 4);
    let timeline: Timeline = serde_json::from_value(read("demo-ledger-timeline.json")).unwrap();
    assert_eq!(timeline.events.len(), 4);
    let diag: DiagnosticsResult =
        serde_json::from_value(read("demo-code-diagnostics.json")).unwrap();
    assert_eq!(diag.removed.len(), 1);
}
