//! The parts of the board: the legend, the module cards, and the four gates. Needs you shows
//! one ask at a time in a slider with its full context; the other gates show one-line rows,
//! grouped by codebase from 13 sessions (ADR 0022, section 6).

use aivory_protocol::{LineKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Stateful, div, px, relative, rgb};

use super::{
    BLOCKED, GAP, Gates, IDLE, NEEDS, StateLine, Tier, WORKING, WidthClass, ab, alert, gate_empty,
    gate_spans, group_open, groups, kind_label, last_lines, shows_card, side_width, state_line,
    state_tone, tier, wait_text, width_class, wrap_index,
};
use crate::actions::Action;
use crate::theme::*;
use crate::ui;
use crate::warroom::{WarRoomEvent, WarRoomScreen};

/// The height of a row of a gate, with its gap, for the count of whole rows.
const ROW_H: f32 = 40.;

/// The text of "Let it fix".
const FIX_TEXT: &str = "Fix the error yourself and go on.";

/// The name, the meaning, and the colors (text, tint) of a gate.
fn gate_info(which: usize) -> (&'static str, &'static str, u32, u32) {
    match which {
        NEEDS => ("Needs you", "Answer these first", tk().q_text, tk().q_bg),
        WORKING => ("Working", "Running now", tk().work_text, tk().work_bg),
        BLOCKED => ("Blocked", "Stopped on an error", tk().e_text, tk().e_bg),
        _ => ("Idle", "No turn running", tk().ink_2, tk().track),
    }
}

/// A card with the one soft shadow.
fn card() -> Div {
    div()
        .bg(rgb(tk().card))
        .rounded(tk().r_card)
        .shadow(ui::card_shadow())
}

/// The fade and the hint at the foot of a scroll area that has more below (the values come
/// from the last frame). `None` when the area shows its end, and in an area too low for the
/// hint: there the fade would cover all of the area.
pub(super) fn more_below(scroll: &gpui::ScrollHandle) -> Option<Div> {
    let more = f32::from(scroll.max_offset().y + scroll.offset().y) > 4.
        && f32::from(scroll.bounds().size.height) >= 96.;
    let page = tk().card;
    more.then(|| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(ui::sw(44.))
            .flex()
            .items_end()
            .justify_center()
            .pb(px(6.))
            .bg(gpui::linear_gradient(
                180.,
                gpui::linear_color_stop(gpui::rgba(page << 8), 0.),
                gpui::linear_color_stop(gpui::rgba(page << 8 | 0xFF), 0.55),
            ))
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_3))
            .child("More below · scroll")
    })
}

