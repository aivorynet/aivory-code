//! The code service on a scratch git repo: two turns go through the real ledger worker, then
//! `code/file`, `code/references`, `ledger/blame`, the diagnostics of a turn, a revert in a
//! linked worktree, and the MCP tool `references`. Synthetic data only.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_agents::mcp::McpServer;
use aivory_protocol::code::{
    CodeFile, CodeSource, DiagnosticsResult, FileBlame, PlacePage, Timeline, TimelineEvent,
    TurnList,
};
use aivory_protocol::rpc::code;
use aivoryd::code::{CodeService, CodeTools, Resolver, SessionCtx, StaticResolver, core_for};
use aivoryd::ledger::{BaselineJob, Job, LedgerWorker, TurnJob, last_turn};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(90);

const BASE: &str = "\
pub fn helper(x: u32) -> u32 {
    x + 1
}

pub fn run() -> u32 {
    helper(1)
}
";

const TURN1: &str = "
pub fn twice(x: u32) -> u32 {
    helper(helper(x))
}
";

const TURN2: &str = "
pub fn parse(s: &str) -> u32 {
    s.parse().unwrap()
}
";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn append(path: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn wait_until(what: &str, ok: impl Fn() -> bool) {
    let begin = Instant::now();
    while !ok() {
        assert!(begin.elapsed() < WAIT, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    wt: PathBuf,
    db: PathBuf,
    worker: LedgerWorker,
    service: CodeService,
    resolver: Arc<dyn Resolver>,
}

fn turn_job(session: &str, workdir: &Path, n: u32, prompt: &str) -> Job {
    Job::Turn(TurnJob {
        session: session.into(),
        workdir: workdir.to_path_buf(),
        provider: "claude".into(),
        model: "opus-5.5".into(),
        n: Some(n),
        prompt: prompt.into(),
        prompt_source: "human",
        started_ms: 1_792_593_000_000 + i64::from(n) * 60_000,
        ended_ms: 1_792_593_030_000 + i64::from(n) * 60_000,
        cost_usd: Some(0.5 * f64::from(n)),
        ..TurnJob::default()
    })
}

/// A repo with one commit, a linked worktree, and two sessions: `sess-1` with two turns in the
/// main checkout, `sess-wt` with one turn in the worktree.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("proj");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("src/lib.rs"), BASE).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    let wt = dir.path().join("wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-branch",
            wt.to_str().unwrap(),
        ],
    );

    let db = dir.path().join("data").join("aivory.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let worker = LedgerWorker::start(db.clone());
    for (session, workdir) in [("sess-1", &repo), ("sess-wt", &wt)] {
        worker.send(Job::Baseline(BaselineJob {
            session: session.into(),
            workdir: workdir.clone(),
            provider: "claude".into(),
            model: "opus-5.5".into(),
        }));
    }
    wait_until("the baselines", || {
        last_turn(&db, "sess-1") == 0
            && ledger_has(&db, "sess-1", 0)
            && ledger_has(&db, "sess-wt", 0)
    });

    append(&repo.join("src/lib.rs"), TURN1);
    worker.send(turn_job("sess-1", &repo, 1, "add the twice helper"));
    wait_until("turn 1", || last_turn(&db, "sess-1") >= 1);
    append(&repo.join("src/lib.rs"), TURN2);
    worker.send(turn_job("sess-1", &repo, 2, "parse a number from a string"));
    wait_until("turn 2", || last_turn(&db, "sess-1") >= 2);

    append(&wt.join("src/lib.rs"), TURN1);
    worker.send(turn_job("sess-wt", &wt, 1, "add the twice helper"));
    wait_until("the worktree turn", || last_turn(&db, "sess-wt") >= 1);

    let ctx = |native: &str, workdir: &Path| SessionCtx {
        native: native.into(),
        codebase: Some("proj".into()),
        workdir: Some(workdir.to_path_buf()),
    };
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver {
        codebases: vec![("proj".into(), repo.clone())],
        sessions: vec![
            ("sess-1".into(), ctx("sess-1", &repo)),
            ("sess-wt".into(), ctx("sess-wt", &wt)),
        ],
    });
    let service = CodeService::live(core_for(&db), Arc::clone(&resolver));
    Fixture {
        _dir: dir,
        repo,
        wt,
        db,
        worker,
        service,
        resolver,
    }
}

