//! The JSON Schema of the surface (`proto/schema/aivory-protocol.schema.json`) and the demo
//! fixtures (`proto/fixtures/`). The Flutter models are tested against the same files.

use schemars::generate::SchemaSettings;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::demo::{self, Demo};
use crate::rpc::{self, DeltaNote, WARROOM_DELTA};
use crate::{HistoryPage, RepoList, TaskList};

/// One JSON Schema (draft 2020-12) document: `methods` maps each method name to the schemas of
/// its params and result, `notifications` maps each notification to its params, and `$defs`
/// holds every type.
pub fn document() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let mut methods = Map::new();
    for (name, params, result) in rpc::method_schemas(&mut generator) {
        methods.insert(
            name.to_string(),
            json!({ "params": params, "result": result }),
        );
    }
    let delta = generator.subschema_for::<DeltaNote>();
    let defs = generator.take_definitions(true);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "AIVory Code protocol",
        "description": "JSON-RPC 2.0 surface of aivoryd. Generated from crates/aivory-protocol; do not edit.",
        "version": crate::PROTOCOL_VERSION,
        "methods": methods,
        "notifications": { WARROOM_DELTA: { "params": delta } },
        "$defs": defs,
    })
}

fn value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// The demo fixtures: file name in `proto/fixtures/` and the JSON of one method result.
pub fn fixtures(demo: &Demo) -> Vec<(&'static str, Value)> {
    let history = |id: &str| HistoryPage {
        session: id.to_string(),
        subagent: None,
        start: 0,
        total: demo.history(id, None).map_or(0, |l| l.len() as u32),
        lines: demo.history(id, None).cloned().unwrap_or_default(),
    };
    let repo = |id: &str| demo.repos.iter().find(|r| r.codebase.id == id);
    let map = |id: &str| demo.maps.iter().find(|m| m.codebase == id);
    vec![
        ("demo-snapshot.json", value(&demo.war_room)),
        ("demo-inbox.json", value(&demo.inbox)),
        (
            "demo-tasks.json",
            value(&TaskList {
                tasks: demo.tasks.clone(),
            }),
        ),
        (
            "demo-repos.json",
            value(&RepoList {
                codebases: demo.war_room.codebases.clone(),
            }),
        ),
        ("demo-repo-aivory-code.json", value(&repo("aivory-code"))),
        ("demo-map-aivory-code.json", value(&map("aivory-code"))),
        ("demo-settings.json", value(&demo.settings)),
        (
            "demo-session-a01c-spans.json",
            value(&demo.traces.get("a01c")),
        ),
        (
            "demo-session-a01c-diff.json",
            value(&demo.diffs.get("a01c")),
        ),
        ("demo-session-a01c-history.json", value(&history("a01c"))),
        (
            "demo-session-a7e2-diff.json",
            value(&demo.diffs.get("a7e2")),
        ),
        ("demo-ledger-turns.json", value(&demo::ledger_turns())),
        ("demo-ledger-blame.json", value(&demo::ledger_blame())),
        ("demo-ledger-timeline.json", value(&demo::ledger_timeline())),
        (
            "demo-code-diagnostics.json",
            value(&demo::ledger_diagnostics()),
        ),
        ("demo-claims.json", value(&demo::claims())),
        ("demo-radar.json", value(&demo::radar())),
        ("demo-mail-7f3a.json", value(&demo::mail("7f3a"))),
        (
            "demo-mail-5e81.json",
            value(&demo::mail(demo::BLOCKED_SESSION)),
        ),
    ]
}

/// `proto/schema/aivory-theme.schema.json`: the manifest, the tokens and the layout file of a
/// theme package. `$schema` in the files of a theme points at the parts.
pub fn theme_document() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "AIVory theme package",
        "description": "Generated from crates/aivory-protocol; do not edit.",
        "catalog": crate::theme::CATALOG_VERSION,
        "manifest": crate::theme::manifest_schema(),
        "tokens": crate::theme::tokens_schema(),
        "layout": crate::theme::layout_schema(),
    })
}

/// `proto/schema/aivory-registry.schema.json`.
pub fn registry_document() -> Value {
    crate::theme::registry_schema()
}
