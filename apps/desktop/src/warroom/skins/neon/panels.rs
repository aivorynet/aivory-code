//! The HUD panels of the Neon (`NeonGrid.dc.html`). They float over the city and
//! show only what matters: Waits for you, the decision with its answers, Its last steps, and
//! the district board with the budget at the bottom. The decision is the one bright panel;
//! the others are dim and light up while the mouse is over them. Glow sits on frames and
//! titles, never on text. All values come from the WarRoom read model. Every answer calls
//! the daemon through `act`.

use aivory_protocol::{AskKind, Pane, SessionState, UsageWindow, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, relative, rgb, rgba};

use super::city::{
    BOARD_GAP, BOARD_H, CHAKRA, CYAN, DIM, MAGENTA, MIST, ORBITRON, PALE, PANEL_W, RED, T_ENTRY,
    T_LABEL, T_SECTION, YELLOW,
};
use super::hud::{button, frame, hex, tag, wait_code};
use super::{Dock, NeonState};
use crate::console::display::{DKind, steps};
use crate::theme::tk;
use crate::ui;
use crate::ui::sw as s;
use crate::warroom::WarRoomScreen;
use crate::warroom::decisions::{question, terminal_note};
use crate::warroom::skins::common::{kind_word, part_of, thousands};

/// The width of the panel column on the right.
const COLUMN_W: f32 = 400.;
/// The districts on the board.
const DISTRICTS: usize = 3;
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
        .px(s(PAD))
        .pt(s(20.))
        .pb(s(12.))
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
    div().w_full().h(s(h)).bg(rgba(0x1A2142CC)).child(
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

impl WarRoomScreen {
    /// All panels of the Neon: the right column and the district board.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn neon_panels(
        &mut self,
        war: &WarRoom,
        waits: &[&Pane],
        target: Option<&Pane>,
        docks: &[Dock],
        dt: f32,
        (width, height): (f32, f32),
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

        // The rows that fit: the height in design pixels, minus the selected panel (an
        // estimate), the gap, the head and the padding of the sessions panel.
        let design = height / ui::factor() - 24. - BOARD_GAP;
        // A short window: the selected panel drops its chat.
        let compact = design < 760.;
        let base = if compact { 270. } else { 380. };
        let selected = target.map_or(0., |p| base + if p.ask.is_some() { 150. } else { 0. });
        let rows = ((design - selected - GAP - 70.) / ROW_H)
            .floor()
            .clamp(2., 9.) as usize;

        // The selected panel is the bright one. With no session on screen the list is.
        let mut column = div()
            .absolute()
            .right(s(24.))
            .top(s(24.))
            .bottom(s(BOARD_GAP))
            .w(s(COLUMN_W))
            .flex()
            .flex_col()
            .gap(s(GAP))
            .child(sessions_panel(
                war,
                waits,
                target,
                rows,
                target.is_none(),
                cx,
            ));
        if let Some(p) = target {
            column = column.child(decision_panel(war, p, ctx, compact, cx));
        }

        // The districts on the board: as many as the width allows next to the budget.
        let room = width / ui::factor() - 448. - 48. - 280.;
        let shown = ((room / 190.).floor().max(0.) as usize).min(DISTRICTS);
        vec![
            column.into_any_element(),
            board_panel(war, docks, shown, cx).into_any_element(),
        ]
    }
}

/// The height of one session row, in design pixels.
const ROW_H: f32 = 54.;

/// What a session does now, in one line: the open question, the last tool call, the last
/// words of the agent. Never the raw output.
fn doing(p: &Pane) -> String {
    if let Some(a) = &p.ask {
        let first = a.text.lines().next().unwrap_or("").trim();
        return format!("{}: {first}", kind_word(a.kind));
    }
    if p.state == SessionState::Idle {
        return "Idle".to_string();
    }
    let lines = steps(&p.tail, 8);
    let want = if p.state == SessionState::Error {
        DKind::Error
    } else {
        DKind::Tool
    };
    let pick = lines
        .iter()
        .rev()
        .find(|d| d.kind == want)
        .or_else(|| lines.iter().rev().find(|d| d.kind == DKind::Agent))
        .or(lines.last());
    match pick {
        Some(d) if d.kind == DKind::Tool => {
            format!("{} {}", d.label.as_deref().unwrap_or("Tool"), d.text)
        }
        Some(d) => d.text.clone(),
        None if p.state == SessionState::Error => "Error".to_string(),
        None => "Working".to_string(),
    }
}

