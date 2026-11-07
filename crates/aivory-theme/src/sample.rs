//! A small valid theme, as plain files. `aivoryd theme init` writes it; the tests use it as a
//! synthetic fixture.

use crate::validate::SCREENS;

/// The files of a minimal good theme with this id and version: `(relative path, bytes)`.
pub fn files(id: &str, version: &str) -> Vec<(String, Vec<u8>)> {
    let manifest = format!(
        "id = \"{id}\"\nname = \"{id}\"\ndescription = \"A starter theme.\"\nversion = \"{version}\"\n\
         license = \"MIT\"\nmin_app = \"0.1.0\"\ncatalog = 1\ndisplays = [\"desktop\"]\ntokens = [\"tokens.toml\"]\n\n\
         [author]\nname = \"Your Name\"\n"
    );
    let tokens = r##"[colors]
bg = "#ffffff"
surface = "#f4f5f7"
text = "#15161b"
muted = "#6b7280"
accent = "#2563eb"
analysis = "#7c3aed"

[colors.state]
needsYou = "#d97706"
working = "#2563eb"
idle = "#6b7280"
parked = "#9ca3af"
error = "#dc2626"
done = "#16a34a"

[fonts]
ui = "sans"
mono = "mono"

[type]
xs = 11
sm = 12
md = 14
lg = 18
xl = 24

[space]
sm = 8
md = 12

[radii]
card = 8

[motion]
easing = "easeOut"

[motion.durations]
fast = 120
"##;
    let mut out = vec![
        ("theme.toml".to_string(), manifest.into_bytes()),
        ("tokens.toml".to_string(), tokens.as_bytes().to_vec()),
        (
            "strings/en.toml".to_string(),
            b"title = \"Fleet\"\n".to_vec(),
        ),
        ("README.md".to_string(), format!("# {id}\n").into_bytes()),
        ("LICENSE".to_string(), b"MIT License\n".to_vec()),
    ];
    for screen in SCREENS {
        let doc = format!(
            "{{\n  \"root\": {{ \"type\": \"panel\", \"id\": \"{screen}\", \"title\": \"$strings.title\", \"children\": [\n    {{ \"type\": \"text\", \"value\": \"$daemon.version\" }}\n  ] }}\n}}\n"
        );
        out.push((format!("layouts/desktop/{screen}.json"), doc.into_bytes()));
    }
    out
}

