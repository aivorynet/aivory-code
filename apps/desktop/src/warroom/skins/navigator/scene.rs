//! The canvas of the Navigator: the sky, the ground grid, the tree lines, the pedestals with
//! their blocks, the agent cubes, the spotlight and the labels, painted from the back to the
//! front. Also the camera goal of a selection.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, Bounds, Pixels, Window, linear_color_stop, linear_gradient};

use super::camera::{Cam, NEAR, Pose, ease_in_out, hull, inside};
use super::tree::{BLOCK, SLAB, Tree};
use crate::theme::tk;
use crate::warroom::skins::common::{Align, Painter, TextCache, ink, mix};

/// A pedestal that is this wide on the screen (pixels) shows its towers.
const NEAR_PX: f32 = 55.;
/// The tilt of the home view.
const HOME_PITCH: f32 = 0.55;

/// The ground far away.
const GROUND: u32 = 0x000000;
/// The sky at the horizon and at the top.
const SKY_LOW: u32 = 0x05103A;
const SKY_TOP: u32 = 0x000000;
const GRID: u32 = 0x2846C8;
const LINE: u32 = 0x8FA8FF;
const PED_TOP: u32 = 0x9AA6C4;
const ROOT_TOP: u32 = 0x9AA6C4;
const PED_EDGE: u32 = 0xA3B3D9;
const WHITE: u32 = 0xFFFFFF;
const BEAM: u32 = 0xFFF4D6;

/// The block colors per state.
pub(super) const WORK: u32 = 0x6E8CFF;
pub(super) const WAIT: u32 = 0xFF8A80;
pub(super) const ERROR: u32 = 0xFFD27A;
pub(super) const IDLE: u32 = 0xC6CAD2;
pub(super) const EMPTY: u32 = 0x4E586E;

/// What a block or a cube shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Look {
    /// No live session works here.
    Empty,
    Idle,
    Work,
    Error,
    /// A session waits for the user.
    Wait,
}

impl Look {
    /// The top color of a block in this state.
    pub fn color(self) -> u32 {
        match self {
            Look::Empty => EMPTY,
            Look::Idle => IDLE,
            Look::Work => WORK,
            Look::Error => ERROR,
            Look::Wait => WAIT,
        }
    }
}

/// Where the camera goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Aim {
    /// The whole tree.
    Home,
    Pedestal(usize),
    Block(usize),
}

/// One agent cube.
#[derive(Debug, Clone)]
pub(super) struct Cube {
    pub id: String,
    pub pos: [f32; 3],
    pub look: Look,
}

/// Everything the canvas shows this frame.
pub(super) struct Scene {
    pub tree: Tree,
    /// Per block: the look and the shown height.
    pub looks: Vec<Look>,
    pub heights: Vec<f32>,
    pub cubes: Vec<Cube>,
    /// The label of each tower, by session id, and the sessions per pedestal.
    pub names: std::collections::HashMap<String, String>,
    pub counts: Vec<usize>,
    /// Per pedestal: a session there waits for the user.
    pub alert: Vec<bool>,
    pub aim: Aim,
    /// The pose that a flight starts from and its progress from 0 to 1.
    pub fly: Option<(Pose, f32)>,
    /// The drag orbit (turn, tilt) and the log of the wheel zoom.
    pub orbit: [f32; 2],
    pub zoom: f32,
    /// The camera walk over the ground (world x, y).
    pub walk: [f32; 2],
    pub sel_block: Option<usize>,
    pub sel_pedestal: Option<usize>,
    /// The spotlight, from 0 (off) to 1.
    pub spot: f32,
    pub t: f32,
    pub hits: Rc<RefCell<Hits>>,
    pub cache: Rc<RefCell<TextCache>>,
}

/// The click targets and the camera of the last frame.
#[derive(Default)]
pub(super) struct Hits {
    pub blocks: Vec<(usize, Vec<[f32; 2]>)>,
    pub pedestals: Vec<(usize, Vec<[f32; 2]>)>,
    pub cubes: Vec<(String, [f32; 2])>,
    /// The group names under the pedestals: the pedestal and the screen rect (x, y, w, h).
    pub groups: Vec<(usize, [f32; 4])>,
    /// The pose as painted (a flight starts here).
    pub pose: Option<Pose>,
    pub cam: Option<Cam>,
    /// The free view area in the window: left, top, right, bottom.
    pub free: [f32; 4],
}

