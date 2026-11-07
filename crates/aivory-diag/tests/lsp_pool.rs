//! Tests of the LSP pool against the fake language server (`tests/support/fake_lsp.rs`).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_diag::Error;
use aivory_diag::lsp::lsp_types::{Position, Range};
use aivory_diag::lsp::{Ctx, LspPool, PoolConfig, ServerSpec};
use aivory_syntax::Language;
use common::*;

fn pos(line: u32, character: u32) -> Position {
    Position { line, character }
}

#[test]
fn lens_operations_work_against_the_fake_server() {
    let dir = tempfile::tempdir().unwrap();
    let lineage = lineage(dir.path(), "repo");
    let pool = fake_pool(None);
    let ov = overlay(
        "s1",
        &[("src/lib.rs", "fn add() {}\nlet x = add();\nadd();\n")],
    );
    let ctx = Ctx {
        lineage: &lineage,
        overlay: &ov,
    };

    let hover = pool.hover(&ctx, "src/lib.rs", pos(1, 0)).unwrap().unwrap();
    assert_eq!(hover.text, "line: let x = add();");

    let defs = pool.definition(&ctx, "src/lib.rs", pos(1, 8)).unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].path, "src/lib.rs");
    assert_eq!(defs[0].range.end.character, 3);

    let refs = pool.references(&ctx, "src/lib.rs", pos(0, 3)).unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[1].range.start.line, 2);

    // The rename returns edits and writes nothing.
    let before = std::fs::read_to_string(lineage.root.join("src/lib.rs")).unwrap();
    let edits = pool.rename(&ctx, "src/lib.rs", pos(0, 3), "sum").unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].path, "src/lib.rs");
    assert_eq!(edits[0].edits[0].new_text, "sum");
    assert_eq!(
        std::fs::read_to_string(lineage.root.join("src/lib.rs")).unwrap(),
        before
    );

    let tokens = pool.semantic_tokens(&ctx, "src/lib.rs").unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!(
        (tokens[0].line, tokens[0].start, tokens[0].length),
        (0, 0, 3)
    );
    assert_eq!(tokens[0].token_type, "function");
    assert_eq!(tokens[0].modifiers, vec!["declaration"]);
    assert_eq!((tokens[1].line, tokens[1].start), (1, 2));
    assert_eq!(tokens[1].token_type, "variable");
    assert!(tokens[1].modifiers.is_empty());

    let range = Range {
        start: pos(0, 0),
        end: pos(0, 5),
    };
    let actions = pool.code_actions(&ctx, "src/lib.rs", range).unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].title, "Remove FIXME");
    assert_eq!(actions[0].kind.as_deref(), Some("quickfix"));
    assert!(actions[0].preferred);
    assert_eq!(actions[0].edits[0].path, "src/lib.rs");
    assert!(actions[1].has_command && actions[1].edits.is_empty());

    pool.stop_all();
    assert!(pool.running().is_empty());
}

#[test]
fn overlay_swap_sends_the_content_of_each_session() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.txt");
    let lineage = lineage(dir.path(), "repo");
    let pool = fake_pool(Some(&log));

    let a = overlay(
        "a",
        &[
            ("src/lib.rs", "alpha\nbeta\n"),
            ("src/only_a.rs", "only a\n"),
        ],
    );
    let hover_a = |pool: &LspPool| {
        pool.hover(
            &Ctx {
                lineage: &lineage,
                overlay: &a,
            },
            "src/lib.rs",
            pos(0, 0),
        )
        .unwrap()
        .unwrap()
        .text
    };
    assert_eq!(hover_a(&pool), "line: alpha");
    // The same session again: nothing is sent.
    assert_eq!(hover_a(&pool), "line: alpha");
    let l = read_log(&log);
    assert_eq!(count(&l, "initialize"), 1);
    assert_eq!(count(&l, "textDocument/didOpen"), 2);
    assert_eq!(count(&l, "textDocument/didChange"), 0);

    // Session B changes the same file and not the other one: didChange and didClose.
    let b = overlay("b", &[("src/lib.rs", "gamma\n")]);
    let h = pool
        .hover(
            &Ctx {
                lineage: &lineage,
                overlay: &b,
            },
            "src/lib.rs",
            pos(0, 0),
        )
        .unwrap()
        .unwrap();
    assert_eq!(h.text, "line: gamma");
    let l = read_log(&log);
    assert_eq!(count(&l, "textDocument/didChange"), 1);
    assert_eq!(count(&l, "textDocument/didClose"), 1);
    assert!(
        l.iter()
            .any(|x| x.starts_with("textDocument/didClose") && x.ends_with("only_a.rs"))
    );

    // Session C has no overlay: the server sees the file on disk.
    let c = overlay("c", &[]);
    let h = pool
        .hover(
            &Ctx {
                lineage: &lineage,
                overlay: &c,
            },
            "src/lib.rs",
            pos(1, 0),
        )
        .unwrap()
        .unwrap();
    assert_eq!(h.text, "line: disk line 1");
    assert_eq!(count(&read_log(&log), "textDocument/didChange"), 2);
    // One server served all three sessions.
    assert_eq!(count(&read_log(&log), "initialize"), 1);
    assert_eq!(pool.running().len(), 1);
    assert_eq!(pool.running()[0].queries, 4);
}

