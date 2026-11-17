//! The Tactical screens Inbox, Tasks, Repos and Settings, in the grammar of the look: a title with
//! readouts, a menu row, and a console of two framed panels (the list at the left, the one thing
//! selected at the right). The same structure as the Bridge screens: title, menu, list, detail.
//! Every control calls the daemon through `actions.rs`. "ALL FUNCTIONS" shows the full screen
//! (with the editor, the filters and the forms) inside the Tactical frame.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{InboxListParams, TaskListParams, method};
use aivory_protocol::{
    InboxFilter, InboxItem, InboxKind, InboxList, LineKind, Pane, StepState, Task, TaskColumn,
    TaskList, rpc::AutopilotPatch,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, SharedString, Stateful, Task as GpuiTask,
    Window, div, px,
};

use crate::actions::{Action, act};
use crate::bridge_screens::{TaskAct, task_act};
use crate::model::Model;
use crate::tactical_kit::{
    self as tac, Btn, Frame, c, code, dots, entry, frame, label, navb, section, tbox, trow,
};
use crate::ui;

/// What a screen asks the shell to do.
#[derive(Debug, Clone)]
pub enum TacEvent {
    OpenSession(String),
    /// Show the full screen of this view inside the frame.
    Full,
}

/// A scrolling column.
fn scroll(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

/// The two panels of a console screen.
pub(crate) fn console(left: Div, right: Div) -> AnyElement {
    div()
        .size_full()
        .flex()
        .gap(px(32.))
        .child(left.w(px(520.)).flex_none())
        .child(right.flex_1().min_w(px(0.)))
        .into_any_element()
}

/// A panel of the console.
pub(crate) fn panel(caption: impl Into<SharedString>) -> Div {
    tbox(caption).p(px(24.)).pt(px(28.)).gap(px(14.))
}

/// `now` or `N min`.
fn minutes(now: i64, since: i64) -> String {
    let m = ((now - since) / 60_000).max(0);
    if m < 1 {
        "now".to_string()
    } else {
        format!("{m} min")
    }
}

/// Reload a list now and then.
fn ticker<V: 'static>(
    cx: &mut Context<V>,
    every: Duration,
    f: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> GpuiTask<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(every).await;
            if this.update(cx, |t, cx| f(t, cx)).is_err() {
                break;
            }
        }
    })
}

/// The last prose rows of a transcript as `YOU:` and `AGENT:` lines.
fn chat_rows(tail: &[aivory_protocol::Line], n: usize) -> Vec<String> {
    let mut rows: Vec<String> = tail
        .iter()
        .filter_map(|l| {
            let t = l.text.lines().next().unwrap_or("").trim();
            match l.kind {
                LineKind::User if !t.is_empty() => Some(format!("YOU: {t}")),
                LineKind::Agent if t.chars().next().is_some_and(char::is_alphabetic) => {
                    Some(format!("AGENT: {t}"))
                }
                _ => None,
            }
        })
        .collect();
    let skip = rows.len().saturating_sub(n);
    rows.split_off(skip)
}

/// A menu that closes with the buttons that every screen has.
fn menu_end(
    mut menu: Vec<AnyElement>,
    open: Option<AnyElement>,
    full: AnyElement,
) -> Vec<AnyElement> {
    menu.push(div().flex_1().into_any_element());
    if let Some(o) = open {
        menu.push(o);
    }
    menu.push(full);
    menu
}

// =============================================================================== Inbox ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InboxTab {
    All,
    Waiting,
    Errors,
    Reviews,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Details,
    Files,
    Chat,
}

pub struct TacticalInbox {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    tab: InboxTab,
    side: Side,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: gpui::Subscription,
}

impl EventEmitter<TacEvent> for TacticalInbox {}

