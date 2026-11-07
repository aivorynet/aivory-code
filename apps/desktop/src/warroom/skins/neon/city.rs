//! The vector city: a perspective projection of a ground grid that fades with distance, road
//! strips with running light, district floors with ground plates, solid buildings (dark
//! faces, lit edges, windows), agent lights with trails, beacon pillars, map pins and the
//! beam tags. The buildings paint from the back to the front.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, Bounds, Pixels, Window, linear_color_stop, linear_gradient};

use super::layout::{BUILDING, City};
use super::pins::{Rect, inside, overlap, place};
use crate::warroom::skins::common::{Align, Painter, TextCache, ink, mix};

/// The ground of the screen.
pub(super) const BG: u32 = 0x050508;
pub(super) const YELLOW: u32 = 0xFCEE0A;
pub(super) const CYAN: u32 = 0x00F0FF;
pub(super) const RED: u32 = 0xFF003C;
pub(super) const MAGENTA: u32 = 0xFF2A6D;
/// The text on the dark ground.
pub(super) const PALE: u32 = 0xEDE6FF;
/// Secondary text.
pub(super) const MIST: u32 = 0x8F86B8;
/// Idle lights and quiet lines.
pub(super) const DIM: u32 = 0x4A4466;
pub(super) const ORBITRON: &str = "Orbitron";
pub(super) const CHAKRA: &str = "Chakra Petch";
// The type scale of the Neon: section title > entry text > label. Entry text stays under
// 0.8 x the section title (14 <= 0.8 x 18). Glow sits on frames and titles, never on text.
/// A section title of a panel (Orbitron capitals).
pub(super) const T_SECTION: f32 = 18.;
/// Entry text: rows, questions, list items (Chakra Petch).
pub(super) const T_ENTRY: f32 = 14.;
/// A label, a note, a count.
pub(super) const T_LABEL: f32 = 12.;
/// The height of the district board at the bottom and its gap to the bottom edge.
pub(super) const BOARD_H: f32 = 124.;
pub(super) const BOARD_GAP: f32 = 22.;
/// The room that the HUD column takes on the right.
pub(super) const PANEL_W: f32 = 448.;
/// The height of a beacon pillar over its light, in world units.
const PILLAR: f32 = 330.;

/// What a light shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    Work,
    Beacon,
    Error,
    Idle,
}

/// The light of one live session.
#[derive(Debug, Clone)]
pub(super) struct Light {
    pub id: String,
    pub pos: [f32; 3],
    pub trail: Vec<[f32; 3]>,
    pub look: Look,
    /// The top of the building under the light.
    pub base: f32,
    /// The pin text of a beacon ("PERMISSION 6M").
    pub label: Option<String>,
    /// The place of a beacon in the queue (the longest wait first).
    pub rank: usize,
    pub selected: bool,
}

/// The pin of a lit or selected building.
#[derive(Debug, Clone)]
pub(super) struct BuildingPin {
    pub building: usize,
    pub text: String,
    /// The session that a click on the pin selects.
    pub id: String,
    pub selected: bool,
}

/// The click targets that the canvas writes: pin boxes first, then the lights.
#[derive(Default)]
pub(super) struct Hits {
    pub pins: Vec<(String, Rect)>,
    pub lights: Vec<(String, [f32; 2])>,
}

/// Everything that one frame needs.
pub(super) struct Scene {
    pub city: City,
    /// Per building: a session works or asks there.
    pub hot: Vec<bool>,
    /// Per building: a session has an error there.
    pub fault: Vec<bool>,
    /// Per district: the busy sessions and all live sessions.
    pub load: Vec<(usize, usize)>,
    pub pins: Vec<BuildingPin>,
    pub lights: Vec<Light>,
    pub t: f32,
    /// 0 to 1 while the screen glitches after a change.
    pub glitch: f32,
    pub hits: Rc<RefCell<Hits>>,
    /// The shaped text that lives between frames.
    pub cache: Rc<RefCell<TextCache>>,
}

/// The height of a building with the lift of an active one.
pub(super) fn top(h: f32, hot: bool) -> f32 {
    h + if hot { 40. } else { 0. }
}

