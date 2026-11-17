//! The Neon screens Inbox, Tasks, Repos and Settings, and the Neon frame around
//! Focus, Lens and the full Settings. Each list screen has the same grammar as the war room:
//! a neon screen title with a glow bar, a menu of tabs under it, a dim list on the left and
//! one detail panel on the right. The detail panel is the bright one when something waits
//! for you; the list dims and lights up while the mouse is over it. Every control calls the
//! daemon through `actions.rs`, as in the Bridge screens.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{AutopilotPatch, InboxListParams, TaskListParams, method};
use aivory_protocol::{
    InboxFilter, InboxItem, InboxKind, InboxList, Pane, SessionState, StepState, Task, TaskColumn,
    TaskList,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FontWeight, Subscription, Task as GpuiTask,
    Window, div, px, relative, rgb, rgba,
};

use super::neon_kit::*;
use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::bridge_screens::{TaskAct, task_act};
use crate::model::Model;

/// What a screen asks the shell to do.
#[derive(Debug, Clone)]
pub(super) enum NightEvent {
    /// Open the pairing card of the full settings screen.
    PairPhone,
    OpenSession(String),
    /// The full Settings screen.
    AllSettings,
}

/// The list screens and their event subscriptions: made when they first show, then kept.
#[derive(Default)]
pub(super) struct NightScreens {
    inbox: Option<(Entity<NightInbox>, Subscription)>,
    tasks: Option<(Entity<NightTasks>, Subscription)>,
    repos: Option<(Entity<NightRepos>, Subscription)>,
    settings: Option<(Entity<NightSettings>, Subscription)>,
    focus: Option<(Entity<super::neon_focus::NightFocus>, Subscription)>,
    lens: Option<(Entity<super::neon_lens::NightLens>, Subscription)>,
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

/// `now` or `6 min` for a wait that began at `since`.
fn minutes(now: i64, since: i64) -> String {
    let m = ((now - since) / 60_000).max(0);
    if m < 1 {
        "now".to_string()
    } else {
        format!("{m} min")
    }
}

/// The color of a session by state: waits for you, fault, working, idle.
fn pane_color(p: &Pane) -> u32 {
    match (&p.ask, p.state) {
        (_, SessionState::Error) => YELLOW,
        (Some(_), _) => RED,
        (None, SessionState::Idle) => MIST,
        (None, _) => CYAN,
    }
}

/// A scrolling column that fills the rest of a panel.
fn scroll_list(id: &'static str, rows: Vec<AnyElement>) -> impl IntoElement {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .pb(px(16.))
        .children(rows)
}

/// The text of an empty list.
fn nothing(text: &str) -> AnyElement {
    div()
        .px(px(24.))
        .py(px(16.))
        .child(label(text.to_string(), MIST).text_size(crate::ui::ts(T_ENTRY)))
        .into_any_element()
}

/// The left panel of a list screen: dim, a section title, then the rows.
fn list_panel(
    group: &'static str,
    (title, color): (&str, u32),
    note: String,
    rows: Vec<AnyElement>,
) -> Div {
    frame(color, group, false)
        .w(px(540.))
        .flex_none()
        .h_full()
        .child(section(title, color, Some(note)))
        .child(scroll_list(group, rows))
}

/// The side tabs inside a detail panel.
fn side_tabs(tabs: Vec<AnyElement>) -> Div {
    div()
        .flex_none()
        .px(px(24.))
        .flex()
        .border_b_1()
        .border_color(gpui::rgba(0x8C96BC30))
        .children(tabs)
}

/// The scrolling body of a detail panel: wide padding, a calm gap.
fn detail_body(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .px(px(24.))
        .pt(px(20.))
        .pb(px(56.))
        .flex()
        .flex_col()
        .gap(px(16.))
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

pub(super) struct NightInbox {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    tab: InboxTab,
    side: Side,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: Subscription,
}

impl EventEmitter<NightEvent> for NightInbox {}

impl NightInbox {
    pub(super) fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
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

    /// The items of the tab.
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

fn kind_color(k: InboxKind) -> u32 {
    match k {
        InboxKind::Error => YELLOW,
        _ => RED,
    }
}

impl Render for NightInbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .map_or("none".to_string(), |s| minutes(now, s));
        let sub = format!(
            "{all} items wait for you ({} need an answer) · oldest {oldest}",
            count(InboxFilter::Answers)
        );

        let mut menu: Vec<AnyElement> = Vec::new();
        for (t, name, n) in [
            (InboxTab::All, "All", all),
            (InboxTab::Waiting, "Waiting", count(InboxFilter::Answers)),
            (InboxTab::Errors, "Errors", count(InboxFilter::Errors)),
            (InboxTab::Reviews, "Reviews", count(InboxFilter::Reviews)),
            (InboxTab::Done, "Done", done),
        ] {
            menu.push(
                menu_tab(
                    format!("ni-tab-{name}"),
                    name,
                    Some(n),
                    YELLOW,
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

        // The left list: one row per item, the longest wait first as the model gives them.
        let mut rows: Vec<AnyElement> = Vec::new();
        for (k, it) in items.iter().enumerate() {
            let id = it.id.clone();
            let c = kind_color(it.kind);
            rows.push(
                row(
                    format!("ni-row-{k}"),
                    &format!("{:02}", k + 1),
                    c,
                    (
                        &it.title,
                        &format!("{} · {}", it.codebase, it.kind.label()),
                        &minutes(now, it.since),
                    ),
                    cur.as_ref().is_some_and(|x| x.id == it.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(nothing("Nothing here"));
        }
        let rows = cap_rows(rows, fit_rows(window, 350., 54.), 54.);
        let left = list_panel(
            "ni-list",
            ("Queue", YELLOW),
            format!("{} in this tab", items.len()),
            rows,
        );

        // The right panel: the decision, bright while it waits for you.
        let waits = cur.as_ref().is_some_and(|i| i.result.is_none());
        let color = cur.as_ref().map_or(CYAN, |i| kind_color(i.kind));
        let mut right = frame(color, "ni-detail", waits)
            .flex_1()
            .min_w(px(0.))
            .h_full();
        if let Some(it) = &cur {
            right = right.child(section(
                it.kind.heading(),
                color,
                Some(match &it.result {
                    Some(r) => format!("DONE · {r}"),
                    None => format!("WAITS {}", minutes(now, it.since).to_uppercase()),
                }),
            ));
            let mut body = detail_body("ni-body").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(crate::ui::ts(T_SECTION))
                            .line_height(crate::ui::ts(26.))
                            .text_color(rgb(0xFFFFFF))
                            .child(it.title.clone()),
                    )
                    .child(label(
                        format!(
                            "{} · {} / {}",
                            it.agents
                                .first()
                                .map_or(String::new(), |a| a.provider.clone()),
                            it.codebase,
                            it.part
                        ),
                        MIST,
                    )),
            );
            if !it.preview.is_empty() {
                body = body.child(entry(it.preview.clone()));
            }
            if it.result.is_none() {
                let mut buttons = div().flex().flex_wrap().gap(px(12.));
                for (k, o) in it.options.iter().enumerate() {
                    let (id, opt) = (it.id.clone(), o.clone());
                    let c = if k == 0 {
                        YELLOW
                    } else if o.to_lowercase().contains("deny") {
                        RED
                    } else {
                        MIST
                    };
                    buttons = buttons.child(
                        button(format!("ni-opt-{k}"), &format!("{} {o}", k + 1), c, k == 0)
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
                body = body.child(buttons);
            }
            match self.side {
                Side::Details => {
                    // The preview above the buttons already shows the same text.
                    if it.text.trim() != it.preview.trim() && !it.text.trim().is_empty() {
                        body = body.child(label("COMMAND OR QUESTION", MIST));
                        body = body.child(code(&it.text, color));
                    }
                    if !it.steps.is_empty() {
                        body = body.child(label("PLAN", MIST));
                        for (k, s) in it.steps.iter().take(6).enumerate() {
                            body = body.child(entry(format!("{}  {s}", k + 1)));
                        }
                    }
                    if !it.tail.is_empty() {
                        body = body.child(label("LATEST IN THE CHAT", MIST));
                        body = body.child(super::neon_focus::chat_column(&it.tail, 10));
                    }
                }
                Side::Files => {
                    body = body.child(label(format!("FILES CHANGED · {}", it.files.len()), MIST));
                    for f in it.files.iter().take(12) {
                        body = body.child(code(f, MIST));
                    }
                }
                Side::Chat => {
                    body = body.child(label("LAST ROWS OF THE CHAT", MIST));
                    body = body.child(super::neon_focus::chat_column(&it.tail, 24));
                }
            }
            let open = it.session.clone();
            let mut tabs: Vec<AnyElement> = Vec::new();
            for (s, name) in [
                (Side::Details, "Details"),
                (Side::Files, "Files"),
                (Side::Chat, "Chat"),
            ] {
                tabs.push(
                    menu_tab(format!("ni-side-{name}"), name, None, CYAN, self.side == s)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            tabs.push(div().flex_1().into_any_element());
            tabs.push(
                menu_tab("ni-open", "Open session ›", None, CYAN, false)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(NightEvent::OpenSession(open.clone()));
                    }))
                    .into_any_element(),
            );
            right = right.child(side_tabs(tabs)).child(body);
        } else {
            right = right
                .child(section("Inbox", CYAN, None))
                .child(nothing("Nothing waits for you."));
        }
        screen(
            "Inbox",
            sub,
            YELLOW,
            (menu, Vec::new()),
            div()
                .size_full()
                .flex()
                .gap(px(32.))
                .child(left)
                .child(right)
                .into_any_element(),
        )
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

pub(super) struct NightTasks {
    model: Entity<Model>,
    data: Option<Rc<TaskList>>,
    tab: TaskTab,
    side: TaskSide,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
}

impl EventEmitter<NightEvent> for NightTasks {}

impl NightTasks {
    pub(super) fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let tick = ticker(cx, Duration::from_secs(5), |t: &mut Self, cx| t.refresh(cx));
        let mut this = Self {
            model,
            data: None,
            tab: TaskTab::Running,
            side: TaskSide::Plan,
            selected: None,
            busy: false,
            _tick: tick,
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

fn step_color(s: StepState) -> u32 {
    match s {
        StepState::Done => MIST,
        StepState::Working => CYAN,
        StepState::Waiting => RED,
        StepState::NotStarted => DIM,
    }
}

fn step_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "DONE",
        StepState::Working => "WORKING",
        StepState::Waiting => "WAITS FOR YOU",
        StepState::NotStarted => "QUEUED",
    }
}

impl Render for NightTasks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (autopilot, queued_n) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map_or((false, 0), |w| (w.autopilot.on, w.queued));
        let n = |t| self.of(t).len();
        let mut menu: Vec<AnyElement> = Vec::new();
        for (t, name) in [
            (TaskTab::Queued, "Queued"),
            (TaskTab::Running, "Running"),
            (TaskTab::Review, "Review"),
            (TaskTab::Done, "Done"),
        ] {
            menu.push(
                menu_tab(
                    format!("nt-tab-{name}"),
                    name,
                    Some(n(t)),
                    YELLOW,
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
        let actions = vec![
            label(format!("{queued_n} queued for autopilot"), MIST)
                .text_size(crate::ui::ts(T_ENTRY))
                .into_any_element(),
            button(
                "nt-autopilot",
                if autopilot {
                    "Autopilot: on, click to stop"
                } else {
                    "Autopilot: off, click to start"
                },
                CYAN,
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
        ];
        let tasks = self.of(self.tab);
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .or(tasks.first())
            .cloned();
        let sub = format!(
            "{} running · {} in review · {} queued",
            n(TaskTab::Running),
            n(TaskTab::Review),
            n(TaskTab::Queued)
        );

        let mut rows: Vec<AnyElement> = Vec::new();
        for (k, t) in tasks.iter().enumerate() {
            let id = t.id.clone();
            let c = if t.waiting_for.is_some() { RED } else { CYAN };
            rows.push(
                row(
                    format!("nt-row-{k}"),
                    &format!("{:02}", k + 1),
                    c,
                    (
                        &t.title,
                        &format!(
                            "{} · {}",
                            t.agents.first().cloned().unwrap_or_default(),
                            t.codebase
                        ),
                        &format!("step {}", (t.step + 1).min(5)),
                    ),
                    cur.as_ref().is_some_and(|x| x.id == t.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(nothing("No task here"));
        }
        let mut rows = cap_rows(rows, fit_rows(window, 350., 54.), 54.);
        // The totals fill the room under the list.
        let all: Vec<Task> = [
            TaskTab::Queued,
            TaskTab::Running,
            TaskTab::Review,
            TaskTab::Done,
        ]
        .into_iter()
        .flat_map(|t| self.of(t))
        .collect();
        let spend: f64 = all.iter().map(|t| t.cost).sum();
        let asking = all.iter().filter(|t| t.waiting_for.is_some()).count();
        rows.push(
            div()
                .flex_none()
                .px(px(24.))
                .pt(px(24.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(label("ALL TASKS", MIST))
                .child(entry(format!("{} tasks · ${spend:.2} spent", all.len())))
                .child(label(
                    if asking > 0 {
                        format!("{asking} wait for your answer")
                    } else {
                        "No task waits for you".to_string()
                    },
                    if asking > 0 { RED } else { CYAN },
                ))
                .child(super::neon_focus::bar(
                    if all.is_empty() {
                        0.
                    } else {
                        1. - asking as f32 / all.len() as f32
                    },
                    CYAN,
                    4.,
                ))
                .into_any_element(),
        );
        let left = list_panel(
            "nt-list",
            ("Tasks", YELLOW),
            format!("{} in this tab", tasks.len()),
            rows,
        );

        let waits = cur.as_ref().is_some_and(|t| t.waiting_for.is_some());
        let mut right = frame(if waits { RED } else { CYAN }, "nt-detail", waits)
            .flex_1()
            .min_w(px(0.))
            .h_full();
        if let Some(t) = &cur {
            let war = self.model.read(cx).war.clone();
            let pane: Option<Pane> = war.as_ref().and_then(|w| {
                t.sessions
                    .iter()
                    .find_map(|s| w.sessions.iter().find(|p| p.id == s.session))
                    .cloned()
            });
            right = right.child(section(
                "Task",
                if waits { RED } else { CYAN },
                Some(format!("${:.2}", t.cost)),
            ));
            let mut body = detail_body("nt-body").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(crate::ui::ts(T_SECTION))
                            .line_height(crate::ui::ts(26.))
                            .text_color(rgb(0xFFFFFF))
                            .child(t.title.clone()),
                    )
                    .child(label(
                        format!(
                            "{} · {} / {}",
                            t.agents.first().cloned().unwrap_or_default(),
                            t.codebase,
                            t.part
                        ),
                        MIST,
                    )),
            );
            match self.side {
                TaskSide::Plan => {
                    body = body.child(label(format!("PLAN · {} STEPS", t.steps.len()), MIST));
                    let mut strip = div().flex_none().flex().gap(px(10.));
                    for (k, s) in t.steps.iter().enumerate() {
                        strip = strip.child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .px(px(10.))
                                .py(px(6.))
                                .border_t_2()
                                .border_color(rgb(step_color(s.state)))
                                .bg(gpui::rgba(0x8C96BC0F))
                                .child(
                                    div()
                                        .font_weight(FontWeight::BOLD)
                                        .text_size(crate::ui::ts(T_LABEL))
                                        .text_color(rgb(step_color(s.state)))
                                        .child(format!("{:02} · {}", k + 1, step_word(s.state))),
                                )
                                .child(entry(s.name.clone())),
                        );
                    }
                    body = body.child(strip);
                    // The detail of the current step, the chat of its session, the files.
                    if let Some((k, s)) =
                        t.steps.iter().enumerate().find(|(_, s)| {
                            matches!(s.state, StepState::Working | StepState::Waiting)
                        })
                    {
                        body = body
                            .child(label(
                                format!("NOW · STEP {:02} · {}", k + 1, s.name.to_uppercase()),
                                step_color(s.state),
                            ))
                            .child(entry(t.prompt.clone()));
                    }
                    if let Some(p) = &pane {
                        body = body
                            .child(label("LATEST CHAT", MIST))
                            .child(super::neon_focus::chat_column(&p.tail, 3));
                        let edited = super::neon_focus::touched(&p.tail);
                        if !edited.is_empty() {
                            body = body
                                .child(label(format!("FILES TOUCHED · {}", edited.len()), MIST));
                            for f in edited.iter().take(3) {
                                body = body.child(code(f, MIST));
                            }
                        }
                    }
                }
                TaskSide::Sessions => {
                    body = body.child(label(format!("SESSIONS · {}", t.sessions.len()), MIST));
                    for (k, s) in t.sessions.iter().enumerate() {
                        let id = s.session.clone();
                        body = body.child(
                            row(
                                format!("nt-s-{k}"),
                                &format!("{:02}", k + 1),
                                CYAN,
                                (&s.title, &s.role, ""),
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.emit(NightEvent::OpenSession(id.clone()));
                                },
                            )),
                        );
                    }
                }
            }
            let open = t.sessions.first().map(|s| s.session.clone());
            let mut tabs: Vec<AnyElement> = Vec::new();
            for (s, name) in [(TaskSide::Plan, "Plan"), (TaskSide::Sessions, "Sessions")] {
                tabs.push(
                    menu_tab(format!("nt-side-{name}"), name, None, CYAN, self.side == s)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            tabs.push(div().flex_1().into_any_element());
            if let Some(a) = TaskAct::of(t.column) {
                let id = t.id.clone();
                tabs.push(
                    menu_tab("nt-act", &format!("{} ›", a.label()), None, CYAN, false)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            task_act(&this.model, &id, a, Self::reload, cx);
                        }))
                        .into_any_element(),
                );
            }
            tabs.push(
                menu_tab("nt-open", "Open session ›", None, CYAN, false)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(id) = &open {
                            cx.emit(NightEvent::OpenSession(id.clone()));
                        }
                    }))
                    .into_any_element(),
            );
            right = right.child(side_tabs(tabs)).child(body);
        } else {
            right = right
                .child(section("Task", CYAN, None))
                .child(nothing("No task here."));
        }
        screen(
            "Tasks",
            sub,
            YELLOW,
            (menu, actions),
            div()
                .size_full()
                .flex()
                .gap(px(32.))
                .child(left)
                .child(right)
                .into_any_element(),
        )
    }
}

// =============================================================================== Repos ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoTab {
    All,
    Live,
    Waiting,
}

pub(super) struct NightRepos {
    model: Entity<Model>,
    tab: RepoTab,
    selected: Option<String>,
    _obs: Subscription,
}

impl EventEmitter<NightEvent> for NightRepos {}

impl NightRepos {
    pub(super) fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            tab: RepoTab::All,
            selected: None,
            _obs: obs,
        }
    }
}

impl Render for NightRepos {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(war) = self.model.read(cx).war.clone() else {
            return screen(
                "Repos",
                "No data yet".to_string(),
                YELLOW,
                (Vec::new(), Vec::new()),
                div().into_any_element(),
            );
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
        let shown: Vec<&aivory_protocol::Codebase> = war
            .codebases
            .iter()
            .filter(|c| match self.tab {
                RepoTab::All => true,
                RepoTab::Live => live(&c.id) > 0,
                RepoTab::Waiting => waits(&c.id) > 0,
            })
            .collect();
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| shown.iter().copied().find(|c| &c.id == id))
            .or(shown.first().copied())
            .cloned();
        let mut menu: Vec<AnyElement> = Vec::new();
        for (t, name, n) in [
            (RepoTab::All, "All", war.codebases.len()),
            (
                RepoTab::Live,
                "Live",
                war.codebases.iter().filter(|c| live(&c.id) > 0).count(),
            ),
            (
                RepoTab::Waiting,
                "Waiting",
                war.codebases.iter().filter(|c| waits(&c.id) > 0).count(),
            ),
        ] {
            menu.push(
                menu_tab(
                    format!("nr-tab-{name}"),
                    name,
                    Some(n),
                    YELLOW,
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
        let counts = war.counts();
        let sub = format!(
            "{} codebases · {} live sessions · {} wait for you",
            war.codebases.len(),
            counts.live,
            counts.needs_you
        );

        let mut rows: Vec<AnyElement> = Vec::new();
        for (k, c) in shown.iter().enumerate() {
            let id = c.id.clone();
            let w = waits(&c.id);
            rows.push(
                row(
                    format!("nr-row-{k}"),
                    &format!("{:02}", k + 1),
                    if w > 0 { RED } else { CYAN },
                    (
                        &c.id,
                        &format!("{} · {}", c.branch, c.path),
                        &format!("{} live", live(&c.id)),
                    ),
                    cur.as_ref().is_some_and(|x| x.id == c.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(nothing("No codebase here"));
        }
        let rows = cap_rows(rows, fit_rows(window, 350., 54.), 54.);
        let left = list_panel(
            "nr-list",
            ("Codebases", YELLOW),
            format!("{} shown", shown.len()),
            rows,
        );

        let mut right = frame(CYAN, "nr-detail", false)
            .flex_1()
            .min_w(px(0.))
            .h_full();
        if let Some(c) = &cur {
            let sessions: Vec<&Pane> = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.state.is_live())
                .collect();
            right = right.child(section(
                "Codebase",
                CYAN,
                Some(format!("{} worktrees", c.worktrees)),
            ));
            let mut body = detail_body("nr-body")
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_size(crate::ui::ts(T_SECTION))
                                .line_height(crate::ui::ts(26.))
                                .text_color(rgb(0xFFFFFF))
                                .child(c.id.clone()),
                        )
                        .child(label(format!("{} · {}", c.branch, c.daemon), MIST)),
                )
                .child(code(&c.path, MIST))
                .child(label(format!("PARTS · {}", c.parts.len()), MIST));
            for p in c.parts.iter().take(6) {
                body = body.child(entry(format!("{} · {}", p.name, p.paths.join(", "))));
            }
            body = body.child(label(format!("LIVE SESSIONS · {}", sessions.len()), MIST));
            for (k, s) in sessions.iter().take(6).enumerate() {
                let id = s.id.clone();
                let now_line = format!("{} · {}", super::neon_focus::doing_line(s), s.provider);
                body = body.child(
                    row(
                        format!("nr-s-{k}"),
                        &format!("{:02}", k + 1),
                        pane_color(s),
                        (&s.title, &now_line, super::neon_focus::state_of(s).0),
                        false,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(NightEvent::OpenSession(id.clone()));
                    })),
                );
            }
            if sessions.len() > 6 {
                body = body.child(label(
                    format!("+{} more live sessions", sessions.len() - 6),
                    MIST,
                ));
            }
            let spend: f64 = sessions.iter().map(|s| s.cost).sum();
            let tokens: u64 = sessions.iter().map(|s| s.tokens).sum();
            body = body.child(label("WORK HERE", MIST)).child(entry(format!(
                "${spend:.2} · {} tok · {} worktrees · {}",
                super::neon_focus::short_count(tokens),
                c.worktrees,
                c.languages.join(", ")
            )));
            let open = sessions.first().map(|s| s.id.clone());
            right = right
                .child(side_tabs(vec![
                    div().flex_1().into_any_element(),
                    menu_tab("nr-open", "Open first live session ›", None, CYAN, false)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(id) = &open {
                                cx.emit(NightEvent::OpenSession(id.clone()));
                            }
                        }))
                        .into_any_element(),
                ]))
                .child(body);
        } else {
            right = right
                .child(section("Codebase", CYAN, None))
                .child(nothing("Pick a codebase."));
        }
        screen(
            "Repos",
            sub,
            YELLOW,
            (menu, Vec::new()),
            div()
                .size_full()
                .flex()
                .gap(px(32.))
                .child(left)
                .child(right)
                .into_any_element(),
        )
    }
}

