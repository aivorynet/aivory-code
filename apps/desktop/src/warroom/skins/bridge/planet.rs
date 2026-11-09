//! The Bridge war room (board `docs/design/looks/bridge-planet/WarRoomPlanet.dc.html`, accepted
//! 2026-11-07): a planet with landing sites. Each codebase is a site, each session a dot, each
//! module of a codebase a pad of its site. Three levels: the planet, a site, an agent. A click
//! locks the brackets on the target, then the camera moves; all of it takes less than a second.
//!
//! Nothing here moves by chance: a dot hops to another pad only when its transcript shows a tool
//! call on another module. The numbered navigation belongs to the shell (`shell/bridge.rs`).
//!
//! More than 8 codebases: the first 7 keep their sites and site 8 holds the others, with its name
//! `+N`. More than 10 modules in a codebase: the first 10 keep their pads, the others share one by
//! a hash of the name, and the pad keeps its first name.

use std::collections::HashMap;
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};

use aivory_protocol::rpc::AutopilotPatch;
use aivory_protocol::{Line, LineKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, Context, Div, MouseButton, PathBuilder, Pixels, Point, RenderImage,
    SharedString, Stateful, Window, canvas, div, img, point, px, rgb,
};
use image::{ImageFormat, RgbaImage};

use crate::actions::Action;
use crate::bridge_kit::{Art, LABEL, short_name};
use crate::warroom::decisions::decisions;
use crate::warroom::{Overlay, WarRoomEvent, WarRoomScreen};

const OR: u32 = 0xFF9900;
const PE: u32 = 0x9C9CFF;
const RS: u32 = 0xCC6666;
const PK: u32 = 0xCC99CC;
const SA: u32 = 0xFF9966;
const PALE: u32 = 0xFFCC99;
/// The state red of the site outlines, the dots that wait and the lines above them.
const RED: u32 = 0xFF3B4E;
const GOLD: u32 = 0xFFC45C;
const BG: u32 = 0x000000;
const WHITE: u32 = 0xFFFFFF;

/// The left and top edge of the picture on the window.
const VP_X: f32 = 224.;
const VP_Y: f32 = 296.;
/// The size of the picture in the board.
const VP_W: f32 = 1216.;
const VP_H: f32 = 604.;
/// The size of the globe space and of a site crop.
const SPACE_W: f32 = 1244.;
const SPACE_H: f32 = 824.;
/// The disc of the planet in the globe space.
const DISC_R: f32 = 390.;
/// The zoom at the planet level.
const Z_GLOBE: f32 = 580. / 780.;
/// The zoom at a site: the crop fills the picture.
const Z_SITE: f32 = VP_W / (2. * 0.17 * DISC_R);
/// The zoom of a site crop at the agent level, against its fit.
const Z_AGENT: f32 = 2.25;

/// The brackets lock this long before the camera moves.
const LOCK_MS: f32 = 230.;
const FLY_MS: f32 = 620.;
const HOP_MS: f32 = 900.;
/// A repaint without a camera move: clouds, pings and the running line.
const TICK: Duration = Duration::from_millis(66);
/// One turn of the cloud strip.
const DRIFT_S: f32 = 260.;

/// The levels of the view.
const PLANET: u8 = 0;
const SITE: u8 = 1;
const AGENT: u8 = 2;

// ---- the pictures --------------------------------------------------------------------------

struct Globe {
    planet: Arc<RenderImage>,
    clouds: Arc<RenderImage>,
    /// The night side, the rim and the halo: black outside the disc, so the clouds end at it.
    shade: Arc<RenderImage>,
}

static GLOBE: OnceLock<Globe> = OnceLock::new();
static CROPS: [OnceLock<Arc<RenderImage>>; 8] = [const { OnceLock::new() }; 8];

fn picture(bytes: &[u8], format: ImageFormat) -> Arc<RenderImage> {
    let px = image::load_from_memory_with_format(bytes, format)
        .map(|i| i.to_rgba8())
        .unwrap_or_else(|_| RgbaImage::new(1, 1));
    crate::rts::to_render(&px)
}

/// The shade over the disc, 900 units wide around it.
fn shade() -> RgbaImage {
    let size = 900_u32;
    let mid = size as f32 / 2.;
    let far = 0.66_f32.hypot(0.72);
    RgbaImage::from_fn(size, size, |x, y| {
        let (u, v) = (x as f32 + 0.5 - mid, y as f32 + 0.5 - mid);
        let d = u.hypot(v);
        if d >= DISC_R {
            let glow = (1. - (d - DISC_R) / 34.).clamp(0., 1.).powi(2) * 0.5;
            let c = |v: f32| (v * glow) as u8;
            return image::Rgba([c(120.), c(140.), c(255.), 255]);
        }
        let (a, b) = ((u + DISC_R) / 780., (v + DISC_R) / 780.);
        let r = (a - 0.34).hypot(b - 0.28) / far;
        let dark = ((r - 0.40) / 0.46).clamp(0., 1.) * 0.72;
        image::Rgba([0, 0, 0, (dark * 255.) as u8])
    })
}

/// Decode the planet once, away from the UI thread.
fn load_globe() {
    static START: Once = Once::new();
    START.call_once(|| {
        std::thread::spawn(|| {
            let _ = GLOBE.set(Globe {
                planet: picture(
                    include_bytes!("../../../../assets/bridge/planet.jpg"),
                    ImageFormat::Jpeg,
                ),
                clouds: picture(
                    include_bytes!("../../../../assets/bridge/clouds.webp"),
                    ImageFormat::WebP,
                ),
                shade: crate::rts::to_render(&shade()),
            });
        });
    });
}

const CROP_BYTES: [&[u8]; 8] = [
    include_bytes!("../../../../assets/bridge/site0.jpg"),
    include_bytes!("../../../../assets/bridge/site1.jpg"),
    include_bytes!("../../../../assets/bridge/site2.jpg"),
    include_bytes!("../../../../assets/bridge/site3.jpg"),
    include_bytes!("../../../../assets/bridge/site4.jpg"),
    include_bytes!("../../../../assets/bridge/site5.jpg"),
    include_bytes!("../../../../assets/bridge/site6.jpg"),
    include_bytes!("../../../../assets/bridge/site7.jpg"),
];

/// Decode the crop of a site when somebody first goes there.
fn load_crop(site: usize) {
    static START: [Once; 8] = [const { Once::new() }; 8];
    START[site].call_once(move || {
        std::thread::spawn(move || {
            let _ = CROPS[site].set(picture(CROP_BYTES[site], ImageFormat::Jpeg));
        });
    });
}

// ---- the sites -----------------------------------------------------------------------------

struct Pad {
    /// In the crop of the site.
    x: f32,
    y: f32,
    /// On the globe, from the middle of the disc.
    gx: f32,
    gy: f32,
}

struct SiteDef {
    cx: f32,
    cy: f32,
    pads: Vec<Pad>,
    cut: Vec<(f32, f32)>,
}

/// The outline of an SVG path of `M` and `L` commands.
fn parse_cut(d: &str) -> Vec<(f32, f32)> {
    let nums: Vec<f32> = d
        .split(|c: char| c.is_ascii_alphabetic() || c.is_whitespace())
        .filter_map(|t| t.parse().ok())
        .collect();
    nums.chunks_exact(2).map(|p| (p[0], p[1])).collect()
}

fn defs() -> &'static [SiteDef] {
    static S: OnceLock<Vec<SiteDef>> = OnceLock::new();
    S.get_or_init(|| {
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../../../assets/bridge/sites.json"))
                .unwrap_or_default();
        let num = |v: &serde_json::Value, k: &str| v[k].as_f64().unwrap_or(0.) as f32;
        raw.as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|s| SiteDef {
                cx: num(s, "cx"),
                cy: num(s, "cy"),
                cut: parse_cut(s["cut"].as_str().unwrap_or("")),
                pads: s["pads"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|p| Pad {
                        x: num(p, "x"),
                        y: num(p, "y"),
                        gx: num(p, "gx"),
                        gy: num(p, "gy"),
                    })
                    .collect(),
            })
            .collect()
    })
}

