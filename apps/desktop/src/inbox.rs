//! The Inbox screen (`Inbox.dc.html`): every waiting ask in one list, answered without the war
//! room. Left the item cards, right the detail of the selected item with its ask card.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use aivory_protocol::rpc::{
    HandoffParams, InboxAnswerParams, InboxListParams, InboxSnoozeParams, SendParams, method,
};
use aivory_protocol::{
    AgentRef, ComposerPrefs, Effort, GateState, InboxFilter, InboxItem, InboxKind, InboxList, Line,
    MINUTE_MS, Mode, PROVIDERS, UnixMs,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, BoxShadow, Context, Div, Entity, EventEmitter, FontWeight, ListAlignment,
    ScrollHandle, SharedString, Subscription, Window, deferred, div, linear_color_stop,
    linear_gradient, list, point, px, relative, rgb, rgba,
};
use gpui_base::input::{InputEvent, TextareaState};

use crate::compose;
use crate::console::{self, BlockList, Frame, Hooks, Pending, Style};
use crate::model::Model;
use crate::theme::*;
use crate::ui;

#[derive(Debug, Clone)]
pub enum InboxEvent {
    OpenSession(String),
    /// A click on a file path in the transcript: show the file in the Lens.
    OpenFile {
        session: String,
        path: String,
    },
}

/// The filter segment, in design order.
const FILTERS: [(InboxFilter, &str); 5] = [
    (InboxFilter::All, "All"),
    (InboxFilter::Answers, "Answers"),
    (InboxFilter::Errors, "Errors"),
    (InboxFilter::Reviews, "Reviews"),
    (InboxFilter::Conflicts, "Conflicts"),
];

const SNOOZE_MINUTES: u32 = 30;
const LIST_W: f32 = 420.;
/// Under this window width the list is narrower (`@media (max-width:1000px)`).
const MEDIUM: f32 = 1000.;
const LIST_W_MEDIUM: f32 = 320.;

// Inbox surfaces that no other screen shares.
fn field_border() -> u32 {
    tk().field_line
}
fn button_border() -> u32 {
    tk().edge
}
fn button_border_hover() -> u32 {
    tk().muted
}
fn icon_hover() -> u32 {
    crate::theme::pick(0xEBECF0, tk().track_2)
}
const GATE_BORDER: u32 = 0x0D0D0E1A;
fn gate_ok() -> u32 {
    tk().ok_text
}
const GATE_BAD: u32 = 0xB91C1C;
fn conflict_bg() -> u32 {
    crate::theme::pick(0xFAFBFC, tk().card)
}
const AGENT_ACCENT: u32 = 0x7C3AED;
fn link_hover() -> u32 {
    crate::theme::pick(0x3A48B8, tk().link)
}

pub struct InboxScreen {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    filter: InboxFilter,
    /// The pick of the user. [`resolve_selection`] decides what shows.
    selected: Option<String>,
    /// The transcript of the selected item, bottom anchored.
    tail: BlockList,
    tail_item: Option<String>,
    /// The session of the selected item, for a click on a file path.
    tail_session: Option<String>,
    input: Option<Entity<TextareaState>>,
    input_events: Option<Subscription>,
    /// The item the text in the composer is for.
    input_for: Option<String>,
    /// Items with an `inbox/answer` call in flight.
    answering: HashSet<String>,
    /// The item whose last `inbox/answer` failed, and the text of the error (a conflict that
    /// closed meanwhile answers with a stale-ask error).
    answer_error: Option<(String, SharedString)>,
    /// The composer tools of the selected item: chips, the dictation, the last result.
    tools: compose::Tools,
    /// The chip menu that is open (mode, agent, effort).
    menu: Option<Chip>,
    /// The mode, agent and effort that the user picked, per session.
    picks: HashMap<String, Picks>,
    /// A list call is in flight.
    refreshing: bool,
    /// A delta came while a list call was in flight.
    dirty: bool,
    /// Bumped by every local change: an older list answer is stale.
    generation: u64,
    /// The `(epoch, cursor)` of the war room that the last refresh answered.
    seen: Option<(u64, u64)>,
    /// The scroll position of the item list (the Navigator look draws a bar for it).
    scroll: ScrollHandle,
    _observe: Subscription,
}

impl EventEmitter<InboxEvent> for InboxScreen {}

