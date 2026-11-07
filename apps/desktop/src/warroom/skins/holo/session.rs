//! The Holo session view (board `HoloSession`): the switcher at the left (waits for you first,
//! then working), the transcript in the middle (prompt, verb-pill steps, agent text, and at the
//! bottom the inline decision card or the composer), and at the right the turn bloom (one
//! strand per turn, length = the lines of the turn) with the facts and the files.

use std::f32::consts::{PI, TAU};

use aivory_protocol::{AskKind, LineKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, SharedString, Window, canvas, div, px, rgb};

use super::super::common::{clip, key_label};
use super::super::hud::turn_sizes;
use super::war::{clock, column, kilo};
use crate::actions::Action;
use crate::console::display;
use crate::holo_kit::*;
use crate::warroom::WarRoomScreen;
use crate::warroom::bridge_data::{provider_name, tests_of, turn_of};

/// The width of the switcher.
const SWITCHER: f32 = 272.;

fn rank(p: &Pane) -> u8 {
    match p.state {
        SessionState::NeedsYou | SessionState::Error => 0,
        SessionState::Working => 1,
        _ => 2,
    }
}

/// The sessions of the switcher: waits first (the longest wait first), then the rest by id.
fn ordered(war: &WarRoom) -> Vec<&Pane> {
    let mut list: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
    list.sort_by(|a, b| {
        let key = |p: &Pane| {
            (
                rank(p),
                if rank(p) == 0 { p.since } else { 0 },
                p.id.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    list
}

/// The ids of the switcher in its order.
pub(super) fn switcher_ids(war: &WarRoom) -> Vec<String> {
    ordered(war).iter().map(|p| p.id.clone()).collect()
}

/// The lines of each turn: a turn starts at a turn mark, or at a prompt of the owner when the
/// tail has no marks.
fn turns_of_pane(pane: &Pane) -> Vec<f32> {
    if pane.tail.iter().any(|l| l.kind == LineKind::Turn) {
        return turn_sizes(pane);
    }
    let mut out: Vec<f32> = Vec::new();
    for l in &pane.tail {
        if l.kind == LineKind::User || out.is_empty() {
            out.push(0.);
        }
        if let Some(last) = out.last_mut() {
            *last += 1.;
        }
    }
    out
}

const BLOOM_H: f32 = 224.;

/// The tip of the strand of turn `i` of `n` in a box of `w` x `BLOOM_H`, the angle and the length.
fn turn_tip(i: usize, n: usize, size: f32, max: f32, w: f32) -> ([f32; 2], f32, f32) {
    let c = [w / 2., BLOOM_H / 2.];
    let k = (w.min(BLOOM_H) / 2. - 18.).max(40.) / 124.;
    let a = -PI / 2. + ((i as f32 - (n as f32 - 1.)) / n as f32) * TAU * 0.92;
    let l = (24. + size / max * 100.) * k;
    ([c[0] + a.cos() * l, c[1] + a.sin() * l], a, l)
}

/// The turn bloom: the strands go around from the oldest to the newest, which sits at the top.
/// Every strand has a dot at its tip; `turn_labels` names them.
fn turn_bloom(sizes: Vec<f32>, edge: u32, w: f32) -> Div {
    div().relative().w_full().h(px(BLOOM_H)).child(
        canvas(
            |_, _, _| (),
            move |b, _, window, _| {
                let o = [f32::from(b.origin.x), f32::from(b.origin.y)];
                let c = [o[0] + w / 2., o[1] + BLOOM_H / 2.];
                let k = (w.min(BLOOM_H) / 2. - 18.).max(40.) / 124.;
                for (r, alpha) in [(120. * k, 0.14), (60. * k, 0.08)] {
                    let pts: Vec<[f32; 2]> = (0..48)
                        .map(|i| {
                            let a = TAU * i as f32 / 48.;
                            [c[0] + a.cos() * r, c[1] + a.sin() * r]
                        })
                        .collect();
                    line(window, &pts, PK, 1., alpha, true);
                }
                let n = sizes.len().max(1);
                let max = sizes.iter().copied().fold(1., f32::max);
                for (i, size) in sizes.iter().enumerate() {
                    let (tip, a, l) = turn_tip(i, n, *size, max, w);
                    let end = [o[0] + tip[0], o[1] + tip[1]];
                    let ctrl = [
                        c[0] + (a + 0.18).cos() * l * 0.5,
                        c[1] + (a + 0.18).sin() * l * 0.5,
                    ];
                    let pts: Vec<[f32; 2]> = (0..=10)
                        .map(|j| {
                            let f = j as f32 / 10.;
                            let u = 1. - f;
                            [
                                u * u * c[0] + 2. * u * f * ctrl[0] + f * f * end[0],
                                u * u * c[1] + 2. * u * f * ctrl[1] + f * f * end[1],
                            ]
                        })
                        .collect();
                    let newest = i + 1 == n;
                    let (col, width, alpha) = if newest {
                        (edge, 2.6, 1.)
                    } else if i + 5 >= n {
                        (PE, 1.8, 0.85)
                    } else {
                        (PK, 1.4, 0.65)
                    };
                    if newest {
                        glow_line(window, &pts, col, width, false);
                    } else {
                        line(window, &pts, col, width, alpha, false);
                    }
                    let r = if newest { 4. } else { 3. };
                    let dot: Vec<[f32; 2]> = (0..12)
                        .map(|j| {
                            let a = TAU * j as f32 / 12.;
                            [end[0] + a.cos() * r, end[1] + a.sin() * r]
                        })
                        .collect();
                    line(window, &dot, col, r * 2., alpha, true);
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

/// The labels of the strands (turn number and lines) at the tips of the newest twelve turns.
/// No turns give a calm note.
fn turn_labels(sizes: &[f32], w: f32) -> Vec<Div> {
    if sizes.is_empty() {
        return vec![
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(th("No turns yet")),
        ];
    }
    let n = sizes.len();
    let max = sizes.iter().copied().fold(1., f32::max);
    (n.saturating_sub(12)..n)
        .map(|i| {
            let (tip, a, _) = turn_tip(i, n, sizes[i], max, w);
            let at = [tip[0] + a.cos() * 24., tip[1] + a.sin() * 14.];
            div()
                .absolute()
                .left(px(at[0] - 28.))
                .top(px(at[1] - 8.))
                .w(px(56.))
                .flex()
                .justify_center()
                .font_family(CODE)
                .text_size(px(S_SMALL - 2.))
                .text_color(rgb(if i + 1 == n { PA } else { 0x7FB0CF }))
                .whitespace_nowrap()
                .child(format!("{} · {}", i + 1, sizes[i]))
        })
        .collect()
}

fn mono(text: impl Into<SharedString>, color: u32, size: f32) -> Div {
    div()
        .font_family(CODE)
        .text_size(px(size))
        .line_height(px(size + 7.))
        .text_color(rgb(color))
        .child(text.into())
}

impl WarRoomScreen {
    pub(super) fn holo_session(
        &mut self,
        war: &WarRoom,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pane) = war.session(id).cloned() else {
            return div().into_any_element();
        };
        self.ensure_diff(id, cx);
        let files: Vec<(String, u32, u32)> = self
            .diffs
            .get(id)
            .and_then(|(d, _)| d.as_ref())
            .map(|d| {
                d.files
                    .iter()
                    .map(|f| (f.path.clone(), f.added, f.removed))
                    .collect()
            })
            .unwrap_or_default();
        let vp = window.viewport_size();
        let w = f32::from(vp.width);
        let hot = pane.ask.is_some();
        let edge = if hot { OR } else { PK };

        // ---- left: the switcher -------------------------------------------------------------
        let list = ordered(war);
        let mut rows = div()
            .id("holo-switcher")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.));
        let mut last_rank = u8::MAX;
        for p in &list {
            if rank(p) != last_rank {
                last_rank = rank(p);
                rows = rows.child(
                    th(match last_rank {
                        0 => "Waits for you",
                        1 => "Working",
                        _ => "Idle",
                    })
                    .flex_none()
                    .px(px(12.))
                    .pt(px(if last_rank == 0 { 0. } else { 20. }))
                    .pb(px(8.))
                    .text_color(rgb(if last_rank == 0 { GD } else { MUTED })),
                );
            }
            let on = p.id == id;
            let color = state_color(p);
            let sid = p.id.clone();
            let note = if rank(p) == 0 {
                clock(war.now, p.since)
            } else {
                kilo(p.tokens)
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("holo-c-{}", p.id)))
                    .flex_none()
                    .h(px(40.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(ink(PK, 0.12)).border_l_2().border_color(rgb(PK))
                    })
                    .hover(|s| s.bg(ink(PK, 0.07)))
                    .child(div().flex_none().size(px(8.)).rounded_full().bg(rgb(color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(S_BODY))
                            .text_color(ink(TXT, if on { 1. } else { 0.75 }))
                            .child(if p.title.is_empty() {
                                p.id.clone()
                            } else {
                                p.title.clone()
                            }),
                    )
                    .child(mono(
                        note,
                        if rank(p) == 0 { 0xE0A37A } else { 0x5F95B8 },
                        S_SMALL - 2.,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.holo.open = Some(sid.clone());
                        cx.notify();
                    })),
            );
        }
        let left = panel("Sessions", false)
            .flex_none()
            .w(px(SWITCHER))
            .h_full()
            .px(px(PAD - 12.))
            .gap(px(14.))
            .child(
                link("holo-back", "‹ War room", PK)
                    .ml(px(12.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.holo.open = None;
                        cx.notify();
                    })),
            )
            .child(rows)
            .child(mono("Alt+J / Alt+K  switch", 0x4F7FA0, S_SMALL - 2.).px(px(12.)));

        // ---- middle: the transcript ---------------------------------------------------------
        let goal = pane.prompt.lines().next().unwrap_or("").trim().to_string();
        let mut meta = provider_name(&pane.provider);
        if !pane.model.is_empty() {
            meta.push_str(&format!(" · {}", clip(&pane.model, 20)));
        }
        if let Some(turn) = turn_of(&pane) {
            meta.push_str(&format!(" · turn {turn}"));
        }
        meta.push_str(&format!(" · {} m", pane.minutes(war.now)));
        let (park_id, focus_id) = (pane.id.clone(), pane.id.clone());
        let head = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family(LABEL)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(S_TITLE - 4.))
                            .line_height(px(S_TITLE + 2.))
                            .text_color(rgb(TXT))
                            .child(if pane.title.is_empty() {
                                format!("{}/{}", pane.codebase, pane.id)
                            } else {
                                format!("{}/{}", pane.codebase, pane.title)
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .gap(px(18.))
                            .child(link("holo-focus", "Focus ›", PK).on_click(
                                cx.listener(move |this, _, _, cx| this.skin_focus(&focus_id, cx)),
                            ))
                            .child(link("holo-park", "Park", MUTED).on_click(cx.listener(
                                move |this, _, _, cx| this.act(Action::Park(park_id.clone()), cx),
                            ))),
                    ),
            )
            .child(mono(meta, 0x5F95B8, S_SMALL - 1.))
            .child(
                div()
                    .mt(px(2.))
                    .max_h(px(LH_BODY * 2.))
                    .overflow_hidden()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .line_height(px(LH_BODY))
                    .text_size(px(S_LEAD))
                    .text_color(rgb(PA))
                    .child(clip(&goal, 140)),
            );
        let rule = div()
            .flex_none()
            .h(px(1.))
            .w_full()
            .bg(gpui::linear_gradient(
                90.,
                gpui::linear_color_stop(ink(PK, 0.35), 0.),
                gpui::linear_color_stop(ink(PK, 0.), 1.),
            ));

        // About as many display lines as the panel holds; the list starts at a message.
        let n = (((f32::from(vp.height) - BAR_H - 520.) / 27.) as usize).clamp(4, 40);
        let mut chat = div()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .flex()
            .flex_col();
        let rows = display_rows(&display::last(&pane.tail, n));
        if rows.is_empty() {
            chat = chat.child(th("No rows yet"));
        }
        chat = chat.children(rows);

        let footer: AnyElement = match pane.ask.as_ref() {
            Some(ask) => {
                let n = ask.options.len();
                let mut buttons = div().flex().flex_wrap().gap(px(10.));
                if pane.controlled {
                    for (i, option) in ask.options.iter().enumerate() {
                        let (sid, aid, opt) = (pane.id.clone(), ask.id.clone(), option.clone());
                        let tone = match option_color(i, n) {
                            c if c == OR => Tone::Primary,
                            c if c == GY => Tone::Ghost,
                            _ => Tone::Plain,
                        };
                        buttons = buttons.child(
                            button(
                                format!("holo-s-ans-{i}"),
                                tone,
                                &format!("{}  {}", i + 1, key_label(option)),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.skin_answer(&sid, &aid, &opt, cx),
                            )),
                        );
                    }
                } else {
                    buttons = buttons.child(th("Answer it in its terminal"));
                }
                let headline = match ask.kind {
                    AskKind::Permission => {
                        format!("Allow {} to run this?", provider_name(&pane.provider))
                    }
                    AskKind::Plan => "Approve the plan?".to_string(),
                    _ => clip(ask.text.lines().next().unwrap_or(""), 110),
                };
                let cmd = match ask.kind {
                    AskKind::Permission | AskKind::Plan => {
                        clip(ask.text.lines().next().unwrap_or(""), 60)
                    }
                    _ => String::new(),
                };
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(20.))
                    .border_1()
                    .border_color(ink(OR, 0.45))
                    .bg(ink(OR, 0.08))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(14.))
                            .child(
                                th(format!("Waits for you · {}", clock(war.now, pane.since)))
                                    .text_color(rgb(GD)),
                            )
                            .child(div().min_w(px(0.)).truncate().child(mono(
                                cmd,
                                0xE0A37A,
                                S_SMALL - 2.,
                            ))),
                    )
                    .child(
                        div()
                            .font_family(LABEL)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(S_BIG - 4.))
                            .line_height(px(S_BIG + 2.))
                            .text_color(rgb(WARM))
                            .child(headline),
                    )
                    .child(buttons)
                    .into_any_element()
            }
            None => {
                let input = self.composer_input(&pane, window, cx);
                let (send_id, pause_id) = (pane.id.clone(), pane.id.clone());
                div()
                    .flex_none()
                    .min_h(px(64.))
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .px(px(18.))
                    .py(px(8.))
                    .border_1()
                    .border_color(ink(PK, 0.4))
                    .bg(ink(PK, 0.04))
                    .child(th("Tell the agent"))
                    .child(div().flex_1().min_w(px(0.)).child(input))
                    .child(button("holo-send", Tone::Plain, "Send").on_click(
                        cx.listener(move |this, _, window, cx| this.submit(&send_id, window, cx)),
                    ))
                    .child(
                        button("holo-pause", Tone::Ghost, "Pause").on_click(cx.listener(
                            move |this, _, _, cx| this.act(Action::Pause(pause_id.clone()), cx),
                        )),
                    )
                    .into_any_element()
            }
        };
        let middle = panel(if hot { "Waits for you" } else { "Session" }, hot)
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .gap(px(18.))
            .child(head)
            .child(rule)
            .child(chat)
            .child(footer);

        // ---- right: the turn bloom, the facts and the files ---------------------------------
        let sizes = turns_of_pane(&pane);
        let turns = sizes.len();
        let lines_total: f32 = sizes.iter().sum();
        let bloom_w = column(w) - 2. * PAD;
        let labels = turn_labels(&sizes, bloom_w);
        let bloom = turn_bloom(sizes, edge, bloom_w);
        let tests = tests_of(&pane.tail)
            .map(|(ok, bad)| format!("{ok}/{}", ok + bad))
            .unwrap_or_else(|| "—".to_string());
        let fact = |name: &str, value: String| {
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(th(name.to_string()))
                .child(
                    div()
                        .font_family(FIGURE)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(px(S_BIG - 4.))
                        .text_color(rgb(PA))
                        .child(value),
                )
        };
        let mut file_list = div().flex().flex_col().gap(px(10.));
        for (path, added, removed) in files.iter().take(6) {
            let short = {
                let parts: Vec<&str> = path.rsplit(['/', '\\']).take(2).collect();
                parts.into_iter().rev().collect::<Vec<_>>().join("/")
            };
            file_list = file_list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(mono(
                        short,
                        0xBFE6F7,
                        S_SMALL - 1.,
                    )))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .gap(px(2.))
                            .h(px(4.))
                            .w(px(70.))
                            .child(div().h_full().w(px((*added as f32).min(46.))).bg(rgb(PK)))
                            .child(div().h_full().w(px((*removed as f32).min(24.))).bg(rgb(SA))),
                    ),
            );
        }
        if files.is_empty() {
            file_list = file_list.child(th("No change yet"));
        }
        let col = column(w);
        let right = div()
            .flex_none()
            .w(px(col))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(
                panel("Turns", false)
                    .h(px(310.))
                    .flex_none()
                    .py(px(PAD - 6.))
                    .child(
                        div().relative().child(bloom).children(labels).child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .font_family(FIGURE)
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_size(px(S_BODY))
                                .text_color(rgb(TXT))
                                .child(turns.to_string()),
                        ),
                    )
                    .child(mono(
                        format!("{turns} turns · {lines_total:.0} lines"),
                        0x5F95B8,
                        S_SMALL - 2.,
                    )),
            )
            .child(
                panel("Facts", false)
                    .flex_1()
                    .min_h(px(0.))
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .gap(px(14.))
                            .child(fact("Tokens", kilo(pane.tokens)))
                            .child(fact("Cost", format!("${:.2}", pane.cost)))
                            .child(fact("Tests", tests)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(th("Context"))
                            .child(
                                div().flex_1().h(px(3.)).bg(ink(PK, 0.12)).child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(
                                            f32::from(pane.context_pct.min(100)) / 100.,
                                        ))
                                        .bg(rgb(if pane.context_pct >= 85 { OR } else { PK })),
                                ),
                            )
                            .child(mono(format!("{}%", pane.context_pct), PA, S_SMALL - 1.)),
                    )
                    .child(th("Files"))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_hidden()
                            .child(file_list),
                    ),
            );

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .font_family(BODY)
            .text_color(rgb(TXT))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .gap(px(GAP))
                    .px(px(MARGIN))
                    .pt(px(30.))
                    .pb(px(40.))
                    .child(left)
                    .child(middle)
                    .child(right),
            )
            .into_any_element()
    }
}
