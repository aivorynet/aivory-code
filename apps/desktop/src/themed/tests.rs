//! Tests of the theme host: a fixture for each component of the catalog, the full sample
//! theme, and drawing with 1, 6 and 30 sessions. Synthetic data only. Tests read
//! `AIVORY_CACHE_DIR` through the demo daemon of the desktop tests; none touches a real store.

use std::collections::BTreeMap;

use aivory_protocol::demo::Demo;
use aivory_protocol::theme::{DisplayClass, ThemeBundle};
use aivory_theme::layout::CATALOG;
use aivory_theme::sample;
use gpui::prelude::*;
use gpui::{AnyView, Context, TestAppContext, Window, div, point, px, size};
use serde_json::{Value, json};

use super::host::{self, ThemedView};
use crate::model::Model;

/// One node of each component of the catalog, with the props that the renderer reads.
fn fixture(kind: &str) -> Value {
    let text = json!({ "type": "text", "value": "$item.title" });
    let base = json!({ "type": kind });
    let mut node = base.clone();
    let set = |n: &mut Value, k: &str, v: Value| n[k] = v;
    match kind {
        "stack" | "grid" | "layer" | "scroll" | "panel" | "frame" | "tabs" | "card" => {
            set(&mut node, "children", json!([text]));
            set(&mut node, "items", json!(["A"]));
        }
        "repeat" => {
            set(&mut node, "items", json!("$sessions"));
            set(&mut node, "children", json!([text]));
        }
        "list" => {
            set(&mut node, "items", json!("$sessions"));
            set(&mut node, "children", json!([text]));
        }
        "responsive" => set(&mut node, "classes", json!({ "compact": text })),
        "sessions" => {
            let tier = |layout: &str| json!({ "layout": layout, "template": text });
            set(
                &mut node,
                "tiers",
                json!({ "1": tier("grid"), "13-30": tier("list") }),
            );
        }
        "text" | "badge" => set(&mut node, "value", json!("$item.title")),
        "readout" => {
            set(&mut node, "label", json!("L"));
            set(&mut node, "value", json!("$inbox.open"));
        }
        "button" | "segment_button" => {
            set(&mut node, "label", json!("Go"));
            set(
                &mut node,
                "on",
                json!({ "press": { "action": "open", "screen": "inbox" } }),
            );
        }
        "row" => set(&mut node, "title", json!("$item.title")),
        "log" | "chart" => set(&mut node, "items", json!("$events")),
        "gauge" => set(&mut node, "value", json!(40)),
        "files" => set(&mut node, "added", json!(3)),
        "diff" => set(&mut node, "value", json!("+a\n-b")),
        "key_hint" => set(&mut node, "keys", json!("Ctrl+K")),
        "composer" => set(&mut node, "mode", json!("dispatch")),
        // Effects and scenes need a fallback node.
        "rain" | "scramble" | "scanlines" | "rings" | "city3d" | "fsn3d" | "radar" | "bloom"
        | "camera_wall" => set(&mut node, "fallback", text),
        _ => {}
    }
    node
}

fn bundle(layouts: BTreeMap<String, Value>) -> ThemeBundle {
    let files = sample::files("fixture-theme", "1.0.0");
    let tokens = files
        .iter()
        .find(|(p, _)| p == "tokens.toml")
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .unwrap_or_default();
    let (_, json) = aivory_theme::tokens::merge(&[("tokens.toml".to_string(), tokens)])
        .unwrap_or_else(|e| panic!("{e:?}"));
    ThemeBundle {
        id: "fixture-theme".into(),
        name: "fixture".into(),
        version: "1.0.0".into(),
        author: "t".into(),
        license: "MIT".into(),
        sha256: "0".into(),
        display: DisplayClass::Desktop,
        tokens: json,
        layouts,
        parts: BTreeMap::new(),
        strings: BTreeMap::new(),
        fonts: Vec::new(),
        preview: None,
    }
}

/// The bundle of the full sample theme, as the daemon serves it.
fn sample_bundle() -> ThemeBundle {
    let mut layouts = BTreeMap::new();
    let mut strings = BTreeMap::new();
    for (path, bytes) in sample::full_files("ledger-sample", "1.0.0") {
        let text = String::from_utf8_lossy(&bytes).to_string();
        if let Some(screen) = path
            .strip_prefix("layouts/desktop/")
            .and_then(|p| p.strip_suffix(".json"))
        {
            let doc: Value = serde_json::from_str(&text).unwrap();
            layouts.insert(screen.to_string(), doc["root"].clone());
        } else if path == "strings/en.toml" {
            strings.insert(
                "en".to_string(),
                aivory_theme::strings::parse(&path, &text).unwrap(),
            );
        }
    }
    let mut b = bundle(layouts);
    b.id = "ledger-sample".into();
    b.strings = strings;
    let tokens = sample::full_files("ledger-sample", "1.0.0")
        .into_iter()
        .find(|(p, _)| p == "tokens.toml")
        .map(|(_, b)| String::from_utf8_lossy(&b).to_string())
        .unwrap();
    b.tokens = aivory_theme::tokens::merge(&[("tokens.toml".to_string(), tokens)])
        .unwrap()
        .1;
    b
}