/// The newest chat lines of a session: what you said and what the agent said. `true` marks
/// a line of yours. Tool calls and output stay out.
fn chat(p: &Pane, n: usize) -> Vec<(bool, String)> {
    let mut v: Vec<(bool, String)> = steps(&p.tail, 16)
        .into_iter()
        .filter(|d| !matches!(d.kind, DKind::Tool | DKind::Error))
        .map(|d| (d.kind == DKind::You, d.text))
        .collect();
    let skip = v.len().saturating_sub(n);
    v.drain(..skip);
    v
}

/// The state of a session as the code of its row: the wait in minutes, or a word.
fn row_code(war: &WarRoom, p: &Pane) -> (String, u32) {
    match (&p.ask, p.state) {
        (_, SessionState::Error) => ("ERR".into(), RED),
        (Some(_), _) => (wait_code(p.minutes(war.now)), MAGENTA),
        (None, SessionState::Idle) => ("IDLE".into(), DIM),
        (None, _) => ("RUN".into(), CYAN),
    }
}

/// All live sessions in one list: what needs you first (the longest wait first), then what
/// works, then what idles. Each row says what the session does now. One click selects it;
/// the selected panel then shows its chat and the keys. This is the overview of the city.
fn sessions_panel(
    war: &WarRoom,
    waits: &[&Pane],
    target: Option<&Pane>,
    rows: usize,
    lit: bool,
    cx: &mut Context<WarRoomScreen>,
) -> Div {
    let mut rest: Vec<&Pane> = war
        .sessions
        .iter()
        .filter(|s| s.state.is_live() && !waits.iter().any(|w| w.id == s.id))
        .collect();
    rest.sort_by_key(|p| {
        (
            p.state == SessionState::Idle,
            p.codebase.clone(),
            p.id.clone(),
        )
    });
    let total = waits.len() + rest.len();
    let side = if waits.is_empty() {
        ("ALL CLEAR".to_string(), CYAN)
    } else {
        (format!("{} NEED YOU", waits.len()), MAGENTA)
    };
    let mut p = panel(MAGENTA, 0.82, lit, "cy-waits")
        .flex_1()
        .min_h(s(0.))
        .overflow_hidden()
        .pb(s(8.))
        .child(head("Sessions", MAGENTA, Some(side)));
    // A footer line takes the room of one row. The selected session always shows.
    let shown = if total > rows {
        rows.saturating_sub(1).max(1)
    } else {
        total
    };
    let mut list: Vec<&Pane> = waits.iter().chain(rest.iter()).copied().collect();
    if let Some(at) = target.and_then(|t| list.iter().position(|q| q.id == t.id))
        && at >= shown
    {
        let chosen = list.remove(at);
        list.insert(shown - 1, chosen);
    }
    for q in list.iter().take(shown) {
        let id = q.id.clone();
        let on = target.is_some_and(|t| t.id == q.id);
        let (code, color) = row_code(war, q);
        let asks = q.ask.is_some() || q.state == SessionState::Error;
        p = p.child(
            div()
                .id(SharedString::from(format!("cy-w-{}", q.id)))
                .h(s(ROW_H))
                .flex_none()
                .px(s(PAD))
                .flex()
                .items_center()
                .gap(s(14.))
                .cursor_pointer()
                .border_l_2()
                .border_color(if on { rgb(color) } else { rgba(0x00000000) })
                .when(on, |d| d.bg(rgba(0xFF2A6D26)))
                .when(!on && asks, |d| d.bg(rgba(0xFF2A6D0D)))
                .hover(|st| st.bg(rgba(0x00F0FF1A)))
                .child(
                    div()
                        .w(s(46.))
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(color))
                        .child(code),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(s(2.))
                        .child(
                            div()
                                .truncate()
                                .text_color(rgb(if on { 0xFFFFFF } else { PALE }))
                                .child(q.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(ui::ts(T_LABEL))
                                .text_color(rgb(MIST))
                                .child(format!("{} · {}", q.codebase, doing(q))),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.neon.sel = Some(id.clone());
                    cx.notify();
                })),
        );
    }
    if total > shown {
        p = p.child(
            div()
                .px(s(PAD))
                .pt(s(4.))
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(MIST))
                .child(format!("+{} more sessions", total - shown)),
        );
    }
    if total == 0 {
        p = p.child(
            div()
                .h(s(40.))
                .px(s(PAD))
                .flex()
                .items_center()
                .text_color(rgb(CYAN))
                .child("No live sessions."),
        );
    }
    p
}

