//! The Tactical dossier (board `TacticalSession`): the contacts at the left, the intercept log
//! in the middle with the order request ([1] [2] [3]) or an order input, and the dossier facts,
//! the mission route and the files hit at the right. Every control goes through `act`.

use aivory_protocol::{Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Window, div, px};

use super::{route_paint, route_steps};
use crate::actions::Action;
use crate::tactical_kit::{self as tac, Btn, c, dots, label, tbox};
use crate::ui;
use crate::warroom::WarRoomScreen;
use crate::warroom::skins::common::{key_label, part_of, short_tool, thousands};
use crate::warroom::skins::hud::contacts;

/// The word of a state.
fn state_word(pane: &Pane) -> &'static str {
    match pane.state {
        SessionState::NeedsYou => "AWAITING ORDERS",
        SessionState::Error => "ERROR",
        SessionState::Working => "ACTIVE · MOVING",
        SessionState::Idle => "NO MOVEMENT",
        SessionState::Parked => "PARKED",
        SessionState::Done => "DONE",
    }
}

/// Waits first, then the working sessions, then the rest.
fn rank(state: SessionState) -> u8 {
    match state {
        SessionState::NeedsYou | SessionState::Error => 0,
        SessionState::Working => 1,
        _ => 2,
    }
}

/// `6M12S` of a number of milliseconds.
fn age(ms: i64) -> String {
    let ms = ms.max(0);
    format!("{}M{:02}S", ms / 60_000, ms / 1000 % 60)
}

/// One row of the intercept log: a speaker label (only on the first line of a message), the
/// text, and how it is drawn.
struct Entry {
    verb: String,
    text: String,
    color: u32,
    bold: bool,
}

/// The rows of the transcript tail. The agent markdown is parsed by the console display, so no
/// raw marker shows; one label per message.
fn entries(pane: &Pane) -> Vec<Entry> {
    use crate::console::display::{DKind, last};
    last(&pane.tail, 16)
        .into_iter()
        .filter(|d| d.kind != DKind::Meta)
        .map(|d| {
            let verb = match (d.first, d.kind) {
                (false, _) => String::new(),
                (true, DKind::You) => "ORD".to_string(),
                (true, DKind::Tool) => dots(
                    &d.label
                        .clone()
                        .unwrap_or_else(|| "TOOL".into())
                        .to_uppercase(),
                    6,
                ),
                (true, DKind::Error) => "ERR".to_string(),
                (true, DKind::Output | DKind::Meta) => String::new(),
                (true, _) => "SAY".to_string(),
            };
            let (text, color, bold) = match d.kind {
                DKind::You => (d.text, tac::INK, false),
                DKind::Tool => (short_tool("", &d.text), tac::TEXT, false),
                DKind::Output => (
                    format!("└ {}", dots(d.text.lines().next().unwrap_or(""), 110)),
                    tac::LABEL,
                    false,
                ),
                DKind::Error => (d.text, tac::WHITE, false),
                DKind::Heading => (d.text, tac::INK, true),
                DKind::Code | DKind::Table => (d.text, tac::MID, false),
                _ => (d.text, tac::TEXT, false),
            };
            Entry {
                verb,
                text,
                color,
                bold,
            }
        })
        .collect()
}

/// A fact of the dossier: a label and a value.
fn fact(name: &str, value: String) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(label(name.to_string()))
        .child(
            div()
                .truncate()
                .font_family(tac::SANS)
                .text_size(ui::ts(tac::S_ENTRY + 1.))
                .text_color(c(tac::INK, 1.))
                .child(value),
        )
}

impl WarRoomScreen {
    /// The dossier of a session.
    pub(super) fn tactical_dossier(
        &mut self,
        war: &WarRoom,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pane) = war.session(id).cloned() else {
            return div().into_any_element();
        };
        let (mut list, _) = contacts(&self.tactical, war);
        // The contact title is cut at 30 characters with an ellipsis; use the whole title.
        for ct in &mut list {
            if let Some(s) = war.session(&ct.id)
                && !s.title.is_empty()
            {
                ct.title = s.title.clone();
            }
        }
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
        let waits = pane.ask.is_some();
        let accent = if waits { tac::WHITE } else { tac::INK };
        let call = format!("{}/{}", pane.codebase, pane.title).to_uppercase();

