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
use super::synapse::{self, tone};
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
        // A word longer than a line (a path) breaks into pieces.
        let mut words: Vec<String> = Vec::new();
        for word in t.split_whitespace() {
            let chars: Vec<char> = word.chars().collect();
            for piece in chars.chunks(width.max(8) - 2) {
                words.push(piece.iter().collect());
            }
        }
        for word in words.iter().map(String::as_str) {
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
/// The width of a label column at each side of the synapse network.
const TAG_W: f32 = 140.;
const FOOT: f32 = 100.;

/// The bloom for a window of `w` x `h` pixels (under the bar).
fn bloom_geo(w: f32, h: f32) -> Bloom {
    let cw = (w - 2. * (MARGIN + column(w) + GAP)).max(240.);
    let ch = (h - TOP - BOTTOM).max(300.);
    let avail = (ch - HEAD - FOOT).max(200.);
    // The sides keep room for the label columns (TAG_W): the network stays between them.
    let s = ((cw / 2. - 28. - 0.6 * TAG_W) / 292.)
        .min((avail / 2. - 6.) / 292.)
        .clamp(0.35, 1.15);
    Bloom {
        cw,
        ch,
        c: [cw / 2., HEAD + avail / 2.],
        s,
    }
}

fn at(c: [f32; 2], a: f32, r: f32) -> [f32; 2] {
    [c[0] + a.cos() * r, c[1] + a.sin() * r]
}

// ---- the picture --------------------------------------------------------------------------

/// The data that the canvas needs.
struct Picture {
    list: Vec<Contact>,
    syn: std::rc::Rc<synapse::Layout>,
    /// The firings on screen: contact index, start time.
    fires: Vec<(usize, f32)>,
    /// Agents at work and agents that wait, one value per minute.
    activity: Vec<(u32, u32)>,
    t: f32,
    cam: synapse::Cam,
}

impl Picture {
    fn paint(&self, p: &mut Painter, r: [f32; 4]) {
        let (w, h) = (r[2], r[3]);
        let g = bloom_geo(w, h);
        let (ox, oy) = (r[0] + MARGIN + column(w) + GAP, r[1] + TOP);

        let s = g.s;

        synapse::paint(
            p,
            &self.syn,
            &self.list,
            &self.fires,
            self.t,
            self.cam,
            [ox, oy],
            s,
        );

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
        let syn = self.holo.syn.get(&list, &books, g.cw, g.ch, g.s);
        let cam = self.holo.orbit.cam();
        // A session that starts to work, waits or fails fires once; nothing moves otherwise.
        let demo = self.model.read(cx).demo;
        {
            let st = &mut self.holo;
            for c in &list {
                // A firing: a new state (works, waits, error), or new output from a working
                // session, at most once per 8 s per session.
                let before = st.seen.get(&c.id).copied();
                let last = before.map_or(f32::MIN, |b| b.2);
                let changed = before.is_some_and(|b| b.0 != c.state)
                    && matches!(
                        c.state,
                        SessionState::Working | SessionState::NeedsYou | SessionState::Error
                    );
                let output = before.is_some_and(|b| c.tokens > b.1)
                    && c.state == SessionState::Working
                    && t - last > 8.;
                let fired = changed || output;
                if fired {
                    st.fires.push((c.id.clone(), t));
                }
                st.seen.insert(
                    c.id.clone(),
                    (c.state, c.tokens, if fired { t } else { last }),
                );
            }
            // The demo has no live output: one working session fires every 6 s in turn.
            if demo {
                let working: Vec<&String> = list
                    .iter()
                    .filter(|c| c.state == SessionState::Working)
                    .map(|c| &c.id)
                    .collect();
                let slot = (t / 6.).floor();
                if let Some(id) = working.get(slot as usize % working.len().max(1))
                    && let Some(seen) = st.seen.get_mut(*id)
                    && seen.2 < slot * 6.
                {
                    seen.2 = t;
                    st.fires.push(((*id).clone(), t));
                }
            }
            st.fires.retain(|(_, start)| t - start < synapse::FIRE_TIME);
            let extra = st.fires.len().saturating_sub(3);
            st.fires.drain(..extra);
        }
        let fires: Vec<(usize, f32)> = self
            .holo
            .fires
            .iter()
            .filter_map(|(id, start)| list.iter().position(|c| &c.id == id).map(|i| (i, *start)))
            .collect();
        let picture = Picture {
            list: list.clone(),
            syn: syn.clone(),
            fires,
            activity,
            t,
            cam,
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

        // Drag turns the synapse, the wheel zooms it.
        let mut center = div()
            .relative()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, _, _| {
                    this.holo.orbit.drag = Some([f32::from(e.position.x), f32::from(e.position.y)]);
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                let Some(from) = this.holo.orbit.drag else {
                    return;
                };
                if e.pressed_button != Some(gpui::MouseButton::Left) {
                    this.holo.orbit.drag = None;
                    return;
                }
                let at = [f32::from(e.position.x), f32::from(e.position.y)];
                this.holo.orbit.turn(at[0] - from[0], at[1] - from[1]);
                this.holo.orbit.drag = Some(at);
                cx.notify();
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, _| {
                    this.holo.orbit.drag = None;
                }),
            )
            .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, window, cx| {
                let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
                this.holo.orbit.wheel(-dy);
                cx.notify();
            }));
        center = center
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .child(th("Synapse network").text_color(rgb(PA)))
                    .child(
                        div()
                            .mt(px(4.))
                            .child(mono(
                                "one region per codebase, one neuron per session",
                                0x5F95B8,
                            ))
                            .child(mono(
                                "a working session fires: pulse in, wave out",
                                0x5F95B8,
                            )),
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
        let spots: Vec<[f32; 2]> = (0..syn.nodes.len()).map(|i| syn.node_at(i, cam)).collect();
        // Codebase labels open their session. They stand in two columns at the sides of the
        // region, sorted by the height of their cluster, at least one row apart, so no two
        // labels ever overlap; a thin leader line runs from each label to its cluster.
        struct Tag {
            k: usize,
            anchor: [f32; 2],
            y: f32,
        }
        let (mut left_tags, mut right_tags): (Vec<Tag>, Vec<Tag>) = (Vec::new(), Vec::new());
        for (k, cl) in syn.clusters.iter().enumerate() {
            if cl.nodes.is_empty() {
                continue;
            }
            let n = cl.nodes.len() as f32;
            let (sx, sy) = cl.nodes.iter().fold((0., 0.), |(ax, ay), &i| {
                let p = spots[i];
                (ax + p[0], ay + p[1])
            });
            let anchor = [sx / n, sy / n];
            let tag = Tag {
                k,
                anchor,
                y: anchor[1],
            };
            if anchor[0] < g.c[0] {
                left_tags.push(tag);
            } else {
                right_tags.push(tag);
            }
        }
        const TAG_ROW: f32 = 30.;
        let (top, bottom) = (HEAD + 18., (g.ch - FOOT - 12.).max(HEAD + 60.));
        let spread = |tags: &mut Vec<Tag>| {
            tags.sort_by(|a, b| a.anchor[1].total_cmp(&b.anchor[1]));
            let mut prev = top - TAG_ROW;
            for t in tags.iter_mut() {
                t.y = t.anchor[1].max(prev + TAG_ROW).max(top);
                prev = t.y;
            }
            // Too many for the room below: push the column up from the bottom.
            let mut next = bottom + TAG_ROW;
            for t in tags.iter_mut().rev() {
                t.y = t.y.min(next - TAG_ROW);
                next = t.y;
            }
        };
        spread(&mut left_tags);
        spread(&mut right_tags);
        let mut leaders: Vec<([f32; 2], [f32; 2], bool)> = Vec::new();
        let mut tag_els: Vec<gpui::Stateful<Div>> = Vec::new();
        for (tags, left) in [(&left_tags, true), (&right_tags, false)] {
            for tag in tags.iter() {
                let cl = &syn.clusters[tag.k];
                let target = cl
                    .nodes
                    .iter()
                    .copied()
                    .find(|&n| list[syn.nodes[n].idx].waits())
                    .or(cl.nodes.first().copied());
                let Some(target) = target.and_then(|n| list.get(syn.nodes[n].idx)) else {
                    continue;
                };
                let id = target.id.clone();
                let waits = cl.nodes.iter().any(|&n| list[syn.nodes[n].idx].waits());
                // The label hugs the outer edge; its line starts at the end of the text.
                let text_w = 10. + 7.4 * (clip(&cl.name, 16).chars().count() as f32 + 3.);
                let x = if left { 0. } else { g.cw - TAG_W };
                let end = if left {
                    text_w.min(TAG_W) + 2.
                } else {
                    g.cw - text_w.min(TAG_W) - 2.
                };
                leaders.push(([end, tag.y], tag.anchor, waits));
                tag_els.push(
                    div()
                        .id(SharedString::from(format!("holo-book-{}", tag.k)))
                        .absolute()
                        .left(px(x))
                        .top(px(tag.y - 12.))
                        .w(px(TAG_W))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .when(!left, |d| d.justify_end())
                        .cursor_pointer()
                        .font_family(LABEL)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_size(px(S_SMALL + 1.))
                        .text_color(ink(if waits { GD } else { PA }, 0.9))
                        .hover(|s| s.text_color(rgb(TXT)))
                        .child(
                            // A dark glass backing: the label stays readable over the network.
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .px(px(5.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .bg(ink(0x020B16, 0.78))
                                .border_1()
                                .border_color(ink(if waits { GD } else { PK }, 0.3))
                                .child(clip(&cl.name, 16))
                                .child(mono(cl.nodes.len().to_string(), 0x5F95B8)),
                        )
                        .on_click(cx.listener(move |_this, _, _, cx| {
                            cx.emit(crate::warroom::WarRoomEvent::OpenSession(id.clone()));
                        })),
                );
            }
        }
        // Zoom: wheel or these buttons; drag turns the network.
        let zoom_btn = |id: &'static str, tip: &'static str| {
            div()
                .id(id)
                .min_w(px(28.))
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(ink(PK, 0.14))
                .border_1()
                .border_color(ink(PK, 0.4))
                .font_family(LABEL)
                .text_size(px(S_SMALL + 1.))
                .text_color(ink(PA, 0.95))
                .cursor_pointer()
                .hover(|s| s.bg(ink(PK, 0.3)).border_color(ink(PK, 0.8)))
                .tooltip(crate::ui::tip(tip))
        };
        let zoom_pct = (self.holo.orbit.zoom * 100.).round() as u32;
        center = center.child(
            div()
                .absolute()
                .left(px(g.c[0] - 90.))
                .top(px((g.ch - FOOT - 40.).max(HEAD + 40.)))
                .w(px(180.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .child(
                    zoom_btn("holo-zoom-out", "Zoom out (wheel down)")
                        .child("−")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.holo.orbit.zoom_by(1. / 1.4);
                            cx.notify();
                        })),
                )
                .child(
                    zoom_btn("holo-zoom-reset", "Back to the start view")
                        .child(format!("{zoom_pct}%"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.holo.orbit.reset();
                            cx.notify();
                        })),
                )
                .child(
                    zoom_btn("holo-zoom-in", "Zoom in (wheel up); drag turns")
                        .child("+")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.holo.orbit.zoom_by(1.4);
                            cx.notify();
                        })),
                ),
        );
        // The leader lines: a short level run from the label, then straight to the cluster.
        center = center.child(
            canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
                    for (from, to, waits) in &leaders {
                        let knee = if from[0] < to[0] {
                            from[0] + 14.
                        } else {
                            from[0] - 14.
                        };
                        let pts = [
                            [from[0] + ox, from[1] + oy],
                            [knee + ox, from[1] + oy],
                            [to[0] + ox, to[1] + oy],
                        ];
                        line(window, &pts, if *waits { GD } else { PK }, 1., 0.45, false);
                    }
                },
            )
            .absolute()
            .inset_0(),
        );
        center = center.children(tag_els);
        // One hit area per neuron: a small tip on hover, a click selects (waits: opens).
        for (i, node) in syn.nodes.iter().enumerate() {
            let con = &list[node.idx];
            let pos = syn.node_at(i, cam);
            let id = con.id.clone();
            let waits = con.waits();
            let group = SharedString::from(format!("holo-syn-{i}"));
            let state = match con.state {
                SessionState::NeedsYou => "waits for you",
                SessionState::Error => "error",
                SessionState::Working => "working",
                _ => "idle",
            };
            let tip = div()
                .absolute()
                .top(px(-10.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(4.))
                .border_1()
                .border_color(ink(tone(con.state), 0.6))
                .bg(ink(BLACK, 0.9))
                .opacity(0.)
                .group_hover(group.clone(), |s| s.opacity(1.))
                .whitespace_nowrap()
                .flex()
                .flex_col()
                .child(
                    div()
                        .font_family(LABEL)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(px(S_SMALL))
                        .text_color(rgb(PA))
                        .child(con.title.clone()),
                )
                .child(mono(
                    format!("{} - {}", con.codebase.to_lowercase(), state),
                    0x7FB0CF,
                ));
            let tip = if pos[0] > g.cw / 2. {
                tip.right(px(24.))
            } else {
                tip.left(px(24.))
            };
            center = center.child(
                div()
                    .id(SharedString::from(format!("holo-tip-{i}")))
                    .group(group)
                    .absolute()
                    .left(px(pos[0] - 13.))
                    .top(px(pos[1] - 13.))
                    .size(px(26.))
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|s| s.bg(ink(PK, 0.14)))
                    .child(tip)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if waits {
                            cx.emit(crate::warroom::WarRoomEvent::OpenSession(id.clone()));
                        } else {
                            this.holo.sel = Some(id.clone());
                        }
                        cx.notify();
                    })),
            );
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
            for l in whole_lines(&why, 28, fit) {
                shown = shown.child(
                    div()
                        .min_h(px(25.))
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
            link("holo-open", "Open session ›", GD).on_click(cx.listener(
                move |_this, _, _, cx| {
                    cx.emit(crate::warroom::WarRoomEvent::OpenSession(open_id.clone()));
                },
            )),
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
                            // A long window name gets an ellipsis; the reset time and the
                            // percent stay inside the card.
                            .child(
                                th(format!("{} {}", u.provider, u.label))
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate(),
                            )
                            .child(mono(resets(u), 0x5F95B8).flex_none().whitespace_nowrap())
                            .child(mono(format!("{}%", u.pct), PA).flex_none()),
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
        let open_id = pane.id.clone();
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
            .child(div().flex().justify_between().child(
                link("holo-sel-open", "Open session ›", PK).on_click(cx.listener(
                    move |_this, _, _, cx| {
                        cx.emit(crate::warroom::WarRoomEvent::OpenSession(open_id.clone()));
                    },
                )),
            ))
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
