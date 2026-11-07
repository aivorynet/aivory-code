//! The Tactical look, war room (board `TacticalRadar`): a phosphor-green radar. The bearing
//! sector is the codebase. The range from the center is the time since the session last moved
//! (log rings 1, 5, 15 and 60 minutes), so an ignored waiting session drifts outward. The sweep
//! turns in 6 s and it is the only free-running motion: a blip is brightest when the sweep
//! passes it. White lock brackets mark a session that awaits orders. The left ruler shows the
//! tokens per minute of each session. Green is the only color; white means "needs you".
//!
//! The right column holds the tracking panel (the order request, the buttons), the mission
//! route, the supply bars and the comms log. The dossier of one session is in `session.rs`.

mod session;

use std::f32::consts::TAU;
use std::time::Instant;

use aivory_protocol::{AskKind, LineKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, MouseButton, MouseDownEvent, Window, canvas, div, px};

use super::common::short_tool;
use super::common::{Align, Painter, asks, key_label, next_ask};
use super::hud::{Contact, Hit, PaintFn, Rect, contacts};
use crate::actions::Action;
use crate::tactical_kit::{self as tac, Btn, c, dots, label, section, tbox};
use crate::ui;
use crate::warroom::WarRoomScreen;

/// The seconds of one turn of the sweep.
const SWEEP_S: f32 = 6.;

/// The ring radius (0 to 1) of a number of minutes. The rings 1, 5, 15 and 60 minutes sit at
/// 1/4, 2/4, 3/4 and 4/4 of the radius; between the rings the scale is logarithmic.
pub(super) fn range(minutes: f32) -> f32 {
    let m = minutes.max(0.);
    if m < 1. {
        0.06 + 0.19 * m
    } else if m < 5. {
        0.25 + 0.25 * m.ln() / 5f32.ln()
    } else if m < 15. {
        0.5 + 0.25 * (m / 5.).ln() / 3f32.ln()
    } else if m < 60. {
        0.75 + 0.25 * (m / 15.).ln() / 4f32.ln()
    } else {
        1.
    }
}

/// Where a contact sits: the bearing in degrees clockwise from the top, and the range 0 to 1.
#[derive(Debug, Clone, Copy)]
pub(super) struct Spot {
    pub deg: f32,
    pub range: f32,
}

/// The codebases in a stable order.
pub(super) fn sorted_books(list: &[Contact]) -> Vec<String> {
    let mut books: Vec<String> = Vec::new();
    for s in list {
        if !books.contains(&s.codebase) {
            books.push(s.codebase.clone());
        }
    }
    books.sort();
    books
}

/// The spot of each contact, in the order of `list`. The sector is the codebase. Inside a sector
/// the contacts keep the order of their session ids, so a blip moves only when its range changes.
pub(super) fn layout(list: &[Contact], books: &[String]) -> Vec<Spot> {
    let mut out = vec![Spot { deg: 0., range: 0. }; list.len()];
    let span = 360. / books.len().max(1) as f32;
    let margin = (span * 0.12).min(8.);
    for (bi, b) in books.iter().enumerate() {
        let mut members: Vec<usize> = (0..list.len())
            .filter(|&i| &list[i].codebase == b)
            .collect();
        members.sort_by(|&a, &z| list[a].id.cmp(&list[z].id));
        let n = members.len() as f32;
        for (k, i) in members.into_iter().enumerate() {
            out[i] = Spot {
                deg: bi as f32 * span + margin + (k as f32 + 0.5) / n * (span - 2. * margin),
                range: range(list[i].minutes),
            };
        }
    }
    out
}

/// The point at a bearing (degrees from the top, clockwise) and a distance.
fn at(center: [f32; 2], deg: f32, dist: f32) -> [f32; 2] {
    let a = deg.to_radians();
    [center[0] + a.sin() * dist, center[1] - a.cos() * dist]
}

/// Text with the vertical center at `y`. The size is in px at 100 %.
#[allow(clippy::too_many_arguments)]
fn txt(
    p: &mut Painter,
    s: &str,
    size: f32,
    bold: bool,
    rgb: u32,
    alpha: f32,
    (x, y): (f32, f32),
    align: Align,
) -> f32 {
    let sz = Painter::scaled(size);
    p.text(
        s,
        sz,
        sz * 1.3,
        tac::MONO,
        bold,
        rgb,
        alpha,
        (x, y - sz * 0.65),
        align,
    )
}

/// The width of a text in px.
fn width_of(p: &mut Painter, s: &str, size: f32, bold: bool) -> f32 {
    f32::from(
        p.shaped(s, Painter::scaled(size), tac::MONO, bold, 0, 1.)
            .width(),
    )
}

/// A dashed segment.
fn dashed(a: [f32; 2], b: [f32; 2], on: f32, off: f32) -> Vec<[f32; 4]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt().max(0.01);
    let (ux, uy) = (dx / len, dy / len);
    let mut segs = Vec::new();
    let mut d = 0.;
    while d < len {
        let e = (d + on).min(len);
        segs.push([a[0] + ux * d, a[1] + uy * d, a[0] + ux * e, a[1] + uy * e]);
        d += on + off;
    }
    segs
}

