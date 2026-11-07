//! The Holo screens Inbox, Tasks, Repos and Settings. Each is a title with readouts on top and
//! two glass panels under it: the left panel lists (with a menu of tabs), the right panel shows
//! the one selected thing. They follow the structure of the Bridge screens (title, menu, list,
//! detail) in the grammar of the Holo boards: calm glass, cyan light, orange only for a wait.
//! `island` in `holo_kit` draws the frame; a view only fills the content.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{InboxListParams, TaskListParams, method};
use aivory_protocol::{
    InboxFilter, InboxItem, InboxKind, InboxList, Pane, StepState, Task, TaskColumn, TaskList,
    rpc::AutopilotPatch,
};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Entity, EventEmitter, Task as GpuiTask, Window, div, px, rgb};

use crate::actions::{Action, act};
use crate::holo_kit::*;
use crate::model::Model;

/// What a screen asks the shell to do.
#[derive(Debug, Clone)]
pub enum HoloEvent {
    OpenSession(String),
    /// The full Settings screen.
    AllSettings,
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

pub(crate) fn minutes(now: i64, since: i64) -> String {
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

pub struct HoloInbox {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    tab: InboxTab,
    side: Side,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: gpui::Subscription,
}

impl EventEmitter<HoloEvent> for HoloInbox {}

impl HoloInbox {
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

    /// The items of the tab, and the one on screen.
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
        InboxKind::Error => GD,
        _ => RS,
    }
}

impl Render for HoloInbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let items = self.items();
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| items.iter().find(|i| &i.id == id))
            .or(items.first())
            .cloned();
        let all = self.data.as_ref().map_or(0, |d| d.items.len());
        let count = |f: InboxFilter| {
            self.data
                .as_ref()
                .map_or(0, |d| d.items.iter().filter(|i| f.matches(i.kind)).count())
        };
        let done = self.data.as_ref().map_or(0, |d| d.done.len());
        let oldest = self
            .data
            .as_ref()
            .and_then(|d| d.items.iter().map(|i| i.since).min())
            .map_or("none".to_string(), |s| minutes(now, s));
        let stats = vec![
            (
                "Waiting for you".to_string(),
                format!("{} decisions", count(InboxFilter::Answers)),
                RS,
            ),
            (
                "Errors".to_string(),
                count(InboxFilter::Errors).to_string(),
                GD,
            ),
            ("Oldest".to_string(), oldest, TXT),
            ("Done today".to_string(), done.to_string(), TXT),
        ];

        let mut left_tabs: Vec<AnyElement> = Vec::new();
        for (t, label, color) in [
            (InboxTab::All, format!("All {all:02}"), RS),
            (
                InboxTab::Waiting,
                format!("Waiting {:02}", count(InboxFilter::Answers)),
                RS,
            ),
            (
                InboxTab::Errors,
                format!("Errors {:02}", count(InboxFilter::Errors)),
                GD,
            ),
            (
                InboxTab::Reviews,
                format!("Reviews {:02}", count(InboxFilter::Reviews)),
                SA,
            ),
            (InboxTab::Done, format!("Done {done:02}"), SA),
        ] {
            left_tabs.push(
                tab(format!("li-tab-{label}"), &label, color, self.tab == t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = t;
                        this.selected = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }

        // The left island: the decision on screen, then the next in line.
        let mut left = div().flex().flex_col();
        let mut right = div().flex().flex_col().gap(px(16.));
        let mut right_title = "Detail".to_string();
        if let Some(it) = &cur {
            let at = items.iter().position(|i| i.id == it.id).unwrap_or(0);
            left = left
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            sec(format!(
                                "{} {:02} of {:02} · {}",
                                if self.tab == InboxTab::Done {
                                    "Done"
                                } else {
                                    "Decision"
                                },
                                at + 1,
                                items.len(),
                                it.kind.label().to_lowercase()
                            ))
                            .text_color(rgb(kind_color(it.kind))),
                        )
                        .child(th(format!("Waits {}", minutes(now, it.since)))),
                )
                .child(big(&it.title).mt(px(12.)))
                .child(
                    div()
                        .mt(px(6.))
                        .font_family(BODY)
                        .text_size(px(S_SMALL))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "{} · {} / {}",
                            it.agents
                                .first()
                                .map_or(String::new(), |a| a.provider.clone()),
                            it.codebase,
                            it.part
                        )),
                );
            let mut pills = div().mt(px(22.)).flex().gap(px(8.));
            if let Some(result) = &it.result {
                pills = pills.child(th(format!("Done · {result}")));
            } else {
                let n = it.options.len();
                for (k, o) in it.options.iter().enumerate() {
                    let (id, opt) = (it.id.clone(), o.clone());
                    pills = pills.child(
                        pill(
                            format!("li-opt-{k}"),
                            option_color(k, n),
                            &option_text(k, o),
                        )
                        .px(px(18.))
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
            left = left.child(pills);
            let rest: Vec<&InboxItem> = items.iter().filter(|i| i.id != it.id).collect();
            if !rest.is_empty() {
                let mut list = div()
                    .mt(px(PAD))
                    .flex()
                    .flex_col()
                    .child(sec("Next in line").mb(px(12.)));
                for (k, o) in rest.iter().enumerate() {
                    let id = o.id.clone();
                    list = list.child(
                        row(
                            format!("li-row-{k}"),
                            &format!(
                                "{:02}",
                                items.iter().position(|i| i.id == o.id).unwrap_or(0) + 1
                            ),
                            kind_color(o.kind),
                            &o.title,
                            &format!("{} · {}", o.codebase, o.kind.label()),
                            &minutes(now, o.since),
                            kind_color(o.kind),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(id.clone());
                            cx.notify();
                        })),
                    );
                }
                left = left.child(list);
            }

            // The right island.
            right_title = it.kind.heading().to_string();
            match self.side {
                Side::Details => {
                    right = right
                        .child(sec("Why it asks"))
                        .child(prose(if it.preview.is_empty() {
                            &it.text
                        } else {
                            &it.preview
                        }))
                        .child(sec("Command or question"))
                        .child(code(&it.text, RS));
                    if !it.steps.is_empty() {
                        right = right.child(sec("Plan")).children(
                            it.steps
                                .iter()
                                .enumerate()
                                .map(|(k, s)| prose(&format!("{}. {s}", k + 1))),
                        );
                    }
                }
                Side::Files => {
                    right = right.child(sec(format!("Files changed · {}", it.files.len())));
                    for f in it.files.iter().take(12) {
                        right = right.child(code(f, TXT));
                    }
                }
                Side::Chat => {
                    right = right.child(sec("Last rows of the chat"));
                    for r in chat_text(&it.tail, 8) {
                        right = right.child(prose(&r));
                    }
                }
            }
        } else {
            left = left.child(big("Nothing waits for you"));
        }
        let open = cur.as_ref().map(|i| i.session.clone());
        let mut right_tabs: Vec<AnyElement> = Vec::new();
        for (s, label) in [
            (Side::Details, "Details"),
            (Side::Files, "Files"),
            (Side::Chat, "Chat"),
        ] {
            right_tabs.push(
                tab(format!("ri-tab-{label}"), label, PK, self.side == s)
                    .h(px(40.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.side = s;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        right_tabs.push(
            tab("ri-open", "Open session ›", PK, false)
                .h(px(40.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &open {
                        cx.emit(HoloEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
        );
        island(
            art,
            Island {
                stats,
                left_title: "Inbox",
                left_color: if count(InboxFilter::Answers) > 0 {
                    RS
                } else {
                    PK
                },
                left_tabs,
                left: left.into_any_element(),
                right_title,
                right_tabs,
                right: right.into_any_element(),
            },
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

pub struct HoloTasks {
    model: Entity<Model>,
    data: Option<Rc<TaskList>>,
    tab: TaskTab,
    side: TaskSide,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
}

impl EventEmitter<HoloEvent> for HoloTasks {}

impl HoloTasks {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
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
        StepState::Done => PE,
        StepState::Working => PK,
        StepState::Waiting => RS,
        StepState::NotStarted => GY,
    }
}

fn step_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "Done",
        StepState::Working => "Working",
        StepState::Waiting => "Waits for you",
        StepState::NotStarted => "Queued",
    }
}

impl Render for HoloTasks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let (autopilot, queued_n) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map_or((false, 0), |w| (w.autopilot.on, w.queued));
        let n = |t| self.of(t).len();
        let stats = vec![
            ("Queued".to_string(), n(TaskTab::Queued).to_string(), TXT),
            (
                "Running".to_string(),
                format!("{} tasks", n(TaskTab::Running)),
                PK,
            ),
            ("In review".to_string(), n(TaskTab::Review).to_string(), GD),
            ("Done".to_string(), n(TaskTab::Done).to_string(), TXT),
        ];
        let mut left_tabs: Vec<AnyElement> = Vec::new();
        for (t, label, color) in [
            (TaskTab::Queued, "Queued", PK),
            (TaskTab::Running, "Running", OR),
            (TaskTab::Review, "Review", GD),
            (TaskTab::Done, "Done", SA),
        ] {
            left_tabs.push(
                tab(
                    format!("lt-tab-{label}"),
                    &format!("{label} {:02}", n(t)),
                    color,
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
        let tasks = self.of(self.tab);
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .or(tasks.first())
            .cloned();
        let mut left = div().flex().flex_col().child(
            sec(format!(
                "{} · {} tasks",
                ["Queued", "Running", "Review", "Done"][self.tab as usize],
                tasks.len()
            ))
            .mb(px(12.)),
        );
        for (k, t) in tasks.iter().enumerate() {
            let id = t.id.clone();
            let c = if t.waiting_for.is_some() { RS } else { PK };
            let mut r = row(
                format!("lt-row-{k}"),
                &format!("{:02}", k + 1),
                c,
                &t.title,
                &format!(
                    "{} · {}",
                    t.agents.first().cloned().unwrap_or_default(),
                    t.codebase
                ),
                &format!("step {}", (t.step + 1).min(5)),
                c,
                cur.as_ref().is_some_and(|x| x.id == t.id),
            );
            r = r.on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id.clone());
                cx.notify();
            }));
            left = left.child(r);
        }
        left = left.child(
            div()
                .mt(px(22.))
                .flex()
                .gap(px(8.))
                .child(
                    pill(
                        "lt-autopilot",
                        PK,
                        if autopilot {
                            "Autopilot on"
                        } else {
                            "Autopilot off"
                        },
                    )
                    .px(px(18.))
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
                .child(th(format!("{queued_n} queued for autopilot")).mt(px(6.))),
        );

        let mut right = div().flex().flex_col().gap(px(14.));
        let mut open: Option<String> = None;
        if let Some(t) = &cur {
            open = t.sessions.first().map(|s| s.session.clone());
            right = right.child(big(&t.title)).child(th(format!(
                "{} · {} / {} · ${:.2}",
                t.agents.first().cloned().unwrap_or_default(),
                t.codebase,
                t.part,
                t.cost
            )));
            match self.side {
                TaskSide::Plan => {
                    right = right.child(sec(format!("Plan · {} steps", t.steps.len())).mt(px(16.)));
                    for (k, s) in t.steps.iter().enumerate() {
                        right = right.child(
                            div()
                                .flex_none()
                                .min_h(px(44.))
                                .py(px(6.))
                                .border_b_1()
                                .border_color(ink(PK, 0.1))
                                .flex()
                                .items_center()
                                .gap(px(14.))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(26.))
                                        .h(px(26.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(rgb(step_color(s.state)))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .font_family(LABEL)
                                        .text_size(px(S_SMALL))
                                        .text_color(rgb(step_color(s.state)))
                                        .child((k + 1).to_string()),
                                )
                                .child(div().flex_1().child(prose(&s.name)))
                                .child(
                                    div()
                                        .font_family(LABEL)
                                        .text_size(px(S_SMALL))
                                        .text_color(rgb(if s.state == StepState::Waiting {
                                            RS
                                        } else {
                                            MUTED
                                        }))
                                        .child(step_word(s.state).to_uppercase()),
                                ),
                        );
                    }
                }
                TaskSide::Sessions => {
                    right =
                        right.child(sec(format!("Sessions · {}", t.sessions.len())).mt(px(16.)));
                    for (k, s) in t.sessions.iter().enumerate() {
                        let id = s.session.clone();
                        right = right.child(
                            row(
                                format!("lt-s-{k}"),
                                &format!("{:02}", k + 1),
                                PK,
                                &s.title,
                                &s.role,
                                "",
                                MUTED,
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| cx.emit(HoloEvent::OpenSession(id.clone())),
                            )),
                        );
                    }
                }
            }
        } else {
            right = right.child(big("No task here"));
        }
        let mut right_tabs: Vec<AnyElement> = Vec::new();
        for (s, label) in [(TaskSide::Plan, "Plan"), (TaskSide::Sessions, "Sessions")] {
            right_tabs.push(
                tab(format!("rt-tab-{label}"), label, PK, self.side == s)
                    .h(px(40.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.side = s;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        right_tabs.push(
            tab("rt-open", "Open session ›", PK, false)
                .h(px(40.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &open {
                        cx.emit(HoloEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
        );
        island(
            art,
            Island {
                stats,
                left_title: "Tasks",
                left_color: PK,
                left_tabs,
                left: left.into_any_element(),
                right_title: "Task".to_string(),
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}

// =============================================================================== Repos ====

pub struct HoloRepos {
    model: Entity<Model>,
    selected: Option<String>,
    _obs: gpui::Subscription,
}

impl EventEmitter<HoloEvent> for HoloRepos {}

impl HoloRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: None,
            _obs: obs,
        }
    }
}

impl Render for HoloRepos {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
        let Some(war) = self.model.read(cx).war.clone() else {
            return island(
                art,
                Island {
                    stats: Vec::new(),
                    left_title: "Repos",
                    left_color: PK,
                    left_tabs: Vec::new(),
                    left: big("No data yet").into_any_element(),
                    right_title: "Codebase".into(),
                    right_tabs: Vec::new(),
                    right: div().into_any_element(),
                },
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
        let found: Option<&aivory_protocol::warroom::Codebase> = self
            .selected
            .as_ref()
            .and_then(|id| war.codebase(id))
            .or(war.codebases.first());
        let cur = found.cloned();
        let stats = vec![
            (
                "Codebases".to_string(),
                war.codebases.len().to_string(),
                TXT,
            ),
            (
                "Live sessions".to_string(),
                war.counts().live.to_string(),
                PK,
            ),
            (
                "Waiting".to_string(),
                war.counts().needs_you.to_string(),
                RS,
            ),
            (
                "Worktrees".to_string(),
                war.codebases
                    .iter()
                    .map(|c| c.worktrees)
                    .sum::<u32>()
                    .to_string(),
                TXT,
            ),
        ];
        let mut left = div()
            .flex()
            .flex_col()
            .child(sec("Codebases · click to show").mb(px(12.)));
        // The height of a panel body at this window height; a row is 60 px. Whole rows only,
        // then a "+N more" line (rule 14).
        let body = f32::from(window.viewport_size().height) - 278.;
        let n = war.codebases.len();
        let room = ((body - 40.) / 60.).floor().max(1.) as usize;
        let shown = if n <= room {
            n
        } else {
            room.saturating_sub(1).max(1)
        };
        for (k, c) in war.codebases.iter().take(shown).enumerate() {
            let id = c.id.clone();
            let w = waits(&c.id);
            left = left.child(
                row(
                    format!("lr-row-{k}"),
                    &format!("{:02}", k + 1),
                    if w > 0 { RS } else { PK },
                    &c.id,
                    &format!("{} · {}", c.branch, c.path),
                    &format!("{} live", live(&c.id)),
                    if w > 0 { RS } else { MUTED },
                    cur.as_ref().is_some_and(|x| x.id == c.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        if n > shown {
            left = left.child(th(format!("+ {} more", n - shown)).mt(px(12.)));
        }
        let mut right = div().flex().flex_col().gap(px(14.));
        let mut open: Option<String> = None;
        if let Some(c) = &cur {
            open = war
                .sessions
                .iter()
                .find(|s| s.codebase == c.id && s.state.is_live())
                .map(|s| s.id.clone());
            right = right
                .child(big(&c.id))
                .child(th(format!(
                    "{} · {} worktrees · {}",
                    c.branch, c.worktrees, c.daemon
                )))
                .child(code(&c.path, TXT))
                .child(sec(format!("Parts · {}", c.parts.len())).mt(px(16.)));
            let parts_shown = c.parts.len().min(3);
            for p in c.parts.iter().take(parts_shown) {
                right = right.child(prose(&format!("{} · {}", p.name, p.paths.join(", "))));
            }
            if c.parts.len() > parts_shown {
                right = right.child(th(format!("+ {} more parts", c.parts.len() - parts_shown)));
            }
            let sessions: Vec<&Pane> = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.state.is_live())
                .collect();
            right = right.child(sec(format!("Live sessions · {}", sessions.len())).mt(px(16.)));
            // What is left of the body after the fixed blocks, in rows of 74 px.
            let used = 215.
                + parts_shown as f32 * 40.
                + if c.parts.len() > parts_shown { 40. } else { 0. };
            let room = ((body - used) / 74.).floor().max(1.) as usize;
            let shown = if sessions.len() <= room {
                sessions.len()
            } else {
                room.saturating_sub(1).max(1)
            };
            for (k, s) in sessions.iter().take(shown).enumerate() {
                let id = s.id.clone();
                right = right.child(
                    row(
                        format!("lr-s-{k}"),
                        &format!("{:02}", k + 1),
                        state_color(s),
                        &s.title,
                        &s.provider,
                        "",
                        MUTED,
                        false,
                    )
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(HoloEvent::OpenSession(id.clone()))),
                    ),
                );
            }
            if sessions.len() > shown {
                right = right.child(th(format!("+ {} more sessions", sessions.len() - shown)));
            }
        }
        let right_tabs = vec![
            tab("rr-open", "Open session ›", PK, false)
                .h(px(40.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(id) = &open {
                        cx.emit(HoloEvent::OpenSession(id.clone()));
                    }
                }))
                .into_any_element(),
        ];
        island(
            art,
            Island {
                stats,
                left_title: "Repos",
                left_color: PK,
                left_tabs: Vec::new(),
                left: left.into_any_element(),
                right_title: "Codebase".into(),
                right_tabs,
                right: right.into_any_element(),
            },
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

/// What a look is, in two sentences, for the Settings detail.
fn look_text(look: aivory_protocol::Look) -> &'static str {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => {
            "The calm default: a rail on the left, plain panels, no theme. Best for long work days and small windows."
        }
        Look::Bridge => {
            "Frame bars and islands on black, numbered navigation. Informative and never overwhelming, with wide gaps between the islands."
        }
        Look::Operator => {
            "A wall of agents as rain and console text, with blue screens for facts and decisions. A busy agent rains faster."
        }
        Look::Neon => {
            "A neon city: a district per codebase, a tower per session. Panels float over the city and glow only when something needs you."
        }
        Look::Navigator => {
            "A 3D file-system view with an IRIX toolchest. Groups open as camera walls; decisions show as notifier dialogs."
        }
        Look::Holo => {
            "See-through glass panels over slow rings and a bloom of strands. One decision at a time, cyan for work, orange for a wait."
        }
        Look::Tactical => {
            "A phosphor radar: the bearing is the codebase, the range is the time since the last move. White brackets mark a session that awaits orders."
        }
        Look::Command => {
            "A command console: a map of units per codebase and a command sidebar. Amber marks a unit that waits for you."
        }
    }
}

pub struct HoloSettings {
    model: Entity<Model>,
    section: Section,
    /// The look shown in the detail; the look in use when `None`.
    picked: Option<aivory_protocol::Look>,
    _obs: gpui::Subscription,
}

impl EventEmitter<HoloEvent> for HoloSettings {}

impl HoloSettings {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            section: Section::Looks,
            picked: None,
            _obs: obs,
        }
    }
}

impl Render for HoloSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let art = Art::of(window);
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
                "Look".to_string(),
                crate::look::name(crate::theme::tk().look).to_string(),
                PK,
            ),
            (
                "Autopilot".to_string(),
                if auto { "on" } else { "off" }.to_string(),
                TXT,
            ),
            ("Running at most".to_string(), max.to_string(), TXT),
            ("Parks after".to_string(), format!("{park} min"), TXT),
        ];
        let mut left_tabs: Vec<AnyElement> = Vec::new();
        for (s, label) in [
            (Section::Looks, "Looks"),
            (Section::Autopilot, "Autopilot"),
            (Section::Providers, "Providers"),
        ] {
            left_tabs.push(
                tab(format!("ls-tab-{label}"), label, PK, self.section == s)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.section = s;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let mut left = div().flex().flex_col();
        let mut right = div().flex().flex_col().gap(px(14.));
        match self.section {
            Section::Looks => {
                left = left.child(sec("Looks · click to read").mb(px(12.)));
                let now = crate::theme::tk().look;
                let shown = self.picked.unwrap_or(now);
                for (k, (look, name)) in crate::look::LOOKS.into_iter().enumerate() {
                    let used = look == now;
                    left = left.child(
                        row(
                            format!("ls-look-{k}"),
                            &format!("{:02}", k + 1),
                            if used { PK } else { GY },
                            name,
                            "",
                            if used { "In use" } else { "" },
                            PK,
                            look == shown,
                        )
                        .min_h(px(40.))
                        .py(px(4.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.picked = Some(look);
                            cx.notify();
                        })),
                    );
                }
                let name = crate::look::name(shown);
                right = right
                    .child(big(name))
                    .child(th(if shown == now {
                        "In use now"
                    } else {
                        "Not in use"
                    }))
                    .child(prose(look_text(shown)))
                    .child(prose(
                        "A look replaces the frame, the colors and the fonts of every screen. The choice is saved in the settings of the daemon.",
                    ));
                if shown != now {
                    right = right.child(div().mt(px(8.)).child(
                        button("ls-switch", Tone::Primary, &format!("Switch to {name}")).on_click(
                            cx.listener(move |this, _, _, cx| {
                                act(&this.model, Action::SetLook(shown), cx);
                                this.picked = None;
                                cx.notify();
                            }),
                        ),
                    ));
                }
            }
            Section::Autopilot => {
                left = left.child(sec("Autopilot").mb(px(12.)));
                left = left.child(
                    div()
                        .mt(px(14.))
                        .flex()
                        .gap(px(8.))
                        .child(
                            pill(
                                "ls-auto",
                                if auto { PK } else { GY },
                                if auto {
                                    "Autopilot on"
                                } else {
                                    "Autopilot off"
                                },
                            )
                            .px(px(18.))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            on: Some(!auto),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                },
                            )),
                        )
                        .child(
                            pill("ls-less", PK, "Fewer")
                                .px(px(18.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            max_running: Some(max.saturating_sub(1).max(1)),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            pill("ls-more", PK, "More")
                                .px(px(18.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    act(
                                        &this.model,
                                        Action::Autopilot(AutopilotPatch {
                                            max_running: Some((max + 1).min(64)),
                                            ..AutopilotPatch::default()
                                        }),
                                        cx,
                                    );
                                })),
                        ),
                );
                right = right
                    .child(big(&format!("Up to {max} sessions run at once")))
                    .child(prose(&format!(
                        "Autopilot is {}. A session parks after {park} minutes without work.",
                        if auto { "on" } else { "off" }
                    )));
            }
            Section::Providers => {
                left = left.child(sec("Usage windows").mb(px(12.)));
                if let Some(w) = &war {
                    for (k, u) in w.usage.iter().enumerate() {
                        left = left.child(row(
                            format!("ls-use-{k}"),
                            &format!("{:02}", k + 1),
                            if u.pct >= 80 { RS } else { PK },
                            &format!("{} {}", u.provider, u.label),
                            if u.estimate {
                                "estimate"
                            } else {
                                "from the provider"
                            },
                            &format!("{}%", u.pct),
                            if u.pct >= 80 { RS } else { TXT },
                            false,
                        ));
                    }
                }
                right = right.child(big("Providers and models")).child(prose(
                    "Models, effort and permission mode per provider are set in the full settings.",
                ));
            }
        }
        let right_tabs = vec![
            tab("rs-all", "All settings ›", PK, false)
                .h(px(40.))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(HoloEvent::AllSettings)))
                .into_any_element(),
        ];
        island(
            art,
            Island {
                stats,
                left_title: "Settings",
                left_color: PK,
                left_tabs,
                left: left.into_any_element(),
                right_title: "Detail".into(),
                right_tabs,
                right: right.into_any_element(),
            },
        )
    }
}
