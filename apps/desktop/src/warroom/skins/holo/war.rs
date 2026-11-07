//! The Holo war room (board `HoloBloom`): the decision at the left (one at a time, "n / N" and
//! Next), today's usage below it, the agent bloom in the middle, the selected session and the
//! last events at the right. Every value comes from the model. The strands do not wobble and
//! nothing is random: a strand has the shape that its session id seeds, and it changes only
//! when the tokens of the session change. Only the packets on working strands and the far
//! rings move.

use std::f32::consts::{PI, TAU};

use aivory_protocol::{AskKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, SharedString, Window, canvas, div, px, relative, rgb};

use super::super::common::{Align, Painter, asks, clip, key_label, next_ask, part_of};
use super::super::hud::Contact;
use crate::actions::Action;
use crate::holo_kit::*;
use crate::warroom::WarRoomScreen;
use crate::warroom::bridge_data::{provider_name, turn_of};

/// The space above the panels (the tabs sit on the panel edge) and below them.
const TOP: f32 = 30.;
const BOTTOM: f32 = 40.;
/// The width of a side column at 1440 px.
const COLUMN: f32 = 320.;

// ---- data ---------------------------------------------------------------------------------

/// The live sessions grouped by codebase. A codebase keeps the place that the model gives it;
/// inside a codebase the sessions sort by id, so a strand moves only when the data changes.
pub(super) fn contacts(war: &WarRoom) -> (Vec<Contact>, Vec<String>) {
    let mut books: Vec<String> = war
        .codebases
        .iter()
        .map(|c| c.id.clone())
        .filter(|id| {
            war.sessions
                .iter()
                .any(|s| s.state.is_live() && &s.codebase == id)
        })
        .collect();
    for s in war.sessions.iter().filter(|s| s.state.is_live()) {
        if !books.contains(&s.codebase) {
            books.push(s.codebase.clone());
        }
    }
    let mut list = Vec::new();
    for b in &books {
        let mut group: Vec<&Pane> = war
            .sessions
            .iter()
            .filter(|s| s.state.is_live() && &s.codebase == b)
            .collect();
        group.sort_by(|a, b| a.id.cmp(&b.id));
        for s in group {
            list.push(Contact {
                id: s.id.clone(),
                title: if s.title.is_empty() {
                    part_of(s)
                } else {
                    clip(&s.title, 30)
                },
                codebase: s.codebase.clone(),
                state: s.state,
                tokens: s.tokens,
                minutes: (war.now - s.since).max(0) as f32 / 60_000.,
                rate: 0.,
            });
        }
    }
    (list, books)
}

/// The session in the Selected panel: the pick of the owner when it is live, else the working
/// session with the most tokens.
fn selected_id(list: &[Contact], sel: Option<&String>) -> Option<String> {
    if let Some(id) = sel.filter(|id| list.iter().any(|c| &c.id == *id)) {
        return Some(id.clone());
    }
    list.iter()
        .filter(|c| c.state == SessionState::Working)
        .max_by_key(|c| (c.tokens, std::cmp::Reverse(c.id.clone())))
        .or(list.first())
        .map(|c| c.id.clone())
}

/// `text` wrapped at word borders into whole lines of at most `width` characters. Whole
/// sentences come first: when the first sentence fits `max` lines, the rest is left out at a
/// sentence end. No line ends with a cut mark.
fn whole_lines(text: &str, width: usize, max: usize) -> Vec<String> {
    let wrap = |t: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut line = String::new();
        for word in t.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            out.push(line);
        }
        out
    };
    let mut taken = String::new();
    for sentence in text.split_inclusive(['.', '?', '!']) {
        let next = format!("{taken}{sentence}");
        if wrap(&next).len() > max {
            break;
        }
        taken = next;
    }
    let mut lines = if taken.trim().is_empty() {
        wrap(text)
    } else {
        wrap(&taken)
    };
    lines.truncate(max);
    lines
}

/// `182 k`, `1.2 M`.
pub(super) fn kilo(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=99_999 => format!("{:.1} k", n as f32 / 1000.),
        100_000..=999_999 => format!("{} k", n / 1000),
        _ => format!("{:.1} M", n as f32 / 1_000_000.),
    }
}