impl InboxScreen {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |this, model, cx| {
            let key = model.read(cx).war.as_ref().map(|w| (w.epoch, w.cursor));
            if key.is_some() && key != this.seen {
                this.seen = key;
                this.refresh(cx);
            }
        });
        let mut this = Self {
            model,
            data: None,
            filter: InboxFilter::All,
            selected: None,
            tail: BlockList::new(ListAlignment::Bottom),
            tail_item: None,
            tail_session: None,
            input: None,
            input_events: None,
            input_for: None,
            answering: HashSet::new(),
            answer_error: None,
            tools: compose::Tools::default(),
            menu: None,
            picks: HashMap::new(),
            refreshing: false,
            dirty: false,
            generation: 0,
            seen: None,
            scroll: ScrollHandle::new(),
            _observe: observe,
        };
        this.refresh(cx);
        this
    }

    /// Load the list again. One call at a time: deltas that come meanwhile ask for one more.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.dirty = true;
            return;
        }
        self.refreshing = true;
        let generation = self.generation;
        let call = self
            .model
            .read(cx)
            .call::<method::InboxListMethod>(InboxListParams::default(), cx);
        cx.spawn(async move |this, cx| {
            let list = call.await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                let stale = generation != this.generation;
                match list {
                    Ok(list) if !stale => this.data = Some(Rc::new(list)),
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("aivory: inbox/list failed: {e:#}");
                        this.model.update(cx, |m, cx| {
                            m.notify_user(crate::notice::failure("inbox/list", &e), cx)
                        });
                    }
                }
                if stale || std::mem::take(&mut this.dirty) {
                    this.refresh(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn now(&self, cx: &gpui::App) -> UnixMs {
        self.model
            .read(cx)
            .war
            .as_ref()
            .map_or_else(|| chrono::Utc::now().timestamp_millis(), |w| w.now)
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&id) && !self.tools.busy() {
            self.tools = compose::Tools::default();
        }
        self.menu = None;
        self.selected = Some(id);
        cx.notify();
    }

    /// Select the first open item that asks in one of these sessions, and show all items.
    pub fn show_sessions(&mut self, sessions: &[String], cx: &mut Context<Self>) {
        self.refresh(cx);
        let found = self.data.as_ref().and_then(|d| {
            d.items
                .iter()
                .find(|i| sessions.contains(&i.session))
                .map(|i| i.id.clone())
        });
        if let Some(id) = found {
            self.filter = InboxFilter::All;
            self.select(id, cx);
        }
    }

    fn set_filter(&mut self, filter: InboxFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    /// Answer an item with an option, a typed text, or both.
    fn answer(
        &mut self,
        id: String,
        option: Option<String>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.answering.insert(id.clone()) {
            return;
        }
        self.answer_error = None;
        let call = self.model.read(cx).call::<method::InboxAnswer>(
            InboxAnswerParams {
                id: id.clone(),
                option,
                text,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| this.answered(&id, result, cx));
        })
        .detach();
    }

    /// The daemon took the answer: the item moves to Done and the next item is selected.
    fn answered(&mut self, id: &str, result: anyhow::Result<InboxItem>, cx: &mut Context<Self>) {
        self.answering.remove(id);
        match result {
            Ok(done) => {
                let filter = self.filter;
                let was_selected = self.selected.as_deref() == Some(id);
                if let Some(data) = self.data.as_mut() {
                    let data = Rc::make_mut(data);
                    let shown: Vec<&str> = data
                        .items
                        .iter()
                        .filter(|i| filter.matches(i.kind))
                        .map(|i| i.id.as_str())
                        .collect();
                    let next = next_after(&shown, id);
                    data.items.retain(|i| i.id != id);
                    data.done.retain(|i| i.id != id);
                    data.done.insert(0, done);
                    if was_selected {
                        self.selected = next;
                    }
                }
            }
            Err(e) => {
                eprintln!("aivory: inbox/answer failed: {e:#}");
                self.answer_error = Some((id.to_string(), answer_error_text(&e).into()));
            }
        }
        self.generation += 1;
        self.refresh(cx);
        cx.notify();
    }

    /// The mode, agent and effort that the session has now (the composer of its pane).
    fn base_prefs(&self, item: &InboxItem, cx: &gpui::App) -> ComposerPrefs {
        let pane = self
            .model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| w.session(&item.session))
            .map(|p| p.composer.clone());
        pane.unwrap_or_else(|| {
            let agent = item.agents.first();
            ComposerPrefs {
                mode: Mode::Edit,
                provider: agent.map_or_else(|| "claude".to_string(), |a| a.provider.clone()),
                model: agent.map(|a| a.model.clone()).unwrap_or_default(),
                effort: Effort::High,
            }
        })
    }

    /// Send the composer text and the chips as the answer of the selected item. A new agent
    /// hands the session off, and the text goes to the new session.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(input), Some(id)) = (self.input.clone(), self.selected.clone()) else {
            return;
        };
        let typed = input.read(cx).value().trim().to_string();
        let Some(item) = self
            .data
            .as_ref()
            .and_then(|d| d.items.iter().find(|i| i.id == id))
            .cloned()
        else {
            return;
        };
        if typed.is_empty() && self.tools.attachments.is_empty() {
            return;
        }
        let text = self
            .tools
            .message(&typed, self.cwd_of(&item, cx).as_deref());
        let base = self.base_prefs(&item, cx);
        let picks = self.picks.get(&item.session).cloned().unwrap_or_default();
        match plan(&base, &picks) {
            Plan::Answer { prefs_changed } => {
                self.answer(id, None, Some(text), cx);
                if prefs_changed {
                    self.model.update(cx, |m, cx| {
                        m.notify_user(
                            crate::notice::Draft::info(
                                "The mode and the effort apply to the next message of the session.",
                            ),
                            cx,
                        )
                    });
                }
            }
            Plan::Handoff(prefs) => self.handoff(&item.session, prefs, text, cx),
        }
        self.tools.sent();
        input.update(cx, |s, cx| s.clean(window, cx));
        cx.notify();
    }

    /// Hand the session off to the picked agent, send the text there, and open the new session.
    fn handoff(
        &mut self,
        session: &str,
        prefs: ComposerPrefs,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let call = self.model.read(cx).call_notify::<method::SessionHandoff>(
            HandoffParams {
                id: session.to_string(),
                provider: prefs.provider.clone(),
                model: Some(prefs.model.clone()),
            },
            cx,
        );
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            if let Some(created) = call.await {
                let new_id = created.id.clone();
                cx.update(|cx| {
                    model.read(cx).send::<method::SessionSend>(
                        SendParams {
                            id: created.id,
                            text,
                            subagent: None,
                            attachments: Vec::new(),
                            composer: Some(prefs),
                        },
                        cx,
                    )
                });
                let _ = this.update(cx, |this, cx| {
                    this.generation += 1;
                    this.refresh(cx);
                    cx.emit(InboxEvent::OpenSession(new_id));
                });
            }
        })
        .detach();
    }

    /// The folder of the codebase of an item, for the `@path` references.
    fn cwd_of(&self, item: &InboxItem, cx: &gpui::App) -> Option<std::path::PathBuf> {
        let war = self.model.read(cx).war.as_ref()?;
        war.codebase(&item.codebase)
            .map(|c| std::path::PathBuf::from(&c.path))
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx);
    }

    fn capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools.capturing {
            return;
        }
        self.tools.capturing = true;
        cx.notify();
        compose::capture(|this: &mut Self| Some(&mut this.tools), window, cx);
    }

    fn dictate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.input.clone() else {
            return;
        };
        let Some(stop) = self.tools.start_listening() else {
            self.tools.stop_listening();
            cx.notify();
            return;
        };
        cx.notify();
        compose::dictate(
            |this: &mut Self| Some(&mut this.tools),
            stop,
            input,
            window,
            cx,
        );
    }

    /// A button of the tool line (stop, the offline model).
    fn tool_action(
        &mut self,
        action: compose::ToolAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.input.clone() else {
            return;
        };
        compose::handle_action(
            self,
            action,
            |this: &mut Self| Some(&mut this.tools),
            input,
            window,
            cx,
        );
    }

    /// Open or close a chip menu.
    fn toggle_menu(&mut self, chip: Chip, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(chip) {
            None
        } else {
            Some(chip)
        };
        cx.notify();
    }

    /// Take a value of a chip menu.
    fn pick(&mut self, item: &InboxItem, chip: Chip, value: &str, cx: &mut Context<Self>) {
        let base = self.base_prefs(item, cx);
        let picks = self.picks.entry(item.session.clone()).or_default();
        match chip {
            Chip::Mode => {
                let mode = mode_from(value);
                picks.mode = (mode != base.mode).then_some(mode);
            }
            Chip::Effort => {
                let effort = effort_from(value);
                picks.effort = (effort != base.effort).then_some(effort);
            }
            Chip::Agent => {
                let (provider, model) = value.split_once(" · ").unwrap_or((value, ""));
                let same = provider == base.provider && model == base.model;
                picks.agent = (!same).then(|| (provider.to_string(), model.to_string()));
            }
        }
        self.menu = None;
        cx.notify();
    }

    fn snooze_all(&mut self, cx: &mut Context<Self>) {
        let call = self.model.read(cx).call::<method::InboxSnooze>(
            InboxSnoozeParams {
                id: None,
                minutes: SNOOZE_MINUTES,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    eprintln!("aivory: inbox/snooze failed: {e:#}");
                    this.model.update(cx, |m, cx| {
                        m.notify_user(crate::notice::failure("inbox/snooze", &e), cx)
                    });
                }
                this.generation += 1;
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// The text field of the composer, made on first use. Enter sends.
    fn ensure_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        if let Some(input) = &self.input {
            return input.clone();
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Or type your own answer…")
                .submit_on_enter(true)
                .auto_grow(1, 4)
        });
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.submit(window, cx);
            }
        });
        self.input = Some(input.clone());
        self.input_events = Some(events);
        input
    }

    /// Show the transcript of an item. A new item gets a new list; new lines of the same item
    /// change only the blocks that differ.
    fn sync_tail(&mut self, id: Option<&str>, session: Option<&str>, lines: Vec<Line>) {
        if self.tail_item.as_deref() != id {
            self.tail = BlockList::new(ListAlignment::Bottom);
            self.tail_item = id.map(str::to_string);
        }
        self.tail_session = session.map(str::to_string);
        self.tail.set(lines, 0, Pending::No);
    }

    /// One item of the transcript list.
    fn tail_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let style = Style::plain(12.5, self.model.read(cx).console_theme());
        let weak = cx.entity().downgrade();
        let toggle = {
            let weak = weak.clone();
            Rc::new(move |abs: u32, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    this.tail.toggle(abs);
                    cx.notify();
                });
            })
        };
        let open_file = {
            let weak = weak.clone();
            Rc::new(move |path: String, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    if let Some(session) = this.tail_session.clone() {
                        cx.emit(InboxEvent::OpenFile { session, path });
                    }
                });
            })
        };
        let hooks = Hooks {
            toggle: Some(toggle),
            open_file: Some(open_file),
            load_older: None,
        };
        let id: SharedString = format!("inbox-{}", self.tail_item.as_deref().unwrap_or("")).into();
        let frame = Frame {
            style: &style,
            hooks: &hooks,
            id: &id,
            tick: 0,
        };
        self.tail.item(ix, &frame)
    }

    fn header(&self, items: &[InboxItem], now: UnixMs, cx: &mut Context<Self>) -> Div {
        let counts = Counts::of(items);
        let mut seg = div()
            .flex()
            .flex_none()
            .gap(px(2.))
            .p(px(2.))
            .bg(rgb(tk().track))
            .rounded(tk().r_button);
        if ui::irix() {
            seg = ui::bevel_edges(false, seg.rounded(px(0.)).bg(rgb(0xBDBDBD)).p(px(4.)));
        }
        for (filter, label) in FILTERS {
            let on = self.filter == filter;
            seg = seg.child(
                div()
                    .id(SharedString::from(format!("flt-{label}")))
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(rgb(tk().card))
                            .font_weight(FontWeight::MEDIUM)
                            .shadow(vec![BoxShadow {
                                color: ui::hsla(tk().line),
                                offset: point(px(0.), px(1.)),
                                blur_radius: px(2.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                    })
                    .when(!on, |d| d.hover(|s| s.text_color(rgb(tk().ink))))
                    .when(ui::irix(), |d| {
                        d.rounded(px(0.))
                            .shadow(Vec::new())
                            .text_color(rgb(0x111111))
                            .bg(rgb(if on { 0xA6A6A6 } else { 0xC8C8C8 }))
                    })
                    .when(ui::irix(), |d| ui::bevel_edges(!on, d))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
                    .child(label)
                    .child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(counts.get(filter).to_string()),
                    ),
            );
        }
        div()
            .flex_none()
            .min_h(HEADER_H)
            .py(px(12.))
            .px(px(32.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .font_family(crate::theme::tk().display)
                    .text_size(crate::ui::ts(crate::theme::T_DISPLAY))
                    .font_weight(FontWeight::NORMAL)
                    .child("Inbox"),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child(header_text(items, now)),
            )
            .child(div().flex_1())
            .child(seg)
            .child(
                div()
                    .id("snooze-all")
                    .h(px(32.))
                    .px(px(12.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .rounded(tk().r_button)
                    .border_1()
                    .border_color(rgba(tk().line_2))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| s.border_color(rgb(tk().ink)))
                    .when(ui::irix(), |d| {
                        d.rounded(px(0.))
                            .border_0()
                            .bg(rgb(0xC8C8C8))
                            .text_color(rgb(0x111111))
                    })
                    .when(ui::irix(), |d| ui::bevel_edges(true, d))
                    .on_click(cx.listener(|this, _, _, cx| this.snooze_all(cx)))
                    .child(format!("Snooze all for {SNOOZE_MINUTES} min")),
            )
    }

    /// One card of the list; a done card is dimmed and shows the result.
    fn item_card(
        &self,
        item: &InboxItem,
        on: bool,
        now: UnixMs,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let done = item.result.is_some();
        let style = kind_style(item.kind);
        let id = item.id.clone();
        let (pill, age) = match &item.result {
            Some(result) => (
                kind_pill(result, tk().track, tk().ink_3),
                SharedString::from("just now"),
            ),
            None => (
                kind_pill(item.kind.label(), style.pill_bg, style.pill_text),
                SharedString::from(wait_text(wait_minutes(item.since, now))),
            ),
        };
        div()
            .id(SharedString::from(format!("it-{}", item.id)))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(14.))
            .py(px(12.))
            .bg(rgb(tk().card))
            .border_1()
            .border_color(if on {
                rgb(tk().primary)
            } else {
                rgba(tk().line)
            })
            .rounded(px(10.))
            .cursor_pointer()
            .when(ui::irix(), |d| {
                d.rounded(px(0.))
                    .border_0()
                    .bg(rgb(if on { 0xDDE4F0 } else { 0xF4F4F4 }))
                    .text_color(rgb(0x111111))
            })
            .when(ui::irix(), |d| ui::bevel_edges(!on, d))
            .when(!on, |d| d.hover(|s| s.border_color(rgba(0x0D0D0E33))))
            .when(on, |d| {
                d.shadow(vec![BoxShadow {
                    color: ui::hsla(0x3574F01F),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }])
            })
            .when(done, |d| d.opacity(0.6))
            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w(px(0.))
                    .child(pill)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .child(age),
                    ),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(format!(
                        "{} · {}",
                        where_text(item),
                        agents_text(&item.agents)
                    )),
            )
            .when(!done, |d| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_BODY))
                        .line_height(relative(1.5))
                        .text_color(rgb(tk().ink_2))
                        .line_clamp(2)
                        .child(item.preview.clone()),
                )
            })
            .into_any_element()
    }

    fn item_list(
        &self,
        shown: &[&InboxItem],
        done: &[InboxItem],
        selected: Option<&str>,
        now: UnixMs,
        list_w: Option<f32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let navigator = tk().look == aivory_protocol::Look::Navigator;
        let size = |d: Div| match list_w {
            // Without a detail the list fills the screen.
            None => d.flex_1().min_w(px(0.)),
            Some(w) => d
                .flex_none()
                .w(px(w))
                .border_r_1()
                .border_color(rgba(tk().line)),
        };
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pl(px(32.))
            .pr(px(16.))
            .pt(px(16.))
            .pb(px(24.));
        for item in shown {
            col = col.child(self.item_card(item, selected == Some(item.id.as_str()), now, cx));
        }
        if !done.is_empty() {
            col = col.child(
                div()
                    .flex_none()
                    .px(px(4.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(tk().ink_3))
                    .child("DONE IN THIS SESSION"),
            );
        }
        for item in done {
            col = col.child(self.item_card(item, selected == Some(item.id.as_str()), now, cx));
        }
        if navigator {
            // The Navigator look shows a visible scroll bar.
            return size(div().h_full())
                .child(crate::warroom::skins::navigator::panels::scroll_area(
                    "inbox-items",
                    &self.scroll,
                    false,
                    col,
                ))
                .into_any_element();
        }
        size(col)
            .id("inbox-items")
            .h_full()
            .overflow_y_scroll()
            .into_any_element()
    }

    fn nav_button(
        &self,
        label: &'static str,
        icon: &'static str,
        icon_first: bool,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let glyph = || ui::icon(icon, 12., rgb(tk().ink));
        div()
            .id(SharedString::from(format!("nav-{label}")))
            .h(px(30.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(tk().r_button)
            .border_1()
            .border_color(rgba(tk().line_2))
            .bg(rgb(tk().card))
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .when(ui::irix(), |d| {
                d.rounded(px(0.))
                    .border_0()
                    .bg(rgb(0xC8C8C8))
                    .text_color(rgb(0x111111))
            })
            .when(ui::irix(), |d| ui::bevel_edges(true, d))
            .when(target.is_none(), |d| d.opacity(0.4))
            .when_some(target, |d, id| {
                d.cursor_pointer()
                    .hover(|s| s.border_color(rgb(tk().ink)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
            })
            .when(icon_first, |d| d.child(glyph()))
            .child(label)
            .when(!icon_first, |d| d.child(glyph()))
            .into_any_element()
    }

    /// The transcript tail: the last line sits at the bottom, older lines fade out at the top.
    fn transcript(&self, cx: &mut Context<Self>) -> Div {
        let bg = self.model.read(cx).console_theme().bg;
        div()
            .flex_1()
            .min_h(px(0.))
            .relative()
            .bg(rgba(bg))
            .child(
                list(
                    self.tail.list.clone(),
                    cx.processor(|this, ix: usize, _, cx| this.tail_row(ix, cx)),
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(12.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(rgba(bg), 0.),
                        linear_color_stop(rgba(bg & 0xFFFF_FF00), 1.),
                    )),
            )
    }

    /// Both sides of a conflict, side by side.
    fn conflict_sides(item: &InboxItem) -> Div {
        let mut row = div().flex_none().flex().gap(px(8.)).px(px(12.)).pb(px(10.));
        for side in &item.sides {
            let mut hunk = div()
                .flex_1()
                .min_w(px(0.))
                .px(px(10.))
                .py(px(8.))
                .border_1()
                .border_color(rgba(tk().line))
                .bg(rgb(conflict_bg()))
                .child(
                    div()
                        .mb(px(4.))
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .font_weight(FontWeight::BOLD)
                        .child(side.title.clone()),
                );
            hunk = hunk.child(side_lines(&side.lines));
            row = row.child(hunk);
        }
        row
    }

    fn ask_card(item: &InboxItem, error: Option<&SharedString>, cx: &mut Context<Self>) -> Div {
        let style = kind_style(item.kind);
        let done = item.result.is_some();
        let heading = match &item.result {
            Some(result) => format!("Done · {result}"),
            None => item.kind.heading().to_string(),
        };
        let mut card = div()
            .flex_none()
            .mx(px(12.))
            .mb(px(10.))
            .rounded(tk().r_button)
            .border_1()
            .border_color(rgb(style.ask_border))
            .bg(rgb(style.ask_bg))
            .overflow_hidden()
            .child(
                div()
                    .py(px(8.))
                    .px(px(12.))
                    .bg(rgb(style.ask_head_bg))
                    .text_color(rgb(style.ask_head))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(heading),
            )
            .child(
                div()
                    .pt(px(10.))
                    .px(px(12.))
                    .pb(px(4.))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .line_height(relative(1.5))
                    .when(item.kind == InboxKind::Permission, |d| {
                        d.font_family(tk().mono)
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                    })
                    .child(item.text.clone()),
            );
        if !item.steps.is_empty() && !done {
            let mut steps = div().py(px(2.)).pl(px(32.)).pr(px(12.)).flex().flex_col();
            for (i, step) in item.steps.iter().enumerate() {
                steps = steps.child(
                    div()
                        .relative()
                        .text_size(crate::ui::ts(crate::theme::T_BODY))
                        .line_height(relative(1.6))
                        .child(div().absolute().left(px(-16.)).child(format!("{}.", i + 1)))
                        .child(step.clone()),
                );
            }
            card = card.child(steps);
        }
        if !item.gates.is_empty() && !done {
            let mut gates = div().flex().flex_wrap().gap(px(8.)).pt(px(6.)).px(px(12.));
            for gate in &item.gates {
                let (mark, color) = match gate.state {
                    GateState::Ok => ("✓", Some(gate_ok())),
                    GateState::Pending => ("○", None),
                    GateState::Failed => ("✕", Some(GATE_BAD)),
                };
                gates = gates.child(
                    div()
                        .h(px(24.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded_full()
                        .bg(rgb(tk().card))
                        .border_1()
                        .border_color(rgba(GATE_BORDER))
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .when_some(color, |d, c| d.text_color(rgb(c)))
                        .child(format!("{mark} {}", gate.label)),
                );
            }
            card = card.child(gates);
        }
        if !done {
            let mut options = div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .pt(px(8.))
                .px(px(12.))
                .pb(px(12.));
            for (i, option) in item.options.iter().enumerate() {
                let (id, answer) = (item.id.clone(), option.clone());
                let primary = i == 0;
                options = options.child(
                    div()
                        .id(SharedString::from(format!("opt-{}-{i}", item.id)))
                        .h(px(30.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .text_size(crate::ui::ts(crate::theme::T_BODY))
                        .cursor_pointer()
                        .when(primary, |d| {
                            d.bg(rgb(tk().primary))
                                .border_color(rgb(tk().primary))
                                .text_color(rgb(tk().card))
                                .font_weight(FontWeight::MEDIUM)
                                .hover(|s| s.bg(rgb(tk().primary_hover)))
                        })
                        .when(!primary, |d| {
                            d.bg(rgb(tk().card))
                                .border_color(rgb(button_border()))
                                .hover(|s| s.border_color(rgb(button_border_hover())))
                        })
                        .when(ui::irix(), |d| {
                            d.rounded(px(0.))
                                .border_color(rgb(if primary { 0x111111 } else { 0xC8C8C8 }))
                                .bg(rgb(0xC8C8C8))
                                .text_color(rgb(0x111111))
                        })
                        .when(ui::irix(), |d| ui::bevel_edges(true, d))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.answer(id.clone(), Some(answer.clone()), None, cx)
                        }))
                        .child(option.clone()),
                );
            }
            card = card.child(options);
            if let Some(text) = error {
                card = card.child(
                    div()
                        .px(px(12.))
                        .pb(px(10.))
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().e_text))
                        .child(text.clone()),
                );
            }
        }
        card
    }

    /// The line over the field and the field with its tools. A done item has no field.
    fn composer(
        &self,
        item: &InboxItem,
        input: Entity<TextareaState>,
        now: UnixMs,
        cx: &mut Context<Self>,
    ) -> Div {
        let done = item.result.is_some();
        let icon_button = |name: &'static str, icon: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(format!("gi-{name}")))
                .tooltip(ui::tip(tip))
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(icon_hover())))
                .child(ui::icon(icon, 15., rgb(tk().ink_3)))
        };
        let prefs = self
            .model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| w.session(&item.session))
            .map(|p| p.composer.clone());
        let mode = prefs.as_ref().map_or("Edit", |p| match p.mode {
            aivory_protocol::Mode::Plan => "Plan",
            aivory_protocol::Mode::Edit => "Edit",
            aivory_protocol::Mode::Auto => "Auto",
        });
        let effort = prefs.as_ref().map_or("high", |p| match p.effort {
            aivory_protocol::Effort::Low => "low",
            aivory_protocol::Effort::Medium => "medium",
            aivory_protocol::Effort::High => "high",
            aivory_protocol::Effort::Max => "max",
        });
        let agent = item
            .agents
            .first()
            .map(|a| format!("{} · {}", a.provider, a.model))
            .unwrap_or_default();
        let _ = (mode, agent, effort);
        let base = self.base_prefs(item, cx);
        let picks = self.picks.get(&item.session).cloned().unwrap_or_default();
        let eff = picks.apply(&base);
        let mut chips = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .ml(px(-4.))
            .min_w(px(0.));
        for (chip, label, changed) in [
            (
                Chip::Mode,
                mode_label(eff.mode).to_string(),
                picks.mode.is_some(),
            ),
            (
                Chip::Agent,
                format!("{} · {}", eff.provider, eff.model),
                picks.agent.is_some(),
            ),
            (
                Chip::Effort,
                effort_label(eff.effort).to_string(),
                picks.effort.is_some(),
            ),
        ] {
            let open = self.menu == Some(chip);
            let tip = match chip {
                Chip::Mode => "Mode",
                Chip::Agent => "Agent: a new agent takes over the session",
                Chip::Effort => "Effort",
            };
            chips = chips.child(
                div()
                    .relative()
                    .flex_none()
                    .child(
                        div()
                            .id(SharedString::from(format!("chip-{chip:?}")))
                            .h(px(22.))
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .text_color(rgb(if open || changed {
                                tk().ink
                            } else {
                                tk().ink_2
                            }))
                            .when(changed, |d| d.font_weight(FontWeight::MEDIUM))
                            .when(open, |d| d.bg(rgb(icon_hover())))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(icon_hover())).text_color(rgb(tk().ink)))
                            .tooltip(ui::tip(tip))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_menu(chip, cx)))
                            .child(format!("{label} ▾")),
                    )
                    .when(open, |d| d.child(self.chip_menu(chip, item, &eff, cx))),
            );
        }
        let field = div()
            .flex()
            .rounded(tk().r_card)
            .border_1()
            .border_color(rgb(field_border()))
            .bg(rgb(tk().card))
            .when(ui::irix(), |d| {
                d.rounded(px(0.)).border_0().bg(rgb(0xFFFFFF))
            })
            .when(ui::irix(), |d| ui::bevel_edges(false, d))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .justify_between()
                    .p(px(4.))
                    .child(
                        icon_button("capture", "icons/capture.svg", "Capture the screen")
                            .on_click(cx.listener(|this, _, window, cx| this.capture(window, cx))),
                    )
                    .child(
                        icon_button("attach", "icons/attach.svg", "Attach files")
                            .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .pt(px(6.))
                    .pb(px(4.))
                    .child(
                        div()
                            .min_h(px(38.))
                            .px(px(2.))
                            .pt(px(2.))
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .line_height(relative(1.45))
                            .child(input),
                    )
                    .child(chips),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .justify_between()
                    .p(px(4.))
                    .when(compose::VOICE_AVAILABLE, |d| {
                        let listening = self.tools.listening.is_some();
                        d.child(
                            icon_button(
                                "mic",
                                "icons/mic.svg",
                                if listening {
                                    "Stop listening"
                                } else {
                                    "Dictate"
                                },
                            )
                            .when(listening, |d| d.bg(rgb(tk().e_bg)))
                            .on_click(cx.listener(|this, _, window, cx| this.dictate(window, cx))),
                        )
                    })
                    .child(
                        div()
                            .id("inbox-send")
                            .tooltip(ui::tip("Send"))
                            .mt_auto()
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(tk().r_button)
                            .bg(rgb(tk().primary))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(tk().primary_hover)))
                            .when(ui::irix(), |d| d.rounded(px(0.)).bg(rgb(0xC8C8C8)))
                            .when(ui::irix(), |d| ui::bevel_edges(true, d))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                            .child(ui::icon(
                                "icons/send.svg",
                                14.,
                                rgb(if ui::irix() { 0x111111 } else { tk().card }),
                            )),
                    ),
            );
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .pt(px(6.))
            .pb(px(12.))
            .border_t_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(2.))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .child(
                        div()
                            .text_color(rgb(AGENT_ACCENT))
                            .child(activity_text(item, now)),
                    )
                    .child(
                        div()
                            .text_color(rgb(tk().ink_3))
                            .child(format!("ctx {}% · ${:.2}", item.context_pct, item.cost)),
                    ),
            )
            .when(!done, |d| {
                d.children(compose::chips(
                    &self.tools,
                    "inbox",
                    compose::Palette::page(),
                    |i| {
                        Box::new(cx.listener(move |this, _, _, cx| {
                            this.tools.remove(i);
                            cx.notify();
                        }))
                    },
                ))
                .children(compose::status(
                    &self.tools,
                    "inbox",
                    compose::Palette::page(),
                    |action| {
                        Box::new(cx.listener(move |this, _, window, cx| {
                            this.tool_action(action, window, cx)
                        }))
                    },
                ))
                .child(field)
            })
    }

    /// The menu of a chip, over the chip.
    fn chip_menu(
        &self,
        chip: Chip,
        item: &InboxItem,
        eff: &ComposerPrefs,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entries: Vec<(String, bool)> = match chip {
            Chip::Mode => [Mode::Plan, Mode::Edit, Mode::Auto]
                .into_iter()
                .map(|m| (mode_label(m).to_string(), m == eff.mode))
                .collect(),
            Chip::Effort => [Effort::Low, Effort::Medium, Effort::High, Effort::Max]
                .into_iter()
                .map(|e| (effort_label(e).to_string(), e == eff.effort))
                .collect(),
            Chip::Agent => PROVIDERS
                .iter()
                .flat_map(|p| provider_models(p).iter().map(move |m| format!("{p} · {m}")))
                .map(|label| {
                    let on = label == format!("{} · {}", eff.provider, eff.model);
                    (label, on)
                })
                .collect(),
        };
        let mut menu = ui::menu(160.);
        for (i, (label, on)) in entries.into_iter().enumerate() {
            let (item, value) = (item.clone(), label.clone());
            menu = menu.child(ui::menu_item(
                SharedString::from(format!("cm-{chip:?}-{i}")),
                on,
                div().child(label).into_any_element(),
                None,
                Box::new(cx.listener(move |this, _, _, cx| this.pick(&item, chip, &value, cx))),
            ));
        }
        deferred(
            div()
                .absolute()
                .bottom(relative(1.))
                .mb(px(6.))
                .left_0()
                .child(menu)
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    cx.notify();
                })),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn detail(
        &self,
        item: &InboxItem,
        position: Option<(usize, usize)>,
        (prev, next): (Option<String>, Option<String>),
        input: Entity<TextareaState>,
        now: UnixMs,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let style = kind_style(item.kind);
        let done = item.result.is_some();
        let session = item.session.clone();
        let pill = match &item.result {
            Some(result) => kind_pill(result, tk().track, tk().ink_3),
            None => kind_pill(item.kind.label(), style.pill_bg, style.pill_text),
        };
        let provider = item.agents.first().map_or("", |a| a.provider.as_str());
        let header = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .pt(px(12.))
            .px(px(16.))
            .pb(px(11.))
            .border_b_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .min_w(px(0.))
                    .child(pill)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(item.title.clone()),
                    )
                    .when(!done, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                .text_color(rgb(tk().ink_3))
                                .whitespace_nowrap()
                                .child(format!(
                                    "waits {}",
                                    wait_text(wait_minutes(item.since, now))
                                )),
                        )
                    })
                    .child(
                        div()
                            .id("open-deep-dive")
                            .flex_none()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .text_color(rgb(tk().link))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(link_hover())))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(InboxEvent::OpenSession(session.clone()))
                            }))
                            .child("Open deep dive →"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(rgb(provider_color(provider))),
                            )
                            .child(agents_text(&item.agents)),
                    )
                    .child(where_text(item))
                    .child(format!("session {}", item.session))
                    .child(format!("${:.2}", item.cost)),
            );
        let pane = div()
            .flex_1()
            .min_h(px(0.))
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(tk().card))
            .border_1()
            .border_color(rgba(tk().line))
            .overflow_hidden()
            .child(header)
            .child(self.transcript(cx))
            .when(item.kind == InboxKind::Conflict, |d| {
                d.child(Self::conflict_sides(item))
            })
            .child(Self::ask_card(
                item,
                self.answer_error
                    .as_ref()
                    .filter(|(i, _)| *i == item.id)
                    .map(|(_, t)| t),
                cx,
            ))
            .child(self.composer(item, input, now, cx))
            .when_some(style.bar, |d, color| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(rgb(color)),
                )
            });
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(16.))
            .pb(px(16.))
            .pl(px(24.))
            .pr(px(32.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .text_color(rgb(tk().ink_3))
                            .child(position_text(position)),
                    )
                    .child(div().flex_1())
                    .child(self.nav_button("Previous", "icons/chevron-up.svg", true, prev, cx))
                    .child(self.nav_button("Next", "icons/chevron-down.svg", false, next, cx)),
            )
            .child(pane)
            .into_any_element()
    }
}