// ---- the model -----------------------------------------------------------------------------

/// A small stable hash of a name.
fn hash(s: &str) -> usize {
    s.bytes()
        .fold(5381_usize, |h, b| h.wrapping_mul(33) ^ usize::from(b))
}

/// The module of a tool call: the folder of the file that it touches, or the file name for a file
/// directly below a source root. `None` for a call on no file.
fn module_of(text: &str) -> Option<String> {
    const ROOTS: [&str; 6] = ["src", "lib", "crates", "apps", "tests", "."];
    let token = text.split_whitespace().find_map(|t| {
        let t = t.trim_matches(|c: char| "\"'`,:;()[]".contains(c));
        let path = t.contains('/') || t.contains('\\');
        let file = t
            .rsplit_once('.')
            .is_some_and(|(a, e)| !a.is_empty() && (1..=5).contains(&e.len()))
            && t.chars().all(|c| c.is_alphanumeric() || "._-".contains(c));
        ((path && !t.contains("://") && !t.starts_with('-')) || file).then_some(t)
    })?;
    let parts: Vec<&str> = token
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && !p.ends_with(':'))
        .collect();
    let (name, dirs) = parts.split_last()?;
    if let Some(dir) = dirs.iter().rev().find(|d| !ROOTS.contains(d)) {
        return Some(dir.to_string());
    }
    let stem = name.split('.').next().unwrap_or(name);
    (!stem.is_empty()).then(|| stem.to_string())
}

/// The last tool call on a file in a transcript tail, newest first.
fn last_module(tail: &[Line]) -> Option<String> {
    tail.iter()
        .rev()
        .filter(|l| l.kind == LineKind::Tool)
        .find_map(|l| module_of(&l.text))
}

/// The site of a codebase in the list of sites: the last site takes the overflow.
fn site_index(sites: &[String], name: &str) -> usize {
    sites.iter().position(|s| s == name).unwrap_or(7)
}

/// The pad of a module in the list of pads of its codebase.
fn pad_index(mods: &[String], module: &str) -> usize {
    mods.iter()
        .position(|m| m == module)
        .unwrap_or_else(|| hash(module) % 10)
}

/// Where a dot stands, and from where it hops.
struct Spot {
    pad: usize,
    from: usize,
    at: Instant,
}

/// A camera: a space point `p` shows at `p * z + (x, y)`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cam {
    x: f32,
    y: f32,
    z: f32,
}

impl Cam {
    fn mix(self, to: Cam, t: f32) -> Cam {
        // The zoom moves by ratio, so that the way in has one even speed.
        let z = self.z * (to.z / self.z).powf(t);
        let k = if (to.z - self.z).abs() < 1e-3 {
            t
        } else {
            (z - self.z) / (to.z - self.z)
        };
        Cam {
            x: self.x + (to.x - self.x) * k,
            y: self.y + (to.y - self.y) * k,
            z,
        }
    }

    fn at(self, x: f32, y: f32) -> (f32, f32) {
        (x * self.z + self.x, y * self.z + self.y)
    }
}

/// A box in the picture.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Boxed {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Boxed {
    fn mix(self, to: Boxed, t: f32) -> Boxed {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Boxed {
            x: m(self.x, to.x),
            y: m(self.y, to.y),
            w: m(self.w, to.w),
            h: m(self.h, to.h),
        }
    }
}

/// The brackets on their way to a box.
#[derive(Clone, Copy)]
struct Lock {
    at: Instant,
    target: Boxed,
    level: u8,
    site: usize,
}

/// The state of the planet view between frames.
#[derive(Default)]
pub(in crate::warroom) struct Planet {
    pub level: u8,
    site: usize,
    pub agent: Option<String>,
    hover: Option<Boxed>,
    lock: Option<(Lock, Option<String>)>,
    /// The camera move in progress, from these cameras.
    fly: Option<(Instant, Cam, Cam)>,
    /// The last cameras on screen.
    shown: Option<(Cam, Cam)>,
    /// The brackets: where they were, when they left, and how they are now.
    br: Boxed,
    br_from: Boxed,
    br_at: Option<Instant>,
    /// The brackets open to the frame and fade after the lock.
    br_open: bool,
    /// The codebase of each site, in order of first sight.
    sites: Vec<String>,
    /// The modules of each codebase, in order of first sight.
    modules: HashMap<String, Vec<String>>,
    spots: HashMap<String, Spot>,
    t0: Option<Instant>,
    waiting: bool,
}

impl Planet {
    /// Bring the sites, the pads and the dots up to date with the daemon.
    fn update(&mut self, war: &WarRoom, now: Instant) {
        let mut names: Vec<&str> = war
            .sessions
            .iter()
            .filter(|p| p.state.is_live())
            .map(|p| p.codebase.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        for n in names {
            if !self.sites.iter().any(|s| s == n) && self.sites.len() < 8 {
                self.sites.push(n.to_string());
            }
        }
        for p in war.sessions.iter().filter(|p| p.state.is_live()) {
            let mods = self.modules.entry(p.codebase.clone()).or_default();
            for l in p.tail.iter().filter(|l| l.kind == LineKind::Tool) {
                if let Some(m) = module_of(&l.text)
                    && mods.len() < 10
                    && !mods.contains(&m)
                {
                    mods.push(m);
                }
            }
            let module = last_module(&p.tail);
            let pad = module.map(|m| pad_index(mods, &m));
            let spot = self.spots.entry(p.id.clone()).or_insert(Spot {
                pad: pad.unwrap_or(hash(&p.id) % 10),
                from: 0,
                at: now - Duration::from_secs(9),
            });
            if let Some(pad) = pad
                && pad != spot.pad
            {
                spot.from = spot.pad;
                spot.pad = pad;
                spot.at = now;
            }
        }
    }

    /// The live sessions of a site, the ones that wait first.
    fn at_site<'a>(&self, war: &'a WarRoom, site: usize) -> Vec<&'a Pane> {
        let mut out: Vec<&Pane> = war
            .sessions
            .iter()
            .filter(|p| p.state.is_live() && site_index(&self.sites, &p.codebase) == site)
            .collect();
        out.sort_by_key(|p| (!waits(p), p.since));
        out
    }

    fn pad_of(&self, id: &str, now: Instant) -> (usize, usize, f32) {
        self.spots.get(id).map_or((0, 0, 1.), |s| {
            let t = (now - s.at).as_secs_f32() * 1000. / HOP_MS;
            (s.from, s.pad, t.clamp(0., 1.))
        })
    }
}

fn waits(p: &Pane) -> bool {
    matches!(p.state, SessionState::NeedsYou | SessionState::Error)
}

