//! The center of the Focus view: the header of the session, its plan, the transcript with a
//! turn number on every tool row, the ask card with all answers, and the full composer.

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::{Ask, AskKind, Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Focusable, FontWeight, SharedString, Window, div, list, px, relative,
    rgb, rgba,
};

use super::FocusScreen;
use super::items::{self, EditRow, Item};
use crate::compose;
use crate::console::blocks::{Pending, ToolState};
use crate::console::todo::TodoState;
use crate::theme::*;
use crate::ui;

/// The orange of the agent: the turn tag and the live dot (`--claude`).
pub(super) const CLAUDE: u32 = 0xD97757;
fn soft_panel() -> u32 {
    crate::theme::pick(0xFAFAFC, tk().card)
}

/// The ask of the session on screen, kept for the list item that draws it.
#[derive(Clone)]
pub(super) struct AskCard {
    pub session: String,
    pub ask: Ask,
    pub controlled: bool,
    pub state: SessionState,
}

/// The words of a permission option, as the dialog of the agent CLIs says them.
pub(super) fn option_label(kind: AskKind, option: &str, command: &str) -> String {
    if kind != AskKind::Permission {
        return option.to_string();
    }
    match option {
        "Allow" => "Allow".to_string(),
        "Always allow" => {
            let word = command.split_whitespace().next().unwrap_or("this");
            format!("Always allow {word}")
        }
        other => other.to_string(),
    }
}

fn state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::NeedsYou => "Needs you",
        SessionState::Error => "Error",
        SessionState::Working => "Working",
        SessionState::Idle => "Idle",
        SessionState::Parked => "Parked",
        SessionState::Done => "Done",
    }
}

