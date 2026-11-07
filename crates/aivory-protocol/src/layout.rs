//! Pure layout helpers that all clients share, ported from the design scripts.
//!
//! - [`sankey`]: the per-codebase flow chart (agents → parts → state), `sankey()` of
//!   `Main.dc.html` and `Repos.dc.html`.
//! - [`auto_columns`], [`pane_height`], [`PaneDensity`]: the session grid.
//! - [`groups`]: the "Needs you" group and one group per codebase.
//! - [`dag`]: card positions of the DAG tab (`Session.dc.html`).

use std::collections::HashMap;

use crate::common::{SessionState, UnixMs};
use crate::session::{DagEdge, DagNode};
use crate::warroom::{Pane, WarRoom};

/// The design rounds to one decimal (`Math.round(x * 10) / 10`).
fn r1(x: f64) -> f64 {
    (x * 10.0 + 0.5).floor() / 10.0
}

/// One flow of the Sankey: a session seen as provider → part → state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowItem {
    pub provider: String,
    pub part: String,
    pub state: SessionState,
    /// The main agent plus its subagents.
    pub weight: u32,
}

/// The flows of the live sessions of one codebase, in session order.
pub fn flow_items<'a>(
    sessions: impl IntoIterator<Item = &'a Pane>,
    codebase: &str,
) -> Vec<FlowItem> {
    sessions
        .into_iter()
        .filter(|s| s.codebase == codebase && s.state.is_live())
        .map(|s| FlowItem {
            provider: s.provider.clone(),
            part: s.part.clone(),
            state: s.state,
            weight: s.agents(),
        })
        .collect()
}

/// The geometry of a Sankey box in view-box units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowBox {
    pub width: f64,
    pub height: f64,
    /// Space for the provider labels on the left.
    pub left: f64,
    /// Space for the state labels on the right.
    pub right: f64,
    pub node_width: f64,
    /// Vertical gap between the nodes of one column.
    pub pad: f64,
    /// Largest height of one agent.
    pub max_unit: f64,
    /// Where the part column sits between the left and the right column (0..1).
    pub middle: f64,
    /// Link width = band height minus this gap.
    pub link_gap: f64,
}

impl FlowBox {
    /// The war-room card (281 × 100).
    pub const CARD: Self = Self {
        width: 281.0,
        height: 100.0,
        left: 62.0,
        right: 76.0,
        node_width: 4.0,
        pad: 8.0,
        max_unit: 8.0,
        middle: 0.42,
        link_gap: 1.5,
    };

    /// The Repos parts tab (1000 × 120).
    pub const WIDE: Self = Self {
        width: 1000.0,
        height: 120.0,
        left: 90.0,
        right: 120.0,
        node_width: 4.0,
        pad: 12.0,
        max_unit: 12.0,
        middle: 0.5,
        link_gap: 2.0,
    };
}

