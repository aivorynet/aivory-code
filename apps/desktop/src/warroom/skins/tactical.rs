//! The Tactical look, war room: a calm radar scope and three panels (board `TacticalRadar`).
//!
//! The scope shows every live session as one contact. The bearing is the codebase. The range
//! from the center is the time since the session last moved (rings at 1, 5, 15 and 60 minutes),
//! so an ignored session drifts outward. Color says the state and nothing else: amber needs
//! you, cyan works, grey idles, red is a fault. Nothing on the scope moves.
//!
//! The right column has three panels: SELECTED (what the session says and does now, the order
//! request with its buttons, Focus), SESSIONS (one row per session with its latest action,
//! the ones that need you first) and BUDGET (the usage windows and the live spend).

use std::f32::consts::TAU;
use std::time::Instant;

use aivory_protocol::{AskKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, SharedString, Window,
    canvas, div, px, relative,
};

use super::common::{Align, Painter, asks, key_label, last_tool};
use super::hud::{Contact, Hit, PaintFn, Rect, contacts};
use crate::actions::Action;
use crate::console::display::{DKind, last};
use crate::tactical_kit::{self as tac, Btn, c, dots, label, tbox};
use crate::ui;
use crate::warroom::WarRoomScreen;

/// The width of the right column at fit 1.
const COLUMN: f32 = 470.;

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

/// The color of a state.
fn state_color(state: SessionState) -> u32 {
    match state {
        SessionState::NeedsYou => tac::AMBER,
        SessionState::Error => tac::ALARM,
        SessionState::Working => tac::CYAN,
        _ => tac::MID,
    }
}

/// `m:ss` of a wait, or `12m` of a longer time.
fn span_text(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}h{:02}", s / 3600, s / 60 % 60)
    }
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

/// Everything the scope paints.
struct Scope {
    list: Vec<Contact>,
    books: Vec<String>,
    spots: Vec<Spot>,
    pick: Option<String>,
}