impl TacticalInbox {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |this, _, cx| this.refresh(cx));
        let tick = ticker(cx, Duration::from_secs(5), |t: &mut Self, cx| t.refresh(cx));
        let mut this = Self {
            model,
            data: None,
            tab: InboxTab::All,
            side: Side::Details,
            selected: None,
            busy: false,
            _tick: tick,
            _obs: obs,
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
                if let Ok(list) = list {
                    let changed = this.data.as_deref() != Some(&list);
                    this.data = Some(Rc::new(list));
                    if changed {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn items(&self) -> Vec<InboxItem> {
        let Some(d) = &self.data else {
            return Vec::new();
        };
        let f = |filter: InboxFilter| {
            d.items
                .iter()
                .filter(|i| filter.matches(i.kind))
                .cloned()
                .collect()
        };
        match self.tab {
            InboxTab::All => d.items.clone(),
            InboxTab::Waiting => f(InboxFilter::Answers),
            InboxTab::Errors => f(InboxFilter::Errors),
            InboxTab::Reviews => f(InboxFilter::Reviews),
            InboxTab::Done => d.done.clone(),
        }
    }
}

/// An item that needs the owner is amber; the rest is quiet.
fn needs_you(kind: InboxKind) -> bool {
    InboxFilter::Answers.matches(kind) || InboxFilter::Errors.matches(kind)
}

impl Render for TacticalInbox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let items = self.items();
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| items.iter().find(|i| &i.id == id))
            .or(items.first())
            .cloned();
        let count = |f: InboxFilter| {
            self.data
                .as_ref()
                .map_or(0, |d| d.items.iter().filter(|i| f.matches(i.kind)).count())
        };
        let all = self.data.as_ref().map_or(0, |d| d.items.len());
        let done = self.data.as_ref().map_or(0, |d| d.done.len());
        let oldest = self
            .data
            .as_ref()
            .and_then(|d| d.items.iter().map(|i| i.since).min())
            .map_or("NONE".to_string(), |s| minutes(now, s).to_uppercase());
        let waiting = count(InboxFilter::Answers);
        let stats = vec![
            (
                waiting.to_string(),
                "AWAIT ORDERS".to_string(),
                if waiting > 0 { tac::WHITE } else { tac::INK },
            ),
            (
                count(InboxFilter::Errors).to_string(),
                "ERRORS".into(),
                tac::INK,
            ),
            (oldest, "OLDEST".into(), tac::INK),
        ];

        let mut menu: Vec<AnyElement> = Vec::new();
        for (t, name, n) in [
            (InboxTab::All, "ALL", all),
            (InboxTab::Waiting, "WAITING", waiting),
            (InboxTab::Errors, "ERRORS", count(InboxFilter::Errors)),
            (InboxTab::Reviews, "REVIEWS", count(InboxFilter::Reviews)),
            (InboxTab::Done, "DONE", done),
        ] {
            menu.push(
                navb(
                    format!("ti-tab-{name}"),
                    &format!("{name} {n:02}"),
                    self.tab == t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    this.selected = None;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        let open = cur.as_ref().map(|i| i.session.clone());
        let open_btn = navb("ti-open", "OPEN SELECTED >", false)
            .border_1()
            .border_color(c(tac::INK, 1.))
            .text_color(c(tac::INK, 1.))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(id) = &open {
                    cx.emit(TacEvent::OpenSession(id.clone()));
                }
            }))
            .into_any_element();
        let full = navb("ti-full", "FULL INBOX >", false)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(TacEvent::Full)))
            .into_any_element();

        // The list.
        let mut list = scroll("ti-list");
        if items.is_empty() {
            list = list.child(entry("Nothing waits for you."));
        }
        for (k, it) in items.iter().enumerate() {
            let id = it.id.clone();
            let hot = needs_you(it.kind) && self.tab != InboxTab::Done;
            list = list.child(
                trow(
                    format!("ti-row-{k}"),
                    if hot { tac::WHITE } else { tac::INK },
                    &dots(&it.title, 40),
                    &format!("{} / {} · {}", it.codebase, it.part, it.kind.label()),
                    &minutes(now, it.since).to_uppercase(),
                    if hot { tac::WHITE } else { tac::LABEL },
                    cur.as_ref().is_some_and(|x| x.id == it.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        let left = panel(format!("SEC-01 / QUEUE · {} ITEMS", items.len()))
            .child(list)
            .child(foot(format!(
                "{all} IN ALL · {waiting} WAIT FOR YOU · {done} DONE. SELECT A ROW FOR THE DECISION."
            )));

        // The detail.
        let mut right = panel("SEC-02 / DECISION");
        if let Some(it) = &cur {
            let hot = needs_you(it.kind) && it.result.is_none();
            right = right
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .justify_between()
                        .child(label(format!(
                            "{} · WAITS {}",
                            it.kind.label().to_uppercase(),
                            minutes(now, it.since).to_uppercase()
                        )))
                        .child(label(
                            it.agents
                                .first()
                                .map_or(String::new(), |a| a.provider.to_uppercase()),
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(tac::SANS)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(ui::ts(tac::S_SECTION))
                        .line_height(ui::ts(tac::S_SECTION) * 1.4)
                        .text_color(c(if hot { tac::WHITE } else { tac::INK }, 1.))
                        .child(it.title.clone()),
                );
            let mut orders = div().flex_none().flex().flex_wrap().gap(px(8.));
            if let Some(result) = &it.result {
                orders = orders.child(label(format!("DONE · {}", result.to_uppercase())));
            } else {
                for (k, o) in it.options.iter().enumerate() {
                    let (id, opt) = (it.id.clone(), o.clone());
                    let kind = if k == 0 { Btn::On } else { Btn::Plain };
                    orders = orders.child(
                        tac::order(
                            format!("ti-opt-{k}"),
                            &format!("[{}] {}", k + 1, dots(&o.to_uppercase(), 16)),
                            kind,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(
                                &this.model,
                                Action::InboxAnswer {
                                    id: id.clone(),
                                    option: Some(opt.clone()),
                                    text: None,
                                },
                                cx,
                            );
                            this.refresh(cx);
                        })),
                    );
                }
            }
            right = right.child(orders);
            let mut tabs = div().flex_none().flex().gap(px(6.));
            for (s, name) in [
                (Side::Details, "DETAILS"),
                (Side::Files, "FILES"),
                (Side::Chat, "CHAT"),
            ] {
                tabs = tabs.child(
                    navb(format!("ti-side-{name}"), name, self.side == s).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        },
                    )),
                );
            }
            right = right.child(tabs);
            let mut body = scroll("ti-detail").gap(px(12.));
            match self.side {
                Side::Details => {
                    let why = if it.preview.is_empty() {
                        &it.text
                    } else {
                        &it.preview
                    };
                    body = body
                        .child(label("WHY IT ASKS"))
                        .child(entry(why.clone()))
                        .child(label("COMMAND OR QUESTION"))
                        .child(code(it.text.clone()));
                    if !it.steps.is_empty() {
                        body = body.child(label("PLAN")).children(
                            it.steps
                                .iter()
                                .take(8)
                                .enumerate()
                                .map(|(k, s)| entry(format!("{}. {s}", k + 1))),
                        );
                    }
                    if !it.files.is_empty() {
                        body = body.child(label(format!("FILES CHANGED · {}", it.files.len())));
                        for f in it.files.iter().take(4) {
                            body = body.child(code(f.clone()));
                        }
                    }
                    let rows = chat_rows(&it.tail, 5);
                    if !rows.is_empty() {
                        body = body.child(label("LAST ROWS OF THE CHAT"));
                        for r in rows {
                            body = body.child(entry(r));
                        }
                    }
                }
                Side::Files => {
                    body = body.child(label(format!("FILES CHANGED · {}", it.files.len())));
                    for f in it.files.iter().take(20) {
                        body = body.child(code(f.clone()));
                    }
                }
                Side::Chat => {
                    body = body.child(label("LAST ROWS OF THE CHAT"));
                    for r in chat_rows(&it.tail, 8) {
                        body = body.child(entry(r));
                    }
                }
            }
            right = right.child(body);
        } else {
            right = right
                .child(section("ALL CLEAR"))
                .child(entry("No decision waits for you. A new ask shows here."));
        }

        frame(Frame {
            title: "INBOX".into(),
            kicker: "EVERY DECISION THAT WAITS FOR YOU, THE LONGEST WAIT FIRST".into(),
            stats,
            menu: menu_end(menu, Some(open_btn), full),
            body: console(left, right),
        })
    }
}