/// The brackets of a lock around a point.
fn lock(p: &mut Painter, at: [f32; 2], half: f32, rgb: u32, alpha: f32) {
    let q = half * 0.5;
    let mut segs = Vec::new();
    for (sx, sy) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
        let (x, y) = (at[0] + sx * half, at[1] + sy * half);
        segs.push([x, y, x - sx * q, y]);
        segs.push([x, y, x, y - sy * q]);
    }
    p.strokes(&segs, rgb, 2., alpha);
}

/// A rectangle outline.
fn frame_lines(p: &mut Painter, (x, y, w, h): (f32, f32, f32, f32), rgb: u32, alpha: f32) {
    p.strokes(
        &[
            [x, y, x + w, y],
            [x + w, y, x + w, y + h],
            [x + w, y + h, x, y + h],
            [x, y + h, x, y],
        ],
        rgb,
        1.,
        alpha,
    );
}

/// The labels of the ruler: 0 and five steps up to `top` tokens per minute.
fn ruler_top(rate: f32) -> f32 {
    [1000., 2000., 5000., 10_000., 20_000., 50_000., 100_000.]
        .into_iter()
        .find(|&t| rate <= t)
        .unwrap_or(200_000.)
}

fn k_text(v: f32) -> String {
    if v < 1. {
        "0".to_string()
    } else if (v / 1000.).fract() == 0. {
        format!("{}K", (v / 1000.) as u32)
    } else {
        format!("{:.1}K", v / 1000.)
    }
}

/// The name of a contact on the scope: `CODEBASE/TITLE`.
fn call_sign(s: &Contact) -> String {
    format!("{}/{}", s.codebase, s.title).to_uppercase()
}

/// Everything the radar paints.
struct Scope {
    list: Vec<Contact>,
    books: Vec<String>,
    spots: Vec<Spot>,
    pick: Option<String>,
    /// The sessions that get a name chip at the lock (the picked one and the first in line).
    chips: Vec<String>,
    t: f32,
}