        // ---- left: the contacts ------------------------------------------------------------
        let mut ordered: Vec<&crate::warroom::skins::hud::Contact> = list.iter().collect();
        ordered.sort_by(|a, b| (rank(a.state), &a.id).cmp(&(rank(b.state), &b.id)));
        let mut rows = div()
            .id("tac-contacts")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        let mut group = u8::MAX;
        for s in ordered {
            let g = rank(s.state);
            if g != group {
                group = g;
                rows = rows.child(
                    div()
                        .flex_none()
                        .px(px(10.))
                        .pt(px(if g == 0 { 4. } else { 20. }))
                        .pb(px(6.))
                        .font_family(tac::MONO)
                        .text_size(ui::ts(tac::S_LABEL))
                        .text_color(c(if g == 0 { tac::WHITE } else { tac::LABEL }, 1.))
                        .child(["AWAITING ORDERS", "ACTIVE", "NO MOVEMENT"][g as usize]),
                );
            }
            let on = s.id == id;
            let (sid, color) = (s.id.clone(), if s.waits() { tac::WHITE } else { tac::INK });
            let mark = if s.waits() {
                div()
                    .flex_none()
                    .size(px(9.))
                    .border_1()
                    .border_color(c(if on { tac::DARK } else { color }, 1.))
            } else {
                div().flex_none().size(px(8.)).rounded(px(4.)).bg(c(
                    if on {
                        tac::DARK
                    } else if g == 1 {
                        tac::INK
                    } else {
                        tac::DIM
                    },
                    1.,
                ))
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("tac-c-{}", s.id)))
                    .flex_none()
                    .min_h(px(36.))
                    .py(px(5.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .font_family(tac::MONO)
                    .text_size(ui::ts(tac::S_LABEL))
                    .bg(c(tac::INK, if on { 1. } else { 0. }))
                    .text_color(c(if on { tac::DARK } else { color }, 1.))
                    .opacity(if on || s.waits() { 1. } else { 0.7 })
                    .hover(|h| h.opacity(1.).bg(c(tac::INK, 0.12)))
                    .child(mark)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(format!("{}/{}", s.codebase, s.title).to_uppercase()),
                    )
                    .child(div().flex_none().child(age((s.minutes * 60_000.) as i64)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tactical.open = Some(sid.clone());
                        cx.notify();
                    })),
            );
        }
        let left = tbox("CONTACTS")
            .flex_none()
            .w(px(340.))
            .p(px(16.))
            .pt(px(24.))
            .gap(px(12.))
            .child(rows)
            .child(
                div()
                    .flex_none()
                    .text_size(ui::ts(tac::S_LABEL))
                    .line_height(ui::ts(tac::S_LABEL) * 1.6)
                    .text_color(c(tac::DIM, 1.))
                    .child("RANGE = TIME SINCE LAST MOVE. SAME CONTACTS AS THE RADAR."),
            );

        // ---- middle: the intercept log -----------------------------------------------------
        let meta = format!(
            "{} · {} · LINES {} · {}",
            pane.provider.to_uppercase(),
            dots(&pane.model, 16).to_uppercase(),
            pane.lines_total,
            part_of(&pane).to_uppercase()
        );
        let head = div()
            .flex_none()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_family(tac::SANS)
                            .font_weight(FontWeight::BOLD)
                            .text_size(ui::ts(tac::S_SECTION + 4.))
                            .text_color(c(tac::INK, 1.))
                            .child(call.clone()),
                    )
                    .child(label(meta)),
            )
            .child({
                let (focus_id, park_id) = (pane.id.clone(), pane.id.clone());
                div()
                    .flex_none()
                    .flex()
                    .gap(px(6.))
                    .child(
                        tac::navb("tac-back", "< RADAR", false)
                            .border_1()
                            .border_color(c(tac::INK, 1.))
                            .text_color(c(tac::INK, 1.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tactical.open = None;
                                cx.notify();
                            })),
                    )
                    .child(tac::navb("tac-focus", "FOCUS", false).on_click(
                        cx.listener(move |this, _, _, cx| this.skin_focus(&focus_id, cx)),
                    ))
                    .child(tac::navb("tac-park", "PARK", false).on_click(cx.listener(
                        move |this, _, _, cx| this.act(Action::Park(park_id.clone()), cx),
                    )))
            });
        let objective = tac::entry(format!(
            "OBJECTIVE: {}",
            dots(pane.prompt.lines().next().unwrap_or("").trim(), 120)
        ));

        let mut log = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .justify_end()
            .gap(px(6.))
            .overflow_hidden();
        for e in entries(&pane) {
            log = log.child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(12.))
                    .text_size(ui::ts(tac::S_LABEL))
                    .line_height(ui::ts(tac::S_LABEL) * 1.5)
                    .child(
                        div()
                            .flex_none()
                            .w(px(52.))
                            .text_color(c(tac::WHITE, 0.8))
                            .child(e.verb),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .when(e.bold, |d| d.font_weight(FontWeight::BOLD))
                            .text_color(c(e.color, 1.))
                            .child(e.text),
                    ),
            );
        }

        let bottom: AnyElement = if let Some(ask) = pane.ask.as_ref() {
            let mut orders = div().flex().flex_wrap().gap(px(8.));
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
                        format!("tac-s-ans-{i}"),
                        &format!("[{}] {}", i + 1, key_label(option)),
                        kind,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.skin_answer(&sid, &aid, &opt, cx)),
                    ),
                );
            }
            let what = match ask.kind {
                aivory_protocol::AskKind::Permission => format!("$ {}", dots(&ask.text, 70)),
                aivory_protocol::AskKind::Plan => format!("PLAN · {} STEPS", ask.steps.len()),
                _ => String::new(),
            };
            let question = match ask.kind {
                aivory_protocol::AskKind::Permission => {
                    format!("Allow {} to run this command?", pane.provider)
                }
                aivory_protocol::AskKind::Plan => {
                    format!("Approve the plan: {}", dots(&ask.text, 90))
                }
                _ => dots(&ask.text, 160),
            };
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(20.))
                .border_1()
                .border_color(c(tac::WHITE, 1.))
                .bg(c(tac::WHITE, 0.04))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_none()
                                .font_family(tac::MONO)
                                .text_size(ui::ts(tac::S_LABEL))
                                .text_color(c(tac::WHITE, 1.))
                                .child(format!(
                                    "ORDER REQUESTED · {}",
                                    tac::clock_ms(war.now - pane.since)
                                )),
                        )
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .font_family(tac::MONO)
                                .text_size(ui::ts(tac::S_LABEL))
                                .text_color(c(tac::TEXT, 1.))
                                .child(what),
                        ),
                )
                .child(tac::entry(question).text_color(c(tac::WHITE, 1.)))
                .child(orders)
                .into_any_element()
        } else {
            let input = self.composer_input(&pane, window, cx);
            let (send_id, hold_id) = (pane.id.clone(), pane.id.clone());
            div()
                .flex_none()
                .min_h(px(56.))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(14.))
                .py(px(6.))
                .border_1()
                .border_color(c(tac::DIM, 1.))
                .child(label("ORDER:"))
                .child(div().flex_1().min_w(px(0.)).child(input))
                .child(tac::order("tac-send", "SEND", Btn::Plain).on_click(
                    cx.listener(move |this, _, window, cx| this.submit(&send_id, window, cx)),
                ))
                .child(tac::order("tac-hold", "HOLD", Btn::Dim).on_click(
                    cx.listener(move |this, _, _, cx| this.act(Action::Pause(hold_id.clone()), cx)),
                ))
                .into_any_element()
        };

        let say = crate::console::display::last(&pane.tail, 40)
            .into_iter()
            .rev()
            .find(|d| d.kind == crate::console::display::DKind::Agent)
            .map(|d| crate::console::display::plain_line(&d.text));
        let mut middle = tbox("INTERCEPT LOG")
            .flex_1()
            .min_w(px(0.))
            .p(px(24.))
            .pt(px(28.))
            .gap(px(16.))
            .child(head)
            .child(objective)
            .child(div().flex_none().h(px(1.)).bg(c(tac::DEEP, 1.)))
            .child(log);
        if let Some(s) = say {
            middle = middle.child(
                div()
                    .flex_none()
                    .flex()
                    .items_end()
                    .gap(px(6.))
                    .child(tac::entry(s).text_color(c(tac::INK, 1.)))
                    .child(div().flex_none().w(px(9.)).h(px(14.)).bg(c(tac::INK, 1.))),
            );
        }
        let middle = middle.child(bottom);

        // ---- right: the facts, the route and the files -------------------------------------
        let mut segs = div().flex().gap(px(3.));
        for k in 0..10 {
            let on = (k + 1) * 10 <= pane.context_pct as usize + 5;
            segs = segs.child(
                div()
                    .flex_1()
                    .h(px(10.))
                    .border_1()
                    .border_color(c(if on { tac::INK } else { tac::DEEP }, 1.))
                    .when(on, |d| d.bg(c(tac::INK, 1.))),
            );
        }
        let facts = tbox("DOSSIER")
            .flex_none()
            .h(px(236.))
            .p(px(24.))
            .pt(px(28.))
            .gap(px(14.))
            .child(
                div()
                    .self_start()
                    .px(px(10.))
                    .py(px(5.))
                    .bg(c(accent, 1.))
                    .font_family(tac::SANS)
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(tac::S_ENTRY))
                    .text_color(c(tac::DARK, 1.))
                    .child(state_word(&pane)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .child(fact("AGENT", pane.provider.to_uppercase()))
                    .child(fact("TOKENS", thousands(pane.tokens)))
                    .child(fact("COST", format!("${:.2}", pane.cost))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .child(fact("LINES", pane.lines_total.to_string()))
                    .child(fact("SUBAGENTS", pane.subagents.len().to_string()))
                    .child(fact("MODEL", dots(&pane.model, 10).to_uppercase())),
            )
            .child(label(format!("CONTEXT {}%", pane.context_pct)))
            .child(segs);

        let route = tbox("MISSION ROUTE").flex_none().h(px(280.)).child(
            Self::tactical_picture(route_paint(route_steps(&pane)))
                .absolute()
                .inset_0(),
        );

        let mut file_rows = div()
            .id("tac-files")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(14.));
        if files.is_empty() {
            file_rows = file_rows.child(label("NO FILES HIT YET"));
        }
        for (path, added, removed) in files.iter().take(20) {
            let (a, d) = (
                (*added as f32 * 2.4).clamp(2., 200.),
                (*removed as f32 * 2.4).min(120.),
            );
            file_rows = file_rows.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .truncate()
                            .font_family(tac::MONO)
                            .text_size(ui::ts(tac::S_LABEL))
                            .text_color(c(tac::TEXT, 1.))
                            .child(path.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(div().h(px(8.)).w(px(a)).bg(c(tac::INK, 1.)))
                            .when(*removed > 0, |r| {
                                r.child(
                                    div()
                                        .h(px(8.))
                                        .w(px(d.max(4.)))
                                        .border_1()
                                        .border_color(c(tac::INK, 1.)),
                                )
                            })
                            .child(
                                div()
                                    .ml(px(8.))
                                    .child(label(format!("+{added} -{removed}"))),
                            ),
                    ),
            );
        }
        let hit = tbox("TARGETS HIT · FILES")
            .flex_1()
            .min_h(px(0.))
            .p(px(24.))
            .pt(px(28.))
            .gap(px(12.))
            .child(file_rows)
            .child(
                div()
                    .flex_none()
                    .text_size(ui::ts(tac::S_LABEL))
                    .text_color(c(tac::DIM, 1.))
                    .child("SOLID = LINES ADDED. OPEN = LINES REMOVED."),
            );
        let right = div()
            .flex_none()
            .w(px(430.))
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(facts)
            .child(route)
            .child(hit);

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(c(tac::BG, 1.))
            .font_family(tac::MONO)
            .text_color(c(tac::INK, 1.))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .gap(px(32.))
                    .px(px(20.))
                    .pt(px(24.))
                    .pb(px(22.))
                    .child(left)
                    .child(middle)
                    .child(right),
            )
            .child(tac::scan())
            .into_any_element()
    }
}
