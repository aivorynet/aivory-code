//! The Command look, war room (`docs/design/looks/rts/MAPPING.md`): the village map. Each codebase
//! is a village around its main building. Each session is a worker robot: it builds a house at
//! its site, or walks the field paths to the forest (a command), the quarry (a read) or the well
//! (a web search). It stands at its flag with a "!" when it waits, sits under a rain cloud after
//! an error, and dozes at the main building when it is idle. A finished session leaves a cottage.
//! Motion means work: the speed of a robot follows the work rate of its session.
//!
//! A click selects a session and shows the workshop window at the bottom. O or a double click
//! opens the session. Space goes to the next wait, F finds, 1-9 go to a village, drag or the arrow
//! keys move the view. The sidebar is in `shell/command.rs`.

use std::cell::Cell;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aivory_protocol::{Line, LineKind, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    RenderImage, ScrollWheelEvent, Window, div, img, px,
};

use crate::actions::Action;
use crate::command_kit::{self as cmd, Btn, c};
use crate::rts::layout::{Item, Layout, SITE_H, SITE_W, Tool, Village, layout, trail};
use crate::rts::{
    ROBOT_FOOT, ROBOT_REST, ROBOT_WALK, ROBOT_WORK, Sprite, sprites, terrain, to_render,
};
use crate::warroom::{WarRoomEvent, WarRoomScreen};

/// The width of the command sidebar (the map takes the rest of the window).
pub const SIDE_W: f32 = 320.;
/// The height of the workshop window.
const SHOP_H: f32 = 204.;
/// The redraw times: while robots work, and for a calm map.
const TICK_WORK: u64 = 50;
/// The names on the map are signs of dark wood, in the colors of the buildings.
const SIGN_WOOD: u32 = 0x33261A;
const SIGN_EDGE: u32 = 0x6A5236;
const SIGN_TEXT: u32 = 0xE6DAC0;

/// The height of a message in the box of waits.
const LOG_ROW: f32 = 58.;
const TICK_CALM: u64 = 400;

/// The ground image and its radar copy, for one layout.
pub struct Ground {
    pub key: u64,
    pub image: Arc<RenderImage>,
    pub radar: Arc<RenderImage>,
    pub radar_size: (f32, f32),
}

/// The work rate of a session, from the growth of its tokens.
#[derive(Clone, Copy)]
struct Rate {
    tokens: u64,
    at: i64,
    per_min: f32,
}

/// The state of the map.
#[derive(Default)]
pub struct RtsMap {
    pub pan: (f32, f32),
    drag: Option<(f32, f32, f32, f32)>,
    pub layout: Rc<Layout>,
    pub ground: Option<Rc<Ground>>,
    building: Option<u64>,
    pub selected: Option<String>,
    /// The longest wait that was selected for the owner: a new one takes the selection again.
    auto: Option<String>,
    pub view: (f32, f32),
    hover: Option<String>,
    /// The text of the find box, when it is open.
    find: Option<String>,
    /// The work phase of each session: it runs at the work rate, so a new rate makes no jump.
    phase: HashMap<String, f32>,
    rates: HashMap<String, Rate>,
    clock: f32,
    last: Option<Instant>,
    waiting: bool,
    /// The box of waits is minimized to an icon.
    log_small: bool,
    /// The zoom; 0 until the first frame sets it to 1.
    pub zoom: f32,
}

thread_local! {
    /// The zoom of the frame in progress: all drawing of the map goes through `z`.
    static ZOOM: Cell<f32> = const { Cell::new(1.) };
}

/// The zoom limits and the step of a key, a button or a wheel notch.
const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 2.;
const ZOOM_STEP: f32 = 1.15;

/// A length on the map as a length on the screen.
fn z(v: f32) -> Pixels {
    px((v * ZOOM.get()).floor())
}

/// The speed factor of a work rate in tokens per minute.
fn speed(per_min: f32) -> f32 {
    (0.7 + per_min / 3000.).clamp(0.7, 2.4)
}

/// The next rate estimate: a soft average of the new sample.
fn next_rate(old: Rate, tokens: u64, at: i64) -> Rate {
    let ms = at - old.at;
    if ms < 1000 {
        return old;
    }
    let sample = tokens.saturating_sub(old.tokens) as f32 * 60_000. / ms as f32;
    Rate {
        tokens,
        at,
        per_min: old.per_min * 0.6 + sample * 0.4,
    }
}

/// The session after `current` in the list of waits, or the first one.
fn next_wait<'a>(waits: &'a [String], current: Option<&str>) -> Option<&'a String> {
    let at = current.and_then(|c| waits.iter().position(|w| w == c));
    match at {
        Some(k) => waits.get((k + 1) % waits.len()),
        None => waits.first(),
    }
}

/// The places on the walk from a site to a resource: back to the main building on the site trail,
/// then out on the resource trail. `f` runs from 0 (site) to 1 (resource).
fn errand(
    v: &Village,
    yard: (f32, f32),
    seed: u32,
    target: (f32, f32),
    rs: u32,
    f: f32,
) -> (f32, f32) {
    if f < 0.5 {
        trail(v.door(), yard, seed, 1. - f * 2.)
    } else {
        trail(v.door(), target, rs, (f - 0.5) * 2.)
    }
}

/// A sprite at its ground point (the center of the image).
fn stand(s: &Sprite, x: f32, y: f32) -> gpui::Img {
    sprite(s, x - s.w() / 2., y - s.h() / 2.)
}

/// A sprite at a place, on whole pixels.
fn sprite(s: &Sprite, x: f32, y: f32) -> gpui::Img {
    img(s.render.clone())
        .absolute()
        .left(z(x))
        .top(z(y))
        .w(z(s.w()))
        .h(z(s.h()))
}

/// A block of pixels.
fn dot(x: f32, y: f32, w: f32, h: f32, color: u32, alpha: f32) -> AnyElement {
    div()
        .absolute()
        .left(z(x))
        .top(z(y))
        .w(z(w))
        .h(z(h))
        .bg(c(color, alpha))
        .into_any_element()
}

/// A soft round cloud of dust, smoke or rain.
fn puff(x: f32, y: f32, r: f32, color: u32, alpha: f32) -> AnyElement {
    div()
        .absolute()
        .left(z(x - r))
        .top(z(y - r * 0.8))
        .w(z(r * 2.))
        .h(z(r * 1.6))
        .rounded_full()
        .bg(c(color, alpha))
        .into_any_element()
}