fn ledger_has(db: &Path, session: &str, n: u32) -> bool {
    aivoryd::ledger::with_ledger(db, |l| {
        Ok(l.list_turns(session)
            .map_err(|e| e.to_string())?
            .iter()
            .any(|t| t.n == n))
    })
    .unwrap_or(false)
}

impl Fixture {
    fn call<T: serde::de::DeserializeOwned>(&self, method: &str, params: Value) -> T {
        let v = self
            .service
            .dispatch(method, params)
            .unwrap_or_else(|e| panic!("{method}: {e}"));
        serde_json::from_value(v).unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    fn error(&self, method: &str, params: Value) -> i64 {
        self.service
            .dispatch(method, params)
            .expect_err("an error")
            .code
    }
}

#[test]
fn a_live_repo_answers_file_references_and_blame_by_prompt() {
    let f = fixture();
    let _keep = &f.worker;

    // code/file: the text, the highlights of the viewport only, the outline.
    let file: CodeFile = f.call(
        "code/file",
        json!({"codebase": "proj", "path": "src/lib.rs", "lines": {"from": 1, "to": 3}}),
    );
    assert_eq!(file.language, "rust");
    assert_eq!(file.text, format!("{BASE}{TURN1}{TURN2}"));
    assert!(file.highlights.iter().all(|h| (1..=3).contains(&h.line)));
    assert!(file.highlights.iter().any(|h| h.line == 1
        && h.start == 0
        && h.end == 3
        && matches!(h.kind, aivory_protocol::code::TokenKind::Keyword)));
    let names: Vec<&str> = file.outline.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["helper", "run", "twice", "parse"]);
    assert_eq!(file.outline[0].line, 1);
    assert_eq!(file.outline[0].end_line, 3);

    // The file at the end of turn 1 has no `parse`.
    let old: CodeFile = f.call(
        "code/file",
        json!({"codebase": "proj", "path": "src/lib.rs", "session": "sess-1", "rev": "turn:1"}),
    );
    assert_eq!(old.text, format!("{BASE}{TURN1}"));
    assert_eq!(old.outline.len(), 3);

