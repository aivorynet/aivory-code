//! Where everything sits on the village map, in map pixels (`docs/design/looks/rts/MAPPING.md`).
//! One village per codebase on a grid that grows with the number of codebases. A village has its
//! main building in the middle, the sites and cottages on rings around it, and a forest, a quarry
//! and a well at its corners. Rivers run in the gaps between the rows of villages.

use std::f32::consts::TAU;

use aivory_protocol::{Line, LineKind, Pane, SessionState, WarRoom};

/// The size of a site (the frame sprite).
pub const SITE_W: f32 = 170.;
pub const SITE_H: f32 = 110.;
/// The margin around the map and the gap between villages.
const EDGE: f32 = 40.;
const GAP_X: f32 = 40.;
/// The gap between rows: a river or grass.
pub const GAP_Y: f32 = 110.;
/// At most this many finished cottages per village: the newest ones.
const COTTAGES: usize = 6;
/// The radii of the first ring and the step to the next ring.
const RING: (f32, f32) = (340., 240.);
const RING_STEP: (f32, f32) = (200., 146.);

/// What a session does now, from its last tool line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Writes files: hammers on its site.
    Edit,
    /// Runs a command: chops wood at the forest.
    Run,
    /// Reads or searches files: breaks stone at the quarry.
    Read,
    /// Searches the web: draws water at the well.
    Web,
}

impl Tool {
    fn of_label(label: &str) -> Option<Self> {
        let l = label.to_lowercase();
        let any = |words: &[&str]| words.iter().any(|w| l.contains(w));
        if any(&["web", "fetch", "browse"]) {
            Some(Self::Web)
        } else if any(&["edit", "write", "patch", "notebook"]) {
            Some(Self::Edit)
        } else if any(&["read", "grep", "glob", "search", "list", "ls"]) {
            Some(Self::Read)
        } else if any(&["bash", "shell", "run", "exec", "command", "powershell"]) {
            Some(Self::Run)
        } else {
            None
        }
    }

    /// The tool of the last tool line that says what it does.
    pub fn of(tail: &[Line]) -> Self {
        tail.iter()
            .rev()
            .filter(|l| l.kind == LineKind::Tool)
            .find_map(|l| l.label.as_deref().and_then(Self::of_label))
            .unwrap_or(Self::Edit)
    }

    /// The word on the workshop window.
    pub fn word(self) -> &'static str {
        match self {
            Self::Edit => "BUILDS · EDITS FILES",
            Self::Run => "CHOPS WOOD · RUNS A COMMAND",
            Self::Read => "BREAKS STONE · READS FILES",
            Self::Web => "DRAWS WATER · SEARCHES THE WEB",
        }
    }
}

/// One session in a village.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub state: SessionState,
    pub tool: Tool,
    pub helpers: usize,
    pub tokens: u64,
    /// The sprite variant of the site.
    pub seed: u32,
    /// The top left of the site or cottage; `None` while the robot idles at the main building.
    pub site: Option<(f32, f32)>,
    /// The place of the robot when it stands still (its foot point).
    pub x: f32,
    pub y: f32,
}

impl Item {
    /// The end of the trail at the site.
    pub fn yard(&self) -> Option<(f32, f32)> {
        self.site.map(|(x, y)| (x + 85., y + 100.))
    }

    pub fn is_house(&self) -> bool {
        matches!(self.state, SessionState::Done | SessionState::Parked)
    }
}

/// One codebase on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct Village {
    pub name: String,
    /// The plot.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// The foot of the main building.
    pub cx: f32,
    pub cy: f32,
    pub forest: (f32, f32),
    pub quarry: (f32, f32),
    pub well: (f32, f32),
    pub slots: String,
    pub free: bool,
    pub items: Vec<Item>,
}

impl Village {
    /// The place in front of the main building.
    pub fn door(&self) -> (f32, f32) {
        (self.cx, self.cy + 44.)
    }