impl Hits {
    /// The cube, block or pedestal under a window point, the front one first.
    pub fn pick(&self, p: [f32; 2]) -> Option<Picked> {
        let cube = self
            .cubes
            .iter()
            .map(|(id, c)| (id, (c[0] - p[0]).hypot(c[1] - p[1])))
            .filter(|(_, d)| *d < 12.)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = cube {
            return Some(Picked::Cube(id.clone()));
        }
        if let Some((g, _)) = self.groups.iter().find(|(_, r)| {
            p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
        }) {
            return Some(Picked::Group(*g));
        }
        if let Some((b, _)) = self.blocks.iter().rev().find(|(_, h)| inside(h, p)) {
            return Some(Picked::Block(*b));
        }
        self.pedestals
            .iter()
            .rev()
            .find(|(_, h)| inside(h, p))
            .map(|(i, _)| Picked::Pedestal(*i))
    }
}

/// What a click hit.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Picked {
    Cube(String),
    Block(usize),
    Pedestal(usize),
    /// The name of a codebase under its pedestal.
    Group(usize),
}

/// The free view area of a canvas of `w` x `h`: left, top, right, bottom.
pub(super) fn free_rect(w: f32, h: f32) -> [f32; 4] {
    [12., 36., (w - 12.).max(120.), (h - 12.).max(120.)]
}

/// The focal length for a canvas of height `h`.
pub(super) fn focal(h: f32) -> f32 {
    h * 0.8
}

/// The screen point of the look-at point in the free area.
fn centre_of(free: [f32; 4]) -> [f32; 2] {
    [
        (free[0] + free[2]) / 2.,
        free[1] + (free[3] - free[1]) * 0.6,
    ]
}

