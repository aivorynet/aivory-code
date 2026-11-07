//! The demo fixtures and the JSON Schema in `proto/` must match the code. Run with
//! `AIVORY_BLESS=1` to write them after a change.

use std::path::PathBuf;

use aivory_protocol::coord::{ClaimList, ClaimState, MailList, RadarList};
use aivory_protocol::demo::Demo;
use aivory_protocol::rpc::{self, Delta, DeltaNote, Method, method};
use aivory_protocol::{
    HistoryPage, InboxList, Line, MapFile, RepoDetail, RepoList, SessionDiff, SessionState,
    SessionTrace, Settings, SettingsView, TaskList, WarRoom, schema,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn proto_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto")
}

fn pretty(v: &Value) -> String {
    let mut text = serde_json::to_string_pretty(&sorted(v)).unwrap();
    text.push('\n');
    text
}

/// Sort object keys. A workspace build with the desktop app turns on serde_json's
/// `preserve_order` (through GPUI), so the map order of `Value` alone depends on the build.
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

/// Compare a generated file with the file on disk, or write it with `AIVORY_BLESS=1`.
fn golden(relative: &str, v: &Value) {
    let path = proto_dir().join(relative);
    let want = pretty(v);
    if std::env::var_os("AIVORY_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
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

#[test]
fn fixtures_are_current() {
    for (name, v) in schema::fixtures(&Demo::new()) {
        golden(&format!("fixtures/{name}"), &v);
    }
}

#[test]
fn schema_is_current() {
    let doc = schema::document();
    for name in rpc::METHODS {
        assert!(doc["methods"][name].is_object(), "schema of {name}");
    }
    golden("schema/aivory-protocol.schema.json", &doc);
}

#[test]
fn theme_schemas_are_current() {
    golden("schema/aivory-theme.schema.json", &schema::theme_document());
    golden(
        "schema/aivory-registry.schema.json",
        &schema::registry_document(),
    );
}

/// Parse a fixture file into its type and serialize it again: the JSON must not change.
/// While `fixtures_are_current` writes the files (`AIVORY_BLESS`), use the generated JSON.
fn round_trip<T: Serialize + DeserializeOwned>(name: &str) -> T {
    let v: Value = if std::env::var_os("AIVORY_BLESS").is_some() {
        let all = schema::fixtures(&Demo::new());
        all.into_iter().find(|(n, _)| *n == name).unwrap().1
    } else {
        let text = std::fs::read_to_string(proto_dir().join("fixtures").join(name)).unwrap();
        serde_json::from_str(&text).unwrap()
    };
    let typed: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(serde_json::to_value(&typed).unwrap(), v, "{name}");
    typed
}

#[test]
fn fixtures_round_trip() {
    let snapshot: WarRoom = round_trip("demo-snapshot.json");
    assert_eq!(snapshot.sessions.len(), 24);
    assert_eq!(snapshot.codebases.len(), 10);
    assert_eq!(snapshot.activity.len(), 60);
    let c = snapshot.counts();
    assert_eq!(
        (c.needs_you, c.error, c.working, c.idle, c.quiet),
        (3, 1, 14, 3, 3)
    );
    assert_eq!(c.agents, 35);

    let inbox: InboxList = round_trip("demo-inbox.json");
    assert_eq!(inbox.items.len(), 6);
    let tasks: TaskList = round_trip("demo-tasks.json");
    assert_eq!(tasks.tasks.len(), 14);
    assert!(tasks.tasks.iter().all(|t| t.steps.len() == 5));
    round_trip::<RepoList>("demo-repos.json");
    round_trip::<RepoDetail>("demo-repo-aivory-code.json");
    let map: MapFile = round_trip("demo-map-aivory-code.json");
    assert!(map.source.contains("name  = \"orchestrator\""));
    round_trip::<SettingsView>("demo-settings.json");
    let trace: SessionTrace = round_trip("demo-session-a01c-spans.json");
    assert_eq!(trace.spans.len(), 21);
    let diff: SessionDiff = round_trip("demo-session-a01c-diff.json");
    assert_eq!(diff.files.len(), 4);
    let history: HistoryPage = round_trip("demo-session-a01c-history.json");
    assert_eq!(history.total as usize, history.lines.len());
    let blame: SessionDiff = round_trip("demo-session-a7e2-diff.json");
    assert!(blame.files[0].lines.iter().any(|l| l.blame.is_some()));

    // The radar frame of Lens.dc.html: four pairs with the scores 1.0, 0.7, 0.4, 0.1.
    let radar: RadarList = round_trip("demo-radar.json");
    let scores: Vec<f32> = radar.hits.iter().map(|h| h.score).collect();
    assert_eq!(scores, [1.0, 0.7, 0.4, 0.1]);
    assert_eq!(radar.sessions.len(), 5);
    let claims: ClaimList = round_trip("demo-claims.json");
    assert!(claims.claims.iter().any(|c| c.state == ClaimState::Intent));
    let mail: MailList = round_trip("demo-mail-7f3a.json");
    assert_eq!(mail.items.len(), 2);
}

#[test]
fn diff_numbers_follow_the_hunk_headers() {
    let demo = Demo::new();
    let budget = &demo.diffs["a01c"].files[0];
    let nums: Vec<(Option<u32>, Option<u32>)> =
        budget.lines.iter().map(|l| (l.old, l.new)).collect();
    // The numbers of the artboard: context 1/1, added 2, context 2/3, ..., removed 4, added 10.
    assert_eq!(
        &nums[..4],
        [
            (None, None),
            (Some(1), Some(1)),
            (None, Some(2)),
            (Some(2), Some(3))
        ]
    );
    assert_eq!(nums[10], (Some(4), None));
    assert_eq!(nums[11], (None, Some(10)));
}

#[test]
fn deltas_update_a_snapshot() {
    let demo = Demo::new();
    let mut view = demo.war_room.clone();
    let mut pane = view.session("7f3a").unwrap().clone();
    pane.state = SessionState::Working;
    pane.ask = None;
    view.apply(
        1,
        &Delta::Session {
            session: Box::new(pane),
        },
    );
    view.apply(
        2,
        &Delta::Lines {
            id: "7f3a".into(),
            subagent: None,
            lines: vec![
                Line::user("Allow"),
                Line::agent("OK. I continue with: allow."),
            ],
            total: 30,
        },
    );
    let s = view.session("7f3a").unwrap();
    assert_eq!(s.state, SessionState::Working);
    assert_eq!(s.tail.last().unwrap().text, "OK. I continue with: allow.");
    assert_eq!(s.lines_total, 30);
    assert_eq!(view.cursor, 2);
    assert_eq!(view.counts().needs_you, 2);

    let note = DeltaNote {
        seq: 3,
        delta: Delta::Counts {
            queued: 2,
            inbox_open: 5,
        },
    };
    let wire = serde_json::to_value(&note).unwrap();
    assert_eq!(
        wire,
        json!({ "seq": 3, "delta": { "type": "counts", "queued": 2, "inboxOpen": 5 } })
    );
    let back: DeltaNote = serde_json::from_value(wire).unwrap();
    view.apply(back.seq, &back.delta);
    assert_eq!((view.queued, view.inbox_open, view.cursor), (2, 5, 3));
}

#[test]
fn settings_set_by_dotted_key() {
    let mut s: Settings = Demo::new().settings.settings;
    s.set_key("autopilot.parkAfterMin", json!(22)).unwrap();
    assert_eq!(s.autopilot.park_after_min, 20, "snaps to the step of 5");
    s.set_key("autopilot.maxRunning", json!(500)).unwrap();
    assert_eq!(s.autopilot.max_running, 64);
    s.set_key("providers.claude.effort", json!("max")).unwrap();
    assert_eq!(s.providers["claude"].effort, aivory_protocol::Effort::Max);
    s.set_key("display.perRow", json!(4)).unwrap();
    assert_eq!(s.display.per_row, Some(4));
    s.set_key("display.perRow", Value::Null).unwrap();
    assert_eq!(s.display.per_row, None);
    s.set_key("notify.error.email", json!(false)).unwrap();
    assert!(!s.notify.error.email);
    assert!(s.set_key("display.nope", json!(1)).is_err());
    assert!(s.set_key("display.theme", json!("purple")).is_err());
}

#[test]
fn typed_requests_and_responses() {
    let req = rpc::request::<method::InboxAnswer>(
        7,
        &rpc::InboxAnswerParams {
            id: "in-7f3a".into(),
            option: Some("Allow".into()),
            text: None,
        },
    );
    assert_eq!(req["method"], "inbox/answer");
    assert_eq!(req["params"], json!({ "id": "in-7f3a", "option": "Allow" }));
    let ok = json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } });
    assert_eq!(
        rpc::parse_response::<method::SessionPause>(&ok).unwrap(),
        rpc::Ack::OK
    );
    let err =
        json!({ "jsonrpc": "2.0", "id": 7, "error": { "code": -32005, "message": "no task 'x'" } });
    assert_eq!(
        rpc::parse_response::<method::SessionPause>(&err)
            .unwrap_err()
            .code,
        rpc::code::NOT_FOUND
    );
    assert_eq!(method::WarroomSubscribe::NAME, "warroom/subscribe");
}