/// A rect with two cut corners (top right and bottom left), clockwise from the top left.
pub(super) fn chamfer(r: Rect, cut: f32) -> Vec<[f32; 2]> {
    let [x, y, w, h] = r;
    let c = cut.min(w / 2.).min(h / 2.);
    vec![
        [x, y],
        [x + w - c, y],
        [x + w, y + c],
        [x + w, y + h],
        [x + c, y + h],
        [x, y + h - c],
    ]
}

/// The closed outline of a polygon as segments.
fn ring(pts: &[[f32; 2]]) -> Vec<[f32; 4]> {
    (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            [a[0], a[1], b[0], b[1]]
        })
        .collect()
}

/// A small stable hash for the window pattern and the glitch slices.
fn noise(a: u32, b: u32) -> u32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^ (h >> 13)
}

/// The camera: the world turns by `th`, tilts by `ph` and looks from `dist`.
struct Cam {
    cs: f32,
    sn: f32,
    cp: f32,
    sp: f32,
    f: f32,
    dist: f32,
}

impl Cam {
    fn new(half: (f32, f32)) -> Self {
        // A fixed angle: a tower stands still until a session starts or ends.
        let th = -0.62f32;
        let ph = 0.95f32;
        Self {
            cs: th.cos(),
            sn: th.sin(),
            cp: ph.cos(),
            sp: ph.sin(),
            f: 900.,
            dist: 1500f32.max(half.0.max(half.1) * 1.9),
        }
    }

    /// The distance of a world point from the camera.
    fn depth(&self, x: f32, y: f32, z: f32) -> f32 {
        -z * self.sp + (x * self.sn + y * self.cs) * self.cp + self.dist
    }

    /// The unscaled screen position of a world point.
    fn proj(&self, x: f32, y: f32, z: f32) -> [f32; 2] {
        let sx = x * self.cs - y * self.sn;
        let depth0 = x * self.sn + y * self.cs;
        let up = z * self.cp + depth0 * self.sp;
        let depth = self.depth(x, y, z);
        [sx * self.f / depth, -up * self.f / depth]
    }
}

/// The projection with the fit into the free area of the canvas.
struct View {
    cam: Cam,
    scale: f32,
    ox: f32,
    oy: f32,
}

impl View {
    fn new(half: (f32, f32), bounds: Bounds<Pixels>) -> Self {
        let cam = Cam::new(half);
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for sx in [-half.0, half.0] {
            for sy in [-half.1, half.1] {
                for z in [0., 200.] {
                    let p = cam.proj(sx, sy, z);
                    for i in 0..2 {
                        lo[i] = lo[i].min(p[i]);
                        hi[i] = hi[i].max(p[i]);
                    }
                }
            }
        }
        let right = if w > 900. { w - PANEL_W } else { w - 24. };
        let (ax, ay) = (28., 28.);
        // Room at the bottom for the ticker and the ground plates under the front corner.
        let (aw, ah) = (right - ax, h - BOARD_H - BOARD_GAP - 8. - ay);
        let scale = (aw / (hi[0] - lo[0])).min(ah / (hi[1] - lo[1])).min(1.6);
        Self {
            ox: f32::from(bounds.origin.x) + ax + (aw - (hi[0] - lo[0]) * scale) / 2.
                - lo[0] * scale,
            oy: f32::from(bounds.origin.y) + ay + (ah - (hi[1] - lo[1]) * scale) / 2.
                - lo[1] * scale,
            cam,
            scale,
        }
    }

    /// The window position of a world point.
    fn at(&self, x: f32, y: f32, z: f32) -> [f32; 2] {
        let p = self.cam.proj(x, y, z);
        [self.ox + p[0] * self.scale, self.oy + p[1] * self.scale]
    }

    fn line(&self, a: [f32; 3], b: [f32; 3]) -> [f32; 4] {
        let (p, q) = (self.at(a[0], a[1], a[2]), self.at(b[0], b[1], b[2]));
        [p[0], p[1], q[0], q[1]]
    }

    /// The four ground corners of a world rect.
    fn quad(&self, x: f32, y: f32, w: f32, d: f32, z: f32) -> [[f32; 2]; 4] {
        [
            self.at(x, y, z),
            self.at(x + w, y, z),
            self.at(x + w, y + d, z),
            self.at(x, y + d, z),
        ]
    }
}