impl Render for InboxScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.ensure_input(window, cx);
        let now = self.now(cx);
        let width = window.viewport_size().width;
        let Some(data) = self.data.clone() else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.header(&[], now, cx))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().muted))
                        .child("Waits for aivoryd"),
                );
        };
        let shown: Vec<&InboxItem> = data
            .items
            .iter()
            .filter(|i| self.filter.matches(i.kind))
            .collect();
        let shown_ids: Vec<&str> = shown.iter().map(|i| i.id.as_str()).collect();
        let done_ids: Vec<&str> = data.done.iter().map(|i| i.id.as_str()).collect();
        let selected = resolve_selection(&shown_ids, &done_ids, self.selected.as_deref());
        self.selected.clone_from(&selected);
        let item = selected.as_deref().and_then(|id| {
            shown
                .iter()
                .copied()
                .find(|i| i.id == id)
                .or_else(|| data.done.iter().find(|i| i.id == id))
        });
        self.sync_tail(
            selected.as_deref(),
            item.map(|i| i.session.as_str()),
            item.map(transcript_of).unwrap_or_default(),
        );
        if self.input_for != selected {
            self.input_for.clone_from(&selected);
            input.update(cx, |s, cx| s.clean(window, cx));
        }
        let index = selected
            .as_deref()
            .and_then(|id| shown_ids.iter().position(|s| *s == id));
        let neighbors = index.map_or((None, None), |i| {
            (
                i.checked_sub(1).map(|p| shown_ids[p].to_string()),
                shown_ids.get(i + 1).map(|n| n.to_string()),
            )
        });
        let position = index.map(|i| (i + 1, shown.len()));
        let narrow = width < NARROW;
        let list_w = (!narrow).then_some(if width >= px(MEDIUM) {
            LIST_W
        } else {
            LIST_W_MEDIUM
        });
        let list = self.item_list(&shown, &data.done, selected.as_deref(), now, list_w, cx);
        let detail = match item {
            Some(item) => self.detail(item, position, neighbors, input, now, cx),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .text_color(rgb(tk().ink_3))
                .child("Nothing needs you. All agents work or wait for tasks.")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .line_height(relative(1.3))
            .child(self.header(&data.items, now, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(list)
                    .when(!narrow, |d| d.child(detail)),
            )
    }
}