/// The small state flag of a site.
fn flag(x: f32, y: f32, color: u32) -> [AnyElement; 2] {
    [
        dot(x, y - 22., 2., 22., 0x4A3018, 1.),
        dot(x + 2., y - 22., 12., 8., color, 1.),
    ]
}

/// A "!" bubble over a robot.
fn bubble(x: f32, y: f32) -> AnyElement {
    div()
        .absolute()
        .left(z(x))
        .top(z(y))
        .w(z(18.))
        .h(z(22.))
        .flex()
        .items_center()
        .justify_center()
        .bg(c(0xFFFFF0, 1.))
        .border_2()
        .border_color(c(0x141414, 1.))
        .child(cmd::data("!", 20. * ZOOM.get()).text_color(c(0xD08A00, 1.)))
        .into_any_element()
}

/// Four corner brackets around a box.
fn brackets(x: f32, y: f32, w: f32, h: f32, color: u32) -> Vec<AnyElement> {
    let l = 16.;
    let col = c(color, 1.);
    let corner = |left: bool, top: bool| {
        let d = div().absolute().size(z(l)).border_color(col);
        let d = if left {
            d.left(z(x)).border_l_2()
        } else {
            d.left(z(x + w - l)).border_r_2()
        };
        let d = if top {
            d.top(z(y)).border_t_2()
        } else {
            d.top(z(y + h - l)).border_b_2()
        };
        d.into_any_element()
    };
    vec![
        corner(true, true),
        corner(false, true),
        corner(true, false),
        corner(false, false),
    ]
}

/// One of the eight directions of the robot for a move on the map. The map shows the ground at half
/// height, so a move up the screen counts double.
fn heading(dx: f32, dy: f32) -> usize {
    let turn = dx.atan2(dy * 2.) / std::f32::consts::FRAC_PI_4;
    turn.round().rem_euclid(8.) as usize
}

/// A frame of a walk after `dist` map pixels: a full stride (two steps) is 26 pixels.
fn walk_frame(dist: f32) -> usize {
    ROBOT_WALK + ((dist / 26. * 6.) as usize) % 6
}

/// A frame of the blow of a tool at a work phase.
fn work_frame(p: f32) -> usize {
    ROBOT_WORK + ((p.rem_euclid(1.) * 6.) as usize).min(5)
}

/// The tool hits the work in this frame.
fn is_hit(frame: usize) -> bool {
    frame == ROBOT_WORK + 3
}

/// The robot at a foot point with its shadow: `load` is what it carries.
fn robot(x: f32, y: f32, dir: usize, frame: usize, load: Option<Tool>, out: &mut Vec<AnyElement>) {
    let sp = sprites().robot(dir, frame);
    out.push(
        div()
            .absolute()
            .left(z(x - 10.))
            .top(z(y - 3.))
            .w(z(26.))
            .h(z(10.))
            .rounded_full()
            .bg(c(0x000000, 0.34))
            .into_any_element(),
    );
    out.push(sprite(sp, x - ROBOT_FOOT.0, y - ROBOT_FOOT.1).into_any_element());
    let top = y - 47.;
    match load {
        Some(Tool::Run) => {
            out.push(dot(x - 11., top - 6., 22., 6., 0x3C2614, 1.));
            out.push(dot(x - 10., top - 5., 20., 4., 0x96602F, 1.));
            out.push(dot(x + 6., top - 5., 4., 4., 0xD6AA6E, 1.));
        }
        Some(Tool::Read) => {
            for q in 0..3 {
                let (qx, qy) = (x - 7. + q as f32 * 5., top - 5. - (q % 2) as f32 * 2.);
                out.push(dot(qx, qy, 5., 5., 0x3C3832, 1.));
                out.push(dot(qx + 1., qy + 1., 3., 3., 0xAAA69A, 1.));
            }
        }
        Some(Tool::Web) => {
            out.push(dot(x - 5., top - 8., 10., 8., 0x28282E, 1.));
            out.push(dot(x - 4., top - 7., 8., 6., 0x78787F, 1.));
            out.push(dot(x - 3., top - 7., 6., 2., 0x5096E6, 1.));
        }
        _ => {}
    }
}

/// How much of a robot shows at a place: it fades out as it walks into the main building or into a
/// house on its way, and comes out at the other side. Nothing walks behind a building.
fn outdoors(v: &Village, own: &str, x: f32, y: f32) -> f32 {
    let open = |dx: f32, dy: f32| ((dx.hypot(dy) - 0.84) / 0.16).clamp(0., 1.);
    let mut seen = open((x - v.cx) / 125., (y - v.cy + 16.) / 76.);
    for o in &v.items {
        if let (true, Some((sx, sy))) = (o.id != own, o.site) {
            let (dx, dy) = (x - sx - SITE_W / 2., y - sy - SITE_H / 2. + 8.);
            seen = seen.min(open(dx / 92., dy / 68.));
        }
    }
    seen
}

/// The swing and the chips of work at a place.
fn sparks(x: f32, y: f32, t: f32, color: u32, out: &mut Vec<AnyElement>) {
    let up = (t * 6.).fract() < 0.5;
    out.push(dot(x, y + if up { -6. } else { 0. }, 2., 8., 0x6E4622, 1.));
    for q in 0..3 {
        let f = (t * 2. + q as f32 * 0.33).fract();
        out.push(dot(
            x + 6. + q as f32 * 4.,
            y + 2. - f * 12.,
            2.,
            2.,
            color,
            1. - f,
        ));
    }
}

