//! Layout files (ADR 0024 sections 3 to 6): the parse and structure rules. The renderer comes
//! later; this module only decides whether a tree is allowed.
//!
//! A layout file is `{ "$schema"?: "...", "root": <node> }`. A node is an object with `type`
//! (a component of the catalog) or `{ "use": "<part>" }`. These keys of a node have a meaning:
//!
//! - `children`: a list of nodes. `fallback`: one node (required for scenes and effects).
//! - `when`: `{ "path": "$...", "is": "nonEmpty|empty|eq|ne|in", "value": <literal> }`.
//! - `on`: `{ "press": <action>, "key": <action with "combo"> }`. An action is
//!   `{ "action": "<name>", <argument>: <path or literal> }`.
//! - `sessions` has `tiers`: `{ "1" | "2-4" | "5-12" | "13-30" | "31+": { "layout": "list|grid|scene",
//!   "template": <node> } }`. The tier `13-30` is required and the tiers `13-30` and `31+` must use
//!   `"layout": "list"` (a scene shows at most 12 sessions).
//! - `responsive` has `classes`: `{ "compact" | "regular" | "wide": <node> }`.
//! - A `panel` with `"role": "analysis"` may use `$analysis.*`.
//!
//! Any other key is a property of the component. A string value that starts with `$` is a data
//! path, `$$` is a literal `$`, and `{ "path": "$...", "map": { ... }, "else": ... }` picks a
//! value from a table.

use std::collections::BTreeSet;

use aivory_protocol::theme::DisplayClass;
use serde_json::{Map, Value};

use crate::{Issue, caps, pointer_key};

pub const CATALOG: &[&str] = &[
    // Layout
    "stack",
    "grid",
    "layer",
    "scroll",
    "spacer",
    "repeat",
    "responsive",
    "sessions",
    // Frame
    "frame",
    "panel",
    "nav",
    "segment_button",
    "button",
    "switch",
    "tabs",
    "look_switch",
    "key_hint",
    // Content
    "text",
    "readout",
    "badge",
    "image",
    "list",
    "row",
    "card",
    "log",
    "files",
    "diff",
    // Sessions
    "decision",
    "decision_queue",
    "transcript",
    "terminal",
    "composer",
    "settings_section",
    // Charts
    "chart",
    "gauge",
    "ruler",
    // Effects
    "rain",
    "scramble",
    "scanlines",
    "rings",
    // Scenes
    "city3d",
    "fsn3d",
    "radar",
    "bloom",
    "camera_wall",
];
const SCENES: &[&str] = &["city3d", "fsn3d", "radar", "bloom", "camera_wall"];
const EFFECTS: &[&str] = &["rain", "scramble", "scanlines", "rings"];
/// The components of the `watch` class.
const WATCH: &[&str] = &[
    "stack", "list", "row", "text", "readout", "badge", "gauge", "decision", "composer",
];
pub const ACTIONS: &[&str] = &[
    "answer",
    "focus",
    "dispatch",
    "newSession",
    "send",
    "pause",
    "park",
    "stop",
    "resume",
    "autopilot",
    "permissionMode",
    "open",
    "setLook",
];
/// Actions that send text to an agent.
const TEXT_ACTIONS: &[&str] = &["send", "dispatch", "newSession", "answer"];
const TEXT_ARGS: &[&str] = &["text", "prompt", "message"];
/// The data roots of ADR 0024 section 3.
pub const ROOTS: &[&str] = &[
    "codebases",
    "sessions",
    "asks",
    "gates",
    "usage",
    "events",
    "inbox",
    "tasks",
    "repos",
    "daemon",
    "analysis",
    "intent",
    "selected",
    "display",
];
pub const TIERS: [&str; 5] = ["1", "2-4", "5-12", "13-30", "31+"];
const WIDTHS: [&str; 3] = ["compact", "regular", "wide"];
const TESTS: [&str; 5] = ["nonEmpty", "empty", "eq", "ne", "in"];
const RESERVED_KEYS: [&str; 3] = ["ctrl+shift+l", "esc", "escape"];