/// `6:12` for the time since `since`.
pub(super) fn clock(now: i64, since: i64) -> String {
    let secs = ((now - since) / 1000).max(0);
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// `2 h 14 m` for a number of minutes.
fn span(minutes: i64) -> String {
    let m = minutes.max(0);
    if m >= 60 {
        format!("{} h {} m", m / 60, m % 60)
    } else {
        format!("{m} m")
    }
}

// ---- geometry -----------------------------------------------------------------------------

/// The width of a side column for a window of width `w`.
pub(super) fn column(w: f32) -> f32 {
    ((w - 2. * MARGIN - 2. * GAP - 400.) / 2.).clamp(256., COLUMN)
}

/// The center region (local to its own corner) and the bloom in it.
#[derive(Debug, Clone, Copy)]
struct Bloom {
    cw: f32,
    ch: f32,
    c: [f32; 2],
    s: f32,
}

const HEAD: f32 = 64.;
const FOOT: f32 = 100.;

/// The bloom for a window of `w` x `h` pixels (under the bar).
fn bloom_geo(w: f32, h: f32) -> Bloom {
    let cw = (w - 2. * (MARGIN + column(w) + GAP)).max(240.);
    let ch = (h - TOP - BOTTOM).max(300.);
    let avail = (ch - HEAD - FOOT).max(200.);
    let s = ((cw / 2. - 28.) / 292.)
        .min((avail / 2. - 6.) / 292.)
        .clamp(0.35, 1.15);
    Bloom {
        cw,
        ch,
        c: [cw / 2., HEAD + avail / 2.],
        s,
    }
}

/// One session on the bloom.
#[derive(Debug, Clone)]
struct Spot {
    a: f32,
    len: f32,
    tip: [f32; 2],
    /// The width of the angle that the session owns.
    step: f32,
}

/// One codebase on the ring.
#[derive(Debug, Clone)]
struct Arc {
    a0: f32,
    a1: f32,
    waits: bool,
    name: String,
    count: usize,
    label: [f32; 2],
    /// The session that a click on the label opens: the first that waits, else the first.
    target: usize,
}

#[derive(Debug, Clone, Default)]
struct Plan {
    spots: Vec<Spot>,
    arcs: Vec<Arc>,
}

/// The identity: the bloom is flat.
fn same(v: [f32; 2]) -> [f32; 2] {
    v
}

fn at(c: [f32; 2], a: f32, r: f32) -> [f32; 2] {
    [c[0] + a.cos() * r, c[1] + a.sin() * r]
}

/// Where the sessions and the codebase arcs sit. The list is grouped by codebase.
fn plan(list: &[Contact], books: &[String], g: &Bloom) -> Plan {
    let n = list.len();
    if n == 0 {
        return Plan::default();
    }
    let max = list.iter().map(|c| c.tokens).max().unwrap_or(1).max(1) as f32;
    let gap = 8f32.to_radians();
    let books: Vec<&String> = books
        .iter()
        .filter(|b| list.iter().any(|c| &c.codebase == *b))
        .collect();
    let step = ((TAU - gap * books.len() as f32) / n as f32).max(0.01);
    let first = list.iter().filter(|c| c.codebase == *books[0]).count();
    let mut ang = -PI / 2. - first as f32 * step / 2.;
    let mut out = Plan::default();
    for b in &books {
        let a0 = ang;
        let mut idx = Vec::new();
        for (i, c) in list.iter().enumerate().filter(|(_, c)| &c.codebase == *b) {
            let a = ang + step / 2.;
            let len = (60. + 150. * (c.tokens as f32 / max).sqrt()) * g.s;
            out.spots.push(Spot {
                a,
                len,
                tip: at(g.c, a, len),
                step,
            });
            idx.push(i);
            ang += step;
        }
        let a1 = ang;
        let waits = idx.iter().any(|&i| list[i].waits());
        out.arcs.push(Arc {
            a0: a0 + 0.02,
            a1: a1 - 0.02,
            waits,
            name: b.to_uppercase(),
            count: idx.len(),
            label: at(g.c, (a0 + a1) / 2., 290. * g.s),
            target: idx
                .iter()
                .copied()
                .find(|&i| list[i].waits())
                .unwrap_or(idx[0]),
        });
        ang += gap;
    }
    out
}

/// A small deterministic random series seeded by a session id.
struct Seed(u32);

impl Seed {
    fn new(id: &str) -> Self {
        let mut h: u32 = 2_166_136_261;
        for b in id.bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
        }
        Self(h | 1)
    }

    /// The next value from 0 to 1.
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 % 10_000) as f32 / 10_000.
    }
}

/// A point on the quadratic curve `p0`, `ctrl`, `p1` at `f`.
fn bezier(p0: [f32; 2], ctrl: [f32; 2], p1: [f32; 2], f: f32) -> [f32; 2] {
    let u = 1. - f;
    [
        u * u * p0[0] + 2. * u * f * ctrl[0] + f * f * p1[0],
        u * u * p0[1] + 2. * u * f * ctrl[1] + f * f * p1[1],
    ]
}

/// The strands of one session: curve start, control point and end, plus a short tail.
fn strands(id: &str, c: [f32; 2], spot: &Spot, count: usize, s: f32) -> Vec<[[f32; 2]; 4]> {
    let mut seed = Seed::new(id);
    (0..count)
        .map(|_| {
            let aa = spot.a + (seed.next() - 0.5) * spot.step * 0.6;
            let l = spot.len * (0.78 + seed.next() * 0.24);
            let ctrl = at(c, aa + (seed.next() - 0.5) * 0.3, l * 0.5);
            let end = at(c, aa, l);
            let ba = aa + (seed.next() - 0.5) * 0.4;
            let bl = (8. + seed.next() * 14.) * s;
            [c, ctrl, end, at(end, ba, bl)]
        })
        .collect()
}