/// `cubic-bezier(.76, 0, .2, 1)` of the board.
fn glide(x: f32) -> f32 {
    let x = x.clamp(0., 1.);
    let bez = |t: f32, a: f32, b: f32| {
        3. * (1. - t) * (1. - t) * t * a + 3. * (1. - t) * t * t * b + t * t * t
    };
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..14 {
        let mid = (lo + hi) / 2.;
        if bez(mid, 0.76, 0.2) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(lo, 0., 1.)
}

fn since_ms(at: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(at).as_secs_f32() * 1000.
}

// ---- drawing -------------------------------------------------------------------------------

/// Antonio at one of the four sizes.
fn text(size: f32, line: f32, color: u32, s: impl Into<SharedString>) -> Div {
    div()
        .font_family(LABEL)
        .text_size(px(size))
        .line_height(px(line))
        .text_color(rgb(color))
        .whitespace_nowrap()
        .child(s.into().to_uppercase())
}

fn spot(x: f32, y: f32, w: f32, h: f32) -> Div {
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
}

/// A pill of the frame with its label at the lower right.
fn pill(id: impl Into<SharedString>, x: f32, y: f32, c: u32, label: &str) -> Stateful<Div> {
    spot(x, y, 190., 62.)
        .id(id.into())
        .flex()
        .justify_end()
        .items_end()
        .pr(px(26.))
        .pb(px(8.))
        .rounded(px(31.))
        .bg(rgb(c))
        .cursor_pointer()
        .hover(|s| s.opacity(0.86))
        .child(text(18., 24., BG, label.to_string()))
}

fn arc(out: &mut Vec<(f32, f32)>, c: (f32, f32), r: (f32, f32), from: f32, to: f32) {
    for k in 0..=14 {
        let a = (from + (to - from) * k as f32 / 14.).to_radians();
        out.push((c.0 + r.0 * a.cos(), c.1 + r.1 * a.sin()));
    }
}

/// The two swept parts of the frame: thick column, thin bar, wide outer curve, tight inner curve.
fn frame_shapes(end: f32) -> Vec<(Vec<(f32, f32)>, u32)> {
    let mut a = vec![(0., 108.), (188., 108.)];
    arc(&mut a, (234., 198.), (46., 34.), 180., 90.);
    a.extend([(end, 232.), (end, 254.)]);
    arc(&mut a, (124., 186.), (124., 68.), 90., 180.);
    let mut b = vec![(end, 262.), (end, 284.)];
    arc(&mut b, (234., 318.), (46., 34.), 270., 180.);
    b.extend([(188., 378.), (0., 378.)]);
    arc(&mut b, (124., 330.), (124., 68.), 180., 270.);
    vec![(a, PE), (b, RS)]
}

/// One line or filled shape of the picture, in the space of its canvas.
struct Ink {
    points: Vec<(f32, f32)>,
    color: u32,
    alpha: f32,
    /// The width of the line; 0 fills the shape.
    width: f32,
}

fn hsla(c: u32, alpha: f32) -> gpui::Hsla {
    let mut h: gpui::Hsla = rgb(c).into();
    h.a = alpha;
    h
}

fn paint(inks: &[Ink], o: Point<Pixels>, window: &mut Window) {
    let at = |p: (f32, f32)| point(o.x + px(p.0), o.y + px(p.1));
    for k in inks {
        if k.points.len() < 2 {
            continue;
        }
        let mut path = if k.width > 0. {
            PathBuilder::stroke(px(k.width))
        } else {
            PathBuilder::fill()
        };
        path.move_to(at(k.points[0]));
        for p in &k.points[1..] {
            path.line_to(at(*p));
        }
        if k.width == 0. {
            path.close();
        }
        if let Ok(built) = path.build() {
            window.paint_path(built, hsla(k.color, k.alpha));
        }
    }
}

fn layer(inks: Vec<Ink>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| paint(&inks, bounds.origin, window),
    )
    .absolute()
    .size_full()
}

/// A bracket `[` or `]` of a width and a half height around a point.
fn bracket(
    c: (f32, f32),
    near: f32,
    far: f32,
    half: f32,
    radius: f32,
    left: bool,
) -> Vec<(f32, f32)> {
    let s = if left { -1. } else { 1. };
    let mut p = vec![
        (c.0 + s * near, c.1 - half),
        (c.0 + s * (far - radius), c.1 - half),
    ];
    let (a, b, d, e) = if left {
        (270., 180., 180., 90.)
    } else {
        (270., 360., 0., 90.)
    };
    arc(
        &mut p,
        (c.0 + s * (far - radius), c.1 - half + radius),
        (radius, radius),
        a,
        b,
    );
    arc(
        &mut p,
        (c.0 + s * (far - radius), c.1 + half - radius),
        (radius, radius),
        d,
        e,
    );
    p.push((c.0 + s * near, c.1 + half));
    p
}

/// A point at a share of a line of points.
fn along(p: &[(f32, f32)], t: f32) -> (f32, f32) {
    let parts: Vec<f32> = p
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .collect();
    let mut left = parts.iter().sum::<f32>() * t;
    for (k, len) in parts.iter().enumerate() {
        if left <= *len {
            let f = left / len.max(0.01);
            return (
                p[k].0 + (p[k + 1].0 - p[k].0) * f,
                p[k].1 + (p[k + 1].1 - p[k].1) * f,
            );
        }
        left -= len;
    }
    p[p.len() - 1]
}

/// Text color, dot fill and dot ring of a state counter: the same as its dots on the map
/// ([`dot_style`]).
fn legend_style(label: &str) -> Option<(u32, u32, u32)> {
    let state = match label {
        "At work" => SessionState::Working,
        "Idle" => SessionState::Idle,
        "Waits" => SessionState::NeedsYou,
        _ => return None,
    };
    let (fill, ring) = state_dot(state);
    // A white or red dot gives its fill to the text; a hollow idle dot gives its ring.
    let color = if fill == BG { ring } else { fill };
    Some((color, fill, ring))
}

fn state_dot(state: SessionState) -> (u32, u32) {
    match state {
        SessionState::NeedsYou | SessionState::Error => (RED, BG),
        SessionState::Working => (WHITE, BG),
        _ => (BG, PE),
    }
}

fn dot_style(p: &Pane) -> (u32, u32, bool) {
    let (fill, ring) = state_dot(p.state);
    (fill, ring, p.state == SessionState::Working)
}

/// A dot with its ping, by its middle on the picture.
fn dot(x: f32, y: f32, r: f32, p: &Pane, clock: f32, delay: f32) -> Div {
    let (fill, line, ping) = dot_style(p);
    let mut out = spot(x - r, y - r, r * 2., r * 2.)
        .rounded_full()
        .bg(rgb(fill))
        .border_2()
        .border_color(rgb(line));
    if ping {
        let f = ((clock + delay) / 2.6).fract();
        let grow = (f / 0.7).min(1.);
        let size = r * 2. * (0.6 + 2.2 * grow);
        out = out.child(
            div()
                .absolute()
                .left(px(r - 2. - size / 2.))
                .top(px(r - 2. - size / 2.))
                .size(px(size))
                .rounded_full()
                .border_1()
                .border_color(hsla(WHITE, 0.9 * (1. - grow))),
        );
    }
    out
}

impl WarRoomScreen {
    /// The session on the agent level, or the pick of the sidebar.
    pub(crate) fn bridge_selected(&self) -> Option<String> {
        let w = self.war.as_ref()?;
        self.bridge
            .planet
            .agent
            .clone()
            .or_else(|| decisions(w).first().map(|p| p.id.clone()))
    }

    /// The level of the view: the shell colors its top segment by it.
    pub(crate) fn bridge_level(&self) -> u8 {
        self.bridge.planet.level
    }

    /// Lock the brackets on a box; the camera moves after them.
    fn planet_fly(&mut self, level: u8, site: usize, agent: Option<String>, target: Boxed) {
        let p = &mut self.bridge.planet;
        let now = Instant::now();
        p.hover = None;
        p.br_from = p.br;
        p.br_at = Some(now);
        p.br_open = false;
        p.lock = Some((
            Lock {
                at: now,
                target,
                level,
                site,
            },
            agent,
        ));
        load_crop(site);
    }

    /// Go to a level with no lock: up, or back from a site.
    fn planet_to(&mut self, level: u8, cx: &mut Context<Self>) {
        let p = &mut self.bridge.planet;
        p.level = level;
        p.lock = None;
        p.hover = None;
        if level < AGENT {
            p.agent = None;
        }
        if let Some((g, s)) = p.shown {
            p.fly = Some((Instant::now(), g, s));
        }
        cx.notify();
    }

