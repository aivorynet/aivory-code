//! The centre of the Holo war room: one neuron in 3D, after the owner's "Neural Synapse" pen
//! (`docs/design/refs/holo/synapse.txt`). The hub is the soma, a lumpy cell body. Each codebase
//! is a wandering, tapered dendrite from the soma into space; its sessions sit on thin twigs
//! at the end of it. Filler dendrites fill the rest of the sphere, as in the pen. A camera
//! orbits the neuron slowly (the pen's auto rotate, 0.7) with perspective and fog.
//!
//! The pen's two-step firing drives the motion: an orange pulse runs from a working session down
//! its twig and its dendrite into the soma, the soma flashes, and a rainbow activation wave runs
//! out over the whole tree (the hue turns with the angle and the distance, as in the pen's
//! palette). Working sessions take turns. Without a working session the tree stays dark teal.
//!
//! Frame cost: the 3D tree is built once for a set of sessions and a size (see `Cache`). A frame
//! projects about 1500 points and strokes the tree in nine large paths (three widths, three
//! depths). Only the segments inside the wave go through an extra batch, by hue band.

use std::f32::consts::{PI, TAU};

use aivory_protocol::SessionState;

use super::super::common::Painter;
use super::super::hud::Contact;
use crate::holo_kit::*;

/// The space above and below the network that the screen keeps for titles and the bottom lines.
const HEAD: f32 = 64.;
const FOOT: f32 = 100.;
/// The pen's network reaches about 50 of its units; a unit is `Layout::unit` pixels.
const REACH: f32 = 50.;
/// Main dendrites on the sphere (codebases first, then fillers).
const MAIN: usize = 16;

/// The camera: still, unless the owner drags it.
const PITCH: f32 = 0.45;
/// The camera distance in units of the network radius.
const CAMERA: f32 = 2.4;

/// The firing cycle in seconds, from the pen: the pulse runs 45 units at 35 units/s, the wave
/// runs to 75 units at 20 units/s, then a short rest.
const PULSE: f32 = 1.3;
const WAVE_SPEED: f32 = 20.;
const WAVE_END: f32 = 75.;
/// How long one firing shows.
pub(super) const FIRE_TIME: f32 = PULSE + WAVE_END / WAVE_SPEED;

/// Width classes of the tapered branches (pixels at scale 1) and fog by depth class.
const WIDTHS: [f32; 5] = [10., 7., 4.5, 2.6, 1.3];
const FOG: [f32; 2] = [1., 0.55];
/// The tube: a dark slate body and a light edge on the side of the light (top left).
const BODY: u32 = 0x325C6E;
const EDGE: u32 = 0x9CCFDE;
/// The soma shades, dark to light (slate to pale teal).
const SHADES: [u32; 12] = [
    0x0C1F28, 0x172C36, 0x223A44, 0x2D4852, 0x385561, 0x43636F, 0x4E717D, 0x597F8B, 0x648C9A,
    0x6F9AA8, 0x7AA8B6, 0x86B6C5,
];
/// Hue bands of the wave.
const BANDS: usize = 10;

/// The yellow of a session that waits and the red of an error.
const WAIT: u32 = 0xFFC83D;
const ERROR: u32 = 0xFF4D5E;
/// The dark teal of the pen's resting tree (base color plus fresnel).
const TEAL: u32 = 0x3E7E94;
/// The pen's input pulse: vec3(3.5, 1.0, 0.1) is an orange that burns to white in its core.
const PULSE_GLOW: u32 = 0xFF6A12;
const PULSE_CORE: u32 = 0xFFE2B0;

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len3(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: V3) -> V3 {
    mul(a, 1. / len3(a).max(1e-6))
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) struct Node {
    /// The neuron in space, around the soma.
    at: V3,
    /// The index in the contact list.
    pub idx: usize,
    /// 0 to 1, fixed per session.
    pub seed: f32,
    /// The way of a pulse: from the neuron down its twig and its dendrite into the soma.
    route: Vec<V3>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// The twig from a junction to a session.
    Twig(usize),
    /// The dendrite of a codebase: soma to junction.
    Main(usize),
    /// A side branch or a filler dendrite.
    Filler,
}

struct Branch {
    pts: Vec<V3>,
    /// The distance of each point from the soma, in pen units.
    dist: Vec<f32>,
    /// The width of each segment in pixels at scale 1 (the taper).
    width: Vec<f32>,
    kind: Kind,
}

pub(super) struct Cluster {
    junction: V3,
    pub name: String,
    pub nodes: Vec<usize>,
    /// The ring radius of its neurons in pixels: the label keeps clear of it.
    ring: f32,
}

/// The soma mesh: points, outward point normals (for smooth light), faces.
type Soma = (Vec<V3>, Vec<V3>, Vec<[u16; 3]>);