    /// The place of a resource.
    pub fn resource(&self, tool: Tool) -> (f32, f32) {
        match tool {
            Tool::Run | Tool::Edit => self.forest,
            // In front of the rocks and the well, not in them.
            Tool::Read => (self.quarry.0, self.quarry.1 + 62.),
            Tool::Web => (self.well.0, self.well.1 + 36.),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    pub villages: Vec<Village>,
    /// The vertical centers of the gaps between rows of villages (the rivers).
    pub gaps: Vec<f32>,
}

impl Layout {
    /// A key that changes when the ground must be drawn again (plots and trails).
    pub fn ground_key(&self) -> u64 {
        let mut k: u64 = 1469598103934665603;
        let mut mix = |v: f32| {
            k ^= v.round() as i64 as u64;
            k = k.wrapping_mul(1099511628211);
        };
        mix(self.w);
        mix(self.h);
        for v in &self.villages {
            mix(v.x);
            mix(v.y);
            mix(v.w);
            mix(v.h);
            for it in &v.items {
                if let Some((x, y)) = it.site {
                    mix(x);
                    mix(y);
                }
            }
        }
        k
    }

    /// The map place of a session.
    pub fn place(&self, id: &str) -> Option<(f32, f32)> {
        self.villages
            .iter()
            .flat_map(|v| &v.items)
            .find(|i| i.id == id)
            .map(|i| (i.x, i.y))
    }
}

/// A stable hash of a text.
pub fn hash(text: &str) -> u32 {
    text.bytes()
        .fold(2166136261, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619))
}

/// A point of a trail: a soft curve from `a` to `b`, its bend set by the seed.
pub fn trail(a: (f32, f32), b: (f32, f32), seed: u32, t: f32) -> (f32, f32) {
    let (mx, my) = ((a.0 + b.0) / 2., (a.1 + b.1) / 2.);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let bend = (seed % 5) as f32 * 0.07 - 0.14;
    let (qx, qy) = (mx - dy * bend, my + dx * bend);
    let u = 1. - t;
    let (x, y) = (
        u * u * a.0 + 2. * u * t * qx + t * t * b.0,
        u * u * a.1 + 2. * u * t * qy + t * t * b.1,
    );
    // The path wanders: a slow and a quick swing across it, none at its two ends.
    let len = dx.hypot(dy).max(1.);
    let s = t * len;
    let phase = (seed % 360) as f32;
    let side = (s / 83. + phase).sin() * 10. + (s / 31. + phase * 1.7).sin() * 4.;
    let swing = side * (6. * t * u).min(1.);
    (x - dy / len * swing, y + dx / len * swing)
}

fn in_village(p: &Pane) -> bool {
    p.state.is_live() || matches!(p.state, SessionState::Done | SessionState::Parked)
}

/// The rings that hold `n` sites and the slots of each ring.
fn rings(n: usize) -> Vec<usize> {
    let mut out = vec![8];
    while out.iter().sum::<usize>() < n {
        out.push(8 + 4 * out.len());
    }
    out
}

/// The site centers of the rings, relative to the main building.
fn slots(rings: &[usize]) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    for (k, &n) in rings.iter().enumerate() {
        let (rx, ry) = (
            RING.0 + k as f32 * RING_STEP.0,
            RING.1 + k as f32 * RING_STEP.1,
        );
        for i in 0..n {
            let a = (i as f32 + 0.5) / n as f32 * TAU;
            out.push((rx * a.cos(), ry * a.sin()));
        }
    }
    out
}

/// The sessions of a codebase: live ones, then the newest finished ones, each with a free slot.
/// A session keeps its slot while other sessions come and go (the slot comes from its id).
fn items_of(war: &WarRoom, name: &str, cx: f32, cy: f32, ring_n: &[usize]) -> Vec<Item> {
    let mut panes: Vec<&Pane> = war
        .sessions
        .iter()
        .filter(|p| p.codebase == name && in_village(p))
        .collect();
    panes.sort_by(|a, b| a.id.cmp(&b.id));
    let places = slots(ring_n);
    let mut used = vec![false; places.len()];
    let mut idle = 0usize;
    panes
        .into_iter()
        .map(|p| {
            let seed = hash(&p.id);
            let mut item = Item {
                id: p.id.clone(),
                title: p.title.clone(),
                provider: p.provider.clone(),
                state: p.state,
                tool: Tool::of(&p.tail),
                helpers: p.subagents.len().min(3),
                tokens: p.tokens,
                seed,
                site: None,
                x: cx,
                y: cy,
            };
            if p.state == SessionState::Idle {
                item.x = cx - 66. + (idle % 4) as f32 * 44.;
                item.y = cy + 86. + (idle / 4) as f32 * 40.;
                idle += 1;
            } else {
                let mut k = seed as usize % places.len();
                while used[k] {
                    k = (k + 1) % places.len();
                }
                used[k] = true;
                let (dx, dy) = places[k];
                let (sx, sy) = (cx + dx - SITE_W / 2., cy + dy - SITE_H / 2.);
                item.site = Some((sx.floor(), sy.floor()));
                item.x = sx + 85.;
                item.y = sy + 100.;
            }
            item
        })
        .collect()
}

/// The sessions that get a place: all live ones and the newest finished ones.
fn shown(war: &WarRoom, name: &str) -> (usize, Vec<String>) {
    let mut done: Vec<&Pane> = war
        .sessions
        .iter()
        .filter(|p| {
            p.codebase == name && matches!(p.state, SessionState::Done | SessionState::Parked)
        })
        .collect();
    done.sort_by(|a, b| b.since.cmp(&a.since).then_with(|| a.id.cmp(&b.id)));
    let hidden = done.iter().skip(COTTAGES).map(|p| p.id.clone()).collect();
    let live = war
        .sessions
        .iter()
        .filter(|p| {
            p.codebase == name
                && matches!(
                    p.state,
                    SessionState::NeedsYou | SessionState::Error | SessionState::Working
                )
        })
        .count();
    (live + done.len().min(COTTAGES), hidden)
}