/// The picture of one session at a time `t`, its work phase `p`.
fn draw_item(v: &Village, it: &Item, t: f32, p: f32, out: &mut Vec<(f32, AnyElement)>) {
    let mut layer: Vec<AnyElement> = Vec::new();
    let seed = it.seed;
    // Each robot of a crew stands by itself in the depth order of the scene.
    let mut alone = |y: f32, seen: f32, layer: Vec<AnyElement>| {
        if seen > 0. {
            let d = div().absolute().size_full().opacity(seen).children(layer);
            out.push((y, d.into_any_element()));
        }
    };
    let shift = (seed % 97) as f32 / 97.;
    let foot;
    match it.state {
        SessionState::Idle => {
            // Rests with a low head: a "Z" floats up.
            foot = it.y;
            robot(it.x, it.y, 1, ROBOT_REST, None, &mut layer);
            let f = (t * 0.25 + shift).fract();
            layer.push(
                cmd::data("Z", 18. * ZOOM.get())
                    .absolute()
                    .left(z(it.x + 8. + f * 8.))
                    .top(z(it.y - 52. - f * 18.))
                    .text_color(c(0xFFFFFF, 1. - f))
                    .into_any_element(),
            );
        }
        SessionState::Working => {
            let Some((sx, sy)) = it.site else { return };
            foot = sy + SITE_H + 1.;
            // The crew works along the front of the site: each robot has its own stretch with two
            // places, so that no robot stands in front of another or behind the building.
            let (cx, cy) = (sx + SITE_W / 2., sy + SITE_H / 2.);
            let (rx, ry) = (SITE_W / 2. + 8., SITE_H / 2. - 4.);
            let builder = usize::from(it.tool == Tool::Edit);
            let crew = it.helpers + builder;
            let arc = TAU * 0.6;
            for k in 0..crew {
                let share = k as f32 / crew as f32;
                let part = arc / crew as f32;
                let s = p / 6. + share + shift;
                let f = s.fract();
                let walking = f < 0.28;
                let moved = if walking { f / 0.28 } else { 1. };
                let there = (s.floor() as i64).rem_euclid(2) as f32;
                let turn = if there > 0. { 1. } else { -1. };
                let place = (1. - there) * 0.25 + there * 0.75;
                let a = -TAU * 0.05 + arc * share + part * (place - turn * 0.5 * (1. - moved));
                let (x, y) = (cx + a.cos() * rx, cy + a.sin() * ry);
                let (dir, frame) = if walking {
                    let dist = moved * part * 0.5 * rx;
                    (
                        heading(-a.sin() * rx * turn, a.cos() * ry * turn),
                        walk_frame(dist),
                    )
                } else {
                    (heading(cx - x, cy - y), work_frame(p * 0.9 + share))
                };
                let mut one = Vec::new();
                robot(x, y, dir, frame, None, &mut one);
                if is_hit(frame) {
                    sparks(
                        x + (cx - x).signum() * 12.,
                        y - 16.,
                        t + share,
                        0xFFE9A0,
                        &mut one,
                    );
                }
                alone(y, 1., one);
            }
            if it.tool == Tool::Edit {
                for j in 0..2 {
                    let f = (p * 0.5 + j as f32 * 0.5).fract();
                    layer.push(puff(
                        sx + 40. + j as f32 * 44.,
                        sy + 54. - f * 24.,
                        5. + f * 6.,
                        0xF5F0E4,
                        0.6 * (1. - f),
                    ));
                }
            } else if let Some(yard) = it.yard() {
                // An errand: to the resource, work there, back with the load.
                let target = v.resource(it.tool);
                let rs = match it.tool {
                    Tool::Run | Tool::Edit => 11,
                    Tool::Read => 12,
                    Tool::Web => 13,
                };
                let cyc = (p * 0.1 + shift).fract();
                let (f, back, load, work) = if cyc < 0.42 {
                    (cyc / 0.42, false, None, false)
                } else if cyc < 0.58 {
                    (1., false, None, true)
                } else {
                    (1. - (cyc - 0.58) / 0.42, true, Some(it.tool), false)
                };
                // Each robot has its own place at the resource and its own side of the path.
                let (ox, oy) = (
                    ((seed % 5) as f32 - 2.) * 30.,
                    ((seed % 3) as f32 - 1.) * 10.,
                );
                let at = |f: f32| {
                    let (x, y) = errand(v, yard, seed, target, rs, f);
                    (x + ox * f * f, y + oy * f * f + (shift - 0.5) * 12.)
                };
                let (x, y) = at(f);
                let len: f32 = (0..16)
                    .map(|k| {
                        let (a, b) = (at(k as f32 / 16.), at((k + 1) as f32 / 16.));
                        (b.0 - a.0).hypot(b.1 - a.1)
                    })
                    .sum();
                let dir = if work {
                    4
                } else {
                    let (a, b) = (at((f - 0.01).max(0.)), at((f + 0.01).min(1.)));
                    let sign = if back { -1. } else { 1. };
                    heading((b.0 - a.0) * sign, (b.1 - a.1) * sign)
                };
                let frame = if work {
                    work_frame(p * 0.9)
                } else {
                    walk_frame(f * len)
                };
                robot(x, y, dir, frame, load, &mut layer);
                if is_hit(frame) {
                    let chip = match it.tool {
                        Tool::Run => 0xC48C50,
                        Tool::Read => 0xC8C4B6,
                        _ => 0x78B4F0,
                    };
                    sparks(x + 10., y - 28., t + shift, chip, &mut layer);
                }
                alone(y, outdoors(v, &it.id, x, y), layer);
                return;
            }
        }
        SessionState::NeedsYou => {
            let Some((sx, sy)) = it.site else { return };
            foot = sy + SITE_H + 1.;
            let (x, y) = (sx + 132., sy + 104.);
            robot(x, y, 0, ROBOT_REST, None, &mut layer);
            let bob = if (t * 1.2).fract() < 0.5 { 2. } else { 0. };
            layer.push(bubble(x - 9., y - 64. - bob));
        }
        SessionState::Error => {
            let Some((sx, sy)) = it.site else { return };
            foot = sy + SITE_H + 1.;
            let (x, y) = (sx + 85., sy + 112.);
            robot(x, y, 0, ROBOT_REST, None, &mut layer);
            layer.push(puff(x - 6., y - 50., 9., 0x5A5C64, 0.92));
            layer.push(puff(x + 7., y - 52., 10., 0x5A5C64, 0.92));
            for q in 0..4 {
                let f = (t * 1.5 + q as f32 * 0.27).fract();
                layer.push(dot(
                    x - 10. + q as f32 * 6.,
                    y - 42. + f * 30.,
                    1.,
                    4.,
                    0x78A8DC,
                    1. - f,
                ));
            }
        }
        _ => return,
    }
    alone(foot, 1., layer);
}