// ---- Pure parts ----

/// The chips over the field of the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chip {
    Mode,
    Agent,
    Effort,
}

/// What the user picked in the chip menus of one session. A value that equals the session's
/// own value is not a pick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Picks {
    mode: Option<Mode>,
    /// Provider and model.
    agent: Option<(String, String)>,
    effort: Option<Effort>,
}

impl Picks {
    /// The prefs that the next message uses: the session's own with the picks over them.
    fn apply(&self, base: &ComposerPrefs) -> ComposerPrefs {
        let mut prefs = base.clone();
        if let Some(mode) = self.mode {
            prefs.mode = mode;
        }
        if let Some((provider, model)) = &self.agent {
            prefs.provider.clone_from(provider);
            prefs.model.clone_from(model);
        }
        if let Some(effort) = self.effort {
            prefs.effort = effort;
        }
        prefs
    }
}

/// What a send does with the picks.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Plan {
    /// Answer the item. `prefs_changed`: a mode or effort pick that the answer cannot carry.
    Answer { prefs_changed: bool },
    /// Another provider takes the session over: hand off, then send there.
    Handoff(ComposerPrefs),
}

fn plan(base: &ComposerPrefs, picks: &Picks) -> Plan {
    let eff = picks.apply(base);
    if eff.provider != base.provider {
        Plan::Handoff(eff)
    } else {
        Plan::Answer {
            prefs_changed: eff != *base,
        }
    }
}