/// Paint one line of text centred in a box of height `h`; `y` is the top of the box.
#[allow(clippy::too_many_arguments)]
fn label(
    pen: &mut Painter,
    s: &str,
    size: f32,
    family: &'static str,
    rgb: u32,
    alpha: f32,
    (x, y, h): (f32, f32, f32),
    align: Align,
) -> f32 {
    pen.text(s, size, h, family, true, rgb, alpha, (x, y), align)
}

/// Paint the whole city, the lights, the pins and the ticker into the canvas.
pub(super) fn paint(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let mut cache = scene.cache.borrow_mut();
    let mut pen = Painter::new(window, cx, &mut cache);
    let city = &scene.city;
    let v = View::new(city.half, bounds);
    let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    pen.rect((x0, y0, w, h), BG, 1.);

    ground(&mut pen, &v, city);
    buildings(&mut pen, &v, scene);
    lights(&mut pen, &v, scene);
    pins(&mut pen, &v, scene, (x0, y0, w, h));
    finish(&mut pen, scene, (x0, y0, w, h));
}

/// The ground: grid, roads, district floors with corner brackets, the glow of lit buildings.
fn ground(pen: &mut Painter, v: &View, city: &City) {
    // The grid fades with distance and toward its edge, in five bands.
    let (gx, gy) = (city.half.0 + 560., city.half.1 + 560.);
    let (near, far) = {
        let ds = [
            v.cam.depth(-gx, -gy, 0.),
            v.cam.depth(gx, -gy, 0.),
            v.cam.depth(gx, gy, 0.),
            v.cam.depth(-gx, gy, 0.),
        ];
        (
            ds.iter().copied().fold(f32::MAX, f32::min),
            ds.iter().copied().fold(f32::MIN, f32::max),
        )
    };
    let mut bands: [Vec<[f32; 4]>; 5] = Default::default();
    let mut put = |a: [f32; 2], b: [f32; 2]| {
        let (mx, my) = ((a[0] + b[0]) / 2., (a[1] + b[1]) / 2.);
        let d = (v.cam.depth(mx, my, 0.) - near) / (far - near);
        let edge = (mx.abs() / gx).max(my.abs() / gy);
        let k = (d.max(edge * edge) * 5.) as usize;
        if k < 5 {
            bands[k].push(v.line([a[0], a[1], 0.], [b[0], b[1], 0.]));
        }
    };
    let step = 80.;
    let mut g = -(gx / step).floor() * step;
    while g <= gx {
        let mut s = -gy;
        while s < gy {
            put([g, s], [g, (s + 2. * step).min(gy)]);
            s += 2. * step;
        }
        g += step;
    }
    let mut g = -(gy / step).floor() * step;
    while g <= gy {
        let mut s = -gx;
        while s < gx {
            put([s, g], [(s + 2. * step).min(gx), g]);
            s += 2. * step;
        }
        g += step;
    }
    for (k, segs) in bands.iter().enumerate() {
        pen.strokes(segs, CYAN, 1., [0.15, 0.11, 0.08, 0.05, 0.025][k]);
    }

    // The roads: a dark strip with thin curbs and light that runs along the middle.
    let mut curbs = Vec::new();
    let mut runs = Vec::new();
    let shift = 0f32;
    for r in &city.roads {
        let (dx, dy) = (r[2] - r[0], r[3] - r[1]);
        let len = dx.hypot(dy);
        if len < 1. {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (-uy * 11., ux * 11.);
        let strip = [
            v.at(r[0] + nx, r[1] + ny, 0.),
            v.at(r[2] + nx, r[3] + ny, 0.),
            v.at(r[2] - nx, r[3] - ny, 0.),
            v.at(r[0] - nx, r[1] - ny, 0.),
        ];
        pen.polygon(&strip, 0x0E0B1C, 0.95);
        curbs.push([strip[0][0], strip[0][1], strip[1][0], strip[1][1]]);
        curbs.push([strip[2][0], strip[2][1], strip[3][0], strip[3][1]]);
        let mut s = shift - 70.;
        while s < len {
            let (a, b) = (s.max(0.), (s + 20.).min(len));
            if b > a {
                runs.push(v.line(
                    [r[0] + ux * a, r[1] + uy * a, 0.],
                    [r[0] + ux * b, r[1] + uy * b, 0.],
                ));
            }
            s += 70.;
        }
    }
    pen.strokes(&curbs, 0x3B3170, 1., 0.8);
    pen.neon(&runs, CYAN, 1.2, 0.45);

    // The district floors: a faint fill, a thin outline and bright corner brackets.
    for d in &city.districts {
        let c = v.quad(d.x, d.y, d.w, d.d, 0.);
        pen.polygon(&c, d.color, 0.045);
        pen.strokes(&ring(&c), d.color, 1., 0.32);
        let arm = 34f32.min(d.w / 3.).min(d.d / 3.);
        let corners = [
            ([d.x, d.y], [1., 1.]),
            ([d.x + d.w, d.y], [-1., 1.]),
            ([d.x + d.w, d.y + d.d], [-1., -1.]),
            ([d.x, d.y + d.d], [1., -1.]),
        ];
        let mut brackets = Vec::new();
        for (p, s) in corners {
            brackets.push(v.line([p[0], p[1], 0.], [p[0] + s[0] * arm, p[1], 0.]));
            brackets.push(v.line([p[0], p[1], 0.], [p[0], p[1] + s[1] * arm, 0.]));
        }
        pen.neon(&brackets, d.color, 1.6, 0.9);
    }
}

/// The buildings from the back to the front: dark faces, a lit roof, windows and edges.
fn buildings(pen: &mut Painter, v: &View, scene: &Scene) {
    let t = scene.t;
    let city = &scene.city;
    let mut order: Vec<usize> = (0..city.buildings.len()).collect();
    let depth = |i: usize| {
        let b = &city.buildings[i];
        v.cam.depth(b.x + BUILDING / 2., b.y + BUILDING / 2., 0.)
    };
    order.sort_by(|a, b| depth(*b).total_cmp(&depth(*a)));
    let foot = [
        (0., 0.),
        (BUILDING, 0.),
        (BUILDING, BUILDING),
        (0., BUILDING),
    ];
    for i in order {
        let b = &city.buildings[i];
        let (hot, fault) = (scene.hot[i], scene.fault[i]);
        let base = city.districts[b.district].color;
        let color = if fault {
            if ((t * 2.) as u32).is_multiple_of(2) {
                RED
            } else {
                0xFF5C7F
            }
        } else {
            base
        };
        let h = top(b.h, hot || fault);
        let wx = |k: usize| b.x + foot[k].0;
        let wy = |k: usize| b.y + foot[k].1;
        let lo: Vec<[f32; 2]> = (0..4).map(|k| v.at(wx(k), wy(k), 0.)).collect();
        let hi: Vec<[f32; 2]> = (0..4).map(|k| v.at(wx(k), wy(k), h)).collect();
        let centre_d = v.cam.depth(b.x + BUILDING / 2., b.y + BUILDING / 2., 0.);
        let centre_x = (lo[0][0] + lo[2][0]) / 2.;

        // The glow of a lit building on the ground.
        if hot || fault {
            let g = v.quad(b.x - 16., b.y - 16., BUILDING + 32., BUILDING + 32., 0.);
            pen.polygon(&g, color, 0.08);
        }
        // The sides that face the camera: the left one catches the light.
        let sides: Vec<usize> = (0..4)
            .filter(|&k| {
                let j = (k + 1) % 4;
                v.cam.depth((wx(k) + wx(j)) / 2., (wy(k) + wy(j)) / 2., 0.) < centre_d
            })
            .collect();
        let mut windows = Vec::new();
        let mut edges = Vec::new();
        for &k in &sides {
            let j = (k + 1) % 4;
            let face = [lo[k], lo[j], hi[j], hi[k]];
            let left = (lo[k][0] + lo[j][0]) / 2. < centre_x;
            let tint = match (left, hot || fault) {
                (true, true) => 0.2,
                (false, true) => 0.11,
                (true, false) => 0.09,
                (false, false) => 0.05,
            };
            pen.polygon(&face, mix(BG, color, tint), 0.95);
            edges.push([lo[k][0], lo[k][1], lo[j][0], lo[j][1]]);
            edges.push([lo[k][0], lo[k][1], hi[k][0], hi[k][1]]);
            edges.push([lo[j][0], lo[j][1], hi[j][0], hi[j][1]]);
            if hot {
                // Rows of windows: a fixed pattern per building.
                let mut z = 14.;
                let mut floor = 0u32;
                while z < h - 8. {
                    for slot in 0..3u32 {
                        let n = noise(i as u32 * 31 + k as u32, floor * 7 + slot);
                        if n % 5 < 3 {
                            let (a, e) = (10. + slot as f32 * 20., 24. + slot as f32 * 20.);
                            let p = |s: f32| {
                                [
                                    wx(k) + (wx(j) - wx(k)) * s / BUILDING,
                                    wy(k) + (wy(j) - wy(k)) * s / BUILDING,
                                    z,
                                ]
                            };
                            windows.push(v.line(p(a), p(e)));
                        }
                    }
                    z += 12.;
                    floor += 1;
                }
            }
        }
        pen.polygon(
            &hi,
            mix(BG, color, if hot || fault { 0.34 } else { 0.13 }),
            0.97,
        );
        pen.strokes(&windows, mix(color, 0xFFFFFF, 0.25), 2., 0.8);
        let roof = ring(&hi);
        if hot || fault {
            pen.neon(&edges, color, 1.2, 0.85);
            pen.neon(&roof, mix(color, 0xFFFFFF, 0.3), 1.4, 1.);
        } else {
            pen.strokes(&edges, color, 1., 0.38);
            pen.strokes(&roof, color, 1., 0.55);
        }
    }
}

/// The agent lights with trails, and the beacon pillars.
fn lights(pen: &mut Painter, v: &View, scene: &Scene) {
    let t = scene.t;
    let mut hits = scene.hits.borrow_mut();
    hits.lights.clear();
    for l in &scene.lights {
        let col = match l.look {
            Look::Work => CYAN,
            Look::Beacon => MAGENTA,
            Look::Error => RED,
            Look::Idle => DIM,
        };
        let p = v.at(l.pos[0], l.pos[1], l.pos[2]);
        if l.look != Look::Idle && l.trail.len() > 2 {
            let segs: Vec<[f32; 4]> = l.trail.windows(2).map(|w| v.line(w[0], w[1])).collect();
            // Three bands fade the trail from the tail to the head.
            let band = segs.len().div_ceil(3);
            for (k, seg) in segs.chunks(band).enumerate() {
                pen.neon(seg, col, 1.2, 0.2 + 0.25 * k as f32);
            }
        }
        let foot = v.at(l.pos[0], l.pos[1], l.base);
        pen.strokes(&[[foot[0], foot[1], p[0], p[1]]], col, 1., 0.45);
        if matches!(l.look, Look::Beacon | Look::Error) {
            let pulse = 0.75 + (t * 2.4).sin() * 0.25;
            let tip = v.at(l.pos[0], l.pos[1], l.pos[2] + PILLAR);
            // The pillar: a soft column of light that fades to the top, and a bright core.
            let wide = if l.selected { 9. } else { 6. };
            pen.polygon_bg(
                &[
                    [p[0] - wide, p[1]],
                    [p[0] + wide, p[1]],
                    [tip[0] + 1., tip[1]],
                    [tip[0] - 1., tip[1]],
                ],
                linear_gradient(
                    0.,
                    linear_color_stop(ink(col, 0.7 * pulse), 0.),
                    linear_color_stop(ink(col, 0.), 1.),
                ),
            );
            let mid = v.at(l.pos[0], l.pos[1], l.pos[2] + PILLAR * 0.45);
            pen.neon(&[[p[0], p[1], mid[0], mid[1]]], col, 1.8, 0.9 * pulse);
            pen.strokes(&[[mid[0], mid[1], tip[0], tip[1]]], col, 1.5, 0.35 * pulse);
            // A bright pulse that climbs the core, eased out at the top.
            let u = (t * 0.45 + l.rank as f32 * 0.37) % 1.;
            let rise = 1. - (1. - u).powi(2);
            let a = v.at(l.pos[0], l.pos[1], l.pos[2] + PILLAR * rise * 0.9);
            let b = v.at(l.pos[0], l.pos[1], l.pos[2] + PILLAR * (rise * 0.9 + 0.08));
            pen.strokes(&[[a[0], a[1], b[0], b[1]]], 0xFFFFFF, 2.5, 0.8 * (1. - u));
            // A ring on the roof that grows and fades.
            let phase = (t * 0.6 + l.rank as f32 * 0.29) % 1.;
            let r = 14. + phase * 40.;
            let pts: Vec<[f32; 2]> = (0..=28)
                .map(|i| {
                    let a = i as f32 / 28. * std::f32::consts::TAU;
                    v.at(l.pos[0] + a.cos() * r, l.pos[1] + a.sin() * r, l.base)
                })
                .collect();
            pen.polyline(&pts, col, 1.5, (1. - phase) * 0.9, false);
        }
        let on = l.look != Look::Error || !((t * 3.) as u32).is_multiple_of(4);
        if on {
            let r = if l.selected { 6. } else { 4.5 };
            if l.look != Look::Idle {
                pen.dot(p, r * 2.8, col, 0.1);
                pen.dot(p, r * 1.7, col, 0.25);
            }
            pen.dot(p, r, col, 1.);
            if l.look != Look::Idle {
                pen.dot(p, r * 0.45, 0xFFFFFF, 0.9);
            }
        }
        if l.selected {
            // A target reticle: four corner ticks around the light.
            let (s, a) = (13., 5.);
            let mut ticks = Vec::new();
            for (dx, dy) in [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)] {
                let (cx, cy) = (p[0] + dx * s, p[1] + dy * s);
                ticks.push([cx, cy, cx - dx * a, cy]);
                ticks.push([cx, cy, cx, cy - dy * a]);
            }
            pen.strokes(&ticks, 0xFFFFFF, 1.5, 1.);
        }
        hits.lights.push((l.id.clone(), p));
    }
}