#[test]
fn each_catalog_component_has_a_fixture_that_compiles() {
    let mut layouts = BTreeMap::new();
    for kind in CATALOG {
        layouts.insert((*kind).to_string(), fixture(kind));
    }
    let compiled = host::compile(&bundle(layouts)).expect("the fixtures compile");
    for kind in CATALOG {
        let node = &compiled.screens[*kind];
        assert_eq!(node.kind, *kind);
        assert!(
            node.any(&|n| n.kind == *kind),
            "{kind} is missing in its fixture"
        );
    }
    assert_eq!(compiled.screens.len(), CATALOG.len());
}

#[test]
fn the_sample_theme_compiles_for_all_screens() {
    let c = host::compile(&sample_bundle()).expect("the sample compiles");
    for screen in aivory_theme::validate::SCREENS {
        assert!(c.layout(screen).is_some(), "{screen}");
    }
    // `$strings.*` is read at load time.
    let wr = c.layout("warroom").unwrap();
    assert!(wr.any(&|n| n.props.iter().any(|(_, p)| matches!(
        p,
        host::Prop::Lit(Value::String(s)) if s == "AIVory Ledger"
    ))));
}

#[test]
fn a_settings_layout_without_the_section_is_not_used() {
    let mut layouts = BTreeMap::new();
    layouts.insert(
        "settings".to_string(),
        json!({ "type": "text", "value": "no section" }),
    );
    layouts.insert("inbox".to_string(), json!({ "type": "text", "value": "x" }));
    let c = host::compile(&bundle(layouts)).unwrap();
    assert!(c.layout("settings").is_none());
    assert!(c.layout("inbox").is_some());
}

/// A stand-in for the compiled Settings screen.
struct Plain;
impl Render for Plain {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("settings")
    }
}

/// Draw the screens of a theme with `n` live sessions, at two window sizes.
fn draw_screens(cx: &mut TestAppContext, n: usize, bundle: ThemeBundle, screens: &[&str]) {
    // No daemon: the model has no feed, and nothing here calls one.
    let endpoint = aivoryd::Endpoint::new("themed-test").expect("an endpoint name");
    let model = cx.new(|_| Model::offline(endpoint));
    let mut war = Demo::new().war_room;
    // Exactly `n` live sessions: copies with new ids when the demo has fewer.
    let live: Vec<_> = war
        .sessions
        .iter()
        .filter(|s| s.state.is_live())
        .cloned()
        .collect();
    let mut sessions: Vec<_> = Vec::new();
    for i in 0..n {
        let mut s = live[i % live.len()].clone();
        if i >= live.len() {
            s.id = format!("{}-{i}", s.id);
        }
        sessions.push(s);
    }
    war.sessions = sessions;
    war.board = war.compute_board();
    let first = war.sessions[0].id.clone();
    let asked = war
        .sessions
        .iter()
        .find(|s| s.ask.is_some())
        .map(|s| s.id.clone());
    crate::themed::set_active(true);
    let id = bundle.id.clone();
    host::store(host::compile(&bundle).unwrap());
    host::set_current(Some(&id));
    let settings: AnyView = cx.new(|_| Plain).into();
    let (view, vcx) = cx.add_window_view(|_, _| ThemedView::new(model.clone(), settings));
    for screen in screens {
        for (w, h) in [(1440., 900.), (800., 600.)] {
            // The demo feed of the model may replace the war room: set ours before each frame.
            model.update(vcx, |m, _| {
                m.war = Some(war.clone());
                m.rebuild_snapshot();
            });
            assert_eq!(model.read_with(vcx, |m, _| m.snapshot.live()), n);
            view.update(vcx, |v, _| {
                v.show(
                    screen,
                    Some(&first),
                    asked.as_deref().filter(|_| *screen == "inbox"),
                );
            });
            let v = AnyView::from(view.clone());
            vcx.draw(point(px(0.), px(0.)), size(px(w), px(h)), move |_, _| {
                v.clone().into_any_element()
            });
        }
    }
}

/// Draw all screens and the decision dialog of the sample theme with `n` live sessions.
fn draw_sessions(cx: &mut TestAppContext, n: usize) {
    draw_screens(cx, n, sample_bundle(), &aivory_theme::validate::SCREENS);
}

#[gpui::test]
fn every_catalog_component_draws(cx: &mut TestAppContext) {
    let mut layouts = BTreeMap::new();
    for kind in CATALOG {
        layouts.insert((*kind).to_string(), fixture(kind));
    }
    let names: Vec<String> = layouts.keys().cloned().collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    draw_screens(cx, 6, bundle(layouts), &names);
}

#[gpui::test]
fn one_session_renders(cx: &mut TestAppContext) {
    draw_sessions(cx, 1);
}

#[gpui::test]
fn six_sessions_render(cx: &mut TestAppContext) {
    draw_sessions(cx, 6);
}

#[gpui::test]
fn thirty_sessions_render(cx: &mut TestAppContext) {
    draw_sessions(cx, 30);
}