// =============================================================================== Tasks ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskTab {
    Queued,
    Running,
    Review,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskSide {
    Plan,
    Sessions,
}

pub struct TacticalTasks {
    model: Entity<Model>,
    data: Option<Rc<TaskList>>,
    tab: TaskTab,
    side: TaskSide,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: gpui::Subscription,
}

impl EventEmitter<TacEvent> for TacticalTasks {}

impl TacticalTasks {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        let tick = ticker(cx, Duration::from_secs(5), |t: &mut Self, cx| t.refresh(cx));
        let mut this = Self {
            model,
            data: None,
            tab: TaskTab::Running,
            side: TaskSide::Plan,
            selected: None,
            busy: false,
            _tick: tick,
            _obs: obs,
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
            .call::<method::TasksList>(TaskListParams::default(), cx);
        cx.spawn(async move |this, cx| {
            let list = call.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Ok(list) = list {
                    let changed = this.data.as_deref() != Some(&list);
                    this.data = Some(Rc::new(list));
                    if changed {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Load the list again after a task action.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.busy = false;
        self.refresh(cx);
    }

    fn of(&self, tab: TaskTab) -> Vec<Task> {
        let Some(d) = &self.data else {
            return Vec::new();
        };
        d.tasks
            .iter()
            .filter(|t| match tab {
                TaskTab::Queued => t.column == TaskColumn::Planned,
                TaskTab::Running => matches!(t.column, TaskColumn::Working | TaskColumn::Waiting),
                TaskTab::Review => t.column == TaskColumn::Review,
                TaskTab::Done => t.column == TaskColumn::Done,
            })
            .cloned()
            .collect()
    }
}

/// The state of a task as a word.
fn col_word(c: TaskColumn) -> &'static str {
    match c {
        TaskColumn::Planned => "QUEUED",
        TaskColumn::Working => "RUNNING",
        TaskColumn::Waiting => "HOLD",
        TaskColumn::Review => "REVIEW",
        TaskColumn::Done => "DONE",
    }
}

/// A line at the bottom of a list panel: counts, so that the panel is never empty.
fn foot(text: String) -> Div {
    div()
        .mt_auto()
        .flex_none()
        .pt(px(10.))
        .border_t_1()
        .border_color(c(tac::DEEP, 1.))
        .child(label(text))
}

fn step_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "DONE",
        StepState::Working => "NOW",
        StepState::Waiting => "HOLD",
        StepState::NotStarted => "QUEUED",
    }
}

impl Render for TacticalTasks {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (autopilot, queued_n) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map_or((false, 0), |w| (w.autopilot.on, w.queued));
        let n = |t| self.of(t).len();
        let stats = vec![
            (n(TaskTab::Queued).to_string(), "QUEUED".into(), tac::INK),
            (n(TaskTab::Running).to_string(), "RUNNING".into(), tac::INK),
            (n(TaskTab::Review).to_string(), "IN REVIEW".into(), tac::INK),
        ];
        let mut menu: Vec<AnyElement> = Vec::new();
        for (t, name) in [
            (TaskTab::Queued, "QUEUED"),
            (TaskTab::Running, "RUNNING"),
            (TaskTab::Review, "REVIEW"),
            (TaskTab::Done, "DONE"),
        ] {
            menu.push(
                navb(
                    format!("tt-tab-{name}"),
                    &format!("{name} {:02}", n(t)),
                    self.tab == t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    this.selected = None;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        menu.push(
            navb(
                "tt-auto",
                &format!("AUTOPILOT: {}", if autopilot { "ON" } else { "OFF" }),
                autopilot,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                act(
                    &this.model,
                    Action::Autopilot(AutopilotPatch {
                        on: Some(!autopilot),
                        ..AutopilotPatch::default()
                    }),
                    cx,
                );
            }))
            .into_any_element(),
        );
        menu.push(label(format!("{queued_n} QUEUED FOR AUTOPILOT")).into_any_element());

        let tasks = self.of(self.tab);
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .or(tasks.first())
            .cloned();
        let open = cur
            .as_ref()
            .and_then(|t| t.sessions.first().map(|s| s.session.clone()));
        let open_btn = navb("tt-open", "OPEN SELECTED >", false)
            .border_1()
            .border_color(c(tac::INK, 1.))
            .text_color(c(tac::INK, 1.))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(id) = &open {
                    cx.emit(TacEvent::OpenSession(id.clone()));
                }
            }))
            .into_any_element();
        let full = navb("tt-full", "FULL TASKS >", false)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(TacEvent::Full)))
            .into_any_element();

        let mut list = scroll("tt-list");
        if tasks.is_empty() {
            list = list.child(entry("No task in this list."));
        }
        for (k, t) in tasks.iter().enumerate() {
            let id = t.id.clone();
            let hot = t.waiting_for.is_some();
            list = list.child(
                trow(
                    format!("tt-row-{k}"),
                    if hot { tac::WHITE } else { tac::INK },
                    &dots(&t.title, 40),
                    &format!(
                        "{} · {}",
                        t.agents.first().cloned().unwrap_or_default().to_uppercase(),
                        t.codebase
                    ),
                    &format!("{} · STEP {}", col_word(t.column), (t.step + 1).min(5)),
                    if hot { tac::WHITE } else { tac::LABEL },
                    cur.as_ref().is_some_and(|x| x.id == t.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        let left = panel(format!("SEC-01 / TASKS · {}", tasks.len()))
            .child(list)
            .child(foot(format!(
                "{} QUEUED · {} RUNNING · {} IN REVIEW. SELECT A ROW FOR THE PLAN.",
                n(TaskTab::Queued),
                n(TaskTab::Running),
                n(TaskTab::Review)
            )));

        let mut right = panel("SEC-02 / TASK");
        if let Some(t) = &cur {
            right = right
                .child(section(dots(&t.title, 44)))
                .child(label(format!(
                    "{} · {} / {} · ${:.2}",
                    t.agents.first().cloned().unwrap_or_default().to_uppercase(),
                    t.codebase.to_uppercase(),
                    t.part.to_uppercase(),
                    t.cost
                )));
            let mut tabs = div().flex_none().flex().gap(px(6.));
            for (s, name) in [(TaskSide::Plan, "PLAN"), (TaskSide::Sessions, "SESSIONS")] {
                tabs = tabs.child(
                    navb(format!("tt-side-{name}"), name, self.side == s).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        },
                    )),
                );
            }
            if let Some(a) = TaskAct::of(t.column) {
                let id = t.id.clone();
                tabs = tabs.child(
                    navb("tt-act", &format!("{} >", a.label().to_uppercase()), false)
                        .border_1()
                        .border_color(c(tac::INK, 1.))
                        .text_color(c(tac::INK, 1.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            task_act(&this.model, &id, a, Self::reload, cx);
                        })),
                );
            }
            right = right.child(tabs);
            let mut body = scroll("tt-detail");
            match self.side {
                TaskSide::Plan => {
                    for (k, s) in t.steps.iter().enumerate() {
                        let hot = s.state == StepState::Waiting;
                        body = body.child(trow(
                            format!("tt-step-{k}"),
                            if hot { tac::WHITE } else { tac::INK },
                            &s.name,
                            &format!("WAYPOINT {}", k + 1),
                            step_word(s.state),
                            if hot { tac::WHITE } else { tac::LABEL },
                            s.state == StepState::Working,
                        ));
                    }
                }
                TaskSide::Sessions => {
                    if t.sessions.is_empty() {
                        body = body.child(entry("No session works on this task yet."));
                    }
                    for (k, s) in t.sessions.iter().enumerate() {
                        let id = s.session.clone();
                        body = body.child(
                            trow(
                                format!("tt-s-{k}"),
                                tac::INK,
                                &dots(&s.title, 40),
                                &s.role,
                                "OPEN >",
                                tac::LABEL,
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| cx.emit(TacEvent::OpenSession(id.clone())),
                            )),
                        );
                    }
                }
            }
            right = right.child(body);
            let war = self.model.read(cx).war.clone();
            let pane = war
                .as_ref()
                .and_then(|w| t.sessions.iter().find_map(|s| w.session(&s.session)));
            if let Some(pn) = pane {
                use crate::console::display::{DKind, last};
                let rows = last(&pn.tail, 40);
                let says: Vec<String> = rows
                    .iter()
                    .filter(|d| d.first && d.kind == DKind::Agent)
                    .map(|d| dots(&d.text, 150))
                    .collect();
                let tools: Vec<String> = rows
                    .iter()
                    .filter(|d| d.kind == DKind::Tool)
                    .map(|d| {
                        dots(
                            &format!("{} {}", d.label.clone().unwrap_or_default(), d.text),
                            90,
                        )
                    })
                    .collect();
                let mut more = div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(c(tac::DEEP, 1.))
                    .child(label(format!(
                        "LATEST FROM {} · ${:.2} · {} LINES",
                        pn.provider.to_uppercase(),
                        pn.cost,
                        pn.lines_total
                    )));
                for s in says.iter().rev().take(2).rev() {
                    more = more.child(entry(s.clone()));
                }
                for t2 in tools.iter().rev().take(3).rev() {
                    more = more.child(code(t2.clone()));
                }
                right = right.child(more);
            }
        } else {
            right = right.child(entry("Pick a task in the list."));
        }

        frame(Frame {
            title: "TASKS".into(),
            kicker: "THE WORK PLAN OF THE FLEET: QUEUED, RUNNING, IN REVIEW, DONE".into(),
            stats,
            menu: menu_end(menu, Some(open_btn), full),
            body: console(left, right),
        })
    }
}