/// A text line that ends with an ellipsis; the full text shows on hover.
fn tip_line(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    let text: SharedString = text.into();
    div()
        .id(id)
        .min_w(px(0.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .tooltip(ui::tip(text.clone()))
        .child(text)
}

/// The provider dot and the model.
fn model_tag(pane: &Pane) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .child(
            div()
                .size(px(6.))
                .rounded_full()
                .bg(rgb(provider_color(&pane.provider))),
        )
        .child(
            div()
                .font_family(tk().mono)
                .whitespace_nowrap()
                .child(pane.model.clone()),
        )
}

impl WarRoomScreen {
    /// The inner width of the board column in px: the window minus rail, sidebar and padding.
    fn board_inner_width(&self) -> f32 {
        let vw = f32::from(self.width);
        let class = width_class(vw);
        let rail = if vw < 720. { 52. } else { 76. };
        let (side, pad) = match class {
            WidthClass::Compact => (0., 16.),
            c => (side_width(c), 24.),
        };
        (vw - rail - side - 2. * pad).max(320.)
    }

    /// The row over the modules: what the lines mean, and the Motion switch.
    pub(super) fn render_legend(&self, cx: &mut Context<Self>) -> Div {
        let motion = self.board.motion;
        div()
            .flex_none()
            .h(px(36.))
            .flex()
            .items_center()
            .gap(px(16.))
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_3))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(tk().ink_2))
                    .child("Codebases"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child("One line per session, from its codebase to its gate"),
            )
            .child(super::stream::legend_item(
                SessionState::NeedsYou,
                "Needs you",
            ))
            .child(super::stream::legend_item(SessionState::Working, "Working"))
            .child(super::stream::legend_item(SessionState::Error, "Blocked"))
            .child(super::stream::legend_item(SessionState::Idle, "Idle"))
            .child(
                div()
                    .id("board-motion")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .tooltip(ui::tip("Moving dots on the active lines"))
                    .child(ui::switch(motion))
                    .child("Motion")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.board.motion = !this.board.motion;
                        cx.notify();
                    })),
            )
    }

    /// The module cards: live slots, the activity of the last minutes. A click filters the
    /// gates to the codebase; the pointer on a card brings its ribbons forward.
    pub(super) fn render_modules(
        &self,
        war: &WarRoom,
        mods: &[String],
        cx: &mut Context<Self>,
    ) -> Div {
        let slots = war.autopilot.max_per_codebase.clamp(1, 8) as usize;
        let filter = self.board.module.clone();
        let mut row = div().flex_none().flex().gap(px(GAP));
        for (i, cb) in mods.iter().enumerate() {
            let live: Vec<&Pane> = war
                .sessions
                .iter()
                .filter(|s| s.codebase == *cb && s.state.is_live() && s.state != SessionState::Idle)
                .collect();
            let on = filter.as_deref() == Some(cb.as_str());
            let dimmed = filter.is_some() && !on;
            let total = slots.max(live.len());
            let dots = (0..total).map(|k| {
                let color = live.get(k).map_or(tk().track_2, |s| state_tone(s.state));
                div().flex_1().h(px(5.)).rounded_full().bg(rgb(color))
            });
            let bars = self.board.bars.get(cb);
            let mut chart = div().h(px(26.)).flex().items_end().gap(px(2.));
            let offset = super::BARS - bars.map_or(0, |b| b.len());
            for k in 0..super::BARS {
                let value = k
                    .checked_sub(offset)
                    .and_then(|j| bars.and_then(|b| b.get(j)))
                    .copied()
                    .unwrap_or(0.);
                let is_last = bars.is_some_and(|b| b.len() > 1) && k == super::BARS - 1;
                chart = chart.child(
                    div()
                        .flex_1()
                        .h(relative(value.max(0.07)))
                        .rounded_t(px(2.))
                        .bg(rgb(if is_last { tk().ink } else { tk().track_2 })),
                );
            }
            let name = cb.clone();
            let hover_name = cb.clone();
            row = row.child(
                div()
                    .id(("module", i))
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w(px(0.))
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .bg(rgb(tk().card))
                    .rounded(tk().r_card)
                    .shadow(if on {
                        ui::ring(tk().ink, 2.)
                    } else {
                        ui::card_shadow()
                    })
                    .when(dimmed, |d| d.opacity(0.55))
                    .cursor_pointer()
                    .when(!on, |d| d.hover(|s| s.shadow(ui::card_shadow_hover())))
                    .tooltip(ui::tip(if on {
                        "Filter on: only this codebase. Click to show all."
                    } else {
                        "Show only this codebase"
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let next = hovered.then(|| hover_name.clone());
                        if this.board.hover != next
                            && (*hovered || this.board.hover.as_ref() == Some(&hover_name))
                        {
                            this.board.hover = next;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.board.module = if this.board.module.as_deref() == Some(name.as_str()) {
                            None
                        } else {
                            Some(name.clone())
                        };
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(ui::ts(T_SMALL))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(cb.clone()),
                            )
                            .child(if on {
                                div()
                                    .flex_none()
                                    .px(px(7.))
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .rounded_full()
                                    .bg(rgb(tk().dark))
                                    .text_color(rgb(tk().card))
                                    .text_size(ui::ts(11.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Filter ×")
                            } else {
                                div()
                                    .flex_none()
                                    .font_family(tk().mono)
                                    .text_size(ui::ts(11.))
                                    .text_color(rgb(tk().ink_3))
                                    .child(format!("{}/{}", live.len(), slots))
                            }),
                    )
                    .child(div().flex().gap(px(3.)).children(dots))
                    .child(chart),
            );
        }
        row
    }

    /// The four gates. An empty gate keeps its header only. The Needs you gate shows the ask
    /// slider; the other gates show one-line rows (the collapse rule, [`shows_card`]).
    pub(super) fn render_gates(
        &mut self,
        war: &WarRoom,
        gates: &Gates<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let counts = gates.counts();
        let spans = gate_spans(self.board_inner_width());
        let tier = tier(gates.live());
        // The slider answers the ask on screen; keep its target for the own-answer field.
        self.board.answer_for = self
            .slider_pane(gates)
            .and_then(|p| p.ask.as_ref().map(|a| (p.id.clone(), a.id.clone())));
        let mut row = div().flex_1().min_h(px(0.)).flex().gap(px(GAP));
        for which in [NEEDS, WORKING, BLOCKED, IDLE] {
            let list = &gates.lists[which];
            let head = self.gate_head(which, list, cx);
            if gate_empty(which, counts) {
                row = row.child(
                    div()
                        .flex_basis(px(0.))
                        .flex_grow(1.)
                        .min_w(px(0.))
                        .child(card().overflow_hidden().child(head)),
                );
                continue;
            }
            if shows_card(which) {
                row = row.child(
                    card()
                        .flex_basis(px(0.))
                        .flex_grow(1.)
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(head)
                        .child(self.ask_slider(war, list, cx)),
                );
                continue;
            }
            let scroll = self.board.gate_scroll[which].clone();
            // Whole rows only: from the height of the last frame, show the rows that fit and a
            // "+N more" line. A click on that line shows all rows (the column then scrolls).
            let more_key = format!("more:{which}");
            let all_rows = self.board.toggled.contains(&more_key);
            let view_h = f32::from(scroll.bounds().size.height);
            let limit = (!all_rows && view_h > 80.)
                .then(|| (((view_h - 20.) / ROW_H).floor() as usize).max(2));
            let body = self.gate_rows(war, which, list, tier, spans[which].1, limit, &more_key, cx);
            // More rows below the visible part: a fade and a hint at the foot of the column, so
            // the last row does not look cut off.
            let fade = more_below(&scroll);
            row = row.child(
                card()
                    .id(("gate-card", which))
                    .opacity(0.72)
                    .hover(|s| s.opacity(1.))
                    .flex_basis(px(0.))
                    .flex_grow(1.)
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(head)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id(("gate", which))
                                    .flex_1()
                                    .min_h(px(0.))
                                    .overflow_y_scroll()
                                    .track_scroll(&scroll)
                                    .px(px(10.))
                                    .py(px(10.))
                                    .child(body),
                            )
                            .children(fade)
                            .child(crate::ui::thumb(
                                &scroll,
                                gpui::rgba(tk().ink_3 << 8 | 0x80),
                                3.,
                            )),
                    ),
            );
        }
        row
    }

    /// A gate header: name, count, meaning, and the bulk action.
    fn gate_head(&self, which: usize, list: &[&Pane], cx: &mut Context<Self>) -> Div {
        let (name, meaning, text, tint) = gate_info(which);
        let empty = list.is_empty();
        let ids: Vec<String> = list.iter().map(|s| s.id.clone()).collect();
        let bulk = match which {
            IDLE if !empty => Some(
                ab("park-all", "Park all", false)
                    .bg(rgb(tk().card))
                    .tooltip(ui::tip("Park every idle session"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for id in &ids {
                            this.act(Action::Park(id.clone()), cx);
                        }
                    })),
            ),
            BLOCKED if list.len() > 1 => Some(
                ab("fix-all", "Fix all", false)
                    .bg(rgb(tk().card))
                    .tooltip(ui::tip("Tell every blocked agent to fix its error"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for id in &ids {
                            this.act(
                                Action::Send {
                                    id: id.clone(),
                                    text: FIX_TEXT.to_string(),
                                    composer: None,
                                },
                                cx,
                            );
                        }
                    })),
            ),
            _ => None,
        };
        div()
            .flex_none()
            .h(px(68.))
            .px(px(20.))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(rgb(if empty { tk().card } else { tint }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .text_size(ui::ts(T_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if empty { tk().ink_3 } else { text }))
                            .child(div().whitespace_nowrap().child(name))
                            .child(
                                div()
                                    .font_family(tk().mono)
                                    .font_weight(FontWeight::BOLD)
                                    .child(list.len().to_string()),
                            ),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(meaning),
                    ),
            )
            .children(bulk)
    }

    /// The rows of a gate. From 13 sessions the rows group by codebase.
    #[allow(clippy::too_many_arguments, reason = "one column of rows")]
    fn gate_rows(
        &self,
        war: &WarRoom,
        which: usize,
        list: &[&Pane],
        tier: Tier,
        width: f32,
        limit: Option<usize>,
        more_key: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let sel = self.focus_session.as_deref();
        let mut items: Vec<AnyElement> = Vec::new();
        if !matches!(tier, Tier::Rows | Tier::Closed) {
            for s in list {
                items.push(
                    self.session_row(war, s, sel == Some(s.id.as_str()), width, cx)
                        .into_any_element(),
                );
            }
        } else {
            for (cb, members) in groups(list) {
                let key = format!("{which}:{cb}");
                let hot = members.iter().any(|s| alert(s));
                let open = group_open(tier, hot, self.board.toggled.contains(&key));
                items.push(
                    self.group_head(&key, &cb, members.len(), hot, open, cx)
                        .into_any_element(),
                );
                if open {
                    for s in members {
                        items.push(
                            self.session_row(war, s, sel == Some(s.id.as_str()), width, cx)
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        let total = items.len();
        let mut hidden = 0;
        if let Some(n) = limit
            && total > n
        {
            // One line of the budget goes to the "+N more" hint.
            let keep = n.saturating_sub(1).max(1);
            hidden = total - keep;
            items.truncate(keep);
        }
        let mut col = div().flex().flex_col().gap(px(2.)).children(items);
        if hidden > 0 {
            let k = more_key.to_string();
            col = col.child(
                div()
                    .id(SharedString::from(format!("gate-more-{which}")))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .hover(|s| s.text_color(rgb(tk().ink)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.board.toggled.insert(k.clone());
                        cx.notify();
                    }))
                    .child(format!("+{hidden} more")),
            );
        }
        col
    }

    /// The head of a codebase group: a click opens or closes it.
    fn group_head(
        &self,
        key: &str,
        codebase: &str,
        count: usize,
        hot: bool,
        open: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let k = key.to_string();
        div()
            .id(SharedString::from(format!("grp-{key}")))
            .h(px(30.))
            .px(px(8.))
            .mt(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(tk().track)))
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_3))
            .child(ui::icon(
                if open {
                    "icons/twist-down.svg"
                } else {
                    "icons/twist-right.svg"
                },
                12.,
                rgb(tk().ink_3),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(tk().ink_2))
                    .child(codebase.to_string()),
            )
            .when(hot && !open, |d| d.child(ui::dot(rgb(tk().q))))
            .child(
                div()
                    .flex_none()
                    .font_family(tk().mono)
                    .child(count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.board.toggled.remove(&k) {
                    this.board.toggled.insert(k.clone());
                }
                cx.notify();
            }))
    }

    /// The ask slider of the Needs you gate: one large ask at a time, with the context that a
    /// person needs to decide.
    fn ask_slider(&self, war: &WarRoom, list: &[&Pane], cx: &mut Context<Self>) -> Div {
        let n = list.len();
        let ix = wrap_index(self.board.ask_ix, 0, n);
        let Some(pane) = list.get(ix).copied() else {
            return div();
        };
        let nav = |id: &'static str, arrow: &'static str, delta: isize, tip: &'static str| {
            div()
                .id(id)
                .size(px(28.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgb(tk().track))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk().track_2)))
                .tooltip(ui::tip(tip))
                .text_size(ui::ts(T_TITLE))
                .text_color(rgb(tk().ink_2))
                .child(arrow)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.board.ask_ix = wrap_index(this.board.ask_ix, delta, n);
                    cx.notify();
                }))
        };
        let mut dots = div().flex().items_center().gap(px(5.));
        if n <= 12 {
            for k in 0..n {
                dots = dots.child(
                    div()
                        .size(px(if k == ix { 7. } else { 5. }))
                        .rounded_full()
                        .bg(rgb(if k == ix { tk().q } else { tk().track_2 })),
                );
            }
        }
        let bar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_none()
            .pb(px(8.))
            .child(nav("ask-prev", "‹", -1, "Previous ask (Left or [)"))
            .child(
                div()
                    .flex_none()
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_2))
                    .child(format!("{} of {n}", ix + 1)),
            )
            .child(div().flex_1().flex().justify_center().child(dots))
            .child(nav("ask-next", "›", 1, "Next ask (Right or ])"));
        div()
            .flex_1()
            .min_h(px(0.))
            .p(px(16.))
            .flex()
            .flex_col()
            .child(bar)
            .child(self.ask_box(war, pane, cx))
    }

    /// One ask with its context: who asks, what it did last, the question, the full command or
    /// plan, the options with their keys, and an own answer.
    fn ask_box(&self, war: &WarRoom, pane: &Pane, cx: &mut Context<Self>) -> Div {
        let id = pane.id.clone();
        let ask = pane.ask.as_ref();
        let mut text_lines = ask.map_or(Vec::new(), |a| {
            a.text.lines().map(str::to_string).collect::<Vec<_>>()
        });
        let question = if text_lines.is_empty() {
            "Waits for you".to_string()
        } else {
            text_lines.remove(0)
        };
        let mut detail: Vec<String> = text_lines
            .into_iter()
            .skip_while(|l| l.trim().is_empty())
            .collect();
        if let Some(a) = ask {
            for (i, step) in a.steps.iter().enumerate() {
                detail.push(format!("{}. {step}", i + 1));
            }
            if let Some(reason) = &a.reason {
                detail.push(format!("Reason: {reason}"));
            }
        }
        let tail = last_lines(&pane.tail, 5);
        let mut tail_box = div()
            .p(px(10.))
            .rounded(px(8.))
            .bg(rgb(tk().track))
            .flex()
            .flex_col()
            .gap(px(2.))
            .font_family(tk().mono)
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_2));
        for (i, l) in tail.iter().enumerate() {
            let first = l.text.lines().next().unwrap_or("").trim();
            let line = match &l.label {
                Some(label) => format!("{label} {first}"),
                None => first.to_string(),
            };
            tail_box = tail_box.child(
                tip_line(SharedString::from(format!("ask-tail-{i}")), line).text_color(rgb(
                    match l.kind {
                        LineKind::Tool => tk().ink,
                        LineKind::Error => tk().e_text,
                        _ => tk().ink_2,
                    },
                )),
            );
        }
        let options: Vec<String> = ask.map_or(Vec::new(), |a| a.options.clone());
        let mut buttons = div().flex().flex_wrap().gap(px(6.));
        for (i, option) in options.iter().enumerate().take(9) {
            let pane_c = pane.clone();
            let opt = option.clone();
            buttons = buttons.child(
                ab(
                    SharedString::from(format!("ask-opt-{i}")),
                    format!("{}  {option}", i + 1),
                    i == 0,
                )
                .tooltip(ui::tip(format!("Key {}", i + 1)))
                .on_click(cx.listener(move |this, _, _, cx| this.answer_option(&pane_c, &opt, cx))),
            );
        }
        let input = self.board.answer.clone();
        let focus_id = id.clone();
        let ask_scroll = self.board.ask_scroll.clone();
        let ask_fade = more_below(&ask_scroll);
        let context = div()
            .id("ask-context")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&ask_scroll)
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(div().pt(px(6.)).child(ui::state_dot(pane.state, 8.)))
                    .child(
                        div()
                            .id("ask-title")
                            .flex_1()
                            .min_w(px(0.))
                            .max_h(px(44.))
                            .overflow_hidden()
                            .line_height(relative(1.45))
                            .text_size(ui::ts(T_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(tk().link)))
                            .tooltip(ui::tip("Open in Focus view"))
                            .child(pane.title.clone())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(WarRoomEvent::Focus(focus_id.clone()))
                            })),
                    ),
            )
            .child(
                // A narrow card wraps the row: the place keeps its own line, not "a…".
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(8.))
                    .gap_y(px(2.))
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(
                        tip_line(
                            "ask-where",
                            format!("{} / {} · {}", pane.codebase, pane.part, kind_label(pane)),
                        )
                        .flex_1()
                        .min_w(ui::sw(140.)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(tk().mono)
                            .text_color(rgb(tk().q_text))
                            .child(format!("waits {}", wait_text(pane, war.now))),
                    )
                    .child(model_tag(pane)),
            )
            .when(!tail.is_empty(), |d| d.child(tail_box))
            .when(!detail.is_empty(), |d| {
                d.child(
                    div()
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(rgb(tk().q_bg))
                        .font_family(tk().mono)
                        .text_size(ui::ts(T_CAPTION))
                        .text_color(rgb(tk().ink))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(detail.into_iter().map(|l| div().child(l))),
                )
            });
        let footer = div()
            .flex_none()
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .border_t_1()
            .border_color(ui::hsla(tk().line))
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .line_height(relative(1.45))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(tk().ink))
                    .child(question),
            )
            .child(buttons)
            .when_some(input, |d, input| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .h(px(34.))
                                .px(px(12.))
                                .flex()
                                .items_center()
                                .rounded_full()
                                .bg(rgb(tk().track))
                                .text_size(ui::ts(T_SMALL))
                                .child(div().flex_1().min_w(px(0.)).child(input)),
                        )
                        .child(
                            ab("ask-send", "Send", false)
                                .tooltip(ui::tip("Send the own answer (Enter)"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.send_own_answer(window, cx)
                                })),
                        ),
                )
            });
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .rounded(tk().r_card)
            .bg(rgb(tk().card))
            .border_1()
            .border_color(ui::hsla(tk().line))
            .overflow_hidden()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(context)
                    .children(ask_fade)
                    .child(crate::ui::thumb(
                        &ask_scroll,
                        gpui::rgba(tk().ink_3 << 8 | 0x80),
                        3.,
                    )),
            )
            .child(footer)
    }

    /// "Let it fix": the agent fixes its error and goes on.
    fn fix_button(&self, pane: &Pane, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = pane.id.clone();
        ab(SharedString::from(format!("fix-{id}")), "Let it fix", true)
            .tooltip(ui::tip("Tell the agent to fix the error and go on"))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.act(
                    Action::Send {
                        id: id.clone(),
                        text: FIX_TEXT.to_string(),
                        composer: None,
                    },
                    cx,
                );
            }))
    }

    /// The quick actions of a row under the pointer.
    fn quick_actions(&self, pane: &Pane, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let id = pane.id.clone();
        let mut acts: Vec<(&'static str, &'static str, Action)> = match pane.state {
            SessionState::Working => vec![
                ("Pause", "Pause after this step", Action::Pause(id.clone())),
                ("Park", "Park the session", Action::Park(id.clone())),
                ("Stop", "Stop the session", Action::Stop(id.clone())),
            ],
            _ => vec![
                ("Resume", "Resume the session", Action::Resume(id.clone())),
                ("Park", "Park the session", Action::Park(id.clone())),
                ("Stop", "Stop the session", Action::Stop(id.clone())),
            ],
        };
        let mut out: Vec<Stateful<Div>> = Vec::new();
        if pane.state == SessionState::Error {
            out.push(self.fix_button(pane, cx));
            acts.remove(0);
        }
        for (label, tip, action) in acts {
            out.push(
                div()
                    .id(SharedString::from(format!("qa-{label}-{id}")))
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .rounded_full()
                    .bg(rgb(tk().card))
                    .border_1()
                    .border_color(ui::hsla(tk().line_2))
                    .text_size(ui::ts(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(tk().track_2)))
                    .tooltip(ui::tip(tip))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.act(action.clone(), cx);
                    })),
            );
        }
        out
    }

    /// A one-line row: dot, title, codebase/part, tool or error or idle time, time. A click
    /// opens the Focus view; the pointer shows the quick actions.
    fn session_row(
        &self,
        war: &WarRoom,
        pane: &Pane,
        on: bool,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let session = pane.id.clone();
        let hover_id = pane.id.clone();
        let hovered = self.board.hover_row.as_deref() == Some(pane.id.as_str());
        let (text, color) = match state_line(pane, war.now) {
            StateLine::Ask { question, .. } => (question, tk().q_text),
            StateLine::Tool(t) => (t, tk().ink_3),
            StateLine::Error(t) => (t, tk().e_text),
            StateLine::Idle(t) => (t, tk().ink_3),
        };
        let text_fits = width >= 520.;
        let actions = if hovered {
            self.quick_actions(pane, cx)
        } else {
            Vec::new()
        };
        div()
            .id(SharedString::from(format!("row-{}", pane.id)))
            .min_h(px(36.))
            .py(px(4.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(8.))
            .line_height(relative(1.45))
            .cursor_pointer()
            .when(on, |d| d.bg(rgb(tk().track_2)))
            .when(!on, |d| d.hover(|s| s.bg(rgb(tk().track))))
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                let next = h.then(|| hover_id.clone());
                if this.board.hover_row != next
                    && (*h || this.board.hover_row.as_ref() == Some(&hover_id))
                {
                    this.board.hover_row = next;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(WarRoomEvent::Focus(session.clone()))))
            .tooltip(ui::tip(format!(
                "{}\n{} / {} · {} {}\n{text}",
                pane.title, pane.codebase, pane.part, pane.provider, pane.model
            )))
            .child(ui::state_dot(pane.state, 7.))
            .child(
                div()
                    .min_w(px(0.))
                    .max_h(px(38.))
                    .overflow_hidden()
                    .flex_grow(1.)
                    .flex_basis(px(0.))
                    .text_size(ui::ts(T_SMALL))
                    .font_weight(FontWeight::MEDIUM)
                    .child(pane.title.clone()),
            )
            .when(hovered, |d| d.children(actions))
            .when(!hovered && text_fits, |d| {
                d.child(
                    div()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .flex_grow(1.)
                        .flex_basis(px(0.))
                        .font_family(tk().mono)
                        .text_size(ui::ts(T_CAPTION))
                        .text_color(rgb(color))
                        .child(text),
                )
            })
            .when(!hovered, |d| {
                d.child(
                    div()
                        .flex_none()
                        .w(px(30.))
                        .flex()
                        .justify_end()
                        .font_family(tk().mono)
                        .text_size(ui::ts(T_CAPTION))
                        .text_color(rgb(tk().ink_3))
                        .child(wait_text(pane, war.now)),
                )
            })
    }
}
