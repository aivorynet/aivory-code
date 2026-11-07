//! The HUD panels of the Neon (`NeonGrid.dc.html`). They float over the city and
//! show only what matters: Waits for you, the decision with its answers, Its last steps, and
//! the district board with the budget at the bottom. The decision is the one bright panel;
//! the others are dim and light up while the mouse is over them. Glow sits on frames and
//! titles, never on text. All values come from the WarRoom read model. Every answer calls
//! the daemon through `act`.

use aivory_protocol::{AskKind, Pane, SessionState, UsageWindow, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, px, relative, rgb, rgba};

use super::city::{
    BOARD_GAP, BOARD_H, CHAKRA, CYAN, DIM, MAGENTA, MIST, ORBITRON, PALE, PANEL_W, RED, T_ENTRY,
    T_LABEL, T_SECTION, YELLOW,
};
use super::hud::{button, frame, hex, tag, wait_code};
use super::{Dock, NeonState};
use crate::console::display::{DKind, steps};
use crate::theme::tk;
use crate::ui;
use crate::warroom::decisions::{question, terminal_note};
use crate::warroom::skins::common::{kind_word, part_of, thousands};
use crate::warroom::{WarRoomEvent, WarRoomScreen};

/// The width of the panel column on the right.
const COLUMN_W: f32 = 400.;
/// The rows of the Waits panel.
const ROWS: usize = 3;
/// The districts on the board.
const DISTRICTS: usize = 3;
/// The lines of the steps panel.
const STEPS: usize = 3;
/// The gap between two panels.
const GAP: f32 = 32.;
/// The inner padding of a panel.
const PAD: f32 = 24.;

/// A floating panel: the chamfered frame behind a column of content. A panel that is not
/// `lit` dims and lights up while the mouse is over it; `name` names its hover group.
fn panel(color: u32, fill: f32, lit: bool, name: &'static str) -> Div {
    let d = div()
        .relative()
        .flex()
        .flex_col()
        .occlude()
        .font_family(CHAKRA)
        .text_size(ui::ts(T_ENTRY))
        .text_color(rgb(PALE))
        .child(frame(color, 14., fill, lit));
    if lit {
        d
    } else {
        d.group(name)
            .opacity(0.72)
            .group_hover(name, |s| s.opacity(1.))
    }
}

/// The head of a panel: the title in neon and a note on the right.
fn head(title: &str, color: u32, side: Option<(String, u32)>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .px(px(PAD))
        .pt(px(20.))
        .pb(px(12.))
        .child(
            div()
                .font_family(ORBITRON)
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(color))
                .child(title.to_uppercase()),
        )
        .children(side.map(|(text, c)| {
            div()
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(c))
                .child(text)
        }))
}

/// A bar on a dark track; `frac` is 0 to 1.
fn bar(frac: f32, color: u32, h: f32) -> Div {
    div().w_full().h(px(h)).bg(rgba(0x1A2142CC)).child(
        div()
            .h_full()
            .w(relative(frac.clamp(0., 1.)))
            .bg(rgb(color)),
    )
}

/// The kind tag of a session: what it waits for, or what it does.
fn kind_tag(p: &Pane) -> (String, u32) {
    match (&p.ask, p.state) {
        (_, SessionState::Error) => ("Error".into(), RED),
        (Some(a), _) => (kind_word(a.kind).to_string(), MAGENTA),
        (None, SessionState::Idle) => ("Idle".into(), MIST),
        (None, _) => ("Working".into(), CYAN),
    }
}

/// The words of an answer button: the three permission answers carry their key.
fn answer_label(kind: AskKind, option: &str, at: usize) -> String {
    match (kind, option) {
        (AskKind::Permission, "Allow") => "1 Allow".to_string(),
        (AskKind::Permission, "Always allow") => "2 Always".to_string(),
        (AskKind::Permission, "Deny") => "3 Deny".to_string(),
        _ => format!("{} {option}", at + 1),
    }
}

