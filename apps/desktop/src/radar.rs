//! The conflict radar (`Lens.dc.html` frame 5): the fourth tab of Repos. A graph of the sessions
//! with risk-colored edges and score chips, and the list of hits, highest score first. The pure
//! parts (risk color, edge layout, hit order, texts) are free functions with tests.

mod view;

use std::time::Duration;

use aivory_protocol::coord::{Claim, ClaimState, RadarHit};

pub use view::{Coord, RadarEvent, RadarView, fetch, score_pill};

pub const BOX_W: f32 = 928.;
pub const BOX_H: f32 = 240.;
pub(crate) const NODE_W: f32 = 160.;
pub(crate) const NODE_H: f32 = 50.;
/// The gap between a node and the end of an edge (the artboard draws 82 px from the center).
const EDGE_GAP_X: f32 = NODE_W / 2. + 2.;
const EDGE_GAP_Y: f32 = NODE_H / 2.;
/// A score from this value on counts as "at risk" (0.7 and 1.0).
const AT_RISK: f32 = 0.65;
/// A row of the hit list with a score under this value is faded (the 0.1 clean rows).
const LOW: f32 = 0.35;
const MAX_COLUMNS: usize = 4;
const MAX_ROWS: usize = 3;
const REFRESH_EVERY: Duration = Duration::from_secs(3);

// ---- Pure parts ----

/// The colors of a risk score (README section 8, "Radar risk").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Risk {
    /// 0 clean, 1 symbols, 2 signature, 3 conflict.
    pub bucket: u8,
    pub edge: u32,
    pub text: u32,
    pub bg: u32,
    pub legend: &'static str,
}

const RISKS: [Risk; 4] = [
    Risk {
        bucket: 0,
        edge: 0xA0AEC0,
        text: 0x5A6578,
        bg: 0xF1F3F6,
        legend: "0.1 clean",
    },
    Risk {
        bucket: 1,
        edge: 0xF59E0B,
        text: 0x92400E,
        bg: 0xFFF8EB,
        legend: "0.4 symbols",
    },
    Risk {
        bucket: 2,
        edge: 0xF97316,
        text: 0xC2410C,
        bg: 0xFFF1E6,
        legend: "0.7 signature",
    },
    Risk {
        bucket: 3,
        edge: 0xDC2626,
        text: 0xB91C1C,
        bg: 0xFDECEC,
        legend: "1.0 conflict",
    },
];

/// The risk class of a score. The scores are 0.1, 0.4, 0.7 and 1.0; a value between two of them
/// takes the class below the midpoint.
pub fn risk(score: f32) -> Risk {
    let bucket = if score >= 0.85 {
        3
    } else if score >= AT_RISK {
        2
    } else if score >= LOW {
        1
    } else {
        0
    };
    RISKS[bucket]
}

pub fn legend() -> [Risk; 4] {
    RISKS
}

/// The stroke width of an edge: 2 to 4 px, wider for a higher score.
pub fn edge_width(score: f32) -> f32 {
    [2., 2.5, 3.5, 4.][risk(score).bucket as usize]
}

/// `0.7`, `1.0`.
pub fn score_text(score: f32) -> String {
    format!("{score:.1}")
}

/// The pairs at 0.7 and up: the orange count of the tab.
pub fn at_risk(hits: &[RadarHit]) -> usize {
    hits.iter().filter(|h| h.score >= AT_RISK).count()
}

/// A faded row of the hit list.
pub fn is_low(score: f32) -> bool {
    score < LOW
}

/// Highest score first, then the lower tier, then the newer hit, then the ids.
pub fn sort_hits(hits: &mut [RadarHit]) {
    hits.sort_by(|x, y| {
        y.score
            .total_cmp(&x.score)
            .then(x.tier.cmp(&y.tier))
            .then(y.at.cmp(&x.at))
            .then_with(|| (&x.a, &x.b).cmp(&(&y.a, &y.b)))
    });
}

/// `Tier 1 · same path · text conflict`.
pub fn tier_text(hit: &RadarHit) -> String {
    let tier = match hit.tier {
        1 => "Tier 1 · same path",
        2 => "Tier 2 · same symbol",
        _ => "Tier 3 · dependency",
    };
    format!("{tier} · {}", merge_text(hit))
}