/// The nearest distance at which all `pts` fit in the free area.
fn fit(pose: Pose, pts: &[[f32; 3]], free: [f32; 4], f: f32) -> f32 {
    let centre = centre_of(free);
    let ok = |d: f32| {
        let c = Cam::new(Pose { dist: d, ..pose }, f, centre);
        pts.iter().all(|p| {
            let s = c.at(*p);
            c.depth(*p) > NEAR * 2.
                && s[0] >= free[0]
                && s[0] <= free[2]
                && s[1] >= free[1]
                && s[1] <= free[3]
        })
    };
    let (mut lo, mut hi) = (60f32, 40_000f32);
    for _ in 0..28 {
        let mid = (lo * hi).sqrt();
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// The eight corners of a box, foot first.
fn corners(x: f32, y: f32, w: f32, d: f32, z0: f32, z1: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(8);
    for z in [z0, z1] {
        out.extend([[x, y, z], [x + w, y, z], [x + w, y + d, z], [x, y + d, z]]);
    }
    out
}

/// The pose that shows the aim, in a free area with focal length `f`.
pub(super) fn goal(aim: Aim, tree: &Tree, heights: &[f32], free: [f32; 4], f: f32) -> Pose {
    let tallest = heights.iter().copied().fold(0., f32::max);
    let e = tree.extent;
    let home = || {
        let pose = Pose {
            target: [(e[0] + e[2]) / 2., (e[1] + e[3]) / 2., 0.],
            yaw: 0.,
            pitch: HOME_PITCH,
            dist: 1000.,
        };
        // The pedestals with their tallest blocks, and the labels under the front edges.
        let mut pts = Vec::new();
        for p in &tree.pedestals {
            pts.extend(corners(p.x, p.y - 34., p.w, p.d + 34., 0., SLAB + tallest));
        }
        Pose {
            dist: fit(pose, &pts, free, f),
            ..pose
        }
    };
    match aim {
        Aim::Home => home(),
        Aim::Pedestal(i) => {
            let Some(p) = tree.pedestals.get(i) else {
                return home();
            };
            let top = tree
                .blocks
                .iter()
                .zip(heights)
                .filter(|(b, _)| b.pedestal == i)
                .map(|(_, h)| *h)
                .fold(0., f32::max);
            let pose = Pose {
                target: p.centre(),
                yaw: 0.,
                pitch: 0.62,
                dist: 1000.,
            };
            let pts = corners(p.x - 40., p.y - 60., p.w + 80., p.d + 100., 0., SLAB + top);
            Pose {
                dist: fit(pose, &pts, free, f).max(420.),
                ..pose
            }
        }
        Aim::Block(i) => {
            let Some(b) = tree.blocks.get(i) else {
                return home();
            };
            let h = heights.get(i).copied().unwrap_or(10.);
            let foot = b.foot();
            let pose = Pose {
                target: [foot[0], foot[1], SLAB + h * 0.5],
                yaw: 0.,
                pitch: 0.58,
                dist: 1000.,
            };
            let pts = corners(
                b.x - 170.,
                b.y - 170.,
                BLOCK + 340.,
                BLOCK + 340.,
                0.,
                SLAB + h + 40.,
            );
            Pose {
                dist: fit(pose, &pts, free, f).max(440.),
                ..pose
            }
        }
    }
}

/// The pose of this frame: the flight toward the goal, then the drag orbit and the zoom.
pub(super) fn pose_now(
    goal: Pose,
    fly: Option<(Pose, f32)>,
    orbit: [f32; 2],
    zoom: f32,
    walk: [f32; 2],
) -> Pose {
    let base = match fly {
        Some((from, k)) => Pose::lerp(&from, &goal, ease_in_out(k)),
        None => goal,
    };
    Pose {
        yaw: base.yaw + orbit[0],
        pitch: (base.pitch + orbit[1]).clamp(0.2, 1.2),
        dist: base.dist * zoom.exp(),
        target: [
            base.target[0] + walk[0],
            base.target[1] + walk[1],
            base.target[2],
        ],
    }
}

/// The camera position in the world.
fn eye_pos(p: &Pose) -> [f32; 3] {
    let (cs, sn, cp, sp) = (p.yaw.cos(), p.yaw.sin(), p.pitch.cos(), p.pitch.sin());
    [
        p.target[0] - p.dist * cp * sn,
        p.target[1] - p.dist * cp * cs,
        p.target[2] + p.dist * sp,
    ]
}

/// A label that the collision pass places.
struct Label {
    text: String,
    size: f32,
    bold: bool,
    /// The top centre.
    at: [f32; 2],
    color: u32,
    alpha: f32,
    /// The pedestal that this label names, when a click on it opens the group view.
    group: Option<usize>,
}

/// A flat-shaded box. Returns the screen hull. `shade` darkens the sides: front, right,
/// back, left.
#[allow(clippy::too_many_arguments)]
fn cuboid(
    pen: &mut Painter,
    cam: &Cam,
    eye: [f32; 3],
    (x, y, w, d): (f32, f32, f32, f32),
    (z0, z1): (f32, f32),
    top: u32,
    edge: (u32, f32),
) -> Option<Vec<[f32; 2]>> {
    let pts = corners(x, y, w, d, z0, z1);
    if !cam.visible(&pts) {
        return None;
    }
    let s: Vec<[f32; 2]> = pts.iter().map(|p| cam.at(*p)).collect();
    // Front (y-), right (x+), back (y+), left (x-): the light comes from the front left.
    let shade = [0.72, 0.52, 0.6, 0.86];
    let normal = [[0., -1.], [1., 0.], [0., 1.], [-1., 0.]];
    let mut verticals = Vec::new();
    for k in 0..4 {
        let j = (k + 1) % 4;
        let to_eye = [
            eye[0] - (pts[k][0] + pts[j][0]) / 2.,
            eye[1] - (pts[k][1] + pts[j][1]) / 2.,
        ];
        if to_eye[0] * normal[k][0] + to_eye[1] * normal[k][1] <= 0. {
            continue;
        }
        pen.polygon(&[s[k], s[j], s[j + 4], s[k + 4]], mix(0, top, shade[k]), 1.);
        verticals.push([s[k][0], s[k][1], s[k + 4][0], s[k + 4][1]]);
        verticals.push([s[j][0], s[j][1], s[j + 4][0], s[j + 4][1]]);
    }
    let roof = [s[4], s[5], s[6], s[7]];
    if eye[2] > z1 {
        pen.polygon(&roof, top, 1.);
    }
    pen.strokes(&verticals, 0x000000, 1., 0.28);
    let ring: Vec<[f32; 4]> = (0..4)
        .map(|k| {
            let (a, b) = (roof[k], roof[(k + 1) % 4]);
            [a[0], a[1], b[0], b[1]]
        })
        .collect();
    pen.strokes(&ring, edge.0, 1., edge.1);
    Some(hull(&s))
}

/// A circle on a horizontal plane, projected.
fn disc(cam: &Cam, c: [f32; 3], r: f32, n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            cam.at([c[0] + a.cos() * r, c[1] + a.sin() * r, c[2]])
        })
        .collect()
}