fn radar(scope: Scope) -> PaintFn {
    Box::new(move |p, r: Rect, hits| {
        let Scope {
            list,
            books,
            spots,
            pick,
        } = &scope;
        let k = ui::factor();
        let (ox, oy, w, h) = (r[0], r[1], r[2], r[3]);
        let rad = ((h / 2. - 46. * k).min(w / 2. - 100. * k)).clamp(110., 900.);
        let center = [ox + w / 2., oy + h / 2. - 6. * k];
        let id = |q: [f32; 2]| q;

        // ---- the scope: one disc, three rings, the sector lines -----------------------------
        // The hairline grid of the backdrop, then the disc over it.
        let step = 56. * k;
        let mut grid = Vec::new();
        let mut gx = ox + (center[0] - ox).rem_euclid(step);
        while gx < ox + w {
            grid.push([gx, oy, gx, oy + h]);
            gx += step;
        }
        let mut gy = oy + (center[1] - oy).rem_euclid(step);
        while gy < oy + h {
            grid.push([ox, gy, ox + w, gy]);
            gy += step;
        }
        p.strokes(&grid, tac::DEEP, 1., 0.55);
        p.dot(center, rad, tac::PANEL, 0.92);
        // The bearing ring: a tick every 10 degrees, longer every 30.
        let mut ring = Vec::new();
        for d in (0..360).step_by(10) {
            let len = if d % 30 == 0 { 9. } else { 5. } * k;
            let (a1, a2) = (at(center, d as f32, rad), at(center, d as f32, rad + len));
            ring.push([a1[0], a1[1], a2[0], a2[1]]);
        }
        p.strokes(&ring, tac::MID, 1., 0.7);
        let nb = books.len().max(1);
        let span = 360. / nb as f32;
        if books.len() > 1 {
            let mut lines = Vec::new();
            for bi in 0..nb {
                let to = at(center, bi as f32 * span, rad);
                lines.push([center[0], center[1], to[0], to[1]]);
            }
            p.strokes(&lines, tac::DIM, 1., 0.45);
        }
        for f in [0.25, 0.5, 0.75] {
            p.arc(center, rad * f, (0., TAU), 1., tac::DIM, 0.7, &id);
        }
        p.arc(center, rad, (0., TAU), 1.5, tac::DIM, 1., &id);
        p.strokes(
            &[
                [center[0] - 8. * k, center[1], center[0] + 8. * k, center[1]],
                [center[0], center[1] - 8. * k, center[0], center[1] + 8. * k],
            ],
            tac::MID,
            1.,
            0.8,
        );
        let mut placed: Vec<[f32; 4]> = Vec::new();
        for (f, name) in [(0.25, "1M"), (0.5, "5M"), (0.75, "15M"), (1., "60M")] {
            placed.push([center[0] + 2. * k, center[1] - rad * f, 34. * k, 18. * k]);
            txt(
                p,
                name,
                12.,
                false,
                tac::LABEL,
                0.8,
                (center[0] + 6. * k, center[1] - rad * f + 9. * k),
                Align::Left,
            );
        }

        // ---- the sector names, outside the rim ----------------------------------------------
        for (bi, b) in books.iter().enumerate() {
            let mid = (bi as f32 + 0.5) * span;
            let q = at(center, mid, rad + 14. * k);
            let waits = list.iter().any(|s| &s.codebase == b && s.waits());
            let members = list.iter().filter(|s| &s.codebase == b).count();
            let text = format!("{} {}", dots(&b.to_uppercase(), 22), members);
            let align = if mid.to_radians().sin() > 0.3 {
                Align::Left
            } else if mid.to_radians().sin() < -0.3 {
                Align::Right
            } else {
                Align::Center
            };
            let wd = txt(
                p,
                &text,
                12.,
                false,
                if waits { tac::AMBER } else { tac::LABEL },
                1.,
                (q[0], q[1]),
                align,
            );
            let x0 = match align {
                Align::Left => q[0],
                Align::Right => q[0] - wd,
                Align::Center => q[0] - wd / 2.,
            };
            placed.push([x0, q[1] - 9. * k, wd, 18. * k]);
            if let Some(s) = list
                .iter()
                .find(|s| &s.codebase == b && s.waits())
                .or_else(|| list.iter().find(|s| &s.codebase == b))
            {
                hits.push((Hit::Pick(s.id.clone()), [x0, q[1] - 9. * k, wd, 18. * k]));
            }
        }

        // ---- the contacts -------------------------------------------------------------------
        let pts: Vec<[f32; 2]> = (0..list.len())
            .map(|i| at(center, spots[i].deg, rad * spots[i].range))
            .collect();
        let mut order: Vec<usize> = (0..list.len()).collect();
        // Draw the quiet ones first; the ones that need you lie on top and place their names first.
        order.sort_by_key(|&i| (!list[i].waits(), list[i].state != SessionState::Working, i));
        for i in order {
            let (s, q) = (&list[i], pts[i]);
            let color = state_color(s.state);
            let picked = pick.as_deref() == Some(s.id.as_str());
            if s.waits() {
                p.dot(q, 14. * k, color, 0.16);
                p.dot(q, 7. * k, color, 1.);
            } else if s.state == SessionState::Working {
                let halo = 10. + (s.rate / 3000.).clamp(0., 1.) * 8.;
                p.dot(q, halo * k, color, 0.12);
                p.dot(q, 6. * k, color, 1.);
            } else {
                p.dot(q, 5. * k, color, 0.9);
            }
            if picked {
                p.arc(q, 18. * k, (0., TAU), 1.5, tac::INK, 1., &id);
            }
            hits.push((
                Hit::Pick(s.id.clone()),
                [q[0] - 16. * k, q[1] - 16. * k, 32. * k, 32. * k],
            ));
        }
        // The names: the ones that need you, the working ones and the picked one first.
        let mut named: Vec<usize> = (0..list.len()).collect();
        named.sort_by_key(|&i| {
            (
                pick.as_deref() != Some(list[i].id.as_str()),
                !list[i].waits(),
                list[i].state != SessionState::Working,
                i,
            )
        });
        for i in named {
            let (s, q) = (&list[i], pts[i]);
            let picked = pick.as_deref() == Some(s.id.as_str());
            let important = picked || s.waits() || s.state == SessionState::Working;
            let mut name = s.title.to_uppercase();
            if s.waits() {
                name = format!(
                    "{}  {}",
                    dots(&name, 26),
                    span_text((s.minutes * 60_000.) as i64)
                );
            }
            let wd = width_of(p, &name, 12., picked);
            let (th, off) = (16. * k, 10. * k);
            let tries = [
                [q[0] + off, q[1] - th / 2., wd, th],
                [q[0] - off - wd, q[1] - th / 2., wd, th],
                [q[0] - wd / 2., q[1] - off - th, wd, th],
                [q[0] - wd / 2., q[1] + off, wd, th],
                [q[0] + off, q[1] - off - th, wd, th],
                [q[0] - off - wd, q[1] + off, wd, th],
            ];
            let free = |t: &[f32; 4], placed: &[[f32; 4]]| {
                t[0] >= ox + 4.
                    && t[0] + t[2] <= ox + w - 4.
                    && t[1] >= oy + 4.
                    && t[1] + t[3] <= oy + h - 4.
                    && pts.iter().enumerate().all(|(j, b)| {
                        j == i
                            || b[0] + 9. * k < t[0]
                            || b[0] - 9. * k > t[0] + t[2]
                            || b[1] + 9. * k < t[1]
                            || b[1] - 9. * k > t[1] + t[3]
                    })
                    && placed.iter().all(|o| {
                        t[0] > o[0] + o[2] + 4.
                            || o[0] > t[0] + t[2] + 4.
                            || t[1] > o[1] + o[3] + 1.
                            || o[1] > t[1] + t[3] + 1.
                    })
            };
            let Some(t) = tries.iter().find(|t| free(t, &placed)) else {
                continue;
            };
            if !important && (s.minutes > 30. || list.len() > 12) {
                // A crowded scope names only what matters.
                continue;
            }
            p.rect((t[0] - 3., t[1], t[2] + 6., t[3]), tac::BG, 0.78);
            let text_color = if picked {
                tac::INK
            } else if s.waits() {
                tac::AMBER
            } else {
                tac::TEXT
            };
            txt(
                p,
                &name,
                12.,
                picked,
                text_color,
                if important { 1. } else { 0.75 },
                (t[0], t[1] + t[3] / 2.),
                Align::Left,
            );
            placed.push(*t);
        }
        if list.is_empty() {
            txt(
                p,
                "NO LIVE SESSIONS",
                20.,
                false,
                tac::LABEL,
                1.,
                (center[0], center[1] + 40. * k),
                Align::Center,
            );
        }

        // ---- the legend (bottom) ------------------------------------------------------------
        let ly = oy + h - 12. * k;
        let mut x = ox + 4.;
        for (color, name) in [
            (tac::AMBER, "NEEDS YOU"),
            (tac::CYAN, "WORKING"),
            (tac::MID, "IDLE"),
            (tac::ALARM, "FAULT"),
        ] {
            p.dot([x + 4. * k, ly], 4. * k, color, 1.);
            let wd = txt(
                p,
                name,
                12.,
                false,
                tac::LABEL,
                1.,
                (x + 14. * k, ly),
                Align::Left,
            );
            x += 14. * k + wd + 22. * k;
        }
        let note = "DISTANCE = TIME SINCE THE LAST MOVE   /   SECTOR = CODEBASE";
        if width_of(p, note, 12., false) + x + 24. * k < w {
            txt(
                p,
                note,
                12.,
                false,
                tac::DIM,
                1.,
                (ox + w - 4., ly),
                Align::Right,
            );
        }
    })
}