// ============================================================================ Settings ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Looks,
    Autopilot,
    Providers,
    Devices,
    Notifications,
}

/// The notification events: key, name, when it fires.
const EVENTS: [(&str, &str, &str); 5] = [
    (
        "needsAnswer",
        "Needs your answer",
        "A question, a permission or a plan waits for you.",
    ),
    ("error", "Error", "A session stops on an error."),
    ("review", "Review", "A task reaches the review gate."),
    ("conflict", "Conflict", "Two sessions edit the same code."),
    (
        "autopilot",
        "Autopilot action",
        "A session parks or a queued task starts.",
    ),
];

/// The notification channels: key, name.
const CHANNELS: [(&str, &str); 3] = [
    ("desktop", "Desktop"),
    ("phone", "Phone"),
    ("email", "E-mail"),
];

/// The channels of one event.
fn channels_of(n: &aivory_protocol::Notify, event: &str) -> aivory_protocol::Channels {
    match event {
        "needsAnswer" => n.needs_answer,
        "error" => n.error,
        "review" => n.review,
        "conflict" => n.conflict,
        _ => n.autopilot,
    }
}

pub(super) struct NightSettings {
    model: Entity<Model>,
    section: Section,
    /// The look that the detail panel describes; the active look when `None`.
    picked: Option<aivory_protocol::Look>,
    /// The settings of the daemon: providers, devices, notifications.
    view: Option<aivory_protocol::SettingsView>,
    provider: Option<String>,
    event: usize,
    _obs: Subscription,
}