/// Paint the landscape into the canvas.
pub(super) fn paint(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let mut cache = scene.cache.borrow_mut();
    let mut pen = Painter::new(window, cx, &mut cache);
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let free_local = free_rect(w, h);
    let f = focal(h);
    let goal = goal(scene.aim, &scene.tree, &scene.heights, free_local, f);
    let pose = pose_now(goal, scene.fly, scene.orbit, scene.zoom, scene.walk);
    let c = centre_of(free_local);
    let cam = Cam::new(pose, f, [ox + c[0], oy + c[1]]);
    let eye = eye_pos(&pose);
    // Level of detail: zoomed out shows pedestals only; the towers show when a pedestal is
    // wide enough on the screen.
    let near = scene.tree.pedestals.iter().skip(1).any(|p| {
        let a = cam.at([p.x, p.y, 0.]);
        let b = cam.at([p.x + p.w, p.y, 0.]);
        (b[0] - a[0]).abs() >= NEAR_PX
    });
    let tree = &scene.tree;

    // The sky down to the horizon, then the ground.
    let hz = cam.horizon().clamp(oy, oy + h);
    pen.rect((ox, oy, w, h), GROUND, 1.);
    if hz > oy {
        let sky = [[ox, oy], [ox + w, oy], [ox + w, hz], [ox, hz]];
        pen.polygon_bg(
            &sky,
            linear_gradient(
                180.,
                linear_color_stop(ink(SKY_TOP, 1.), 0.),
                linear_color_stop(ink(SKY_LOW, 1.), 1.),
            ),
        );
        let haze_h = (h * 0.22).min(oy + h - hz);
        let haze = [
            [ox, hz],
            [ox + w, hz],
            [ox + w, hz + haze_h],
            [ox, hz + haze_h],
        ];
        pen.polygon_bg(
            &haze,
            linear_gradient(
                180.,
                linear_color_stop(ink(0x0A1A5C, 1.), 0.),
                linear_color_stop(ink(GROUND, 1.), 1.),
            ),
        );
        pen.strokes(&[[ox, hz, ox + w, hz]], 0x5C86D6, 1., 0.55);
    }

    grid(&mut pen, &cam, tree);

    // The tree lines: a faint wide glow under a thin bright line.
    let lines: Vec<[f32; 4]> = tree
        .lines
        .iter()
        .filter_map(|l| cam.segment([l[0], l[1], 0.], [l[2], l[3], 0.]))
        .collect();
    pen.strokes(&lines, LINE, 5., 0.1);
    pen.strokes(&lines, LINE, 1.4, 0.75);

    let mut hits = scene.hits.borrow_mut();
    hits.blocks.clear();
    hits.pedestals.clear();
    hits.cubes.clear();
    hits.groups.clear();

    // The cubes on the ground go first: a pedestal in front covers them.
    for cube in scene.cubes.iter().filter(|c| near && c.pos[2] < SLAB * 0.5) {
        paint_cube(&mut pen, &cam, eye, cube, scene.t, &mut hits);
    }

    // The pedestals from the back to the front: the slab, the spotlight pool and the name.
    let mut order: Vec<usize> = (0..tree.pedestals.len()).collect();
    let depth = |i: usize| cam.depth(tree.pedestals[i].centre());
    order.sort_by(|a, b| depth(*b).total_cmp(&depth(*a)));
    let mut labels: Vec<(u8, Label)> = Vec::new();
    for pi in order {
        let p = &tree.pedestals[pi];
        let alert = scene.alert.get(pi).copied().unwrap_or(false);
        let root = pi == 0;
        let chosen = scene.sel_pedestal == Some(pi);
        let edge = if alert {
            (mix(WAIT, WHITE, 0.2), 0.9)
        } else if chosen {
            (WHITE, 0.95)
        } else {
            (PED_EDGE, 0.75)
        };
        let top = if root { ROOT_TOP } else { PED_TOP };
        if let Some(hl) = cuboid(
            &mut pen,
            &cam,
            eye,
            (p.x, p.y, p.w, p.d),
            (0., SLAB),
            top,
            edge,
        ) {
            hits.pedestals.push((pi, hl));
        }
        // The pool of the spotlight on the pedestal top.
        if let Some(b) = scene.sel_block.filter(|b| tree.blocks[*b].pedestal == pi)
            && scene.spot > 0.
            && near
        {
            let foot = tree.blocks[b].foot();
            let pool = disc(&cam, [foot[0], foot[1], SLAB + 0.5], BLOCK * 1.05, 28);
            pen.polygon(&pool, BEAM, 0.24 * scene.spot);
        }
        // The name under the front edge of the pedestal.
        let fr = p.front();
        if cam.depth([fr[0], fr[1], 0.]) > NEAR {
            let a = cam.at([fr[0], fr[1], 0.]);
            labels.push((
                if chosen || alert { 2 } else { 3 },
                Label {
                    text: if root {
                        "fleet".to_string()
                    } else if near {
                        p.id.clone()
                    } else {
                        format!("{} · {}", p.id, scene.counts.get(pi).copied().unwrap_or(0))
                    },
                    size: 13.,
                    bold: true,
                    at: [a[0], a[1] + 6.],
                    color: if alert { 0xFFC9CB } else { WHITE },
                    alpha: 0.96,
                    group: (!root).then_some(pi),
                },
            ));
        }
    }

    // All blocks and raised cubes of all pedestals in one sort from the back to the front,
    // so that a tall block never covers a nearer block of the next pedestal (owner 2026-11-05).
    let mut items: Vec<(f32, Option<usize>, Option<usize>)> = Vec::new();
    for (bi, b) in tree.blocks.iter().enumerate() {
        if near {
            items.push((cam.depth(b.foot()), Some(bi), None));
        }
    }
    for (ci, cube) in scene.cubes.iter().enumerate() {
        let on = near && cube.pos[2] >= SLAB * 0.5;
        if on {
            items.push((cam.depth(cube.pos), None, Some(ci)));
        }
    }
    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, bi, ci) in items {
        if let Some(ci) = ci {
            paint_cube(&mut pen, &cam, eye, &scene.cubes[ci], scene.t, &mut hits);
            continue;
        }
        let Some(bi) = bi else { continue };
        let b = &tree.blocks[bi];
        let look = scene.looks[bi];
        let bh = scene.heights[bi];
        let sel = scene.sel_block == Some(bi);
        let mut color = look.color();
        if look == Look::Wait {
            let k = 0.5 + 0.5 * (scene.t * 2.4).sin();
            color = mix(color, 0xFF8A8D, 0.35 * k);
        }
        let edge = if sel {
            (WHITE, 1.)
        } else {
            (mix(color, WHITE, 0.45), 0.85)
        };
        let Some(hl) = cuboid(
            &mut pen,
            &cam,
            eye,
            (b.x, b.y, BLOCK, BLOCK),
            (SLAB, SLAB + bh),
            color,
            edge,
        ) else {
            continue;
        };
        let wide = hl.iter().map(|p| p[0]).fold(f32::MIN, f32::max)
            - hl.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
        let top = cam.at([b.x + BLOCK / 2., b.y + BLOCK / 2., SLAB + bh]);
        let roof_top = hl.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let prio = if sel {
            0
        } else if look == Look::Wait {
            1
        } else {
            4
        };
        if sel || look == Look::Wait || wide > 40. {
            labels.push((
                prio,
                Label {
                    text: scene.names.get(&b.name).cloned().unwrap_or_default(),
                    size: if sel { 13. } else { 11.5 },
                    bold: sel || look == Look::Wait,
                    at: [top[0], roof_top - 18.],
                    color: WHITE,
                    alpha: if sel || look == Look::Wait { 1. } else { 0.86 },
                    group: None,
                },
            ));
        }
        hits.blocks.push((bi, hl));
    }

    // The spotlight beam from above onto the selected block.
    if let Some(bi) = scene.sel_block
        && scene.spot > 0.
        && near
    {
        let b = &tree.blocks[bi];
        let foot = b.foot();
        let bh = scene.heights[bi];
        let base = disc(&cam, [foot[0], foot[1], SLAB + bh], BLOCK * 0.95, 20);
        let apex = disc(&cam, [foot[0], foot[1], SLAB + bh + 560.], 12., 12);
        let all: Vec<[f32; 2]> = base.iter().chain(apex.iter()).copied().collect();
        if cam.visible(&[[foot[0], foot[1], SLAB + bh + 560.]]) {
            let beam = hull(&all);
            pen.polygon_bg(
                &beam,
                linear_gradient(
                    180.,
                    linear_color_stop(ink(BEAM, 0.), 0.),
                    linear_color_stop(ink(BEAM, 0.26 * scene.spot), 1.),
                ),
            );
        }
    }

    // The labels: the selection first, then waits, pedestals, blocks; never on top of another.
    labels.sort_by_key(|(prio, _)| *prio);
    let mut placed: Vec<[f32; 4]> = Vec::new();
    for (_, l) in labels {
        let lw = f32::from(
            pen.shaped(&l.text, l.size, tk().sans, l.bold, l.color, l.alpha)
                .width(),
        );
        let r = [l.at[0] - lw / 2. - 3., l.at[1] - 1., lw + 6., l.size + 6.];
        let free = placed.iter().all(|q| {
            r[0] + r[2] <= q[0] || q[0] + q[2] <= r[0] || r[1] + r[3] <= q[1] || q[1] + q[3] <= r[1]
        });
        if !free || r[1] < oy + 4. || r[1] > oy + h {
            continue;
        }
        placed.push(r);
        if let Some(g) = l.group {
            hits.groups.push((g, [r[0], r[1] - 4., r[2], r[3] + 8.]));
        }
        pen.text(
            &l.text,
            l.size,
            l.size + 4.,
            tk().sans,
            l.bold,
            0x000000,
            0.75,
            (l.at[0] + 1., l.at[1] + 1.),
            Align::Center,
        );
        pen.text(
            &l.text,
            l.size,
            l.size + 4.,
            tk().sans,
            l.bold,
            l.color,
            l.alpha,
            (l.at[0], l.at[1]),
            Align::Center,
        );
    }

    hits.pose = Some(pose);
    hits.cam = Some(cam);
    hits.free = [
        ox + free_local[0],
        oy + free_local[1],
        ox + free_local[2],
        oy + free_local[3],
    ];
}

