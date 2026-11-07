//! Parser tests on synthetic JSON-lines fixtures that follow the shapes of the real CLIs.

use std::path::PathBuf;

use aivory_agents::{AgentEvent, Usage, parser_for};
use aivory_sessions::Provider;
use serde_json::json;

fn parse_fixture(provider: Provider, name: &str) -> Vec<AgentEvent> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    let text = std::fs::read_to_string(path).unwrap();
    let mut parser = parser_for(provider);
    text.lines().flat_map(|l| parser.parse_line(l)).collect()
}

fn mapped(events: &[AgentEvent]) -> Vec<&AgentEvent> {
    events
        .iter()
        .filter(|e| !matches!(e, AgentEvent::Raw { .. }))
        .collect()
}

fn raw_kinds(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Raw { kind, .. } => Some(kind.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn claude_stream_maps_to_agent_events() {
    let events = parse_fixture(Provider::Claude, "claude-stream.jsonl");
    let m = mapped(&events);
    assert_eq!(
        m[0],
        &AgentEvent::Started {
            session_id: Some("sess-claude-1".into()),
            model: Some("claude-test".into()),
            cwd: Some("C:\\synthetic\\app".into())
        }
    );
    assert_eq!(m[1], &AgentEvent::TextDelta { text: "Hel".into() });
    assert_eq!(m[2], &AgentEvent::TextDelta { text: "lo".into() });
    // The `assistant` text "Hello" repeats the deltas and is skipped; the tool_use is kept.
    assert_eq!(
        m[3],
        &AgentEvent::ToolCall {
            id: "toolu_1".into(),
            name: "Bash".into(),
            input: json!({"command": "echo hi"})
        }
    );
    assert_eq!(
        m[4],
        &AgentEvent::ToolResult {
            id: "toolu_1".into(),
            output: "hi".into(),
            is_error: false
        }
    );
    assert_eq!(
        m[5],
        &AgentEvent::ToolResult {
            id: "toolu_2".into(),
            output: "line one\nline two".into(),
            is_error: true
        }
    );
    // A message without deltas keeps its text.
    assert_eq!(
        m[6],
        &AgentEvent::TextDelta {
            text: "Done without deltas".into()
        }
    );
    assert_eq!(
        m[7],
        &AgentEvent::PermissionRequest {
            id: "toolu_9".into(),
            tool: "Write".into(),
            input: json!({"file_path": "x.txt"}),
            outcome: Some("denied".into())
        }
    );
    assert_eq!(
        m[8],
        &AgentEvent::TurnCompleted {
            usage: Some(Usage {
                input_tokens: 3,
                output_tokens: 5,
                cached_input_tokens: 7,
                reasoning_tokens: 4
            }),
            cost_usd: Some(0.0123),
            duration_ms: Some(1234)
        }
    );
    assert_eq!(m.len(), 9);
    assert!(events.last().unwrap().is_terminal());

    let raw = raw_kinds(&events);
    for expected in [
        "system/hook_started",
        "stream_event/message_start",
        "assistant/thinking",
        "rate_limit_event",
        "future_event_type",
        "non_json",
    ] {
        assert!(
            raw.iter().any(|k| k == expected),
            "raw kind {expected} missing in {raw:?}"
        );
    }
}

#[test]
fn codex_stream_maps_to_agent_events() {
    let events = parse_fixture(Provider::Codex, "codex-stream.jsonl");
    let m = mapped(&events);
    assert_eq!(
        m[0],
        &AgentEvent::Started {
            session_id: Some("thread-codex-1".into()),
            model: None,
            cwd: None
        }
    );
    assert_eq!(
        m[1],
        &AgentEvent::ToolCall {
            id: "item_1".into(),
            name: "command_execution".into(),
            input: json!({"command": "echo hi"})
        }
    );
    assert_eq!(
        m[2],
        &AgentEvent::ToolResult {
            id: "item_1".into(),
            output: "hi\n".into(),
            is_error: false
        }
    );
    // item_2 was never announced by item.started: the call is created on completion.
    assert!(matches!(m[3], AgentEvent::ToolCall { id, .. } if id == "item_2"));
    assert!(matches!(m[4], AgentEvent::ToolResult { id, is_error: true, .. } if id == "item_2"));
    assert!(matches!(m[5], AgentEvent::ToolCall { name, .. } if name == "file_change"));
    assert!(matches!(m[6], AgentEvent::ToolResult { output, .. } if output == "completed"));
    assert!(matches!(m[7], AgentEvent::ToolCall { name, .. } if name == "srv/lookup"));
    assert!(
        matches!(m[8], AgentEvent::ToolResult { output, is_error: false, .. } if output == "found")
    );
    assert_eq!(m[9], &AgentEvent::TextDelta { text: "OK".into() });
    assert_eq!(
        m[10],
        &AgentEvent::TurnCompleted {
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 2,
                cached_input_tokens: 4,
                reasoning_tokens: 1
            }),
            cost_usd: None,
            duration_ms: None
        }
    );
    assert_eq!(m.len(), 11);

    let raw = raw_kinds(&events);
    for expected in [
        "turn.started",
        "item.completed/reasoning",
        "item.completed/future_item_type",
        "future.event",
    ] {
        assert!(
            raw.iter().any(|k| k == expected),
            "raw kind {expected} missing in {raw:?}"
        );
    }
}