/// `claude` -> `Claude`.
fn capital(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// ` · resets 2 h 14 m` for a window that has a reset time.
fn reset_text(w: &UsageWindow, now: i64) -> String {
    match w.resets_at {
        Some(at) => {
            let m = (at - now).max(0) / 60_000;
            if m >= 60 {
                format!(" · resets {} h {:02} m", m / 60, m % 60)
            } else {
                format!(" · resets {m} m")
            }
        }
        None if w.estimate => " · estimate".to_string(),
        None => String::new(),
    }
}

/// One display line as steps text: the text and its color. The lines come from
/// `console::display`, so no raw markdown marker shows.
fn step_line(kind: DKind, label: Option<&str>, text: &str) -> (String, u32) {
    match kind {
        DKind::Tool => (format!("● {} {text}", label.unwrap_or("")), CYAN),
        DKind::Output => (format!("  └ {text}"), MIST),
        DKind::Error => (format!("! {text}"), RED),
        DKind::You => (format!("› {text}"), 0xFFFFFF),
        DKind::Heading => (text.to_string(), YELLOW),
        DKind::Bullet | DKind::Agent => (text.to_string(), PALE),
        DKind::Code | DKind::Table => (text.to_string(), MIST),
        DKind::Meta => (text.to_string(), DIM),
    }
}

impl WarRoomScreen {
    /// All panels of the Neon: the right column and the district board.
    pub(super) fn neon_panels(
        &mut self,
        war: &WarRoom,
        waits: &[&Pane],
        target: Option<&Pane>,
        docks: &[Dock],
        dt: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // The context bar eases to its value.
        let st: &mut NeonState = &mut self.neon;
        let ctx = target.map_or(0., |p| {
            st.eased(
                &format!("ctx-{}", p.id),
                f32::from(p.context_pct) / 100.,
                dt,
            )
        });
        st.ease.retain(|_, v| v.is_finite());

        // The decision is the one bright panel. With no session on screen the waits panel is.
        let mut column = div()
            .absolute()
            .right(px(24.))
            .top(px(24.))
            .bottom(px(BOARD_GAP))
            .w(px(COLUMN_W))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(waits_panel(war, waits, target, target.is_none(), cx));
        if let Some(p) = target {
            column = column.child(decision_panel(war, p, ctx, cx));
        }
        column = column.child(steps_panel(target));

        vec![
            column.into_any_element(),
            board_panel(war, docks, cx).into_any_element(),
        ]
    }
}

/// What waits for you, the longest wait first. A click selects the session.
fn waits_panel(
    war: &WarRoom,
    waits: &[&Pane],
    target: Option<&Pane>,
    lit: bool,
    cx: &mut Context<WarRoomScreen>,
) -> Div {
    let mut p = panel(MAGENTA, 0.82, lit, "cy-waits")
        .flex_none()
        .pb(px(12.))
        .child(head(
            "Waits for you",
            MAGENTA,
            Some((
                if waits.len() > ROWS {
                    format!("+{} MORE", waits.len() - ROWS)
                } else {
                    format!("{}", waits.len())
                },
                MIST,
            )),
        ));
    if waits.is_empty() {
        p = p.child(
            div()
                .h(px(40.))
                .px(px(PAD))
                .flex()
                .items_center()
                .text_color(rgb(CYAN))
                .child("All clear. Nothing waits for you."),
        );
    }
    for q in waits.iter().take(ROWS) {
        let id = q.id.clone();
        let on = target.is_some_and(|t| t.id == q.id);
        let color = if q.state == SessionState::Error {
            YELLOW
        } else {
            MAGENTA
        };
        p = p.child(
            div()
                .id(SharedString::from(format!("cy-w-{}", q.id)))
                .h(px(40.))
                .px(px(PAD))
                .flex()
                .items_center()
                .gap(px(16.))
                .cursor_pointer()
                .when(on, |d| d.bg(rgba(0xFF2A6D26)))
                .hover(|s| s.bg(rgba(0xFF2A6D1A)))
                .child(
                    div()
                        .w(px(44.))
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(color))
                        .child(wait_code(q.minutes(war.now))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(if on { 0xFFFFFF } else { PALE }))
                        .child(q.title.clone()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.neon.sel = Some(id.clone());
                    cx.notify();
                })),
        );
    }
    p
}