// =============================================================================== Repos ====

pub struct TacticalRepos {
    model: Entity<Model>,
    selected: Option<String>,
    _obs: gpui::Subscription,
}

impl EventEmitter<TacEvent> for TacticalRepos {}

impl TacticalRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: None,
            _obs: obs,
        }
    }
}

impl Render for TacticalRepos {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let full = navb("tr-full", "FULL REPOS >", false)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(TacEvent::Full)))
            .into_any_element();
        let Some(war) = self.model.read(cx).war.clone() else {
            return frame(Frame {
                title: "REPOS".into(),
                kicker: "WAITING FOR THE DAEMON".into(),
                stats: Vec::new(),
                menu: menu_end(Vec::new(), None, full),
                body: entry("No data yet.").into_any_element(),
            });
        };
        let live = |c: &str| {
            war.sessions
                .iter()
                .filter(|s| s.codebase == c && s.state.is_live())
                .count()
        };
        let waits = |c: &str| {
            war.sessions
                .iter()
                .filter(|s| s.codebase == c && s.ask.is_some())
                .count()
        };
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| war.codebase(id))
            .or(war.codebases.first())
            .cloned();
        let counts = war.counts();
        let stats = vec![
            (
                war.codebases.len().to_string(),
                "CODEBASES".into(),
                tac::INK,
            ),
            (counts.live.to_string(), "LIVE".into(), tac::INK),
            (
                counts.needs_you.to_string(),
                "AWAIT ORDERS".into(),
                if counts.needs_you > 0 {
                    tac::WHITE
                } else {
                    tac::INK
                },
            ),
        ];
        let first_live = cur.as_ref().and_then(|cb| {
            war.sessions
                .iter()
                .find(|s| s.codebase == cb.id && s.state.is_live())
                .map(|s| s.id.clone())
        });
        let open_btn = navb("tr-open", "OPEN SELECTED >", false)
            .border_1()
            .border_color(c(tac::INK, 1.))
            .text_color(c(tac::INK, 1.))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(id) = &first_live {
                    cx.emit(TacEvent::OpenSession(id.clone()));
                }
            }))
            .into_any_element();

        let mut list = scroll("tr-list");
        for (k, cb) in war.codebases.iter().enumerate() {
            let id = cb.id.clone();
            let w = waits(&cb.id);
            list = list.child(
                trow(
                    format!("tr-row-{k}"),
                    if w > 0 { tac::WHITE } else { tac::INK },
                    &cb.id,
                    &format!("{} · {}", cb.branch, dots(&cb.path, 36)),
                    &format!("{} LIVE", live(&cb.id)),
                    if w > 0 { tac::WHITE } else { tac::LABEL },
                    cur.as_ref().is_some_and(|x| x.id == cb.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        let left = panel(format!("SEC-01 / CODEBASES · {}", war.codebases.len()))
            .child(list)
            .child(foot(format!(
                "{} LIVE SESSIONS · {} WAIT FOR YOU. SELECT A ROW FOR THE PARTS.",
                counts.live, counts.needs_you
            )));

        let mut right = panel("SEC-02 / CODEBASE");
        if let Some(cb) = &cur {
            right = right
                .child(section(cb.id.to_uppercase()))
                .child(label(format!(
                    "{} · {} WORKTREES · {}",
                    cb.branch.to_uppercase(),
                    cb.worktrees,
                    cb.daemon.to_uppercase()
                )))
                .child(code(cb.path.clone()))
                .child(label(format!(
                    "LANGUAGES · {}",
                    cb.languages.join(", ").to_uppercase()
                )));
            let mut body = scroll("tr-detail").gap(px(8.));
            body = body.child(label(format!("PARTS · {}", cb.parts.len())));
            for p in cb.parts.iter().take(8) {
                body = body.child(entry(format!("{} · {}", p.name, p.paths.join(", "))));
            }
            let sessions: Vec<&Pane> = war
                .sessions
                .iter()
                .filter(|s| s.codebase == cb.id && s.state.is_live())
                .collect();
            body = body.child(label(format!("LIVE SESSIONS · {}", sessions.len())).mt(px(12.)));
            for (k, s) in sessions.iter().enumerate() {
                let id = s.id.clone();
                let hot = s.ask.is_some();
                body = body.child(
                    trow(
                        format!("tr-s-{k}"),
                        if hot { tac::WHITE } else { tac::INK },
                        &dots(&s.title, 40),
                        &s.tail
                            .iter()
                            .rev()
                            .find(|l| l.kind == aivory_protocol::LineKind::Tool)
                            .map_or_else(
                                || s.provider.to_uppercase(),
                                |l| {
                                    dots(
                                        &format!(
                                            "{} {}",
                                            l.label.clone().unwrap_or_default(),
                                            l.text
                                        ),
                                        48,
                                    )
                                },
                            ),
                        "OPEN >",
                        tac::LABEL,
                        false,
                    )
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(TacEvent::OpenSession(id.clone()))),
                    ),
                );
            }
            right = right.child(body);
        }

        frame(Frame {
            title: "REPOS".into(),
            kicker: "THE CODEBASES THE FLEET WORKS IN".into(),
            stats,
            menu: menu_end(Vec::new(), Some(open_btn), full),
            body: console(left, right),
        })
    }
}

