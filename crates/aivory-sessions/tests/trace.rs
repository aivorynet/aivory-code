//! Fixture tests for `transcript::trace`: the span tree, times, durations, open calls,
//! sub-agent spans, the DAG, and malformed input. The fixtures in
//! `tests/fixtures/transcript` contain synthetic data only. Times are seconds after
//! `2026-03-01T10:00:00Z`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use aivory_protocol::{SessionState, SessionTrace, Span, SpanKind, layout};
use aivory_sessions::Provider;
use aivory_sessions::timeutil::parse_rfc3339_ms;
use aivory_sessions::transcript::trace;

fn fx(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("transcript")
        .join(rel)
}

/// Unix ms of `2026-03-01T10:00:00Z` plus `sec` seconds.
fn t(sec: i64) -> i64 {
    parse_rfc3339_ms("2026-03-01T10:00:00Z").unwrap() + sec * 1000
}

fn sec(ms: i64) -> i64 {
    (ms - t(0)) / 1000
}

/// `id Kind name <parent> start..end state tokens-in/tokens-out`, `open` for no end.
fn row(s: &Span) -> String {
    let end = s.end.map_or("open".to_string(), |e| sec(e).to_string());
    format!(
        "{} {:?} {} <{}> {}..{end} {} {}/{}",
        s.id,
        s.kind,
        s.name,
        s.parent.as_deref().unwrap_or("-"),
        sec(s.start),
        s.state.code(),
        s.tokens_in,
        s.tokens_out
    )
}

fn rows(tr: &SessionTrace) -> Vec<String> {
    tr.spans.iter().map(row).collect()
}

fn span<'a>(tr: &'a SessionTrace, id: &str) -> &'a Span {
    tr.spans.iter().find(|s| s.id == id).unwrap()
}

fn count(tr: &SessionTrace, kind: SpanKind) -> usize {
    tr.spans.iter().filter(|s| s.kind == kind).count()
}