/// The selected session: the kind, the title, the command or question, the answers, the
/// context bar. The answers call the daemon through `act`. This is the bright panel.
fn decision_panel(war: &WarRoom, p: &Pane, context: f32, cx: &mut Context<WarRoomScreen>) -> Div {
    let (word, color) = kind_tag(p);
    let edge = if p.ask.is_some() { YELLOW } else { CYAN };
    let mut card = panel(edge, 0.88, true, "cy-decision")
        .flex_none()
        .px(px(PAD))
        .py(px(PAD))
        .gap(px(14.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_LABEL))
                .child(tag(&word, color))
                .when(p.ask.is_some(), |d| {
                    d.child(
                        div()
                            .text_color(rgb(color))
                            .child(format!("WAITS {}", wait_code(p.minutes(war.now)))),
                    )
                })
                .child(div().flex_1())
                .child(div().text_color(rgb(DIM)).child(hex(&p.id))),
        )
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_SECTION))
                .line_height(ui::ts(26.))
                .text_color(rgb(YELLOW))
                .line_clamp(2)
                .child(p.title.clone()),
        )
        .child(
            div()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(MIST))
                .child(format!("{} · {} / {}", p.provider, p.codebase, part_of(p))),
        );
    let mut row = div().flex().flex_wrap().gap(px(12.)).pt(px(4.));
    if let Some(ask) = &p.ask {
        if ask.kind == AskKind::Permission {
            card = card.child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .bg(rgba(0x0000008C))
                    .border_l_2()
                    .border_color(rgb(YELLOW))
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_ENTRY - 1.))
                    .line_height(ui::ts(20.))
                    .text_color(rgb(PALE))
                    .line_clamp(3)
                    .child(ask.text.clone()),
            );
        } else {
            card = card.child(
                div()
                    .text_size(ui::ts(T_ENTRY))
                    .line_height(ui::ts(21.))
                    .text_color(rgb(PALE))
                    .line_clamp(4)
                    .child(question(p, ask)),
            );
        }
        for (i, step) in ask.steps.iter().take(3).enumerate() {
            card = card.child(
                div()
                    .text_size(ui::ts(T_LABEL))
                    .line_height(ui::ts(18.))
                    .text_color(rgb(MIST))
                    .truncate()
                    .child(format!("{}  {step}", i + 1)),
            );
        }
        if p.controlled {
            for (i, option) in ask.options.iter().enumerate() {
                let (session, ask_id, pick) = (p.id.clone(), ask.id.clone(), option.clone());
                let label = answer_label(ask.kind, option, i);
                let deny = label.to_lowercase().contains("deny");
                let color = if i == 0 {
                    YELLOW
                } else if deny {
                    MAGENTA
                } else {
                    MIST
                };
                row = row.child(
                    button(
                        SharedString::from(format!("cy-a-{}-{i}", p.id)),
                        &label,
                        color,
                        i == 0,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.skin_answer(&session, &ask_id, &pick, cx);
                        this.neon.sel = None;
                    })),
                );
            }
        } else {
            card = card.child(terminal_note().text_color(rgb(MIST)));
        }
    }
    let (open, focus) = (p.id.clone(), p.id.clone());
    row = row
        .child(
            button(
                SharedString::from(format!("cy-o-{}", p.id)),
                "Open",
                CYAN,
                false,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(WarRoomEvent::OpenSession(open.clone()));
            })),
        )
        .child(
            button(
                SharedString::from(format!("cy-f-{}", p.id)),
                "Focus",
                CYAN,
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.skin_focus(&focus, cx))),
        );
    card.child(row)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(MIST))
                .child(format!(
                    "Context {}%  ·  {} tok  ·  ${:.2}",
                    p.context_pct,
                    thousands(p.tokens),
                    p.cost
                )),
        )
        .child(bar(
            context,
            if p.context_pct >= 85 { MAGENTA } else { CYAN },
            4.,
        ))
}