impl WarRoomScreen {
    /// The last viewport, the pan and the layout (the sidebar radar reads them).
    pub fn rts_map(&self) -> &RtsMap {
        &self.rts
    }

    /// Move the view so that a point of the map (in map pixels) is in the middle.
    pub fn rts_jump(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        self.rts.pan = (x - self.rts.view.0 / 2., y - self.rts.view.1 / 2.);
        self.clamp_pan();
        cx.notify();
    }

    /// Esc: close the find box, else the workshop window.
    pub(crate) fn rts_close(&mut self) {
        if self.rts.find.take().is_none() {
            self.rts.selected = None;
        }
    }

    /// Go to a session on the map and select it.
    pub fn rts_goto(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some((x, y)) = self.rts.layout.place(id) {
            self.rts_jump(x, y, cx);
        }
        self.rts.selected = Some(id.to_string());
        cx.notify();
    }

    /// Go to a village by its name.
    pub fn rts_village(&mut self, name: &str, cx: &mut Context<Self>) {
        let at = self
            .rts
            .layout
            .villages
            .iter()
            .find(|v| v.name == name)
            .map(|v| (v.cx, v.cy));
        if let Some((x, y)) = at {
            self.rts_jump(x, y, cx);
        }
    }

    fn clamp_pan(&mut self) {
        let l = &self.rts.layout;
        let (vw, vh) = self.rts.view;
        self.rts.pan.0 = self.rts.pan.0.clamp(0., (l.w - vw).max(0.));
        self.rts.pan.1 = self.rts.pan.1.clamp(0., (l.h - vh).max(0.));
    }

    /// What the find box matches: sessions by title, villages by name.
    fn rts_matches(&self, text: &str) -> Vec<(String, Option<String>)> {
        let q = text.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        for v in &self.rts.layout.villages {
            if v.name.to_lowercase().contains(&q) {
                out.push((v.name.clone(), None));
            }
            for it in &v.items {
                if it.title.to_lowercase().contains(&q) {
                    out.push((it.title.clone(), Some(it.id.clone())));
                }
            }
        }
        out.truncate(5);
        out
    }

    fn rts_find_key(&mut self, k: &Keystroke, cx: &mut Context<Self>) {
        let Some(mut text) = self.rts.find.take() else {
            return;
        };
        match k.key.as_str() {
            "escape" => {
                cx.notify();
                return;
            }
            "enter" => {
                let hit = self.rts_matches(&text).into_iter().next();
                match hit {
                    Some((_, Some(id))) => self.rts_goto(&id, cx),
                    Some((name, None)) => self.rts_village(&name, cx),
                    None => {}
                }
                cx.notify();
                return;
            }
            "backspace" => {
                text.pop();
            }
            "space" => text.push(' '),
            _ => {
                if let Some(ch) = k
                    .key_char
                    .as_ref()
                    .filter(|ch| !k.modifiers.control && ch.chars().count() == 1)
                {
                    text.push_str(ch);
                }
            }
        }
        self.rts.find = Some(text);
        cx.notify();
    }