/// The 3D tree around the soma at (0, 0, 0).
pub(super) struct Layout {
    key: u64,
    /// The soma on screen, local to the centre region.
    pub hub: [f32; 2],
    pub nodes: Vec<Node>,
    pub clusters: Vec<Cluster>,
    branches: Vec<Branch>,
    /// The outline radius of the soma in pixels.
    soma_r: f32,
    /// Pixels per pen unit.
    unit: f32,
    /// The network radius in pixels.
    radius: f32,
    /// The screen stretch in x: the region is wider than high.
    stretch: f32,
    /// The region size, for the label clamps.
    cw: f32,
    bottom: f32,
    s: f32,
    /// Dust motes in space.
    dust: Vec<(V3, u8)>,
    /// The soma as a 3D mesh: the pen's bumped sphere, in pixels around (0, 0, 0).
    soma: Soma,
}

/// A point on screen with its depth (0 near to 1 far) and its perspective scale.
#[derive(Clone, Copy)]
struct Proj {
    xy: [f32; 2],
    depth: f32,
    scale: f32,
}

/// The mouse control of the camera: drag turns, the wheel zooms. The pen's auto rotate
/// goes on under it.
pub(in crate::warroom) struct Orbit {
    /// Yaw and pitch that the owner added by drag, in radians.
    pub yaw: f32,
    pub pitch: f32,
    /// 1 is the start; the wheel goes from 0.5 to 3.
    pub zoom: f32,
    /// The mouse position of the drag in progress.
    pub drag: Option<[f32; 2]>,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.,
            zoom: 1.,
            drag: None,
        }
    }
}

impl Orbit {
    /// Drag by `dx`, `dy` pixels.
    pub fn turn(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.006;
        self.pitch = (self.pitch + dy * 0.006).clamp(-PITCH - 1.2, 1.3 - PITCH);
    }
    /// A wheel step of `dy` pixels (negative zooms in).
    pub fn wheel(&mut self, dy: f32) {
        self.zoom = (self.zoom * (-dy * 0.0015).exp()).clamp(0.5, 3.);
    }
    /// The camera of this frame.
    pub fn cam(&self) -> Cam {
        Cam {
            yaw: self.yaw,
            pitch: PITCH + self.pitch,
            zoom: self.zoom,
        }
    }
}

/// The camera of one frame.
#[derive(Clone, Copy)]
pub(in crate::warroom) struct Cam {
    yaw: f32,
    pitch: f32,
    zoom: f32,
}

impl Layout {
    /// The camera `c` looks at the soma.
    fn project(&self, p: V3, c: Cam) -> Proj {
        let (sy, cy) = c.yaw.sin_cos();
        let (sp, cp) = c.pitch.sin_cos();
        let x1 = p[0] * cy + p[2] * sy;
        let z1 = -p[0] * sy + p[2] * cy;
        let y2 = p[1] * cp - z1 * sp;
        let z2 = p[1] * sp + z1 * cp;
        let d = CAMERA * self.radius;
        let scale = d / (d + z2).max(d * 0.2) * c.zoom;
        Proj {
            xy: [
                self.hub[0] + x1 * scale * self.stretch,
                self.hub[1] + y2 * scale,
            ],
            depth: ((z2 / self.radius + 1.) / 2.).clamp(0., 1.),
            scale,
        }
    }

    /// A point in camera space: x right, y down, z away from the camera.
    fn turn(&self, p: V3, c: Cam) -> V3 {
        let (sy, cy) = c.yaw.sin_cos();
        let (sp, cp) = c.pitch.sin_cos();
        let x1 = p[0] * cy + p[2] * sy;
        let z1 = -p[0] * sy + p[2] * cy;
        [x1, p[1] * cp - z1 * sp, p[1] * sp + z1 * cp]
    }

    /// The neuron of node `n` on screen at time `t`, local to the centre region.
    pub(super) fn node_at(&self, n: usize, c: Cam) -> [f32; 2] {
        self.project(self.nodes[n].at, c).xy
    }

    /// The label of cluster `k` on screen at time `t`: in space beyond the end of its
    /// dendrite, past the ring of its neurons, so it turns with the tree.
    pub(super) fn label_at(&self, k: usize, cam: Cam) -> [f32; 2] {
        let c = &self.clusters[k];
        let out = add(
            c.junction,
            mul(norm(c.junction), c.ring * 1.2 + 30. * self.s),
        );
        let p = self.project(out, cam).xy;
        [p[0].clamp(80., self.cw - 80.), p[1].clamp(14., self.bottom)]
    }
}

/// The layout that the last frame used.
#[derive(Default)]
pub(in crate::warroom) struct Cache(Option<std::rc::Rc<Layout>>);