impl EventEmitter<NightEvent> for NightSettings {}

impl NightSettings {
    pub(super) fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        let mut this = Self {
            model,
            section: Section::Looks,
            picked: None,
            view: None,
            provider: None,
            event: 0,
            _obs: obs,
        };
        this.refresh(cx);
        this
    }

    /// Read the settings of the daemon again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(view) = result {
                    this.view = Some(view);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Switch one notification channel; the daemon returns the saved settings.
    fn set_notify(&mut self, event: &str, channel: &str, on: bool, cx: &mut Context<Self>) {
        let call = self.model.read(cx).call::<method::SettingsSet>(
            aivory_protocol::rpc::SettingsSetParams {
                key: format!("notify.{event}.{channel}"),
                value: serde_json::Value::Bool(on),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if let (Ok(settings), Some(view)) = (result, this.view.as_mut()) {
                    view.settings = settings;
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// What a look is, in one sentence.
fn look_text(look: aivory_protocol::Look) -> &'static str {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => {
            "Light and quiet. A rail on the left, white panels, and the plain layout of every screen."
        }
        Look::Bridge => {
            "Starship console. Colored frame segments, numbered navigation and two islands per screen."
        }
        Look::Operator => {
            "Operator wall. Green rain, function keys in the top line and blue instrument screens."
        }
        Look::Neon => "Neon on night. A 3D city of districts, glow frames and a header with tabs.",
        Look::Navigator => {
            "A workstation desktop. A toolchest bar on top and grey windows for the screens."
        }
        Look::Holo => "See-through glass. Calm panels that float over the war room.",
        Look::Tactical => "A radar of live sessions with tactical panels and one bar on top.",
        Look::Command => "A command console: units on a map and a command sidebar on the right.",
    }
}

/// One line about a look, for the list.
fn look_short(look: aivory_protocol::Look) -> &'static str {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => "Light, quiet, a rail on the left",
        Look::Bridge => "Starship console, orange frame",
        Look::Operator => "Green rain and function keys",
        Look::Neon => "3D night city, glow frames",
        Look::Navigator => "Workstation desktop, grey windows",
        Look::Holo => "See-through glass panels",
        Look::Tactical => "Radar of live sessions",
        Look::Command => "Command map with units",
    }
}