/// The layout of a war room.
pub fn layout(war: &WarRoom) -> Layout {
    let mut names: Vec<String> = war.codebases.iter().map(|c| c.id.clone()).collect();
    for p in &war.sessions {
        if in_village(p) && !names.contains(&p.codebase) {
            names.push(p.codebase.clone());
        }
    }
    if names.is_empty() {
        return Layout {
            w: 1200.,
            h: 900.,
            ..Default::default()
        };
    }
    let per_book = war.autopilot.max_per_codebase.max(1) as usize;
    let n = names.len();
    let ncol = ((n as f32 * 1.3).sqrt().ceil() as usize).max(3).min(n);
    // Measure every village first.
    struct M {
        name: String,
        rings: Vec<usize>,
        hidden: Vec<String>,
        w: f32,
        h: f32,
    }
    let ms: Vec<M> = names
        .into_iter()
        .map(|name| {
            let (sites, hidden) = shown(war, &name);
            let r = rings(sites);
            let last = (r.len() - 1) as f32;
            let w = (RING.0 + last * RING_STEP.0 + 150.) * 2.;
            let h = (RING.1 + last * RING_STEP.1 + 150.) * 2.;
            M {
                name,
                rings: r,
                hidden,
                w,
                h,
            }
        })
        .collect();
    let col_w: Vec<f32> = (0..ncol)
        .map(|c| {
            ms.iter()
                .skip(c)
                .step_by(ncol)
                .map(|m| m.w)
                .fold(0., f32::max)
        })
        .collect();
    let nrow = ms.len().div_ceil(ncol);
    let row_h: Vec<f32> = (0..nrow)
        .map(|r| {
            ms.iter()
                .skip(r * ncol)
                .take(ncol)
                .map(|m| m.h)
                .fold(0., f32::max)
        })
        .collect();
    let mut villages = Vec::new();
    let mut gaps = Vec::new();
    let mut y = EDGE;
    for (r, rh) in row_h.iter().enumerate() {
        let mut x = EDGE;
        for (c, cw) in col_w.iter().enumerate() {
            let Some(m) = ms.get(r * ncol + c) else {
                break;
            };
            let (cx, cy) = ((x + cw / 2.).floor(), (y + rh / 2.).floor());
            let items: Vec<Item> = items_of(war, &m.name, cx, cy, &m.rings)
                .into_iter()
                .filter(|i| !m.hidden.contains(&i.id))
                .collect();
            let running = items
                .iter()
                .filter(|i| matches!(i.state, SessionState::Working | SessionState::NeedsYou))
                .count();
            let free = per_book.saturating_sub(running);
            let (px, py) = (cx - m.w / 2., cy - m.h / 2.);
            villages.push(Village {
                name: m.name.clone(),
                x: px,
                y: py,
                w: m.w,
                h: m.h,
                cx,
                cy,
                forest: (px + 100., py + 80.),
                quarry: (px + 90., py + m.h - 76.),
                well: (px + m.w - 90., py + 90.),
                slots: if free == 0 {
                    format!("{running} OF {per_book} RUN")
                } else {
                    format!("{free} FREE")
                },
                free: free > 0,
                items,
            });
            x += cw + GAP_X;
        }
        y += rh;
        if r + 1 < nrow {
            gaps.push(y + GAP_Y / 2.);
        }
        y += GAP_Y;
    }
    let w = EDGE * 2. + col_w.iter().sum::<f32>() + GAP_X * (ncol as f32 - 1.);
    Layout {
        w: w.max(1200.),
        h: (y - GAP_Y + EDGE).max(900.),
        villages,
        gaps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_tool_line_picks_the_work() {
        let tool = |label: &str| Line::labeled(LineKind::Tool, label, "x");
        assert_eq!(Tool::of(&[]), Tool::Edit);
        assert_eq!(Tool::of(&[tool("Bash")]), Tool::Run);
        assert_eq!(Tool::of(&[tool("Bash"), tool("Grep")]), Tool::Read);
        assert_eq!(Tool::of(&[tool("WebSearch")]), Tool::Web);
        assert_eq!(Tool::of(&[tool("Read"), tool("Edit")]), Tool::Edit);
    }

    #[test]
    fn rings_hold_every_site() {
        for n in [0, 1, 8, 9, 20, 21, 60] {
            let r = rings(n);
            assert!(r.iter().sum::<usize>() >= n.max(8));
            assert_eq!(slots(&r).len(), r.iter().sum::<usize>());
        }
    }

    #[test]
    fn a_trail_joins_its_ends() {
        let (a, b) = ((10., 20.), (300., 90.));
        assert_eq!(trail(a, b, 3, 0.), a);
        assert_eq!(trail(a, b, 3, 1.), b);
    }
}