    // code/references of `helper` (the call in `run`, line 6): the index answers.
    let refs: PlacePage = f.call(
        "code/references",
        json!({"codebase": "proj", "path": "src/lib.rs", "line": 6, "col": 6}),
    );
    assert_eq!(refs.name, "helper");
    assert_eq!(refs.source, CodeSource::Index);
    assert!(!refs.indexing);
    let lines: Vec<u32> = refs.items.iter().map(|p| p.line).collect();
    assert!(lines.contains(&6) && lines.contains(&10), "{lines:?}");
    assert!(refs.items.iter().all(|p| p.path == "src/lib.rs"));
    let call_in_twice = refs.items.iter().find(|p| p.line == 10).unwrap();
    assert!(call_in_twice.preview.contains("helper(helper(x))"));
    assert_eq!(call_in_twice.len, Some(6));
    // A session reads its own delta (turn 2 updated it): the same answer.
    let with_session: PlacePage = f.call(
        "code/references",
        json!({"codebase": "proj", "path": "src/lib.rs", "line": 6, "col": 6, "session": "sess-1"}),
    );
    assert_eq!(with_session.total, refs.total);
    // Paging.
    let page2: PlacePage = f.call(
        "code/references",
        json!({"codebase": "proj", "path": "src/lib.rs", "line": 6, "col": 6, "perPage": 1, "page": 2}),
    );
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.total, refs.total);

    // code/definition and code/hover.
    let def: PlacePage = f.call(
        "code/definition",
        json!({"codebase": "proj", "path": "src/lib.rs", "line": 6, "col": 6}),
    );
    assert_eq!(def.items.len(), 1);
    assert_eq!((def.items[0].line, def.items[0].col), (1, 7));
    assert!(def.items[0].preview.starts_with("pub fn helper"));
    let hover: aivory_protocol::code::Hover = f.call(
        "code/hover",
        json!({"codebase": "proj", "path": "src/lib.rs", "line": 6, "col": 6}),
    );
    assert_eq!(hover.name, "helper");
    assert!(hover.text.contains("pub fn helper(x: u32) -> u32"));

    // ledger/blame: the committed lines, then turn 1, then turn 2.
    let blame: FileBlame = f.call(
        "ledger/blame",
        json!({"codebase": "proj", "path": "src/lib.rs", "session": "sess-1"}),
    );
    assert_eq!(blame.hunks.len(), 3, "{:?}", blame.hunks);
    let first = &blame.hunks[0];
    assert_eq!((first.line, first.count), (1, 7));
    assert_eq!(first.commit.as_ref().unwrap().summary, "first");
    assert!(first.turn.is_none());
    let (h1, h2) = (&blame.hunks[1], &blame.hunks[2]);
    assert!(h1.commit.is_none() && h2.commit.is_none());
    let (t1, t2) = (h1.turn.as_ref().unwrap(), h2.turn.as_ref().unwrap());
    assert_eq!((t1.n, t1.prompt.as_str()), (1, "add the twice helper"));
    assert_eq!(
        (t2.n, t2.prompt.as_str()),
        (2, "parse a number from a string")
    );
    assert_eq!(t1.session, "sess-1");
    assert_eq!(t1.model, "opus-5.5");
    assert_eq!(t1.time, 1_792_593_090_000);
    assert_eq!(t2.cost, Some(1.0));
    assert_eq!(first.count + h1.count + h2.count, 15);

    // ledger/turns and ledger/timeline.
    let turns: TurnList = f.call("ledger/turns", json!({"session": "sess-1"}));
    assert_eq!(
        turns.turns.iter().map(|t| t.n).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(turns.turns[2].prompt, "parse a number from a string");
    assert_eq!(turns.turns[2].files[0].path, "src/lib.rs");
    assert!(turns.turns[1].snapshot);
    assert_eq!(turns.turns[1].ended_at, 1_792_593_090_000);
    let timeline: Timeline = f.call(
        "ledger/timeline",
        json!({"codebase": "proj", "path": "src/lib.rs", "session": "sess-1"}),
    );
    let kinds: Vec<&str> = timeline
        .events
        .iter()
        .map(|e| match e {
            TimelineEvent::Commit { .. } => "commit",
            TimelineEvent::Turn { .. } => "turn",
        })
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "commit").count(), 1);
    assert_eq!(kinds.iter().filter(|k| **k == "turn").count(), 2);

    // Errors.
    assert_eq!(
        f.error("code/file", json!({"codebase": "nope", "path": "a.rs"})),
        code::NOT_FOUND
    );
    assert_eq!(
        f.error(
            "code/file",
            json!({"codebase": "proj", "path": "../etc/passwd"})
        ),
        code::INVALID_PARAMS
    );
    assert_eq!(
        f.error(
            "code/file",
            json!({"codebase": "proj", "path": "src/none.rs"})
        ),
        code::NOT_FOUND
    );
    assert_eq!(
        f.error("ledger/turns", json!({"session": "ghost"})),
        code::SESSION_NOT_FOUND
    );
    assert_eq!(
        f.error(
            "code/hover",
            json!({"codebase": "proj", "path": "src/lib.rs", "line": 99, "col": 0})
        ),
        code::INVALID_PARAMS
    );

    // The service wrote nothing to the repo: HEAD and the status stay.
    assert_eq!(git(&f.repo, &["log", "--oneline"]).lines().count(), 1);
}