impl Cache {
    /// The layout for these sessions and this region; it is built again only when one changes.
    pub(super) fn get(
        &mut self,
        list: &[Contact],
        books: &[String],
        cw: f32,
        ch: f32,
        s: f32,
    ) -> std::rc::Rc<Layout> {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for b in bytes {
                h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        for c in list {
            feed(c.id.as_bytes());
            feed(c.codebase.as_bytes());
            feed(&[0]);
        }
        feed(&(cw as i32).to_le_bytes());
        feed(&(ch as i32).to_le_bytes());
        if let Some(l) = self.0.as_ref().filter(|l| l.key == h) {
            return l.clone();
        }
        let l = std::rc::Rc::new(build(list, books, cw, ch, s, h));
        self.0 = Some(l.clone());
        l
    }
}

fn hash(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for b in s.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    h
}

fn seed(id: &str) -> f32 {
    (hash(id) % 10_000) as f32 / 10_000.
}

/// A small seeded random source, so a codebase keeps the shape of its dendrite.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(seed | 1)
    }
    /// 0 to 1.
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 % 100_000) as f32 / 100_000.
    }
    /// -0.5 to 0.5.
    fn jitter(&mut self) -> f32 {
        self.next() - 0.5
    }
    /// A random direction on the sphere.
    fn dir(&mut self) -> V3 {
        let phi = self.next() * TAU;
        let z = self.next() * 2. - 1.;
        let r = (1. - z * z).sqrt();
        [phi.cos() * r, z * 0.8, phi.sin() * r]
    }
}

/// The pen's `createWanderingPath`: a random walk from `start` in the direction `dir` with
/// `segments` steps; each step turns the direction by `jitter`. With `end`, the walk bends so
/// its last point lands on `end`.
fn wander(
    rng: &mut Rng,
    start: V3,
    dir: V3,
    length: f32,
    segments: usize,
    jitter: f32,
    end: Option<V3>,
) -> Vec<V3> {
    let step = length / segments as f32;
    let mut d = norm(dir);
    let mut pts = vec![start];
    let mut cur = start;
    for _ in 0..segments {
        d = norm(add(
            d,
            [
                rng.jitter() * jitter,
                rng.jitter() * jitter,
                rng.jitter() * jitter,
            ],
        ));
        cur = add(cur, mul(d, step));
        pts.push(cur);
    }
    if let Some(e) = end {
        let miss = sub(e, pts[segments]);
        for (i, p) in pts.iter_mut().enumerate() {
            *p = add(*p, mul(miss, i as f32 / segments as f32));
        }
        pts[segments] = e;
    }
    pts
}

fn length(pts: &[V3]) -> f32 {
    pts.windows(2).map(|w| len3(sub(w[1], w[0]))).sum()
}

impl Layout {
    /// Add a branch. `from` is its distance from the soma at the first point (pen units);
    /// `radius` is the width at the first point, `taper` makes it thin out to the tip as the
    /// pen does (`radius * (1 - u)^0.6`).
    fn add(&mut self, pts: Vec<V3>, from: f32, radius: f32, taper: bool, kind: Kind) {
        let n = pts.len() - 1;
        let mut dist = Vec::with_capacity(pts.len());
        let mut d = from;
        dist.push(d);
        for w in pts.windows(2) {
            d += len3(sub(w[1], w[0])) / self.unit;
            dist.push(d);
        }
        let width = (0..n)
            .map(|i| {
                let u = (i as f32 + 0.5) / n as f32;
                if taper {
                    radius * (1. - u).powf(0.6)
                } else {
                    radius
                }
            })
            .collect();
        self.branches.push(Branch {
            pts,
            dist,
            width,
            kind,
        });
    }

    /// Side branches off `main`, as in the pen: thinner and shorter further out.
    fn sides(&mut self, rng: &mut Rng, main: &[V3], count: usize, radius: f32) {
        let len = length(main);
        for _ in 0..count {
            let t = 0.2 + rng.next() * 0.6;
            let i = ((main.len() - 1) as f32 * t) as usize;
            let tangent = norm(sub(main[i + 1], main[i]));
            let dir = norm(add(norm(cross(tangent, rng.dir())), mul(tangent, 0.5)));
            let side_len = (1. - t) * len * (0.4 + rng.next() * 0.4);
            let pts = wander(rng, main[i], dir, side_len, 15, 0.6, None);
            let from = self.soma_r / self.unit + t * len / self.unit;
            self.add(pts, from, radius * (1. - t) * 0.8, true, Kind::Filler);
        }
    }
}