fn mode_from(label: &str) -> Mode {
    match label {
        "Plan" => Mode::Plan,
        "Auto" => Mode::Auto,
        _ => Mode::Edit,
    }
}

fn effort_from(label: &str) -> Effort {
    match label {
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "max" => Effort::Max,
        _ => Effort::High,
    }
}

use crate::warroom::{effort_label, mode_label, provider_models};

/// The item counts of the filter segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Counts {
    all: usize,
    answers: usize,
    errors: usize,
    reviews: usize,
    conflicts: usize,
}

impl Counts {
    fn of(items: &[InboxItem]) -> Self {
        let mut c = Self {
            all: items.len(),
            ..Self::default()
        };
        for item in items {
            match item.kind.filter() {
                InboxFilter::Answers => c.answers += 1,
                InboxFilter::Errors => c.errors += 1,
                InboxFilter::Reviews => c.reviews += 1,
                InboxFilter::Conflicts => c.conflicts += 1,
                InboxFilter::All => {}
            }
        }
        c
    }

    fn get(&self, filter: InboxFilter) -> usize {
        match filter {
            InboxFilter::All => self.all,
            InboxFilter::Answers => self.answers,
            InboxFilter::Errors => self.errors,
            InboxFilter::Reviews => self.reviews,
            InboxFilter::Conflicts => self.conflicts,
        }
    }
}

