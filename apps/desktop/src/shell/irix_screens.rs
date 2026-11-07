//! The Navigator Tasks and Repos screens (ADR 0022): a title, a key row, a list box at the left
//! and a detail pane at the right, all in the bevelled workstation style. The full screens of
//! the Minimal layout stay behind the `Board` and `Manage` keys, so no function is lost.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{AutopilotPatch, TaskListParams, TaskUpdateParams, method};
use aivory_protocol::{
    AskKind, Pane, SessionState, StepState, Task, TaskColumn, TaskList, WarRoom,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FontWeight, ScrollHandle, Task as GpuiTask,
    Window, div, px, rgb,
};

use crate::actions::{Action, act};
use crate::model::Model;
use crate::repos::ReposScreen;
use crate::tasks::TasksScreen;
use crate::theme::*;
use crate::ui;
use crate::warroom::skins::navigator::panels::{
    ALERT, GAP_PANEL, LED_AMBER, LED_GREEN, LED_OFF, LED_RED, MUTED, PAD, S_ENTRY, S_LABEL, Series,
    TEXT, heading, kv, led, list_box, list_row, menu_bar, note, page, push, scroll_area, section,
    seg, strip_chart, sunken,
};

/// What a Navigator screen asks the shell to do.
#[derive(Debug, Clone)]
pub enum IrixEvent {
    /// The deep dive of a session.
    OpenSession(String),
    /// The Focus view of a session.
    OpenFocus(String),
    /// The war room.
    WarRoom,
}

/// The LED of a session: red waits, amber errs, grey idles, green works.
pub(super) fn led_for(s: &Pane) -> u32 {
    match (&s.ask, s.state) {
        (_, SessionState::Error) => LED_AMBER,
        (Some(_), _) => LED_RED,
        (None, SessionState::Idle) => 0x9A9A9A,
        _ => LED_GREEN,
    }
}

/// Call `f` on the view every `every`.
pub(super) fn ticker<V: 'static>(
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

/// The width of the list box at the left.
const LIST_W: f32 = 440.;

/// A list and a detail pane side by side. The list scrolls; the detail pane scrolls.
fn two_panes(list: Div, detail: Div) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .gap(px(GAP_PANEL))
        .child(list.w(px(LIST_W)).flex_none().h_full())
        .child(detail.flex_1().min_w_0().h_full())
}

/// The detail pane: a sunken white panel with a scroll bar.
fn detail_pane(id: &'static str, handle: &ScrollHandle, body: Div) -> Div {
    sunken(div().size_full().child(scroll_area(
        id,
        handle,
        false,
        div().p(px(PAD)).flex().flex_col().gap(px(20.)).child(body),
    )))
    .size_full()
}

/// A row of small facts at the top of a detail pane.
fn facts(list: Vec<(&str, String, u32)>) -> Div {
    let mut row = div().flex().flex_wrap().gap_x(px(GAP_PANEL)).gap_y(px(10.));
    for (label, value, color) in list {
        row = row.child(kv(label, value, color));
    }
    row
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    Queued,
    Running,
    Review,
    Done,
    All,
}

impl Filter {
    const ALL: [(Filter, &'static str); 5] = [
        (Filter::Queued, "Queued"),
        (Filter::Running, "Running"),
        (Filter::Review, "Review"),
        (Filter::Done, "Done"),
        (Filter::All, "All"),
    ];

    fn takes(self, t: &Task) -> bool {
        match self {
            Filter::Queued => t.column == TaskColumn::Planned,
            Filter::Running => matches!(t.column, TaskColumn::Working | TaskColumn::Waiting),
            Filter::Review => t.column == TaskColumn::Review,
            Filter::Done => t.column == TaskColumn::Done,
            Filter::All => true,
        }
    }
}

/// The browse view is the list with its detail; the full view is the screen of the Minimal
/// layout (new task, drag to move).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browse,
    Full,
}

fn step_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "done",
        StepState::Working => "working",
        StepState::Waiting => "waits for you",
        StepState::NotStarted => "queued",
    }
}

fn step_led(s: StepState) -> u32 {
    match s {
        StepState::Done => LED_GREEN,
        StepState::Working => LED_AMBER,
        StepState::Waiting => LED_RED,
        StepState::NotStarted => LED_OFF,
    }
}