/// The pen's soma: an icosphere (three subdivisions, 1280 faces) whose points move out by
/// `sin(2x)cos(2y)sin(2z)*0.5 + sin(5x+3y)*0.2` (pen units, radius 3.3), scaled to `r` pixels.
fn soma_mesh(r: f32) -> Soma {
    let g = (1. + 5f32.sqrt()) / 2.;
    let mut pts: Vec<V3> = [
        [-1., g, 0.],
        [1., g, 0.],
        [-1., -g, 0.],
        [1., -g, 0.],
        [0., -1., g],
        [0., 1., g],
        [0., -1., -g],
        [0., 1., -g],
        [g, 0., -1.],
        [g, 0., 1.],
        [-g, 0., -1.],
        [-g, 0., 1.],
    ]
    .iter()
    .map(|p| norm(*p))
    .collect();
    let mut faces: Vec<[u16; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..3 {
        let mut mid = std::collections::HashMap::new();
        let mut next = Vec::with_capacity(faces.len() * 4);
        let mut half = |a: u16, b: u16, pts: &mut Vec<V3>| {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let (p, q) = (pts[a as usize], pts[b as usize]);
                pts.push(norm(mul(add(p, q), 0.5)));
                (pts.len() - 1) as u16
            })
        };
        for [a, b, c] in faces {
            let (ab, bc, ca) = (
                half(a, b, &mut pts),
                half(b, c, &mut pts),
                half(c, a, &mut pts),
            );
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    let pts = pts
        .into_iter()
        .map(|d| {
            let v = mul(d, 3.3);
            // The pen's bumps, a little softer at this size.
            let n = ((2. * v[0]).sin() * (2. * v[1]).cos() * (2. * v[2]).sin() * 0.5
                + (5. * v[0] + 3. * v[1]).sin() * 0.2)
                * 0.6;
            mul(d, (3.3 + n) / 3.3 * r)
        })
        .collect();
    // Point normals: the sum of the face normals around each point, outward.
    let pts: Vec<V3> = pts;
    let mut normals = vec![[0f32; 3]; pts.len()];
    for f in &faces {
        let [a, b, c] = f.map(usize::from);
        let mut n = cross(sub(pts[b], pts[a]), sub(pts[c], pts[a]));
        if dot(n, pts[a]) < 0. {
            n = mul(n, -1.);
        }
        for i in [a, b, c] {
            normals[i] = add(normals[i], n);
        }
    }
    let normals = normals.into_iter().map(norm).collect();
    (pts, normals, faces)
}

fn build(list: &[Contact], books: &[String], cw: f32, ch: f32, s: f32, key: u64) -> Layout {
    let avail = (ch - HEAD - FOOT).max(200.);
    let hub = [cw / 2., HEAD + avail / 2.];
    let rx = (cw / 2. - 110.).max(90.);
    let ry = (avail / 2. - 30.).max(80.);
    let radius = (ry * 1.25).min(rx / 1.0);
    let unit = radius / REACH * 1.1;
    let soma_r = (3.3 * unit).clamp(24. * s, 52. * s);
    let mut out = Layout {
        key,
        hub,
        nodes: Vec::new(),
        clusters: Vec::new(),
        branches: Vec::new(),
        soma_r,
        unit,
        radius,
        stretch: (rx / ry).clamp(1., 1.2),
        cw,
        bottom: HEAD + avail + 20.,
        s,
        dust: Vec::new(),
        soma: soma_mesh(soma_r * 1.8),
    };

    let books: Vec<&String> = books
        .iter()
        .filter(|b| list.iter().any(|c| &c.codebase == *b))
        .collect();
    let nc = books.len();
    let mut used: Vec<V3> = Vec::new();
    for (k, b) in books.iter().enumerate() {
        // Codebases around the vertical axis, a little above and below the equator.
        let a = -0.6 * PI + k as f32 * TAU / nc as f32;
        let el: f32 = [0.0, 0.42, -0.42][k % 3];
        let dir = [a.cos() * el.cos(), el.sin(), a.sin() * el.cos()];
        used.push(dir);
        let ring = if nc == 1 { 0.6 } else { 0.86 };
        let junction = mul(dir, radius * ring);
        let mut rng = Rng::new(hash(b));
        let start = mul(dir, soma_r * 0.8);
        let main = wander(
            &mut rng,
            start,
            dir,
            len3(sub(junction, start)),
            25,
            0.35,
            Some(junction),
        );
        let main_r = (0.4 + rng.next() * 0.3) * unit * 2.2;
        let count = 3 + (rng.next() * 4.) as usize;
        out.sides(&mut rng, &main, count, main_r);
        let cl = out.clusters.len();
        out.add(main.clone(), soma_r / unit, main_r, true, Kind::Main(cl));
        let main_b = out.branches.len() - 1;
        let junction_d = *out.branches[main_b].dist.last().unwrap_or(&0.);

        let members: Vec<usize> = (0..list.len())
            .filter(|&i| &list[i].codebase == *b)
            .collect();
        let m = members.len();
        let rr = if m == 1 {
            34. * s
        } else {
            (20. + 10. * m as f32).min(78.) * s
        };
        // The neurons sit on a ring across the end of the dendrite.
        let u = norm(cross(dir, [0., 1., 0.]));
        let v = cross(u, dir);
        let mut mine = Vec::new();
        for (j, &i) in members.iter().enumerate() {
            let na = j as f32 * TAU / m.max(1) as f32 + 0.4;
            let at = if m == 1 {
                add(junction, mul(dir, rr))
            } else {
                add(
                    junction,
                    add(
                        add(mul(u, na.cos() * rr), mul(v, na.sin() * rr)),
                        mul(dir, rr * 0.35),
                    ),
                )
            };
            let mut twig_rng = Rng::new(hash(&list[i].id));
            let twig = wander(
                &mut twig_rng,
                junction,
                sub(at, junction),
                rr,
                8,
                0.5,
                Some(at),
            );
            let n = out.nodes.len();
            let mut route: Vec<V3> = twig.iter().rev().copied().collect();
            route.extend(main.iter().rev().skip(1));
            route.push([0., 0., 0.]);
            out.add(twig, junction_d, 2., true, Kind::Twig(n));
            out.nodes.push(Node {
                at,
                idx: i,
                seed: seed(&list[i].id),
                route,
            });
            mine.push(n);
        }
        out.clusters.push(Cluster {
            junction,
            name: (*b).to_uppercase(),
            nodes: mine,
            ring: rr,
        });
    }

    // Filler dendrites: the pen's soma grows 18; they keep clear of the codebase dendrites.
    let mut rng = Rng::new(0x5EED_u32 ^ list.len() as u32);
    let mut tries = 0;
    while used.len() < MAIN.max(nc) && tries < 80 {
        tries += 1;
        let dir = norm(rng.dir());
        let clear = used.iter().all(|u| {
            let dot = u[0] * dir[0] + u[1] * dir[1] + u[2] * dir[2];
            dot < 0.9
        });
        if !clear {
            continue;
        }
        used.push(dir);
        let reach = (0.4 + rng.next() * 0.5) * radius;
        let pts = wander(&mut rng, mul(dir, soma_r * 0.8), dir, reach, 20, 0.4, None);
        let r = (0.4 + rng.next() * 0.3) * unit * 2.;
        let count = 2 + (rng.next() * 3.) as usize;
        out.sides(&mut rng, &pts, count, r);
        out.add(pts, soma_r / unit, r, true, Kind::Filler);
    }

    // Dust: the pen's three particle clouds, as a few motes in the space around the neuron.
    let mut rng = Rng::new(0xD057);
    out.dust = (0..120)
        .map(|i| {
            let d = norm(rng.dir());
            let r = (0.2 + 1.2 * rng.next().sqrt()) * radius;
            (mul(d, r), (i % 3) as u8)
        })
        .collect();
    out
}

/// The pen's cosine palette with a = b = 0.5, c = 1, d = (0, 0.33, 0.67): a rainbow.
fn palette(x: f32) -> u32 {
    let ch = |d: f32| ((0.5 + 0.5 * (TAU * (x + d)).cos()) * 255.) as u32;
    ch(0.) << 16 | ch(0.33) << 8 | ch(0.67)
}

/// The color of a session: yellow waits, red is an error, cyan works, slate is idle.
pub(super) fn tone(state: SessionState) -> u32 {
    match state {
        SessionState::NeedsYou => WAIT,
        SessionState::Error => ERROR,
        SessionState::Working => PK,
        _ => GY,
    }
}

/// The width class nearest to a tube diameter in pixels.
fn width_class(px: f32) -> usize {
    WIDTHS
        .iter()
        .position(|&w| px >= w * 0.8)
        .unwrap_or(WIDTHS.len() - 1)
}

fn depth_class(depth: f32) -> usize {
    usize::from(depth > 0.55)
}

/// A polyline moved by `d` pixels (the light edge of a tube).
fn moved(line: &[[f32; 2]], d: [f32; 2]) -> Vec<[f32; 2]> {
    line.iter().map(|q| [q[0] + d[0], q[1] + d[1]]).collect()
}

/// Paint the network. `o` is the corner of the centre region in window pixels.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint(
    p: &mut Painter,
    l: &Layout,
    list: &[Contact],
    fires: &[(usize, f32)],
    t: f32,
    cam: Cam,
    o: [f32; 2],
    s: f32,
) {
    let hub = [l.hub[0] + o[0], l.hub[1] + o[1]];
    let at = |q: V3| {
        let pr = l.project(q, cam);
        Proj {
            xy: [pr.xy[0] + o[0], pr.xy[1] + o[1]],
            ..pr
        }
    };
    // The state of each cluster: 2 waits, 1 works, 0 idle.
    let rank: Vec<u8> = l
        .clusters
        .iter()
        .map(|c| {
            c.nodes
                .iter()
                .map(|&n| match list[l.nodes[n].idx].state {
                    SessionState::NeedsYou | SessionState::Error => 2,
                    SessionState::Working => 1,
                    _ => 0,
                })
                .max()
                .unwrap_or(0)
        })
        .collect();

    // Dust: motes in space, dimmer in the fog.
    for (q, kind) in &l.dust {
        let pr = at(*q);
        let fog = 1. - pr.depth * 0.8;
        let (r, c, alpha) = match kind {
            0 => (0.9, 0x3A6688, 0.6),
            1 => (1.4, 0x1A5577, 0.5),
            _ => (1.8, 0x00AACC, 0.28),
        };
        p.dot(pr.xy, r * pr.scale, c, alpha * fog);
    }

    // Project the tree once: the points of every branch on screen.
    let proj: Vec<Vec<Proj>> = l
        .branches
        .iter()
        .map(|b| b.pts.iter().map(|q| at(*q)).collect())
        .collect();

    // The resting tree: dark teal, tapered, in the fog. One path per width and depth class;
    // a polyline breaks where its class changes.
    let k = s.max(0.8);
    let classes = WIDTHS.len() * 2;
    let mut tree: Vec<Vec<Vec<[f32; 2]>>> = vec![Vec::new(); classes];
    for (b, pts) in l.branches.iter().zip(&proj) {
        let class = |i: usize| {
            let depth = (pts[i].depth + pts[i + 1].depth) / 2.;
            width_class(b.width[i] * pts[i].scale * k) * 2 + depth_class(depth)
        };
        let mut cls = class(0);
        let mut run = vec![pts[0].xy];
        for i in 0..b.width.len() {
            let c = class(i);
            if c != cls {
                let last = pts[i].xy;
                tree[cls].push(std::mem::replace(&mut run, vec![last]));
                cls = c;
            }
            run.push(pts[i + 1].xy);
        }
        tree[cls].push(run);
    }
    // Far first, so the near branches lie on top. Each class is a solid tube: the dark body,
    // then a light edge toward the top left.
    for d in (0..2).rev() {
        for (w, &width) in WIDTHS.iter().enumerate() {
            let lines = &tree[w * 2 + d];
            if lines.is_empty() {
                continue;
            }
            p.polylines(lines, BODY, width, FOG[d] * 0.95);
            if width >= 2. {
                let off = [-width * 0.2, -width * 0.24];
                let edge: Vec<Vec<[f32; 2]>> = lines.iter().map(|l| moved(l, off)).collect();
                p.polylines(&edge, EDGE, (width * 0.3).max(1.), FOG[d] * 0.4);
            } else {
                p.polylines(lines, TEAL, width, FOG[d] * 0.5);
            }
        }
    }

    // State tints over the base: a twig and a dendrite that wait glow yellow, an error red,
    // a working twig a faint cyan.
    let mut warm = Vec::new();
    let mut red = Vec::new();
    let mut cyan = Vec::new();
    for (b, pts) in l.branches.iter().zip(&proj) {
        let v = match b.kind {
            Kind::Twig(n) => match list[l.nodes[n].idx].state {
                SessionState::NeedsYou => &mut warm,
                SessionState::Error => &mut red,
                SessionState::Working => &mut cyan,
                _ => continue,
            },
            Kind::Main(k) if rank[k] == 2 => &mut warm,
            Kind::Main(k) if rank[k] == 1 => &mut cyan,
            _ => continue,
        };
        v.push(pts.iter().map(|q| q.xy).collect::<Vec<_>>());
    }
    p.polylines(&cyan, PK, 1.2, 0.35);
    for (v, c) in [(&warm, WAIT), (&red, ERROR)] {
        p.polylines(v, c, 5., 0.07);
        p.polylines(v, c, 1.3, 0.85);
    }

    // The firing, only when a session changes state: the pulse runs in, then the wave out.
    let mut flash = 0f32;
    for &(contact, start) in fires {
        let Some(node) = l.nodes.iter().find(|n| n.idx == contact) else {
            continue;
        };
        let phase = t - start;
        if phase < 0. {
            continue;
        }
        if phase < PULSE {
            pulse(p, l, &node.route, phase / PULSE, &at);
        } else {
            let act = (phase - PULSE) * WAVE_SPEED;
            if act < WAVE_END {
                wave(p, l, &proj, act, t, k);
                flash = flash.max((-(act * 0.2).powi(2)).exp());
            }
        }
    }
    // The soma: the pen's bumped sphere in 3D, turned by the camera, lit from the top left.
    // Front faces only, in twelve shades: twelve paths. It lights up when a pulse arrives.
    let (verts, normals, faces) = &l.soma;
    let turned: Vec<V3> = normals.iter().map(|v| l.turn(*v, cam)).collect();
    let screen: Vec<[f32; 2]> = verts.iter().map(|v| at(*v).xy).collect();
    let light = norm([-0.45, -0.65, -0.6]);
    let mut shades: Vec<Vec<[[f32; 2]; 3]>> = vec![Vec::new(); SHADES.len()];
    let mut lit = Vec::new();
    for f in faces {
        let [a, b, c] = f.map(usize::from);
        // Smooth light: the mean of the turned point normals.
        let n = norm(add(add(turned[a], turned[b]), turned[c]));
        if n[2] > 0.15 {
            continue;
        }
        let lambert = dot(n, light).max(0.);
        // The pen's fresnel: a teal rim where the surface turns away.
        let rim = (1. + n[2]).powi(4) * 0.3;
        let shade = (0.08 + 0.62 * lambert + rim).clamp(0., 0.999);
        let tri = [screen[a], screen[b], screen[c]];
        shades[(shade * SHADES.len() as f32) as usize].push(tri);
        lit.push(tri);
    }
    for (tris, &c) in shades.iter().zip(SHADES.iter()) {
        p.polygons(tris, c, 1.);
    }
    if flash > 0.01 {
        p.polygons(&lit, 0xFFF0E0, 0.7 * flash);
        p.dot(hub, l.unit * 7. * k, 0xFFF0E0, 0.15 * flash);
    }

    // The junctions and the neurons, far ones first and smaller.
    for (ki, c) in l.clusters.iter().enumerate() {
        let j = at(c.junction);
        let color = [GY, PK, WAIT][rank[ki] as usize];
        p.dot(j.xy, 7. * j.scale, color, 0.1);
        p.dot(j.xy, 3. * j.scale, color, 0.8);
    }
    let mut order: Vec<(usize, Proj)> = l
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (i, at(n.at)))
        .collect();
    order.sort_by(|a, b| b.1.depth.total_cmp(&a.1.depth));
    for (i, pr) in order {
        let n = &l.nodes[i];
        let c = &list[n.idx];
        let pos = pr.xy;
        let color = tone(c.state);
        let waits = c.waits();
        let wave = 0.5;
        let z = k * pr.scale.clamp(0.7, 1.3);
        let fog = 1. - pr.depth * 0.35;
        if waits {
            p.dot(pos, (13. + 7. * wave) * z, color, (0.05 + 0.1 * wave) * fog);
            p.dot(pos, 9. * z, color, 0.18 * fog);
            p.dot(pos, 5. * z, color, fog);
        } else if c.state == SessionState::Working {
            // The halo in the pen's palette, fixed per session; the core stays light.
            let h = n.seed;
            p.dot(pos, (15. + 3. * wave) * z, palette(h), 0.1 * fog);
            p.dot(pos, 11. * z, palette(h + 0.33), 0.14 * fog);
            p.dot(pos, 7. * z, palette(h + 0.66), 0.22 * fog);
            p.dot(pos, 4.5 * z, PA, fog);
        } else {
            p.dot(pos, 3.5 * z, color, 0.7 * fog);
        }
        p.arc(pos, 11. * z, (0., TAU), 1.2, PA, 0.9 * fog, &|v| v);
    }
}