fn radar(scope: Scope) -> PaintFn {
    Box::new(move |p, r: Rect, hits| {
        let Scope {
            list,
            books,
            spots,
            pick,
            chips,
            t,
        } = &scope;
        let (ox, oy, w, h) = (r[0], r[1], r[2], r[3]);
        let rcol = w - 20. - 452.;
        let rad = (((h - 40.) / 2. - 34.).min((rcol - 168.) / 2. - 24.)).clamp(110., 370.);
        let center = [ox + 160. + (rcol - 168.) / 2., oy + h / 2. - 10.];

        // ---- the ruler: tokens per minute --------------------------------------------------
        let (ax, ry) = (ox + 82., oy + 32.);
        let rh = (h - 32. - 54.).max(300.);
        txt(
            p,
            "TOKENS / MIN",
            12.,
            false,
            tac::LABEL,
            1.,
            (ox + 22., oy + 14.),
            Align::Left,
        );
        let top = ruler_top(list.iter().map(|s| s.rate).fold(0., f32::max));
        let y_of = |v: f32| ry + rh - (v / top).clamp(0., 1.) * rh;
        p.strokes(&[[ax, ry, ax, ry + rh]], tac::MID, 1.5, 1.);
        let mut ticks = Vec::new();
        for k in 0..=50 {
            let y = ry + rh - k as f32 / 50. * rh;
            let len = if k % 10 == 0 {
                22.
            } else if k % 5 == 0 {
                14.
            } else {
                8.
            };
            ticks.push([ax - len, y, ax, y]);
        }
        p.strokes(&ticks, tac::DIM, 1., 1.);
        for i in 0..=5 {
            let v = top * i as f32 / 5.;
            txt(
                p,
                &k_text(v),
                12.,
                false,
                tac::LABEL,
                1.,
                (ax - 28., y_of(v)),
                Align::Right,
            );
        }
        let mut by_rate: Vec<&Contact> = list.iter().filter(|s| s.rate >= 0.5).collect();
        by_rate.sort_by(|a, b| b.rate.total_cmp(&a.rate).then(a.id.cmp(&b.id)));
        let mut last = -99.;
        for s in by_rate.into_iter().take(((rh / 18.) as usize).min(14)) {
            let mut y = y_of(s.rate);
            if y - last < 18. {
                y = last + 18.;
            }
            last = y;
            if y > ry + rh + 8. {
                break;
            }
            let color = if s.waits() { tac::WHITE } else { tac::INK };
            let picked = pick.as_deref() == Some(s.id.as_str());
            p.polygon(
                &[[ax, y], [ax + 9., y - 5.], [ax + 9., y + 5.]],
                color,
                if picked { 1. } else { 0.75 },
            );
            let name = dots(&s.title.to_uppercase(), 12);
            let wd = txt(
                p,
                &name,
                12.,
                picked,
                color,
                if picked { 1. } else { 0.75 },
                (ax + 15., y),
                Align::Left,
            );
            hits.push((Hit::Pick(s.id.clone()), [ax, y - 9., wd + 20., 18.]));
        }

        // ---- the scope ---------------------------------------------------------------------
        p.dot(center, rad, 0x010C03, 1.);
        let mut bearings = Vec::new();
        for d in (0..360).step_by(30) {
            let to = at(center, d as f32, rad);
            bearings.push([center[0], center[1], to[0], to[1]]);
        }
        p.strokes(&bearings, tac::DEEP, 1., 1.);
        p.strokes(
            &[
                [center[0] - rad, center[1], center[0] + rad, center[1]],
                [center[0], center[1] - rad, center[0], center[1] + rad],
            ],
            tac::DIM,
            1.,
            1.,
        );
        let id = |q: [f32; 2]| q;
        for f in [0.25, 0.5, 0.75] {
            p.arc(center, rad * f, (0., TAU), 1., tac::DIM, 1., &id);
        }
        p.arc(center, rad, (0., TAU), 6., tac::INK, 0.12, &id);
        p.arc(center, rad, (0., TAU), 2., tac::INK, 1., &id);
        for (f, name) in [(0.25, "1M"), (0.5, "5M"), (0.75, "15M"), (1., "60M")] {
            txt(
                p,
                name,
                12.,
                false,
                tac::DIM,
                1.,
                (center[0] + 8., center[1] - rad * f + 10.),
                Align::Left,
            );
        }
        let mut rim = Vec::new();
        for d in (0..360).step_by(5) {
            let long = d % 30 == 0;
            let a = at(center, d as f32, if long { rad - 18. } else { rad - 8. });
            let b = at(center, d as f32, rad);
            rim.push([a[0], a[1], b[0], b[1]]);
        }
        p.strokes(&rim, tac::MID, 1., 1.);
        let nb = books.len().max(1);
        let span = 360. / nb as f32;
        let mut sectors = Vec::new();
        for bi in 0..nb {
            let to = at(center, bi as f32 * span, rad);
            sectors.extend(dashed(center, to, 6., 6.));
        }
        p.strokes(&sectors, tac::MID, 1.5, 0.8);
        p.strokes(
            &[
                [center[0] - 10., center[1], center[0] + 10., center[1]],
                [center[0], center[1] - 10., center[0], center[1] + 10.],
            ],
            tac::INK,
            1.5,
            1.,
        );

        // ---- the sweep ---------------------------------------------------------------------
        let turn = (t / SWEEP_S).fract();
        let head = turn * 360.;
        let slices = 28;
        let width = 36.;
        for k in 0..slices {
            let d0 = head - k as f32 * width / slices as f32;
            let d1 = head - (k + 1) as f32 * width / slices as f32;
            let (a, b) = (at(center, d0, rad), at(center, d1, rad));
            let fade = 1. - k as f32 / slices as f32;
            p.polygon(&[center, a, b], tac::INK, 0.34 * fade.powf(1.6));
        }
        let tip = at(center, head, rad);
        p.strokes(&[[center[0], center[1], tip[0], tip[1]]], 0xBEFFCD, 2., 0.9);

        let mut tag_rects: Vec<[f32; 4]> = Vec::new();
        // ---- the sector tags ---------------------------------------------------------------
        for (bi, b) in books.iter().enumerate() {
            let mid = (bi as f32 + 0.5) * span;
            let q = at(center, mid, rad + 2.);
            let members = list.iter().filter(|s| &s.codebase == b).count();
            let waits = list.iter().any(|s| &s.codebase == b && s.waits());
            let name = dots(&b.to_uppercase(), 24);
            let count = members.to_string();
            let (wn, wc) = (
                width_of(p, &name, 12., false),
                width_of(p, &count, 12., false),
            );
            let bw = wn + wc + 8. + 20.;
            let (bx, by) = (q[0] - bw / 2., q[1] - 12.);
            tag_rects.push([bx, by, bw, 24.]);
            p.rect((bx, by, bw, 24.), 0x000700, 1.);
            frame_lines(p, (bx, by, bw, 24.), tac::DIM, 1.);
            txt(
                p,
                &name,
                12.,
                false,
                if waits { tac::WHITE } else { tac::INK },
                1.,
                (bx + 10., q[1]),
                Align::Left,
            );
            txt(
                p,
                &count,
                12.,
                false,
                tac::LABEL,
                1.,
                (bx + 18. + wn, q[1]),
                Align::Left,
            );
            if let Some(s) = list
                .iter()
                .find(|s| &s.codebase == b && s.waits())
                .or_else(|| list.iter().find(|s| &s.codebase == b))
            {
                hits.push((Hit::Pick(s.id.clone()), [bx, by, bw, 24.]));
            }
        }

        // ---- the blips ---------------------------------------------------------------------
        let mut placed_labels: Vec<[f32; 4]> = tag_rects.clone();
        let mut order: Vec<usize> = (0..list.len()).collect();
        order.sort_by_key(|&i| (!list[i].waits(), list[i].id.clone()));
        for i in order {
            let (s, spot) = (&list[i], spots[i]);
            let q = at(center, spot.deg, rad * spot.range);
            // The blip is brightest when the sweep passes and fades until the next turn.
            let behind = ((head - spot.deg).rem_euclid(360.)) / 360.;
            let glow = 1. - behind;
            let picked = pick.as_deref() == Some(s.id.as_str());
            if s.waits() {
                p.dot(q, 9., tac::WHITE, 0.2 + 0.2 * glow);
                p.dot(q, 4., tac::WHITE, 1.);
                lock(p, q, 17., tac::WHITE, 0.65 + 0.35 * glow);
                if chips.contains(&s.id) {
                    let name = call_sign(s);
                    let wait = format!(
                        "AWAITING ORDERS {}",
                        tac::clock_ms((s.minutes * 60_000.) as i64)
                    );
                    let wn = width_of(p, &name, 12., true) + 16.;
                    let ww = width_of(p, &wait, 12., false);
                    let right = q[0] < center[0] + rad * 0.54;
                    let x0 = if right { q[0] - 17. } else { q[0] + 17. - wn };
                    let xw = if right { q[0] - 17. } else { q[0] + 17. - ww };
                    p.rect((x0, q[1] + 22., wn, 22.), tac::INK, 1.);
                    txt(
                        p,
                        &name,
                        12.,
                        true,
                        tac::DARK,
                        1.,
                        (x0 + 8., q[1] + 33.),
                        Align::Left,
                    );
                    txt(
                        p,
                        &wait,
                        12.,
                        false,
                        tac::WHITE,
                        1.,
                        (xw, q[1] + 56.),
                        Align::Left,
                    );
                    placed_labels.push([x0, q[1] + 22., wn.max(ww), 40.]);
                }
            } else {
                let (color, text) = if s.state == SessionState::Working {
                    (tac::INK, tac::INK)
                } else {
                    (tac::DIM, tac::DIM)
                };
                p.dot(q, 9., color, 0.22 * glow);
                p.dot(q, 4., color, 0.3 + 0.7 * glow);
                // A name only when it fits: fewer labels rather than crowded ones.
                // The whole name where it fits: right of the blip, else left, else a line above
                // or below. A short cut only when no place is free.
                let full = s.title.to_uppercase();
                let wf = width_of(p, &full, 12., false);
                let edge = ox + rcol - 8.;
                let free_at = |r: [f32; 4], placed: &[[f32; 4]]| {
                    r[0] >= ox + 150.
                        && r[0] + r[2] <= edge
                        && placed.iter().all(|o| {
                            r[0] > o[0] + o[2] + 4.
                                || o[0] > r[0] + r[2] + 4.
                                || r[1] > o[1] + o[3] + 2.
                                || o[1] > r[1] + r[3] + 2.
                        })
                };
                let spots_try = [
                    [q[0] + 9., q[1] - 9., wf, 18.],
                    [q[0] - 9. - wf, q[1] - 9., wf, 18.],
                    [q[0] + 9., q[1] - 24., wf, 18.],
                    [q[0] + 9., q[1] + 6., wf, 18.],
                    [q[0] - 9. - wf, q[1] - 24., wf, 18.],
                    [q[0] - 9. - wf, q[1] + 6., wf, 18.],
                ];
                let mut chosen = spots_try
                    .into_iter()
                    .find(|r| free_at(*r, &placed_labels))
                    .map(|r| (full.clone(), r));
                // No free place: cut the name at a word border until it fits.
                let mut words: Vec<&str> = full.split_whitespace().collect();
                while chosen.is_none() && words.len() > 1 {
                    words.pop();
                    let short = words.join(" ");
                    let ws = width_of(p, &short, 12., false);
                    for r in [
                        [q[0] + 9., q[1] - 9., ws, 18.],
                        [q[0] - 9. - ws, q[1] - 9., ws, 18.],
                    ] {
                        if chosen.is_none() && free_at(r, &placed_labels) {
                            chosen = Some((short.clone(), r));
                        }
                    }
                }
                if let Some((name, rect)) = chosen {
                    txt(
                        p,
                        &name,
                        12.,
                        false,
                        text,
                        0.55 + 0.45 * glow,
                        (rect[0], rect[1] + 9.),
                        Align::Left,
                    );
                    placed_labels.push(rect);
                }
            }
            if picked {
                let color = if s.waits() { tac::WHITE } else { tac::INK };
                p.arc(q, 24., (0., TAU), 1., color, 0.8, &id);
            }
            hits.push((Hit::Pick(s.id.clone()), [q[0] - 16., q[1] - 16., 32., 32.]));
        }
        if list.is_empty() {
            txt(
                p,
                "NO CONTACTS",
                14.,
                false,
                tac::DIM,
                1.,
                (center[0], center[1] + 40.),
                Align::Center,
            );
        }

        // ---- the count and the legend ------------------------------------------------------
        let (rx, ry2) = (center[0] + rad * 0.72, center[1] - rad - 16.);
        let num = list.len().to_string();
        let nw = width_of(p, &num, 28., true);
        let bw = nw + 24. + 92.;
        let bx = (rx.min(ox + rcol - bw - 32.), ry2.max(oy + 8.), bw, 44.);
        frame_lines(p, bx, tac::INK, 1.);
        txt(
            p,
            &num,
            28.,
            true,
            tac::INK,
            1.,
            (bx.0 + 12., bx.1 + 22.),
            Align::Left,
        );
        p.strokes(
            &[[bx.0 + nw + 24., bx.1, bx.0 + nw + 24., bx.1 + 44.]],
            tac::INK,
            1.,
            1.,
        );
        txt(
            p,
            "LIVE",
            12.,
            false,
            tac::INK,
            1.,
            (bx.0 + nw + 36., bx.1 + 14.),
            Align::Left,
        );
        txt(
            p,
            "CONTACTS",
            12.,
            false,
            tac::INK,
            1.,
            (bx.0 + nw + 36., bx.1 + 31.),
            Align::Left,
        );
        txt(
            p,
            "RANGE = TIME SINCE LAST MOVE  /  BEARING = CODEBASE  /  SWEEP 6 S",
            12.,
            false,
            tac::LABEL,
            1.,
            (ox + 196., center[1] + rad + 24.),
            Align::Left,
        );
    })
}