#[test]
fn the_diagnostics_of_a_turn_are_stored_before_and_after() {
    let f = fixture();
    // The pipeline runs in the background after turn 2 and sets the counts of the turn row.
    wait_until("the findings of turn 2", || {
        aivoryd::ledger::with_ledger(&f.db, |l| {
            Ok(l.get_turn("sess-1", 2)
                .map_err(|e| e.to_string())?
                .is_some_and(|t| t.diagnostics_added.is_some()))
        })
        .unwrap_or(false)
    });
    let d2: DiagnosticsResult = f.call(
        "code/diagnostics",
        json!({"codebase": "proj", "session": "sess-1", "turn": 2}),
    );
    assert_eq!(d2.turn, Some(2));
    assert_eq!(d2.added.len(), 1, "{:?}", d2.added);
    assert_eq!(d2.added[0].rule, "aivory/risky-unwrap");
    assert_eq!(d2.added[0].path, "src/lib.rs");
    assert_eq!(d2.added[0].line, 14);
    assert_eq!(d2.added[0].tool, "aivory-rules");
    assert!(d2.removed.is_empty());
    assert_eq!(d2.rows.len(), 1);

    // The last turn is the default, and the file path filters.
    let latest: DiagnosticsResult = f.call(
        "code/diagnostics",
        json!({"codebase": "proj", "session": "sess-1", "path": "src/lib.rs"}),
    );
    assert_eq!(latest.turn, Some(2));
    let other: DiagnosticsResult = f.call(
        "code/diagnostics",
        json!({"codebase": "proj", "session": "sess-1", "path": "src/other.rs"}),
    );
    assert!(other.rows.is_empty());

    // Turn 1 added no finding. The turn row has the counts of both turns.
    let d1: DiagnosticsResult = f.call(
        "code/diagnostics",
        json!({"codebase": "proj", "session": "sess-1", "turn": 1}),
    );
    assert!(d1.added.is_empty() && d1.rows.is_empty());
    let turns: TurnList = f.call("ledger/turns", json!({"session": "sess-1"}));
    assert_eq!(turns.turns[2].diagnostics_added, Some(1));
    assert_eq!(turns.turns[1].diagnostics_added, Some(0));

    // code/file shows the stored findings of the file.
    let file: CodeFile = f.call(
        "code/file",
        json!({"codebase": "proj", "path": "src/lib.rs", "session": "sess-1"}),
    );
    assert_eq!(file.diagnostics.len(), 1);
    assert_eq!(file.diagnostics[0].line, 14);

    // Without a session the fast stages run on the changed files of the checkout.
    let live: DiagnosticsResult = f.call("code/diagnostics", json!({"codebase": "proj"}));
    assert_eq!(live.turn, None);
    assert!(live.rows.iter().any(|r| r.rule == "aivory/risky-unwrap"));
    assert_eq!(
        f.error("code/diagnostics", json!({"codebase": "proj", "turn": 1})),
        code::INVALID_PARAMS
    );
    assert_eq!(
        f.error(
            "code/diagnostics",
            json!({"codebase": "proj", "session": "sess-1", "turn": 9})
        ),
        code::NOT_FOUND
    );
}

#[test]
fn a_revert_restores_only_a_linked_worktree() {
    let f = fixture();
    // The session in the main checkout: refused, and the checkout stays as it is.
    let before = std::fs::read_to_string(f.repo.join("src/lib.rs")).unwrap();
    assert_eq!(
        f.error("ledger/revert", json!({"session": "sess-1", "turn": 2})),
        code::INVALID_STATE
    );
    assert_eq!(
        std::fs::read_to_string(f.repo.join("src/lib.rs")).unwrap(),
        before
    );

    // The session in the worktree: the file returns to the state before turn 1.
    assert!(
        std::fs::read_to_string(f.wt.join("src/lib.rs"))
            .unwrap()
            .contains("twice")
    );
    let result: aivory_protocol::code::RevertResult =
        f.call("ledger/revert", json!({"session": "sess-wt", "turn": 1}));
    assert_eq!(result.restored, 0);
    assert_eq!(result.snapshot.len(), 12);
    // Git on Windows can write CRLF.
    let restored = std::fs::read_to_string(f.wt.join("src/lib.rs")).unwrap();
    assert_eq!(restored.replace("\r\n", "\n"), BASE);
    // The main checkout is untouched.
    assert_eq!(
        std::fs::read_to_string(f.repo.join("src/lib.rs")).unwrap(),
        before
    );
    // The ledger has an operation for it.
    let ops = aivoryd::ledger::with_ledger(&f.db, |l| {
        Ok(l.list_ops("sess-wt").map_err(|e| e.to_string())?.len())
    })
    .unwrap();
    assert!(ops >= 1);
    // The baseline cannot be undone.
    assert_eq!(
        f.error("ledger/revert", json!({"session": "sess-wt", "turn": 0})),
        code::INVALID_PARAMS
    );
}