/// Whole minutes since `since`.
fn wait_minutes(since: UnixMs, now: UnixMs) -> i64 {
    (now - since).max(0) / MINUTE_MS
}

/// `6 min`, `1 h`, `2 h 5 min`.
fn wait_text(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// `6 open · oldest waits 12 min`.
fn header_text(items: &[InboxItem], now: UnixMs) -> String {
    let oldest = items
        .iter()
        .map(|i| wait_minutes(i.since, now))
        .max()
        .map_or_else(|| "—".to_string(), wait_text);
    format!("{} open · oldest waits {oldest}", items.len())
}

/// `2 of 5`, or the note for a done item.
fn position_text(position: Option<(usize, usize)>) -> String {
    match position {
        Some((n, m)) => format!("{n} of {m}"),
        None => "done in this session".to_string(),
    }
}

/// What the user sees selected: the pick when it shows (open in this filter, or done), else the
/// first open item.
fn resolve_selection(shown: &[&str], done: &[&str], pick: Option<&str>) -> Option<String> {
    match pick {
        Some(p) if shown.contains(&p) || done.contains(&p) => Some(p.to_string()),
        _ => shown.first().map(|s| (*s).to_string()),
    }
}

/// The item to select after `answered` leaves the list: the one that takes its place, else the
/// last one. An id that is not in the list gives the first item.
fn next_after(shown: &[&str], answered: &str) -> Option<String> {
    let Some(at) = shown.iter().position(|id| *id == answered) else {
        return shown.first().map(|s| (*s).to_string());
    };
    let rest: Vec<&str> = shown.iter().copied().filter(|id| *id != answered).collect();
    rest.get(at.min(rest.len().saturating_sub(1)))
        .map(|s| (*s).to_string())
}

/// `claude · opus-5.5`, or `claude · haiku-5.5 and codex · gpt-5.2-codex` for a conflict.
fn agents_text(agents: &[AgentRef]) -> String {
    agents
        .iter()
        .map(|a| format!("{} · {}", a.provider, a.model))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// `billing / payment`.
fn where_text(item: &InboxItem) -> String {
    format!("{} / {}", item.codebase, item.part)
}

/// The line over the composer.
fn activity_text(item: &InboxItem, now: UnixMs) -> String {
    if item.result.is_some() {
        return "Running again".to_string();
    }
    match item.kind {
        InboxKind::Review => "Done · waits for your review".to_string(),
        InboxKind::Conflict => "Trial merge failed".to_string(),
        _ => format!(
            "Waiting for your answer · {}",
            wait_text(wait_minutes(item.since, now))
        ),
    }
}

/// The transcript of an item: the tail, and for a done item the result as the answer.
fn transcript_of(item: &InboxItem) -> Vec<Line> {
    let mut lines = item.tail.clone();
    if let Some(result) = &item.result {
        lines.push(Line::user(result.clone()));
    }
    lines
}

// ---- Styles ----

/// The colors of one kind: pill, top bar and ask card.
#[derive(Debug, Clone, Copy)]
struct KindStyle {
    pill_bg: u32,
    pill_text: u32,
    bar: Option<u32>,
    ask_bg: u32,
    ask_border: u32,
    ask_head_bg: u32,
    ask_head: u32,
}

/// The kind colors for the current page: the light cards, or the tokens on a dark page.
fn kind_style(kind: InboxKind) -> KindStyle {
    let s = light_kind_style(kind);
    if !tk().night {
        return s;
    }
    let (pill_bg, pill_text) = match kind {
        InboxKind::Review => (tk().ok_bg, tk().ok_text),
        InboxKind::Conflict => (tk().work_bg, tk().work_text),
        _ => (s.pill_bg, s.pill_text),
    };
    KindStyle {
        pill_bg,
        pill_text,
        bar: s.bar,
        ask_bg: tk().card,
        ask_border: tk().track_2,
        ask_head_bg: pill_bg,
        ask_head: pill_text,
    }
}

fn light_kind_style(kind: InboxKind) -> KindStyle {
    match kind {
        InboxKind::Permission | InboxKind::Plan | InboxKind::Question => KindStyle {
            pill_bg: tk().q_bg,
            pill_text: tk().q_text,
            bar: Some(tk().q),
            ask_bg: 0xFFFBF4,
            ask_border: 0xF1D9B5,
            ask_head_bg: 0xFDF1DE,
            ask_head: 0x92400E,
        },
        InboxKind::Error => KindStyle {
            pill_bg: tk().e_bg,
            pill_text: tk().e_text,
            bar: Some(tk().e),
            ask_bg: 0xFFF8F8,
            ask_border: 0xF2C7C7,
            ask_head_bg: 0xFCE8E8,
            ask_head: 0x991B1B,
        },
        InboxKind::Review => KindStyle {
            pill_bg: 0xEAF7EE,
            pill_text: 0x15803D,
            bar: Some(tk().ok),
            ask_bg: 0xF6FCF8,
            ask_border: 0xBFE5CB,
            ask_head_bg: 0xE5F5EA,
            ask_head: 0x166534,
        },
        InboxKind::Conflict => KindStyle {
            pill_bg: 0xF3EEFF,
            pill_text: 0x6D28D9,
            bar: None,
            ask_bg: 0xFAF7FF,
            ask_border: 0xDCCFFB,
            ask_head_bg: 0xEFE8FE,
            ask_head: 0x5B21B6,
        },
    }
}

/// The kind pill (`.kd`).
fn kind_pill(label: &str, bg: u32, text: u32) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(1.))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(text))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(label.to_string())
}