#[test]
fn a_span_belongs_to_its_turn() {
    let demo = Demo::new();
    let pane = demo.war_room.session("a7e2").unwrap();
    let trace = aivory_protocol::demo::generic_trace(pane, 0);
    assert_eq!(trace.turn_of_span("s2"), Some(1));
    assert_eq!(trace.turn_of_span("s3"), Some(2));
    assert_eq!(trace.turn_of_span("s1"), None, "the run span has no turn");
    assert_eq!(trace.turn_of_span("nope"), None);
    // A child span has the turn of its parent.
    let busy = demo
        .war_room
        .sessions
        .iter()
        .find(|s| !s.subagents.is_empty());
    let busy = aivory_protocol::demo::generic_trace(busy.unwrap(), 0);
    assert_eq!(busy.turn_of_span("s4"), Some(2));
}

#[test]
fn a_card_moves_only_to_an_allowed_column() {
    use aivory_protocol::TaskColumn::*;
    assert!(Planned.can_move_to(Working));
    assert!(Planned.can_move_to(Planned), "no move is fine");
    assert!(!Planned.can_move_to(Review));
    assert!(!Working.can_move_to(Planned));
    for from in aivory_protocol::TaskColumn::ALL {
        if from != Waiting {
            assert!(!from.can_move_to(Waiting), "{from:?} to Waiting");
        }
    }
    assert!(Done.can_move_to(Review));
    assert!(Planned.refusal(Review).contains("Working, Done"));
}