#[test]
fn symbols_and_a_rename_preview_come_from_the_index() {
    let f = fixture();
    let symbols: aivory_protocol::code::SymbolPage =
        f.call("code/symbols", json!({"codebase": "proj", "query": "tw"}));
    assert_eq!(symbols.items.len(), 1);
    assert_eq!(symbols.items[0].name, "twice");
    assert_eq!(symbols.items[0].line, 9);
    let none: aivory_protocol::code::SymbolPage = f.call(
        "code/symbols",
        json!({"codebase": "proj", "query": "helper", "kind": "struct"}),
    );
    assert!(none.items.is_empty());

    let preview: aivory_protocol::code::RenamePreview = f.call(
        "code/rename",
        json!({"codebase": "proj", "path": "src/lib.rs", "line": 6, "col": 6, "newName": "assist"}),
    );
    assert_eq!(preview.old_name, "helper");
    assert!(!preview.exact);
    assert_eq!(preview.files.len(), 1);
    let edits = &preview.files[0].edits;
    // The declaration and three uses.
    assert_eq!(edits.len(), 4, "{edits:?}");
    assert_eq!((edits[0].line, edits[0].col, edits[0].end_col), (1, 7, 13));
    assert!(edits.iter().all(|e| e.new_text == "assist"));
    // The files stay as they are.
    assert!(
        std::fs::read_to_string(f.repo.join("src/lib.rs"))
            .unwrap()
            .contains("helper(1)")
    );
    assert_eq!(
        f.error(
            "code/rename",
            json!({"codebase": "proj", "path": "src/lib.rs", "line": 6, "col": 6, "newName": "1bad"}),
        ),
        code::INVALID_PARAMS
    );
}

// ---------------------------------------------------------------------------------------------
// The MCP tools

fn mcp(port: u16, token: &str, body: &Value) -> Value {
    let body = body.to_string();
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let json = resp.split_once("\r\n\r\n").unwrap().1;
    serde_json::from_str(json).unwrap()
}

fn tool(port: u16, token: &str, name: &str, args: Value) -> (String, bool) {
    let v = mcp(
        port,
        token,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}),
    );
    (
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
        v["result"]["isError"].as_bool().unwrap(),
    )
}

#[test]
fn the_agent_tools_answer_in_the_compact_format() {
    let f = fixture();
    let server = McpServer::start(|_| false).unwrap();
    server.set_tools(Arc::new(CodeTools::new(
        core_for(&f.db),
        Arc::clone(&f.resolver),
    )));
    // The agent runs in the repo: the codebase comes from the folder.
    server.set_cwd(&f.repo);
    let (port, token) = (server.port(), server.token().to_string());

    let listed = mcp(
        port,
        &token,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "permission",
            "references",
            "definition",
            "symbols",
            "outline",
            "diagnostics"
        ]
    );

    // The format of design 03: a title with the count, one line per result.
    let (text, error) = tool(port, &token, "references", json!({"symbol": "helper"}));
    assert!(!error, "{text}");
    let mut lines = text.lines();
    let head = lines.next().unwrap();
    assert!(
        head.starts_with("references(sym: \"helper\")  →  3 results"),
        "{head}"
    );
    assert!(text.contains("src/lib.rs:6"));
    assert!(text.contains("helper(helper(x))"));
    assert!(!text.contains("more, use page"));

    let (text, _) = tool(port, &token, "definition", json!({"symbol": "helper"}));
    assert!(
        text.starts_with("definition(sym: \"helper\")  →  1 result\n"),
        "{text}"
    );
    assert!(text.contains("src/lib.rs:1"));
    assert!(text.contains("pub fn helper(x: u32) -> u32"));

    // The index can still apply the last turn on a slow runner: ask until it answers.
    wait_until("symbols finds parse", || {
        let (text, _) = tool(port, &token, "symbols", json!({"query": "pa"}));
        text.contains("src/lib.rs:13") && text.contains("pub fn parse")
    });

    let (text, _) = tool(port, &token, "outline", json!({"path": "src/lib.rs"}));
    assert!(
        text.starts_with("outline(path: \"src/lib.rs\")  →  4 items\n"),
        "{text}"
    );
    assert!(text.contains("  function twice  :9"));

    let (text, _) = tool(port, &token, "diagnostics", json!({"path": "src/lib.rs"}));
    assert!(text.contains("aivory/risky-unwrap"), "{text}");
    assert!(
        text.contains("src/lib.rs:14:") || text.contains("src/lib.rs:15:"),
        "{text}"
    );

    // Errors are error results with a reason, and the tools stay inside the codebase.
    let (text, error) = tool(port, &token, "references", json!({}));
    assert!(error && text.contains("`symbol`"), "{text}");
    let (text, error) = tool(port, &token, "outline", json!({"path": "../secret.rs"}));
    assert!(error, "{text}");
    let (_, error) = tool(
        port,
        &token,
        "symbols",
        json!({"query": "x", "kind": "nonsense"}),
    );
    assert!(error);
    // The tools changed nothing.
    assert_eq!(git(&f.repo, &["status", "--porcelain"]).lines().count(), 1);
}