fn ask_word(k: AskKind) -> &'static str {
    match k {
        AskKind::Permission => "a permission",
        AskKind::Plan => "a plan approval",
        AskKind::Question => "an answer",
        AskKind::Error => "a look at an error",
    }
}

/// `1 m 05 s`, `42 s`.
fn span(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 60 {
        format!("{} m {:02} s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

// =============================================================================== Tasks ====

pub struct IrixTasks {
    model: Entity<Model>,
    full: Entity<TasksScreen>,
    data: Option<Rc<TaskList>>,
    filter: Filter,
    mode: Mode,
    selected: Option<String>,
    /// The growth of the cost of each task, one sample per poll.
    spend: HashMap<String, Series>,
    scroll_list: ScrollHandle,
    scroll_detail: ScrollHandle,
    busy: bool,
    _tick: GpuiTask<()>,
}

impl EventEmitter<IrixEvent> for IrixTasks {}

impl IrixTasks {
    pub fn new(model: Entity<Model>, full: Entity<TasksScreen>, cx: &mut Context<Self>) -> Self {
        let tick = ticker(cx, Duration::from_secs(5), |t: &mut Self, cx| t.refresh(cx));
        let mut this = Self {
            model,
            full,
            data: None,
            filter: Filter::Running,
            mode: Mode::Browse,
            selected: None,
            spend: HashMap::new(),
            scroll_list: ScrollHandle::new(),
            scroll_detail: ScrollHandle::new(),
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
                    for t in &list.tasks {
                        this.spend.entry(t.id.clone()).or_default().sample(t.cost);
                    }
                    this.data = Some(Rc::new(list));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn visible(&self) -> Vec<Task> {
        self.data.as_ref().map_or_else(Vec::new, |d| {
            d.tasks
                .iter()
                .filter(|t| self.filter.takes(t))
                .cloned()
                .collect()
        })
    }

    fn count(&self, f: Filter) -> usize {
        self.data
            .as_ref()
            .map_or(0, |d| d.tasks.iter().filter(|t| f.takes(t)).count())
    }

    /// Move a task to another column, then read the list again.
    fn move_to(&mut self, id: String, to: TaskColumn, cx: &mut Context<Self>) {
        let call = self.model.read(cx).call_notify::<method::TasksUpdate>(
            TaskUpdateParams {
                id,
                column: Some(to),
                ..Default::default()
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            call.await;
            let _ = this.update(cx, |this, cx| this.refresh(cx));
        })
        .detach();
    }

    fn list(&self, tasks: &[Task], cur: Option<&Task>, cx: &mut Context<Self>) -> Div {
        let mut rows: Vec<AnyElement> = Vec::new();
        for (k, t) in tasks.iter().enumerate() {
            let id = t.id.clone();
            let waits = t.waiting_for.is_some();
            let dot = if waits {
                LED_RED
            } else {
                match t.column {
                    TaskColumn::Working => LED_GREEN,
                    TaskColumn::Review => LED_AMBER,
                    _ => LED_OFF,
                }
            };
            let sub = format!(
                "{} / {} · {}",
                t.codebase,
                t.part,
                t.agents.first().cloned().unwrap_or_default()
            );
            let note = if waits {
                "waits".to_string()
            } else {
                format!("step {}", (t.step + 1).min(5))
            };
            rows.push(
                list_row(
                    format!("it-row-{k}"),
                    Some(dot),
                    &t.title,
                    &sub,
                    &note,
                    cur.is_some_and(|c| c.id == t.id),
                    waits,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .p(px(PAD))
                    .child(note("No task in this list."))
                    .into_any_element(),
            );
        }
        div().child(list_box(vec![
            div()
                .size_full()
                .child(scroll_area(
                    "it-list",
                    &self.scroll_list,
                    false,
                    div().flex().flex_col().children(rows),
                ))
                .into_any_element(),
        ]))
    }

    fn detail(&self, cur: Option<&Task>, cx: &mut Context<Self>) -> Div {
        let Some(t) = cur else {
            return detail_pane(
                "it-detail",
                &self.scroll_detail,
                div().child(note("Pick a task in the list.")),
            );
        };
        let mut body = div().flex().flex_col().gap(px(20.));
        body = body.child(section(&t.title)).child(facts(vec![
            ("Playbook", format!("{:?}", t.playbook), TEXT),
            ("Priority", format!("{:?}", t.priority), TEXT),
            ("State", t.column.label().to_string(), TEXT),
            ("Agent", t.agents.first().cloned().unwrap_or_default(), TEXT),
            ("Part", format!("{} / {}", t.codebase, t.part), TEXT),
            ("Spend", format!("${:.2}", t.cost), TEXT),
        ]));
        if let Some(kind) = t.waiting_for {
            body = body.child(
                div()
                    .text_size(ui::ts(S_ENTRY))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(ALERT))
                    .child(format!("This task waits for {}.", ask_word(kind))),
            );
        }
        // The plan: one row per step with its LED, the actor and the time.
        let mut plan = div().flex().flex_col().gap(px(2.));
        for (k, s) in t.steps.iter().enumerate() {
            let mut line = format!("{} · {}", k + 1, s.name);
            if let Some(a) = &s.actor {
                line.push_str(&format!(" · {a}"));
            }
            let time = s.duration_ms.map(span).unwrap_or_default();
            plan = plan.child(
                div()
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(led(step_led(s.state)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui::ts(S_ENTRY))
                            .text_color(rgb(TEXT))
                            .child(line),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui::ts(S_LABEL))
                            .text_color(rgb(if s.state == StepState::Waiting {
                                ALERT
                            } else {
                                MUTED
                            }))
                            .child(format!("{} {time}", step_word(s.state))),
                    ),
            );
        }
        body = body
            .child(section(&format!("Plan · {} steps", t.steps.len())))
            .child(plan);
        // Strip charts: the time of each step, and the spend per poll.
        let times: Vec<f32> = t
            .steps
            .iter()
            .map(|s| s.duration_ms.unwrap_or(0) as f32)
            .collect();
        let mut charts = div().flex().gap(px(GAP_PANEL));
        if times.iter().any(|v| *v > 0.) {
            charts = charts.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(section("Time per step"))
                    .child(strip_chart(&times, 0x3CE05A, 72., 1., "ms per step")),
            );
        }
        if let Some(series) = self.spend.get(&t.id) {
            charts = charts.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(section("Spend per poll"))
                    .child(strip_chart(
                        &series.list(),
                        0xF2B233,
                        72.,
                        0.01,
                        "USD per poll",
                    )),
            );
        }
        body = body.child(charts);
        // The sessions of the task.
        if !t.sessions.is_empty() {
            let mut rows: Vec<AnyElement> = Vec::new();
            for (k, s) in t.sessions.iter().enumerate() {
                let id = s.session.clone();
                rows.push(
                    list_row(
                        format!("it-s-{k}"),
                        Some(if s.state.is_live() {
                            LED_GREEN
                        } else {
                            LED_OFF
                        }),
                        &s.title,
                        &s.role,
                        "",
                        false,
                        false,
                    )
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(IrixEvent::OpenSession(id.clone()))),
                    )
                    .into_any_element(),
                );
            }
            body = body
                .child(section(&format!("Sessions · {}", t.sessions.len())))
                .child(sunken(div().flex().flex_col().children(rows)));
        }
        // The keys: open the session, move the task.
        let first = t.sessions.first().map(|s| s.session.clone());
        let mut keys = menu_bar().flex_wrap();
        if let Some(id) = first.clone() {
            keys = keys.child(push("it-open", "Open session", true).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(IrixEvent::OpenSession(id.clone()))),
            ));
        }
        if let Some(id) = first {
            keys = keys.child(push("it-focus", "Focus view", false).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(IrixEvent::OpenFocus(id.clone()))),
            ));
        }
        for (k, to) in t.column.moves_to().iter().enumerate() {
            let (id, to) = (t.id.clone(), *to);
            keys = keys.child(
                push(
                    format!("it-mv-{k}"),
                    &format!("Move to {}", to.label()),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.move_to(id.clone(), to, cx))),
            );
        }
        body = body.child(section("Actions")).child(keys);
        let prompt: String = t.prompt.chars().take(700).collect();
        if !prompt.is_empty() {
            body = body.child(section("Prompt")).child(note(prompt));
        }
        detail_pane("it-detail", &self.scroll_detail, body)
    }
}

