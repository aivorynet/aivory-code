//! The strip on top of the Focus view: back to the war room, one chip per live session
//! (the ones that need you first), the count, and New session. A hover on a chip that needs
//! you opens its question with the answers.

use std::time::Duration;

use aivory_protocol::rpc::{NewSessionParams, method};
use aivory_protocol::{Ask, Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, Context, Div, FontWeight, SharedString, Window, anchored, canvas, deferred,
    div, point, px, rgb, rgba,
};
use gpui_base::input::TextareaState;

use super::{FocusEvent, FocusScreen};
use crate::theme::*;
use crate::ui;
use crate::warroom::{effort_label, mode_label};

/// What a chip needs to know about its session.
struct Chip {
    id: String,
    title: String,
    state: SessionState,
    wait: String,
    ask: Option<Ask>,
    controlled: bool,
    codebase: String,
    provider: String,
}

/// `6m` for a wait of 6 minutes.
fn wait_text(minutes: i64) -> String {
    if minutes >= 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

impl FocusScreen {
    fn chips(&self, cx: &gpui::App) -> Vec<Chip> {
        let Some(war) = self.war(cx) else {
            return Vec::new();
        };
        let mut chips: Vec<Chip> = war
            .sessions
            .iter()
            .filter(|s| s.state.is_live())
            .map(|s| Chip {
                id: s.id.clone(),
                title: s.title.clone(),
                state: s.state,
                wait: wait_text(s.minutes(war.now)),
                ask: s.ask.clone(),
                controlled: s.controlled,
                codebase: s.codebase.clone(),
                provider: s.provider.clone(),
            })
            .collect();
        // The sessions that need you come first; the order inside a group stays.
        chips.sort_by_key(|c| !matches!(c.state, SessionState::NeedsYou | SessionState::Error));
        chips
    }

    /// The pointer enters or leaves a chip.
    fn hover_chip(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            self.chip_hover = Some(id.to_string());
            self.pop_for = Some(id.to_string());
        } else if self.chip_hover.as_deref() == Some(id) {
            self.chip_hover = None;
            self.settle(cx);
        }
        cx.notify();
    }

    /// The pointer enters or leaves the popup.
    fn hover_pop(&mut self, on: bool, cx: &mut Context<Self>) {
        self.pop_hover = on;
        if !on {
            self.settle(cx);
        }
    }

    /// Close the popup shortly after the pointer left both the chip and the popup.
    fn settle(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(160))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.chip_hover.is_none() && !this.pop_hover && this.pop_for.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn strip(&mut self, pane: &Pane, cx: &mut Context<Self>) -> Div {
        let chips = self.chips(cx);
        let need = chips
            .iter()
            .filter(|c| matches!(c.state, SessionState::NeedsYou | SessionState::Error))
            .count();
        let back = div()
            .id("focus-back")
            .flex_none()
            .h(ui::sw(34.))
            .pl(px(10.))
            .pr(px(12.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded_full()
            .bg(rgb(tk().dark))
            .text_color(rgb(0xFFFFFF))
            .text_size(ui::ts(T_SMALL))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(|s| s.bg(rgb(tk().dark_hover)))
            .tooltip(ui::tip("Back to the war room (Esc)"))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(FocusEvent::Back)))
            .child(ui::icon("icons/back.svg", 15., rgb(0xFFFFFF)))
            .child("War room")
            .child(
                div()
                    .font_family(tk().mono)
                    .text_size(ui::ts(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .opacity(0.7)
                    .child("Esc"),
            );
        let mut row = div()
            .id("focus-chips")
            .size_full()
            .flex()
            .items_center()
            .gap(px(6.))
            .overflow_x_scroll();
        let mut sep_done = false;
        for c in &chips {
            let needs = matches!(c.state, SessionState::NeedsYou | SessionState::Error);
            if !needs && !sep_done && need > 0 {
                sep_done = true;
                row = row.child(
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(22.))
                        .mx(px(4.))
                        .bg(rgb(tk().track_2)),
                );
            }
            row = row.child(self.chip(c, needs, cx));
        }
        row = row.track_scroll(&self.strip_scroll);
        // Bring the chip of this session into view once the chips are measured.
        if self.strip_for.as_deref() != Some(&self.session) {
            let view = self.strip_scroll.bounds();
            if let (Some(b), true) = (
                self.chip_bounds.get(&self.session),
                view.size.width > px(0.),
            ) {
                let over = b.right() - view.right();
                if over > px(0.) {
                    let mut off = self.strip_scroll.offset();
                    off.x -= over + px(12.);
                    self.strip_scroll.set_offset(off);
                }
                self.strip_for = Some(self.session.clone());
            } else {
                // The chips are measured in the first frame: draw once more, a few times at most.
                self.strip_tries += 1;
                if self.strip_tries < 4 {
                    cx.notify();
                } else {
                    self.strip_for = Some(self.session.clone());
                }
            }
        }
        let count = format!("{} live · {need} need you", chips.len());
        let _ = pane;
        div()
            .flex_none()
            .min_h(ui::sw(52.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(rgb(tk().page))
            .border_b_1()
            .border_color(rgba(tk().line))
            .child(back)
            .child(
                // The chips end in a fade, so no chip meets the count.
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .overflow_hidden()
                    .child(row)
                    .child(div().absolute().top_0().bottom_0().right_0().w(px(36.)).bg(
                        gpui::linear_gradient(
                            90.,
                            gpui::linear_color_stop(rgba(tk().page << 8), 0.),
                            gpui::linear_color_stop(rgba(tk().page << 8 | 0xFF), 1.),
                        ),
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(count),
            )
            .child(self.new_session_button(cx))
    }

    fn chip(&self, c: &Chip, needs: bool, cx: &mut Context<Self>) -> AnyElement {
        let current = c.id == self.session;
        let id = c.id.clone();
        let hover_id = c.id.clone();
        let weak = cx.entity().downgrade();
        let bounds_id = c.id.clone();
        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id(SharedString::from(format!("chip-{}", c.id)))
                    .h(ui::sw(34.))
                    .max_w(px(230.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .rounded_full()
                    .bg(rgb(if needs { tk().q_bg } else { tk().track }))
                    .text_size(ui::ts(T_SMALL))
                    .cursor_pointer()
                    .when(current, |d| {
                        d.bg(rgb(tk().page)).shadow(ui::ring(tk().ink, 2.))
                    })
                    .when(!current, |d| {
                        d.hover(move |s| {
                            s.bg(rgb(if needs {
                                crate::theme::pick(0xF9DDE3, tk().q_bg)
                            } else {
                                tk().track_2
                            }))
                        })
                    })
                    .when(c.state == SessionState::Idle && !current, |d| {
                        d.opacity(0.6)
                    })
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(FocusEvent::Open(id.clone()))))
                    .when(needs, |d| {
                        d.on_hover(cx.listener(move |this, on: &bool, _, cx| {
                            this.hover_chip(&hover_id, *on, cx)
                        }))
                    })
                    .child(ui::state_dot(c.state, 8.))
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(c.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(tk().mono)
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(if needs { tk().q_text } else { tk().ink_3 }))
                            .when(needs, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(c.wait.clone()),
                    )
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                let _ = weak.update(cx, |this, _| {
                                    this.chip_bounds.insert(bounds_id.clone(), bounds);
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    /// The popup of a chip that needs you: the question and the answers.
    pub(super) fn popup(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.pop_for.clone()?;
        let bounds = *self.chip_bounds.get(&id)?;
        let chip = self.chips(cx).into_iter().find(|c| c.id == id)?;
        let ask = chip.ask.clone()?;
        let needs_color = if chip.state == SessionState::Error {
            tk().e_text
        } else {
            tk().q_text
        };
        let mut answers = div().flex().flex_wrap().gap(px(6.));
        if chip.controlled {
            for (i, option) in ask.options.iter().enumerate() {
                let (session, ask_id, value) = (chip.id.clone(), ask.id.clone(), option.clone());
                answers = answers.child(
                    ui::button_sm(
                        SharedString::from(format!("pop-{}-{i}", chip.id)),
                        super::chat::option_label(ask.kind, option, &ask.text),
                        i == 0,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.answer(&session, &ask_id, Some(value.clone()), None, cx);
                        this.pop_for = None;
                        cx.notify();
                    })),
                );
            }
        } else {
            answers = answers.child(
                div()
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child("Answer in its terminal"),
            );
        }
        answers = answers.child(
            ui::button_sm("pop-later", "Later", false).on_click(cx.listener(|this, _, _, cx| {
                this.pop_for = None;
                cx.notify();
            })),
        );
        let card = div()
            .w(px(340.))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .bg(rgb(tk().card))
            .rounded(px(14.))
            .shadow(ui::popover_shadow())
            .child(
                div()
                    .text_size(ui::ts(T_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(needs_color))
                    .child(format!(
                        "{} · {} · {} · {}",
                        ask.kind.tag(),
                        chip.codebase,
                        chip.provider,
                        chip.wait
                    )),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(ui::ts(T_SMALL))
                    .child(chip.title.clone()),
            )
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .line_height(gpui::relative(1.45))
                    .text_color(rgb(tk().ink_2))
                    .when(ask.kind == aivory_protocol::AskKind::Permission, |d| {
                        d.font_family(tk().mono)
                    })
                    .child(ask.text.clone()),
            )
            .child(answers);
        // A transparent strip joins the chip and the card: the pointer never leaves both.
        let at = point(bounds.left(), bounds.bottom());
        Some(
            deferred(
                anchored()
                    .position(at)
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("focus-pop")
                            .pt(px(6.))
                            .occlude()
                            .on_hover(cx.listener(|this, on: &bool, _, cx| this.hover_pop(*on, cx)))
                            .child(card),
                    ),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    /// The New session button with its popover: a prompt for a session in the same codebase.
    fn new_session_button(&mut self, cx: &mut Context<Self>) -> Div {
        let open = self.new_input.is_some();
        let mut wrap = div().relative().flex_none().child(
            ui::dark_button("focus-new", "+ New session")
                .h(ui::sw(34.))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_new(window, cx))),
        );
        if let Some(input) = self.new_input.clone().filter(|_| open) {
            let panel = div()
                .w(px(360.))
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .bg(rgb(tk().card))
                .rounded(px(14.))
                .shadow(ui::popover_shadow())
                .child(
                    div()
                        .text_size(ui::ts(T_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(tk().ink_3))
                        .child(self.new_scope(cx)),
                )
                .child(
                    div()
                        .p(px(8.))
                        .rounded(px(10.))
                        .bg(rgb(tk().track))
                        .text_size(ui::ts(T_BODY))
                        .child(input),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(6.))
                        .child(ui::button_sm("focus-new-no", "Cancel", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.new_input = None;
                                cx.notify();
                            }),
                        ))
                        .child(
                            ui::button_sm("focus-new-go", "Start", true)
                                .on_click(cx.listener(|this, _, _, cx| this.start_session(cx))),
                        ),
                );
            wrap = wrap.child(
                deferred(
                    div()
                        .absolute()
                        .top(px(42.))
                        .right_0()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.new_input = None;
                            cx.notify();
                        }))
                        .child(panel),
                )
                .with_priority(3),
            );
        }
        wrap
    }

    /// `aivory-code / ui · claude opus-5.5 · Edit` above the prompt.
    fn new_scope(&self, cx: &gpui::App) -> String {
        match self.pane(cx) {
            Some(p) => format!(
                "New session in {} / {} · {} {} · {} · {}",
                p.codebase,
                p.part,
                p.provider,
                p.model,
                mode_label(p.composer.mode),
                effort_label(p.composer.effort)
            ),
            None => "New session".to_string(),
        }
    }

    fn toggle_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_input.take().is_none() {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("What should the new session do?")
                    .auto_grow(3, 6)
            });
            input.update(cx, |s, cx| s.focus(window, cx));
            self.new_input = Some(input);
        }
        cx.notify();
    }

    /// `session/new` in the codebase and part of the session on screen, then open it.
    fn start_session(&mut self, cx: &mut Context<Self>) {
        let (Some(pane), Some(input)) = (self.pane(cx), self.new_input.clone()) else {
            return;
        };
        let prompt = input.read(cx).value().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let task = self.model.read(cx).call_notify::<method::SessionNew>(
            NewSessionParams {
                codebase: pane.codebase.clone(),
                part: Some(pane.part.clone()),
                title: None,
                prompt,
                composer: pane.composer.clone(),
                task: None,
                attachments: Vec::new(),
            },
            cx,
        );
        self.new_input = None;
        cx.spawn(async move |this, cx| {
            if let Some(created) = task.await {
                let _ = this.update(cx, |_, cx| cx.emit(FocusEvent::Open(created.id)));
            }
        })
        .detach();
        cx.notify();
    }
}