/// What the checks need to know about the package.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub class: DisplayClass,
    /// The names of the files in `parts/`.
    pub parts: &'a BTreeSet<String>,
    /// The keys of the strings tables that every language has. `None`: the package has none.
    pub strings: Option<&'a BTreeSet<String>>,
}

/// The result of a check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checked {
    pub issues: Vec<Issue>,
    /// The parts that the file inserts.
    pub uses: BTreeSet<String>,
}

#[derive(Clone, Copy)]
struct Scope {
    item: bool,
    analysis: bool,
}

struct Walker<'a> {
    file: &'a str,
    ctx: Context<'a>,
    out: Checked,
    nodes: usize,
    depth_reported: bool,
    nodes_reported: bool,
}

/// Check one layout file or one part file (parsed JSON).
pub fn check(file: &str, doc: &Value, ctx: Context<'_>) -> Checked {
    let mut w = Walker {
        file,
        ctx,
        out: Checked::default(),
        nodes: 0,
        depth_reported: false,
        nodes_reported: false,
    };
    let Some(obj) = doc.as_object() else {
        w.bad("", "the file must be a JSON object with a \"root\" node");
        return w.out;
    };
    for k in obj.keys() {
        if k != "root" && k != "$schema" {
            w.bad(
                &format!("/{}", pointer_key(k)),
                "unknown key; a layout file has \"$schema\" and \"root\"",
            );
        }
    }
    if let Some(s) = obj.get("$schema")
        && !s.is_string()
    {
        w.bad("/$schema", "must be a string");
    }
    match obj.get("root") {
        Some(root) => w.node(
            root,
            "/root",
            1,
            Scope {
                item: false,
                analysis: false,
            },
        ),
        None => w.bad("", "missing \"root\""),
    }
    w.out
}