impl Render for IrixTasks {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (autopilot, queued) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map_or((false, 0), |w| (w.autopilot.on, w.queued));
        let total = self.data.as_ref().map_or(0, |d| d.tasks.len());
        let waiting = self.data.as_ref().map_or(0, |d| {
            d.tasks.iter().filter(|t| t.waiting_for.is_some()).count()
        });
        let sub = format!(
            "{total} tasks · {} running · {waiting} waiting for you · {queued} queued for autopilot",
            self.count(Filter::Running)
        );
        let mut keys = menu_bar().flex_wrap();
        if self.mode == Mode::Browse {
            for (f, label) in Filter::ALL {
                keys = keys.child(
                    seg(
                        format!("it-f-{label}"),
                        &format!("{label} {}", self.count(f)),
                        self.filter == f,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = f;
                        this.selected = None;
                        cx.notify();
                    })),
                );
            }
        }
        keys = keys.child(div().flex_1());
        keys = keys
            .child(
                seg(
                    "it-autopilot",
                    if autopilot {
                        "Autopilot on"
                    } else {
                        "Autopilot off"
                    },
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
                })),
            )
            .child(
                seg("it-browse", "List", self.mode == Mode::Browse).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.mode = Mode::Browse;
                        cx.notify();
                    },
                )),
            )
            .child(
                seg("it-full", "Board", self.mode == Mode::Full).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.mode = Mode::Full;
                        cx.notify();
                    },
                )),
            );
        let body = match self.mode {
            Mode::Full => div()
                .flex_1()
                .min_h_0()
                .child(self.full.clone())
                .into_any_element(),
            Mode::Browse => {
                let tasks = self.visible();
                let cur = self
                    .selected
                    .as_ref()
                    .and_then(|id| tasks.iter().find(|t| &t.id == id))
                    .or(tasks.first())
                    .cloned();
                two_panes(
                    self.list(&tasks, cur.as_ref(), cx),
                    self.detail(cur.as_ref(), cx),
                )
                .into_any_element()
            }
        };
        page().child(heading("Tasks", &sub)).child(keys).child(body)
    }
}