/// The faint ground grid, in five bands that fade with the distance.
fn grid(pen: &mut Painter, cam: &Cam, tree: &Tree) {
    let e = tree.extent;
    let step = 120.;
    let pad = 900.;
    let (x0, x1) = (
        ((e[0] - pad) / step).floor() * step,
        ((e[2] + pad) / step).ceil() * step,
    );
    let (y0, y1) = (
        ((e[1] - pad) / step).floor() * step,
        ((e[3] + pad * 1.6) / step).ceil() * step,
    );
    let (cx, cy) = ((x0 + x1) / 2., (y0 + y1) / 2.);
    let (hx, hy) = ((x1 - x0) / 2., (y1 - y0) / 2.);
    let mut bands: [Vec<[f32; 4]>; 5] = Default::default();
    let mut put = |a: [f32; 2], b: [f32; 2]| {
        let m = [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
        let edge = ((m[0] - cx).abs() / hx).max((m[1] - cy).abs() / hy);
        let k = (edge.powf(1.6) * 5.) as usize;
        if k < 5
            && let Some(s) = cam.segment([a[0], a[1], 0.], [b[0], b[1], 0.])
        {
            bands[k].push(s);
        }
    };
    let span = 2. * step;
    let mut x = x0;
    while x <= x1 {
        let mut y = y0;
        while y < y1 {
            put([x, y], [x, (y + span).min(y1)]);
            y += span;
        }
        x += step;
    }
    let mut y = y0;
    while y <= y1 {
        let mut x = x0;
        while x < x1 {
            put([x, y], [(x + span).min(x1), y]);
            x += span;
        }
        y += step;
    }
    for (k, segs) in bands.iter().enumerate() {
        pen.strokes(segs, GRID, 1., [0.34, 0.26, 0.18, 0.11, 0.05][k]);
    }
}

/// One agent cube with its glow.
fn paint_cube(pen: &mut Painter, cam: &Cam, eye: [f32; 3], cube: &Cube, t: f32, hits: &mut Hits) {
    let s = 14.;
    let [x, y, z] = cube.pos;
    if cam.depth(cube.pos) < NEAR * 2. {
        return;
    }
    let color = match cube.look {
        Look::Wait => 0xFFB3B5,
        Look::Error => 0xFFD89A,
        Look::Idle => 0xC9CED8,
        _ => 0xEAF2FF,
    };
    let glow = match cube.look {
        Look::Wait => WAIT,
        Look::Error => ERROR,
        Look::Idle => IDLE,
        _ => 0x8DB6FF,
    };
    let c = cam.at([x, y, z + s / 2.]);
    let r = (s * cam.f / cam.depth(cube.pos)).clamp(3., 18.);
    let breath = if cube.look == Look::Wait {
        0.75 + 0.25 * (t * 2.4).sin()
    } else {
        1.
    };
    pen.dot(c, r * 1.9, glow, 0.14 * breath);
    pen.dot(c, r * 1.1, glow, 0.3 * breath);
    let _ = cuboid(
        pen,
        cam,
        eye,
        (x - s / 2., y - s / 2., s, s),
        (z, z + s),
        color,
        (WHITE, 0.9),
    );
    hits.cubes.push((cube.id.clone(), c));
}

#[cfg(test)]
mod tests {
    use super::super::tree::layout;
    use super::*;

    fn tree() -> Tree {
        let input: Vec<(String, Vec<String>)> = (0..6)
            .map(|i| (format!("cb{i}"), (0..4).map(|k| format!("p{k}")).collect()))
            .collect();
        layout(&input)
    }

    #[test]
    fn the_home_view_shows_the_whole_tree() {
        let t = tree();
        let heights = vec![60.; t.blocks.len()];
        let free = free_rect(1440., 860.);
        let f = focal(860.);
        let pose = goal(Aim::Home, &t, &heights, free, f);
        let cam = Cam::new(pose, f, centre_of(free));
        for p in &t.pedestals {
            for c in corners(p.x, p.y, p.w, p.d, 0., SLAB) {
                let s = cam.at(c);
                assert!(s[0] >= free[0] - 0.5 && s[0] <= free[2] + 0.5);
                assert!(s[1] >= free[1] - 0.5 && s[1] <= free[3] + 0.5);
            }
        }
    }

    #[test]
    fn a_block_view_is_nearer_than_the_home_view() {
        let t = tree();
        let heights = vec![60.; t.blocks.len()];
        let free = free_rect(1440., 860.);
        let f = focal(860.);
        let home = goal(Aim::Home, &t, &heights, free, f);
        let near = goal(Aim::Block(5), &t, &heights, free, f);
        assert!(near.dist < home.dist);
        assert_eq!(near.target[0], t.blocks[5].foot()[0]);
    }

    #[test]
    fn the_pose_adds_the_orbit_and_the_zoom() {
        let g = Pose {
            target: [0.; 3],
            yaw: 0.,
            pitch: 0.5,
            dist: 1000.,
        };
        let p = pose_now(g, None, [0.2, 5.], 2f32.ln(), [0.; 2]);
        assert!((p.yaw - 0.2).abs() < 1e-6);
        assert_eq!(p.pitch, 1.2);
        assert!((p.dist - 2000.).abs() < 0.1);
        let mut from = g;
        from.dist = 500.;
        assert!((pose_now(g, Some((from, 0.)), [0.; 2], 0., [0.; 2]).dist - 500.).abs() < 0.1);
    }

    #[test]
    fn the_wait_look_wins() {
        assert!(Look::Wait > Look::Error && Look::Error > Look::Work && Look::Work > Look::Idle);
    }
}