/// The pen's input pulse at `f` (0 to 1) along `route`: a hot core with a trail behind it.
fn pulse(p: &mut Painter, l: &Layout, route: &[V3], f: f32, at: &dyn Fn(V3) -> Proj) {
    let mut cum = vec![0.];
    for w in route.windows(2) {
        let last = cum[cum.len() - 1];
        cum.push(last + len3(sub(w[1], w[0])));
    }
    let total = cum[cum.len() - 1].max(1.);
    let point = |f: f32| {
        let want = f.clamp(0., 1.) * total;
        let i = cum.partition_point(|&c| c < want).clamp(1, route.len() - 1);
        let r = (want - cum[i - 1]) / (cum[i] - cum[i - 1]).max(1e-3);
        at(add(route[i - 1], mul(sub(route[i], route[i - 1]), r)))
    };
    let trail = (6. * l.unit / total).min(0.25);
    let tail: Vec<[f32; 2]> = (0..=6)
        .map(|i| point(f - trail * i as f32 / 6.).xy)
        .collect();
    let head = point(f);
    p.polylines(std::slice::from_ref(&tail), PULSE_GLOW, 6., 0.18);
    p.polylines(std::slice::from_ref(&tail), PULSE_GLOW, 2.2, 0.9);
    p.dot(head.xy, 9. * head.scale, PULSE_GLOW, 0.25);
    p.dot(head.xy, 3. * head.scale, PULSE_CORE, 1.);
}