/// The result of the trial merge.
pub fn merge_text(hit: &RadarHit) -> String {
    if !hit.trial {
        return match &hit.trial_error {
            Some(why) => format!("no trial merge: {why}"),
            None => "no trial merge".to_string(),
        };
    }
    match risk(hit.score).bucket {
        3 => "text conflict",
        2 => "clean, signature used",
        1 => "clean, symbols shared",
        _ => "clean, no shared symbols",
    }
    .to_string()
}

/// The two key lines of a row: the first two keys, or the key and the conflict file.
pub fn key_lines(hit: &RadarHit) -> (String, String) {
    let first = hit.keys.first().cloned().unwrap_or_default();
    let second = match hit.keys.get(1) {
        Some(key) => key.clone(),
        None => match hit.conflict_files.first() {
            Some(file) => format!("conflict in {file}"),
            None => String::new(),
        },
    };
    (first, second)
}

/// A node of the graph box, in the 928x240 view box. `x` and `y` are the center.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePos {
    pub id: String,
    pub x: f32,
    pub y: f32,
}

/// Place the sessions: columns of at most two (three above eight sessions), the first session
/// of the highest hit in the first column. A column with one node centers it.
pub fn layout_nodes(ids: &[String]) -> Vec<NodePos> {
    let ids = &ids[..ids.len().min(MAX_COLUMNS * MAX_ROWS)];
    let rows = if ids.len() <= 8 { 2 } else { MAX_ROWS };
    let columns = ids.len().div_ceil(rows).max(1);
    let mut out = Vec::with_capacity(ids.len());
    for (c, chunk) in ids.chunks(rows).enumerate() {
        let x = if columns == 1 {
            464.
        } else {
            150. + c as f32 * (778. - 150.) / (columns - 1) as f32
        };
        let ys: &[f32] = match chunk.len() {
            1 => &[120.],
            2 => &[52., 188.],
            _ => &[52., 120., 188.],
        };
        for (id, y) in chunk.iter().zip(ys) {
            out.push(NodePos {
                id: id.clone(),
                x,
                y: *y,
            });
        }
    }
    out
}

