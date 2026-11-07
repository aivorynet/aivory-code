//! The Decisions section of war room v2: one white tile per open ask, the oldest first, and the
//! context popup with the chat and the code behind a decision.

use aivory_protocol::rpc::{AnswerParams, method};
use aivory_protocol::session::DiffLineKind;
use aivory_protocol::{Ask, AskKind, Pane, SessionState, WarRoom};
use gpui::AnimationExt;
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, FontWeight, SharedString, deferred, div, px, relative, rgb, rgba,
};

use super::pane::option_label;
use super::{BodyKey, FocusTab, WarRoomScreen};
use crate::theme::*;
use crate::ui;

/// The narrowest decision tile (`minmax(320px, 1fr)`).
pub(super) const TILE_MIN: f32 = 320.;
pub(super) const TILE_GAP: f32 = 16.;

/// The tile columns for a grid width.
pub(super) fn tile_columns(width: f32) -> u16 {
    (((width + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor() as u16).max(1)
}

/// The live sessions with an open ask, the longest wait first.
pub(super) fn decisions(war: &WarRoom) -> Vec<&Pane> {
    let mut list: Vec<&Pane> = war
        .sessions
        .iter()
        .filter(|s| s.state.is_live() && s.ask.is_some())
        .collect();
    list.sort_by_key(|s| s.since);
    list
}

/// The short kind of a tile: `Permission`, `Plan`, `Question`, `Error`.
fn kind_label(kind: AskKind) -> &'static str {
    match kind {
        AskKind::Permission => "Permission",
        AskKind::Plan => "Plan",
        AskKind::Question => "Question",
        AskKind::Error => "Error",
    }
}

/// The question of a tile as one sentence.
pub(super) fn question(pane: &Pane, ask: &Ask) -> String {
    match ask.kind {
        AskKind::Permission => format!("Allow {} to run {}?", pane.provider, ask.text),
        AskKind::Plan => format!("Approve the plan: {}", ask.text),
        _ => ask.text.clone(),
    }
}

fn state_text(state: SessionState) -> u32 {
    if state == SessionState::Error {
        tk().e_text
    } else {
        tk().q_text
    }
}

impl WarRoomScreen {
    /// `Decisions · N waiting for you`, then the tiles.
    pub(super) fn render_decisions(
        &self,
        war: &WarRoom,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let list = decisions(war);
        let mut head = section_head(
            "Decisions",
            (!list.is_empty()).then(|| format!("{} waiting for you", list.len())),
        );
        if !list.is_empty() {
            head = head.child(div().flex_1()).child(
                ui::button_sm("walk", "Answer one by one", false).on_click(cx.listener(
                    |this, _, _, cx| {
                        let first = this
                            .war
                            .as_ref()
                            .and_then(|w| decisions(w).first().map(|s| s.id.clone()));
                        if let Some(id) = first {
                            this.open_focus(&id, FocusTab::Chat, cx);
                            this.walk = true;
                        }
                    },
                )),
            );
        }
        let body = if list.is_empty() {
            div()
                .p(px(18.))
                .rounded(tk().r_card)
                .border_1()
                .border_color(rgba(tk().line_2))
                .text_color(rgb(tk().ink_3))
                .text_size(ui::ts(T_BODY))
                .child("Nothing is waiting for you.")
        } else {
            let mut grid = div()
                .w_full()
                .grid()
                .grid_cols(tile_columns(width))
                .gap(px(TILE_GAP));
            for pane in list {
                if let Some(ask) = &pane.ask {
                    grid = grid.child(
                        self.tile(pane, ask, war.now, cx).with_animation(
                            SharedString::from(format!("tile-{}", ask.id)),
                            gpui::Animation::new(std::time::Duration::from_millis(360))
                                .with_easing(gpui::ease_out_quint()),
                            |el, t| el.opacity(t),
                        ),
                    );
                }
            }
            grid
        };
        div().w_full().flex().flex_col().child(head).child(body)
    }

    fn tile(
        &self,
        pane: &Pane,
        ask: &Ask,
        now: aivory_protocol::UnixMs,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.context.as_deref() == Some(pane.id.as_str());
        let toggle = |id: &str| {
            let id = id.to_string();
            cx.listener(move |this, _, _, cx| {
                this.context = (this.context.as_deref() != Some(id.as_str())).then(|| id.clone());
                cx.notify();
            })
        };
        let toggle_question = toggle(&pane.id);
        let toggle_link = toggle(&pane.id);
        let mut footer = div()
            .mt_auto()
            .pt(px(6.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.));
        if pane.controlled {
            for (i, option) in ask.options.iter().take(2).enumerate() {
                footer = footer.child(self.answer_button(pane, ask, i, option, ("tile", i), cx));
            }
        } else {
            footer = footer.child(terminal_note());
        }
        footer = footer.child(div().flex_1()).child(
            div()
                .id(SharedString::from(format!("ctx-{}", pane.id)))
                .px(px(4.))
                .py(px(6.))
                .text_size(ui::ts(T_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(tk().link))
                .cursor_pointer()
                .hover(|s| s.underline())
                .on_click(toggle_link)
                .child("Context"),
        );
        div()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(20.))
            .px(px(22.))
            .pb(px(18.))
            .bg(rgb(tk().card))
            .rounded(tk().r_tile)
            .shadow(if open {
                ui::ring(tk().ink, 2.)
            } else {
                ui::card_shadow()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(ui::state_dot(pane.state, 8.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(state_text(pane.state)))
                            .child(kind_label(ask.kind)),
                    )
                    .child(div().flex_1())
                    .child(format!("{} min ago", pane.minutes(now))),
            )
            .child(
                div()
                    .id(SharedString::from(format!("dq-{}", pane.id)))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_color(rgb(tk().ink))
                    .cursor_pointer()
                    .tooltip(ui::tip("Show the chat and code behind this decision"))
                    .on_click(toggle_question)
                    .map(|d| {
                        if ask.kind == AskKind::Permission {
                            // A command is long: a short line, then the command in mono, 3 lines.
                            d.child(
                                div()
                                    .font_family(tk().display_heading)
                                    .text_size(ui::ts(17.))
                                    .font_weight(FontWeight::NORMAL)
                                    .child(format!("Allow {} to run", pane.provider)),
                            )
                            .child(
                                div()
                                    .font_family(tk().mono)
                                    .text_size(ui::ts(T_SMALL))
                                    .line_height(px(20.))
                                    .text_color(rgb(tk().ink_2))
                                    .line_clamp(3)
                                    .child(ask.text.clone()),
                            )
                        } else {
                            d.child(
                                div()
                                    .font_family(tk().display_heading)
                                    .text_size(ui::ts(18.))
                                    .line_height(relative(1.3))
                                    .font_weight(FontWeight::NORMAL)
                                    .line_clamp(5)
                                    .child(question(pane, ask)),
                            )
                        }
                    }),
            )
            .child(
                div()
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(format!(
                        "{} / {} · {}",
                        pane.codebase, pane.part, pane.provider
                    )),
            )
            .child(footer)
    }

    /// An answer pill: the first option is the graphite default.
    pub(super) fn answer_button(
        &self,
        pane: &Pane,
        ask: &Ask,
        i: usize,
        option: &str,
        id: (&'static str, usize),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let params = AnswerParams {
            id: pane.id.clone(),
            ask: ask.id.clone(),
            option: Some(option.to_string()),
            text: None,
        };
        let session = pane.id.clone();
        ui::button_sm(
            SharedString::from(format!("{}-{}-{}", id.0, pane.id, id.1)),
            option_label(ask.kind, option, &ask.text),
            i == 0,
        )
        .when(i > 0 && id.0 == "tile", |d| d.bg(rgb(tk().track)))
        .tooltip(ui::tip(format!("Key {}", i + 1)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.send::<method::SessionAnswer>(params.clone(), cx);
            this.answered(&session, cx);
        }))
    }

    /// The context popup of a decision: the question and the answers on top, the chat on the
    /// left, the first changed file on the right.
    pub(super) fn render_context(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let war = self.war.clone()?;
        let id = self.context.clone()?;
        let pane = war.session(&id)?.clone();
        let Some(ask) = pane.ask.clone() else {
            self.context = None;
            return None;
        };
        self.ensure_diff(&id, cx);
        let key = BodyKey {
            session: id.clone(),
            subagent: None,
            focus: true,
        };
        self.ensure_body(&key, &pane, cx);
        let mut answers = div().flex().flex_wrap().gap(px(8.)).pb(px(20.));
        if pane.controlled {
            for (i, option) in ask.options.iter().enumerate() {
                answers =
                    answers.child(self.answer_button(&pane, &ask, i, option, ("ctxa", i), cx));
            }
        } else {
            answers = answers.child(terminal_note());
        }
        let close = cx.listener(|this, _, _, cx| {
            this.context = None;
            cx.notify();
        });
        let chat = div()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(column_label("Chat", None))
            .child(div().h(px(380.)).child(self.transcript(&key, cx)))
            .when(!ask.steps.is_empty(), |d| {
                let mut steps = div().mt(px(12.)).flex().flex_col().gap(px(4.));
                for (i, step) in ask.steps.iter().enumerate() {
                    steps = steps.child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_size(ui::ts(T_SMALL))
                            .child(
                                div()
                                    .text_color(rgb(tk().ink_3))
                                    .child(format!("{}.", i + 1)),
                            )
                            .child(div().flex_1().min_w(px(0.)).child(step.clone())),
                    );
                }
                d.child(steps)
            });
        let file = self
            .diffs
            .get(&id)
            .and_then(|(d, _)| d.as_ref())
            .and_then(|d| d.files.iter().find(|f| !f.lines.is_empty()).cloned());
        let code = div().min_w(px(0.)).flex().flex_col().map(|d| match file {
            Some(f) => d
                .child(column_label(
                    &f.path,
                    Some(format!("+{} −{}", f.added, f.removed)),
                ))
                .child(
                    div()
                        .id("ctx-diff")
                        .h(px(380.))
                        .overflow_y_scroll()
                        .rounded(tk().r_button)
                        .bg(rgb(tk().track))
                        .py(px(6.))
                        .child(diff_rows(&f.lines, true)),
                ),
            None => d.child(column_label("Code", None)).child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child("No changed files yet."),
            ),
        });
        let modal = div()
            .id("ctx-modal")
            .absolute()
            .top(px(54.))
            .left(relative(0.5))
            .ml(px(-540.))
            .w(px(1080.))
            .flex()
            .flex_col()
            .gap(px(18.))
            .py(px(28.))
            .px(px(32.))
            .bg(rgb(tk().card))
            .rounded(px(28.))
            .shadow(ui::popover_shadow())
            .occlude()
            .child(
                div()
                    .text_size(ui::ts(T_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(state_text(pane.state)))
                    .child(format!(
                        "{} · waits {} min",
                        ask.kind.label(),
                        pane.minutes(war.now)
                    )),
            )
            .child(
                div()
                    .max_w(px(760.))
                    .text_size(ui::ts(18.))
                    .line_height(relative(1.4))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(question(&pane, &ask)),
            )
            .child(answers)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(ui::ts(T_BODY))
                            .child(pane.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(tk().mono)
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(format!(
                                "{} / {} · {} {}",
                                pane.codebase, pane.part, pane.provider, pane.model
                            )),
                    )
                    .child(div().flex_1())
                    .child(ui::button_sm("ctx-open", "Open session", false).on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.context = None;
                            this.open_focus(&id, FocusTab::Chat, cx);
                        })
                    }))
                    .child(
                        div()
                            .id("ctx-close")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(tk().track)))
                            .tooltip(ui::tip("Close"))
                            .on_click(close)
                            .child(ui::icon("icons/close.svg", 13., rgb(tk().ink_3))),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(px(28.))
                    .child(chat)
                    .child(code),
            );
        Some(
            deferred(
                div()
                    .absolute()
                    .inset_0()
                    .child(
                        div()
                            .id("ctx-scrim")
                            .absolute()
                            .inset_0()
                            .bg(rgba(0x00000052))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context = None;
                                cx.notify();
                            })),
                    )
                    .child(modal),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