// =============================================================================== Repos ====

pub struct IrixRepos {
    model: Entity<Model>,
    full: Entity<ReposScreen>,
    mode: Mode,
    selected: Option<String>,
    /// The tokens that the live sessions of each codebase use, one sample per two seconds.
    tokens: HashMap<String, Series>,
    scroll_list: ScrollHandle,
    scroll_detail: ScrollHandle,
    _obs: gpui::Subscription,
    _tick: GpuiTask<()>,
}

impl EventEmitter<IrixEvent> for IrixRepos {}

impl IrixRepos {
    pub fn new(model: Entity<Model>, full: Entity<ReposScreen>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        let tick = ticker(cx, Duration::from_secs(2), |t: &mut Self, cx| t.sample(cx));
        Self {
            model,
            full,
            mode: Mode::Browse,
            selected: None,
            tokens: HashMap::new(),
            scroll_list: ScrollHandle::new(),
            scroll_detail: ScrollHandle::new(),
            _obs: obs,
            _tick: tick,
        }
    }

    fn sample(&mut self, cx: &mut Context<Self>) {
        let Some(war) = self.model.read(cx).war.clone() else {
            return;
        };
        for c in &war.codebases {
            let total: u64 = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.state.is_live())
                .map(|s| s.tokens)
                .sum();
            self.tokens
                .entry(c.id.clone())
                .or_default()
                .sample(total as f64);
        }
        cx.notify();
    }

    fn list(&self, war: &WarRoom, cur: Option<&str>, cx: &mut Context<Self>) -> Div {
        let mut rows: Vec<AnyElement> = Vec::new();
        for (k, c) in war.codebases.iter().enumerate() {
            let id = c.id.clone();
            let live = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.state.is_live())
                .count();
            let waits = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.ask.is_some())
                .count();
            let (dot, text) = if waits > 0 {
                (LED_RED, format!("{waits} wait"))
            } else if live > 0 {
                (LED_GREEN, format!("{live} live"))
            } else {
                (LED_OFF, String::new())
            };
            rows.push(
                list_row(
                    format!("ir-row-{k}"),
                    Some(dot),
                    &c.id,
                    &format!("{} · {}", c.branch, c.path),
                    &text,
                    cur == Some(c.id.as_str()),
                    waits > 0,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .p(px(PAD))
                    .child(note("No codebase yet. The Manage key adds one."))
                    .into_any_element(),
            );
        }
        div().child(list_box(vec![
            div()
                .size_full()
                .child(scroll_area(
                    "ir-list",
                    &self.scroll_list,
                    false,
                    div().flex().flex_col().children(rows),
                ))
                .into_any_element(),
        ]))
    }

    fn detail(&self, war: &WarRoom, cur: Option<&str>, cx: &mut Context<Self>) -> Div {
        let Some(c) = cur.and_then(|id| war.codebase(id)) else {
            return detail_pane(
                "ir-detail",
                &self.scroll_detail,
                div().child(note("Pick a codebase in the list.")),
            );
        };
        let sessions: Vec<&Pane> = war
            .sessions
            .iter()
            .filter(|s| s.codebase == c.id && s.state.is_live())
            .collect();
        let spend: f64 = sessions.iter().map(|s| s.cost).sum();
        let mut body = div().flex().flex_col().gap(px(20.));
        body = body
            .child(section(&c.id))
            .child(facts(vec![
                ("Branch", c.branch.clone(), TEXT),
                ("Worktrees", c.worktrees.to_string(), TEXT),
                ("Daemon", c.daemon.clone(), TEXT),
                ("Languages", c.languages.join(", "), TEXT),
                ("Live sessions", sessions.len().to_string(), TEXT),
                ("Spend now", format!("${spend:.2}"), TEXT),
            ]))
            .child(
                div()
                    .font_family(tk().mono)
                    .text_size(ui::ts(S_ENTRY))
                    .line_height(gpui::relative(1.45))
                    .text_color(rgb(MUTED))
                    .child(c.path.clone()),
            );
        // The strip chart: tokens that the live sessions use, per two seconds.
        if let Some(series) = self.tokens.get(&c.id) {
            body = body
                .child(section("Tokens per two seconds"))
                .child(strip_chart(
                    &series.list(),
                    0x3CE05A,
                    44.,
                    1.,
                    "tokens per 2 s",
                ));
        }
        // The parts.
        body = body.child(section(&format!("Parts · {}", c.parts.len())));
        let mut parts = div().flex().flex_col();
        for p in &c.parts {
            parts = parts.child(
                div()
                    .min_h(px(36.))
                    .py(px(6.))
                    .flex()
                    .gap(px(16.))
                    .child(
                        div()
                            .w(px(150.))
                            .flex_none()
                            .truncate()
                            .text_size(ui::ts(S_ENTRY))
                            .text_color(rgb(TEXT))
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(tk().mono)
                            .text_size(ui::ts(S_LABEL))
                            .line_height(gpui::relative(1.45))
                            .text_color(rgb(MUTED))
                            .child(p.paths.join(", ")),
                    ),
            );
        }
        if c.parts.is_empty() {
            parts = parts.child(note("This codebase has no part yet."));
        }
        body = body.child(parts);
        // The live sessions.
        body = body.child(section(&format!("Live sessions · {}", sessions.len())));
        let mut rows: Vec<AnyElement> = Vec::new();
        for (k, s) in sessions.iter().enumerate() {
            let id = s.id.clone();
            rows.push(
                list_row(
                    format!("ir-s-{k}"),
                    Some(led_for(s)),
                    &s.title,
                    &format!("{} · {}", s.provider, s.model),
                    if s.ask.is_some() { "waits" } else { "" },
                    false,
                    s.ask.is_some(),
                )
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(IrixEvent::OpenSession(id.clone()))),
                )
                .into_any_element(),
            );
        }
        if rows.is_empty() {
            body = body.child(note("Nobody works here now."));
        } else {
            body = body.child(sunken(div().flex().flex_col().children(rows)));
        }
        // The keys.
        let target = sessions
            .iter()
            .find(|s| s.ask.is_some())
            .or(sessions.first())
            .map(|s| s.id.clone());
        let mut keys = menu_bar().flex_wrap();
        if let Some(id) = target.clone() {
            keys = keys.child(push("ir-open", "Open session", true).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(IrixEvent::OpenSession(id.clone()))),
            ));
        }
        if let Some(id) = target {
            keys = keys.child(push("ir-focus", "Focus view", false).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(IrixEvent::OpenFocus(id.clone()))),
            ));
        }
        keys = keys.child(
            push("ir-war", "War room", false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(IrixEvent::WarRoom))),
        );
        body = body.child(section("Actions")).child(keys);
        detail_pane("ir-detail", &self.scroll_detail, body)
    }
}