/// The ground plates of the districts, the beacon pins and the building pins. The plates
/// stand at a fixed corner; the pins take free spots in a collision pass.
fn pins(pen: &mut Painter, v: &View, scene: &Scene, (x0, y0, w, h): (f32, f32, f32, f32)) {
    let city = &scene.city;
    let right = if w > 900. { PANEL_W } else { 24. };
    let area: Rect = [
        x0 + 8.,
        y0 + 8.,
        w - right - 8.,
        h - BOARD_H - BOARD_GAP - 14.,
    ];
    let board = [
        x0,
        y0 + h - BOARD_H - BOARD_GAP,
        w - right,
        BOARD_H + BOARD_GAP,
    ];
    let mut taken: Vec<Rect> = vec![board, [x0 + w - right, y0, right, h]];
    let mut hits = scene.hits.borrow_mut();
    hits.pins.clear();
    let small = Painter::scaled(11.);
    let big = Painter::scaled(12.);
    let box_h = (big * 1.8).round();

    // The screen boxes of the buildings: a plate keeps off them.
    let blocks: Vec<Rect> = city
        .buildings
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let hgt = top(b.h, scene.hot[i] || scene.fault[i]);
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for (dx, dy) in [
                (0., 0.),
                (BUILDING, 0.),
                (BUILDING, BUILDING),
                (0., BUILDING),
            ] {
                for z in [0., hgt] {
                    let p = v.at(b.x + dx, b.y + dy, z);
                    for k in 0..2 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
            }
            [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]]
        })
        .collect();

    // The ground plates: a tag that hangs under a corner of the district, the front corner
    // first, on the first spot that is free of buildings and of other plates.
    for (di, d) in city.districts.iter().enumerate() {
        let c4 = v.quad(d.x, d.y, d.w, d.d, 0.);
        let pick = |f: &dyn Fn(&[f32; 2]) -> f32| {
            c4.iter()
                .copied()
                .max_by(|a, b| f(a).total_cmp(&f(b)))
                .unwrap_or([0., 0.])
        };
        let (front, left, right) = (pick(&|p| p[1]), pick(&|p| -p[0]), pick(&|p| p[0]));
        let name = pen.fit(&d.id.to_uppercase(), big, ORBITRON, true, 190.);
        let (busy, all) = scene.load.get(di).copied().unwrap_or((0, 0));
        let count = format!("{busy}/{all}");
        let code = format!("{:02}", di + 1);
        let wn = f32::from(pen.shaped(&name, big, ORBITRON, true, 0, 1.).width());
        let wc = f32::from(pen.shaped(&count, small, CHAKRA, true, 0, 1.).width());
        let wk = f32::from(pen.shaped(&code, small, CHAKRA, true, 0, 1.).width());
        let block = wk + 12.;
        let pw = block + 10. + wn + 12. + wc + 10.;
        // A spot: the corner, the share of the plate left of the corner, the drop.
        let (below, lower, above) = (12., 20. + box_h, -12. - box_h);
        let spots = [
            (front, 0.5, below),
            (front, 1., below),
            (front, 0., below),
            (left, 1., below),
            (left, 0.5, below),
            (right, 0., below),
            (right, 0.5, below),
            (front, 0.5, lower),
            (left, 1., above),
            (right, 0., above),
        ];
        let at = |&(c, k, dy): &([f32; 2], f32, f32)| (c, [c[0] - pw * k, c[1] + dy, pw, box_h]);
        let free = |r: &Rect, solid: bool| {
            inside(r, &area)
                && !taken.iter().any(|t| overlap(t, r))
                && !(solid && blocks.iter().any(|b| overlap(b, r)))
        };
        let (corner, rect) = spots
            .iter()
            .map(at)
            .find(|(_, r)| free(r, true))
            .or_else(|| spots.iter().map(at).find(|(_, r)| free(r, false)))
            .unwrap_or_else(|| at(&spots[0]));
        taken.push(rect);
        let edge = if rect[1] > corner[1] {
            rect[1]
        } else {
            rect[1] + box_h
        };
        pen.strokes(&[[corner[0], corner[1], corner[0], edge]], d.color, 1., 0.7);
        pen.dot(corner, 2.5, d.color, 1.);
        let shape = chamfer(rect, 7.);
        pen.polygon(&shape, BG, 0.92);
        pen.polygon(&shape, d.color, 0.08);
        pen.polyline(&shape, d.color, 1., 0.85, true);
        pen.polygon(
            &chamfer([rect[0], rect[1], block, box_h], 7.),
            d.color,
            0.95,
        );
        let mid = rect[0] + block / 2.;
        label(
            pen,
            &code,
            small,
            CHAKRA,
            BG,
            1.,
            (mid, rect[1], box_h),
            Align::Center,
        );
        let tx = rect[0] + block + 10.;
        label(
            pen,
            &name,
            big,
            ORBITRON,
            d.color,
            1.,
            (tx, rect[1], box_h),
            Align::Left,
        );
        let cx2 = rect[0] + pw - 10.;
        label(
            pen,
            &count,
            small,
            CHAKRA,
            PALE,
            0.75,
            (cx2, rect[1], box_h),
            Align::Right,
        );
    }

    // The beacon pins: on the pillar, as high as a free spot allows.
    let mut beacons: Vec<&Light> = scene
        .lights
        .iter()
        .filter(|l| l.label.is_some() && matches!(l.look, Look::Beacon | Look::Error))
        .collect();
    beacons.sort_by_key(|l| (!l.selected, l.rank));
    for l in beacons {
        let text = l.label.clone().unwrap_or_default();
        let col = if l.look == Look::Error { RED } else { MAGENTA };
        let tw = f32::from(pen.shaped(&text, small, CHAKRA, true, 0, 1.).width());
        let size = (tw + 22., box_h - 2.);
        let spot = [1.04, 0.9, 0.7, 0.5].into_iter().find_map(|k| {
            let anchor = v.at(l.pos[0], l.pos[1], l.pos[2] + PILLAR * k);
            place(anchor, size, &mut taken, &area).map(|p| (anchor, p))
        });
        let Some((anchor, p)) = spot else { continue };
        let r = p.rect;
        pen.polyline(&[anchor, p.knee], col, 1.2, 0.95, false);
        let shape = chamfer(r, 6.);
        pen.polygon(&shape, col, 0.95);
        if l.selected {
            pen.polyline(&shape, 0xFFFFFF, 1.5, 1., true);
        }
        label(
            pen,
            &text,
            small,
            CHAKRA,
            BG,
            1.,
            (r[0] + r[2] / 2., r[1], r[3]),
            Align::Center,
        );
        hits.pins.push((l.id.clone(), r));
    }

    // The building pins: the selection first, then the lit buildings from the front.
    let mut order: Vec<&BuildingPin> = scene.pins.iter().collect();
    let depth = |p: &BuildingPin| {
        let b = &city.buildings[p.building];
        v.cam.depth(b.x, b.y, 0.)
    };
    order.sort_by(|a, b| {
        (!a.selected)
            .cmp(&!b.selected)
            .then(depth(a).total_cmp(&depth(b)))
    });
    for bp in order {
        let b = &city.buildings[bp.building];
        let col = if scene.fault[bp.building] {
            RED
        } else {
            city.districts[b.district].color
        };
        let hgt = top(b.h, scene.hot[bp.building] || scene.fault[bp.building]);
        let anchor = v.at(b.x + BUILDING / 2., b.y + BUILDING / 2., hgt);
        let tw = f32::from(pen.shaped(&bp.text, small, CHAKRA, true, 0, 1.).width());
        let Some(p) = place(anchor, (tw + 20., box_h - 4.), &mut taken, &area) else {
            continue;
        };
        let r = p.rect;
        pen.polyline(&[anchor, p.knee], col, 1., 0.8, false);
        pen.dot(anchor, 2., col, 1.);
        let shape = chamfer(r, 5.);
        pen.polygon(&shape, BG, 0.9);
        pen.polygon(
            &[
                [r[0], r[1]],
                [r[0] + 3., r[1]],
                [r[0] + 3., r[1] + r[3] - 3.],
                [r[0], r[1] + r[3] - 3.],
            ],
            col,
            1.,
        );
        let edge = if bp.selected { 0xFFFFFF } else { col };
        pen.polyline(&shape, edge, 1., if bp.selected { 1. } else { 0.7 }, true);
        let tx = r[0] + 3. + (r[2] - 3.) / 2.;
        label(
            pen,
            &bp.text,
            small,
            CHAKRA,
            col,
            1.,
            (tx, r[1], r[3]),
            Align::Center,
        );
        hits.pins.push((bp.id.clone(), r));
    }
}