/// A message of the chat.
struct Msg {
    you: bool,
    text: String,
}

/// The last `n` messages of the chat: what the owner asked and what the agent said. Tool
/// calls, code and output are not chat.
fn chat(p: &Pane, n: usize) -> Vec<Msg> {
    let mut out: Vec<Msg> = Vec::new();
    for d in last(&p.tail, 80) {
        let you = match d.kind {
            DKind::You => true,
            DKind::Agent | DKind::Heading | DKind::Bullet => false,
            _ => continue,
        };
        let text = d.text.trim();
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(m) if !d.first && m.you == you => {
                m.text.push(' ');
                m.text.push_str(text);
            }
            _ => out.push(Msg {
                you,
                text: text.to_string(),
            }),
        }
    }
    let skip = out.len().saturating_sub(n);
    out.drain(..skip);
    out
}

/// What the session does now, in one short line.
fn doing(p: &Pane) -> String {
    match (p.state, p.ask.as_ref()) {
        (SessionState::NeedsYou | SessionState::Error, Some(a)) => match a.kind {
            AskKind::Permission => format!("asks to run {}", dots(&a.text, 60)),
            AskKind::Plan => "asks you to approve a plan".to_string(),
            AskKind::Question => format!("asks: {}", dots(&a.text, 60)),
            AskKind::Error => format!("error: {}", dots(&a.text, 60)),
        },
        (SessionState::NeedsYou | SessionState::Error, None) => "waits for you".to_string(),
        (SessionState::Working, _) => {
            last_tool(p).map_or_else(|| "works".to_string(), |t| dots(&t, 60))
        }
        _ => "idle".to_string(),
    }
}