/// The last lines of the selected session, newest at the bottom. Dim until the mouse is over.
fn steps_panel(target: Option<&Pane>) -> Div {
    let live = target.is_some_and(|p| p.state != SessionState::Idle);
    let note = match target {
        Some(_) if live => ("LIVE".to_string(), RED),
        Some(_) => ("IDLE".to_string(), MIST),
        None => ("NO SESSION".to_string(), MIST),
    };
    let mut body = div()
        .flex_1()
        .min_h(px(0.))
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(px(PAD))
        .pb(px(PAD))
        .font_family(CHAKRA)
        .text_size(ui::ts(T_LABEL + 1.));
    if let Some(p) = target {
        for d in steps(&p.tail, STEPS) {
            let (text, color) = step_line(d.kind, d.label.as_deref(), &d.text);
            let mono = matches!(d.kind, DKind::Code | DKind::Table);
            body = body.child(
                div()
                    .flex_none()
                    .line_height(ui::ts(20.))
                    .line_clamp(2)
                    .when(mono, |x| x.font_family(tk().mono))
                    .text_color(rgb(color))
                    .child(text),
            );
        }
    }
    panel(CYAN, 0.8, false, "cy-steps")
        .flex_1()
        .min_h(px(80.))
        .overflow_hidden()
        .child(head("Its last steps", CYAN, Some(note)))
        .child(body)
}

/// One cell of the budget: the spend as a readout and the usage windows as text.
fn budget_cell(war: &WarRoom) -> Div {
    let live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
    let tokens: u64 = live.iter().map(|s| s.tokens).sum();
    let cost: f64 = live.iter().map(|s| s.cost).sum();
    let mut cell = div()
        .w(px(280.))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pl(px(16.))
        .border_l_2()
        .border_color(rgb(YELLOW))
        .child(
            div()
                .font_family(ORBITRON)
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(YELLOW))
                .child("BUDGET"),
        )
        .child(
            div()
                .text_size(ui::ts(T_ENTRY))
                .text_color(rgb(PALE))
                .child(format!("${cost:.2} · {} tok", thousands(tokens))),
        );
    for w in war.usage.iter().take(2) {
        let color = match w.pct {
            0..=69 => MIST,
            70..=89 => YELLOW,
            _ => MAGENTA,
        };
        cell = cell.child(
            div()
                .text_size(ui::ts(T_LABEL))
                .line_height(ui::ts(16.))
                .text_color(rgb(color))
                .child(format!(
                    "{} {} {}%{}",
                    capital(&w.provider),
                    w.label,
                    w.pct,
                    reset_text(w, war.now)
                )),
        );
    }
    cell
}

/// The district board: per district who works and the spend, then the budget. The districts
/// stay in the order of the city (by codebase id), so a cell never moves. A click selects the
/// district's next ask, else a busy session.
fn board_panel(war: &WarRoom, docks: &[Dock], cx: &mut Context<WarRoomScreen>) -> Div {
    let mut row = div()
        .absolute()
        .left(px(PAD))
        .right(px(PAD))
        .top(px(PAD / 2.))
        .bottom(px(PAD / 2.))
        .flex()
        .items_center()
        .gap(px(24.));
    if docks.is_empty() {
        row = row.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MIST))
                .child("No live districts"),
        );
    }
    for d in docks.iter().take(DISTRICTS) {
        let pick = d.pick.clone();
        let name_color = if d.asks > 0 { MAGENTA } else { d.color };
        row = row.child(
            div()
                .id(SharedString::from(format!("cy-d-{}", d.name)))
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .pl(px(16.))
                .py(px(8.))
                .border_l_2()
                .border_color(rgb(name_color))
                .cursor_pointer()
                .opacity(0.7)
                .hover(|s| s.bg(rgba(0x00F0FF14)).opacity(1.))
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .text_size(ui::ts(T_ENTRY))
                        .text_color(rgb(name_color))
                        .child(d.name.clone()),
                )
                .child(
                    div()
                        .text_size(ui::ts(T_LABEL))
                        .line_height(ui::ts(16.))
                        .text_color(rgb(PALE))
                        .child(d.state.clone()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = &pick {
                        this.neon.sel = Some(id.clone());
                        cx.notify();
                    }
                })),
        );
    }
    if docks.len() > DISTRICTS {
        row = row.child(
            div()
                .flex_none()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(MIST))
                .child(format!("+{}", docks.len() - DISTRICTS)),
        );
    }
    row = row.child(budget_cell(war));
    div()
        .absolute()
        .left(px(24.))
        .right(px(PANEL_W))
        .bottom(px(BOARD_GAP))
        .h(px(BOARD_H))
        .occlude()
        .font_family(CHAKRA)
        .text_color(rgb(PALE))
        .child(frame(CYAN, 14., 0.82, false))
        .child(row)
}