// ---- the picture --------------------------------------------------------------------------

/// The data that the canvas needs.
struct Picture {
    list: Vec<Contact>,
    books: Vec<String>,
    sel: Option<String>,
    /// Agents at work and agents that wait, one value per minute.
    activity: Vec<(u32, u32)>,
    t: f32,
}

impl Picture {
    fn paint(&self, p: &mut Painter, r: [f32; 4]) {
        let (w, h) = (r[2], r[3]);
        let g = bloom_geo(w, h);
        let (ox, oy) = (r[0] + MARGIN + column(w) + GAP, r[1] + TOP);
        let c = [ox + g.c[0], oy + g.c[1]];
        let s = g.s;
        let plan = plan(&self.list, &self.books, &g);
        let max = self.list.iter().map(|c| c.tokens).max().unwrap_or(1).max(1) as f32;

        // The faint ring and the dotted ring that turns slowly.
        p.arc(c, 252. * s, (0., TAU), 1., PK, 0.14, &same);
        let turn = self.t / 80. * TAU;
        let dashes = (TAU * 300. * s / 13.) as usize;
        let segs: Vec<[f32; 4]> = (0..dashes)
            .map(|k| {
                let a = turn + k as f32 * TAU / dashes.max(1) as f32;
                let (p0, p1) = (at(c, a, 300. * s), at(c, a + 3. / (300. * s), 300. * s));
                [p0[0], p0[1], p1[0], p1[1]]
            })
            .collect();
        p.strokes(&segs, PU, 1., 0.7);

        // The codebase arcs.
        for arc in &plan.arcs {
            let color = if arc.waits { OR } else { PK };
            p.arc(c, 262. * s, (arc.a0, arc.a1), 14., color, 0.08, &same);
            p.arc(
                c,
                262. * s,
                (arc.a0, arc.a1),
                4.,
                color,
                if arc.waits { 1. } else { 0.7 },
                &same,
            );
        }

        // The strands. Waiting sessions are bright; the rest is dimmed.
        for (con, spot) in self.list.iter().zip(&plan.spots) {
            let waits = con.waits();
            let idle = !waits && con.state != SessionState::Working;
            let (c1, c2) = if waits {
                (OR, 0xFFC48A)
            } else if idle {
                (0x2A5878, 0x22496A)
            } else {
                (PK, PE)
            };
            let picked = self.sel.as_deref() == Some(con.id.as_str());
            let alpha = if waits {
                0.95
            } else if idle {
                0.4
            } else if picked {
                0.9
            } else {
                0.55
            };
            let bundle = strands(&con.id, c, spot, if idle { 3 } else { 6 }, s);
            let mut curves = Vec::new();
            for (k, st) in bundle.iter().enumerate() {
                let col = if k % 2 == 0 { c1 } else { c2 };
                let mut pts: Vec<[f32; 2]> = (0..=14)
                    .map(|j| bezier(st[0], st[1], st[2], j as f32 / 14.))
                    .collect();
                pts.push(st[3]);
                if waits {
                    p.polyline(&pts, col, 3.6, 0.1, false);
                }
                p.polyline(&pts, col, if k == 0 { 1.4 } else { 1. }, alpha, false);
                curves.push(pts);
            }
            // The light packet runs on the strands of working sessions only. A session with
            // more tokens sends it faster.
            if con.state == SessionState::Working {
                let share = con.tokens as f32 / max;
                let period = 5. - 2.5 * share.sqrt();
                let seed = Seed::new(&con.id).0 % 997;
                let f = (self.t / period + seed as f32 / 997.).fract();
                if let Some(st) = bundle.first() {
                    let a = bezier(st[0], st[1], st[2], f);
                    let b = bezier(st[0], st[1], st[2], (f + 0.05).min(1.));
                    p.strokes(&[[a[0], a[1], b[0], b[1]]], 0xF2FDFF, 2.4, 0.9);
                    p.dot(b, 2.4, 0xF2FDFF, 0.9);
                }
            }
            let tip_color = if waits {
                OR
            } else if idle {
                0x2A5878
            } else {
                PA
            };
            let tip = curves
                .first()
                .and_then(|c| c.get(14))
                .copied()
                .unwrap_or(spot.tip);
            if waits || picked {
                p.dot(tip, if waits { 8. } else { 6. }, tip_color, 0.18);
            }
            p.dot(tip, if waits { 4.5 } else { 2.5 }, tip_color, 1.);
            // The lead to the ask pill.
            if waits {
                let o = at(c, spot.a, spot.len + 20. * s);
                let right = spot.a.cos() >= 0.;
                let hx = o[0] + if right { 12. } else { -12. };
                p.polyline(&[tip, o, [hx, o[1]]], OR, 1., 0.8, false);
            }
        }

        // The core.
        p.dot(c, 42. * s, PK, 0.1);
        p.dot(c, 34. * s, PK, 0.12);
        p.dot(c, 28. * s, 0x04101F, 0.88);
        p.arc(c, 28. * s, (0., TAU), 1.2, PK, 1., &same);

        // The two quiet lines at the bottom: agents at work and agents that wait, per minute.
        let width = g.cw.min(580.);
        let col = (width - 40.) / 2.;
        let x0 = ox + (g.cw - width) / 2.;
        let base = oy + g.ch - 6.;
        for (n, (name, color)) in [("AGENTS AT WORK", SA), ("AGENTS THAT WAIT", OR)]
            .iter()
            .enumerate()
        {
            let x = x0 + n as f32 * (col + 40.);
            let series: Vec<f32> = self
                .activity
                .iter()
                .map(|a| if n == 0 { a.0 } else { a.1 } as f32)
                .collect();
            let top = series.iter().copied().fold(1., f32::max);
            let now = series.last().copied().unwrap_or(0.);
            p.text(
                name,
                S_SMALL,
                16.,
                LABEL,
                true,
                MUTED,
                0.8,
                (x, base - 54.),
                Align::Left,
            );
            p.text(
                &format!("{now:.0}"),
                S_SMALL,
                16.,
                CODE,
                false,
                PA,
                0.9,
                (x + col, base - 54.),
                Align::Right,
            );
            p.strokes(&[[x, base, x + col, base]], PK, 1., 0.18);
            if series.len() > 1 {
                let pts: Vec<[f32; 2]> = series
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        [
                            x + col * i as f32 / (series.len() - 1) as f32,
                            base - 3. - v / top * 26.,
                        ]
                    })
                    .collect();
                let mut fill = pts.clone();
                fill.push([x + col, base]);
                fill.push([x, base]);
                p.polygon(&fill, *color, 0.1);
                p.polyline(&pts, *color, 1.2, 0.9, false);
            }
        }
    }
}