/// The bearing of the contact `id` in degrees.
fn bearing(list: &[Contact], spots: &[Spot], id: &str) -> Option<f32> {
    list.iter().position(|c| c.id == id).map(|i| spots[i].deg)
}

// ---- the route of a mission --------------------------------------------------------------

/// The state of a waypoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Wp {
    Done,
    Now,
    Hold,
    Todo,
}

/// The waypoints of a mission: the latest tool calls as done waypoints, then the plan steps or
/// the order that waits (hold), or the call that runs now.
pub(super) fn route_steps(pane: &Pane) -> Vec<(String, Wp)> {
    let mut tools: Vec<String> = pane
        .tail
        .iter()
        .filter(|l| l.kind == LineKind::Tool)
        .map(|l| {
            let name = l.label.clone().unwrap_or_else(|| {
                short_tool("", &l.text)
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string()
            });
            dots(&name.to_uppercase(), 10)
        })
        .collect();
    let keep = if pane.ask.is_some() { 4 } else { 5 };
    let skip = tools.len().saturating_sub(keep);
    let tools = tools.split_off(skip);
    let mut out: Vec<(String, Wp)> = vec![("START".to_string(), Wp::Done)];
    out.extend(tools.into_iter().map(|n| (n, Wp::Done)));
    match pane.ask.as_ref() {
        Some(a) if !a.steps.is_empty() => {
            for (i, s) in a.steps.iter().take(3).enumerate() {
                let name = dots(&s.to_uppercase(), 10);
                out.push((name, if i == 0 { Wp::Hold } else { Wp::Todo }));
            }
        }
        Some(a) => {
            let name = match a.kind {
                AskKind::Permission => "PERMIT",
                AskKind::Plan => "PLAN",
                AskKind::Question => "ANSWER",
                AskKind::Error => "ERROR",
            };
            out.push((name.to_string(), Wp::Hold));
        }
        None => {
            if pane.state == SessionState::Working
                && let Some(last) = out.last_mut()
            {
                last.1 = Wp::Now;
            }
        }
    }
    out.push(("DONE".to_string(), Wp::Todo));
    out
}