#[test]
fn codex_failure_reports_the_inner_api_message() {
    let events = parse_fixture(Provider::Codex, "codex-failed.jsonl");
    let errors: Vec<&AgentEvent> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Error { .. }))
        .collect();
    let text =
        "The 'synthetic-model' model is not supported when using Codex with a ChatGPT account.";
    assert_eq!(
        errors[0],
        &AgentEvent::Error {
            message: text.into(),
            fatal: false
        }
    );
    assert_eq!(
        errors[1],
        &AgentEvent::Error {
            message: text.into(),
            fatal: true
        }
    );
    assert_eq!(errors.len(), 2);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnCompleted { .. }))
    );
}

#[test]
fn gemini_stream_maps_to_agent_events() {
    let events = parse_fixture(Provider::Gemini, "gemini-stream.jsonl");
    let m = mapped(&events);
    assert_eq!(
        m[0],
        &AgentEvent::Started {
            session_id: Some("sess-gemini-1".into()),
            model: Some("gemini-test".into()),
            cwd: None
        }
    );
    assert_eq!(m[1], &AgentEvent::TextDelta { text: "O".into() });
    assert_eq!(m[2], &AgentEvent::TextDelta { text: "K".into() });
    assert_eq!(
        m[3],
        &AgentEvent::ToolCall {
            id: "tool-1".into(),
            name: "run_shell_command".into(),
            input: json!({"command": "echo hi"})
        }
    );
    assert_eq!(
        m[4],
        &AgentEvent::ToolResult {
            id: "tool-1".into(),
            output: "hi".into(),
            is_error: false
        }
    );
    assert_eq!(
        m[5],
        &AgentEvent::ToolResult {
            id: "tool-2".into(),
            output: "boom".into(),
            is_error: true
        }
    );
    assert_eq!(
        m[6],
        &AgentEvent::Error {
            message: "careful".into(),
            fatal: false
        }
    );
    assert_eq!(
        m[7],
        &AgentEvent::TurnCompleted {
            usage: Some(Usage {
                input_tokens: 6,
                output_tokens: 3,
                cached_input_tokens: 1,
                reasoning_tokens: 0
            }),
            cost_usd: None,
            duration_ms: Some(100)
        }
    );
    assert_eq!(m.len(), 8);
    let raw = raw_kinds(&events);
    assert!(raw.contains(&"message/user".to_string()) && raw.contains(&"future_event".to_string()));
}

#[test]
fn events_serialize_with_a_type_tag() {
    let v = serde_json::to_value(AgentEvent::TextDelta { text: "x".into() }).unwrap();
    assert_eq!(v, json!({"type": "text_delta", "text": "x"}));
    let v = serde_json::to_value(AgentEvent::Raw {
        provider: Provider::Codex,
        kind: "k".into(),
        data: json!(1),
    })
    .unwrap();
    assert_eq!(v["type"], "raw");
    assert_eq!(v["provider"], "codex");
}

#[test]
fn garbage_never_panics() {
    for provider in Provider::ALL {
        let mut p = parser_for(provider);
        for line in [
            "",
            "   ",
            "{",
            "[1,2]",
            "null",
            "{\"type\":5}",
            "{\"type\":\"result\"}",
            "{\"type\":\"item.completed\"}",
            "{\"type\":\"assistant\",\"message\":1}",
        ] {
            let _ = p.parse_line(line);
        }
    }
}