#[test]
fn one_server_per_lineage_and_queries_in_a_line() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.txt");
    let lineage = lineage(dir.path(), "repo");
    let pool = Arc::new(fake_pool(Some(&log)));
    let ov = overlay("s", &[("src/lib.rs", "one\ntwo\nthree\n")]);
    std::thread::scope(|s| {
        for i in 0..8u32 {
            let (pool, lineage, ov) = (&pool, &lineage, &ov);
            s.spawn(move || {
                let h = pool
                    .hover(
                        &Ctx {
                            lineage,
                            overlay: ov,
                        },
                        "src/lib.rs",
                        pos(i % 3, 0),
                    )
                    .unwrap()
                    .unwrap();
                assert!(h.text.starts_with("line: "));
            });
        }
    });
    let l = read_log(&log);
    assert_eq!(count(&l, "initialize"), 1);
    assert_eq!(count(&l, "textDocument/hover"), 8);
    assert_eq!(count(&l, "textDocument/didOpen"), 1);
    assert_eq!(pool.running().len(), 1);

    // A second lineage gets its own server.
    let other = common::lineage(dir.path(), "other");
    pool.hover(
        &Ctx {
            lineage: &other,
            overlay: &ov,
        },
        "src/lib.rs",
        pos(0, 0),
    )
    .unwrap();
    assert_eq!(pool.running().len(), 2);
    assert_eq!(count(&read_log(&log), "initialize"), 2);
}

#[test]
fn diagnostics_come_from_publish_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let lineage = lineage(dir.path(), "repo");
    let pool = fake_pool(None);
    let ov = overlay(
        "s",
        &[
            ("src/lib.rs", "fn a() {}\n// FIXME later\n"),
            ("src/b.rs", "clean\n"),
        ],
    );
    let ctx = Ctx {
        lineage: &lineage,
        overlay: &ov,
    };
    let files = vec!["src/lib.rs".to_owned(), "src/b.rs".to_owned()];
    let res = pool.diagnostics(&ctx, &files).unwrap();
    assert_eq!(res.len(), 2);
    let lib = res.iter().find(|f| f.file == "src/lib.rs").unwrap();
    assert_eq!(lib.diagnostics.len(), 1);
    assert_eq!(lib.diagnostics[0].range.start.line, 1);
    assert_eq!(lib.diagnostics[0].message, "FIXME found");
    assert!(
        res.iter()
            .find(|f| f.file == "src/b.rs")
            .unwrap()
            .diagnostics
            .is_empty()
    );

    // A change of the content gives new diagnostics.
    let ov2 = overlay(
        "s",
        &[("src/lib.rs", "fn a() {}\n"), ("src/b.rs", "x FIXME\n")],
    );
    let res = pool
        .diagnostics(
            &Ctx {
                lineage: &lineage,
                overlay: &ov2,
            },
            &files,
        )
        .unwrap();
    assert!(
        res.iter()
            .find(|f| f.file == "src/lib.rs")
            .unwrap()
            .diagnostics
            .is_empty()
    );
    assert_eq!(
        res.iter()
            .find(|f| f.file == "src/b.rs")
            .unwrap()
            .diagnostics
            .len(),
        1
    );
}

