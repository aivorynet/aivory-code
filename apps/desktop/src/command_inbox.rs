//! The Command look, Inbox (board `CommandInbox`): two panels. The queue at the left (filter
//! tabs, one row per open decision, the oldest first) and the decision at the right: the kind and
//! the wait, the title, what the agent wants, the answers 1 to 3, the outcome, a free text field
//! with the mic, and Open session / Next. Keys: J K move, 1 2 3 answer, O open, Enter sends
//! the field. Every control calls the daemon through `actions.rs`.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{InboxListParams, method};
use aivory_protocol::{InboxFilter, InboxItem, InboxKind, InboxList};
use gpui::prelude::*;
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Subscription,
    Task as GpuiTask, Window, div, px,
};
use gpui_base::input::{InputEvent, TextareaState};

use crate::actions::{Action, act};
use crate::command_kit::{self as cmd, Btn, c};
use crate::compose;
use crate::model::Model;
use crate::ui;

/// What the Inbox asks the shell to do.
#[derive(Debug, Clone)]
pub enum CommandInboxEvent {
    OpenSession(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    All,
    Answers,
    Faults,
    Reviews,
}

impl Tab {
    fn filter(self) -> Option<InboxFilter> {
        match self {
            Tab::All => None,
            Tab::Answers => Some(InboxFilter::Answers),
            Tab::Faults => Some(InboxFilter::Errors),
            Tab::Reviews => Some(InboxFilter::Reviews),
        }
    }
}

pub struct CommandInbox {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    tab: Tab,
    selected: Option<String>,
    input: Entity<TextareaState>,
    tools: compose::Tools,
    focus: FocusHandle,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: Subscription,
    _events: Subscription,
}

impl EventEmitter<CommandInboxEvent> for CommandInbox {}

impl Focusable for CommandInbox {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The word for the kind of an item: what the agent wants.
fn wants(it: &InboxItem) -> String {
    let who = it
        .agents
        .first()
        .map_or("THE AGENT".to_string(), |a| cmd::up(&a.provider));
    match it.kind {
        InboxKind::Permission => format!("{who} WANTS TO RUN"),
        InboxKind::Plan => format!("{who} PLANS"),
        InboxKind::Question => format!("{who} ASKS"),
        InboxKind::Error => format!("{who} STOPPED ON"),
        InboxKind::Review => format!("{who} ASKS FOR A REVIEW"),
        InboxKind::Conflict => "TWO SESSIONS EDIT THE SAME CODE".into(),
    }
}

/// The second line of a queue row: the kind and the key point.
fn row_line(it: &InboxItem) -> String {
    let kind = cmd::up(it.kind.label());
    let point = match it.kind {
        InboxKind::Plan if !it.steps.is_empty() => format!("{} STEPS", it.steps.len()),
        InboxKind::Review if !it.files.is_empty() => format!("{} FILES", it.files.len()),
        InboxKind::Conflict => it
            .agents
            .iter()
            .map(|a| cmd::up(&a.provider))
            .collect::<Vec<_>>()
            .join(" + "),
        _ => cmd::up(it.preview.lines().next().unwrap_or("")),
    };
    if point.is_empty() {
        kind
    } else {
        format!("{kind} · {point}")
    }
}

impl CommandInbox {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |this, _, cx| this.refresh(cx));
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |t, cx| t.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("OR TELL THE AGENT WHAT TO DO")
                .auto_grow(1, 3)
                .submit_on_enter(true)
        });
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.send(window, cx);
            }
        });
        let mut this = Self {
            model,
            data: None,
            tab: Tab::All,
            selected: None,
            input,
            tools: compose::Tools::default(),
            focus: cx.focus_handle(),
            busy: false,
            _tick: tick,
            _obs: obs,
            _events: events,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let call = self
            .model
            .read(cx)
            .call::<method::InboxListMethod>(InboxListParams::default(), cx);
        cx.spawn(async move |this, cx| {
            let list = call.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Ok(list) = list
                    && this.data.as_deref() != Some(&list)
                {
                    this.data = Some(Rc::new(list));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn items(&self) -> Vec<InboxItem> {
        let Some(d) = &self.data else {
            return Vec::new();
        };
        let mut out: Vec<InboxItem> = match self.tab.filter() {
            None => d.items.clone(),
            Some(f) => d
                .items
                .iter()
                .filter(|i| f.matches(i.kind))
                .cloned()
                .collect(),
        };
        // The oldest first (board: "Open decisions, oldest first").
        out.sort_by_key(|i| i.since);
        out
    }

    fn current(&self, items: &[InboxItem]) -> Option<usize> {
        self.selected
            .as_ref()
            .and_then(|id| items.iter().position(|i| &i.id == id))
            .or(if items.is_empty() { None } else { Some(0) })
    }

    fn count(&self, f: Option<InboxFilter>) -> usize {
        self.data.as_ref().map_or(0, |d| match f {
            None => d.items.len(),
            Some(f) => d.items.iter().filter(|i| f.matches(i.kind)).count(),
        })
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let items = self.items();
        let Some(k) = self.current(&items) else {
            return;
        };
        let n = items.len() as isize;
        let next = (k as isize + by).rem_euclid(n) as usize;
        self.selected = Some(items[next].id.clone());
        cx.notify();
    }

    fn answer(&mut self, k: usize, cx: &mut Context<Self>) {
        let items = self.items();
        let Some(it) = self.current(&items).map(|i| items[i].clone()) else {
            return;
        };
        if it.result.is_some() {
            return;
        }
        let Some(opt) = it.options.get(k).cloned() else {
            return;
        };
        act(
            &self.model,
            Action::InboxAnswer {
                id: it.id.clone(),
                option: Some(opt),
                text: None,
            },
            cx,
        );
        self.after_answer(&items, &it.id, cx);
    }

    /// Select the next item after an answer and read the list again.
    fn after_answer(&mut self, items: &[InboxItem], done: &str, cx: &mut Context<Self>) {
        let k = items.iter().position(|i| i.id == done).unwrap_or(0);
        self.selected = items
            .iter()
            .skip(k + 1)
            .chain(items.iter().take(k))
            .next()
            .map(|i| i.id.clone());
        self.refresh(cx);
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let items = self.items();
        let Some(it) = self.current(&items).map(|i| items[i].clone()) else {
            return;
        };
        act(
            &self.model,
            Action::InboxAnswer {
                id: it.id.clone(),
                option: None,
                text: Some(text),
            },
            cx,
        );
        self.input.update(cx, |s, cx| s.clean(window, cx));
        self.after_answer(&items, &it.id, cx);
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let items = self.items();
        if let Some(k) = self.current(&items) {
            cx.emit(CommandInboxEvent::OpenSession(items[k].session.clone()));
        }
    }

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

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The keys act only when the text field does not have the focus.
        if self.input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let m = &ev.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        match ev.keystroke.key.as_str() {
            "j" | "down" => self.step(1, cx),
            "k" | "up" => self.step(-1, cx),
            "1" => self.answer(0, cx),
            "2" => self.answer(1, cx),
            "3" => self.answer(2, cx),
            "o" => self.open(cx),
            "enter" => self.input.update(cx, |s, cx| s.focus(window, cx)),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for CommandInbox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let items = self.items();
        let cur = self.current(&items);

        // ---- the queue ----
        let mut tabs = div()
            .flex_none()
            .flex()
            .gap(px(20.))
            .px(px(20.))
            .pt(px(18.))
            .border_b_1()
            .border_color(c(cmd::RULE, 1.));
        for (t, name) in [
            (Tab::All, "ALL"),
            (Tab::Answers, "ANSWERS"),
            (Tab::Faults, "FAULTS"),
            (Tab::Reviews, "REVIEWS"),
        ] {
            let on = self.tab == t;
            tabs = tabs.child(
                cmd::entry(format!("{name} {}", self.count(t.filter())))
                    .id(("cmd-tab", t as usize))
                    .pb(px(6.))
                    .border_b_2()
                    .border_color(c(
                        if on { cmd::GREEN } else { cmd::PANEL },
                        if on { 1. } else { 0. },
                    ))
                    .text_color(c(if on { cmd::GREEN } else { cmd::LABEL }, 1.))
                    .cursor_pointer()
                    .hover(|s| s.text_color(c(cmd::WHITE, 1.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = t;
                        this.selected = None;
                        cx.notify();
                    })),
            );
        }
        let mut list = div()
            .id("cmd-queue")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if items.is_empty() {
            list = list.child(
                cmd::entry("Nothing waits for you.")
                    .px(px(22.))
                    .py(px(22.))
                    .text_color(c(cmd::INK_2, 1.)),
            );
        }
        for (k, it) in items.iter().enumerate() {
            let sel = cur == Some(k);
            let id = it.id.clone();
            let fault = matches!(it.kind, InboxKind::Error);
            let line_c = if fault {
                cmd::RED
            } else if sel {
                cmd::INK_2
            } else {
                cmd::LABEL
            };
            let mut row = div()
                .id(("cmd-row", k))
                .flex()
                .gap(px(8.))
                .py(px(22.))
                .pr(px(20.))
                .cursor_pointer()
                .when(k > 0, |d| d.border_t_1().border_color(c(cmd::RULE, 1.)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }));
            row = if sel {
                row.relative().pl(px(22.)).bg(c(cmd::CARD, 1.)).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(4.))
                        .bg(c(cmd::AMBER, 1.)),
                )
            } else {
                row.pl(px(22.)).hover(|s| s.bg(c(cmd::ROW_HOVER, 1.)))
            };
            list = list.child(
                row.child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(
                            cmd::section(cmd::up(&it.title))
                                .text_color(c(if sel { 0xFFFFFF } else { cmd::INK }, 1.))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis(),
                        )
                        .child(
                            cmd::entry(row_line(it))
                                .text_color(c(line_c, 1.))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis(),
                        ),
                )
                .child(cmd::data(cmd::mins(now, it.since), cmd::S_SECTION).flex_none()),
            );
        }
        let queue = cmd::panel(
            div().size_full().flex().flex_col().child(tabs).child(list),
            cmd::PANEL,
        )
        .w(px(400.))
        .flex_none()
        .h_full();

        // ---- the decision ----
        let mut body = div()
            .size_full()
            .flex()
            .flex_col()
            .pt(px(32.))
            .pr(px(36.))
            .pb(px(28.))
            .pl(px(32.));
        let mut edge = cmd::PANEL;
        if let Some(k) = cur {
            let it = &items[k];
            let fault = matches!(it.kind, InboxKind::Error);
            let color = if fault { cmd::RED } else { cmd::AMBER };
            edge = color;
            let who = it
                .agents
                .first()
                .map_or(String::new(), |a| cmd::up(&a.provider));
            let ctx = [cmd::up(&it.codebase), cmd::up(&it.part), who.clone()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            body = body
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            cmd::entry(format!(
                                "{} · {}",
                                cmd::up(it.kind.label()),
                                cmd::mins(now, it.since)
                            ))
                            .text_color(c(color, 1.)),
                        )
                        .child(
                            cmd::data(format!("{} OF {}", k + 1, items.len()), cmd::S_ENTRY)
                                .text_color(c(cmd::LABEL, 1.)),
                        ),
                )
                .child(
                    cmd::title(cmd::up(&it.title))
                        .mt(px(14.))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis(),
                )
                .child(
                    cmd::entry(format!("[{ctx}]"))
                        .mt(px(8.))
                        .text_color(c(cmd::LABEL, 1.)),
                )
                .child(div().flex_1().min_h(px(32.)))
                .child(cmd::entry(wants(it)).text_color(c(cmd::LABEL, 1.)));
            if !it.text.is_empty() {
                body = body.child(
                    div()
                        .id("cmd-ask")
                        .mt(px(8.))
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .px(px(18.))
                        .py(px(16.))
                        .bg(c(cmd::CARD, 1.))
                        .child(
                            cmd::data(it.text.clone(), cmd::S_SECTION)
                                .text_color(c(cmd::WHITE, 1.)),
                        ),
                );
            }
            if !it.preview.is_empty() && it.preview != it.text {
                body = body.child(
                    cmd::entry(it.preview.clone())
                        .mt(px(12.))
                        .text_color(c(cmd::INK_2, 1.)),
                );
            }
            if !it.steps.is_empty() {
                let mut steps = div().mt(px(12.)).flex().flex_col().gap(px(4.));
                for (n, s) in it.steps.iter().take(6).enumerate() {
                    steps = steps
                        .child(cmd::entry(format!("{}. {s}", n + 1)).text_color(c(cmd::INK_2, 1.)));
                }
                body = body.child(steps);
            }
            body = body.child(div().flex_1().min_h(px(32.)));
            let mut answers = div().flex().flex_wrap().gap(px(10.));
            if let Some(result) = &it.result {
                answers = answers.child(cmd::section(format!("DONE · {}", cmd::up(result))));
            } else {
                let n = it.options.len();
                for (j, o) in it.options.iter().take(3).enumerate() {
                    let kind = if j == 0 {
                        Btn::Go
                    } else if j + 1 == n && n > 1 {
                        Btn::Quiet
                    } else {
                        Btn::Plain
                    };
                    answers = answers.child(
                        cmd::button(("cmd-ans", j), format!("{} · {}", j + 1, cmd::up(o)), kind)
                            .on_click(cx.listener(move |this, _, _, cx| this.answer(j, cx))),
                    );
                }
            }
            body = body.child(answers);
            let outcome = match it.kind {
                InboxKind::Permission => {
                    format!("{who} runs it on Allow and goes on with the task.")
                }
                InboxKind::Plan => format!("{who} starts the plan on Approve."),
                InboxKind::Question => format!("{who} goes on with your answer."),
                InboxKind::Error => format!("{who} tries again on Retry."),
                InboxKind::Review => "The change merges on Approve.".to_string(),
                InboxKind::Conflict => "The other session waits until you pick a side.".to_string(),
            };
            body = body.child(
                cmd::entry(outcome.trim().to_string())
                    .mt(px(14.))
                    .text_color(c(cmd::INK_2, 1.)),
            );
        } else {
            body = body
                .child(cmd::title("ALL CLEAR"))
                .child(
                    cmd::entry("No decision waits for you. A new ask shows here.")
                        .mt(px(8.))
                        .text_color(c(cmd::INK_2, 1.)),
                )
                .child(div().flex_1());
        }
        let listening = self.tools.listening.is_some();
        body = body
            .child(
                div()
                    .mt(px(28.))
                    .min_h(px(56.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pl(px(16.))
                    .pr(px(10.))
                    .py(px(9.))
                    .bg(c(cmd::CARD, 1.))
                    .font_family(cmd::SANS)
                    .text_size(ui::ts(cmd::S_ENTRY))
                    .text_color(c(cmd::INK, 1.))
                    .child(div().flex_1().min_w(px(0.)).child(self.input.clone()))
                    .child(cmd::label("ENTER").text_color(c(cmd::HINT, 1.)))
                    .child(
                        cmd::mic("cmd-mic", listening)
                            .on_click(cx.listener(|this, _, window, cx| this.dictate(window, cx))),
                    ),
            )
            .child(
                div()
                    .mt(px(26.))
                    .flex()
                    .gap(px(24.))
                    .child(
                        div()
                            .id("cmd-open")
                            .flex()
                            .gap(px(6.))
                            .cursor_pointer()
                            .child(cmd::entry("OPEN SESSION").text_color(c(cmd::GREEN, 1.)))
                            .child(cmd::entry("O").text_color(c(cmd::HINT, 1.)))
                            .on_click(cx.listener(|this, _, _, cx| this.open(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("cmd-next")
                            .flex()
                            .gap(px(6.))
                            .cursor_pointer()
                            .child(cmd::entry("NEXT").text_color(c(cmd::INK_2, 1.)))
                            .child(cmd::entry("J").text_color(c(cmd::HINT, 1.)))
                            .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
                    ),
            );
        let decision = cmd::panel(
            div()
                .size_full()
                .border_l_4()
                .border_color(c(edge, 1.))
                .child(body),
            cmd::PANEL,
        )
        .flex_1()
        .min_w(px(0.))
        .h_full();

        div()
            .size_full()
            .flex()
            .gap(px(cmd::GAP))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .font_family(cmd::SANS)
            .child(queue)
            .child(decision)
    }
}
