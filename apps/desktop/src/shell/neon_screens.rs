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
    Window, div, px, rgb,
};

use super::neon_kit::*;
use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::model::Model;

/// What a screen asks the shell to do.
#[derive(Debug, Clone)]
pub(super) enum NightEvent {
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
        .py(px(20.))
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
            "{} decisions wait for you · oldest {oldest}",
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
                    body = body.child(label("COMMAND OR QUESTION", MIST));
                    body = body.child(code(&it.text, color));
                    if !it.steps.is_empty() {
                        body = body.child(label("PLAN", MIST));
                        for (k, s) in it.steps.iter().take(6).enumerate() {
                            body = body.child(entry(format!("{}  {s}", k + 1)));
                        }
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
                    for r in chat_text(&it.tail, 8) {
                        body = body.child(entry(r));
                    }
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

/// The last prose rows of a transcript as `You: text` lines.
fn chat_text(tail: &[aivory_protocol::Line], n: usize) -> Vec<String> {
    use aivory_protocol::LineKind;
    let mut rows: Vec<String> = tail
        .iter()
        .filter_map(|l| {
            let t = l.text.lines().next().unwrap_or("").trim();
            match l.kind {
                LineKind::User if !t.is_empty() => Some(format!("You: {t}")),
                LineKind::Agent if t.chars().next().is_some_and(char::is_alphabetic) => {
                    Some(format!("Agent: {t}"))
                }
                _ => None,
            }
        })
        .collect();
    let skip = rows.len().saturating_sub(n);
    rows.split_off(skip)
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
        StepState::Done => GREEN,
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
                    "Autopilot on"
                } else {
                    "Autopilot off"
                },
                if autopilot { GREEN } else { CYAN },
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
        let rows = cap_rows(rows, fit_rows(window, 350., 54.), 54.);
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
                    for (k, s) in t.steps.iter().enumerate() {
                        body = body.child(
                            div()
                                .flex_none()
                                .h(px(40.))
                                .flex()
                                .items_center()
                                .gap(px(16.))
                                .border_b_1()
                                .border_color(gpui::rgba(0x8C96BC1A))
                                .child(
                                    div()
                                        .w(px(28.))
                                        .flex_none()
                                        .font_weight(FontWeight::BOLD)
                                        .text_size(crate::ui::ts(T_LABEL))
                                        .text_color(rgb(step_color(s.state)))
                                        .child(format!("{:02}", k + 1)),
                                )
                                .child(div().flex_1().child(entry(s.name.clone())))
                                .child(label(step_word(s.state), step_color(s.state))),
                        );
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
            for (k, s) in sessions.iter().take(5).enumerate() {
                let id = s.id.clone();
                body = body.child(
                    row(
                        format!("nr-s-{k}"),
                        &format!("{:02}", k + 1),
                        pane_color(s),
                        (&s.title, &s.provider, ""),
                        false,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(NightEvent::OpenSession(id.clone()));
                    })),
                );
            }
            let open = sessions.first().map(|s| s.id.clone());
            right = right
                .child(side_tabs(vec![
                    div().flex_1().into_any_element(),
                    menu_tab("nr-open", "Open session ›", None, CYAN, false)
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
}

pub(super) struct NightSettings {
    model: Entity<Model>,
    section: Section,
    /// The look that the detail panel describes; the active look when `None`.
    picked: Option<aivory_protocol::Look>,
    _obs: Subscription,
}

impl EventEmitter<NightEvent> for NightSettings {}

impl NightSettings {
    pub(super) fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            section: Section::Looks,
            picked: None,
            _obs: obs,
        }
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
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        let actions = vec![
            button("ns-all", "All settings ›", CYAN, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NightEvent::AllSettings)))
                .into_any_element(),
        ];

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut right = frame(CYAN, "ns-detail", false)
            .flex_1()
            .min_w(px(0.))
            .h_full();
        let (left_title, left_note);
        match self.section {
            Section::Looks => {
                left_title = "Looks";
                left_note = "click to switch".to_string();
                for (k, (l, name)) in crate::look::LOOKS.into_iter().enumerate() {
                    let on = l == look;
                    rows.push(
                        row(
                            format!("ns-look-{k}"),
                            &format!("{:02}", k + 1),
                            if on { YELLOW } else { CYAN },
                            (name, "", if on { "ON" } else { "" }),
                            on,
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
                    .child(entry(look_text(shown)))
                    .child(label(
                        if active {
                            format!("{shown_name} is the active look.")
                        } else {
                            format!("{shown_name} is not active. The active look is {}.", crate::look::name(look))
                        },
                        if active { GREEN } else { MIST },
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
                        Some(if active { "ACTIVE" } else { "OFF" }.to_string()),
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
                                "Autopilot on"
                            } else {
                                "Autopilot off"
                            },
                            AutopilotPatch {
                                on: Some(!auto),
                                ..AutopilotPatch::default()
                            },
                            if auto { GREEN } else { CYAN },
                            cx,
                        ))
                        .child(patch_button(
                            "ns-less",
                            "Fewer",
                            AutopilotPatch {
                                max_running: Some(max.saturating_sub(1).max(1)),
                                ..AutopilotPatch::default()
                            },
                            CYAN,
                            cx,
                        ))
                        .child(patch_button(
                            "ns-more",
                            "More",
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
                        None,
                    ))
                    .child(detail_body("ns-body").child(entry(format!(
                        "Autopilot is {}. A session parks after {park} minutes without work.",
                        if auto { "on" } else { "off" }
                    ))));
            }
            Section::Providers => {
                left_title = "Usage windows";
                left_note = String::new();
                if let Some(w) = &war {
                    for (k, u) in w.usage.iter().enumerate() {
                        let c = if u.pct >= 80 { RED } else { CYAN };
                        rows.push(
                            row(
                                format!("ns-use-{k}"),
                                &format!("{:02}", k + 1),
                                c,
                                (
                                    &format!("{} {}", u.provider, u.label),
                                    if u.estimate {
                                        "estimate"
                                    } else {
                                        "from the provider"
                                    },
                                    &format!("{}%", u.pct),
                                ),
                                false,
                            )
                            .into_any_element(),
                        );
                    }
                }
                if rows.is_empty() {
                    rows.push(nothing("No usage window yet"));
                }
                right = right
                    .child(section("Providers and models", CYAN, None))
                    .child(detail_body("ns-body").child(entry(
                    "Models, effort and permission mode per provider are set in the full settings.",
                )));
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
                    menu_tab("nf-set-back", "Summary", None, YELLOW, false)
                        .on_click(cx.listener(|this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                            this.bridge_full_settings = false;
                            cx.notify();
                        }))
                        .into_any_element(),
                    menu_tab("nf-set-all", "All settings", None, YELLOW, true).into_any_element(),
                ];
                framed(
                    "Settings",
                    "Every setting of the app".to_string(),
                    (menu, Vec::new()),
                    content,
                )
                .into_any_element()
            }
            Screen::Focus(id) => {
                use super::neon_focus::{NightFocus, NightFocusEvent};
                if let Some((v, _)) = &self.night_screens.focus {
                    if v.read(cx).session() != id {
                        v.update(cx, |v, cx| v.show(&id, cx));
                    }
                } else {
                    let v = cx.new(|cx| NightFocus::new(model, id.clone(), window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &NightFocusEvent, cx| match e {
                        NightFocusEvent::Back => t.go(Screen::WarRoom, cx),
                        NightFocusEvent::Open(id) => t.open_focus(id, cx),
                        NightFocusEvent::Detail(id) => t.open_session(id, cx),
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