#[test]
fn an_idle_server_stops_after_15_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let lineage = lineage(dir.path(), "repo");
    let pool = fake_pool(None);
    assert_eq!(pool.config().idle_timeout, Duration::from_secs(15 * 60));
    let ov = overlay("s", &[]);
    let ctx = Ctx {
        lineage: &lineage,
        overlay: &ov,
    };
    pool.hover(&ctx, "src/lib.rs", pos(0, 0)).unwrap();
    assert!(pool.is_running(&lineage, Language::Rust));
    let first_pid = pool.running()[0].pid;

    let report = pool.tick_at(Instant::now() + Duration::from_secs(14 * 60));
    assert!(report.idle_stopped.is_empty());
    assert!(pool.is_running(&lineage, Language::Rust));

    let report = pool.tick_at(Instant::now() + Duration::from_secs(16 * 60));
    assert_eq!(
        report.idle_stopped,
        vec![("repo".to_owned(), "fake".to_owned())]
    );
    assert!(!pool.is_running(&lineage, Language::Rust));
    assert!(pool.running().is_empty());

    // The next query starts a new server.
    pool.hover(&ctx, "src/lib.rs", pos(0, 0)).unwrap();
    assert!(pool.is_running(&lineage, Language::Rust));
    assert_ne!(pool.running()[0].pid, first_pid);
}

#[test]
fn the_memory_cap_stops_the_least_used_server_first() {
    let dir = tempfile::tempdir().unwrap();
    let mb = 1024 * 1024;
    let cfg = PoolConfig {
        memory_cap_bytes: 150 * mb,
        ..config()
    };
    let pool = LspPool::new(cfg, vec![fake_spec(None)], Arc::new(FixedProbe(100 * mb)));
    let ov = overlay("s", &[]);
    let query = |id: &str| {
        let l = lineage(dir.path(), id);
        pool.hover(
            &Ctx {
                lineage: &l,
                overlay: &ov,
            },
            "src/lib.rs",
            pos(0, 0),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(15));
    };
    // Two servers of 100 MB are above the cap of 150 MB: the start of l2 stops l1.
    query("l1");
    assert_eq!(pool.running().len(), 1);
    query("l2");
    let names: Vec<String> = pool.running().iter().map(|s| s.lineage.clone()).collect();
    assert_eq!(names, vec!["l2"]);
}

#[test]
fn tick_evicts_in_order_of_last_use() {
    let dir = tempfile::tempdir().unwrap();
    let mb = 1024 * 1024;
    let probe = Arc::new(MapProbe::default());
    let cfg = PoolConfig {
        memory_cap_bytes: 150 * mb,
        ..config()
    };
    let pool = LspPool::new(cfg, vec![fake_spec(None)], probe.clone());
    let ov = overlay("s", &[]);
    let ask = |id: &str| {
        let l = lineage(dir.path(), id);
        pool.hover(
            &Ctx {
                lineage: &l,
                overlay: &ov,
            },
            "src/lib.rs",
            pos(0, 0),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(15));
    };
    for id in ["a", "b", "c", "d"] {
        ask(id);
    }
    ask("a"); // order of last use, oldest first: b, c, d, a
    for s in pool.running() {
        probe.0.lock().unwrap().insert(s.pid, 100 * mb);
    }
    assert!(probe.0.lock().unwrap().len() == 4);
    // 400 MB with a cap of 150 MB: stop b, c, d in this order (100 MB are left).
    let report = pool.tick();
    let order: Vec<&str> = report.evicted.iter().map(|k| k.0.as_str()).collect();
    assert_eq!(order, vec!["b", "c", "d"]);
    let left: Vec<String> = pool.running().iter().map(|s| s.lineage.clone()).collect();
    assert_eq!(left, vec!["a"]);
}

#[test]
fn memory_of_a_real_process_is_read_with_sysinfo() {
    let dir = tempfile::tempdir().unwrap();
    let l = lineage(dir.path(), "repo");
    // The default probe reads the process through sysinfo.
    let pool = LspPool::new(
        config(),
        vec![fake_spec(None)],
        Arc::new(aivory_diag::lsp::SysinfoProbe),
    );
    let ov = overlay("s", &[]);
    pool.hover(
        &Ctx {
            lineage: &l,
            overlay: &ov,
        },
        "src/lib.rs",
        pos(0, 0),
    )
    .unwrap();
    let info = &pool.running()[0];
    assert!(info.memory_bytes > 100_000, "{info:?}");
    assert!(aivory_diag::lsp::total_memory() > info.memory_bytes);
}