/// A Sankey column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowColumn {
    Provider,
    Part,
    State,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SankeyNode {
    pub column: FlowColumn,
    /// Provider id, part name, or state label.
    pub key: String,
    /// Set in the state column (the node color).
    pub state: Option<SessionState>,
    /// Agents through this node.
    pub value: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SankeyLink {
    /// The link color: working ink 9 %, idle #8A96A8 18 %, needs you orange 50 %, error red 50 %.
    pub state: SessionState,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    /// Stroke width.
    pub width: f64,
}

impl SankeyLink {
    /// The SVG path: a horizontal cubic Bézier from (x0, y0) to (x1, y1).
    pub fn svg_path(&self) -> String {
        let xm = r1((self.x0 + self.x1) / 2.0);
        format!(
            "M{},{}C{},{} {},{} {},{}",
            self.x0, self.y0, xm, self.y0, xm, self.y1, self.x1, self.y1
        )
    }
}

/// A label next to a node. Clients offset it a little from `anchor_x`: to the left of a
/// provider node (right-aligned), to the right of a part or state node.
#[derive(Debug, Clone, PartialEq)]
pub struct SankeyLabel {
    pub column: FlowColumn,
    /// The x of the node edge that the label touches, in view-box units.
    pub anchor_x: f64,
    /// The vertical center.
    pub y: f64,
    /// `claude ×3`, `payment`, `needs you 2`.
    pub text: String,
    /// Set in the state column (the label color).
    pub state: Option<SessionState>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sankey {
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<SankeyNode>,
    /// In paint order: attention links come last, so they draw on top.
    pub links: Vec<SankeyLink>,
    pub labels: Vec<SankeyLabel>,
}

/// ((source node, target node), state, agents) of one link.
type StageLink = ((usize, usize), SessionState, u32);

struct Node {
    key: String,
    state: Option<SessionState>,
    value: u32,
    x: f64,
    y: f64,
    h: f64,
    out_y: f64,
    in_y: f64,
}

/// Lay out the flows of one codebase. `part_order` is the map order of its parts.
pub fn sankey<'a>(
    items: &[FlowItem],
    part_order: impl IntoIterator<Item = &'a str>,
    b: &FlowBox,
) -> Sankey {
    let part_order: Vec<&str> = part_order.into_iter().collect();
    let mut cols: [Vec<Node>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for it in items {
        let keys = [
            (it.provider.clone(), None),
            (it.part.clone(), None),
            (it.state.label().to_string(), Some(it.state)),
        ];
        for (c, (key, state)) in keys.into_iter().enumerate() {
            match cols[c].iter_mut().find(|n| n.key == key) {
                Some(n) => n.value += it.weight,
                None => cols[c].push(Node {
                    key,
                    state,
                    value: it.weight,
                    x: 0.0,
                    y: 0.0,
                    h: 0.0,
                    out_y: 0.0,
                    in_y: 0.0,
                }),
            }
        }
    }
    let part_rank = |k: &str| {
        part_order
            .iter()
            .position(|p| *p == k)
            .map_or(-1, |i| i as i64)
    };
    cols[1].sort_by_key(|n| part_rank(&n.key));
    cols[2].sort_by_key(|n| n.state);

    let total: u32 = items.iter().map(|i| i.weight).sum();
    let mut sankey = Sankey {
        width: b.width,
        height: b.height,
        nodes: Vec::new(),
        links: Vec::new(),
        labels: Vec::new(),
    };
    if total == 0 {
        return sankey;
    }
    let total = f64::from(total);
    let unit = cols
        .iter()
        .map(|c| (b.height - b.pad * (c.len() as f64 - 1.0)) / total)
        .fold(b.max_unit, f64::min);
    let xs = [
        b.left,
        b.left + (b.width - b.right - b.node_width - b.left) * b.middle,
        b.width - b.right - b.node_width,
    ];
    for (ci, col) in cols.iter_mut().enumerate() {
        let mut y = (b.height - total * unit - b.pad * (col.len() as f64 - 1.0)) / 2.0;
        for n in col.iter_mut() {
            n.x = xs[ci];
            n.y = y;
            n.h = f64::from(n.value) * unit;
            n.out_y = y;
            n.in_y = y;
            y += n.h + b.pad;
        }
    }

    // Stage 0: provider → part, per (provider, part, state). Stage 1: part → state.
    let mut stages: [Vec<StageLink>; 2] = [Vec::new(), Vec::new()];
    let index = |cols: &[Vec<Node>; 3], c: usize, key: &str| {
        cols[c].iter().position(|n| n.key == key).unwrap_or(0)
    };
    for it in items {
        let p = index(&cols, 0, &it.provider);
        let q = index(&cols, 1, &it.part);
        let s = index(&cols, 2, it.state.label());
        for (si, ends) in [(p, q), (q, s)].into_iter().enumerate() {
            match stages[si]
                .iter_mut()
                .find(|(e, st, _)| *e == ends && *st == it.state)
            {
                Some(link) => link.2 += it.weight,
                None => stages[si].push((ends, it.state, it.weight)),
            }
        }
    }
    let mut links: Vec<SankeyLink> = Vec::new();
    for (si, stage) in stages.iter().enumerate() {
        let (src_col, dst_col) = (si, si + 1);
        let n = stage.len();
        let mut y0 = vec![0.0; n];
        let mut y1 = vec![0.0; n];
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| (stage[i].0.1, stage[i].1));
        for &i in &order {
            let h = f64::from(stage[i].2) * unit;
            let src = &mut cols[src_col][stage[i].0.0];
            y0[i] = src.out_y + h / 2.0;
            src.out_y += h;
        }
        // Both sorts start from the insertion order, as in the design.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| (stage[i].0.0, stage[i].1));
        for &i in &order {
            let h = f64::from(stage[i].2) * unit;
            let dst = &mut cols[dst_col][stage[i].0.1];
            y1[i] = dst.in_y + h / 2.0;
            dst.in_y += h;
        }
        for (i, ((a, z), state, value)) in stage.iter().enumerate() {
            let h = f64::from(*value) * unit;
            links.push(SankeyLink {
                state: *state,
                x0: r1(cols[src_col][*a].x + b.node_width),
                y0: r1(y0[i]),
                x1: r1(cols[dst_col][*z].x),
                y1: r1(y1[i]),
                width: r1((h - b.link_gap).max(1.0)),
            });
        }
    }
    // Calm states first, attention last (on top). The sort is stable.
    links.sort_by_key(|l| std::cmp::Reverse(l.state));
    sankey.links = links;

    let columns = [FlowColumn::Provider, FlowColumn::Part, FlowColumn::State];
    for (ci, col) in cols.iter().enumerate() {
        for n in col {
            sankey.nodes.push(SankeyNode {
                column: columns[ci],
                key: n.key.clone(),
                state: n.state,
                value: n.value,
                x: r1(n.x),
                y: r1(n.y),
                w: b.node_width,
                h: r1(n.h.max(2.0)),
            });
            let (anchor_x, text) = match columns[ci] {
                FlowColumn::Provider if n.value > 1 => (n.x, format!("{} ×{}", n.key, n.value)),
                FlowColumn::Provider => (n.x, n.key.clone()),
                FlowColumn::Part => (n.x + b.node_width, n.key.clone()),
                FlowColumn::State => (n.x + b.node_width, format!("{} {}", n.key, n.value)),
            };
            sankey.labels.push(SankeyLabel {
                column: columns[ci],
                anchor_x: r1(anchor_x),
                y: r1(n.y + n.h / 2.0),
                text,
                state: n.state,
            });
        }
    }
    sankey
}

/// Panes-per-row choices of the grid header (besides Auto).
pub const PER_ROW: [u8; 7] = [1, 2, 3, 4, 8, 9, 16];

/// Width of the session grid for a window width: minus the 76 px rail (hidden under 720 px)
/// and 64 px of padding.
pub fn grid_width(window_width: f64) -> f64 {
    window_width - if window_width > 720.0 { 76.0 } else { 0.0 } - 64.0
}

/// Auto columns: `floor((gridW + 12) / (fontSize × 19 + 12))`, 1-16. Font 16 gives 4 panes
/// per row at 1440 px.
pub fn auto_columns(grid_width: f64, font_size: f64) -> u32 {
    let n = ((grid_width + 12.0) / (font_size * 19.0 + 12.0)).floor();
    n.clamp(1.0, 16.0) as u32
}

/// Pane height in px for a column count.
pub fn pane_height(columns: u32) -> u32 {
    match columns {
        0 | 1 => 640,
        2 => 580,
        3 => 520,
        4 => 480,
        5..=7 => 420,
        8 | 9 => 300,
        _ => 200,
    }
}

/// How much a pane shows: mini mode at 8+ columns, micro at 16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDensity {
    Normal,
    Mini,
    Micro,
}