impl Walker<'_> {
    fn bad(&mut self, ptr: &str, rule: &str) {
        self.out.issues.push(Issue::new(self.file, ptr, rule));
    }

    fn node(&mut self, v: &Value, ptr: &str, depth: usize, scope: Scope) {
        if depth > caps::DEPTH {
            if !self.depth_reported {
                self.depth_reported = true;
                self.bad(ptr, &format!("depth is over {}", caps::DEPTH));
            }
            return;
        }
        self.nodes += 1;
        if self.nodes > caps::NODES {
            if !self.nodes_reported {
                self.nodes_reported = true;
                self.bad(ptr, &format!("more than {} nodes", caps::NODES));
            }
            return;
        }
        let Some(obj) = v.as_object() else {
            self.bad(ptr, "a node must be an object");
            return;
        };
        if let Some(part) = obj.get("use") {
            self.use_node(obj, part, ptr);
            return;
        }
        let Some(kind) = obj.get("type").and_then(Value::as_str) else {
            self.bad(&format!("{ptr}/type"), "a node needs a \"type\" string");
            return;
        };
        if !CATALOG.contains(&kind) {
            self.bad(
                &format!("{ptr}/type"),
                &format!("unknown component \"{kind}\""),
            );
            return;
        }
        if self.ctx.class == DisplayClass::Watch && !WATCH.contains(&kind) {
            self.bad(
                &format!("{ptr}/type"),
                &format!("component \"{kind}\" is not available on the watch"),
            );
        }
        let mut scope = scope;
        if let Some(role) = obj.get("role") {
            if role.as_str() == Some("analysis") && kind == "panel" {
                scope.analysis = true;
            } else if role.as_str() == Some("analysis") {
                self.bad(
                    &format!("{ptr}/role"),
                    "role \"analysis\" is only for a panel",
                );
            }
        }
        if (SCENES.contains(&kind) || EFFECTS.contains(&kind)) && !obj.contains_key("fallback") {
            self.bad(ptr, &format!("\"{kind}\" needs a \"fallback\" node"));
        }
        if kind == "repeat" || kind == "list" {
            scope.item = true;
        }
        self.scene_params(kind, obj, ptr);
        for (k, val) in obj {
            let p = format!("{ptr}/{}", pointer_key(k));
            match k.as_str() {
                "type" | "role" => {}
                "id" => self.id(val, &p),
                "children" => self.children(val, &p, depth, scope),
                "fallback" => self.node(val, &p, depth + 1, scope),
                "when" => self.when(val, &p, scope),
                "on" => self.on(val, &p, scope),
                "tiers" if kind == "sessions" => self.tiers(val, &p, depth, scope),
                "classes" if kind == "responsive" => self.classes(val, &p, depth, scope),
                _ => self.value(val, &p, scope),
            }
        }
        if kind == "sessions" && !obj.contains_key("tiers") {
            self.bad(ptr, "\"sessions\" needs \"tiers\"");
        }
        if kind == "responsive" && !obj.contains_key("classes") {
            self.bad(ptr, "\"responsive\" needs \"classes\"");
        }
    }

    fn use_node(&mut self, obj: &Map<String, Value>, part: &Value, ptr: &str) {
        for k in obj.keys() {
            if k != "use" && k != "when" {
                self.bad(
                    &format!("{ptr}/{}", pointer_key(k)),
                    "a \"use\" node has only \"use\" and \"when\"",
                );
            }
        }
        match part.as_str() {
            Some(name) if self.ctx.parts.contains(name) => {
                self.out.uses.insert(name.to_string());
            }
            Some(name) => self.bad(&format!("{ptr}/use"), &format!("unknown part \"{name}\"")),
            None => self.bad(&format!("{ptr}/use"), "must be a part name"),
        }
        if let Some(w) = obj.get("when") {
            self.when(
                w,
                &format!("{ptr}/when"),
                Scope {
                    item: true,
                    analysis: false,
                },
            );
        }
    }

    fn id(&mut self, v: &Value, ptr: &str) {
        let ok = v.as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        });
        if !ok {
            self.bad(ptr, "id must be 1 to 64 characters of [A-Za-z0-9_-]");
        }
    }

    fn children(&mut self, v: &Value, ptr: &str, depth: usize, scope: Scope) {
        let Some(list) = v.as_array() else {
            self.bad(ptr, "children must be a list of nodes");
            return;
        };
        for (i, c) in list.iter().enumerate() {
            self.node(c, &format!("{ptr}/{i}"), depth + 1, scope);
        }
    }

    fn tiers(&mut self, v: &Value, ptr: &str, depth: usize, scope: Scope) {
        let Some(map) = v.as_object() else {
            self.bad(ptr, "tiers must be an object");
            return;
        };
        if !map.contains_key("13-30") {
            self.bad(
                ptr,
                "tier \"13-30\" is required (from 13 sessions the theme shows rows)",
            );
        }
        for (k, t) in map {
            let p = format!("{ptr}/{}", pointer_key(k));
            if !TIERS.contains(&k.as_str()) {
                self.bad(&p, &format!("unknown tier; use {}", TIERS.join(", ")));
                continue;
            }
            let Some(t) = t.as_object() else {
                self.bad(
                    &p,
                    "a tier must be an object with \"layout\" and \"template\"",
                );
                continue;
            };
            let layout = t.get("layout").and_then(Value::as_str);
            if !matches!(layout, Some("list" | "grid" | "scene")) {
                self.bad(&format!("{p}/layout"), "layout must be list, grid or scene");
            }
            if (k == "13-30" || k == "31+") && layout != Some("list") {
                self.bad(
                    &format!("{p}/layout"),
                    "tiers 13-30 and 31+ must use layout \"list\"",
                );
            }
            for key in t.keys() {
                if key != "layout" && key != "template" {
                    self.bad(
                        &format!("{p}/{}", pointer_key(key)),
                        "unknown key of a tier",
                    );
                }
            }
            match t.get("template") {
                Some(n) => self.node(
                    n,
                    &format!("{p}/template"),
                    depth + 1,
                    Scope {
                        item: true,
                        ..scope
                    },
                ),
                None => self.bad(&p, "a tier needs a \"template\" node"),
            }
        }
    }

    fn classes(&mut self, v: &Value, ptr: &str, depth: usize, scope: Scope) {
        let Some(map) = v.as_object() else {
            self.bad(ptr, "classes must be an object");
            return;
        };
        if map.is_empty() {
            self.bad(ptr, "classes needs at least one width class");
        }
        let allowed: &[&str] = match self.ctx.class {
            DisplayClass::Phone => &WIDTHS[..1],
            DisplayClass::Fold => &WIDTHS[..2],
            _ => &WIDTHS,
        };
        for (k, n) in map {
            let p = format!("{ptr}/{}", pointer_key(k));
            if !allowed.contains(&k.as_str()) {
                self.bad(
                    &p,
                    &format!(
                        "width class \"{k}\" is not allowed here; use {}",
                        allowed.join(", ")
                    ),
                );
                continue;
            }
            self.node(n, &p, depth + 1, scope);
        }
    }

    fn when(&mut self, v: &Value, ptr: &str, scope: Scope) {
        let Some(o) = v.as_object() else {
            self.bad(ptr, "when must be an object");
            return;
        };
        for k in o.keys() {
            if !matches!(k.as_str(), "path" | "is" | "value") {
                self.bad(
                    &format!("{ptr}/{}", pointer_key(k)),
                    "when has path, is and value only (one test)",
                );
            }
        }
        match o.get("path").and_then(Value::as_str) {
            Some(s) if s.starts_with('$') && !s.starts_with("$$") => {
                self.path(s, &format!("{ptr}/path"), scope)
            }
            _ => self.bad(&format!("{ptr}/path"), "must be a data path ($...)"),
        }
        let test = o.get("is").and_then(Value::as_str);
        if !test.is_some_and(|t| TESTS.contains(&t)) {
            self.bad(
                &format!("{ptr}/is"),
                &format!("test must be one of {}", TESTS.join(", ")),
            );
            return;
        }
        match (test, o.get("value")) {
            (Some("nonEmpty" | "empty"), None) => {}
            (Some("nonEmpty" | "empty"), Some(_)) => {
                self.bad(&format!("{ptr}/value"), "this test has no value")
            }
            (Some("eq" | "ne"), Some(v)) if is_literal(v) => {}
            (Some("in"), Some(Value::Array(a))) if a.iter().all(is_literal) => {}
            _ => self.bad(
                &format!("{ptr}/value"),
                "the value must be a literal (a list of literals for \"in\")",
            ),
        }
    }

    fn on(&mut self, v: &Value, ptr: &str, scope: Scope) {
        let Some(o) = v.as_object() else {
            self.bad(ptr, "on must be an object");
            return;
        };
        for (event, act) in o {
            let p = format!("{ptr}/{}", pointer_key(event));
            if event != "press" && event != "key" {
                self.bad(&p, "an action fires on a user event only: \"press\" or \"key\" (no timers, no load events)");
                continue;
            }
            self.action(act, &p, event == "key", scope);
        }
    }

    fn action(&mut self, v: &Value, ptr: &str, key_event: bool, scope: Scope) {
        let Some(o) = v.as_object() else {
            self.bad(ptr, "an action must be an object");
            return;
        };
        let name = o.get("action").and_then(Value::as_str);
        let Some(name) = name.filter(|n| ACTIONS.contains(n)) else {
            let shown = o.get("action").and_then(Value::as_str).unwrap_or("?");
            self.bad(
                &format!("{ptr}/action"),
                &format!("unknown action \"{shown}\""),
            );
            return;
        };
        if key_event {
            match o.get("combo").and_then(Value::as_str) {
                Some(c)
                    if RESERVED_KEYS
                        .contains(&c.to_ascii_lowercase().replace(' ', "").as_str()) =>
                {
                    self.bad(
                        &format!("{ptr}/combo"),
                        &format!("\"{c}\" is a reserved key of the app"),
                    );
                }
                Some(c) if !c.is_empty() && c.len() <= 32 => {}
                _ => self.bad(
                    &format!("{ptr}/combo"),
                    "a key event needs a \"combo\" such as \"Ctrl+K\"",
                ),
            }
        }
        for (k, val) in o {
            let p = format!("{ptr}/{}", pointer_key(k));
            match k.as_str() {
                "action" => {}
                "combo" if key_event => {}
                _ if !k.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                    || !k.bytes().all(|b| b.is_ascii_alphanumeric()) =>
                {
                    self.bad(&p, "an argument name uses [a-z][A-Za-z0-9]*");
                }
                _ if TEXT_ACTIONS.contains(&name) && TEXT_ARGS.contains(&k.as_str()) => {
                    self.bad(
                        &p,
                        "text for an agent comes only from a composer that the user types into",
                    );
                }
                _ => {
                    if !is_literal(val) || val.is_null() {
                        self.bad(&p, "an argument is a data path or a literal");
                    } else {
                        self.value(val, &p, scope);
                    }
                }
            }
        }
    }

    fn scene_params(&mut self, kind: &str, obj: &Map<String, Value>, ptr: &str) {
        let one_of = |w: &mut Self, key: &str, allowed: &[&str]| {
            if let Some(v) = obj.get(key)
                && !v.as_str().is_some_and(|s| allowed.contains(&s))
            {
                w.bad(
                    &format!("{ptr}/{key}"),
                    &format!("must be one of {}", allowed.join(", ")),
                );
            }
        };
        let range = |w: &mut Self, key: &str, lo: f64, hi: f64| {
            if let Some(v) = obj.get(key)
                && !v.as_f64().is_some_and(|n| (lo..=hi).contains(&n))
            {
                w.bad(
                    &format!("{ptr}/{key}"),
                    &format!("must be a number from {lo} to {hi}"),
                );
            }
        };
        match kind {
            "city3d" => one_of(self, "height", &["tokens", "cost", "turns"]),
            "bloom" => one_of(self, "strand", &["tokensHour", "tokensTotal"]),
            "radar" => {
                range(self, "sweep_s", 2.0, 20.0);
                if let Some(v) = obj.get("rings_min")
                    && !v
                        .as_array()
                        .is_some_and(|a| a.len() <= 6 && a.iter().all(|n| n.is_number()))
                {
                    self.bad(
                        &format!("{ptr}/rings_min"),
                        "must be a list of up to 6 numbers",
                    );
                }
            }
            "rain" => {
                if let Some(v) = obj.get("speed")
                    && !(v.as_str() == Some("activity")
                        || v.as_f64().is_some_and(|n| (0.1..=10.0).contains(&n)))
                {
                    self.bad(
                        &format!("{ptr}/speed"),
                        "must be \"activity\" or a number from 0.1 to 10",
                    );
                }
            }
            _ => {}
        }
    }

    /// A property value: literals, data paths, `map` objects, lists, plain objects.
    fn value(&mut self, v: &Value, ptr: &str, scope: Scope) {
        match v {
            Value::String(s) => self.string(s, ptr, scope),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    self.value(x, &format!("{ptr}/{i}"), scope);
                }
            }
            Value::Object(o) => {
                if o.contains_key("type") || o.contains_key("use") {
                    self.bad(
                        ptr,
                        "a node is only allowed in children, fallback, tiers and classes",
                    );
                    return;
                }
                let is_map = o.contains_key("map");
                if is_map {
                    match o.get("path").and_then(Value::as_str) {
                        Some(p) if p.starts_with('$') && !p.starts_with("$$") => {
                            self.path(p, &format!("{ptr}/path"), scope)
                        }
                        _ => self.bad(&format!("{ptr}/path"), "a map needs a data path"),
                    }
                    if !o.get("map").is_some_and(Value::is_object) {
                        self.bad(&format!("{ptr}/map"), "map must be a table of values");
                    }
                }
                for (k, x) in o {
                    if is_map && k == "path" {
                        continue;
                    }
                    self.value(x, &format!("{ptr}/{}", pointer_key(k)), scope);
                }
            }
            _ => {}
        }
    }

    fn string(&mut self, s: &str, ptr: &str, scope: Scope) {
        if s.starts_with("$$") {
            // A literal "$..." text.
        } else if s.starts_with('$') {
            self.path(s, ptr, scope);
            return;
        }
        if s.chars().count() > caps::TEXT_CHARS {
            self.bad(
                ptr,
                &format!(
                    "text literal is longer than {} characters",
                    caps::TEXT_CHARS
                ),
            );
        }
    }

    /// A data path: `$root.a.b`, `$item.x`, `$strings.key`.
    fn path(&mut self, s: &str, ptr: &str, scope: Scope) {
        let body = &s[1..];
        let mut it = body.split('.');
        let root = it.next().unwrap_or_default();
        let rest: Vec<&str> = it.collect();
        if body.is_empty()
            || !body
                .split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            self.bad(
                ptr,
                &format!("\"{s}\" is not a data path ($root.name.name)"),
            );
            return;
        }
        match root {
            "item" if !scope.item => {
                self.bad(ptr, "$item is only valid inside repeat, list and sessions")
            }
            "item" => {}
            "strings" => {
                let key = rest.join(".");
                match self.ctx.strings {
                    _ if key.is_empty() => self.bad(ptr, "$strings needs a key ($strings.key)"),
                    None => self.bad(
                        ptr,
                        "the package has no strings table (strings/<lang>.toml)",
                    ),
                    Some(keys) if !keys.contains(&key) => {
                        self.bad(
                            ptr,
                            &format!("unknown strings key \"{key}\" (every language needs it)"),
                        );
                    }
                    Some(_) => {}
                }
            }
            "analysis" if !scope.analysis => {
                self.bad(
                    ptr,
                    "$analysis is only valid inside a panel with \"role\": \"analysis\"",
                );
            }
            r if r == "analysis" || ROOTS.contains(&r) => {}
            r => self.bad(
                ptr,
                &format!("unknown data root \"{r}\"; use {}", ROOTS.join(", ")),
            ),
        }
    }
}