#[test]
fn missing_server_and_unknown_language_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let l = lineage(dir.path(), "repo");
    let ov = overlay("s", &[]);
    let ctx = Ctx {
        lineage: &l,
        overlay: &ov,
    };
    let spec = ServerSpec::new("ghost", "aivory-no-such-server-xyz", &[], &[Language::Rust]);
    let pool = LspPool::new(config(), vec![spec], Arc::new(FixedProbe(1)));
    let err = pool.hover(&ctx, "src/lib.rs", pos(0, 0)).unwrap_err();
    assert!(
        matches!(err, Error::ServerNotFound(ref n) if n == "ghost"),
        "{err}"
    );
    assert!(pool.available(Language::Rust).is_none());
    let err = pool.hover(&ctx, "notes.txt", pos(0, 0)).unwrap_err();
    assert!(matches!(err, Error::NoServer(_)), "{err}");
    let err = pool.hover(&ctx, "main.py", pos(0, 0)).unwrap_err();
    assert!(matches!(err, Error::NoServer(_)), "{err}");
}

#[test]
fn a_missing_file_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let l = lineage(dir.path(), "repo");
    let pool = fake_pool(None);
    let ov = overlay("s", &[]);
    // The file is neither in the overlay nor on disk.
    let err = pool
        .hover(
            &Ctx {
                lineage: &l,
                overlay: &ov,
            },
            "src/missing.rs",
            pos(0, 0),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err}");
}

#[test]
fn the_known_servers_cover_the_languages_of_the_design() {
    let specs = aivory_diag::lsp::known_servers();
    let ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    for want in [
        "rust-analyzer",
        "typescript-language-server",
        "pyright",
        "gopls",
        "clangd",
        "jdtls",
    ] {
        assert!(ids.contains(&want), "{want}");
    }
    assert!(specs.iter().any(|s| s.serves(Language::Tsx)));
}

/// The real rust-analyzer. Run with `cargo test -p aivory-diag -- --ignored`.
#[test]
#[ignore = "needs rust-analyzer on PATH"]
fn real_rust_analyzer() {
    let pool = LspPool::default();
    let Some(spec) = pool.available(Language::Rust) else {
        eprintln!("rust-analyzer is not on PATH: skipped");
        return;
    };
    let ok = std::process::Command::new(spec.find(&pool.config().path_var).unwrap())
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("rust-analyzer does not run (rustup component missing): skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn twice() -> i32 {\n    add(1, 1) + add(2, 2)\n}\n\nfn broken() -> i32 {\n    \"text\"\n}\n";
    std::fs::write(root.join("src/lib.rs"), src).unwrap();
    let lineage = aivory_diag::lsp::Lineage {
        id: "proj".into(),
        root: root.clone(),
    };
    let ov = overlay("s", &[("src/lib.rs", src)]);
    let ctx = Ctx {
        lineage: &lineage,
        overlay: &ov,
    };

    // The server needs time to load the project: retry the hover.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut hover = None;
    while Instant::now() < deadline {
        if let Ok(Some(h)) = pool.hover(&ctx, "src/lib.rs", pos(5, 5)) {
            hover = Some(h);
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let hover = hover.expect("hover after loading");
    assert!(hover.text.contains("add"), "{}", hover.text);

    let defs = pool.definition(&ctx, "src/lib.rs", pos(5, 5)).unwrap();
    assert_eq!(defs[0].path, "src/lib.rs");
    assert_eq!(defs[0].range.start.line, 0);
    let refs = pool.references(&ctx, "src/lib.rs", pos(0, 8)).unwrap();
    assert!(refs.len() >= 3, "{refs:?}");
    let edits = pool.rename(&ctx, "src/lib.rs", pos(0, 8), "plus").unwrap();
    assert_eq!(edits.len(), 1);
    assert!(edits[0].edits.len() >= 3);
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        src
    );
    assert!(!pool.semantic_tokens(&ctx, "src/lib.rs").unwrap().is_empty());
    let diags = pool.diagnostics(&ctx, &["src/lib.rs".to_owned()]).unwrap();
    assert!(!diags[0].diagnostics.is_empty(), "type error is reported");
    pool.stop_all();
}