/// The film finish: glitch slices, a vignette and soft scanlines.
fn finish(pen: &mut Painter, scene: &Scene, (x0, y0, w, h): (f32, f32, f32, f32)) {
    let t = scene.t;
    if scene.glitch > 0. {
        // A few torn slices for a short moment after a change.
        let frame = (t * 30.) as u32;
        for k in 0..4u32 {
            let n = noise(frame, k);
            let y = y0 + (n % 1000) as f32 / 1000. * h;
            let sh = 2. + (n >> 10) as f32 % 7.;
            let col = if k % 2 == 0 { MAGENTA } else { CYAN };
            pen.rect((x0, y, w, sh), col, 0.16 * scene.glitch);
        }
    }
    // The vignette: soft dark bands on the four edges.
    let band = 140f32.min(h / 4.);
    let dark = |a: f32| ink(BG, a);
    let sides = [
        ([x0, y0, x0 + w, y0 + band], 0.),
        ([x0, y0 + h - band, x0 + w, y0 + h], 180.),
        ([x0, y0, x0 + band, y0 + h], 270.),
        ([x0 + w - band, y0, x0 + w, y0 + h], 90.),
    ];
    for (r, angle) in sides {
        pen.polygon_bg(
            &[[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]],
            linear_gradient(
                angle,
                linear_color_stop(dark(0.0), 0.),
                linear_color_stop(dark(0.7), 1.),
            ),
        );
    }
    // The scanlines.
    let mut lines = Vec::with_capacity((h / 4.) as usize + 1);
    let mut y = y0 + 1.5;
    while y < y0 + h {
        lines.push([x0, y, x0 + w, y]);
        y += 4.;
    }
    pen.strokes(&lines, 0x000000, 1., 0.11);
}