/// The sessions of the hits in the order of the sorted hit list.
pub fn node_order(hits: &[RadarHit]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for hit in hits {
        for id in [&hit.a, &hit.b] {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

/// The curve of an edge in the view box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeGeom {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub c1: (f32, f32),
    pub c2: (f32, f32),
    /// Where the score chip sits.
    pub mid: (f32, f32),
}

/// A straight line between nodes in one column or one row, a cubic curve otherwise.
pub fn edge_geom(a: &NodePos, b: &NodePos) -> EdgeGeom {
    let same_column = (a.x - b.x).abs() < 1.;
    let (from, to) = if same_column {
        let (top, bottom) = if a.y <= b.y { (a, b) } else { (b, a) };
        (
            (top.x, top.y + EDGE_GAP_Y),
            (bottom.x, bottom.y - EDGE_GAP_Y),
        )
    } else {
        let (left, right) = if a.x <= b.x { (a, b) } else { (b, a) };
        (
            (left.x + EDGE_GAP_X, left.y),
            (right.x - EDGE_GAP_X, right.y),
        )
    };
    let straight = same_column || (a.y - b.y).abs() < 1.;
    if straight {
        let mid = ((from.0 + to.0) / 2., (from.1 + to.1) / 2.);
        return EdgeGeom {
            from,
            to,
            c1: from,
            c2: to,
            mid,
        };
    }
    let xm = (from.0 + to.0) / 2.;
    let c1 = (xm, from.1);
    let c2 = (xm, to.1);
    // The point at t = 1/2 of the cubic.
    let at = |p0: f32, p1: f32, p2: f32, p3: f32| (p0 + 3. * p1 + 3. * p2 + p3) / 8.;
    EdgeGeom {
        from,
        to,
        c1,
        c2,
        mid: (at(from.0, c1.0, c2.0, to.0), at(from.1, c1.1, c2.1, to.1)),
    }
}

/// The claim to show as a pill next to a file: the session with the strongest live claim on the
/// path (editing before intent; a landed claim shows nothing).
pub fn claim_pill(claims: &[Claim], path: &str) -> Option<(String, ClaimState)> {
    let on_path = |c: &&Claim| c.path.as_deref() == Some(path) && c.state != ClaimState::Landed;
    claims
        .iter()
        .filter(on_path)
        .find(|c| c.state == ClaimState::Editing)
        .or_else(|| claims.iter().find(on_path))
        .map(|c| (c.session.clone(), c.state))
}

/// The text of the ping that "Send ping" sends to `to` about the hit.
pub fn ping_text(hit: &RadarHit, other: &str) -> String {
    let key = hit.keys.first().map_or("the same code", String::as_str);
    format!(
        "{other} also changes {key} (radar tier {}, risk {})",
        hit.tier,
        score_text(hit.score)
    )
}

/// `5 sessions · 4 pairs · 2 at risk`.
pub fn graph_meta(sessions: usize, pairs: usize, risky: usize) -> String {
    let s = if sessions == 1 { "session" } else { "sessions" };
    let p = if pairs == 1 { "pair" } else { "pairs" };
    format!("{sessions} {s} · {pairs} {p} · {risky} at risk")
}

/// `4 pairs · sorted by score · trial merge with git merge-tree`.
pub fn hits_meta(pairs: usize) -> String {
    let p = if pairs == 1 { "pair" } else { "pairs" };
    format!("{pairs} {p} · sorted by score · trial merge with git merge-tree")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo;

    fn hit(a: &str, b: &str, score: f32, tier: u8, at: i64) -> RadarHit {
        RadarHit {
            a: a.into(),
            b: b.into(),
            codebase: "billing".into(),
            tier,
            keys: vec![format!("path:{a}")],
            score,
            conflict_files: Vec::new(),
            trial: true,
            trial_error: None,
            at,
            status: None,
        }
    }

    #[test]
    fn risk_color_follows_the_score() {
        assert_eq!(risk(0.1).edge, 0xA0AEC0);
        assert_eq!(risk(0.4).edge, 0xF59E0B);
        // 0.7 as an f32 is a little under 0.7.
        assert_eq!(risk(0.7).edge, 0xF97316);
        assert_eq!(risk(1.0).edge, 0xDC2626);
        assert_eq!(risk(1.0).bg, 0xFDECEC);
        assert_eq!(risk(0.4).text, 0x92400E);
        assert_eq!(risk(0.0).bucket, 0);
    }

    #[test]
    fn edges_get_wider_with_the_score() {
        let widths: Vec<f32> = [0.1, 0.4, 0.7, 1.0].map(edge_width).to_vec();
        assert_eq!(widths, [2., 2.5, 3.5, 4.]);
    }

    #[test]
    fn hits_sort_by_score_then_tier_then_age() {
        let mut hits = vec![
            hit("a", "b", 0.4, 2, 5),
            hit("c", "d", 1.0, 3, 1),
            hit("e", "f", 1.0, 1, 1),
            hit("g", "h", 0.4, 2, 9),
        ];
        sort_hits(&mut hits);
        let order: Vec<&str> = hits.iter().map(|h| h.a.as_str()).collect();
        assert_eq!(order, ["e", "c", "g", "a"]);
    }

    #[test]
    fn the_demo_radar_reads_like_the_artboard() {
        let mut hits = demo::radar().hits;
        sort_hits(&mut hits);
        let scores: Vec<String> = hits.iter().map(|h| score_text(h.score)).collect();
        assert_eq!(scores, ["1.0", "0.7", "0.4", "0.1"]);
        assert_eq!(at_risk(&hits), 2);
        assert_eq!(tier_text(&hits[0]), "Tier 1 · same path · text conflict");
        assert_eq!(
            tier_text(&hits[1]),
            "Tier 3 · dependency · clean, signature used"
        );
        assert_eq!(
            tier_text(&hits[3]),
            "Tier 1 · same path · clean, no shared symbols"
        );
        assert!(is_low(hits[3].score) && !is_low(hits[2].score));
        assert_eq!(graph_meta(5, 4, 2), "5 sessions · 4 pairs · 2 at risk");
    }

    #[test]
    fn nodes_sit_in_columns_of_the_box() {
        let ids: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        let nodes = layout_nodes(&ids);
        let xs: Vec<f32> = nodes.iter().map(|n| n.x).collect();
        assert_eq!(xs, [150., 150., 464., 464., 778.]);
        // The lone node of the last column is centered.
        assert_eq!(nodes[4].y, 120.);
        assert_eq!((nodes[0].y, nodes[1].y), (52., 188.));
        assert!(
            nodes
                .iter()
                .all(|n| n.x >= 80. && n.x <= BOX_W - 80. && n.y >= 25. && n.y <= BOX_H - 25.)
        );
        assert_eq!(layout_nodes(&ids[..1])[0].x, 464.);
        let many: Vec<String> = (0..20).map(|i| i.to_string()).collect();
        assert_eq!(layout_nodes(&many).len(), MAX_COLUMNS * MAX_ROWS);
    }

    #[test]
    fn edges_leave_the_node_sides_as_the_artboard() {
        let n = |id: &str, x: f32, y: f32| NodePos {
            id: id.into(),
            x,
            y,
        };
        let vertical = edge_geom(&n("a", 150., 52.), &n("b", 150., 188.));
        assert_eq!((vertical.from, vertical.to), ((150., 77.), (150., 163.)));
        assert_eq!(vertical.mid, (150., 120.));
        let horizontal = edge_geom(&n("a", 150., 52.), &n("b", 464., 52.));
        assert_eq!((horizontal.from, horizontal.to), ((232., 52.), (382., 52.)));
        assert_eq!(horizontal.mid, (307., 52.));
        let curve = edge_geom(&n("a", 464., 52.), &n("b", 778., 120.));
        assert_eq!((curve.from, curve.to), ((546., 52.), (696., 120.)));
        assert!((curve.mid.0 - 621.).abs() < 1. && (curve.mid.1 - 86.).abs() < 0.5);
        // The order of the two ends does not matter.
        let swapped = edge_geom(&n("b", 778., 120.), &n("a", 464., 52.));
        assert_eq!(swapped, curve);
    }

    #[test]
    fn nodes_follow_the_order_of_the_sorted_hits() {
        let hits = vec![hit("x", "y", 1.0, 1, 0), hit("y", "z", 0.4, 2, 0)];
        assert_eq!(node_order(&hits), ["x", "y", "z"]);
    }

    #[test]
    fn a_hit_without_a_trial_names_the_reason() {
        let mut h = hit("a", "b", 0.4, 2, 0);
        h.trial = false;
        h.trial_error = Some("no common base".into());
        assert_eq!(
            tier_text(&h),
            "Tier 2 · same symbol · no trial merge: no common base"
        );
        h.conflict_files = vec!["src/a.rs".into()];
        h.keys = vec!["sym:rust:billing::Invoice::total".into()];
        assert_eq!(key_lines(&h).1, "conflict in src/a.rs");
    }

    #[test]
    fn the_claim_pill_prefers_editing() {
        let c = |session: &str, state, path: &str| Claim {
            session: session.into(),
            codebase: "billing".into(),
            key: format!("path:{path}"),
            state,
            path: Some(path.into()),
            range: None,
            lease_s: 60,
        };
        let claims = vec![
            c("k9d4", ClaimState::Intent, "a.rs"),
            c("7f3a", ClaimState::Editing, "a.rs"),
            c("m2e5", ClaimState::Landed, "b.rs"),
        ];
        assert_eq!(
            claim_pill(&claims, "a.rs"),
            Some(("7f3a".to_string(), ClaimState::Editing))
        );
        assert_eq!(claim_pill(&claims, "b.rs"), None);
        assert_eq!(claim_pill(&claims, "c.rs"), None);
    }

    #[test]
    fn the_ping_names_the_other_session_and_the_risk() {
        let h = hit("a", "b", 0.7, 3, 0);
        assert_eq!(
            ping_text(&h, "8a22"),
            "8a22 also changes path:a (radar tier 3, risk 0.7)"
        );
    }
}