    /// The keys of the map.
    pub(crate) fn rts_key(&mut self, k: &Keystroke, cx: &mut Context<Self>) {
        if self.rts.find.is_some() {
            self.rts_find_key(k, cx);
            return;
        }
        let step = 160.;
        let key = k.key.as_str();
        match key {
            "left" => self.rts.pan.0 -= step,
            "right" => self.rts.pan.0 += step,
            "up" => self.rts.pan.1 -= step,
            "down" => self.rts.pan.1 += step,
            "o" => {
                if let Some(id) = self.rts.selected.clone() {
                    cx.emit(WarRoomEvent::OpenSession(id));
                }
                return;
            }
            "n" => {
                cx.emit(WarRoomEvent::NewSession);
                return;
            }
            "f" => self.rts.find = Some(String::new()),
            "+" | "=" => {
                self.rts_zoom(ZOOM_STEP, None, cx);
                return;
            }
            "-" => {
                self.rts_zoom(1. / ZOOM_STEP, None, cx);
                return;
            }
            "space" => {
                let waits = self
                    .model
                    .read(cx)
                    .war
                    .as_ref()
                    .map(rts_waits)
                    .unwrap_or_default();
                if let Some(id) = next_wait(&waits, self.rts.selected.as_deref()).cloned() {
                    self.rts_goto(&id, cx);
                }
                return;
            }
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                let n = key.parse::<usize>().unwrap_or(1) - 1;
                // With an open ask the first keys answer it (the buttons show the numbers).
                if n < 3 && self.rts_options() > n {
                    self.rts_answer(n, cx);
                    return;
                }
                if let Some(v) = self.rts.layout.villages.get(n).map(|v| v.name.clone()) {
                    self.rts_village(&v, cx);
                }
                return;
            }
            "escape" => self.rts.selected = None,
            _ => return,
        }
        self.clamp_pan();
        cx.notify();
    }

    /// The number of answers of the selected session.
    fn rts_options(&self) -> usize {
        self.rts
            .selected
            .as_ref()
            .and_then(|id| self.war.as_ref()?.session(id))
            .and_then(|p| p.ask.as_ref())
            .map_or(0, |a| a.options.len())
    }

    fn rts_answer(&mut self, k: usize, cx: &mut Context<Self>) {
        let Some(id) = self.rts.selected.clone() else {
            return;
        };
        let Some(pane) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| w.session(&id))
            .cloned()
        else {
            return;
        };
        let Some(ask) = pane.ask else {
            return;
        };
        let Some(opt) = ask.options.get(k).cloned() else {
            return;
        };
        self.act(
            Action::Answer {
                id,
                ask: ask.id,
                option: Some(opt),
                text: None,
            },
            cx,
        );
    }

    /// Draw the ground again in the background when the plots or the sites change.
    fn ensure_ground(&mut self, cx: &mut Context<Self>) {
        let key = self.rts.layout.ground_key();
        if self.rts.ground.as_ref().is_some_and(|g| g.key == key) || self.rts.building == Some(key)
        {
            return;
        }
        self.rts.building = Some(key);
        let l = (*self.rts.layout).clone();
        let task = cx.background_executor().spawn(async move {
            let g = terrain::ground(&l);
            let r = terrain::radar(&g, 264, 188);
            let size = (r.width() as f32, r.height() as f32);
            (to_render(&g), to_render(&r), size)
        });
        cx.spawn(async move |this, cx| {
            let (image, radar, radar_size) = task.await;
            let _ = this.update(cx, |t, cx| {
                if t.rts.building == Some(key) {
                    t.rts.building = None;
                }
                t.rts.ground = Some(Rc::new(Ground {
                    key,
                    image,
                    radar,
                    radar_size,
                }));
                cx.notify();
            });
        })
        .detach();
    }

    /// Move the clock and the work phases; ask for the next frame.
    fn rts_tick(&mut self, war: &WarRoom, cx: &mut Context<Self>) {
        let now = Instant::now();
        let dt = self
            .rts
            .last
            .replace(now)
            .map_or(0., |l| (now - l).as_secs_f32().min(0.5));
        self.rts.clock = (self.rts.clock + dt) % 3600.;
        let mut working = false;
        for p in &war.sessions {
            if p.state != SessionState::Working {
                continue;
            }
            working = true;
            let r = self
                .rts
                .rates
                .entry(p.id.clone())
                .and_modify(|r| *r = next_rate(*r, p.tokens, war.now))
                .or_insert(Rate {
                    tokens: p.tokens,
                    at: war.now,
                    per_min: 0.,
                });
            let v = speed(r.per_min);
            let ph = self.rts.phase.entry(p.id.clone()).or_insert(0.);
            *ph = (*ph + dt * v) % 3600.;
        }
        if !self.rts.waiting {
            self.rts.waiting = true;
            let ms = if working { TICK_WORK } else { TICK_CALM };
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(ms))
                    .await;
                let _ = this.update(cx, |t, cx| {
                    t.rts.waiting = false;
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// The Command war room: the map with its overlays.
    pub(super) fn render_command(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vp = window.viewport_size();
        self.rts.zoom = self.rts_scale();
        ZOOM.set(self.rts.zoom);
        self.rts.view = (
            (f32::from(vp.width) - SIDE_W).max(200.) / self.rts.zoom,
            f32::from(vp.height) / self.rts.zoom,
        );
        let lay = layout(war);
        if *self.rts.layout != lay {
            self.rts.layout = Rc::new(lay);
        }
        self.clamp_pan();
        self.ensure_ground(cx);
        self.rts_tick(war, cx);
        // The map keys need the focus: take it when nothing else holds it.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        // A new longest wait takes the selection once; Esc can close it.
        let first = war.board.asks.first().cloned();
        if first != self.rts.auto {
            self.rts.auto = first.clone();
            if first.is_some() {
                self.rts.selected = first;
            }
        }
        if let Some(id) = &self.rts.selected
            && war.session(id).is_none()
        {
            self.rts.selected = None;
        }
        let lay = self.rts.layout.clone();
        let (pan, (vw, vh)) = (self.rts.pan, self.rts.view);
        let seen = |x: f32, y: f32, w: f32, h: f32| {
            x + w > pan.0 - 60.
                && x < pan.0 + vw + 60.
                && y + h > pan.1 - 60.
                && y < pan.1 + vh + 60.
        };
        let s = sprites();
        let t = self.rts.clock;

        let mut world = div()
            .absolute()
            .left(z(-pan.0))
            .top(z(-pan.1))
            .w(z(lay.w))
            .h(z(lay.h));
        world = match &self.rts.ground {
            Some(g) => world.child(
                img(g.image.clone())
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(z(lay.w))
                    .h(z(lay.h)),
            ),
            None => world.bg(c(0x0A0C0A, 1.)),
        };
        // Everything that stands on the ground, sorted by the foot line.
        let mut scene: Vec<(f32, AnyElement)> = Vec::new();
        let mut labels: Vec<AnyElement> = Vec::new();
        // Nothing stands on the map before its ground is ready.
        let ready = self.rts.ground.is_some();
        for v in lay.villages.iter().filter(|_| ready) {
            // the main building, with smoke while its village works
            if seen(v.cx - 180., v.cy - 240., 360., 400.) {
                scene.push((v.cy + 40., stand(&s.hall, v.cx, v.cy).into_any_element()));
                if v.items.iter().any(|i| i.state == SessionState::Working) {
                    for j in 0..3 {
                        let f = (t * 0.5 + j as f32 / 3.).fract();
                        scene.push((
                            v.cy + 41.,
                            puff(
                                v.cx + 52. + (f * 5.).sin() * 3.,
                                v.cy - 190. - f * 34.,
                                4. + f * 6.,
                                0xF5F0E4,
                                0.66 * (1. - f),
                            ),
                        ));
                    }
                }
                labels.push(
                    div()
                        .absolute()
                        .left(z(v.cx - pan.0) - px(160.))
                        .top(z(v.cy - 236. - pan.1))
                        .w(px(320.))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .px(px(9.))
                                .pt(px(3.))
                                .pb(px(2.))
                                .bg(c(SIGN_WOOD, 0.9))
                                .border_1()
                                .border_color(c(SIGN_EDGE, 1.))
                                .child(cmd::data(v.name.clone(), 19.).text_color(c(SIGN_TEXT, 1.)))
                                .child(
                                    cmd::data(v.slots.clone(), 19.).text_color(c(
                                        if v.free { 0xB4C68C } else { SIGN_TEXT },
                                        1.,
                                    )),
                                ),
                        )
                        .into_any_element(),
                );
            }
            // the resources (the forest is part of the ground)
            let (qx, qy) = v.quarry;
            if seen(qx - 150., qy - 130., 300., 260.) {
                scene.push((qy + 40., stand(&s.quarry, qx, qy).into_any_element()));
            }
            let (wx, wy) = v.well;
            if seen(wx - 40., wy - 70., 90., 90.) {
                scene.push((wy + 30., stand(&s.well, wx, wy).into_any_element()));
            }
            // the sites, cottages and robots
            for it in &v.items {
                let (x, y) = (it.x, it.y);
                let shows = if it.state == SessionState::Working && it.tool != Tool::Edit {
                    true
                } else {
                    seen(x - 90., y - 130., 180., 170.)
                };
                if !shows {
                    continue;
                }
                let phase = self.rts.phase.get(&it.id).copied().unwrap_or(0.);
                let sel = self.rts.selected.as_deref() == Some(it.id.as_str());
                let hover = self.rts.hover.as_deref() == Some(it.id.as_str());
                draw_item(v, it, t, phase, &mut scene);
                let (hx, hy, hw, hh, foot) = match it.site {
                    Some((sx, sy)) => {
                        let (sp, lift) = if it.is_house() {
                            (s.cottage(&it.provider), 0.)
                        } else {
                            (s.site(it.seed), 0.)
                        };
                        let (dx, dy) = (sx + SITE_W / 2., sy + SITE_H / 2. + lift);
                        // Robots behind the middle of a site are behind it, the others in front.
                        scene.push((dy, stand(sp, dx, dy).into_any_element()));
                        if !it.is_house() {
                            let col = state_color(it.state);
                            let mut f = flag(sx + 30., sy + 96., col).into_iter();
                            if let (Some(a), Some(b)) = (f.next(), f.next()) {
                                scene.push((sy + SITE_H + 1., a));
                                scene.push((sy + SITE_H + 1., b));
                            }
                        }
                        (sx, sy, SITE_W, SITE_H, sy + SITE_H)
                    }
                    None => (x - 18., y - 42., 36., 46., y),
                };
                let id = it.id.clone();
                let open = it.id.clone();
                let over = it.id.clone();
                scene.push((
                    foot + 2.,
                    div()
                        .id(gpui::SharedString::from(format!("rts-i-{}", it.id)))
                        .absolute()
                        .left(z(hx))
                        .top(z(hy))
                        .w(z(hw))
                        .h(z(hh))
                        .cursor_pointer()
                        .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                            if *on {
                                this.rts.hover = Some(over.clone());
                            } else if this.rts.hover.as_deref() == Some(over.as_str()) {
                                this.rts.hover = None;
                            }
                            cx.notify();
                        }))
                        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                            if ev.click_count() >= 2 {
                                cx.emit(WarRoomEvent::OpenSession(open.clone()));
                            } else {
                                this.rts.selected = Some(id.clone());
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                ));
                if sel || matches!(it.state, SessionState::NeedsYou | SessionState::Error) {
                    let col = if sel { 0xFFFFFF } else { state_color(it.state) };
                    for e in brackets(hx - 4., hy - 4., hw + 8., hh + 8., col) {
                        scene.push((foot + 3., e));
                    }
                }
                // Titles of the sites stay; a cottage or a dozing robot names itself on hover.
                // They leave the map when it is zoomed far out, where they would overlap.
                let far = self.rts.zoom < 0.75;
                if it.site.is_some() && !it.is_house() && !far || hover || sel {
                    labels.push(
                        cmd::data(cmd::up(&it.title), 16.)
                            .absolute()
                            .left(z(hx - 2. - pan.0))
                            .top(z(hy + hh + 2. - pan.1))
                            .max_w(px(160.))
                            .px(px(4.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .bg(c(SIGN_WOOD, 0.62))
                            .text_color(c(
                                match it.state {
                                    SessionState::NeedsYou => 0xE8C478,
                                    SessionState::Error => 0xE29A8A,
                                    _ => SIGN_TEXT,
                                },
                                1.,
                            ))
                            .into_any_element(),
                    );
                }
            }
        }
        scene.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, e) in scene {
            world = world.child(e);
        }

        let log = self.rts_log(war, cx);
        let shop = self.rts_shop(war, cx);
        let find = self.rts_find(cx);
        let zoom_box =
            div()
                .absolute()
                .top(px(12.))
                .right(px(12.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    cmd::button("rts-zoom-in", "+", Btn::Plain)
                        .on_click(cx.listener(|this, _, _, cx| this.rts_zoom(ZOOM_STEP, None, cx))),
                )
                .child(cmd::button("rts-zoom-out", "-", Btn::Plain).on_click(
                    cx.listener(|this, _, _, cx| this.rts_zoom(1. / ZOOM_STEP, None, cx)),
                ));
        let hint = shop.is_none().then(|| {
            cmd::data(
                format!(
                    "DRAG MOVES · WHEEL ZOOMS · SPACE NEXT WAIT · F FIND · 1-9 VILLAGES · {} VILLAGES",
                    lay.villages.len()
                ),
                18.,
            )
            .absolute()
            .left(px(16.))
            .bottom(px(12.))
            .px(px(6.))
            .bg(c(0x0A060C, 0.78))
            .text_color(c(0xD6E6C0, 1.))
        });

        div()
            .id("rts-map")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(c(0x0A0C0A, 1.))
            .font_family(cmd::SANS)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, _| {
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                    this.rts.drag = Some((x, y, this.rts.pan.0, this.rts.pan.1));
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some((x0, y0, p0, p1)) = this.rts.drag {
                    if ev.pressed_button != Some(MouseButton::Left) {
                        this.rts.drag = None;
                        return;
                    }
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                    let k = this.rts_scale();
                    this.rts.pan = (p0 - (x - x0) / k, p1 - (y - y0) / k);
                    this.clamp_pan();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.rts.drag = None),
            )
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let dy = f32::from(ev.delta.pixel_delta(px(20.)).y);
                if dy != 0. {
                    let at = (f32::from(ev.position.x), f32::from(ev.position.y));
                    let step = if dy > 0. { ZOOM_STEP } else { 1. / ZOOM_STEP };
                    this.rts_zoom(step, Some(at), cx);
                }
            }))
            .child(world)
            .children((!ready).then(|| {
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(cmd::data("PREPARING THE MAP", 24.).text_color(c(0xD6E6C0, 1.)))
            }))
            .children(labels)
            .child(zoom_box)
            .children(log)
            .children(shop)
            .children(find)
            .children(hint)
            .into_any_element()
    }

    /// The zoom in use.
    fn rts_scale(&self) -> f32 {
        if self.rts.zoom > 0. {
            self.rts.zoom
        } else {
            1.
        }
    }

    /// Change the zoom by a factor. The map point under `at` (a screen point, else the middle of the
    /// view) stays where it is.
    fn rts_zoom(&mut self, factor: f32, at: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let old = self.rts_scale();
        let new = (old * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let (mx, my) = at.unwrap_or((self.rts.view.0 * old / 2., self.rts.view.1 * old / 2.));
        let point = (self.rts.pan.0 + mx / old, self.rts.pan.1 + my / old);
        self.rts.zoom = new;
        self.rts.view = (self.rts.view.0 * old / new, self.rts.view.1 * old / new);
        self.rts.pan = (point.0 - mx / new, point.1 - my / new);
        self.clamp_pan();
        cx.notify();
    }

    /// What needs you, in a box: a click on a message goes to its place on the map.
    fn rts_log(&mut self, war: &WarRoom, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let waits = rts_waits(war);
        if waits.is_empty() {
            return None;
        }
        if self.rts.log_small {
            // Minimized: only the mail icon and the number of waits.
            return Some(
                div()
                    .id("rts-log")
                    .absolute()
                    .left(px(12.))
                    .top(px(12.))
                    .h(px(42.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .bg(c(0x1E2124, 0.97))
                    .cursor_pointer()
                    .hover(|st| st.bg(c(0x24301C, 1.)))
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.rts.log_small = false;
                        cx.notify();
                    }))
                    .child(crate::ui::icon(
                        "icons/mail.svg",
                        24.,
                        gpui::rgba(0xFFFFFFFF),
                    ))
                    .child(cmd::data(waits.len().to_string(), 24.).text_color(c(cmd::AMBER, 1.))),
            );
        }
        let mut list = div()
            .id("rts-log-list")
            .flex()
            .flex_col()
            .max_h(px(LOG_ROW * 5.5))
            .overflow_y_scroll();
        for (k, id) in waits.iter().enumerate() {
            let Some(p) = war.session(id) else { continue };
            let error = p.state == SessionState::Error;
            let color = if error { cmd::RED } else { cmd::AMBER };
            let what = if error {
                "STOPPED ON AN ERROR".to_string()
            } else {
                let text = p.ask.as_ref().map_or("", |a| a.text.as_str());
                cmd::up(text.lines().next().unwrap_or(""))
            };
            let selected = self.rts.selected.as_deref() == Some(id.as_str());
            let go = id.clone();
            list = list.child(
                div()
                    .id(("rts-go", k))
                    .flex_none()
                    .h(px(LOG_ROW))
                    .flex()
                    .gap(px(10.))
                    .pr(px(10.))
                    .border_b_1()
                    .border_color(c(0x34383C, 1.))
                    .bg(c(if selected { 0x2C3226 } else { cmd::WELL }, 1.))
                    .cursor_pointer()
                    .hover(|st| st.bg(c(0x24301C, 1.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.rts_goto(&go, cx)))
                    .child(div().flex_none().w(px(5.)).h_full().bg(c(color, 1.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .gap(px(10.))
                                    .child(
                                        cmd::data(cmd::up(&p.title), 20.)
                                            .min_w(px(0.))
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_color(c(cmd::WHITE, 1.)),
                                    )
                                    .child(
                                        cmd::data(cmd::mins(war.now, p.since), 18.)
                                            .flex_none()
                                            .text_color(c(color, 1.)),
                                    ),
                            )
                            .child(
                                cmd::label(format!("{} · {}", p.codebase, what))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis(),
                            ),
                    ),
            );
        }
        Some(
            div()
                .id("rts-log")
                .absolute()
                .left(px(12.))
                .top(px(12.))
                .w(px(420.))
                .flex()
                .flex_col()
                .bg(c(0x1E2124, 0.97))
                .border_2()
                .border_color(c(0x6A7076, 1.))
                .occlude()
                .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(
                    div()
                        .id("rts-log-head")
                        .flex_none()
                        .h(px(34.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .bg(c(0x353A3F, 1.))
                        .cursor_pointer()
                        .hover(|st| st.bg(c(0x454B50, 1.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.rts.log_small = true;
                            cx.notify();
                        }))
                        .child(cmd::data("NEEDS YOU", 20.).text_color(c(cmd::WHITE, 1.)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(14.))
                                .child(
                                    cmd::data(waits.len().to_string(), 20.)
                                        .text_color(c(cmd::AMBER, 1.)),
                                )
                                .child(cmd::data("-", 24.).text_color(c(cmd::WHITE, 1.))),
                        ),
                )
                .child(list),
        )
    }

    /// The find box.
    fn rts_find(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let text = self.rts.find.clone()?;
        let hits = self.rts_matches(&text);
        let mut body = div().p(px(10.)).flex().flex_col().gap(px(4.)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(cmd::data(format!("FIND · {text}_"), 24.).text_color(c(cmd::WHITE, 1.)))
                .child(
                    cmd::button("rts-find-x", "X", Btn::Plain).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.rts.find = None;
                            cx.notify();
                        },
                    )),
                ),
        );
        if hits.is_empty() && !text.trim().is_empty() {
            body = body.child(cmd::label("NOTHING FOUND"));
        }
        for (k, (name, id)) in hits.into_iter().enumerate() {
            let kind = if id.is_some() { "ROBOT" } else { "VILLAGE" };
            let shown = cmd::up(&name);
            body = body.child(
                div()
                    .id(("rts-hit", k))
                    .flex()
                    .justify_between()
                    .gap(px(12.))
                    .cursor_pointer()
                    .hover(|st| st.bg(c(cmd::ROW_HOVER, 1.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rts.find = None;
                        match &id {
                            Some(id) => this.rts_goto(id, cx),
                            None => this.rts_village(&name, cx),
                        }
                    }))
                    .child(
                        cmd::entry(shown)
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis(),
                    )
                    .child(cmd::label(kind).flex_none()),
            );
        }
        Some(
            div()
                .absolute()
                .top(px(120.))
                .left(px(((self.rts.view.0 - 420.) / 2.).floor()))
                .w(px(420.))
                .bg(c(0x1E2124, 0.97))
                .border_2()
                .border_color(c(0x6A7076, 1.))
                .child(body),
        )
    }

    /// The workshop window of the selected session: its work, its last messages, its buttons.
    fn rts_shop(&mut self, war: &WarRoom, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let id = self.rts.selected.clone()?;
        let p = war.session(&id)?.clone();
        let tool = Tool::of(&p.tail);
        let work = match p.state {
            SessionState::Working => tool.word().to_string(),
            SessionState::NeedsYou => "WAITS FOR YOUR ANSWER".to_string(),
            SessionState::Error => "STOPPED ON AN ERROR".to_string(),
            SessionState::Idle => "DOZES AT THE MAIN BUILDING".to_string(),
            SessionState::Parked => "PARKED · A FINISHED COTTAGE".to_string(),
            SessionState::Done => "DONE · A FINISHED COTTAGE".to_string(),
        };
        let left = div()
            .w(px(250.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                cmd::data(cmd::up(&p.title), 24.)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(c(cmd::WHITE, 1.)),
            )
            .child(cmd::label(format!("{} · {}", p.codebase, p.provider)))
            .child(cmd::data(work, 18.).text_color(c(state_color(p.state), 1.)))
            .child(cmd::label(format!(
                "{} · CONTEXT {}%",
                cmd::mins(war.now, p.since),
                p.context_pct
            )))
            .child(cmd::label(format!("{} TOKENS", cmd::tokens(p.tokens))));
        let mut mid = div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(10.))
            .py(px(6.))
            .bg(c(cmd::WELL, 1.))
            .overflow_hidden();
        let room = if p.ask.is_some() { 4 } else { 6 };
        for l in last_lines(&p.tail, room) {
            let (who, col) = line_head(&l, &p.provider);
            mid = mid.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(cmd::data(who, 18.).flex_none().text_color(c(col, 1.)))
                    .child(
                        cmd::data(l.text.lines().next().unwrap_or("").to_string(), 18.)
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis(),
                    ),
            );
        }
        let mut right = div().w(px(300.)).flex_none().flex().flex_col().gap(px(6.));
        if let Some(a) = &p.ask {
            let n = a.options.len();
            let mut row = div().flex().flex_wrap().gap(px(6.));
            for (k, o) in a.options.iter().take(3).enumerate() {
                let kind = if k == 0 {
                    Btn::Go
                } else if k + 1 == n && n > 1 {
                    Btn::Quiet
                } else {
                    Btn::Plain
                };
                row = row.child(
                    cmd::button(("rts-ans", k), format!("{} {}", k + 1, cmd::up(o)), kind)
                        .on_click(cx.listener(move |this, _, _, cx| this.rts_answer(k, cx))),
                );
            }
            right = right.child(row);
        }
        let mut row = div().mt_auto().flex().flex_wrap().gap(px(6.));
        let open = id.clone();
        row = row.child(cmd::button("rts-open", "O OPEN", Btn::Plain).on_click(
            cx.listener(move |_, _, _, cx| cx.emit(WarRoomEvent::OpenSession(open.clone()))),
        ));
        let (a, b) = (id.clone(), id.clone());
        if p.controlled && p.state.is_live() {
            row = row
                .child(cmd::button("rts-park", "PARK", Btn::Plain).on_click(
                    cx.listener(move |this, _, _, cx| this.act(Action::Park(a.clone()), cx)),
                ))
                .child(cmd::button("rts-stop", "STOP", Btn::Quiet).on_click(
                    cx.listener(move |this, _, _, cx| this.act(Action::Stop(b.clone()), cx)),
                ));
        } else if p.state == SessionState::Parked {
            row = row.child(cmd::button("rts-resume", "RESUME", Btn::Go).on_click(
                cx.listener(move |this, _, _, cx| this.act(Action::Resume(a.clone()), cx)),
            ));
        }
        right = right.child(row);
        Some(
            div()
                .id("rts-shop")
                .absolute()
                .left(px(12.))
                .right(px(12.))
                .bottom(px(12.))
                .h(px(SHOP_H))
                .p(px(10.))
                .flex()
                .gap(px(12.))
                .bg(c(0x1E2124, 0.97))
                .border_2()
                .border_color(c(0x6A7076, 1.))
                .occlude()
                .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(left)
                .child(mid)
                .child(right)
                .child(
                    div()
                        .flex_none()
                        .child(cmd::button("rts-x", "X", Btn::Plain).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.rts.selected = None;
                                cx.notify();
                            },
                        ))),
                ),
        )
    }
}