impl Render for IrixRepos {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let sub = war.as_ref().map_or("Waits for aivoryd".to_string(), |w| {
            let n = w.counts();
            format!(
                "{} codebases · {} live sessions · {} waiting for you",
                w.codebases.len(),
                n.live,
                n.needs_you
            )
        });
        let keys = menu_bar()
            .child(div().flex_1())
            .child(
                seg("ir-browse", "Browse", self.mode == Mode::Browse).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.mode = Mode::Browse;
                        cx.notify();
                    },
                )),
            )
            .child(
                seg("ir-full", "Manage", self.mode == Mode::Full).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.mode = Mode::Full;
                        cx.notify();
                    },
                )),
            );
        let body = match (self.mode, war) {
            (Mode::Full, _) => div()
                .flex_1()
                .min_h_0()
                .child(self.full.clone())
                .into_any_element(),
            (Mode::Browse, None) => div()
                .child(note("The war room has no data yet."))
                .into_any_element(),
            (Mode::Browse, Some(war)) => {
                let cur: Option<String> = self
                    .selected
                    .clone()
                    .filter(|id| war.codebase(id).is_some())
                    .or_else(|| war.codebases.first().map(|c| c.id.clone()));
                two_panes(
                    self.list(&war, cur.as_deref(), cx),
                    self.detail(&war, cur.as_deref(), cx),
                )
                .into_any_element()
            }
        };
        page().child(heading("Repos", &sub)).child(keys).child(body)
    }
}

