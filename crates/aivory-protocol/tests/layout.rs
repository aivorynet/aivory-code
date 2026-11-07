//! The layout helpers against the values of the design scripts.

use aivory_protocol::demo::Demo;
use aivory_protocol::layout::{self, FlowBox, PaneDensity};
use aivory_protocol::{DagEdge, SessionState};
use serde_json::Value;

/// Compare the Sankey of every codebase with the output of the design script
/// (`tests/design-sankey.js`).
fn check_sankey(expected: &str, b: &FlowBox) {
    let expected: Value = serde_json::from_str(expected).unwrap();
    let demo = Demo::new();
    let view = &demo.war_room;
    let active = layout::active_codebases(view);
    let mut sorted = active.clone();
    sorted.sort_unstable();
    // Sort here too: with serde_json's `preserve_order` (a workspace build with GPUI) the keys
    // keep the file order.
    let mut want_ids: Vec<&str> = expected
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    want_ids.sort_unstable();
    assert_eq!(sorted, want_ids, "codebases with live sessions");
    for id in active {
        let cb = view.codebase(id).unwrap();
        let items = layout::flow_items(&view.sessions, id);
        let got = layout::sankey(&items, cb.part_names(), b);
        let want = &expected[id];

        let links: Vec<(String, f64)> = got.links.iter().map(|l| (l.svg_path(), l.width)).collect();
        let want_links: Vec<(String, f64)> = want["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                (
                    l["d"].as_str().unwrap().to_string(),
                    l["w"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(links, want_links, "links of {id}");

        let nodes: Vec<[f64; 3]> = got.nodes.iter().map(|n| [n.x, n.y, n.h]).collect();
        let want_nodes: Vec<[f64; 3]> = serde_json::from_value(want["nodes"].clone()).unwrap();
        assert_eq!(nodes, want_nodes, "nodes of {id}");

        let labels: Vec<(f64, String)> = got.labels.iter().map(|l| (l.y, l.text.clone())).collect();
        let want_labels: Vec<(f64, String)> =
            serde_json::from_value(want["labels"].clone()).unwrap();
        assert_eq!(labels, want_labels, "labels of {id}");
    }
}

#[test]
fn sankey_cards_match_the_war_room_script() {
    check_sankey(include_str!("sankey-main.json"), &FlowBox::CARD);
}

#[test]
fn sankey_wide_matches_the_repos_script() {
    check_sankey(include_str!("sankey-repos.json"), &FlowBox::WIDE);
}

#[test]
fn sankey_puts_attention_links_on_top() {
    let demo = Demo::new();
    let items = layout::flow_items(&demo.war_room.sessions, "billing");
    let cb = demo.war_room.codebase("billing").unwrap();
    let s = layout::sankey(&items, cb.part_names(), &FlowBox::CARD);
    let last = s.links.last().unwrap();
    assert_eq!(last.state, SessionState::NeedsYou);
    assert!(
        layout::sankey(&[], cb.part_names(), &FlowBox::CARD)
            .links
            .is_empty()
    );
}

#[test]
fn grid_columns_and_pane_heights() {
    // 1440 px window, 12 px font: (1300 + 12) / (408 + 12) = 3.1 -> 3 columns.
    assert_eq!(layout::grid_width(1440.0), 1300.0);
    assert_eq!(layout::auto_columns(1300.0, 16.0), 4);
    assert_eq!(layout::auto_columns(1300.0, 12.0), 5);
    assert_eq!(layout::auto_columns(100.0, 18.0), 1);
    assert_eq!(layout::auto_columns(100_000.0, 9.0), 16);
    let heights: Vec<u32> = [1, 2, 3, 4, 5, 7, 8, 9, 16]
        .iter()
        .map(|&c| layout::pane_height(c))
        .collect();
    assert_eq!(heights, [640, 580, 520, 480, 420, 420, 300, 300, 200]);
    assert_eq!(PaneDensity::for_columns(4), PaneDensity::Normal);
    assert_eq!(PaneDensity::for_columns(8), PaneDensity::Mini);
    assert_eq!(PaneDensity::for_columns(16), PaneDensity::Micro);
}

#[test]
fn grid_groups_put_needs_you_first() {
    let demo = Demo::new();
    let view = &demo.war_room;
    let groups = layout::groups(view, 3);
    let needs = &groups[0];
    assert_eq!(needs.label, "Needs you");
    assert_eq!(needs.span, 3);
    assert_eq!(needs.count, 6, "oldest wait");
    let ids: Vec<&str> = needs
        .sessions
        .iter()
        .map(|&i| view.sessions[i].id.as_str())
        .collect();
    assert_eq!(ids, ["7f3a", "c218", "9b0e"]);

    let labels: Vec<&str> = groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Needs you",
            "aivory-code",
            "aivory-guard",
            "billing",
            "smart-inference",
            "kronk",
            "slaide",
            "aivory-hugo",
            "mobile"
        ]
    );
    let inference = groups.iter().find(|g| g.id == "smart-inference").unwrap();
    assert_eq!(inference.state, SessionState::Error);
    let mobile = groups.iter().find(|g| g.id == "mobile").unwrap();
    assert_eq!((mobile.span, mobile.count), (1, 1));
}

#[test]
fn dag_positions_match_the_session_script() {
    let demo = Demo::new();
    let trace = &demo.traces["a01c"];
    let pos = layout::dag(&trace.nodes, &trace.edges);
    assert_eq!(
        pos,
        [
            (0.0, 150.0),
            (260.0, 150.0),
            (520.0, 40.0),
            (520.0, 260.0),
            (780.0, 150.0),
            (1040.0, 150.0)
        ]
    );
    // A cycle does not hang.
    let cycle = [
        DagEdge {
            from: "n1".into(),
            to: "n2".into(),
        },
        DagEdge {
            from: "n2".into(),
            to: "n1".into(),
        },
    ];
    assert_eq!(layout::dag(&trace.nodes[..2], &cycle).len(), 2);
}