// ---- small elements -----------------------------------------------------------------------

fn mono(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .font_family(CODE)
        .text_size(px(S_SMALL - 1.))
        .line_height(px(S_SMALL + 6.))
        .text_color(rgb(color))
        .child(text.into())
}

/// The ring of today's panel: a track, an arc for `pct`, the figure in the middle.
fn ring(pct: u8, color: u32) -> Div {
    let frac = f32::from(pct.min(100)) / 100.;
    div()
        .relative()
        .flex_none()
        .size(px(96.))
        .child(
            canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    let c = [
                        f32::from(b.origin.x) + f32::from(b.size.width) / 2.,
                        f32::from(b.origin.y) + f32::from(b.size.height) / 2.,
                    ];
                    let r = 42.;
                    let track: Vec<[f32; 2]> =
                        (0..=48).map(|i| at(c, TAU * i as f32 / 48., r)).collect();
                    line(window, &track, PK, 5., 0.12, true);
                    let steps = ((frac * 48.) as usize).max(1);
                    let arc: Vec<[f32; 2]> = (0..=steps)
                        .map(|i| at(c, -PI / 2. + TAU * frac * i as f32 / steps as f32, r))
                        .collect();
                    if frac > 0. {
                        line(window, &arc, color, 14., 0.1, false);
                        line(window, &arc, color, 5., 1., false);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .font_family(FIGURE)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(px(S_BIG))
                        .text_color(rgb(TXT))
                        .child(format!("{pct}%")),
                ),
        )
}

/// A bar of 3 px with a fill of `pct` percent.
fn meter(pct: u8, color: u32) -> Div {
    div().h(px(3.)).w_full().bg(ink(PK, 0.12)).child(
        div()
            .h_full()
            .w(relative(f32::from(pct.min(100)) / 100.))
            .bg(rgb(color)),
    )
}