/// The picture of a route: a grid, the legs (solid up to the waypoint that holds), the
/// waypoints (filled = done, bracket = now or hold, open square = to do) and their names.
pub(super) fn route_paint(steps: Vec<(String, Wp)>) -> PaintFn {
    Box::new(move |p, r: Rect, _| {
        let (ox, oy, w, h) = (r[0], r[1], r[2], r[3]);
        let mut grid = Vec::new();
        for k in 1..4 {
            let y = oy + h * k as f32 / 4.;
            grid.push([ox, y, ox + w, y]);
        }
        for k in 1..6 {
            let x = ox + w * k as f32 / 6.;
            grid.push([x, oy, x, oy + h]);
        }
        p.strokes(&grid, 0x06280F, 1., 1.);
        let n = steps.len();
        if n == 0 {
            return;
        }
        let pad = 48.;
        let pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let x = if n == 1 {
                    ox + w / 2.
                } else {
                    ox + pad + i as f32 * (w - 2. * pad) / (n - 1) as f32
                };
                [x, oy + h * (0.5 - 0.17 * (i as f32 * 1.1).sin())]
            })
            .collect();
        let hold = steps
            .iter()
            .position(|s| matches!(s.1, Wp::Now | Wp::Hold))
            .unwrap_or(n - 1);
        for i in 1..n {
            let (a, b) = (pts[i - 1], pts[i]);
            if i <= hold {
                p.strokes(&[[a[0], a[1], b[0], b[1]]], tac::INK, 6., 0.12);
                p.strokes(&[[a[0], a[1], b[0], b[1]]], tac::INK, 2., 1.);
            } else {
                p.strokes(&dashed(a, b, 5., 6.), tac::MID, 1.5, 1.);
            }
        }
        for (i, ((name, state), q)) in steps.iter().zip(pts.iter()).enumerate() {
            let up = i % 2 == 1;
            let (color, bold) = match state {
                Wp::Done => (tac::INK, false),
                Wp::Now => (tac::INK, true),
                Wp::Hold => (tac::WHITE, true),
                Wp::Todo => (tac::LABEL, false),
            };
            match state {
                Wp::Done => p.dot(*q, 5., tac::INK, 1.),
                Wp::Now => {
                    p.dot(*q, 5., tac::INK, 1.);
                    lock(p, *q, 14., tac::INK, 1.);
                }
                Wp::Hold => {
                    p.dot(*q, 9., tac::WHITE, 0.25);
                    p.dot(*q, 5., tac::WHITE, 1.);
                    lock(p, *q, 14., tac::WHITE, 1.);
                }
                Wp::Todo => frame_lines(p, (q[0] - 7., q[1] - 7., 14., 14.), tac::MID, 1.),
            }
            let tag = match state {
                Wp::Hold => " · HOLD",
                Wp::Now => " · NOW",
                _ => "",
            };
            let text = format!("WP{} {}{}", i + 1, name, tag);
            txt(
                p,
                &text,
                12.,
                bold,
                color,
                1.,
                (q[0], q[1] + if up { -26. } else { 26. }),
                Align::Center,
            );
        }
    })
}