/// The lines of a conflict side (`.cfp .ln`): 11.5 px console text, no side padding.
fn side_lines(lines: &[Line]) -> AnyElement {
    let style = Style {
        pad_x: 0.,
        ..Style::plain(11.5, crate::theme::PaneTheme::WHITE)
    };
    let hooks = Hooks::default();
    let id: SharedString = "inbox-side".into();
    let frame = Frame {
        style: &style,
        hooks: &hooks,
        id: &id,
        tick: 0,
    };
    console::draw::stack(lines, &frame).into_any_element()
}

/// The text of a failed answer: the message of the daemon, or the error chain.
fn answer_error_text(e: &anyhow::Error) -> String {
    match e.downcast_ref::<aivory_protocol::rpc::RpcError>() {
        Some(r) => r.message.clone(),
        None => format!("{e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    fn prefs() -> ComposerPrefs {
        ComposerPrefs {
            mode: Mode::Edit,
            provider: "claude".into(),
            model: "opus-5.5".into(),
            effort: Effort::High,
        }
    }

    #[test]
    fn no_pick_is_a_plain_answer() {
        assert_eq!(
            plan(&prefs(), &Picks::default()),
            Plan::Answer {
                prefs_changed: false
            }
        );
    }

    #[test]
    fn a_mode_or_effort_pick_is_noted_and_the_answer_stays() {
        let picks = Picks {
            mode: Some(Mode::Plan),
            effort: Some(Effort::Max),
            ..Picks::default()
        };
        assert_eq!(
            plan(&prefs(), &picks),
            Plan::Answer {
                prefs_changed: true
            }
        );
    }

    #[test]
    fn another_provider_hands_the_session_off_with_all_picks() {
        let picks = Picks {
            agent: Some(("codex".into(), "gpt-5.2-codex".into())),
            effort: Some(Effort::Low),
            ..Picks::default()
        };
        let Plan::Handoff(eff) = plan(&prefs(), &picks) else {
            panic!("a handoff");
        };
        assert_eq!(eff.provider, "codex");
        assert_eq!(eff.model, "gpt-5.2-codex");
        assert_eq!(eff.effort, Effort::Low);
        assert_eq!(eff.mode, Mode::Edit);
    }

    #[test]
    fn another_model_of_the_same_provider_is_no_handoff() {
        let picks = Picks {
            agent: Some(("claude".into(), "haiku-5.5".into())),
            ..Picks::default()
        };
        assert_eq!(
            plan(&prefs(), &picks),
            Plan::Answer {
                prefs_changed: true
            }
        );
        assert_eq!(picks.apply(&prefs()).model, "haiku-5.5");
    }

    #[test]
    fn the_menu_words_map_back_to_the_values() {
        for m in [Mode::Plan, Mode::Edit, Mode::Auto] {
            assert_eq!(mode_from(mode_label(m)), m);
        }
        for e in [Effort::Low, Effort::Medium, Effort::High, Effort::Max] {
            assert_eq!(effort_from(effort_label(e)), e);
        }
    }

    fn item(id: &str, kind: InboxKind, minutes_ago: i64) -> InboxItem {
        InboxItem {
            id: id.to_string(),
            kind,
            session: id.to_string(),
            ask: None,
            title: id.to_string(),
            codebase: "billing".to_string(),
            part: "payment".to_string(),
            agents: vec![AgentRef {
                provider: "claude".to_string(),
                model: "opus-5.5".to_string(),
            }],
            since: 1_000_000_000 - minutes_ago * MINUTE_MS,
            cost: 1.0,
            context_pct: 40,
            preview: String::new(),
            text: String::new(),
            options: vec!["Allow".to_string()],
            steps: Vec::new(),
            gates: Vec::new(),
            sides: Vec::new(),
            prompt: String::new(),
            files: Vec::new(),
            tail: vec![Line::agent("hello")],
            result: None,
        }
    }

    const NOW: UnixMs = 1_000_000_000;

    #[test]
    fn counts_follow_the_filter_groups() {
        let items = [
            item("a", InboxKind::Permission, 1),
            item("b", InboxKind::Plan, 1),
            item("c", InboxKind::Question, 1),
            item("d", InboxKind::Error, 1),
            item("e", InboxKind::Review, 1),
            item("f", InboxKind::Conflict, 1),
        ];
        let c = Counts::of(&items);
        assert_eq!(
            [
                c.get(InboxFilter::All),
                c.get(InboxFilter::Answers),
                c.get(InboxFilter::Errors),
                c.get(InboxFilter::Reviews),
                c.get(InboxFilter::Conflicts),
            ],
            [6, 3, 1, 1, 1]
        );
        assert_eq!(Counts::of(&[]), Counts::default());
    }

    #[test]
    fn demo_inbox_counts_match_the_artboard() {
        let inbox = Demo::new().inbox;
        let c = Counts::of(&inbox.items);
        assert_eq!(
            (c.all, c.answers, c.errors, c.reviews, c.conflicts),
            (6, 3, 1, 1, 1)
        );
        assert_eq!(
            header_text(&inbox.items, Demo::new().war_room.now),
            "6 open · oldest waits 12 min"
        );
    }

    #[test]
    fn next_item_takes_the_place_of_the_answered_one() {
        let ids = ["a", "b", "c"];
        assert_eq!(next_after(&ids, "a").as_deref(), Some("b"));
        assert_eq!(next_after(&ids, "b").as_deref(), Some("c"));
        // The last item: the new last item.
        assert_eq!(next_after(&ids, "c").as_deref(), Some("b"));
        assert_eq!(next_after(&["a"], "a"), None);
        assert_eq!(next_after(&[], "a"), None);
        // An id that is not in the list: the first item.
        assert_eq!(next_after(&ids, "x").as_deref(), Some("a"));
    }

    #[test]
    fn selection_falls_back_to_the_first_open_item() {
        let shown = ["a", "b"];
        let done = ["z"];
        assert_eq!(
            resolve_selection(&shown, &done, Some("b")).as_deref(),
            Some("b")
        );
        assert_eq!(
            resolve_selection(&shown, &done, Some("z")).as_deref(),
            Some("z")
        );
        // The pick left the list or the filter: the first open item.
        assert_eq!(
            resolve_selection(&shown, &done, Some("q")).as_deref(),
            Some("a")
        );
        assert_eq!(resolve_selection(&shown, &done, None).as_deref(), Some("a"));
        assert_eq!(resolve_selection(&[], &done, None), None);
        assert_eq!(
            resolve_selection(&[], &done, Some("z")).as_deref(),
            Some("z")
        );
    }

    #[test]
    fn wait_text_reads_as_minutes_then_hours() {
        assert_eq!(wait_text(0), "0 min");
        assert_eq!(wait_text(12), "12 min");
        assert_eq!(wait_text(59), "59 min");
        assert_eq!(wait_text(60), "1 h");
        assert_eq!(wait_text(125), "2 h 5 min");
        assert_eq!(wait_minutes(NOW - 6 * MINUTE_MS - 59_000, NOW), 6);
        // A clock that runs behind never gives a negative wait.
        assert_eq!(wait_minutes(NOW + MINUTE_MS, NOW), 0);
    }

    #[test]
    fn header_names_the_oldest_wait() {
        let items = [
            item("a", InboxKind::Permission, 6),
            item("b", InboxKind::Review, 12),
        ];
        assert_eq!(header_text(&items, NOW), "2 open · oldest waits 12 min");
        assert_eq!(header_text(&[], NOW), "0 open · oldest waits —");
    }

    #[test]
    fn position_and_activity_texts() {
        assert_eq!(position_text(Some((2, 5))), "2 of 5");
        assert_eq!(position_text(None), "done in this session");
        let mut a = item("a", InboxKind::Permission, 6);
        assert_eq!(activity_text(&a, NOW), "Waiting for your answer · 6 min");
        a.kind = InboxKind::Review;
        assert_eq!(activity_text(&a, NOW), "Done · waits for your review");
        a.result = Some("merged".to_string());
        assert_eq!(activity_text(&a, NOW), "Running again");
    }

    #[test]
    fn a_done_item_ends_with_the_result() {
        let mut a = item("a", InboxKind::Permission, 6);
        assert_eq!(transcript_of(&a), a.tail);
        a.result = Some("allowed".to_string());
        let lines = transcript_of(&a);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], Line::user("allowed"));
    }

    #[test]
    fn the_demo_conflict_answers_with_its_first_option() {
        // The conflict item offers "Ping both" first; the card answers with the option text
        // through inbox/answer, so the daemon decides what the option does.
        let demo = aivory_protocol::demo::Demo::new();
        let conflict = demo
            .inbox
            .items
            .iter()
            .find(|i| i.kind == InboxKind::Conflict)
            .expect("the demo has a conflict");
        assert_eq!(
            conflict.options.first().map(String::as_str),
            Some("Ping both")
        );
        assert_eq!(conflict.agents.len(), 2);
    }

    #[test]
    fn a_failed_answer_shows_the_message_of_the_daemon() {
        let e = anyhow::Error::new(aivory_protocol::rpc::RpcError::new(
            aivory_protocol::rpc::code::STALE_ASK,
            "the item is not open any more",
        ));
        assert_eq!(answer_error_text(&e), "the item is not open any more");
        assert_eq!(
            answer_error_text(&anyhow::anyhow!("pipe closed")),
            "pipe closed"
        );
    }

    #[test]
    fn agents_join_with_and() {
        let mut a = item("a", InboxKind::Conflict, 2);
        a.agents.push(AgentRef {
            provider: "codex".to_string(),
            model: "gpt-5.2-codex".to_string(),
        });
        assert_eq!(
            agents_text(&a.agents),
            "claude · opus-5.5 and codex · gpt-5.2-codex"
        );
        assert_eq!(where_text(&a), "billing / payment");
    }
}