/// A small picture of a look: its page, bar, two cards and text lines in its own colors.
fn look_thumb(look: aivory_protocol::Look) -> Div {
    use aivory_protocol::Look;
    let t = crate::look::tokens(look, aivory_protocol::Theme::Light, false);
    let bar = |w: f32, rgb: u32| div().flex_none().h(px(6.)).w(px(w)).bg(c(rgb, 1.));
    let lines = || {
        div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(bar(90., t.ink))
            .child(bar(130., t.ink_2))
            .child(bar(70., t.muted))
            .child(bar(110., t.ink_3))
            .child(bar(100., t.ink_2))
    };
    let pane = |alpha: f32| {
        div()
            .flex_1()
            .h_full()
            .p(px(10.))
            .bg(c(t.card, alpha))
            .border_1()
            .border_color(c(t.ink_3, 0.7))
    };
    let dotp = |x: f32, y: f32, d: f32, rgb: u32| {
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .size(px(d))
            .rounded_full()
            .bg(c(rgb, 1.))
    };
    let ring = |d: f32| {
        div()
            .size(px(d))
            .rounded_full()
            .border_1()
            .border_color(c(t.ink_3, 1.))
            .flex()
            .items_center()
            .justify_center()
    };
    let titlebar = |rgb: u32| {
        div()
            .h(px(20.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .bg(c(rgb, 1.))
            .child(div().size(px(8.)).bg(c(t.logo, 1.)))
            .child(bar(60., t.ink_2))
    };
    let scene: Div = match look {
        Look::Tactical => div()
            .flex_1()
            .flex()
            .gap(px(10.))
            .child(
                pane(1.).flex().items_center().justify_center().child(
                    ring(170.)
                        .relative()
                        .child(ring(110.).child(ring(50.)))
                        .child(dotp(40., 32., 9., t.q))
                        .child(dotp(118., 88., 9., t.ok))
                        .child(dotp(66., 120., 9., t.work)),
                ),
            )
            .child(pane(1.).child(lines()).child(
                div().mt(px(14.)).flex().flex_col().gap(px(8.)).children(
                    [t.q, t.work, t.ok, t.work].into_iter().map(|rgb| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().size(px(8.)).rounded_full().bg(c(rgb, 1.)))
                            .child(bar(120., t.ink_3))
                    }),
                ),
            )),
        Look::Command => div()
            .flex_1()
            .flex()
            .gap(px(10.))
            .child(
                pane(1.)
                    .relative()
                    .child(dotp(30., 30., 22., t.work))
                    .child(dotp(110., 60., 22., t.ok))
                    .child(dotp(70., 130., 22., t.q))
                    .child(dotp(190., 110., 22., t.e)),
            )
            .child(
                div()
                    .w(px(120.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(pane(1.).child(lines()))
                    .child(div().flex_none().h(px(40.)).bg(c(t.work, 1.))),
            ),
        Look::Operator => div().flex_1().flex().gap(px(8.)).children((0..6).map(|i| {
            div()
                .flex_1()
                .h_full()
                .bg(c(t.card, 1.))
                .border_1()
                .border_color(c(t.ink_3, 0.7))
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(6.))
                .children((0..(4 + i % 3)).map(|k| {
                    bar(
                        6. + ((i * 7 + k * 5) % 5) as f32 * 6.,
                        if k == 0 { t.ok } else { t.work },
                    )
                }))
        })),
        Look::Neon => div()
            .flex_1()
            .flex()
            .items_end()
            .gap(px(6.))
            .bg(c(t.card, 1.))
            .px(px(10.))
            .children(
                [70., 120., 90., 160., 110., 140., 80., 130., 100., 150.]
                    .into_iter()
                    .enumerate()
                    .map(|(i, h)| {
                        div()
                            .flex_1()
                            .h(px(h))
                            .bg(c(t.track, 1.))
                            .border_1()
                            .border_color(c(if i % 3 == 0 { t.work } else { t.q }, 1.))
                    }),
            ),
        Look::Navigator => div()
            .flex_1()
            .flex()
            .gap(px(8.))
            .child(
                div()
                    .w(px(44.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children((0..6).map(|_| {
                        div()
                            .size(px(36.))
                            .bg(c(t.card, 1.))
                            .border_1()
                            .border_color(c(t.ink_3, 1.))
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(c(t.card, 1.))
                    .border_1()
                    .border_color(c(t.ink_3, 1.))
                    .child(titlebar(t.work))
                    .child(div().p(px(10.)).child(lines())),
            ),
        Look::Holo => div()
            .flex_1()
            .flex()
            .gap(px(14.))
            .p(px(10.))
            .bg(c(t.work, 0.35))
            .child(pane(0.45).rounded(px(12.)).child(lines()))
            .child(pane(0.45).rounded(px(12.)).child(lines())),
        Look::Bridge => div()
            .flex_1()
            .flex()
            .gap(px(8.))
            .child(
                div()
                    .w(px(70.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().flex_1().bg(c(t.work, 1.)).rounded_tl(px(24.)))
                    .child(div().h(px(30.)).bg(c(t.q, 1.)))
                    .child(div().h(px(30.)).bg(c(t.e, 1.)).rounded_bl(px(24.))),
            )
            .child(pane(1.).rounded(px(12.)).child(lines())),
        _ => div()
            .flex_1()
            .flex()
            .gap(px(10.))
            .child(pane(1.).child(lines()))
            .child(pane(1.).child(lines())),
    };
    div()
        .flex_none()
        .h(px(280.))
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(10.))
        .bg(c(t.page, 1.))
        .border_1()
        .border_color(c(tac::DIM, 1.))
        .child(titlebar(t.track))
        .child(scene)
        .child(
            div().flex_none().flex().gap(px(6.)).children(
                [t.page, t.card, t.ink, t.work, t.q, t.e, t.ok]
                    .into_iter()
                    .map(|rgb| {
                        div()
                            .flex_1()
                            .h(px(16.))
                            .bg(c(rgb, 1.))
                            .border_1()
                            .border_color(c(tac::DIM, 1.))
                    }),
            ),
        )
}

/// One short line about a look.
fn look_line(look: aivory_protocol::Look) -> &'static str {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => "calm default, white page",
        Look::Bridge => "rounded frame, numbered tabs",
        Look::Operator => "green rain, instrument screens",
        Look::Neon => "dark city, glowing readouts",
        Look::Navigator => "grey desktop, toolchest bar",
        Look::Holo => "see-through glass panels",
        Look::Tactical => "military terminal, calm radar",
        Look::Command => "unit map and command card",
    }
}

/// One paragraph about a look.
fn look_text(look: aivory_protocol::Look) -> &'static str {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => {
            "The calm default: white page, one rail, round pills. Best for long work days."
        }
        Look::Bridge => {
            "Rounded frame segments and numbered tabs in warm panel colors. The panels group the work by island."
        }
        Look::Operator => {
            "Green rain on black, F-keys in the top line, and instrument screens that appear on focus."
        }
        Look::Neon => "Neon on dark: a header with tabs, hard edges and glowing readouts.",
        Look::Navigator => {
            "A grey desktop with a toolchest bar and windows, as on an old workstation."
        }
        Look::Holo => {
            "See-through glass panels on a deep blue field. Calm, with one strand per session."
        }
        Look::Tactical => {
            "A military terminal: a calm radar of your sessions, amber when one needs you, cyan for live work. Bearing is the codebase, range is the time since the last move."
        }
        Look::Command => "A command console: a map of units per codebase and a command sidebar.",
    }
}

// ============================================================================ Settings ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Looks,
    Autopilot,
    Providers,
}