/// The pen's activation wave at `act` pen units from the soma: a bright front and a fading
/// residue behind it, in the rainbow of the angle and the distance.
fn wave(p: &mut Painter, l: &Layout, proj: &[Vec<Proj>], act: f32, t: f32, k: f32) {
    let mut front: Vec<Vec<[f32; 4]>> = vec![Vec::new(); BANDS];
    let mut rest: Vec<Vec<[f32; 4]>> = vec![Vec::new(); BANDS];
    for (b, pts) in l.branches.iter().zip(proj) {
        for i in 0..b.width.len() {
            let d = (b.dist[i] + b.dist[i + 1]) / 2.;
            if d > act {
                continue;
            }
            let behind = act - d;
            let crest = (-(behind * behind) * 0.2).exp();
            // smoothstep(act, act - 25, d)
            let x = (behind / 25.).clamp(0., 1.);
            let residue = x * x * (3. - 2. * x) * (1. - act / WAVE_END).max(0.);
            // The pen's angle: atan(z, x) of the point in space.
            let q = b.pts[i];
            let hue = (q[2].atan2(q[0]) * 0.15 + d * 0.05 - t * 0.5).rem_euclid(1.);
            let band = ((hue * BANDS as f32) as usize).min(BANDS - 1);
            let (a, c) = (pts[i].xy, pts[i + 1].xy);
            let seg = [a[0], a[1], c[0], c[1]];
            if crest > 0.12 {
                front[band].push(seg);
            } else if residue > 0.15 {
                rest[band].push(seg);
            }
        }
    }
    for band in 0..BANDS {
        let c = palette((band as f32 + 0.5) / BANDS as f32);
        if !rest[band].is_empty() {
            p.strokes(&rest[band], c, 5. * k, 0.06);
            p.strokes(&rest[band], c, 1.8 * k, 0.6);
        }
        if !front[band].is_empty() {
            p.strokes(&front[band], c, 8. * k, 0.12);
            p.strokes(&front[band], c, 2.4 * k, 0.95);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: &str, book: &str) -> Contact {
        Contact {
            id: id.into(),
            title: id.into(),
            codebase: book.into(),
            state: SessionState::Working,
            tokens: 10,
            minutes: 0.,
            rate: 0.,
        }
    }

    #[test]
    fn the_layout_has_one_node_per_session_and_is_cached() {
        let list = vec![contact("a", "x"), contact("b", "x"), contact("c", "y")];
        let books = vec!["x".to_string(), "y".to_string()];
        let mut cache = Cache::default();
        let l = cache.get(&list, &books, 800., 700., 1.);
        assert_eq!(l.nodes.len(), 3);
        assert_eq!(l.clusters.len(), 2);
        assert!(std::rc::Rc::ptr_eq(
            &l,
            &cache.get(&list, &books, 800., 700., 1.)
        ));
        let more = vec![contact("a", "x")];
        let l2 = cache.get(&more, &books, 800., 700., 1.);
        assert_eq!(l2.nodes.len(), 1);
    }

    #[test]
    fn a_pulse_route_ends_in_the_soma_and_dendrites_reach_their_junction() {
        let list = vec![contact("a", "x"), contact("b", "y")];
        let books = vec!["x".to_string(), "y".to_string()];
        let l = build(&list, &books, 900., 800., 1., 0);
        for n in &l.nodes {
            assert_eq!(*n.route.last().unwrap(), [0., 0., 0.]);
            assert_eq!(n.route[0], n.at);
        }
        for b in &l.branches {
            if let Kind::Main(k) = b.kind {
                assert_eq!(*b.pts.last().unwrap(), l.clusters[k].junction);
            }
            assert!(b.dist.windows(2).all(|w| w[1] >= w[0]));
        }
        assert!(l.branches.len() > MAIN);
    }

    #[test]
    fn a_drag_turns_the_tree_and_keeps_the_soma_in_place() {
        let list = vec![contact("a", "x")];
        let books = vec!["x".to_string()];
        let l = build(&list, &books, 900., 800., 1., 0);
        let mut o = Orbit::default();
        assert_eq!(l.project([0., 0., 0.], o.cam()).xy, l.hub);
        let a = l.node_at(0, o.cam());
        o.turn(200., 0.);
        let b = l.node_at(0, o.cam());
        assert!((a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 1.);
    }
}