impl FocusScreen {
    pub(super) fn chat(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.sync_items(pane);
        let body = div()
            .relative()
            .flex_1()
            .min_h(px(96.))
            .px(px(18.))
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix: usize, _, cx| this.chat_item(ix, cx)),
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(gpui_base::Scrollbar::vertical(&self.list)),
            );
        compose::media_target(
            div().flex_1().min_w(ui::sw(380.)).h_full(),
            |this: &mut Self| Some(&mut this.tools),
            cx,
        )
        .flex()
        .flex_col()
        .bg(rgb(tk().card))
        .border_r_1()
        .border_color(rgba(tk().line))
        .child(self.chat_head(pane, cx))
        .children(self.plan(pane))
        .child(body)
        .child(self.composer(pane, window, cx))
    }

    /// Bring the list in line with the transcript of the pane.
    fn sync_items(&mut self, pane: &Pane) {
        let pending = match (pane.state, pane.ask.as_ref().map(|a| a.kind)) {
            (SessionState::Working, _) => Pending::Running,
            (SessionState::NeedsYou, Some(AskKind::Permission)) => Pending::Waiting,
            _ => Pending::No,
        };
        let start = pane.lines_total.saturating_sub(pane.tail.len() as u32);
        let mut new: Vec<Item> = items::items(&pane.tail, start, pending)
            .into_iter()
            .filter(|i| !matches!(i, Item::Meta(_)))
            .collect();
        if pane.state != SessionState::Working {
            for i in &mut new {
                if let Item::Edit(e) = i {
                    e.live = false;
                }
            }
        }
        let card = pane.ask.as_ref().map(|a| AskCard {
            session: pane.id.clone(),
            ask: a.clone(),
            controlled: pane.controlled,
            state: pane.state,
        });
        let ask_key = |c: &Option<AskCard>| c.as_ref().map(|c| c.ask.id.clone());
        let ask_changed = ask_key(&card) != ask_key(&self.ask_card);
        if new == self.items && !ask_changed {
            self.ask_card = card;
            return;
        }
        let old_n = self.items.len();
        let had = self.ask_card.is_some();
        if had {
            self.list.splice(old_n..old_n + 1, 0);
        }
        let new_n = new.len();
        let prefix = self
            .items
            .iter()
            .zip(&new)
            .take_while(|(a, b)| a == b)
            .count();
        let room = old_n.min(new_n) - prefix;
        let suffix = self
            .items
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        if prefix + suffix < old_n || prefix + suffix < new_n {
            self.list
                .splice(prefix..old_n - suffix, new_n - prefix - suffix);
        }
        if card.is_some() {
            self.list.splice(new_n..new_n, 1);
        }
        self.items = new;
        self.ask_card = card;
    }

    fn chat_head(&self, pane: &Pane, cx: &mut Context<Self>) -> Div {
        let needs = matches!(pane.state, SessionState::NeedsYou | SessionState::Error);
        let ibtn = |id: &'static str, icon: &'static str, size: f32, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .size(ui::sw(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgb(tk().track))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk().track_2)))
                .tooltip(ui::tip(tip))
                .child(ui::icon(icon, size, rgb(tk().ink)))
        };
        let branch = self
            .war(cx)
            .and_then(|w| w.codebase(&pane.codebase))
            .map(|c| c.branch.clone())
            .unwrap_or_default();
        let mut meta = format!(
            "{} / {} · {} {}",
            pane.codebase, pane.part, pane.provider, pane.model
        );
        if !branch.is_empty() {
            meta.push_str(&format!(" · {branch}"));
        }
        meta.push_str(&format!(" · ctx {}% · ${:.2}", pane.context_pct, pane.cost));
        let session = SessionRef {
            id: pane.id.clone(),
        };
        div()
            .flex_none()
            .px(px(18.))
            .pt(px(14.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .h(ui::sw(24.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded_full()
                            .bg(rgb(if needs { tk().q_bg } else { tk().track }))
                            .text_color(rgb(if needs { tk().q_text } else { tk().ink }))
                            .text_size(ui::ts(T_CAPTION))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ui::state_dot(pane.state, 8.))
                            .child(state_label(pane.state)),
                    )
                    .child(div().flex_1())
                    .when(pane.controlled, |d| {
                        let (pause, stop) = (session.clone(), session.clone());
                        d.child(
                            ibtn("focus-pause", "icons/pause.svg", 14., "Pause the session")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model
                                        .read(cx)
                                        .send::<method::SessionPause>(pause.clone(), cx)
                                })),
                        )
                        .child(
                            ibtn("focus-stop", "icons/stop.svg", 12., "Stop the session").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.model
                                        .read(cx)
                                        .send::<method::SessionStop>(stop.clone(), cx)
                                }),
                            ),
                        )
                    }),
            )
            .child(
                div()
                    .font_family(tk().display_heading)
                    .text_size(ui::ts(T_HEADING))
                    .line_height(ui::ts(26.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(pane.title.clone()),
            )
            .child(
                div()
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(meta),
            )
    }

    /// The plan of the last todo list of the transcript.
    fn plan(&self, pane: &Pane) -> Option<Div> {
        let todos = items::plan(&pane.tail);
        if todos.is_empty() {
            return None;
        }
        let done = todos.iter().filter(|t| t.state == TodoState::Done).count();
        let pct = done as f32 / todos.len() as f32;
        let mut steps = div().mt(px(8.)).flex().flex_wrap().gap_y(px(2.));
        for t in &todos {
            let (color, weight) = match t.state {
                TodoState::Done => (tk().ink_2, FontWeight::NORMAL),
                TodoState::Active => (tk().q, FontWeight::SEMIBOLD),
                TodoState::Pending => (tk().ink_3, FontWeight::NORMAL),
            };
            let bx = div()
                .flex_none()
                .size(px(12.))
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(8.))
                .text_color(rgb(0xFFFFFF))
                .map(|d| match t.state {
                    TodoState::Done => d.bg(rgb(tk().ink)).child("✓"),
                    TodoState::Active => d.border_1().border_color(rgb(tk().q)),
                    TodoState::Pending => d
                        .border_1()
                        .border_color(rgb(crate::theme::pick(0xC7C7CC, tk().quiet))),
                });
            steps = steps.child(
                div()
                    .w(relative(0.5))
                    .pr(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(ui::ts(T_CAPTION))
                    .line_height(ui::ts(18.))
                    .text_color(rgb(color))
                    .font_weight(weight)
                    .child(bx)
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(t.text.clone()),
                    ),
            );
        }
        Some(
            div()
                .flex_none()
                .mx(px(18.))
                .mt(px(2.))
                .mb(px(6.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(12.))
                .bg(rgb(soft_panel()))
                .border_1()
                .border_color(rgba(tk().line))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(ui::ts(T_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(tk().ink_3))
                        .child("PLAN")
                        .child(
                            div()
                                .font_family(tk().mono)
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{done}/{}", todos.len())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .h(px(4.))
                                .rounded_full()
                                .bg(rgb(tk().track_2))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(pct))
                                        .rounded_full()
                                        .bg(rgb(tk().ink)),
                                ),
                        ),
                )
                .child(steps),
        )
    }

    /// One list item: an item of the transcript, or the ask card behind the last one.
    fn chat_item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let body = match self.items.get(ix).cloned() {
            Some(item) => self.item_el(ix, &item, cx),
            None => match self.ask_card.clone() {
                Some(card) => self.ask_el(&card, cx),
                None => return div().into_any_element(),
            },
        };
        let last = ix + 1 >= self.items.len() + usize::from(self.ask_card.is_some());
        div()
            .w_full()
            .pt(if ix == 0 { px(8.) } else { px(0.) })
            .pb(if last { px(14.) } else { px(12.) })
            .child(body)
            .into_any_element()
    }

    fn item_el(&mut self, ix: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let text_size = ui::ts(T_BODY);
        match item {
            Item::User(text) => div()
                .w_full()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.9))
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(16.))
                        .bg(rgb(tk().track))
                        .text_size(text_size)
                        .line_height(ui::ts(21.))
                        .child(text.clone()),
                )
                .into_any_element(),
            Item::Agent(text) => {
                let lines: Vec<aivory_protocol::Line> = text
                    .lines()
                    .map(|l| aivory_protocol::Line::new(aivory_protocol::LineKind::Agent, l))
                    .collect();
                let style = crate::console::draw::Style {
                    pad_x: 0.,
                    pad_top: 0.,
                    pad_bottom: 0.,
                    ..crate::console::draw::Style::plain(T_BODY, crate::theme::PaneTheme::WHITE)
                };
                let hooks = crate::console::draw::Hooks::default();
                let id: SharedString = format!("focus-agent-{ix}").into();
                let frame = crate::console::draw::Frame {
                    style: &style,
                    hooks: &hooks,
                    id: &id,
                    tick: 0,
                };
                crate::console::draw::stack(&lines, &frame).into_any_element()
            }
            Item::Edit(e) => self.edit_row(ix, e, cx),
            Item::Skim {
                reads,
                searches,
                turn,
            } => {
                let mut parts = Vec::new();
                if *reads > 0 {
                    parts.push(format!(
                        "Read {reads} file{}",
                        if *reads == 1 { "" } else { "s" }
                    ));
                }
                if *searches > 0 {
                    parts.push(format!(
                        "searched {searches} pattern{}",
                        if *searches == 1 { "" } else { "s" }
                    ));
                }
                tool_row(("skim", ix))
                    .child(parts.join(" · "))
                    .child(right(format!("turn {turn}")))
                    .into_any_element()
            }
            Item::Run {
                label,
                text,
                result,
                state,
                turn,
            } => {
                let verb = match label.as_str() {
                    "Bash" => "Ran".to_string(),
                    other => other.to_string(),
                };
                let tail = match state {
                    ToolState::Running => "running".to_string(),
                    ToolState::Waiting => "waits for you".to_string(),
                    ToolState::Failed => "failed".to_string(),
                    ToolState::Done => result.clone(),
                };
                tool_row(("run", ix))
                    .child(turn_chip(*turn))
                    .child(verb)
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(tk().mono)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(tk().ink))
                            .child(text.clone()),
                    )
                    .child(right(tail).text_color(rgb(match state {
                        ToolState::Failed => tk().e_text,
                        ToolState::Waiting => tk().q_text,
                        _ => tk().ink_3,
                    })))
                    .into_any_element()
            }
            Item::Sub { name, text } => div()
                .flex()
                .gap(px(6.))
                .text_size(ui::ts(T_CAPTION))
                .text_color(rgb(tk().ink_3))
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(crate::theme::pick(0x6D28D9, tk().link)))
                        .child(format!("↳ {name}")),
                )
                .child(div().min_w(px(0.)).child(text.clone()))
                .into_any_element(),
            Item::Error(text) => div()
                .text_size(ui::ts(T_SMALL))
                .text_color(rgb(tk().e_text))
                .child(text.clone())
                .into_any_element(),
            Item::Meta(_) => div().into_any_element(),
            Item::Media(m) => div()
                .w_full()
                .flex()
                .justify_end()
                .child(crate::media::preview(
                    m,
                    ("focus-media", ix),
                    crate::media::page_style(),
                ))
                .into_any_element(),
        }
    }

    /// An edit row: a click opens the file in the editor at the hunk of this turn.
    fn edit_row(&mut self, ix: usize, e: &EditRow, cx: &mut Context<Self>) -> AnyElement {
        let target = e.clone();
        let mut right_text = e.range.clone();
        if e.live {
            right_text = if right_text.is_empty() {
                "live".into()
            } else {
                format!("{right_text} · live")
            };
        }
        tool_row(("edit", ix))
            .tooltip(ui::tip(format!("Show turn {} in the editor", e.turn)))
            .on_click(cx.listener(move |this, _, _, cx| this.show_edit(&target, cx)))
            .child(turn_chip(e.turn))
            .child(e.verb)
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(tk().mono)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(tk().ink))
                    .child(e.name().to_string()),
            )
            .child(right(right_text).when(e.live, |d| d.text_color(rgb(CLAUDE))))
            .into_any_element()
    }

    /// The ask card: the question and every answer.
    fn ask_el(&mut self, card: &AskCard, cx: &mut Context<Self>) -> AnyElement {
        let ask = &card.ask;
        let color = if card.state == SessionState::Error {
            tk().e_text
        } else {
            tk().q_text
        };
        let mut answers = div().flex().flex_wrap().gap(px(6.));
        let given = self.answered.get(&ask.id).cloned();
        if let Some(given) = &given {
            answers = answers.child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("Answered: {given}")),
            );
        } else if card.controlled {
            for (i, option) in ask.options.iter().enumerate() {
                let (session, ask_id, value) =
                    (card.session.clone(), ask.id.clone(), option.clone());
                answers = answers.child(
                    ui::button_sm(
                        SharedString::from(format!("focus-ask-{i}")),
                        option_label(ask.kind, option, &ask.text),
                        i == 0,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.answer(&session, &ask_id, Some(value.clone()), None, cx)
                    })),
                );
            }
        } else {
            answers = answers.child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child("Answer in its terminal"),
            );
        }
        let mut col = div()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .bg(rgb(tk().card))
            .rounded(px(12.))
            .shadow(ui::card_shadow())
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(color))
                    .child(ask.kind.label()),
            );
        if ask.kind == AskKind::Permission {
            col = col.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(rgb(tk().track))
                    .font_family(tk().mono)
                    .text_size(ui::ts(12.5))
                    .child(ask.text.clone()),
            );
        } else {
            col = col.child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .line_height(ui::ts(20.))
                    .text_color(rgb(tk().ink_2))
                    .child(ask.text.clone()),
            );
        }
        if let Some(reason) = &ask.reason {
            col = col.child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child(reason.clone()),
            );
        }
        for (i, step) in ask.steps.iter().enumerate() {
            col = col.child(
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
        col.child(answers).into_any_element()
    }

    // ---- the composer -------------------------------------------------------------------

    fn composer(&mut self, pane: &Pane, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.controls.sync(pane);
        let focused = self.input.focus_handle(cx).is_focused(window);
        let ibtn = |id: &'static str, icon: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .size(ui::sw(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(rgb(tk().track))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk().track_2)))
                .tooltip(ui::tip(tip))
                .child(ui::icon(icon, 15., rgb(tk().ink)))
        };
        // The chips: the lines of the editor and the attached files.
        let mut media = div()
            .flex_none()
            .min_h(px(40.))
            .px(px(10.))
            .py(px(7.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .bg(rgb(soft_panel()));
        for (i, r) in self.refs.iter().enumerate() {
            media = media.child(chip(
                ("ref", i),
                r.label(),
                true,
                cx.listener(move |this, _, _, cx| {
                    if let Some(r) = this.refs.get(i)
                        && r.live
                    {
                        this.sel = None;
                    }
                    if i < this.refs.len() {
                        this.refs.remove(i);
                    }
                    cx.notify();
                }),
            ));
        }
        for (i, a) in self.tools.attachments.iter().enumerate() {
            media = media.child(chip(
                ("att", i),
                a.name.clone(),
                false,
                cx.listener(move |this, _, _, cx| {
                    this.tools.remove(i);
                    cx.notify();
                }),
            ));
        }
        if self.refs.is_empty() && self.tools.attachments.is_empty() {
            media = media.child(
                div()
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child("Select lines in the editor to reference them here"),
            );
        }
        let status = compose::status(&self.tools, "focus", compose::Palette::page(), |action| {
            Box::new(cx.listener(move |this, _, window, cx| {
                let input = this.input.clone();
                compose::handle_action(
                    this,
                    action,
                    |this: &mut Self| Some(&mut this.tools),
                    input,
                    window,
                    cx,
                )
            }))
        });
        let field = div()
            .flex_1()
            .min_w(px(0.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(10.))
            .bg(rgb(if focused { tk().card } else { tk().track }))
            .when(focused, |d| d.shadow(ui::ring(tk().ink, 2.)))
            .text_size(ui::ts(T_BODY))
            .line_height(ui::ts(21.))
            .cursor_text()
            .child(self.input.clone());
        let cb = div()
            .flex()
            .items_start()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        ibtn(
                            "focus-capture",
                            "icons/capture.svg",
                            "Capture a screen area",
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if !this.tools.capturing {
                                this.tools.capturing = true;
                                compose::capture(
                                    |this: &mut Self| Some(&mut this.tools),
                                    window,
                                    cx,
                                );
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        ibtn("focus-attach", "icons/attach.svg", "Attach files").on_click(
                            cx.listener(|_, _, window, cx| {
                                compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx)
                            }),
                        ),
                    ),
            )
            .child(field)
            .child(
                div()
                    .id("focus-send")
                    .flex_none()
                    .self_end()
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgb(tk().dark))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(tk().dark_hover)))
                    .tooltip(ui::tip("Send (Enter)"))
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                    .child(ui::icon("icons/send.svg", 15., rgb(0xFFFFFF))),
            );
        let mut dr = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .pl(px(50.))
            .pr(px(10.))
            .pb(px(10.))
            .text_size(ui::ts(T_CAPTION))
            .text_color(rgb(tk().ink_3));
        let providers = self.model.read(cx).providers.clone();
        dr = dr
            .child(self.controls.row(
                "focus-ctl",
                pane,
                &providers,
                &crate::compose_controls::ControlStyle::page(),
                |this: &mut Self| &mut this.controls,
                cx,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .font_family(tk().mono)
                    .child(format!("ctx {}%", pane.context_pct)),
            );
        if compose::VOICE_AVAILABLE {
            let listening = self.tools.listening.is_some();
            dr = dr.child(
                div()
                    .id("focus-mic")
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(tk().track)))
                    .when(listening, |d| d.bg(rgb(tk().q_bg)))
                    .tooltip(ui::tip(if listening {
                        "Stop listening"
                    } else {
                        "Dictate"
                    }))
                    .on_click(cx.listener(|this, _, window, cx| this.dictate(window, cx)))
                    .child(ui::icon("icons/mic.svg", 15., rgb(tk().ink_3))),
            );
        }
        div()
            .flex_none()
            .mx(px(14.))
            .mb(px(14.))
            .rounded(px(14.))
            .bg(rgb(tk().card))
            .shadow(ui::card_shadow())
            .child(media)
            .children(status)
            .child(cb)
            .child(dr)
    }

    /// The mic: dictate into the composer; a second click stops.
    fn dictate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stop) = self.tools.start_listening() else {
            self.tools.stop_listening();
            cx.notify();
            return;
        };
        cx.notify();
        compose::dictate(
            |this: &mut Self| Some(&mut this.tools),
            stop,
            self.input.clone(),
            window,
            cx,
        );
    }
}