/// `id agent 'title' state duration span`
fn node_rows(tr: &SessionTrace) -> Vec<String> {
    tr.nodes
        .iter()
        .map(|n| {
            format!(
                "{} {} '{}' {} {:?} {}",
                n.id,
                n.agent,
                n.title,
                n.state.code(),
                n.duration_ms,
                n.span.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

fn edge_rows(tr: &SessionTrace) -> Vec<String> {
    tr.edges
        .iter()
        .map(|e| format!("{}>{}", e.from, e.to))
        .collect()
}

const CLAUDE: &str =
    "trace/claude/projects/C--synthetic-app/cccccccc-0000-4000-8000-000000000001.jsonl";

fn claude_trace(rel: &str, now: i64) -> SessionTrace {
    trace(
        Provider::Claude,
        &fx(rel),
        "cccccccc-0000-4000-8000-000000000001",
        now,
    )
    .unwrap()
}

// ---- Claude ---------------------------------------------------------------------------------

#[test]
fn claude_span_tree_times_tokens_and_subagents() {
    let tr = claude_trace(CLAUDE, t(30));
    assert_eq!(tr.session, "cccccccc-0000-4000-8000-000000000001");
    // Parents first, siblings by start time. The malformed line is skipped.
    // m2 has three blocks (Agent, Read, Glob): one LLM call that ends at its last block.
    // The Glob call has no result in a closed turn: it ends with the turn, state parked.
    // The Agent call is replaced by the span of the sub-agent file that names its id.
    assert_eq!(
        rows(&tr),
        [
            "s1 Run session cccccccc <-> 0..open w 12100/245",
            "s2 Turn turn 1 · Fix the parser <s1> 0..20 d 7200/150",
            "s3 Llm chat <s2> 0..3 d 1000/20",
            "s4 Tool Bash <s2> 3..7 d 0/0",
            "s5 Llm chat <s2> 7..10 d 2000/40",
            "s6 Agent invoke Explore <s2> 9..16 d 1200/30",
            "s7 Llm chat <s6> 10..11 d 500/10",
            "s8 Tool Grep <s6> 11..13 d 0/0",
            "s9 Llm chat <s6> 13..16 d 700/20",
            "s10 Tool Read <s2> 10..12 e 0/0",
            "s11 Tool Glob <s2> 10..20 p 0/0",
            "s12 Llm chat <s2> 12..15 d 3000/60",
            "s13 Turn turn 2 · Add a test <s1> 20..open w 4900/95",
            "s14 Llm chat <s13> 20..22 d 4000/80",
            "s15 Agent invoke general-purpose <s13> 21..open w 900/15",
            "s16 Llm chat <s15> 21..23 d 900/15",
            "s17 Tool Bash <s15> 23..open w 0/0",
            "s18 Tool Bash <s13> 22..open w 0/0",
        ]
    );
    assert_eq!(count(&tr, SpanKind::Run), 1);
    assert_eq!(count(&tr, SpanKind::Turn), 2);
    assert_eq!(count(&tr, SpanKind::Agent), 2);
    assert_eq!(tr.stats.turns, 2);
    assert_eq!(tr.stats.subagents, 2);
    assert_eq!(tr.stats.tool_calls, 6);
    assert_eq!((tr.stats.cost, tr.stats.context_pct), (0.0, 0));
}

#[test]
fn claude_span_fields() {
    let tr = claude_trace(CLAUDE, t(30));
    let bash = span(&tr, "s4");
    assert_eq!(bash.tool.as_deref(), Some("bash"));
    assert_eq!(bash.input, "cargo test");
    assert_eq!(bash.output, "test result: ok");
    assert_eq!(bash.agent, "main");
    assert_eq!(bash.model, "claude-test-1");
    let read = span(&tr, "s10");
    assert_eq!(
        (
            read.tool.as_deref(),
            read.input.as_str(),
            read.output.as_str()
        ),
        (Some("file_read"), "src/a.rs", "boom")
    );
    assert_eq!(
        read.state,
        SessionState::Error,
        "is_error gives the error state"
    );
    let grep = span(&tr, "s8");
    assert_eq!(grep.agent, "Explore");
    assert_eq!(grep.output, "src/a.rs:1: parse\nsrc/b.rs:2: parse");
    let llm = span(&tr, "s3");
    assert_eq!(
        llm.input, "Fix the parser",
        "the first call of a turn shows the prompt"
    );
    assert_eq!(
        llm.output, "Bash",
        "a call without text shows its tool calls"
    );
    assert_eq!(span(&tr, "s5").input, "Bash result");
    assert_eq!(span(&tr, "s12").output, "Done with turn one.");
    let agent = span(&tr, "s6");
    assert_eq!(
        (
            agent.input.as_str(),
            agent.output.as_str(),
            agent.agent.as_str()
        ),
        ("Find usages", "Found 2 usages.", "Explore")
    );
    assert_eq!(span(&tr, "s2").output, "Done with turn one.");
    assert_eq!(
        span(&tr, "s1").output,
        "2 turns · 6 tool calls · 2 subagents"
    );
    assert!(tr.spans.iter().all(|s| s.cost == 0.0));
}

#[test]
fn claude_open_tool_call_has_no_end_and_works() {
    let tr = claude_trace(CLAUDE, t(30));
    for id in ["s13", "s17", "s18"] {
        let s = span(&tr, id);
        assert_eq!((s.end, s.state), (None, SessionState::Working), "{id}");
    }
    let open = span(&tr, "s18");
    assert_eq!(
        (open.input.as_str(), open.output.as_str()),
        ("cargo test -p x", "")
    );
    // The same call in a turn that is closed ends with the turn.
    let abandoned = span(&tr, "s11");
    assert_eq!(abandoned.end, Some(t(20)));
}

#[test]
fn claude_dag_has_the_main_agent_and_one_node_per_subagent() {
    let tr = claude_trace(CLAUDE, t(30));
    assert_eq!(
        node_rows(&tr),
        [
            "n1 main 'Fix the parser' w Some(30000) s1",
            "n2 Explore 'Find usages' d Some(7000) s6",
            "n3 general-purpose 'Write tests' w Some(9000) s15",
        ]
    );
    assert_eq!(edge_rows(&tr), ["n1>n2", "n1>n3"]);
    // The clients place the cards: one position for each node.
    assert_eq!(layout::dag(&tr.nodes, &tr.edges).len(), tr.nodes.len());
}

#[test]
fn claude_wire_format_has_no_positions_and_skips_empty_options() {
    let tr = claude_trace(CLAUDE, t(30));
    let json = serde_json::to_value(&tr).unwrap();
    let node = &json["nodes"][0];
    assert!(node.get("x").is_none() && node.get("y").is_none());
    assert!(
        json["spans"][0].get("parent").is_none(),
        "the run has no parent"
    );
    assert!(
        json["spans"][0].get("end").is_none(),
        "an open span has no end"
    );
}

#[test]
fn claude_subagent_states_come_from_meta_and_transcript() {
    // Four sub-agent files: a1 is finished and answers the `Agent` call of the main file, a2
    // stopped after a tool result, a3 waits for a tool result, and a4 ended on an API error.
    let rel = "claude/projects/C--synthetic-app/aaaaaaaa-0000-4000-8000-000000000001.jsonl";
    let tr = trace(Provider::Claude, &fx(rel), "aaaaaaaa", t(1000)).unwrap();
    let agents: Vec<String> = tr
        .spans
        .iter()
        .filter(|s| s.kind == SpanKind::Agent)
        .map(|s| {
            format!(
                "{} {}..{} {}",
                s.agent,
                sec(s.start),
                s.end.map_or("open".into(), |e| sec(e).to_string()),
                s.state.code()
            )
        })
        .collect();
    // Siblings come by start time; the finished agent starts at the time of its call.
    assert_eq!(
        agents,
        [
            "Explore 2..33 d",
            "general-purpose 10..12 p",
            "Plan 20..open w",
            "Explore 40..41 e"
        ]
    );
    // The tool call that the first agent answers is not a second span.
    assert_eq!(tr.stats.tool_calls, 3);
    assert_eq!(edge_rows(&tr), ["n1>n2", "n1>n3", "n1>n4", "n1>n5"]);
    let titles: Vec<&str> = tr.nodes.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(
        titles[1..],
        [
            "Find usages",
            "Write tests",
            "Plan the change",
            "Check docs"
        ]
    );
    // An agent that works keeps the turn and the run open, although the turn ended.
    assert_eq!(span(&tr, "s2").end, None);
    assert_eq!(tr.nodes[0].state, SessionState::Working);
}

#[test]
fn claude_old_layout_agent_pairs_with_its_call_and_is_not_a_second_span() {
    // The old layout has no meta file, so no `toolUseId`. The agent file pairs with the nearest
    // earlier `Task` call, and takes its type and task.
    let rel =
        "trace/claude/old/projects/C--synthetic-app/dddddddd-0000-4000-8000-000000000004.jsonl";
    let tr = trace(
        Provider::Claude,
        &fx(rel),
        "dddddddd-0000-4000-8000-000000000004",
        t(30),
    )
    .unwrap();
    assert_eq!(
        rows(&tr),
        [
            "s1 Run session dddddddd <-> 0..12 d 60/18",
            "s2 Turn turn 1 · Look around <s1> 0..12 d 60/18",
            "s3 Llm chat <s2> 0..2 d 10/5",
            "s4 Agent invoke Explore <s2> 2..6 d 30/7",
            "s5 Llm chat <s4> 4..6 d 30/7",
            "s6 Llm chat <s2> 3..12 d 20/6",
        ]
    );
    assert_eq!(tr.stats.subagents, 1);
    assert_eq!(tr.stats.tool_calls, 0);
    assert_eq!(node_rows(&tr)[1], "n2 Explore 'Look' d Some(4000) s4");
}

#[test]
fn claude_closed_session_ends_at_its_last_entry() {
    let tr = claude_trace("trace/claude/closed.jsonl", t(1000));
    assert_eq!(
        rows(&tr),
        [
            "s1 Run session cccccccc <-> 0..2 d 10/5",
            "s2 Turn turn 1 · Say hi <s1> 0..2 d 10/5",
            "s3 Llm chat <s2> 0..2 d 10/5",
        ]
    );
    assert_eq!(tr.nodes.len(), 1);
    assert!(tr.edges.is_empty());
    assert_eq!(tr.nodes[0].duration_ms, Some(2000));
}

#[test]
fn claude_unfinished_turn_is_open_only_while_the_file_is_fresh() {
    let fresh = claude_trace("trace/claude/stalled.jsonl", t(10));
    assert_eq!(fresh.spans[1].end, None);
    assert_eq!(fresh.spans[1].state, SessionState::Working);
    assert_eq!(
        fresh.nodes[0].duration_ms,
        Some(10_000),
        "an open node runs to now"
    );
    let old = claude_trace("trace/claude/stalled.jsonl", t(1000));
    assert_eq!(old.spans[1].end, Some(t(2)));
    assert_eq!(old.spans[1].state, SessionState::Parked);
    assert_eq!(old.spans[0].state, SessionState::Parked);
}

#[test]
fn claude_api_error_gives_the_error_state() {
    let tr = claude_trace("trace/claude/api_error.jsonl", t(1000));
    assert_eq!(
        rows(&tr),
        [
            "s1 Run session cccccccc <-> 0..4 e 10/5",
            "s2 Turn turn 1 · Say hi <s1> 0..4 e 10/5",
            "s3 Llm chat <s2> 0..2 d 10/5",
            "s4 Tool Bash <s2> 2..3 d 0/0",
        ]
    );
}

#[test]
fn claude_entries_without_times_use_the_file_time() {
    let path = fx("trace/claude/no_time.jsonl");
    let mtime = i64::try_from(
        fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let tr = trace(Provider::Claude, &path, "nt", mtime + 5000).unwrap();
    assert_eq!(tr.spans.len(), 3);
    for s in &tr.spans {
        assert_eq!((s.start, s.end), (mtime, Some(mtime)), "{}", s.id);
    }
    assert_eq!((tr.spans[2].tokens_in, tr.spans[2].tokens_out), (1, 2));
}

// ---- Codex ----------------------------------------------------------------------------------

#[test]
fn codex_span_tree_times_and_tokens() {
    let tr = trace(
        Provider::Codex,
        &fx("trace/codex/session.jsonl"),
        "cdx-t",
        t(30),
    )
    .unwrap();
    // A `token_count` event gives the tokens of the call before it. The malformed line is
    // skipped. The patch exits with code 1: error. The web search ends where it starts.
    assert_eq!(
        rows(&tr),
        [
            "s1 Run session cdx-t <-> 3..open w 6000/170",
            "s2 Turn turn 1 · Fix the bug <s1> 3..20 d 6000/170",
            "s3 Llm chat <s2> 3..6 d 1000/50",
            "s4 Tool shell_command <s2> 6..8 d 0/0",
            "s5 Llm chat <s2> 8..11 d 2000/80",
            "s6 Tool apply_patch <s2> 11..13 e 0/0",
            "s7 Llm chat <s2> 13..15 d 3000/40",
            "s8 Tool web_search <s2> 14..14 d 0/0",
            "s9 Turn turn 2 · Second prompt <s1> 20..open w 0/0",
            "s10 Llm chat <s9> 20..22 d 0/0",
            "s11 Tool shell_command <s9> 22..open w 0/0",
        ]
    );
    assert_eq!(tr.stats.turns, 2);
    assert_eq!(tr.stats.tool_calls, 4);
    assert_eq!(tr.stats.subagents, 0);
    assert_eq!(node_rows(&tr).len(), 1, "Codex has no sub-agent");
    assert!(tr.edges.is_empty());
}

#[test]
fn codex_span_fields() {
    let tr = trace(
        Provider::Codex,
        &fx("trace/codex/session.jsonl"),
        "cdx-t",
        t(30),
    )
    .unwrap();
    let shell = span(&tr, "s4");
    assert_eq!(
        (
            shell.tool.as_deref(),
            shell.input.as_str(),
            shell.output.as_str()
        ),
        (Some("bash"), "ls", "file.txt")
    );
    assert_eq!(shell.model, "gpt-test-1");
    let patch = span(&tr, "s6");
    assert_eq!(
        (
            patch.tool.as_deref(),
            patch.input.as_str(),
            patch.output.as_str()
        ),
        (Some("file_edit"), "src/a.rs", "patch failed")
    );
    assert_eq!(span(&tr, "s7").output, "All done.");
    assert_eq!(span(&tr, "s5").input, "shell_command result");
    assert_eq!(span(&tr, "s3").input, "Fix the bug");
    assert_eq!(span(&tr, "s8").input, "rust serde");
    assert_eq!(span(&tr, "s11").input, "sleep 100");
}

// ---- Gemini ---------------------------------------------------------------------------------

#[test]
fn gemini_jsonl_span_tree_times_and_tokens() {
    let tr = trace(
        Provider::Gemini,
        &fx("trace/gemini/session.jsonl"),
        "gem-t",
        t(30),
    )
    .unwrap();
    // The tokens in are `input + tool`, the tokens out are `output + thoughts`. A call ends at
    // its own time. A call with the status `executing` and no result is open. The malformed
    // line is skipped.
    assert_eq!(
        rows(&tr),
        [
            "s1 Run session gem-t <-> 2..open w 5505/70",
            "s2 Turn turn 1 · Fix the parser <s1> 2..20 d 3005/60",
            "s3 Llm chat <s2> 2..6 d 1005/40",
            "s4 Tool read_file <s2> 6..8 d 0/0",
            "s5 Tool run_shell_command <s2> 6..12 e 0/0",
            "s6 Llm chat <s2> 12..15 d 2000/20",
            "s7 Turn turn 2 · Run it again <s1> 20..open w 2500/10",
            "s8 Llm chat <s7> 20..24 d 2500/10",
            "s9 Tool run_shell_command <s7> 24..open w 0/0",
        ]
    );
    assert_eq!(tr.stats.turns, 2);
    assert_eq!(tr.stats.tool_calls, 3);
    let read = span(&tr, "s4");
    assert_eq!(
        (
            read.tool.as_deref(),
            read.input.as_str(),
            read.output.as_str()
        ),
        (Some("file_read"), "src/a.rs", "line1")
    );
    assert_eq!(span(&tr, "s5").output, "exit code 101");
    assert_eq!(span(&tr, "s6").input, "run_shell_command result");
    assert_eq!(span(&tr, "s3").model, "gemini-test-1");
    assert!(tr.edges.is_empty());
}

#[test]
fn gemini_document_clamps_a_call_that_ends_before_its_message() {
    let tr = trace(
        Provider::Gemini,
        &fx("gemini/session.json"),
        "gem-1",
        t(1000),
    )
    .unwrap();
    assert_eq!(
        tr.stats.turns, 2,
        "the injected session context is not a prompt"
    );
    assert_eq!(tr.stats.tool_calls, 6);
    for s in tr.spans.iter().filter(|s| s.kind == SpanKind::Tool) {
        assert_eq!(s.end, Some(s.start), "{}", s.id);
    }
    // The last message is an error: the turn and the run stopped on it.
    assert_eq!(tr.spans[0].state, SessionState::Error);
    assert!(tr.spans[0].end.is_some());
}

// ---- errors and size ------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aivory-trace-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn missing_and_invalid_files_are_errors_and_an_empty_file_is_a_parked_run() {
    let dir = TempDir::new("errors");
    assert!(trace(Provider::Claude, &dir.0.join("missing.jsonl"), "x", t(1)).is_err());
    let bad = dir.0.join("session-bad.json");
    fs::write(&bad, "{ \"messages\": [ ").unwrap();
    assert!(trace(Provider::Gemini, &bad, "x", t(1)).is_err());
    let empty = dir.0.join("empty.jsonl");
    fs::write(&empty, "").unwrap();
    for provider in Provider::ALL {
        let tr = trace(provider, &empty, "x", t(1)).unwrap();
        assert_eq!(tr.spans.len(), 1, "{provider:?}");
        assert_eq!(tr.spans[0].kind, SpanKind::Run);
        assert_eq!(tr.spans[0].state, SessionState::Parked);
        assert_eq!(tr.stats.turns, 0);
        assert_eq!(tr.nodes.len(), 1);
    }
}

#[test]
fn a_huge_tool_result_is_cut_to_300_characters() {
    let dir = TempDir::new("huge");
    let path = dir.0.join("huge.jsonl");
    let mut out = fs::File::create(&path).unwrap();
    writeln!(
        out,
        r#"{{"type":"user","timestamp":"2026-03-01T10:00:00.000Z","message":{{"role":"user","content":"Read the log"}}}}"#
    )
    .unwrap();
    writeln!(
        out,
        r#"{{"type":"assistant","timestamp":"2026-03-01T10:00:01.000Z","message":{{"id":"a","role":"assistant","model":"m","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"r1","name":"Read","input":{{"file_path":"big.log"}}}}]}}}}"#
    )
    .unwrap();
    writeln!(
        out,
        r#"{{"type":"user","timestamp":"2026-03-01T10:00:02.000Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"r1","content":"{}"}}]}}}}"#,
        "x".repeat(400_000)
    )
    .unwrap();
    drop(out);
    let tr = trace(Provider::Claude, &path, "huge", t(5)).unwrap();
    let read = tr.spans.iter().find(|s| s.kind == SpanKind::Tool).unwrap();
    assert_eq!(read.output.chars().count(), 300);
    assert_eq!(read.end, Some(t(2)));
}

/// A 50 MB Claude file must take less than 1 s in a release build. Run it with
/// `cargo test --release -p aivory-sessions --test trace -- --ignored`.
#[test]
#[ignore = "slow in a debug build"]
fn a_50_mb_claude_file_takes_less_than_a_second() {
    let dir = TempDir::new("fifty");
    let path = dir.0.join("fifty.jsonl");
    let mut out = std::io::BufWriter::new(fs::File::create(&path).unwrap());
    let filler = "y".repeat(2000);
    let mut n = 0u64;
    let mut size = 0usize;
    while size < 50 * 1024 * 1024 {
        let (m, s) = (n / 60 % 60, n % 60);
        let prompt = format!(
            r#"{{"type":"user","timestamp":"2026-03-01T11:{m:02}:{s:02}.000Z","message":{{"role":"user","content":"Prompt {n}"}}}}"#
        );
        let call = format!(
            r#"{{"type":"assistant","timestamp":"2026-03-01T11:{m:02}:{s:02}.100Z","message":{{"id":"m{n}","role":"assistant","model":"m","stop_reason":"tool_use","content":[{{"type":"tool_use","id":"t{n}","name":"Bash","input":{{"command":"ls {filler}"}}}}],"usage":{{"input_tokens":5,"cache_read_input_tokens":1000,"output_tokens":9}}}}}}"#
        );
        let result = format!(
            r#"{{"type":"user","timestamp":"2026-03-01T11:{m:02}:{s:02}.900Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{n}","content":"{filler}{filler}"}}]}}}}"#
        );
        let noise = format!(
            r#"{{"type":"api-request-blob","message":{{"role":"system","content":[{{"type":"text","text":"{filler}"}}]}}}}"#
        );
        for line in [prompt, call, result, noise] {
            size += line.len() + 1;
            writeln!(out, "{line}").unwrap();
        }
        n += 1;
    }
    drop(out);
    let started = Instant::now();
    let tr = trace(Provider::Claude, &path, "fifty", t(0)).unwrap();
    let took = started.elapsed();
    assert_eq!(count(&tr, SpanKind::Turn) as u64, n);
    assert_eq!(count(&tr, SpanKind::Tool) as u64, n);
    assert!(took.as_secs_f64() < 1.0, "took {took:?}");
}

// ---- Cache split ------------------------------------------------------------------------------

fn llm_cache(tr: &SessionTrace) -> Vec<(u64, Option<u64>, Option<u64>)> {
    tr.spans
        .iter()
        .filter(|s| s.kind == SpanKind::Llm)
        .map(|s| (s.tokens_in, s.tokens_cache_read, s.tokens_cache_write))
        .collect()
}

#[test]
fn claude_llm_spans_split_the_cache_counts() {
    let tr = claude_trace(CLAUDE, t(30));
    // m1: input 100 + write 50 + read 850. The usage block names both counts, 0 included.
    let first = &tr.spans[2];
    assert_eq!(
        (
            first.tokens_in,
            first.tokens_cache_read,
            first.tokens_cache_write
        ),
        (1000, Some(850), Some(50))
    );
    assert!(
        llm_cache(&tr)
            .iter()
            .all(|(tin, r, w)| r.unwrap_or(0) + w.unwrap_or(0) <= *tin)
    );
    // The fields stay out of the JSON when the transcript has no count.
    let json = serde_json::to_value(&tr.spans[1]).unwrap();
    assert!(json.get("tokensCacheRead").is_none());
}

#[test]
fn codex_llm_spans_split_the_cached_tokens() {
    let tr = trace(
        Provider::Codex,
        &fx("trace/codex/session.jsonl"),
        "codex-1",
        t(30),
    )
    .unwrap();
    let calls = llm_cache(&tr);
    // The first call has `cached_input_tokens`. The others have none: no split.
    assert_eq!(calls[0], (1000, Some(800), None));
    assert!(
        calls[1..]
            .iter()
            .all(|(_, r, w)| r.is_none() && w.is_none())
    );
}