/// The ground, the accent and the text color of a look, for the swatch and the preview.
fn look_colors(look: aivory_protocol::Look) -> (u32, u32, u32) {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => (0xF6F7F9, 0x2E6BE6, 0x1B1F27),
        Look::Bridge => (0x120A1E, 0xFF9C33, 0xF2E8FF),
        Look::Operator => (0x020A06, 0x33FF77, 0xB8FFD0),
        Look::Neon => (BG, YELLOW, PALE),
        Look::Navigator => (0xC6C6C6, 0x7088AE, 0x111111),
        Look::Holo => (0x040A16, 0x7FD8FF, 0xE6F6FF),
        Look::Tactical => (0x262626, 0x8FD14F, 0xEDEDED),
        Look::Command => (0x0B1118, 0x4FB8C4, 0xD6E4EC),
    }
}

/// A small mock war room in the colors of a look: a header with tabs, a map with blocks,
/// two HUD panels on the right and a board at the bottom. The Neon and the Holo show blocks
/// of different heights; the other looks show round nodes.
fn look_preview(look: aivory_protocol::Look) -> Div {
    use aivory_protocol::Look;
    let (bg, accent, ink) = look_colors(look);
    let line = |w: f32, c: u32, a: u32| div().h(px(6.)).w(relative(w)).bg(rgba(c << 8 | a));
    let panel = |title: f32, rows: [f32; 3]| {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .border_1()
            .border_color(rgba(accent << 8 | 0x99))
            .bg(rgba(accent << 8 | 0x10))
            .child(line(title, accent, 0xFF))
            .child(line(rows[0], ink, 0x99))
            .child(line(rows[1], ink, 0x77))
            .child(line(rows[2], ink, 0x55))
    };
    let blocky = matches!(
        look,
        Look::Neon | Look::Holo | Look::Command | Look::Tactical
    );
    let mut map = div().flex_1().relative().overflow_hidden();
    for (k, (x, y, h)) in [
        (0.12, 0.55, 40.),
        (0.28, 0.35, 64.),
        (0.44, 0.58, 30.),
        (0.60, 0.30, 52.),
        (0.74, 0.55, 72.),
        (0.38, 0.12, 36.),
    ]
    .into_iter()
    .enumerate()
    {
        let c = if k == 3 { 0xFF2A6D } else { accent };
        let node = if blocky {
            div()
                .absolute()
                .left(relative(x))
                .top(relative(y))
                .w(px(26.))
                .h(px(h))
                .mt(px(-h + 30.))
                .bg(rgba(c << 8 | 0x33))
                .border_1()
                .border_color(rgb(c))
        } else {
            div()
                .absolute()
                .left(relative(x))
                .top(relative(y))
                .w(px(18.))
                .h(px(18.))
                .rounded_full()
                .bg(rgba(c << 8 | 0x55))
                .border_1()
                .border_color(rgb(c))
        };
        map = map.child(node);
    }
    div()
        .w_full()
        .flex_1()
        .min_h(px(280.))
        .flex()
        .flex_col()
        .bg(rgb(bg))
        .border_1()
        .border_color(rgba(accent << 8 | 0x99))
        .child(
            div()
                .h(px(28.))
                .flex_none()
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .bg(rgba(accent << 8 | 0x22))
                .child(div().w(px(10.)).h(px(10.)).bg(rgb(accent)))
                .child(div().w(px(46.)).h(px(14.)).bg(rgb(accent)))
                .child(div().w(px(46.)).h(px(14.)).bg(rgba(ink << 8 | 0x33)))
                .child(div().w(px(46.)).h(px(14.)).bg(rgba(ink << 8 | 0x33)))
                .child(div().flex_1())
                .child(div().w(px(60.)).h(px(8.)).bg(rgba(ink << 8 | 0x88))),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .gap(px(10.))
                .p(px(10.))
                .child(map)
                .child(
                    div()
                        .w(px(150.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(panel(0.6, [0.9, 0.7, 0.8]))
                        .child(panel(0.8, [0.6, 0.9, 0.5])),
                ),
        )
        .child(
            div()
                .h(px(36.))
                .flex_none()
                .mx(px(10.))
                .mb(px(10.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(14.))
                .border_1()
                .border_color(rgba(accent << 8 | 0x66))
                .child(line(0.2, accent, 0xFF))
                .child(line(0.25, ink, 0x88))
                .child(line(0.2, ink, 0x66)),
        )
}

/// An autopilot change as a button.
fn patch_button(
    id: &'static str,
    text: &str,
    patch: AutopilotPatch,
    color: u32,
    cx: &mut Context<NightSettings>,
) -> AnyElement {
    button(id, text, color, false)
        .on_click(cx.listener(move |this, _, _, cx| {
            act(&this.model, Action::Autopilot(patch.clone()), cx);
        }))
        .into_any_element()
}

/// The word and the color of a provider state.
fn provider_state(status: aivory_protocol::ProviderStatus) -> (&'static str, u32) {
    use aivory_protocol::ProviderStatus;
    match status {
        ProviderStatus::Connected => ("CONNECTED", CYAN),
        ProviderStatus::Acp => ("ACP", MIST),
        ProviderStatus::Off => ("OFF", DIM),
    }
}

impl Render for NightSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let (auto, max, park) = war.as_ref().map_or((false, 0, 0), |w| {
            (
                w.autopilot.on,
                w.autopilot.max_running,
                w.autopilot.park_after_min,
            )
        });
        let look = crate::theme::tk().look;
        let sub = format!(
            "Look {} · autopilot {} · up to {max} sessions",
            crate::look::name(look),
            if auto { "on" } else { "off" }
        );
        let mut menu: Vec<AnyElement> = Vec::new();
        for (s, name) in [
            (Section::Looks, "Looks"),
            (Section::Autopilot, "Autopilot"),
            (Section::Providers, "Providers"),
            (Section::Devices, "Devices"),
            (Section::Notifications, "Notifications"),
        ] {
            menu.push(
                menu_tab(
                    format!("ns-tab-{name}"),
                    name,
                    None,
                    YELLOW,
                    self.section == s,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.section = s;
                    if matches!(s, Section::Devices | Section::Providers) {
                        this.refresh(cx);
                    }
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        // The rest (daemons, display, account) lives in the full settings screen.
        menu.push(
            menu_tab("ns-tab-Other", "Daemons and account ›", None, MIST, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NightEvent::AllSettings)))
                .into_any_element(),
        );
        menu.push(
            menu_tab("ns-tab-Pair", "Pair a phone ›", None, MIST, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NightEvent::PairPhone)))
                .into_any_element(),
        );
        let actions: Vec<AnyElement> = Vec::new();

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut right = frame(CYAN, "ns-detail", false)
            .flex_1()
            .min_w(px(0.))
            .h_full();
        let (left_title, left_note);
        match self.section {
            Section::Looks => {
                left_title = "Looks";
                left_note = "click to preview".to_string();
                for (k, (l, name)) in crate::look::LOOKS.into_iter().enumerate() {
                    let on = l == look;
                    rows.push(
                        row(
                            format!("ns-look-{k}"),
                            &format!("{:02}", k + 1),
                            look_colors(l).1,
                            (name, look_short(l), if on { "ACTIVE" } else { "" }),
                            self.picked.unwrap_or(look) == l,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.picked = Some(l);
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                }
                let shown = self.picked.unwrap_or(look);
                let active = shown == look;
                let shown_name = crate::look::name(shown);
                let mut body = detail_body("ns-body")
                    .child(look_preview(shown))
                    .child(label("PALETTE · GROUND, ACCENT, TEXT", MIST))
                    .child({
                        let (g, a, t) = look_colors(shown);
                        let mut sw = div().flex().gap(px(12.));
                        for (c, name) in [(g, "Ground"), (a, "Accent"), (t, "Text")] {
                            sw = sw.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .w(px(48.))
                                            .h(px(32.))
                                            .bg(rgb(c))
                                            .border_1()
                                            .border_color(rgba(0x8C96BC66)),
                                    )
                                    .child(label(format!("{name} {c:06X}"), MIST)),
                            );
                        }
                        sw
                    })
                    .child(entry(look_text(shown)))
                    .child(label(
                        if active {
                            format!("{shown_name} is the active look.")
                        } else {
                            format!(
                                "{shown_name} is not active. The active look is {}.",
                                crate::look::name(look)
                            )
                        },
                        if active { CYAN } else { MIST },
                    ))
                    .child(label(
                        "A look replaces the frame, the colors and the fonts of every screen. The choice is saved in the settings of the daemon.",
                        MIST,
                    ));
                if !active {
                    body = body.child(
                        div().flex().child(
                            button(
                                "ns-switch",
                                &format!("Switch to {shown_name}"),
                                YELLOW,
                                true,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    act(&this.model, Action::SetLook(shown), cx);
                                    this.picked = None;
                                },
                            )),
                        ),
                    );
                }
                right = right
                    .child(section(
                        shown_name,
                        if active { YELLOW } else { CYAN },
                        Some(if active { "ACTIVE" } else { "PREVIEW" }.to_string()),
                    ))
                    .child(body);
            }
            Section::Autopilot => {
                left_title = "Autopilot";
                left_note = if auto { "ON" } else { "OFF" }.to_string();
                rows.push(
                    div()
                        .px(px(24.))
                        .py(px(8.))
                        .flex()
                        .flex_wrap()
                        .gap(px(12.))
                        .child(patch_button(
                            "ns-auto",
                            if auto {
                                "Autopilot: on, click to stop"
                            } else {
                                "Autopilot: off, click to start"
                            },
                            AutopilotPatch {
                                on: Some(!auto),
                                ..AutopilotPatch::default()
                            },
                            CYAN,
                            cx,
                        ))
                        .child(patch_button(
                            "ns-less",
                            "Fewer sessions",
                            AutopilotPatch {
                                max_running: Some(max.saturating_sub(1).max(1)),
                                ..AutopilotPatch::default()
                            },
                            CYAN,
                            cx,
                        ))
                        .child(patch_button(
                            "ns-more",
                            "More sessions",
                            AutopilotPatch {
                                max_running: Some((max + 1).min(64)),
                                ..AutopilotPatch::default()
                            },
                            CYAN,
                            cx,
                        ))
                        .into_any_element(),
                );
                right = right
                    .child(section(
                        &format!("Up to {max} sessions run at once"),
                        CYAN,
                        Some(if auto { "ON" } else { "OFF" }.to_string()),
                    ))
                    .child(
                        detail_body("ns-body")
                            .child(entry(format!(
                                "Autopilot is {}. A session parks after {park} minutes without work.",
                                if auto { "on" } else { "off" }
                            )))
                            .child(label(
                                "Autopilot starts queued tasks when a slot is free, and parks sessions that wait for you.",
                                MIST,
                            ))
                            .child(label(
                                "Fewer sessions and More sessions change the number of slots.",
                                MIST,
                            )),
                    );
            }
            Section::Providers => {
                left_title = "Providers";
                let list: Vec<aivory_protocol::ProviderInfo> = self
                    .view
                    .as_ref()
                    .map(|v| v.providers.clone())
                    .unwrap_or_default();
                left_note = format!(
                    "{} connected",
                    list.iter()
                        .filter(|p| p.status == aivory_protocol::ProviderStatus::Connected)
                        .count()
                );
                let cur = self
                    .provider
                    .as_ref()
                    .and_then(|id| list.iter().find(|p| &p.id == id))
                    .or(list.first())
                    .cloned();
                for (k, p) in list.iter().enumerate() {
                    let id = p.id.clone();
                    let (word, c) = provider_state(p.status);
                    rows.push(
                        row(
                            format!("ns-prov-{k}"),
                            &format!("{:02}", k + 1),
                            c,
                            (&p.name, &p.auth, word),
                            cur.as_ref().is_some_and(|x| x.id == p.id),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.provider = Some(id.clone());
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                }
                if rows.is_empty() {
                    rows.push(nothing(if self.view.is_some() {
                        "No provider"
                    } else {
                        "Loading the providers"
                    }));
                }
                if let Some(p) = &cur {
                    let (word, c) = provider_state(p.status);
                    let mut body = detail_body("ns-body")
                        .child(entry(p.auth.clone()))
                        .child(label(format!("MODELS · {}", p.models.len()), MIST));
                    for m in p.models.iter().take(10) {
                        body = body.child(code(m, MIST));
                    }
                    if !p.usage.is_empty() {
                        body = body.child(label("USAGE WINDOWS", MIST));
                    }
                    for u in &p.usage {
                        body = body
                            .child(label(
                                format!(
                                    "{} · {}%{}",
                                    u.label,
                                    u.pct,
                                    if u.estimate { " · estimate" } else { "" }
                                ),
                                if u.pct >= 80 { RED } else { INK },
                            ))
                            .child(super::neon_focus::bar(
                                f32::from(u.pct) / 100.,
                                if u.pct >= 80 { RED } else { CYAN },
                                4.,
                            ));
                    }
                    body = body.child(label(
                        "The model, effort and permission mode of a provider are set per session in the composer.",
                        DIM,
                    ));
                    right = right
                        .child(section(&p.name, c, Some(word.to_string())))
                        .child(body);
                } else {
                    right = right
                        .child(section("Providers", CYAN, None))
                        .child(nothing("No provider to show."));
                }
            }
            Section::Devices => {
                left_title = "Paired devices";
                let devices: Vec<aivory_protocol::Device> = self
                    .view
                    .as_ref()
                    .map(|v| v.devices.clone())
                    .unwrap_or_default();
                left_note = format!("{} paired", devices.len());
                for (k, d) in devices.iter().enumerate() {
                    rows.push(
                        row(
                            format!("ns-dev-{k}"),
                            &format!("{:02}", k + 1),
                            if d.online { CYAN } else { DIM },
                            (&d.name, &d.os, if d.online { "ONLINE" } else { "OFFLINE" }),
                            false,
                        )
                        .into_any_element(),
                    );
                }
                if rows.is_empty() {
                    rows.push(nothing(if self.view.is_some() {
                        "No device is paired"
                    } else {
                        "Loading the devices"
                    }));
                }
                let mut body = detail_body("ns-body")
                    .child(entry(
                        "A paired phone shows the war room and answers asks away from the desk.",
                    ))
                    .child(
                        div().flex().child(
                            button("ns-pair", "Pair a phone", YELLOW, true).on_click(
                                cx.listener(|_, _, _, cx| cx.emit(NightEvent::PairPhone)),
                            ),
                        ),
                    );
                if let Some(v) = &self.view {
                    body = body
                        .child(label("ACCOUNT", MIST))
                        .child(entry(format!(
                            "{} · {} seats · {}",
                            v.account.team, v.account.seats, v.account.license
                        )))
                        .child(label(format!("Renews {}", v.account.renews), MIST));
                }
                right = right
                    .child(section(
                        "Devices",
                        CYAN,
                        Some(format!("{} PAIRED", devices.len())),
                    ))
                    .child(body);
            }
            Section::Notifications => {
                left_title = "Notify me when";
                left_note = String::new();
                let settings = self.view.as_ref().map(|v| v.settings.notify.clone());
                for (k, (key, name, _)) in EVENTS.iter().enumerate() {
                    let on = settings.as_ref().map_or(0, |n| {
                        let c = channels_of(n, key);
                        usize::from(c.desktop) + usize::from(c.phone) + usize::from(c.email)
                    });
                    rows.push(
                        row(
                            format!("ns-ev-{k}"),
                            &format!("{:02}", k + 1),
                            if on > 0 { CYAN } else { DIM },
                            (name, "", &format!("{on} of 3")),
                            self.event == k,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.event = k;
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                }
                let (key, name, when) = EVENTS[self.event.min(EVENTS.len() - 1)];
                let mut body = detail_body("ns-body").child(entry(when));
                match &settings {
                    Some(n) => {
                        let c = channels_of(n, key);
                        for (ch, ch_name) in CHANNELS {
                            let on = match ch {
                                "desktop" => c.desktop,
                                "phone" => c.phone,
                                _ => c.email,
                            };
                            body = body.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(16.))
                                    .h(px(48.))
                                    .border_b_1()
                                    .border_color(gpui::rgba(0x8C96BC1A))
                                    .child(entry(ch_name))
                                    .child(
                                        button(
                                            format!("ns-n-{key}-{ch}"),
                                            if on {
                                                "On, click to turn off"
                                            } else {
                                                "Off, click to turn on"
                                            },
                                            if on { CYAN } else { MIST },
                                            on,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_notify(key, ch, !on, cx);
                                            }),
                                        ),
                                    ),
                            );
                        }
                    }
                    None => {
                        body = body.child(label("Loading the settings", MIST));
                    }
                }
                right = right.child(section(name, CYAN, None)).child(body);
            }
        }
        let left = list_panel("ns-list", (left_title, YELLOW), left_note, rows);
        screen(
            "Settings",
            sub,
            YELLOW,
            (menu, actions),
            div()
                .size_full()
                .flex()
                .gap(px(32.))
                .child(left)
                .child(right)
                .into_any_element(),
        )
    }
}