/// The frame of a tool row (`.tool`).
fn tool_row(id: (&'static str, usize)) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .px(px(10.))
        .py(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(10.))
        .bg(rgb(soft_panel()))
        .border_1()
        .border_color(rgba(tk().line))
        .text_size(ui::ts(T_CAPTION))
        .text_color(rgb(tk().ink_3))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
}

/// The turn number of a tool row (`.turn`).
fn turn_chip(turn: u32) -> Div {
    div()
        .flex_none()
        .px(px(5.))
        .rounded(px(5.))
        .bg(rgb(CLAUDE))
        .text_color(rgb(0xFFFFFF))
        .font_family(tk().mono)
        .text_size(ui::ts(11.))
        .child(turn.to_string())
}

/// The right column of a tool row (`.r`).
fn right(text: String) -> Div {
    div()
        .flex_none()
        .max_w(relative(0.45))
        .ml_auto()
        .pl(px(8.))
        .font_family(tk().mono)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(text)
}

/// A chip of the media row (`.mchip`).
fn chip(
    id: (&'static str, usize),
    text: String,
    reference: bool,
    remove: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> Div {
    div()
        .h(ui::sw(26.))
        .pl(px(10.))
        .pr(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded_full()
        .bg(rgb(tk().card))
        .border_1()
        .border_color(rgba(0x00000014))
        .text_size(ui::ts(T_CAPTION))
        .when(reference, |d| {
            d.font_family(tk().mono)
                .text_color(rgb(crate::theme::pick(0x0550AE, tk().link)))
        })
        .child(text)
        .child(
            div()
                .id(id)
                .size(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_color(rgb(tk().ink_3))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
                .tooltip(ui::tip("Remove"))
                .on_click(remove)
                .child("×"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_permission_option_names_the_command() {
        assert_eq!(
            option_label(AskKind::Permission, "Always allow", "cargo test -p x"),
            "Always allow cargo"
        );
        assert_eq!(
            option_label(AskKind::Question, "Always allow", "x"),
            "Always allow"
        );
    }
}