fn is_literal(v: &Value) -> bool {
    matches!(
        v,
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(class: DisplayClass, doc: Value) -> Vec<String> {
        let parts: BTreeSet<String> = ["card".to_string()].into();
        let strings: BTreeSet<String> = ["title".to_string()].into();
        let ctx = Context {
            class,
            parts: &parts,
            strings: Some(&strings),
        };
        check("layouts/desktop/warroom.json", &doc, ctx)
            .issues
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn desk(root: Value) -> Vec<String> {
        run(DisplayClass::Desktop, json!({ "root": root }))
    }

    #[test]
    fn a_good_tree_passes() {
        let issues = desk(json!({
            "type": "panel", "id": "fleet", "variant": "glass", "title": "$strings.title",
            "when": { "path": "$asks", "is": "nonEmpty" },
            "children": [
                { "type": "repeat", "items": "$sessions", "children": [
                    { "type": "text", "value": "$item.title", "tone": { "path": "$item.state", "map": { "needsYou": "alert" }, "else": "plain" } },
                    { "type": "button", "label": "Open session",
                      "on": { "press": { "action": "focus", "id": "$item.id" } } } ] },
                { "type": "sessions", "tiers": {
                    "1": { "layout": "scene", "template": { "type": "text", "value": "$item.title" } },
                    "13-30": { "layout": "list", "template": { "use": "card" } } } },
                { "type": "rain", "speed": "activity", "fallback": { "type": "spacer" } },
                { "type": "text", "value": "$$5 cost" }
            ]
        }));
        assert!(issues.is_empty(), "{issues:?}");
    }

    fn has(issues: &[String], needle: &str) {
        assert!(
            issues.iter().any(|i| i.contains(needle)),
            "{needle} not in {issues:#?}"
        );
    }

    #[test]
    fn errors_name_file_pointer_and_rule() {
        let issues = desk(json!({ "type": "stack", "children": [
            { "type": "text" }, { "type": "text" },
            { "type": "button", "on": { "press": { "action": "delete" } } } ] }));
        assert_eq!(
            issues,
            [
                "layouts/desktop/warroom.json /root/children/2/on/press/action: unknown action \"delete\""
            ]
        );
    }

    #[test]
    fn the_grammar_rules_hold() {
        has(&desk(json!({ "type": "script" })), "unknown component");
        has(
            &desk(json!({ "type": "text", "value": "$secrets.key" })),
            "unknown data root",
        );
        has(
            &desk(json!({ "type": "text", "value": "$item.id" })),
            "$item",
        );
        has(
            &desk(json!({ "type": "text", "value": "$analysis.x" })),
            "$analysis",
        );
        has(
            &desk(json!({ "type": "text", "value": "x".repeat(201) })),
            "longer than 200",
        );
        has(&desk(json!({ "type": "rain" })), "fallback");
        has(&desk(json!({ "use": "nope" })), "unknown part");
        has(
            &desk(json!({ "type": "text", "value": "$strings.nope" })),
            "strings key",
        );
        has(
            &desk(json!({ "type": "button", "on": { "load": { "action": "focus" } } })),
            "user event",
        );
        has(
            &desk(
                json!({ "type": "button", "on": { "key": { "action": "focus", "combo": "Ctrl+Shift+L" } } }),
            ),
            "reserved",
        );
        has(
            &desk(
                json!({ "type": "button", "on": { "press": { "action": "send", "text": "rm -rf /" } } }),
            ),
            "composer",
        );
        has(
            &desk(json!({ "type": "city3d", "height": "x", "fallback": {"type":"spacer"} })),
            "one of",
        );
        has(
            &desk(
                json!({ "type": "sessions", "tiers": { "5-12": { "layout": "scene", "template": {"type":"spacer"} } } }),
            ),
            "13-30",
        );
        has(
            &desk(
                json!({ "type": "sessions", "tiers": { "13-30": { "layout": "scene", "template": {"type":"spacer"} } } }),
            ),
            "must use layout",
        );
    }

    #[test]
    fn analysis_inside_an_analysis_panel_is_fine() {
        let issues = desk(
            json!({ "type": "panel", "role": "analysis", "children": [{ "type": "text", "value": "$analysis.x" }] }),
        );
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn caps_hold() {
        let mut node = json!({ "type": "spacer" });
        for _ in 0..45 {
            node = json!({ "type": "stack", "children": [node] });
        }
        has(&desk(node), "depth");
        let kids: Vec<Value> = (0..4001).map(|_| json!({ "type": "spacer" })).collect();
        has(&desk(json!({ "type": "stack", "children": kids })), "nodes");
    }

    #[test]
    fn classes_follow_the_display_class() {
        let doc = json!({ "root": { "type": "responsive", "classes": { "regular": { "type": "spacer" } } } });
        has(&run(DisplayClass::Phone, doc.clone()), "not allowed here");
        assert!(run(DisplayClass::Fold, doc).is_empty());
        has(
            &run(DisplayClass::Watch, json!({ "root": { "type": "chart" } })),
            "not available on the watch",
        );
    }
}