impl WarRoomScreen {
    /// The overview: decision, usage, bloom, selected session, last events.
    pub(super) fn holo_overview(
        &mut self,
        war: &WarRoom,
        t: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vp = window.viewport_size();
        let (w, h) = (f32::from(vp.width), f32::from(vp.height) - BAR_H);
        let g = bloom_geo(w, h);
        let col = column(w);
        let (list, books) = contacts(war);
        let queue = asks(war);
        let pick_id = self
            .holo
            .pick
            .clone()
            .filter(|id| queue.iter().any(|p| &p.id == id))
            .or_else(|| queue.first().map(|p| p.id.clone()));
        let pick = pick_id
            .as_ref()
            .and_then(|id| queue.iter().find(|p| &p.id == id).copied());
        let index = pick_id
            .as_ref()
            .and_then(|id| queue.iter().position(|p| &p.id == id))
            .map_or(0, |i| i + 1);
        let next = next_ask(&queue, pick_id.as_deref());
        let sel_id = selected_id(&list, self.holo.sel.as_ref());
        let sel = sel_id.as_ref().and_then(|id| war.session(id));

        let activity: Vec<(u32, u32)> = war
            .activity
            .iter()
            .rev()
            .take(60)
            .rev()
            .map(|a| (a.active, a.waiting))
            .collect();
        let picture = Picture {
            list: list.clone(),
            books: books.clone(),
            sel: sel_id.clone(),
            activity,
            t,
        };
        let cache = self.holo.cache.clone();
        let bloom = canvas(
            |_, _, _| (),
            move |b, _, window, cx| {
                let rect = [
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                ];
                let mut cache = cache.borrow_mut();
                let mut p = Painter::new(window, cx, &mut cache);
                picture.paint(&mut p, rect);
            },
        )
        .absolute()
        .inset_0();

        // ---- left column --------------------------------------------------------------------
        let spend_h = ((h - TOP - BOTTOM - GAP) * 0.45).clamp(230., 340.);
        let dec_h = h - TOP - BOTTOM - GAP - spend_h;
        let decision = self.decision_panel(war, pick, (index, queue.len()), next, dec_h, cx);
        let left = div()
            .flex_none()
            .w(px(col))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(decision)
            .child(self.usage_panel(war, spend_h));

        // ---- center overlay -----------------------------------------------------------------
        let plan = plan(&list, &books, &g);
        let mut center = div().relative().flex_1().min_w(px(0.)).h_full();
        center = center
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .child(th("Agent bloom").text_color(rgb(PA)))
                    .child(
                        div()
                            .mt(px(4.))
                            .child(mono("one strand bundle per session", 0x5F95B8))
                            .child(mono("length = tokens of the session", 0x5F95B8)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(6.))
                    .child(legend_row("Working", PK))
                    .child(legend_row("Waits for you", OR))
                    .child(legend_row("Idle", 0x2A5878)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(g.c[0] - 40.))
                    .top(px(g.c[1] - 24. * g.s.max(0.8)))
                    .w(px(80.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .font_family(FIGURE)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(S_BIG - 3.))
                            .line_height(px(S_BIG - 3.))
                            .text_color(rgb(TXT))
                            .child(list.len().to_string()),
                    )
                    .child(th("Live").text_size(px(S_SMALL - 2.))),
            );
        if list.is_empty() {
            center = center.child(
                div()
                    .absolute()
                    .left(px(g.c[0] - 120.))
                    .top(px(g.c[1] + 60.))
                    .w(px(240.))
                    .flex()
                    .justify_center()
                    .child(th("No live sessions")),
            );
        }
        // Codebase labels open their session.
        for (k, arc) in plan.arcs.iter().enumerate() {
            let Some(target) = list.get(arc.target) else {
                continue;
            };
            let id = target.id.clone();
            center = center.child(
                div()
                    .id(SharedString::from(format!("holo-book-{k}")))
                    .absolute()
                    .left(px(arc.label[0] - 80.))
                    .top(px(arc.label[1] - 12.))
                    .w(px(160.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .font_family(LABEL)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_size(px(S_SMALL + 1.))
                    .text_color(ink(if arc.waits { GD } else { PA }, 0.9))
                    .hover(|s| s.text_color(rgb(TXT)))
                    .child(clip(&arc.name, 16))
                    .child(mono(arc.count.to_string(), 0x5F95B8))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.holo.open = Some(id.clone());
                        cx.notify();
                    })),
            );
        }
        // The tips select a session; the ask pills open it.
        for (i, (con, spot)) in list.iter().zip(&plan.spots).enumerate() {
            let id = con.id.clone();
            if con.waits() {
                let o = at(g.c, spot.a, spot.len + 20. * g.s);
                let right = spot.a.cos() >= 0.;
                let hx = o[0] + if right { 16. } else { -16. };
                let wait = clock(war.now, war.session(&con.id).map_or(war.now, |p| p.since));
                let pill = div()
                    .id(SharedString::from(format!("holo-ask-{i}")))
                    .absolute()
                    .top(px(o[1] - 13.))
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded_full()
                    .border_1()
                    .border_color(ink(OR, 0.8))
                    .bg(ink(0x1E0C00, 0.6))
                    .cursor_pointer()
                    .font_family(LABEL)
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(S_SMALL))
                    .text_color(rgb(0xFFE0C8))
                    .whitespace_nowrap()
                    .hover(|s| s.bg(ink(OR, 0.3)))
                    .child(div().size(px(7.)).rounded_full().bg(rgb(OR)))
                    .child(format!(
                        "{}/{}",
                        con.codebase.to_lowercase(),
                        clip(&con.title, 18)
                    ))
                    .child(mono(wait, 0xE0A37A))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.holo.open = Some(id.clone());
                        cx.notify();
                    }));
                center = center.child(if right {
                    pill.left(px(hx))
                } else {
                    pill.right(px(g.cw - hx))
                });
            } else {
                center = center.child(
                    div()
                        .id(SharedString::from(format!("holo-tip-{i}")))
                        .absolute()
                        .left(px(spot.tip[0] - 11.))
                        .top(px(spot.tip[1] - 11.))
                        .size(px(22.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.holo.sel = Some(id.clone());
                            cx.notify();
                        })),
                );
            }
        }

        // ---- right column -------------------------------------------------------------------
        let events_h = spend_h;
        let right = div()
            .flex_none()
            .w(px(col))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(self.selected_panel(war, sel, cx))
            .child(events_panel(war, events_h));

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .font_family(BODY)
            .text_color(rgb(TXT))
            .child(bloom)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .gap(px(GAP))
                    .px(px(MARGIN))
                    .pt(px(TOP))
                    .pb(px(BOTTOM))
                    .child(left)
                    .child(center)
                    .child(right),
            )
            .into_any_element()
    }

    /// The decision: one ask at a time, "n / N" in the tab and Next.
    fn decision_panel(
        &mut self,
        war: &WarRoom,
        pick: Option<&Pane>,
        (index, total): (usize, usize),
        next: Option<String>,
        dec_h: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some((pane, ask)) = pick.and_then(|p| Some((p, p.ask.as_ref()?))) else {
            return panel("No decision", false)
                .flex_1()
                .min_h(px(0.))
                .child(
                    div()
                        .mt(px(8.))
                        .font_family(LABEL)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(px(S_BIG - 4.))
                        .line_height(px(S_BIG + 2.))
                        .text_color(rgb(PA))
                        .child("Nothing waits for you."),
                )
                .child(
                    div()
                        .mt(px(12.))
                        .text_color(ink(DIM, 0.8))
                        .child("Open a session from the bloom to read its work."),
                );
        };
        let wait = clock(war.now, pane.since);
        let headline = match ask.kind {
            AskKind::Permission => format!("Allow {} to run this?", provider_name(&pane.provider)),
            AskKind::Plan => "Approve the plan?".to_string(),
            _ => clip(ask.text.lines().next().unwrap_or(""), 120),
        };
        let detail = match ask.kind {
            AskKind::Permission => Some(ask.text.clone()),
            AskKind::Plan if !ask.steps.is_empty() => Some(
                ask.steps
                    .iter()
                    .take(4)
                    .enumerate()
                    .map(|(i, s)| format!("{}. {}", i + 1, clip(s, 40)))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            AskKind::Plan => Some(ask.text.clone()),
            _ => None,
        };
        let mut body = div()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(mono(
                format!(
                    "{}/{} · {} · {}",
                    pane.codebase,
                    part_of(pane),
                    pane.provider,
                    wait
                ),
                0xE0A37A,
            ))
            .child(
                div()
                    .font_family(LABEL)
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(S_BIG - 3.))
                    .line_height(px(S_BIG + 1.))
                    .text_color(rgb(WARM))
                    .child(headline),
            );
        if let Some(text) = detail {
            body = body.child(
                code(&clip(&text, 220), OR)
                    .max_h(px(120.))
                    .overflow_hidden(),
            );
        }
        // Why it asks: the reason of the ask, else the last words of the agent.
        let why = ask
            .reason
            .clone()
            .filter(|r| !r.trim().is_empty())
            .or_else(|| {
                pane.tail
                    .iter()
                    .rev()
                    .find(|l| l.kind == aivory_protocol::LineKind::Agent)
                    .map(|l| crate::console::display::plain_line(&l.text))
                    .filter(|t| !t.trim().is_empty())
            });
        // The lines that fit under the command: about 25 px each, whole sentences first.
        let fit = ((dec_h - 380.) / 25.).floor().clamp(0., 4.) as usize;
        if let Some(why) = why
            && fit > 0
        {
            let mut shown = div().flex().flex_col();
            for l in whole_lines(&why, 34, fit) {
                shown = shown.child(
                    div()
                        .h(px(25.))
                        .whitespace_nowrap()
                        .text_color(ink(0xFFD9BD, 0.85))
                        .child(l),
                );
            }
            body = body.child(th("Why it asks")).child(shown);
        }
        // The answers: the first is the warm primary and fills the row.
        let n = ask.options.len();
        let mut first = div().flex().flex_col().gap(px(10.));
        let mut rest = div().flex().gap(px(10.));
        for (i, option) in ask.options.iter().enumerate() {
            let (sid, aid, opt) = (pane.id.clone(), ask.id.clone(), option.clone());
            let tone = match option_color(i, n) {
                c if c == OR => Tone::Primary,
                c if c == GY => Tone::Ghost,
                _ => Tone::Plain,
            };
            let b = button(
                format!("holo-ans-{i}"),
                tone,
                &format!("{}  {}", i + 1, key_label(option)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.skin_answer(&sid, &aid, &opt, cx)));
            if i == 0 {
                first = first.child(b.w_full());
            } else {
                rest = rest.child(b.flex_1());
            }
        }
        let (open_id, park_id) = (pane.id.clone(), pane.id.clone());
        let mut links = div().flex().items_center().justify_between().child(
            link("holo-open", "Open session ›", GD).on_click(cx.listener(move |this, _, _, cx| {
                this.holo.open = Some(open_id.clone());
                cx.notify();
            })),
        );
        links = links.child(link("holo-park", "Park", MUTED).on_click(
            cx.listener(move |this, _, _, cx| this.act(Action::Park(park_id.clone()), cx)),
        ));
        if total > 1 {
            links = links.child(link("holo-next", "Next ›", PA).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.holo.pick = next.clone();
                    cx.notify();
                },
            )));
        }
        panel(&format!("Waits for you · {index} / {total}"), true)
            .flex_1()
            .min_h(px(0.))
            .gap(px(14.))
            .child(body)
            .child(first)
            .child(rest)
            .child(links)
    }

    /// Today: the busiest usage window as a ring, every window as a bar, and the cost.
    fn usage_panel(&self, war: &WarRoom, height: f32) -> Div {
        let mut p = panel("Today", false).h(px(height)).flex_none();
        let top = war
            .usage
            .iter()
            .enumerate()
            .max_by_key(|(_, u)| u.pct)
            .map(|(i, _)| i);
        let color_of = |u: &aivory_protocol::UsageWindow| {
            if u.pct >= 85 {
                OR
            } else {
                match u.provider.as_str() {
                    "claude" => SA,
                    "codex" => PK,
                    _ => PE,
                }
            }
        };
        let resets = |u: &aivory_protocol::UsageWindow| {
            u.resets_at
                .map(|r| format!("resets {}", span((r - war.now) / 60_000)))
                .unwrap_or_else(|| {
                    if u.estimate {
                        "estimate".into()
                    } else {
                        String::new()
                    }
                })
        };
        let live = war.sessions.iter().filter(|s| s.state.is_live());
        let (agents, cost) = live.fold((0, 0.), |(a, c), s| (a + s.agents(), c + s.cost));
        let footer = format!("{agents} agents · ${cost:.2} live");
        // The busiest window: the ring, its name on two lines (nothing is cut), its reset.
        let head = top.and_then(|i| war.usage.get(i)).map(|u| {
            div()
                .flex()
                .items_center()
                .gap(px(18.))
                .child(ring(u.pct, color_of(u)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(th(u.provider.clone()))
                        .child(th(u.label.clone()).text_color(rgb(PA)))
                        .child(mono(resets(u), 0x5F95B8))
                        .child(mono(footer, 0x5F95B8)),
                )
        });
        // The other windows: name, reset and figure on one line, the bar under it.
        let mut rest = div().flex().flex_col().gap(px(4.));
        let others: Vec<&aivory_protocol::UsageWindow> = war
            .usage
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != top)
            .map(|(_, u)| u)
            .collect();
        // Whole rows only: the ring head, the footer and the paddings come first.
        // The head is 86 px, a row 24 px with a gap of 4 px.
        let room = ((height - 2. * PAD - 86. - 16. + 4.) / 28.).floor().max(0.) as usize;
        let shown = if others.len() <= room {
            others.len()
        } else {
            room.saturating_sub(1)
        };
        for u in others.iter().take(shown).copied() {
            rest = rest.child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(th(format!("{} {}", u.provider, u.label)).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .justify_end()
                                    .child(mono(resets(u), 0x5F95B8).whitespace_nowrap()),
                            )
                            .child(mono(format!("{}%", u.pct), PA)),
                    )
                    .child(meter(u.pct, color_of(u))),
            );
        }
        if others.len() > shown {
            rest = rest.child(mono(
                format!("+ {} more windows", others.len() - shown),
                0x5F95B8,
            ));
        }
        if war.usage.is_empty() {
            rest = rest.child(th("No usage data"));
        }
        p = p.gap(px(16.)).children(head).child(rest);
        p
    }

    /// The selected session: where it works, what it did last, its context.
    fn selected_panel(&mut self, war: &WarRoom, sel: Option<&Pane>, cx: &mut Context<Self>) -> Div {
        let Some(pane) = sel else {
            return panel("Selected", false)
                .flex_1()
                .min_h(px(0.))
                .child(th("No live session"));
        };
        let minutes = pane.minutes(war.now);
        let mut meta = format!("{} · ", pane.provider);
        if let Some(turn) = turn_of(pane) {
            meta.push_str(&format!("turn {turn} · "));
        }
        meta.push_str(&format!("{} m · {} tok", minutes, kilo(pane.tokens)));
        let goal = pane.prompt.lines().next().unwrap_or("").trim();
        let steps: Vec<(String, String)> = pane
            .tail
            .iter()
            .filter(|l| l.kind == aivory_protocol::LineKind::Tool)
            .rev()
            .take(3)
            .map(|l| {
                (
                    l.label.clone().unwrap_or_else(|| "Tool".to_string()),
                    super::super::common::short_tool("", &l.text),
                )
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let mut steps_box = div().flex().flex_col().gap(px(12.));
        for (label, text) in steps {
            steps_box = steps_box.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(verb(&label))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .max_h(px(36.))
                            .overflow_hidden()
                            .child(mono(text, 0xBFE6F7)),
                    ),
            );
        }
        let (open_id, focus_id) = (pane.id.clone(), pane.id.clone());
        panel("Selected", false)
            .flex_1()
            .min_h(px(0.))
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .max_h(px((S_BIG + 1.) * 2.))
                            .overflow_hidden()
                            .font_family(LABEL)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(S_BIG - 3.))
                            .line_height(px(S_BIG + 1.))
                            .text_color(rgb(TXT))
                            .child(format!("{}/{}", pane.codebase, pane.title)),
                    )
                    .child(mono(meta, 0x5F95B8))
                    .child(
                        div()
                            .max_h(px(LH_BODY * 3.))
                            .overflow_hidden()
                            .text_color(rgb(PA))
                            .line_height(px(LH_BODY))
                            .child(goal.to_string()),
                    )
                    .child(steps_box)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(th("Context"))
                            .child(div().flex_1().child(meter(pane.context_pct, PK)))
                            .child(mono(format!("{}%", pane.context_pct), PA)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        link("holo-sel-open", "Open session ›", PK).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.holo.open = Some(open_id.clone());
                                cx.notify();
                            },
                        )),
                    )
                    .child(link("holo-sel-focus", "Focus ›", MUTED).on_click(
                        cx.listener(move |this, _, _, cx| this.skin_focus(&focus_id, cx)),
                    )),
            )
    }
}