/// A full theme: layouts for all 8 screens of the desktop class that use the catalog, a strings
/// table, and the tokens of `files` with more space names. The desktop tests and the
/// end-to-end run use it. `aivoryd theme init` still writes the small starter.
pub fn full_files(id: &str, version: &str) -> Vec<(String, Vec<u8>)> {
    use serde_json::{Value, json};

    fn top() -> Value {
        json!({ "type": "stack", "dir": "row", "gap": "md", "align": "center", "children": [
            { "type": "text", "value": "$strings.brand", "size": "lg", "weight": "bold" },
            { "type": "nav", "dir": "row" },
            { "type": "spacer" },
            { "type": "look_switch" },
            { "type": "badge", "value": "$daemon.version", "tone": "muted" }
        ] })
    }
    fn page(children: Vec<Value>) -> Value {
        let mut all = vec![top()];
        all.extend(children);
        json!({ "type": "stack", "dir": "column", "gap": "md", "pad": "md", "grow": 1, "children": all })
    }
    fn press(action: &str, extra: Value) -> Value {
        let mut a = json!({ "action": action });
        if let (Some(o), Some(e)) = (a.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                o.insert(k.clone(), v.clone());
            }
        }
        json!({ "press": a })
    }
    fn focus() -> Value {
        press("focus", json!({ "id": "$item.id" }))
    }
    fn card(lines: u32) -> Value {
        json!({ "type": "card", "title": "$item.title", "sub": "$item.codebase",
            "state": "$item.state", "on": focus(), "children": [
            { "type": "text", "value": "$item.prompt", "lines": lines, "size": "sm", "color": "muted" },
            { "type": "stack", "dir": "row", "gap": "sm", "align": "center", "children": [
                { "type": "badge", "state": "$item.state" },
                { "type": "text", "value": "$item.model", "size": "xs", "color": "muted" }
            ] }
        ] })
    }
    fn row() -> Value {
        json!({ "type": "row", "title": "$item.title", "sub": "$item.codebase",
            "meta": "$item.model", "state": "$item.state", "on": focus() })
    }
    fn readout(label: &str, path: &str, color: &str) -> Value {
        json!({ "type": "readout", "label": label, "value": path, "color": color, "grow": 1 })
    }
    fn waiting(free: bool) -> Value {
        json!({ "type": "panel", "variant": "glass", "title": "$strings.waiting", "grow": 1,
            "children": [ { "type": "decision_queue", "grow": 1, "free_text": free } ] })
    }

    let sessions_panel = json!({ "type": "panel", "title": "$strings.sessions", "grow": 1,
        "children": [ { "type": "sessions", "grow": 1, "gap": "sm", "tiers": {
            "1": { "layout": "grid", "template": card(4) },
            "2-4": { "layout": "grid", "template": card(3) },
            "5-12": { "layout": "grid", "template": card(2) },
            "13-30": { "layout": "list", "template": row() },
            "31+": { "layout": "list", "template": row() }
        } } ] });
    let side = |wide: bool| {
        let gauge = json!({ "type": "gauge", "label": "$item.label", "value": "$item.value",
            "unit": "$item.unit" });
        let analysis = json!({ "type": "panel", "role": "analysis",
            "when": { "path": "$analysis", "is": "nonEmpty" },
            "children": [ { "type": "repeat", "items": "$analysis", "gap": "sm",
                "children": [ gauge ] } ] });
        let mut s = json!({ "type": "stack", "dir": "column", "gap": "md",
            "children": [ waiting(false), analysis ] });
        if wide {
            s["width"] = json!(380);
        } else {
            s["height"] = json!(320);
        }
        s
    };
    let warroom = page(vec![
        json!({ "type": "stack", "dir": "row", "gap": "md", "children": [
            readout("Needs you", "$gates.needsYou.count", "state.needsYou"),
            readout("Working", "$gates.working.count", "state.working"),
            readout("Blocked", "$gates.blocked.count", "state.error"),
            readout("Idle", "$gates.idle.count", "state.idle"),
            readout("Inbox", "$inbox.open", "text"),
            readout("Queued", "$tasks.queued", "text")
        ] }),
        json!({ "type": "responsive", "grow": 1, "classes": {
            "compact": { "type": "stack", "dir": "column", "gap": "md", "grow": 1,
                "children": [ sessions_panel.clone(), side(false) ] },
            "regular": { "type": "stack", "dir": "row", "gap": "md", "grow": 1,
                "children": [ sessions_panel, side(true) ] }
        } }),
        json!({ "type": "panel", "title": "Activity", "children": [
            { "type": "chart", "kind": "stacked", "items": "$events", "height": 40 }
        ] }),
    ]);
    let button = |label: &str, action: &str, extra: Value| json!({ "type": "button", "label": label, "on": press(action, extra) });
    let mode = |label: &str, m: &str| {
        json!({ "type": "segment_button", "label": label,
            "on": press("permissionMode", json!({ "id": "$selected.id", "mode": m })) })
    };
    let chat = json!({ "type": "stack", "dir": "column", "gap": "md", "grow": 1, "children": [
        { "type": "panel", "title": "$selected.title", "grow": 1,
          "children": [ { "type": "transcript", "id": "chat", "grow": 1 } ] },
        { "type": "composer", "id": "reply", "placeholder": "Reply to the agent" }
    ] });
    let facts = json!({ "type": "panel", "title": "$strings.session", "children": [
        { "type": "readout", "label": "State", "value": "$selected.state" },
        { "type": "readout", "label": "Model", "value": "$selected.model" },
        { "type": "readout", "label": "Cost", "value": "$selected.cost", "unit": " USD" },
        { "type": "gauge", "label": "Context", "value": "$selected.contextPct", "unit": "%" }
    ] });
    let session = page(vec![
        json!({ "type": "stack", "dir": "row", "gap": "md", "grow": 1,
        "children": [
        chat.clone(),
        { "type": "stack", "dir": "column", "gap": "md", "width": 320, "children": [
            facts.clone(),
            { "type": "panel", "title": "Controls", "children": [
                { "type": "stack", "dir": "row", "wrap": true, "gap": "sm", "children": [
                    button("Pause", "pause", json!({ "id": "$selected.id" })),
                    button("Park", "park", json!({ "id": "$selected.id" })),
                    button("Resume", "resume", json!({ "id": "$selected.id" })),
                    button("Stop", "stop", json!({ "id": "$selected.id" }))
                ] },
                { "type": "stack", "dir": "row", "gap": "sm", "children": [
                    mode("Plan", "plan"), mode("Edit", "edit"), mode("Auto", "auto")
                ] }
            ] },
            { "type": "panel", "variant": "monitor", "title": "Terminal", "grow": 1,
              "children": [ { "type": "terminal", "grow": 1 } ] }
        ] }
    ] }),
    ]);
    let inbox = page(vec![json!({ "type": "panel", "variant": "glass",
        "title": "$strings.waiting", "grow": 1,
        "children": [ { "type": "decision_queue", "grow": 1, "free_text": true } ] })]);
    let tasks = page(vec![
        json!({ "type": "stack", "dir": "row", "gap": "md", "grow": 1,
        "children": [
        { "type": "stack", "dir": "column", "gap": "md", "width": 360, "children": [
            { "type": "panel", "title": "Queue", "children": [
                { "type": "readout", "label": "Queued tasks", "value": "$tasks.queued" },
                { "type": "composer", "id": "queue", "mode": "dispatch",
                  "placeholder": "Queue a task" },
                button("Turn the autopilot off", "autopilot", json!({ "on": false }))
            ] },
            { "type": "panel", "title": "Activity", "grow": 1,
              "children": [ { "type": "log", "items": "$events", "grow": 1 } ] }
        ] },
        { "type": "panel", "title": "$strings.sessions", "grow": 1, "children": [
            { "type": "list", "items": "$sessions", "grow": 1, "empty": "No sessions.",
              "children": [ row() ] }
        ] }
    ] }),
    ]);
    let repo_row = json!({ "type": "row", "title": "$item.name", "sub": "$item.branch",
        "meta": "$item.sessions" });
    let repos = page(vec![
        json!({ "type": "panel", "title": "$strings.repos", "grow": 1,
        "children": [ { "type": "tabs", "id": "repo-tabs", "grow": 1,
            "items": ["Repos", "Parts"], "children": [
            { "type": "list", "items": "$repos.items", "grow": 1, "empty": "No repos.",
              "children": [ repo_row ] },
            { "type": "repeat", "items": "$codebases", "dir": "row", "wrap": true, "gap": "sm",
              "children": [ { "type": "card", "title": "$item.name", "sub": "$item.path",
                "width": 260 } ] }
        ] } ] }),
    ]);
    let settings = page(vec![
        json!({ "type": "frame", "segments": ["Settings"], "grow": 1,
        "children": [ { "type": "settings_section", "grow": 1 } ] }),
    ]);
    let focus_screen = page(vec![
        json!({ "type": "stack", "dir": "row", "gap": "md", "grow": 1,
        "children": [
        chat,
        { "type": "stack", "dir": "column", "gap": "md", "width": 320, "children": [
            facts,
            { "type": "panel", "title": "Prompt", "children": [
                { "type": "diff", "value": "$selected.prompt" } ] },
            { "type": "panel", "variant": "monitor", "title": "Terminal", "grow": 1,
              "children": [ { "type": "terminal", "grow": 1 } ] }
        ] }
    ] }),
    ]);
    let decision = json!({ "type": "stack", "dir": "column", "gap": "md", "pad": "md", "children": [
        { "type": "text", "value": "$strings.decide", "size": "lg", "weight": "bold" },
        { "type": "text", "value": "$selected.title", "color": "muted" },
        { "type": "decision", "free_text": true },
        { "type": "key_hint", "keys": "Esc", "label": "Close" }
    ] });

    let mut out = files(id, version);
    out.retain(|(p, _)| !p.starts_with("layouts/") && p != "strings/en.toml");
    for (p, bytes) in &mut out {
        if p == "tokens.toml" {
            let t = String::from_utf8_lossy(bytes).replace(
                "[space]\nsm = 8\nmd = 12\n",
                "[space]\nxs = 4\nsm = 8\nmd = 12\nlg = 20\n",
            );
            *bytes = t.into_bytes();
        }
    }
    out.push((
        "strings/en.toml".to_string(),
        b"brand = \"AIVory Ledger\"\nwaiting = \"Waiting for you\"\nsessions = \"Sessions\"\n\
          session = \"Session\"\nrepos = \"Repos\"\ndecide = \"This needs your answer\"\n"
            .to_vec(),
    ));
    for (screen, doc) in [
        ("warroom", warroom),
        ("session", session),
        ("inbox", inbox),
        ("tasks", tasks),
        ("repos", repos),
        ("settings", settings),
        ("focus", focus_screen),
        ("decision", decision),
    ] {
        let text = serde_json::to_string_pretty(&json!({ "root": doc })).unwrap_or_default();
        out.push((format!("layouts/desktop/{screen}.json"), text.into_bytes()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full sample passes the validator. It writes itself to `AIVORY_SAMPLE_DIR` when the
    /// variable is set (the end-to-end run adds that folder with `aivoryd theme add`).
    #[test]
    fn the_full_sample_is_a_valid_theme() {
        let keep = std::env::var("AIVORY_SAMPLE_DIR").ok();
        let dir = keep.as_ref().map_or_else(
            || std::env::temp_dir().join(format!("aivory-sample-{}", std::process::id())),
            std::path::PathBuf::from,
        );
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, bytes) in full_files("ledger-sample", "1.0.0") {
            let path = dir.join(&rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let report = crate::validate::validate_dir(&dir, &crate::validate::Options::default());
        let issues: Vec<String> = report.issues.iter().map(ToString::to_string).collect();
        if keep.is_none() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        assert!(issues.is_empty(), "{issues:#?}");
    }
}