    fn planet_answer(&mut self, id: &str, k: usize, cx: &mut Context<Self>) {
        let Some(war) = &self.war else { return };
        let Some(p) = war.session(id) else { return };
        let Some(ask) = &p.ask else { return };
        let Some(option) = ask.options.get(k).cloned() else {
            return;
        };
        let action = Action::Answer {
            id: id.to_string(),
            ask: ask.id.clone(),
            option: Some(option),
            text: None,
        };
        self.act(action, cx);
        self.planet_to(SITE, cx);
    }

    /// The digits answer on the agent level; Backspace goes up one level.
    pub(in crate::warroom) fn bridge_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let level = self.bridge.planet.level;
        if key == "backspace" && level > PLANET {
            self.planet_to(level - 1, cx);
            return;
        }
        if level == AGENT
            && let Some(id) = self.bridge.planet.agent.clone()
            && let Ok(n @ 1..=9) = key.parse::<usize>()
        {
            self.planet_answer(&id, n - 1, cx);
        }
    }

    pub(in crate::warroom) fn render_bridge(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        load_globe();
        let now = Instant::now();
        let art = Art::of(window);
        let (vw, vh) = (VP_W + art.dx, VP_H + art.dy);
        let p = &mut self.bridge.planet;
        let t0 = *p.t0.get_or_insert(now);
        let clock = (now - t0).as_secs_f32();
        p.update(war, now);

        // The lock ends: the level changes, and the camera leaves.
        if p.lock
            .as_ref()
            .is_some_and(|(l, _)| since_ms(l.at, now) >= LOCK_MS)
            && let Some((lock, agent)) = p.lock.take()
        {
            p.level = lock.level;
            p.site = lock.site;
            p.agent = agent;
            p.lock = None;
            if let Some((g, s)) = p.shown {
                p.fly = Some((now, g, s));
            }
            p.br_from = lock.target;
            p.br_at = Some(now);
            p.br_open = true;
        }
        if p.level == AGENT
            && !p
                .agent
                .as_ref()
                .is_some_and(|id| war.session(id).is_some_and(|s| s.state.is_live()))
        {
            p.level = SITE;
            p.agent = None;
        }
        let sites = defs();
        let fit = (vw / VP_W).min(vh / VP_H);
        let zg = Z_GLOBE * fit;
        let home = Cam {
            x: vw / 2. - zg * SPACE_W / 2.,
            y: vh / 2. - zg * SPACE_H / 2.,
            z: zg,
        };
        let def = sites.get(p.site);
        let globe_to = match (p.level, def) {
            (PLANET, _) | (_, None) => home,
            (_, Some(d)) => {
                let z = Z_SITE * fit;
                Cam {
                    x: vw / 2. - z * (SPACE_W / 2. + d.cx),
                    y: vh / 2. - z * (SPACE_H / 2. + d.cy),
                    z,
                }
            }
        };
        let zs = vw / SPACE_W;
        let mut surf_to = Cam {
            x: 0.,
            y: (vh - SPACE_H * zs) / 2.,
            z: zs,
        };
        let cur = p.agent.as_ref().and_then(|id| war.session(id));
        if let (AGENT, Some(a), Some(d)) = (p.level, cur, def) {
            let pad = &d.pads[p.pad_of(&a.id, now).1];
            let z = Z_AGENT * zs;
            surf_to = Cam {
                // The crop always fills the picture: it has no ground beyond its edge.
                x: (300. * fit - z * pad.x).min(0.),
                y: (vh / 2. - z * pad.y).clamp(vh - SPACE_H * z, 0.),
                z,
            };
        }
        let (mut globe, mut surf) = (globe_to, surf_to);
        let mut flown = FLY_MS * 2.;
        if let Some((at, g, s)) = p.fly {
            flown = since_ms(at, now);
            if flown >= FLY_MS {
                p.fly = None;
            } else {
                let t = glide(flown / FLY_MS);
                globe = g.mix(globe_to, t);
                surf = s.mix(surf_to, t);
            }
        }
        p.shown = Some((globe, surf));
        let level = p.level;
        let open = p.site;
        let moving = p.fly.is_some() || p.lock.is_some() || p.br_at.is_some();

        // The brackets.
        let full = Boxed {
            x: 8.,
            y: 8.,
            w: vw - 16.,
            h: vh - 16.,
        };
        let (br_to, br_ms) = if p.br_open {
            (full, FLY_MS)
        } else if let Some((l, _)) = &p.lock {
            (l.target, 220.)
        } else {
            (p.hover.unwrap_or(p.br), 220.)
        };
        let mut br_alpha = if p.br_open {
            0.
        } else if p.lock.is_some() || p.hover.is_some() {
            1.
        } else {
            0.
        };
        match p.br_at {
            Some(at) => {
                let f = since_ms(at, now) / br_ms;
                p.br = p.br_from.mix(br_to, glide(f));
                if p.br_open {
                    br_alpha = (1. - f).clamp(0., 1.);
                }
                if f >= 1. {
                    p.br_at = None;
                    p.br_open = false;
                }
            }
            None => p.br = br_to,
        }
        let brk = p.br;

        // A repaint at frame rate while the camera moves, else a calm tick.
        if moving {
            window.request_animation_frame();
        }
        if !p.waiting {
            p.waiting = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(TICK).await;
                let _ = this.update(cx, |t, cx| {
                    t.bridge.planet.waiting = false;
                    cx.notify();
                });
            })
            .detach();
        }

        let queue = decisions(war);
        let live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
        let fade_late = ((flown - 500.) / 180.).clamp(0., 1.);

        let mut view = div()
            .absolute()
            .left(px(VP_X))
            .top(px(VP_Y))
            .w(px(vw))
            .h(px(vh))
            .overflow_hidden()
            .rounded_tl(px(34.))
            .bg(rgb(BG));

        // ---- the globe ----
        match GLOBE.get() {
            Some(g) => {
                let (dx, dy) = globe.at(SPACE_W / 2. - DISC_R, SPACE_H / 2. - DISC_R);
                let side = DISC_R * 2. * globe.z;
                view = view.child(
                    img(g.planet.clone())
                        .absolute()
                        .left(px(dx))
                        .top(px(dy))
                        .size(px(side)),
                );
                if level == PLANET {
                    let strip = side * 2.;
                    let off = (clock / DRIFT_S).fract() * strip;
                    let alpha = 0.6 * (flown / 200.).clamp(0., 1.);
                    let sheet = |x: f32| {
                        img(g.clouds.clone())
                            .absolute()
                            .left(px(x))
                            .top_0()
                            .w(px(strip))
                            .h(px(side))
                    };
                    view = view.child(
                        spot(dx, dy, side, side)
                            .overflow_hidden()
                            .opacity(alpha)
                            .child(sheet(-off))
                            .child(sheet(strip - off)),
                    );
                }
                let wide = 900. * globe.z;
                view = view.child(
                    img(g.shade.clone())
                        .absolute()
                        .left(px(dx - 60. * globe.z))
                        .top(px(dy - 60. * globe.z))
                        .size(px(wide)),
                );
            }
            None => {
                view = view.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(text(18., 24., OR, "Scanning the planet")),
                );
            }
        }

        // ---- the marks on the globe ----
        let mid = |x: f32, y: f32| globe.at(SPACE_W / 2. + x, SPACE_H / 2. + y);
        let mut inks: Vec<Ink> = Vec::new();
        let mut dots: Vec<AnyElement> = Vec::new();
        let mut labels: Vec<AnyElement> = Vec::new();
        let ladder = [30., 170., 310., 450.];
        let site_box = |d: &SiteDef| {
            let (x, y) = mid(d.cx, d.cy);
            Boxed {
                x: x - 66.,
                y: y - 66.,
                w: 132.,
                h: 132.,
            }
        };
        let names = self.bridge.planet.sites.clone();
        for (g, d) in sites.iter().enumerate() {
            let Some(name) = names.get(g) else { continue };
            let agents = self.bridge.planet.at_site(war, g);
            let waiting = agents.iter().filter(|a| waits(a)).count();
            let working = agents
                .iter()
                .filter(|a| a.state == SessionState::Working)
                .count();
            let color = if waiting > 0 { RED } else { GOLD };
            inks.push(Ink {
                points: d
                    .cut
                    .iter()
                    .chain(d.cut.first())
                    .map(|c| mid(c.0, c.1))
                    .collect(),
                color,
                alpha: 1.,
                width: 1.5,
            });
            for (k, a) in agents.iter().enumerate() {
                let (from, to, t) = self.bridge.planet.pad_of(&a.id, now);
                let (pa, pb) = (&d.pads[from], &d.pads[to]);
                let t = glide(t);
                let (x, y) = mid(pa.gx + (pb.gx - pa.gx) * t, pa.gy + (pb.gy - pa.gy) * t);
                let r = if waits(a) { 9. } else { 8. } * globe.z.max(0.6);
                let shift = (k % 3) as f32 * 3. * globe.z;
                dots.push(
                    dot(x + shift, y - shift, r, a, clock, k as f32 * 0.41).into_any_element(),
                );
            }
            if level != PLANET {
                continue;
            }
            let (ex, ey) = mid(d.cx, d.cy);
            if waiting > 0 {
                let beat = 1. + 0.05 * (1. - ((clock / 2.4).fract() * 2. - 1.).abs());
                for left in [true, false] {
                    inks.push(Ink {
                        points: bracket(
                            (ex, ey),
                            46. * globe.z * beat,
                            74. * globe.z * beat,
                            74. * globe.z * beat,
                            10. * globe.z,
                            left,
                        ),
                        color: OR,
                        alpha: 1.,
                        width: 4.,
                    });
                }
            }
            let left = g < 4;
            let y = ladder[g % 4] * fit;
            let (lx, knee) = if left {
                (150., 186.)
            } else {
                (vw - 150., vw - 186.)
            };
            let line = vec![(lx, y + 24.), (knee, y + 24.), (ex, ey)];
            if waiting > 0 {
                let f = (clock / 3.).fract();
                let (a, b) = (along(&line, f), along(&line, (f + 0.04).min(1.)));
                inks.push(Ink {
                    points: vec![a, b],
                    color: WHITE,
                    alpha: 1.,
                    width: 3.,
                });
            }
            inks.push(Ink {
                points: line,
                color,
                alpha: 1.,
                width: 1.5,
            });
            let more = names.len() == 8 && g == 7;
            let extra = if more {
                let mut other: Vec<&str> = live
                    .iter()
                    .map(|s| s.codebase.as_str())
                    .filter(|c| !names.iter().any(|n| n == c))
                    .collect();
                other.sort_unstable();
                other.dedup();
                other.len()
            } else {
                0
            };
            let title = if extra > 0 {
                format!("{name} +{extra} {}", agents.len())
            } else {
                format!("{name} {}", agents.len())
            };
            let sub = if waiting > 0 {
                format!("{waiting} wait · {working} at work")
            } else {
                format!("{working} at work")
            };
            let tone = if waiting > 0 { RED } else { OR };
            let target = site_box(d);
            let word = |s: Div| if left { s } else { s.flex().justify_end() };
            labels.push(
                spot(if left { 16. } else { vw - 192. }, y, 176., 44.)
                    .id(("pl-lab", g))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |t, on: &bool, _, cx| {
                        t.planet_hover(on.then_some(target), cx);
                    }))
                    .on_click(cx.listener(move |t, _, _, cx| {
                        t.planet_fly(SITE, g, None, target);
                        cx.notify();
                    }))
                    .child(word(text(18., 24., tone, title)))
                    .child(word(text(14., 18., tone, sub)))
                    .into_any_element(),
            );
            labels.push(
                spot(target.x, target.y, target.w, target.h)
                    .id(("pl-site", g))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |t, on: &bool, _, cx| {
                        t.planet_hover(on.then_some(target), cx);
                    }))
                    .on_click(cx.listener(move |t, _, _, cx| {
                        t.planet_fly(SITE, g, None, target);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        view = view.child(layer(inks)).children(dots);

        // The gold crosshair on the site of the oldest decision.
        if let (PLANET, Some(first)) = (level, queue.first()) {
            let g = site_index(&names, &first.codebase);
            if let Some(d) = sites.get(g) {
                let (x, y) = mid(d.cx, d.cy);
                view = view
                    .child(spot(0., y, vw, 1.).bg(hsla(OR, 0.75)))
                    .child(spot(x, 0., 1., vh).bg(hsla(OR, 0.75)));
            }
        }
        view = view.children(labels);

        // ---- the surface of a site ----
        if level >= SITE
            && let Some(d) = def
        {
            let name = names.get(open).cloned().unwrap_or_default();
            let mods = self
                .bridge
                .planet
                .modules
                .get(&name)
                .cloned()
                .unwrap_or_default();
            let shown = CROPS[open].get().cloned();
            let alpha = if shown.is_some() {
                ((flown - 400.) / 220.).clamp(0., 1.)
            } else {
                0.
            };
            let mut ground = div().absolute().size_full().opacity(alpha);
            if let Some(c) = shown {
                ground = ground.child(
                    img(c)
                        .absolute()
                        .left(px(surf.x))
                        .top(px(surf.y))
                        .w(px(SPACE_W * surf.z))
                        .h(px(SPACE_H * surf.z)),
                );
            }
            let at = |x: f32, y: f32| surf.at(x, y);
            let mut marks: Vec<Ink> = Vec::new();
            for k in 1..15 {
                let (x, _) = at(k as f32 * 88., 0.);
                marks.push(Ink {
                    points: vec![(x, 0.), (x, vh)],
                    color: PE,
                    alpha: 0.24,
                    width: 1.,
                });
            }
            for k in 1..10 {
                let (_, y) = at(0., k as f32 * 88.);
                marks.push(Ink {
                    points: vec![(0., y), (vw, y)],
                    color: PE,
                    alpha: 0.24,
                    width: 1.,
                });
            }
            let route: Vec<(f32, f32)> = d.pads.iter().map(|q| at(q.x, q.y)).collect();
            marks.push(Ink {
                points: route.clone(),
                color: BG,
                alpha: 0.6,
                width: 7.,
            });
            marks.push(Ink {
                points: route.clone(),
                color: GOLD,
                alpha: 1.,
                width: 3.,
            });
            ground = ground.child(layer(marks));
            for (k, q) in route.iter().enumerate() {
                let r = 9. * (surf.z / zs).sqrt();
                ground = ground.child(
                    spot(q.0 - r, q.1 - r, r * 2., r * 2.)
                        .rounded_full()
                        .bg(rgb(BG))
                        .border_2()
                        .border_color(rgb(GOLD)),
                );
                if let (SITE, true, Some(module)) = (level, fade_late > 0., mods.get(k)) {
                    ground = ground.child(
                        spot(q.0 - 24., q.1 + 14., 110., 18.)
                            .opacity(fade_late)
                            .child(div().absolute().px(px(5.)).bg(hsla(BG, 0.72)).child(text(
                                14.,
                                18.,
                                0xCDBDF0,
                                module.clone(),
                            ))),
                    );
                }
            }
            let agents = self.bridge.planet.at_site(war, open);
            let picked = self.bridge.planet.agent.clone();
            for (k, a) in agents.iter().enumerate() {
                if level == AGENT && picked.as_deref() != Some(a.id.as_str()) {
                    continue;
                }
                let (from, to, t) = self.bridge.planet.pad_of(&a.id, now);
                let (pa, pb) = (&d.pads[from], &d.pads[to]);
                let t = glide(t);
                let (x, y) = at(pa.x + (pb.x - pa.x) * t, pa.y + (pb.y - pa.y) * t);
                let shift = (k % 3) as f32 * 8.;
                let (x, y) = (x + shift, y - shift);
                let wait = waits(a);
                let r = if a.state == SessionState::Idle {
                    9.
                } else {
                    12.
                };
                ground = ground.child(dot(x, y, r, a, clock, k as f32 * 0.41));
                if wait {
                    let beat = 1. + 0.05 * (1. - ((clock / 2.4).fract() * 2. - 1.).abs());
                    let mut lines = Vec::new();
                    for left in [true, false] {
                        lines.push(Ink {
                            points: bracket((x, y), 12. * beat, 34. * beat, 30. * beat, 8., left),
                            color: OR,
                            alpha: 1.,
                            width: 5.,
                        });
                    }
                    ground = ground.child(layer(lines));
                }
                if level == SITE && fade_late > 0. {
                    let tag = (x - 105.).clamp(4., vw - 216.);
                    let tag_y = (y - 92.).max(4.);
                    let color = if wait {
                        RED
                    } else if a.state == SessionState::Idle {
                        PE
                    } else {
                        OR
                    };
                    let doing = match a.state {
                        SessionState::NeedsYou | SessionState::Error => {
                            format!("{} · {} min", kind_word(a), a.minutes(war.now))
                        }
                        SessionState::Idle => format!("Idle {} min", a.minutes(war.now)),
                        _ => last_step(a)
                            .map_or_else(|| "At work".to_string(), |(v, t)| format!("{v} {t}")),
                    };
                    let id = a.id.clone();
                    let target = Boxed {
                        x: x - 50.,
                        y: y - 50.,
                        w: 100.,
                        h: 100.,
                    };
                    let name = if a.title.is_empty() { &a.id } else { &a.title };
                    // A click on the dot on the map flies to the agent too.
                    let dot_id = a.id.clone();
                    ground = ground.child(
                        spot(x - 20., y - 20., 40., 40.)
                            .id(("pl-dot", k))
                            .cursor_pointer()
                            .on_click(cx.listener(move |t, _, _, cx| {
                                t.planet_fly(AGENT, open, Some(dot_id.clone()), target);
                                cx.notify();
                            })),
                    );
                    ground = ground.child(
                        spot(tag, tag_y, 210., 44.)
                            .id(("pl-tag", k))
                            .opacity(fade_late)
                            .px(px(8.))
                            .py(px(2.))
                            .bg(rgb(BG))
                            .overflow_hidden()
                            .cursor_pointer()
                            .on_click(cx.listener(move |t, _, _, cx| {
                                t.planet_fly(AGENT, open, Some(id.clone()), target);
                                cx.notify();
                            }))
                            .child(text(18., 22., color, short_name(name, 26)))
                            .child(text(14., 18., PE, doing)),
                    );
                }
            }
            view = view.child(ground);
        }

        // ---- the panel of an agent ----
        if let (AGENT, Some(a)) = (level, cur)
            && fade_late > 0.
        {
            view = view.child(self.planet_panel(a, war, vw, vh, fade_late, cx));
        }

        // ---- the brackets ----
        if br_alpha > 0. {
            let side = |left: bool| {
                let d = div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(px(44.))
                    .border_color(rgb(OR))
                    .border_t(px(10.))
                    .border_b(px(10.));
                if left {
                    d.left_0().border_l(px(10.)).rounded_l(px(26.))
                } else {
                    d.right_0().border_r(px(10.)).rounded_r(px(26.))
                }
            };
            view = view.child(
                spot(brk.x, brk.y, brk.w, brk.h)
                    .opacity(br_alpha)
                    .child(side(true))
                    .child(side(false)),
            );
        }

        let mut out = div()
            .absolute()
            .size_full()
            .overflow_hidden()
            .bg(rgb(BG))
            .font_family(LABEL)
            .child(self.planet_header(war, art, cx))
            .child(view);
        if self.open == Overlay::NewSession {
            let form = self.new_session_popover(window, cx);
            out = out.child(self.overlay(
                div().absolute().left(px(214.)).top(px(300.)).child(form),
                cx,
            ));
        }
        out.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
        )
        .into_any_element()
    }

    fn planet_hover(&mut self, target: Option<Boxed>, cx: &mut Context<Self>) {
        let p = &mut self.bridge.planet;
        if p.lock.is_none() && p.level == PLANET {
            p.br_from = if p.hover.is_none() {
                target.unwrap_or(p.br)
            } else {
                p.br
            };
            p.hover = target;
            p.br_at = Some(Instant::now());
            cx.notify();
        }
    }

    /// The header: the frame, the status line, the data block, the pills and the path.
    fn planet_header(&mut self, war: &WarRoom, art: Art, cx: &mut Context<Self>) -> Div {
        let (level, id) = (self.bridge.planet.level, self.bridge.planet.agent.clone());
        let name = self
            .bridge
            .planet
            .sites
            .get(self.bridge.planet.site)
            .cloned()
            .unwrap_or_default();
        let cur = id.as_ref().and_then(|i| war.session(i));
        let scope: Vec<&Pane> = war
            .sessions
            .iter()
            .filter(|p| p.state.is_live() && (level == PLANET || p.codebase == name))
            .collect();
        let count = |s: SessionState| scope.iter().filter(|p| p.state == s).count();
        let (work, idle) = (count(SessionState::Working), count(SessionState::Idle));
        let wait = scope.iter().filter(|p| waits(p)).count();
        let (head, red) = match (level, cur) {
            (AGENT, Some(a)) if waits(a) => (
                format!("{} · waits {} min", kind_word(a), a.minutes(war.now)),
                true,
            ),
            (AGENT, Some(a)) if a.state == SessionState::Idle => {
                (format!("Idle · {} min", a.minutes(war.now)), false)
            }
            (AGENT, Some(a)) => (format!("At work · {} min", a.minutes(war.now)), false),
            _ if wait > 0 => (
                format!("{wait} wait for you · {work} at work · {idle} idle"),
                true,
            ),
            _ if level == PLANET => (format!("All clear · {work} at work · {idle} idle"), false),
            _ => (format!("{work} at work · {idle} idle"), false),
        };
        let title = match (level, cur) {
            (AGENT, Some(a)) => short_name(if a.title.is_empty() { &a.id } else { &a.title }, 28),
            (SITE, _) => format!("Site {name}"),
            _ => format!("War room {}", scope.len()),
        };

        // The data block: real values only.
        let cost: f64 = scope.iter().map(|p| p.cost).sum();
        let oldest = scope
            .iter()
            .filter(|p| waits(p))
            .map(|p| p.minutes(war.now))
            .max();
        let mut seen: Vec<&str> = Vec::new();
        let usage: Vec<(String, String)> = war
            .usage
            .iter()
            .filter(|u| {
                let new = !seen.contains(&u.provider.as_str());
                seen.push(&u.provider);
                new
            })
            .take(3)
            .map(|u| (u.provider.clone(), format!("{} %", u.pct)))
            .collect();
        let use_at = |k: usize| usage.get(k).cloned().unwrap_or_default();
        let rows: Vec<[(String, String); 3]> = match (level, cur) {
            (AGENT, Some(a)) => vec![
                [
                    ("Agent".into(), a.provider.clone()),
                    ("Cost".into(), format!("${:.2}", a.cost)),
                    ("Context".into(), format!("{} %", a.context_pct)),
                ],
                [
                    ("Model".into(), a.model.clone()),
                    ("Tokens".into(), crate::command_kit::tokens(a.tokens)),
                    Default::default(),
                ],
            ],
            _ => vec![
                [
                    ("At work".into(), work.to_string()),
                    ("Spend".into(), format!("${cost:.2}")),
                    use_at(0),
                ],
                [
                    ("Idle".into(), idle.to_string()),
                    (
                        "Oldest wait".into(),
                        oldest.map_or("·".to_string(), |m| format!("{m} min")),
                    ),
                    use_at(1),
                ],
                [
                    ("Waits".into(), wait.to_string()),
                    ("Sites".into(), self.bridge.planet.sites.len().to_string()),
                    use_at(2),
                ],
            ],
        };
        let mut block = div()
            .absolute()
            .left(px(248.))
            .top(px(64.))
            .flex()
            .flex_col();
        for row in rows {
            let mut line = div().flex().h(px(26.));
            for (k, (a, b)) in row.into_iter().enumerate() {
                let (wa, wb) = [(104., 124.), (124., 136.), (92., 84.)][k];
                // The state counters wear the colors of their dots on the map.
                let legend = if k == 0 && level != AGENT {
                    legend_style(&a)
                } else {
                    None
                };
                let label = match legend {
                    Some((color, fill, ring)) => div()
                        .w(px(wa))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .size(px(12.))
                                .flex_none()
                                .rounded_full()
                                .bg(rgb(fill))
                                .border_2()
                                .border_color(rgb(ring)),
                        )
                        .child(text(18., 24., color, a)),
                    None => div().w(px(wa)).child(text(18., 24., OR, a)),
                };
                let value = legend.map_or(PE, |(color, _, _)| color);
                line = line
                    .child(label)
                    .child(div().w(px(wb)).child(text(18., 24., value, b)));
            }
            block = block.child(line);
        }

        // The frame.
        let end = 700. + art.dx;
        let shapes = frame_shapes(end)
            .into_iter()
            .map(|(points, color)| Ink {
                points,
                color,
                alpha: 1.,
                width: 0.,
            })
            .collect();

        // The pills of the level.
        let dx = art.dx;
        let on = war.autopilot.on;
        let mut pills: Vec<Stateful<Div>> = Vec::new();
        match level {
            PLANET => {
                pills.push(
                    pill("pl-p1", 1026. + dx, 84., PE, "New session")
                        .on_click(cx.listener(|t, _, _, cx| t.toggle(Overlay::NewSession, cx))),
                );
                pills.push(
                    pill(
                        "pl-p2",
                        1226. + dx,
                        84.,
                        PK,
                        if on { "Autopilot on" } else { "Autopilot off" },
                    )
                    .on_click(cx.listener(move |t, _, _, cx| {
                        t.act(
                            Action::Autopilot(AutopilotPatch {
                                on: Some(!on),
                                ..AutopilotPatch::default()
                            }),
                            cx,
                        );
                    })),
                );
                pills.push(
                    pill("pl-p4", 1226. + dx, 156., RS, "Pause all").on_click(cx.listener(
                        |t, _, _, cx| {
                            let ids: Vec<String> = t
                                .war
                                .iter()
                                .flat_map(|w| &w.sessions)
                                .filter(|p| p.controlled && p.state == SessionState::Working)
                                .map(|p| p.id.clone())
                                .collect();
                            for id in ids {
                                t.act(Action::Pause(id), cx);
                            }
                        },
                    )),
                );
            }
            SITE => {
                pills.push(
                    pill("pl-p1", 1026. + dx, 84., PE, "New session")
                        .on_click(cx.listener(|t, _, _, cx| t.toggle(Overlay::NewSession, cx))),
                );
                pills.push(
                    pill("pl-p4", 1226. + dx, 156., RS, "Back")
                        .on_click(cx.listener(|t, _, _, cx| t.planet_to(PLANET, cx))),
                );
            }
            _ => {
                if let Some(a) = cur {
                    // Focus is the one way into the session: "Open session" did the same.
                    let (focus, stop) = (a.id.clone(), a.id.clone());
                    pills.push(
                        pill("pl-p1", 1026. + dx, 84., SA, "Focus").on_click(cx.listener(
                            move |_, _, _, cx| {
                                cx.emit(WarRoomEvent::Focus(focus.clone()));
                            },
                        )),
                    );
                    if a.controlled {
                        pills.push(pill("pl-p3", 1026. + dx, 156., PK, "Stop").on_click(
                            cx.listener(move |t, _, _, cx| t.act(Action::Stop(stop.clone()), cx)),
                        ));
                    }
                }
                pills.push(
                    pill("pl-p4", 1226. + dx, 156., RS, "Back")
                        .on_click(cx.listener(|t, _, _, cx| t.planet_to(SITE, cx))),
                );
            }
        }

        // The path in the bar slots; the half bar marks the level.
        let agent_name = cur
            .map(|a| short_name(if a.title.is_empty() { &a.id } else { &a.title }, 30))
            .unwrap_or_default();
        let slots = [
            ("Fleet".to_string(), 740. + dx, 190.),
            (
                if level == PLANET { String::new() } else { name },
                934. + dx,
                250.,
            ),
            (agent_name, 1188. + dx, 186.),
        ];
        let mut path = div().absolute().size_full();
        for (k, (label, x, w)) in slots.into_iter().enumerate() {
            let here = k as u8 == level;
            path = path
                .child(
                    spot(x, 232., w, 22.)
                        .id(("pl-crumb", k))
                        .flex()
                        .items_center()
                        .justify_end()
                        .pr(px(10.))
                        .bg(rgb(PK))
                        .cursor_pointer()
                        .on_click(cx.listener(move |t, _, _, cx| {
                            if (k as u8) < t.bridge.planet.level {
                                t.planet_to(k as u8, cx);
                            }
                        }))
                        .child(text(14., 18., BG, label)),
                )
                .child(
                    spot(x, 262., w, if here { 11. } else { 22. }).bg(rgb(if here {
                        SA
                    } else {
                        PK
                    })),
                );
        }

        div()
            .absolute()
            .size_full()
            .child(layer(shapes))
            .child(spot(706. + dx, 232., 28., 22.).bg(rgb(OR)))
            .child(spot(706. + dx, 262., 28., 22.).bg(rgb(OR)))
            .child(spot(1379. + dx, 262., 61., 22.).bg(rgb(RS)))
            .child(div().absolute().left(px(248.)).top(px(16.)).child(text(
                28.,
                36.,
                if red { RED } else { OR },
                head,
            )))
            .child(block)
            .child(
                div()
                    .absolute()
                    .right(px(24.))
                    .top_0()
                    .child(text(64., 76., OR, title).font_weight(gpui::FontWeight::BOLD)),
            )
            .children(pills)
            .child(path)
    }

    /// The panel of the agent level: what it asks or does, the answers, its last steps.
    fn planet_panel(
        &mut self,
        a: &Pane,
        war: &WarRoom,
        vw: f32,
        vh: f32,
        alpha: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let (x, ask) = (vw - 616., waits(a).then_some(a.ask.as_ref()).flatten());
        let (main, side) = if ask.is_some() { (RS, SA) } else { (OR, PE) };
        let h = (vh - 44.).min(560.);
        let w = 588.;

        let mut top = vec![(0., 96.)];
        arc(&mut top, (44., 96.), (44., 96.), 180., 270.);
        top.extend([(w, 0.), (w, 10.), (60., 10.)]);
        arc(&mut top, (60., 26.), (16., 16.), 270., 180.);
        top.push((44., 96.));
        let mut low = vec![(0., h - 96.)];
        arc(&mut low, (44., h - 96.), (44., 96.), 180., 90.);
        low.extend([(w, h), (w, h - 10.), (60., h - 10.)]);
        arc(&mut low, (60., h - 26.), (16., 16.), 90., 180.);
        low.push((44., h - 96.));
        let band = vec![(0., 100.), (44., 100.), (44., h - 100.), (0., h - 100.)];
        let shapes = vec![
            Ink {
                points: top,
                color: main,
                alpha: 1.,
                width: 0.,
            },
            Ink {
                points: band,
                color: side,
                alpha: 1.,
                width: 0.,
            },
            Ink {
                points: low,
                color: main,
                alpha: 1.,
                width: 0.,
            },
        ];

        let (kind, question, detail) = match ask {
            Some(q) => (
                format!("{} · waits {} min", kind_word(a), a.minutes(war.now)),
                q.text.lines().next().unwrap_or("").to_string(),
                q.text.lines().nth(1).unwrap_or("").to_string(),
            ),
            None => {
                let (verb, what) = last_step(a).unwrap_or(("At work".to_string(), String::new()));
                (
                    format!("{} · {} min", state_word(a.state), a.minutes(war.now)),
                    format!("{verb} {what}"),
                    a.title.clone(),
                )
            }
        };
        let tone = if ask.is_some() { RED } else { OR };
        let mut body = div()
            .absolute()
            .left(px(78.))
            .top(px(44.))
            .right(px(18.))
            .flex()
            .flex_col()
            .child(text(18., 24., tone, kind))
            .child(
                div()
                    .mt(px(8.))
                    .font_family(LABEL)
                    .text_size(px(28.))
                    .line_height(px(36.))
                    .text_color(rgb(OR))
                    .child(question.to_uppercase()),
            )
            .child(
                div()
                    .mt(px(12.))
                    .min_h(px(48.))
                    .font_family(LABEL)
                    .text_size(px(18.))
                    .line_height(px(24.))
                    .text_color(rgb(PE))
                    .child(detail),
            );
        if let Some(q) = ask
            && a.controlled
        {
            let mut row = div().relative().mt(px(20.)).h(px(62.));
            let colors = [PE, SA, PK];
            let n = q.options.len().clamp(1, 3);
            for (k, o) in q.options.iter().take(3).enumerate() {
                let id = a.id.clone();
                let w = 482. / n as f32 - 6.;
                row = row.child(
                    spot(k as f32 * (w + 8.), 0., w, 62.)
                        .id(("pl-ans", k))
                        .flex()
                        .justify_end()
                        .items_end()
                        .pr(px(22.))
                        .pb(px(8.))
                        .rounded(px(31.))
                        .bg(rgb(colors[k]))
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.86))
                        .on_click(cx.listener(move |t, _, _, cx| t.planet_answer(&id, k, cx)))
                        .child(text(
                            18.,
                            24.,
                            BG,
                            format!("{} {}", k + 1, short_name(o, 14)),
                        )),
                );
            }
            body = body.child(row);
        } else if ask.is_some() {
            body = body.child(div().mt(px(20.)).child(text(
                18.,
                24.,
                PE,
                "Answer this ask in its terminal",
            )));
        }
        let mut steps = div().flex().flex_col().mt(px(24.));
        for (verb, what) in steps_of(a, 5) {
            steps = steps.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .h(px(32.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(62.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_end()
                            .pr(px(8.))
                            .rounded_r(px(11.))
                            .bg(rgb(verb_color(&verb)))
                            .child(text(14., 18., BG, short_name(&verb, 7))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .font_family(LABEL)
                            .text_size(px(18.))
                            .line_height(px(24.))
                            .text_color(rgb(OR))
                            .whitespace_nowrap()
                            .child(what),
                    ),
            );
        }
        body = body.child(steps);
        spot(x, 0., vw - x, vh)
            .opacity(alpha)
            .bg(rgb(BG))
            .occlude()
            .child(spot(10., 22., w, h).child(layer(shapes)))
            .child(body)
    }
}