/// A state word with its time: `WAITS 6:00`, `WORKS 3:12`, `IDLE 12:40`.
fn state_text(p: &Pane, now: i64) -> String {
    let word = match p.state {
        SessionState::NeedsYou => "WAITS",
        SessionState::Error => "ERROR",
        SessionState::Working => "WORKS",
        _ => "IDLE",
    };
    format!("{word} {}", span_text(now - p.since))
}

impl WarRoomScreen {
    /// The war room in the Tactical look.
    pub(super) fn tactical_render(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        self.tactical.started.get_or_insert(now);
        self.tactical.observe(war, now);
        let k = ui::factor();
        let vh = f32::from(window.viewport_size().height);
        // Whole rows only: the list is what the height allows under the selected panel.
        let free = vh - 44. - 36. - 48. - 160. * k - 430. * k - 60. * k;
        let rows = ((free / (55. * k)) as usize).clamp(3, 8);
        self.tactical_overview(war, rows, cx)
    }

    fn tactical_overview(
        &mut self,
        war: &WarRoom,
        rows: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            .filter(|id| list.iter().any(|s| &s.id == id))
            .or_else(|| queue.first().map(|p| p.id.clone()))
            .or_else(|| list.first().map(|s| s.id.clone()));
        let pick = pick_id.as_ref().and_then(|id| war.session(id));
        let paint = radar(Scope {
            list,
            books,
            spots,
            pick: pick_id.clone(),
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

        let k = ui::factor();
        let selected = Self::tactical_selected(pick, war.now, cx);
        let sessions = Self::tactical_sessions(war, pick_id.as_deref(), rows, cx);
        let budget = Self::tactical_budget(war);

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(c(tac::BG, 1.))
            .font_family(tac::MONO)
            .text_color(c(tac::INK, 1.))
            .px(px(28.))
            .pt(px(18.))
            .pb(px(20.))
            .flex()
            .gap(px(28.))
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
            .child(div().flex_1().min_w(px(0.)).relative().child(picture))
            .child(
                div()
                    .flex_none()
                    .w(px(COLUMN * (1. + (k - 1.) * 0.5)))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .child(selected)
                    .child(sessions)
                    .child(budget),
            )
            .into_any_element()
    }

    /// SELECTED: the session in the middle of the screen. What it says, what it does now, the
    /// request that waits for you with its buttons, and Focus.
    fn tactical_selected(pick: Option<&Pane>, now: i64, cx: &mut Context<Self>) -> Div {
        let panel = tbox("SEC-01 / SELECTED")
            .flex_1()
            .min_h(px(250. * ui::factor()))
            .p(px(22.))
            .pt(px(26.))
            .gap(px(12.));
        let Some(pane) = pick else {
            return panel
                .child(tac::section("NO SESSION"))
                .child(tac::entry("Start a session with New session in the bar."));
        };
        let color = state_color(pane.state);
        let (open_id, park_id) = (pane.id.clone(), pane.id.clone());
        let keys = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(tac::order("tac-park", "PARK", Btn::Plain).on_click(
                cx.listener(move |this, _, _, cx| this.act(Action::Park(park_id.clone()), cx)),
            ))
            .child(
                tac::order("tac-focus", "FOCUS >", Btn::On).on_click(cx.listener(
                    move |_this, _, _, cx| {
                        cx.emit(crate::warroom::WarRoomEvent::OpenSession(open_id.clone()));
                    },
                )),
            );
        let mut panel = panel
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .font_family(tac::SANS)
                            .font_weight(FontWeight::BOLD)
                            .text_size(ui::ts(tac::S_SECTION))
                            .line_height(ui::ts(tac::S_SECTION) * 1.25)
                            .text_color(c(tac::INK, 1.))
                            .line_clamp(2)
                            .child(if pane.title.is_empty() {
                                pane.id.clone()
                            } else {
                                pane.title.clone()
                            }),
                    )
                    .child(keys),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(tac::dot(color))
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui::ts(tac::S_LABEL))
                            .text_color(c(color, 1.))
                            .child(state_text(pane, now)),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(ui::ts(tac::S_LABEL))
                            .text_color(c(tac::LABEL, 1.))
                            .child(format!(
                                "{} · {} · {}",
                                pane.codebase.to_uppercase(),
                                pane.provider.to_uppercase(),
                                pane.model.to_uppercase()
                            )),
                    ),
            );
        if let Some(ask) = pane.ask.as_ref() {
            let kind = match ask.kind {
                AskKind::Permission => "PERMISSION",
                AskKind::Plan => "PLAN",
                AskKind::Question => "QUESTION",
                AskKind::Error => "ERROR",
            };
            let mut req = div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(8.))
                .pl(px(12.))
                .border_l(px(2.))
                .border_color(c(color, 1.))
                .child(
                    label(format!("{} ASKS · {kind}", pane.provider.to_uppercase()))
                        .text_color(c(color, 1.)),
                );
            req = match ask.kind {
                AskKind::Permission => req.child(tac::code(format!("$ {}", dots(&ask.text, 140)))),
                AskKind::Plan => req
                    .child(tac::entry("Approve the plan?"))
                    .child(tac::code(dots(&ask.steps.join("  >  "), 140))),
                _ => req.child(tac::entry(dots(&ask.text, 200)).line_clamp(4)),
            };
            let mut orders = div().flex().flex_wrap().gap(px(8.));
            for (i, option) in ask.options.iter().enumerate() {
                let (sid, aid, opt) = (pane.id.clone(), ask.id.clone(), option.clone());
                let kind = if i == 0 && !ask.risky {
                    Btn::On
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
            panel = panel.child(req.child(orders));
        } else {
            panel = panel.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(label("NOW"))
                    .child(tac::code(doing(pane))),
            );
        }
        // The chat: the last words of the owner and of the agent, in the text font.
        let has_ask = pane.ask.is_some();
        let msgs = chat(pane, if has_ask { 1 } else { 3 });
        let mut talk = div()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(label("CHAT"));
        if msgs.is_empty() {
            talk = talk.child(label("NO MESSAGES YET"));
        }
        for m in msgs {
            talk = talk.child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(46. * ui::factor()))
                            .pt(px(3.))
                            .text_size(ui::ts(tac::S_LABEL))
                            .text_color(c(if m.you { tac::CYAN } else { tac::LABEL }, 1.))
                            .child(if m.you { "YOU" } else { "AGENT" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .font_family(tac::SANS)
                            .text_size(ui::ts(tac::S_ENTRY))
                            .line_height(ui::ts(tac::S_ENTRY) * 1.4)
                            .text_color(c(tac::TEXT, 1.))
                            .line_clamp(if has_ask { 4 } else { 3 })
                            .child(m.text),
                    ),
            );
        }
        panel.child(talk)
    }

    /// SESSIONS: one row per live session with its latest action. The ones that need you first.
    fn tactical_sessions(
        war: &WarRoom,
        pick: Option<&str>,
        rows: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
        let rank = |s: SessionState| match s {
            SessionState::NeedsYou | SessionState::Error => 0,
            SessionState::Working => 1,
            _ => 2,
        };
        live.sort_by(|a, b| {
            rank(a.state)
                .cmp(&rank(b.state))
                .then(a.since.cmp(&b.since))
                .then(a.id.cmp(&b.id))
        });
        let total = live.len();
        let need = live.iter().filter(|s| s.ask.is_some()).count();
        let caption = if need > 0 {
            format!("SEC-02 / SESSIONS · {total} LIVE · {need} NEED YOU")
        } else {
            format!("SEC-02 / SESSIONS · {total} LIVE")
        };
        let mut panel = tbox(caption)
            .flex_none()
            .py(px(12.))
            .pt(px(16.))
            .gap(px(0.));
        if live.is_empty() {
            panel = panel.child(div().px(px(20.)).child(label("NO LIVE SESSIONS")));
        }
        let hidden = total.saturating_sub(rows);
        for (i, s) in live.into_iter().take(rows).enumerate() {
            let on = pick == Some(s.id.as_str());
            let color = state_color(s.state);
            let sid = s.id.clone();
            let title = if s.title.is_empty() {
                s.id.clone()
            } else {
                s.title.clone()
            };
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("tac-row-{i}")))
                    .flex_none()
                    .px(px(18.))
                    .py(px(5.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .cursor_pointer()
                    .border_l(px(2.))
                    .border_color(c(tac::CYAN, if on { 1. } else { 0. }))
                    .bg(c(tac::CYAN, if on { 0.08 } else { 0. }))
                    .hover(|h| h.bg(c(tac::INK, 0.05)))
                    .child(tac::dot(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .font_family(tac::SANS)
                                    .text_size(ui::ts(tac::S_ENTRY))
                                    .line_height(ui::ts(tac::S_ENTRY) * 1.3)
                                    .text_color(c(if on { tac::INK } else { tac::TEXT }, 1.))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(ui::ts(tac::S_LABEL))
                                    .text_color(c(tac::LABEL, 1.))
                                    .child(doing(s)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui::ts(tac::S_LABEL))
                            .text_color(c(color, 1.))
                            .child(span_text(war.now - s.since)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tactical.pick = Some(sid.clone());
                        cx.notify();
                    })),
            );
        }
        if hidden > 0 {
            panel = panel.child(
                div()
                    .px(px(20.))
                    .pt(px(4.))
                    .child(label(format!("+{hidden} MORE ON THE SCOPE"))),
            );
        }
        panel
    }

    /// BUDGET: the usage windows as thin bars, and the spend of the live sessions.
    fn tactical_budget(war: &WarRoom) -> Div {
        let live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
        let spend: f64 = live.iter().map(|s| s.cost).sum();
        let tokens: u64 = live.iter().map(|s| s.tokens).sum();
        let mut panel = tbox("SEC-03 / BUDGET")
            .flex_none()
            .p(px(16.))
            .pt(px(20.))
            .gap(px(8.));
        for u in war.usage.iter().take(3) {
            let pct = f32::from(u.pct).min(100.);
            let color = if pct >= 95. {
                tac::ALARM
            } else if pct >= 75. {
                tac::AMBER
            } else {
                tac::CYAN
            };
            panel = panel.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .text_size(ui::ts(tac::S_LABEL))
                    .child(
                        div()
                            .flex_none()
                            .w(px(130. * ui::factor()))
                            .truncate()
                            .text_color(c(tac::LABEL, 1.))
                            .child(format!(
                                "{} · {}",
                                dots(&u.provider.to_uppercase(), 8),
                                win_tag(&u.label)
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(6.))
                            .bg(c(tac::DEEP, 1.))
                            .child(div().h_full().w(relative(pct / 100.)).bg(c(color, 1.))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(38. * ui::factor()))
                            .text_color(c(if pct >= 75. { color } else { tac::TEXT }, 1.))
                            .child(format!("{}%", u.pct)),
                    ),
            );
        }
        panel.child(
            div()
                .flex_none()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .child(
                    div()
                        .font_family(tac::SANS)
                        .font_weight(FontWeight::BOLD)
                        .text_size(ui::ts(tac::S_SECTION))
                        .text_color(c(tac::INK, 1.))
                        .child(format!("${spend:.2}")),
                )
                .child(label(format!(
                    "LIVE SPEND · {} SESSIONS · {} TOK",
                    live.len(),
                    compact(tokens)
                ))),
        )
    }
}

/// A short name of a usage window: `5H`, `7D`, `1D`, `MONTH`.
fn win_tag(label: &str) -> String {
    let l = label.to_lowercase();
    if l.contains("5 h") || l.contains("5h") {
        "5H".to_string()
    } else if l.contains("week") || l.contains("7 d") {
        "7D".to_string()
    } else if l.contains("day") || l.contains("daily") {
        "1D".to_string()
    } else if l.contains("month") {
        "MONTH".to_string()
    } else {
        dots(&l.split_whitespace().next().unwrap_or("").to_uppercase(), 5)
    }
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