// ---- the screen ----------------------------------------------------------------------------

/// A label with the clock of a daemon time.
fn log_line(time: i64, text: String, color: u32) -> Div {
    div()
        .flex_none()
        .flex()
        .gap(px(12.))
        .h(px(22.))
        .items_center()
        .overflow_hidden()
        .font_family(tac::MONO)
        .text_size(ui::ts(tac::S_LABEL))
        .child(
            div()
                .flex_none()
                .text_color(c(tac::DIM, 1.))
                .child(tac::utc(time)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(c(color, 1.))
                .child(text),
        )
}

/// `2.9M`, `410K`, `850`.
fn compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{}K", n / 1000)
    } else {
        n.to_string()
    }
}

/// The order that a state of a session reports in the comms log.
fn comms_text(s: &Pane) -> (String, u32) {
    let name = format!("{}/{}", s.codebase, s.title).to_uppercase();
    match (&s.state, s.ask.as_ref()) {
        (SessionState::NeedsYou | SessionState::Error, Some(a)) => (
            format!("{name}  REQ ORDER: {}", dots(&a.text.to_uppercase(), 60)),
            tac::WHITE,
        ),
        (SessionState::NeedsYou | SessionState::Error, None) => {
            (format!("{name}  AWAITS ORDERS"), tac::WHITE)
        }
        (SessionState::Working, _) => (format!("{name}  WORKING"), tac::INK),
        _ => (format!("{name}  NO MOVEMENT"), tac::LABEL),
    }
}

impl WarRoomScreen {
    /// The war room (and the dossier of a session) in the Tactical look.
    pub(super) fn tactical_render(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let st = &mut self.tactical;
        let t = (now - *st.started.get_or_insert(now)).as_secs_f32();
        st.observe(war, now);
        if st.open.as_ref().is_some_and(|id| war.session(id).is_none()) {
            st.open = None;
        }
        let open = st.open.clone();
        crate::ui::animate(window, cx);
        match open {
            Some(id) => self.tactical_dossier(war, &id, window, cx),
            None => self.tactical_overview(war, t, cx),
        }
    }