impl PaneDensity {
    pub fn for_columns(columns: u32) -> Self {
        match columns {
            16.. => Self::Micro,
            8.. => Self::Mini,
            _ => Self::Normal,
        }
    }
}

/// A group of the session grid.
#[derive(Debug, Clone, PartialEq)]
pub struct GridGroup {
    /// `needs` or the codebase id.
    pub id: String,
    /// `Needs you` or the codebase id.
    pub label: String,
    /// The group dot: needs you, error or working.
    pub state: SessionState,
    /// The minutes of the oldest wait (Needs you), else the live sessions of the codebase.
    pub count: i64,
    /// Columns that the group spans.
    pub span: u32,
    /// Indexes into [`WarRoom::sessions`].
    pub sessions: Vec<usize>,
}

/// The groups of the grid: "Needs you" first (oldest wait first, full row), then one group
/// per codebase with its other live sessions (the group spans min(count, columns)).
pub fn groups(view: &WarRoom, columns: u32) -> Vec<GridGroup> {
    let live: Vec<usize> = (0..view.sessions.len())
        .filter(|&i| in_grid(&view.sessions[i], view.now))
        .collect();
    let mut out = Vec::new();
    let mut needs: Vec<usize> = live
        .iter()
        .copied()
        .filter(|&i| view.sessions[i].state == SessionState::NeedsYou)
        .collect();
    needs.sort_by_key(|&i| view.sessions[i].since);
    if let Some(&oldest) = needs.first() {
        out.push(GridGroup {
            id: "needs".into(),
            label: "Needs you".into(),
            state: SessionState::NeedsYou,
            count: view.sessions[oldest].minutes(view.now),
            span: columns,
            sessions: needs,
        });
    }
    for cb in &view.codebases {
        let all: Vec<usize> = live
            .iter()
            .copied()
            .filter(|&i| view.sessions[i].codebase == cb.id)
            .collect();
        let mut rest: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&i| view.sessions[i].state != SessionState::NeedsYou)
            .collect();
        // Errors first, then working, then idle; the newest change first in each.
        rest.sort_by_key(|&i| {
            let s = &view.sessions[i];
            (state_rank(s.state), std::cmp::Reverse(s.since))
        });
        if rest.is_empty() {
            continue;
        }
        let error = rest
            .iter()
            .any(|&i| view.sessions[i].state == SessionState::Error);
        out.push(GridGroup {
            id: cb.id.clone(),
            label: cb.id.clone(),
            state: if error {
                SessionState::Error
            } else {
                SessionState::Working
            },
            count: all.len() as i64,
            span: (rest.len() as u32).min(columns),
            sessions: rest,
        });
    }
    // Codebases with work in progress come before codebases where every session is idle.
    out.sort_by_key(|g| {
        g.id != "needs"
            && g.sessions
                .iter()
                .all(|&i| view.sessions[i].state == SessionState::Idle)
    });
    out
}