/// In place of the answers of a session that runs in a terminal: aivoryd cannot answer it.
pub(super) fn terminal_note() -> Div {
    div()
        .py(px(6.))
        .text_size(ui::ts(T_SMALL))
        .text_color(rgb(tk().ink_3))
        .child("Answer in its terminal")
}

/// An `h2` of the page with its meta text (`.sh`).
pub(super) fn section_head(title: &'static str, meta: Option<String>) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(14.))
        .mb(px(20.))
        .child(
            div()
                .font_family(tk().display_heading)
                .text_size(ui::ts(22.))
                .font_weight(FontWeight::NORMAL)
                .child(title),
        )
        .when_some(meta, |d, m| {
            d.child(
                div()
                    .text_size(ui::ts(T_BODY))
                    .text_color(rgb(tk().ink_3))
                    .child(m),
            )
        })
}

fn column_label(left: &str, right: Option<String>) -> Div {
    div()
        .mb(px(10.))
        .flex()
        .justify_between()
        .font_family(tk().mono)
        .text_size(ui::ts(T_CAPTION))
        .text_color(rgb(tk().ink_3))
        .child(left.to_string())
        .children(right)
}

/// Diff rows: old number, new number, text. `soft` sits on a grey surface (no row tint for
/// hunks and context).
pub(super) fn diff_rows(lines: &[aivory_protocol::session::DiffLine], soft: bool) -> Div {
    let mut rows = div()
        .flex()
        .flex_col()
        .font_family(tk().mono)
        .text_size(ui::ts(T_CAPTION))
        .line_height(px(21.));
    for line in lines {
        let (bg, fg) = match line.kind {
            DiffLineKind::Added => (
                Some(crate::theme::pick(0xF0FAF3, tk().ok_bg)),
                crate::theme::pick(0x14532D, tk().ok_text),
            ),
            DiffLineKind::Removed => (
                Some(crate::theme::pick(0xFEF3F3, tk().q_bg)),
                crate::theme::pick(0x7F1D1D, tk().q_text),
            ),
            DiffLineKind::Hunk => (
                (!soft).then_some(crate::theme::pick(0xF3F5F8, tk().track)),
                tk().ink_3,
            ),
            DiffLineKind::Context => (None, tk().ink_2),
        };
        let no = |n: Option<u32>| {
            div()
                .w(px(40.))
                .flex_none()
                .pr(px(10.))
                .flex()
                .justify_end()
                .text_color(rgb(crate::theme::pick(0x6B7385, tk().quiet)))
                .child(n.map(|n| n.to_string()).unwrap_or_default())
        };
        rows = rows.child(
            div()
                .flex()
                .when_some(bg, |d, c| d.bg(rgb(c)))
                .child(no(line.old))
                .child(no(line.new))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(rgb(fg))
                        .child(line.text.clone()),
                ),
        );
    }
    rows
}
