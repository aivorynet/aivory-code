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

/// An item that needs the owner is white; the rest is green.
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
        let open_btn = navb("ti-open", "OPEN SESSION >", false)
            .border_1()
            .border_color(c(tac::INK, 1.))
            .text_color(c(tac::INK, 1.))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(id) = &open {
                    cx.emit(TacEvent::OpenSession(id.clone()));
                }
            }))
            .into_any_element();
        let full = navb("ti-full", "ALL FUNCTIONS >", false)
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
        let left = panel(format!("QUEUE · {} ITEMS", items.len())).child(list);

        // The detail.
        let mut right = panel("DECISION");
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
                let n = it.options.len();
                for (k, o) in it.options.iter().enumerate() {
                    let (id, opt) = (it.id.clone(), o.clone());
                    let kind = if k == 0 {
                        Btn::On
                    } else if k + 1 == n && n > 1 {
                        Btn::Dim
                    } else {
                        Btn::Plain
                    };
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
            title: "INBOX",
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
        let open_btn = navb("tt-open", "OPEN SESSION >", false)
            .border_1()
            .border_color(c(tac::INK, 1.))
            .text_color(c(tac::INK, 1.))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(id) = &open {
                    cx.emit(TacEvent::OpenSession(id.clone()));
                }
            }))
            .into_any_element();
        let full = navb("tt-full", "ALL FUNCTIONS >", false)
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
                    &format!("STEP {}", (t.step + 1).min(5)),
                    if hot { tac::WHITE } else { tac::LABEL },
                    cur.as_ref().is_some_and(|x| x.id == t.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        let left = panel(format!("TASKS · {}", tasks.len())).child(list);

        let mut right = panel("TASK");
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
        } else {
            right = right.child(entry("Pick a task in the list."));
        }

        frame(Frame {
            title: "TASKS",
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
    /// The first codebase of the list page on view.
    start: usize,
    _obs: gpui::Subscription,
}

impl EventEmitter<TacEvent> for TacticalRepos {}

impl TacticalRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: None,
            start: 0,
            _obs: obs,
        }
    }
}

impl Render for TacticalRepos {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let full = navb("tr-full", "ALL FUNCTIONS >", false)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(TacEvent::Full)))
            .into_any_element();
        let Some(war) = self.model.read(cx).war.clone() else {
            return frame(Frame {
                title: "REPOS",
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
        let open_btn = navb("tr-open", "OPEN SESSION >", false)
            .border_1()
            .border_color(c(tac::INK, 1.))
            .text_color(c(tac::INK, 1.))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(id) = &first_live {
                    cx.emit(TacEvent::OpenSession(id.clone()));
                }
            }))
            .into_any_element();

        // Whole rows only: all of them when they fit, else a page and a "+N more" row.
        let room = f32::from(window.viewport_size().height) - 44. - 340.;
        let cap = ((room / 52.) as usize).max(3);
        let total = war.codebases.len();
        let (from, take) = if total <= cap {
            (0, total)
        } else {
            (self.start.min(total - 1), cap - 1)
        };
        let mut list = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_hidden();
        for (k, cb) in war.codebases.iter().enumerate().skip(from).take(take) {
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
        if total > cap {
            let rest = total.saturating_sub(from + take);
            let next = if rest > 0 { from + take } else { 0 };
            list = list.child(
                navb(
                    "tr-more",
                    &if rest > 0 {
                        format!("+{rest} MORE >")
                    } else {
                        "< BACK TO THE FIRST".to_string()
                    },
                    false,
                )
                .h(px(40.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start = next;
                    cx.notify();
                })),
            );
        }
        let left = panel(format!("CODEBASES · {}", war.codebases.len())).child(list);

        let mut right = panel("CODEBASE");
        if let Some(cb) = &cur {
            right = right
                .child(section(cb.id.to_uppercase()))
                .child(label(format!(
                    "{} · {} WORKTREES · {}",
                    cb.branch.to_uppercase(),
                    cb.worktrees,
                    cb.daemon.to_uppercase()
                )))
                .child(code(cb.path.clone()));
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
                        &s.provider.to_uppercase(),
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
            title: "REPOS",
            kicker: "THE CODEBASES THE FLEET WORKS IN".into(),
            stats,
            menu: menu_end(Vec::new(), Some(open_btn), full),
            body: console(left, right),
        })
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
            "A phosphor radar: bearing is the codebase, range is the time since the last move."
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
        let full = navb("ts-full", "ALL SETTINGS >", false)
            .border_1()
            .border_color(c(tac::INK, 1.))
            .text_color(c(tac::INK, 1.))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(TacEvent::Full)))
            .into_any_element();

        let mut left = panel(match self.section {
            Section::Looks => "LOOKS · CLICK TO SWITCH",
            Section::Autopilot => "AUTOPILOT",
            Section::Providers => "USAGE WINDOWS",
        });
        let mut right = panel("DETAIL");
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
                            "",
                            if look == here { "IN USE" } else { "" },
                            tac::INK,
                            look == shown,
                        )
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
                    .child(label(if on { "STATUS: ON" } else { "STATUS: OFF" }).text_color(c(
                        if on { tac::INK } else { tac::LABEL },
                        1.,
                    )))
                    .child(entry(look_text(shown)))
                    .child(entry(
                        "A look replaces the frame, the colors and the fonts of every screen. The choice is saved in the settings of the daemon.",
                    ));
                if !on {
                    right = right.child(
                        tac::order("ts-switch", &format!("SWITCH TO {name}"), Btn::On)
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
                    )));
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
                right = right.child(section("PROVIDERS AND MODELS")).child(entry(
                    "Models, effort and permission mode per provider are set in all settings.",
                ));
            }
        }

        frame(Frame {
            title: "SETUP",
            kicker: "LOOK, AUTOPILOT AND PROVIDERS".into(),
            stats,
            menu: menu_end(menu, None, full),
            body: console(left, right),
        })
    }
}