fn kind_word(p: &Pane) -> String {
    if p.state == SessionState::Error {
        return "Error".to_string();
    }
    p.ask
        .as_ref()
        .map_or("Question".to_string(), |a| format!("{:?}", a.kind))
}

fn state_word(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "At work",
        SessionState::Idle => "Idle",
        SessionState::NeedsYou => "Waits",
        SessionState::Error => "Error",
        SessionState::Parked => "Parked",
        SessionState::Done => "Done",
    }
}

/// The last tool calls of a session, oldest first: the verb and its first line.
fn steps_of(p: &Pane, n: usize) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = p
        .tail
        .iter()
        .rev()
        .filter(|l| l.kind == LineKind::Tool && !l.text.trim().is_empty())
        .take(n)
        .map(|l| {
            (
                l.label.clone().unwrap_or_else(|| "Tool".to_string()),
                l.text.lines().next().unwrap_or("").to_string(),
            )
        })
        .collect();
    out.reverse();
    out
}

fn last_step(p: &Pane) -> Option<(String, String)> {
    let (verb, what) = steps_of(p, 1).into_iter().next()?;
    Some((verb, module_of(&what).unwrap_or(what)))
}

fn verb_color(verb: &str) -> u32 {
    match verb.to_lowercase().as_str() {
        "edit" | "write" | "multiedit" | "patch" => OR,
        "read" | "grep" | "glob" => PE,
        "bash" | "shell" | "run" => PALE,
        _ => PK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_is_the_folder_of_the_file() {
        assert_eq!(
            module_of("src/webhook/retry.rs · 212 lines").as_deref(),
            Some("webhook")
        );
        assert_eq!(
            module_of("crates/ledger/src/compact.rs").as_deref(),
            Some("ledger")
        );
        assert_eq!(module_of("src/main.rs").as_deref(), Some("main"));
        assert_eq!(module_of("retry.rs").as_deref(), Some("retry"));
    }

    #[test]
    fn a_call_on_no_file_has_no_module() {
        assert_eq!(module_of("cargo test -p payment webhook"), None);
        assert_eq!(module_of("https://example.com/retry"), None);
    }

    #[test]
    fn more_than_eight_codebases_share_the_last_site() {
        let sites: Vec<String> = (0..8).map(|k| format!("c{k}")).collect();
        assert_eq!(site_index(&sites, "c3"), 3);
        assert_eq!(site_index(&sites, "another"), 7);
    }

    #[test]
    fn a_module_beyond_ten_shares_a_pad() {
        let mods: Vec<String> = (0..10).map(|k| format!("m{k}")).collect();
        assert_eq!(pad_index(&mods, "m4"), 4);
        assert!(pad_index(&mods, "another") < 10);
    }

    #[test]
    fn every_site_has_ten_pads_and_an_outline() {
        assert_eq!(defs().len(), 8);
        for s in defs() {
            assert_eq!(s.pads.len(), 10);
            assert!(s.cut.len() > 10);
        }
    }

    #[test]
    fn a_hop_needs_a_real_tool_call() {
        let mut war = aivory_protocol::demo::Demo::new().war_room;
        let mut p = Planet::default();
        let now = Instant::now();
        p.update(&war, now);
        let s = war
            .sessions
            .iter_mut()
            .find(|s| s.state.is_live())
            .expect("a live session");
        let id = s.id.clone();
        let before = p.spots[&id].pad;
        p.update(&aivory_protocol::demo::Demo::new().war_room, now);
        assert_eq!(p.spots[&id].pad, before);
    }

    #[test]
    fn the_glide_starts_and_ends_at_rest() {
        assert!(glide(0.) < 0.001);
        assert!((glide(1.) - 1.).abs() < 0.01);
        assert!(glide(0.5) > 0.3 && glide(0.5) < 0.8);
    }

    #[test]
    fn the_camera_ends_on_its_target() {
        let (a, b) = (
            Cam {
                x: 0.,
                y: 0.,
                z: 1.,
            },
            Cam {
                x: 40.,
                y: -20.,
                z: 9.,
            },
        );
        let c = a.mix(b, 1.);
        assert!((c.z - 9.).abs() < 0.01 && (c.x - 40.).abs() < 0.1);
    }
}