// ============================================================================ the shell ====

/// The Navigator screens that the shell keeps: made when they first show.
#[derive(Default)]
pub(super) struct IrixScreens {
    tasks: Option<(Entity<IrixTasks>, gpui::Subscription)>,
    repos: Option<(Entity<IrixRepos>, gpui::Subscription)>,
    pub(super) focus: Option<(Entity<super::irix_focus::IrixFocus>, gpui::Subscription)>,
}

impl super::Shell {
    fn irix_event(&mut self, event: &IrixEvent, cx: &mut Context<Self>) {
        match event {
            IrixEvent::OpenSession(id) => self.open_session(id, cx),
            IrixEvent::OpenFocus(id) => self.open_focus(id, cx),
            IrixEvent::WarRoom => self.go(super::Screen::WarRoom, cx),
        }
    }

    /// The Navigator Tasks screen.
    pub(super) fn irix_tasks_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.irix_screens.tasks.is_none() {
            let (model, full) = (self.model.clone(), self.tasks.clone());
            let view = cx.new(|cx| IrixTasks::new(model, full, cx));
            let sub = cx.subscribe(&view, |this, _, e: &IrixEvent, cx| this.irix_event(e, cx));
            self.irix_screens.tasks = Some((view, sub));
        }
        match &self.irix_screens.tasks {
            Some((view, _)) => view.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The Navigator Repos screen.
    pub(super) fn irix_repos_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.irix_screens.repos.is_none() {
            let (model, full) = (self.model.clone(), self.repos.clone());
            let view = cx.new(|cx| IrixRepos::new(model, full, cx));
            let sub = cx.subscribe(&view, |this, _, e: &IrixEvent, cx| this.irix_event(e, cx));
            self.irix_screens.repos = Some((view, sub));
        }
        match &self.irix_screens.repos {
            Some((view, _)) => view.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The Navigator Focus view of the session on screen.
    pub(super) fn irix_focus_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::irix_focus::{IrixFocus, IrixFocusEvent};
        let super::Screen::Focus(id) = &self.screen else {
            return div().into_any_element();
        };
        let id = id.clone();
        if let Some((view, _)) = &self.irix_screens.focus {
            if view.read(cx).session() != id.as_str() {
                view.update(cx, |v, cx| v.show(&id, cx));
            }
            return view.clone().into_any_element();
        }
        let model = self.model.clone();
        let view = cx.new(|cx| IrixFocus::new(model, id, window, cx));
        let sub = cx.subscribe(&view, |this, _, e: &IrixFocusEvent, cx| match e {
            IrixFocusEvent::Open(id) => this.open_focus(id, cx),
            IrixFocusEvent::Detail(id) => this.open_session(id, cx),
            IrixFocusEvent::Back => this.go(super::Screen::WarRoom, cx),
            IrixFocusEvent::OpenFile { session, path } => this.open_session_file(session, path, cx),
        });
        self.irix_screens.focus = Some((view.clone(), sub));
        view.into_any_element()
    }
}