// ======================================================================= the shell glue ====

impl Shell {
    /// The Neon version of the screen on view: Inbox, Tasks, Repos and Settings are
    /// Neon screens; Focus, Lens and the full Settings keep their content inside the
    /// Neon frame with a title and a menu.
    pub(super) fn night_content(
        &mut self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        fn handle(this: &mut Shell, event: &NightEvent, cx: &mut Context<Shell>) {
            match event {
                NightEvent::OpenSession(id) => this.open_session(id, cx),
                NightEvent::PairPhone => this.open_pairing(cx),
                NightEvent::AllSettings => {
                    this.bridge_full_settings = true;
                    cx.notify();
                }
            }
        }
        let model = self.model.clone();
        match self.screen.clone() {
            Screen::Inbox => {
                if self.night_screens.inbox.is_none() {
                    let v = cx.new(|cx| NightInbox::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &NightEvent, cx| handle(t, e, cx));
                    self.night_screens.inbox = Some((v, sub));
                }
                self.night_screens
                    .inbox
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Tasks => {
                if self.night_screens.tasks.is_none() {
                    let v = cx.new(|cx| NightTasks::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &NightEvent, cx| handle(t, e, cx));
                    self.night_screens.tasks = Some((v, sub));
                }
                self.night_screens
                    .tasks
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Repos => {
                if self.night_screens.repos.is_none() {
                    let v = cx.new(|cx| NightRepos::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &NightEvent, cx| handle(t, e, cx));
                    self.night_screens.repos = Some((v, sub));
                }
                self.night_screens
                    .repos
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Settings if !self.bridge_full_settings => {
                if self.night_screens.settings.is_none() {
                    let v = cx.new(|cx| NightSettings::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &NightEvent, cx| handle(t, e, cx));
                    self.night_screens.settings = Some((v, sub));
                }
                self.night_screens
                    .settings
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Settings => {
                let menu = vec![
                    menu_tab("nf-set-back", "‹ Settings", None, YELLOW, false)
                        .on_click(cx.listener(|this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                            this.bridge_full_settings = false;
                            cx.notify();
                        }))
                        .into_any_element(),
                    menu_tab("nf-set-all", "Daemons and account", None, YELLOW, true)
                        .into_any_element(),
                ];
                framed(
                    "Settings",
                    "Every setting of the app".to_string(),
                    (menu, Vec::new()),
                    content,
                )
                .into_any_element()
            }
            Screen::Session(id) => {
                use super::neon_focus::{NightFocus, NightFocusEvent};
                if let Some((v, _)) = &self.night_screens.focus {
                    if v.read(cx).session() != id {
                        v.update(cx, |v, cx| v.show(&id, cx));
                    }
                } else {
                    let v = cx.new(|cx| NightFocus::new(model, id.clone(), window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &NightFocusEvent, cx| match e {
                        NightFocusEvent::Back => t.go_back(cx),
                        NightFocusEvent::Open(id) => t.open_session(id, cx),
                        NightFocusEvent::OpenFile { session, path } => {
                            t.open_session_file(session, path, cx)
                        }
                    });
                    self.night_screens.focus = Some((v, sub));
                }
                self.night_screens
                    .focus
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Lens => {
                use super::neon_lens::{NightLens, NightLensEvent};
                if self.night_screens.lens.is_none() {
                    let v = cx.new(|cx| NightLens::new(model, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &NightLensEvent, cx| match e {
                        NightLensEvent::Back => {
                            let back = std::mem::replace(&mut t.lens_back, Screen::Repos);
                            t.go(back, cx);
                        }
                        NightLensEvent::OpenSession(id) => t.open_session(id, cx),
                    });
                    self.night_screens.lens = Some((v, sub));
                }
                let target = self.bridge_screens.lens_target.take();
                if let (Some((v, _)), Some((codebase, path, session))) =
                    (&self.night_screens.lens, target)
                {
                    v.update(cx, |v, cx| v.show(&codebase, &path, session, cx));
                }
                self.night_screens
                    .lens
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            _ => content,
        }
    }
}

/// A screen of the look that holds the content of another screen inside a neon frame.
fn framed(
    title: &str,
    sub: String,
    bar: (Vec<AnyElement>, Vec<AnyElement>),
    content: AnyElement,
) -> Div {
    screen(
        title,
        sub,
        YELLOW,
        bar,
        frame(CYAN, "nf-frame", true)
            .size_full()
            .overflow_hidden()
            .child(div().flex_1().min_h(px(0.)).child(content))
            .into_any_element(),
    )
}