    fn tactical_overview(&mut self, war: &WarRoom, t: f32, cx: &mut Context<Self>) -> AnyElement {
        let (mut list, _) = contacts(&self.tactical, war);
        // The contact title is cut at 30 characters with an ellipsis; use the whole title.
        for ct in &mut list {
            if let Some(s) = war.session(&ct.id)
                && !s.title.is_empty()
            {
                ct.title = s.title.clone();
            }
        }
        let books = sorted_books(&list);
        let spots = layout(&list, &books);
        let queue = asks(war);
        let pick_id = self
            .tactical
            .pick
            .clone()
            .filter(|id| queue.iter().any(|p| &p.id == id))
            .or_else(|| queue.first().map(|p| p.id.clone()));
        let pick = pick_id
            .as_ref()
            .and_then(|id| queue.iter().find(|p| &p.id == id).copied());
        let next = next_ask(&queue, pick_id.as_deref());
        let at_index = pick_id
            .as_ref()
            .and_then(|id| queue.iter().position(|p| &p.id == id));
        let prev = at_index
            .filter(|_| !queue.is_empty())
            .map(|i| queue[(i + queue.len() - 1) % queue.len()].id.clone());
        let brg = pick_id.as_deref().and_then(|id| bearing(&list, &spots, id));

        let mut chips: Vec<String> = queue.iter().take(3).map(|p| p.id.clone()).collect();
        if let Some(id) = &pick_id {
            chips.push(id.clone());
        }
        let paint = radar(Scope {
            list,
            books,
            spots,
            pick: pick_id.clone(),
            chips,
            t,
        });
        let (cache, hits) = (self.tactical.cache.clone(), self.tactical.hits.clone());
        let picture = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let rect = [
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                ];
                let mut cache = cache.borrow_mut();
                let mut hits = hits.borrow_mut();
                hits.clear();
                let mut p = Painter::new(window, cx, &mut cache);
                paint(&mut p, rect, &mut hits);
            },
        )
        .absolute()
        .inset_0();

        let tracking = Self::tactical_tracking(
            pick,
            brg,
            war.now,
            (at_index.map_or(0, |i| i + 1), queue.len()),
            (prev, next),
            cx,
        );
        let route = Self::tactical_route(pick);
        let supply = Self::tactical_supply(war);
        let comms = Self::tactical_comms(war);

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(c(tac::BG, 1.))
            .font_family(tac::MONO)
            .text_color(c(tac::INK, 1.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                    let hit = this
                        .tactical
                        .hits
                        .borrow()
                        .iter()
                        .rev()
                        .find(|(_, r)| {
                            x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
                        })
                        .map(|(h, _)| h.clone());
                    if let Some(Hit::Pick(id)) = hit {
                        this.tactical.pick = Some(id);
                        cx.notify();
                    }
                }),
            )
            .child(picture)
            .child(
                div()
                    .absolute()
                    .top(px(18.))
                    .right(px(20.))
                    .bottom(px(22.))
                    .w(px(452.))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .child(tracking)
                    .child(route)
                    .child(supply)
                    .child(comms),
            )
            .child(tac::scan())
            .into_any_element()
    }

    /// The tracking panel: the contact that waits, its order request and the order buttons.
    fn tactical_tracking(
        pick: Option<&Pane>,
        brg: Option<f32>,
        now: i64,
        (index, total): (usize, usize),
        (prev, next): (Option<String>, Option<String>),
        cx: &mut Context<Self>,
    ) -> Div {
        let caption = if total == 0 {
            "TRACKING".to_string()
        } else {
            format!("TRACKING · CONTACT {index} OF {total}")
        };
        let panel = tbox(caption)
            .flex_none()
            .h(px(300.))
            .p(px(24.))
            .gap(px(12.));
        let Some(pane) = pick else {
            return panel
                .child(section("ALL CLEAR"))
                .child(tac::entry("No contact awaits orders."))
                .child(label(
                    "A white lock on the scope marks a contact that awaits orders.",
                ));
        };
        let Some(ask) = pane.ask.as_ref() else {
            return panel.child(section(dots(&pane.title.to_uppercase(), 28)));
        };
        let mut panel = panel
            .child(
                div()
                    .self_start()
                    .px(px(12.))
                    .py(px(5.))
                    .bg(c(tac::INK, 1.))
                    .font_family(tac::SANS)
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(ui::ts(tac::S_SECTION))
                    .text_color(c(tac::DARK, 1.))
                    .child(dots(
                        &format!("{}/{}", pane.codebase, pane.title).to_uppercase(),
                        30,
                    )),
            )
            .child(label(format!(
                "BRG {:03.0}° · RNG {} · {} · LINES {}",
                brg.unwrap_or(0.),
                {
                    let ms = (now - pane.since).max(0);
                    format!("{}M{:02}S", ms / 60_000, ms / 1000 % 60)
                },
                pane.provider.to_uppercase(),
                pane.lines_total
            )));
        match ask.kind {
            AskKind::Permission => {
                panel = panel
                    .child(tac::entry(format!("{} asks to run:", pane.provider)))
                    .child(tac::code(format!("$ {}", dots(&ask.text, 90))));
            }
            AskKind::Plan => {
                panel = panel
                    .child(tac::entry("Approve the plan?"))
                    .child(tac::code(dots(&ask.steps.join("  >  "), 90)));
            }
            _ => {
                panel = panel.child(tac::entry(dots(&ask.text, 150)));
            }
        }
        let mut orders = div().flex().gap(px(8.));
        let n = ask.options.len();
        for (i, option) in ask.options.iter().enumerate() {
            let (sid, aid, opt) = (pane.id.clone(), ask.id.clone(), option.clone());
            let kind = if i == 0 && !ask.risky {
                Btn::On
            } else if i + 1 == n && n > 1 {
                Btn::Dim
            } else {
                Btn::Plain
            };
            orders = orders.child(
                tac::order(
                    format!("tac-ans-{i}"),
                    &format!("[{}] {}", i + 1, key_label(option)),
                    kind,
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.skin_answer(&sid, &aid, &opt, cx)),
                ),
            );
        }
        let (open_id, park_id) = (pane.id.clone(), pane.id.clone());
        let mut nav = div().flex().items_center().justify_between();
        nav = nav.child(tac::navb("tac-prev", "< PREV", false).on_click(cx.listener(
            move |this, _, _, cx| {
                this.tactical.pick = prev.clone();
                cx.notify();
            },
        )));
        nav = nav.child(tac::navb("tac-park", "PARK", false).on_click(
            cx.listener(move |this, _, _, cx| this.act(Action::Park(park_id.clone()), cx)),
        ));
        nav = nav.child(
            tac::navb("tac-open", "OPEN DOSSIER >", false)
                .border_1()
                .border_color(c(tac::INK, 1.))
                .text_color(c(tac::INK, 1.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tactical.open = Some(open_id.clone());
                    cx.notify();
                })),
        );
        nav = nav.child(tac::navb("tac-next", "NEXT >", false).on_click(cx.listener(
            move |this, _, _, cx| {
                this.tactical.pick = next.clone();
                cx.notify();
            },
        )));
        panel.child(orders).child(nav)
    }

    /// The mission route of the contact in the tracking panel.
    fn tactical_route(pick: Option<&Pane>) -> Div {
        let caption = match pick {
            Some(p) => format!(
                "MISSION ROUTE · {}",
                dots(&format!("{}/{}", p.codebase, p.title).to_uppercase(), 28)
            ),
            None => "MISSION ROUTE".to_string(),
        };
        let panel = tbox(caption).flex_none().h(px(172.));
        let Some(pane) = pick else {
            return panel.p(px(24.)).child(label("NO CONTACT IN TRACKING"));
        };
        let paint = route_paint(route_steps(pane));
        panel.child(Self::tactical_picture(paint).absolute().inset_0())
    }

    /// A canvas that paints `paint` and takes no clicks.
    pub(super) fn tactical_picture(paint: PaintFn) -> impl Styled + IntoElement {
        let cache = std::rc::Rc::new(std::cell::RefCell::new(super::common::TextCache::default()));
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let rect = [
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                ];
                let mut cache = cache.borrow_mut();
                let mut hits = Vec::new();
                let mut p = Painter::new(window, cx, &mut cache);
                paint(&mut p, rect, &mut hits);
            },
        )
    }

    /// The supply bars: one column per usage window, and the spend of the live sessions.
    fn tactical_supply(war: &WarRoom) -> Div {
        let mut cols = div().flex().gap(px(14.));
        for u in war.usage.iter().take(3) {
            let lit = ((u.pct as f32) / 12.5).ceil().min(8.) as usize;
            let mut bar = div().flex().flex_col().gap(px(2.));
            for k in (0..8).rev() {
                let on = k < lit;
                let warn = (k + 1) as f32 * 12.5 > 75.;
                let color = if warn { tac::WHITE } else { tac::INK };
                bar = bar.child(
                    div()
                        .h(px(9.))
                        .border_1()
                        .border_color(c(if on { color } else { tac::DEEP }, 1.))
                        .when(on, |d| d.bg(c(color, 1.))),
                );
            }
            cols = cols.child(
                div()
                    .w(px(58.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(bar)
                    .child(
                        div()
                            .text_size(ui::ts(tac::S_LABEL))
                            .text_color(c(tac::LABEL, 1.))
                            .child(dots(&u.provider.to_uppercase(), 7)),
                    )
                    .child(
                        div()
                            .text_size(ui::ts(tac::S_LABEL))
                            .text_color(c(tac::INK, 1.))
                            .child(format!("{}%", u.pct)),
                    ),
            );
        }
        let live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
        let spend: f64 = live.iter().map(|s| s.cost).sum();
        let tokens: u64 = live.iter().map(|s| s.tokens).sum();
        let readout = div()
            .flex_none()
            .w(px(160.))
            .h(px(96.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .bg(c(0x0A2A12, 1.))
            .border_1()
            .border_color(c(tac::DIM, 1.))
            .child(label("SPEND, LIVE"))
            .child(
                div()
                    .font_family(tac::SANS)
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(ui::ts(tac::S_BIG))
                    .text_color(c(tac::INK, 1.))
                    .child(format!("${spend:05.2}")),
            )
            .child(label(format!(
                "{} LIVE · {} TOK",
                live.len(),
                compact(tokens)
            )));
        tbox("SUPPLY")
            .flex_none()
            .h(px(150.))
            .px(px(24.))
            .py(px(20.))
            .flex_row()
            .items_center()
            .justify_between()
            .child(cols)
            .child(readout)
    }

    /// The comms log: the latest move of each live session, newest first.
    fn tactical_comms(war: &WarRoom) -> Div {
        let mut live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
        live.sort_by(|a, b| b.since.cmp(&a.since).then(a.id.cmp(&b.id)));
        let mut log = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .px(px(24.))
            .py(px(20.));
        for s in live.into_iter().take(5) {
            let (text, color) = comms_text(s);
            log = log.child(log_line(s.since, text, color));
        }
        tbox("COMMS · AIVORYD").flex_1().min_h(px(120.)).child(log)
    }
}