/// Minutes of idle time after which a session leaves the grid for the quiet row.
pub const STALE_IDLE_MIN: i64 = 60;

/// A session has a pane in the grid: it is live, and an idle session is not stale.
pub fn in_grid(pane: &Pane, now: UnixMs) -> bool {
    pane.state.is_live()
        && !(pane.state == SessionState::Idle && pane.minutes(now) >= STALE_IDLE_MIN)
}

fn state_rank(state: SessionState) -> u8 {
    match state {
        SessionState::NeedsYou => 0,
        SessionState::Error => 1,
        SessionState::Working => 2,
        _ => 3,
    }
}

/// Codebases that have live sessions, in codebase order (one Sankey card each).
pub fn active_codebases(view: &WarRoom) -> Vec<&str> {
    view.codebases
        .iter()
        .filter(|c| {
            view.sessions
                .iter()
                .any(|s| s.codebase == c.id && s.state.is_live())
        })
        .map(|c| c.id.as_str())
        .collect()
}

/// Minutes between two times, for labels such as `waits 6 min`.
pub fn minutes_between(from: UnixMs, to: UnixMs) -> i64 {
    (to - from).max(0) / crate::common::MINUTE_MS
}

/// DAG card size and spacing of the deep dive.
pub const DAG_CARD: (f64, f64) = (200.0, 96.0);
const DAG_COLUMN_STEP: f64 = 260.0;
const DAG_ROW_STEP: f64 = 220.0;
const DAG_CENTER_Y: f64 = 198.0;

/// Top-left card positions, in node order. A node's column is its longest path from a root;
/// the cards of a column center on one line.
pub fn dag(nodes: &[DagNode], edges: &[DagEdge]) -> Vec<(f64, f64)> {
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let mut depth = vec![0usize; nodes.len()];
    // Longest path by relaxation; a cycle stops after `len` rounds.
    for _ in 0..nodes.len() {
        let mut changed = false;
        for e in edges {
            if let (Some(&a), Some(&b)) = (index.get(e.from.as_str()), index.get(e.to.as_str()))
                && depth[b] < depth[a] + 1
            {
                depth[b] = depth[a] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut per_column: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, d) in depth.iter().enumerate() {
        per_column.entry(*d).or_default().push(i);
    }
    let mut pos = vec![(0.0, 0.0); nodes.len()];
    for (d, members) in per_column {
        let n = members.len() as f64;
        for (row, &i) in members.iter().enumerate() {
            let y = DAG_CENTER_Y - DAG_CARD.1 / 2.0 + (row as f64 - (n - 1.0) / 2.0) * DAG_ROW_STEP;
            pos[i] = (d as f64 * DAG_COLUMN_STEP, y);
        }
    }
    pos
}