pub struct TacticalSettings {
    model: Entity<Model>,
    section: Section,
    /// The look whose detail shows; the look in use when `None`.
    look: Option<aivory_protocol::Look>,
    _obs: gpui::Subscription,
}

impl EventEmitter<TacEvent> for TacticalSettings {}

impl TacticalSettings {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            section: Section::Looks,
            look: None,
            _obs: obs,
        }
    }
}

impl Render for TacticalSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let (auto, max, park) = war.as_ref().map_or((false, 0, 0), |w| {
            (
                w.autopilot.on,
                w.autopilot.max_running,
                w.autopilot.park_after_min,
            )
        });
        let stats = vec![
            (
                crate::look::name(crate::theme::tk().look).to_uppercase(),
                "LOOK".into(),
                tac::INK,
            ),
            (
                if auto { "ON" } else { "OFF" }.to_string(),
                "AUTOPILOT".into(),
                tac::INK,
            ),
            (max.to_string(), "RUN AT MOST".into(), tac::INK),
        ];
        let mut menu: Vec<AnyElement> = Vec::new();
        for (s, name) in [
            (Section::Looks, "LOOKS"),
            (Section::Autopilot, "AUTOPILOT"),
            (Section::Providers, "PROVIDERS"),
        ] {
            menu.push(
                navb(format!("ts-tab-{name}"), name, self.section == s)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.section = s;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        // "OTHER": the full settings screen (providers, devices, machines, notifications).
        menu.push(
            navb("ts-full", "FULL SETTINGS >", false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(TacEvent::Full)))
                .into_any_element(),
        );

        let mut left = panel(match self.section {
            Section::Looks => "SEC-01 / LOOKS · CLICK TO SWITCH",
            Section::Autopilot => "SEC-01 / AUTOPILOT",
            Section::Providers => "SEC-01 / USAGE WINDOWS",
        });
        let mut right = panel("SEC-02 / DETAIL");
        match self.section {
            Section::Looks => {
                let mut list = scroll("ts-list");
                let here = crate::theme::tk().look;
                let shown = self.look.unwrap_or(here);
                for (k, (look, name)) in crate::look::LOOKS.into_iter().enumerate() {
                    list = list.child(
                        trow(
                            format!("ts-look-{k}"),
                            tac::INK,
                            name,
                            look_line(look),
                            if look == here { "IN USE" } else { "" },
                            tac::INK,
                            look == shown,
                        )
                        .min_h(px(56.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.look = Some(look);
                            cx.notify();
                        })),
                    );
                }
                left = left.child(list);
                let name = crate::look::name(shown).to_uppercase();
                let on = shown == here;
                right = right
                    .child(section(name.clone()))
                    .child(look_thumb(shown))
                    .child(entry(look_text(shown)))
                    .child(entry(
                        "A look replaces the frame, the colors and the fonts of every screen. The choice is saved in the settings of the daemon.",
                    ));
                if let Some(w) = war.as_ref() {
                    right = right.child(label(format!(
                        "AUTOPILOT · {} · UP TO {} AT ONCE · PARK AFTER {} MIN",
                        if w.autopilot.on { "ON" } else { "OFF" },
                        w.autopilot.max_running,
                        w.autopilot.park_after_min
                    )));
                    for u in w.usage.iter().take(3) {
                        right = right.child(label(format!(
                            "USAGE · {} {} · {}%",
                            u.provider.to_uppercase(),
                            u.label.to_uppercase(),
                            u.pct
                        )));
                    }
                }
                if on {
                    right = right.child(label("THIS LOOK IS IN USE"));
                } else {
                    right = right.child(
                        tac::order("ts-switch", "APPLY", Btn::On)
                            .self_start()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                act(&this.model, Action::SetLook(shown), cx);
                                this.look = None;
                            })),
                    );
                }
            }
            Section::Autopilot => {
                let mut row = div().flex_none().flex().gap(px(8.));
                row = row
                    .child(
                        tac::order(
                            "ts-auto",
                            if auto {
                                "AUTOPILOT ON"
                            } else {
                                "AUTOPILOT OFF"
                            },
                            if auto { Btn::On } else { Btn::Plain },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(
                                &this.model,
                                Action::Autopilot(AutopilotPatch {
                                    on: Some(!auto),
                                    ..AutopilotPatch::default()
                                }),
                                cx,
                            );
                        })),
                    )
                    .child(
                        tac::order("ts-less", "FEWER", Btn::Plain).on_click(cx.listener(
                            move |this, _, _, cx| {
                                act(
                                    &this.model,
                                    Action::Autopilot(AutopilotPatch {
                                        max_running: Some(max.saturating_sub(1).max(1)),
                                        ..AutopilotPatch::default()
                                    }),
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(
                        tac::order("ts-more", "MORE", Btn::Plain).on_click(cx.listener(
                            move |this, _, _, cx| {
                                act(
                                    &this.model,
                                    Action::Autopilot(AutopilotPatch {
                                        max_running: Some((max + 1).min(64)),
                                        ..AutopilotPatch::default()
                                    }),
                                    cx,
                                );
                            },
                        )),
                    );
                left = left.child(row);
                right = right
                    .child(section(format!("UP TO {max} SESSIONS RUN AT ONCE")))
                    .child(entry(format!(
                        "Autopilot is {}. A session parks after {park} minutes without work.",
                        if auto { "on" } else { "off" }
                    )))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(label("PARK AFTER"))
                            .child(tac::order("ts-park-less", "- 5 MIN", Btn::Plain).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            park_after_min: Some(park.saturating_sub(5).max(5)),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                }),
                            ))
                            .child(tac::order("ts-park-more", "+ 5 MIN", Btn::Plain).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            park_after_min: Some((park + 5).min(240)),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                }),
                            )),
                    );
            }
            Section::Providers => {
                let mut list = scroll("ts-list");
                if let Some(w) = &war {
                    for (k, u) in w.usage.iter().enumerate() {
                        let hot = u.pct >= 80;
                        list = list.child(trow(
                            format!("ts-use-{k}"),
                            if hot { tac::WHITE } else { tac::INK },
                            &format!("{} {}", u.provider, u.label),
                            if u.estimate {
                                "ESTIMATE"
                            } else {
                                "FROM THE PROVIDER"
                            },
                            &format!("{}%", u.pct),
                            if hot { tac::WHITE } else { tac::INK },
                            false,
                        ));
                    }
                }
                left = left.child(list);
                right = right.child(section("PROVIDERS AND MODELS"));
                let providers = self.model.read(cx).providers.clone();
                if providers.is_empty() {
                    right = right.child(entry("No provider is installed."));
                }
                let mut pl = scroll("ts-prov").gap(px(10.)).pb(px(24.));
                for (pi, (id, models)) in providers.into_iter().enumerate() {
                    let mut row = div().flex().flex_wrap().gap(px(8.));
                    for (mi, m) in models.into_iter().enumerate() {
                        let (provider, model) = (id.clone(), m.clone());
                        row = row.child(
                            tac::order(format!("ts-model-{pi}-{mi}"), &m, Btn::Plain).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::SetDefaultModel {
                                            provider: provider.clone(),
                                            model: model.clone(),
                                        },
                                        cx,
                                    );
                                }),
                            ),
                        );
                    }
                    pl = pl
                        .child(label(format!(
                            "{} · CLICK A MODEL FOR THE DEFAULT",
                            id.to_uppercase()
                        )))
                        .child(row);
                }
                right = right.child(pl).child(label(
                    "Change the effort and the permission mode in the full settings.",
                ));
            }
        }

        frame(Frame {
            title: "SETUP".into(),
            kicker: "LOOK, AUTOPILOT AND PROVIDERS".into(),
            stats,
            menu,
            body: console(left, right),
        })
    }
}