/// The selected session: the kind, the title, what it does now, its chat, the command or
/// question, the answers, Focus and the context bar. The answers call the daemon through
/// `act`. This is the bright panel.
fn decision_panel(
    war: &WarRoom,
    p: &Pane,
    context: f32,
    compact: bool,
    cx: &mut Context<WarRoomScreen>,
) -> Div {
    let (word, color) = kind_tag(p);
    let edge = if p.ask.is_some() { YELLOW } else { CYAN };
    let mut card = panel(edge, 0.88, true, "cy-decision")
        .flex_none()
        .px(s(PAD))
        .py(s(PAD))
        .gap(s(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(s(12.))
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
                .truncate()
                .child(format!("{} · {} / {}", p.provider, p.codebase, part_of(p))),
        )
        .child(
            div()
                .flex()
                .gap(s(8.))
                .text_size(ui::ts(T_LABEL))
                .child(div().flex_none().text_color(rgb(CYAN)).child("NOW"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_clamp(2)
                        .text_color(rgb(PALE))
                        .child(doing(p)),
                ),
        );
    // The chat: the last words of you and of the agent.
    let lines = if compact { Vec::new() } else { chat(p, 3) };
    if !lines.is_empty() {
        let mut block = div()
            .flex()
            .flex_col()
            .gap(s(4.))
            .pl(s(12.))
            .border_l_2()
            .border_color(rgb(DIM));
        for (you, text) in lines {
            block = block.child(
                div()
                    .line_height(ui::ts(19.))
                    .line_clamp(2)
                    .text_color(rgb(if you { 0xFFFFFF } else { PALE }))
                    .child(if you { format!("› {text}") } else { text }),
            );
        }
        card = card.child(block);
    }
    let mut row = div().flex().flex_wrap().gap(s(12.)).pt(s(4.));
    if let Some(ask) = &p.ask {
        if ask.kind == AskKind::Permission {
            card = card.child(
                div()
                    .px(s(16.))
                    .py(s(10.))
                    .bg(rgba(0x0000008C))
                    .border_l_2()
                    .border_color(rgb(YELLOW))
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_LABEL))
                    .line_height(ui::ts(17.))
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
                    .line_clamp(3)
                    .child(question(p, ask)),
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
    let focus = p.id.clone();
    row = row.child(
        button(
            SharedString::from(format!("cy-f-{}", p.id)),
            "Focus",
            CYAN,
            true,
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

/// One cell of the budget: the spend as a readout and the usage windows as text.
fn budget_cell(war: &WarRoom) -> Div {
    let live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
    let tokens: u64 = live.iter().map(|s| s.tokens).sum();
    let cost: f64 = live.iter().map(|s| s.cost).sum();
    let mut cell = div()
        .w(s(280.))
        .flex_none()
        .flex()
        .flex_col()
        .gap(s(4.))
        .pl(s(16.))
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
fn board_panel(
    war: &WarRoom,
    docks: &[Dock],
    limit: usize,
    cx: &mut Context<WarRoomScreen>,
) -> Div {
    let mut row = div()
        .absolute()
        .left(s(PAD))
        .right(s(PAD))
        .top(s(PAD / 2.))
        .bottom(s(PAD / 2.))
        .flex()
        .items_center()
        .gap(s(24.));
    if docks.is_empty() || limit == 0 {
        row = row.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MIST))
                .child(if docks.is_empty() {
                    "No live districts"
                } else {
                    "Districts: see the plates"
                }),
        );
    }
    for d in docks.iter().take(limit) {
        let pick = d.pick.clone();
        let name_color = if d.asks > 0 { MAGENTA } else { d.color };
        row = row.child(
            div()
                .id(SharedString::from(format!("cy-d-{}", d.name)))
                .flex_1()
                .min_w(s(0.))
                .flex()
                .flex_col()
                .gap(s(6.))
                .pl(s(16.))
                .py(s(8.))
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
    if docks.len() > limit {
        row = row.child(
            div()
                .id("cy-more-repos")
                .flex_none()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(MIST))
                .tooltip(ui::tip(
                    "More repos have live sessions. Open Repos in the header to see all.",
                ))
                .child(format!("+{} more repos", docks.len() - limit)),
        );
    }
    row = row.child(budget_cell(war));
    div()
        .absolute()
        .left(s(24.))
        .right(s(PANEL_W))
        .bottom(s(BOARD_GAP))
        .h(s(BOARD_H))
        .occlude()
        .font_family(CHAKRA)
        .text_color(rgb(PALE))
        .child(frame(CYAN, 14., 0.82, false))
        .child(row)
}