fn legend_row(name: &str, color: u32) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(th(name.to_string()))
        .child(div().w(px(24.)).h(px(2.)).bg(rgb(color)))
}

/// The last events: the sessions that changed state most recently, newest first.
fn events_panel(war: &WarRoom, height: f32) -> Div {
    let mut recent: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
    recent.sort_by(|a, b| (b.since, &a.id).cmp(&(a.since, &b.id)));
    let rows = ((height - 2. * PAD) / 74.).floor().clamp(1., 4.) as usize;
    let mut list = div().flex().flex_col().gap(px(12.));
    for s in recent.iter().take(rows) {
        let (text, color) = match (s.state, s.ask.as_ref()) {
            (SessionState::NeedsYou, Some(a)) => (
                format!("waits: {}", crate::console::display::plain_line(&a.text)),
                GD,
            ),
            (SessionState::NeedsYou, None) => ("waits for you".to_string(), GD),
            (SessionState::Error, _) => ("hit an error".to_string(), GD),
            (SessionState::Working, _) => (format!("works on {}", part_of(s)), 0xBFE6F7),
            _ => ("is idle".to_string(), 0x5F95B8),
        };
        let ago = ((war.now - s.since) / 60_000).max(0);
        list = list.child(
            div()
                .flex()
                .flex_col()
                .child(mono(
                    if ago < 1 {
                        "now".to_string()
                    } else {
                        format!("{ago} min ago")
                    },
                    0x4F7FA0,
                ))
                .child(
                    div()
                        .max_h(px((S_SMALL + 6.) * 2.))
                        .overflow_hidden()
                        .child(mono(format!("{}/{} {}", s.codebase, s.title, text), color)),
                ),
        );
    }
    if recent.is_empty() {
        list = list.child(th("No live sessions"));
    }
    panel("Last events", false)
        .h(px(height))
        .flex_none()
        .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: &str, codebase: &str, state: SessionState, tokens: u64) -> Contact {
        Contact {
            id: id.into(),
            title: id.into(),
            codebase: codebase.into(),
            state,
            tokens,
            minutes: 1.,
            rate: 0.,
        }
    }

    #[test]
    fn a_strand_keeps_its_shape() {
        let g = bloom_geo(1440., 824.);
        let list = vec![
            contact("a", "x", SessionState::Working, 100),
            contact("b", "y", SessionState::NeedsYou, 50),
        ];
        let books = vec!["x".to_string(), "y".to_string()];
        let one = plan(&list, &books, &g);
        let two = plan(&list, &books, &g);
        assert_eq!(one.spots.len(), 2);
        assert_eq!(one.spots[0].tip, two.spots[0].tip);
        let s1 = strands("a", g.c, &one.spots[0], 6, g.s);
        let s2 = strands("a", g.c, &two.spots[0], 6, g.s);
        assert_eq!(s1, s2);
        assert!(one.arcs[1].waits && !one.arcs[0].waits);
    }

    #[test]
    fn the_longest_strand_belongs_to_the_most_tokens() {
        let g = bloom_geo(1440., 824.);
        let list = vec![
            contact("a", "x", SessionState::Working, 100),
            contact("b", "x", SessionState::Working, 25),
        ];
        let p = plan(&list, &["x".to_string()], &g);
        assert!(p.spots[0].len > p.spots[1].len);
    }

    #[test]
    fn numbers_read_short() {
        assert_eq!(kilo(950), "950");
        assert_eq!(kilo(182_000), "182 k");
        assert_eq!(clock(372_000, 0), "6:12");
        assert_eq!(span(134), "2 h 14 m");
    }

    #[test]
    fn the_selected_session_is_the_busiest_worker() {
        let list = vec![
            contact("a", "x", SessionState::NeedsYou, 900),
            contact("b", "x", SessionState::Working, 10),
            contact("c", "x", SessionState::Working, 20),
        ];
        assert_eq!(selected_id(&list, None).as_deref(), Some("c"));
        assert_eq!(
            selected_id(&list, Some(&"a".to_string())).as_deref(),
            Some("a")
        );
        assert_eq!(
            selected_id(&list, Some(&"gone".to_string())).as_deref(),
            Some("c")
        );
    }
}