/// The sessions that wait: open asks first, then errors.
pub fn rts_waits(war: &WarRoom) -> Vec<String> {
    let mut out = war.board.asks.clone();
    for id in &war.board.blocked.ids {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

fn state_color(s: SessionState) -> u32 {
    match s {
        SessionState::NeedsYou => cmd::AMBER,
        SessionState::Error => cmd::RED,
        SessionState::Working => cmd::LAMP_RUN,
        _ => cmd::INK_3,
    }
}

/// The last lines that a person reads (no meta lines and no output).
pub fn last_lines(tail: &[Line], n: usize) -> Vec<Line> {
    let mut out: Vec<Line> = tail
        .iter()
        .rev()
        .filter(|l| matches!(l.kind, LineKind::User | LineKind::Agent | LineKind::Tool))
        .take(n)
        .cloned()
        .collect();
    out.reverse();
    out
}

/// The head of a message line and its color.
pub fn line_head(l: &Line, provider: &str) -> (String, u32) {
    match l.kind {
        LineKind::User => ("YOU".into(), cmd::AMBER),
        LineKind::Tool => (cmd::up(l.label.as_deref().unwrap_or("TOOL")), cmd::INK_3),
        _ => (cmd::up(provider), cmd::GREEN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_follows_the_tokens() {
        let r = Rate {
            tokens: 1000,
            at: 0,
            per_min: 0.,
        };
        let n = next_rate(r, 4000, 60_000);
        assert!((n.per_min - 1200.).abs() < 0.1);
        assert_eq!(next_rate(r, 9000, 10).tokens, 1000);
        assert!(speed(0.) < speed(3000.));
        assert!(speed(1e9) <= 2.4);
    }

    #[test]
    fn space_walks_through_the_waits() {
        let w = vec!["a".to_string(), "b".to_string()];
        assert_eq!(next_wait(&w, None), Some(&w[0]));
        assert_eq!(next_wait(&w, Some("a")), Some(&w[1]));
        assert_eq!(next_wait(&w, Some("b")), Some(&w[0]));
        assert_eq!(next_wait(&w, Some("x")), Some(&w[0]));
        assert_eq!(next_wait(&[], None), None);
    }

    #[test]
    fn the_last_lines_skip_output() {
        let tail = vec![
            Line::new(LineKind::Meta, "m"),
            Line::new(LineKind::User, "u"),
            Line::new(LineKind::Output, "o"),
            Line::new(LineKind::Agent, "a"),
        ];
        let l = last_lines(&tail, 6);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].text, "a");
    }
}