#[test]
fn the_lists_of_the_settings_have_their_own_methods() {
    let mut s: Settings = Demo::new().settings.settings;
    assert!(s.set_key("acp", json!([])).is_err());
    assert!(s.set_key("pinned", json!([])).is_err());
    // Old JSON has neither list.
    let mut v = serde_json::to_value(&s).unwrap();
    v.as_object_mut().unwrap().remove("acp");
    v.as_object_mut().unwrap().remove("pinned");
    let old: Settings = serde_json::from_value(v).unwrap();
    assert!(old.acp.is_empty() && old.pinned.is_empty());
}

#[test]
fn a_removed_codebase_leaves_the_view() {
    let mut view = Demo::new().war_room;
    let before = view.codebases.len();
    view.apply(1, &Delta::CodebaseRemoved { id: "docs".into() });
    assert_eq!(view.codebases.len(), before - 1);
    assert!(view.codebase("docs").is_none());
    let wire = serde_json::to_value(Delta::CodebaseRemoved { id: "x".into() }).unwrap();
    assert_eq!(wire, json!({ "type": "codebaseRemoved", "id": "x" }));
}

#[test]
fn the_daemon_info_of_an_older_daemon_has_no_host() {
    let info: rpc::DaemonInfo = serde_json::from_value(json!({
        "name": "aivoryd", "version": "0.1.0", "protocol": "jsonrpc-2.0/ndjson",
        "contract": "0.1.0", "pid": 1, "demo": false
    }))
    .unwrap();
    assert!(info.host.is_empty() && info.cwd.is_empty());
}
