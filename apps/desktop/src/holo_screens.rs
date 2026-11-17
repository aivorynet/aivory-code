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
use crate::bridge_screens::{TaskAct, task_act};
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
        InboxKind::Review => SA,
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
                    .child(sec("Next in line"))
                    .child(
                        th("Dot: orange waits for you · warm is an error · violet is a review")
                            .mb(px(12.)),
                    );
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
                    if !it.preview.is_empty() && it.preview.trim() != it.text.trim() {
                        right = right.child(sec("Why it asks")).child(prose(&it.preview));
                    }
                    right = right
                        .child(sec("Command or question"))
                        .child(code(&it.text, kind_color(it.kind)));
                    let recent = chat_text(&it.tail, 2);
                    if !recent.is_empty() {
                        right = right.child(sec("Latest in the chat"));
                        for r in recent {
                            right =
                                right.child(prose(&r).max_h(px(LH_BODY * 2.)).overflow_hidden());
                        }
                    }
                    if !it.steps.is_empty() {
                        right = right.child(sec("Plan")).children(
                            it.steps
                                .iter()
                                .enumerate()
                                .map(|(k, s)| prose(&format!("{}. {s}", k + 1))),
                        );
                    }
                    // The lower half: the numbers of the item as glass tiles, then the files
                    // and the last tool calls in mono on a dim inset (R3).
                    let tile = |label: &str, value: String, accent: u32| {
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(70.))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(2.))
                            .px(px(14.))
                            .rounded(px(10.))
                            .border_1()
                            .border_color(ink(accent, 0.45))
                            .bg(gpui::linear_gradient(
                                160.,
                                gpui::linear_color_stop(ink(0xBFEFFF, 0.14), 0.),
                                gpui::linear_color_stop(ink(0x1A5E92, 0.10), 1.),
                            ))
                            .child(th(label.to_string()))
                            .child(
                                div()
                                    .truncate()
                                    .font_family(LABEL)
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_size(px(S_BIG))
                                    .text_color(rgb(PA))
                                    .child(value),
                            )
                    };
                    right = right.child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(tile("Waits", minutes(now, it.since), OR))
                            .child(tile("Cost", format!("${:.2}", it.cost), PK))
                            .child(tile("Context", format!("{}%", it.context_pct), PE))
                            .child(tile("Files", it.files.len().to_string(), PK)),
                    );
                    let mut inset = div()
                        .flex_1()
                        .min_h(px(120.))
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .p(px(14.))
                        .rounded(px(8.))
                        .bg(ink(0x010A18, 0.6))
                        .border_1()
                        .border_color(ink(PK, 0.22))
                        .border_l_2()
                        .border_color(ink(PK, 0.5))
                        .font_family(CODE)
                        .text_size(px(S_CODE));
                    let mut any = false;
                    for f in it.files.iter().take(4) {
                        any = true;
                        inset = inset.child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .child(div().flex_none().text_color(rgb(PE)).child("edit"))
                                .child(div().truncate().text_color(rgb(PA)).child(f.clone())),
                        );
                    }
                    for l in it
                        .tail
                        .iter()
                        .filter(|l| l.kind == aivory_protocol::LineKind::Tool)
                        .rev()
                        .take(3)
                    {
                        any = true;
                        inset = inset.child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .child(
                                    div().flex_none().text_color(rgb(PK)).child(
                                        l.label
                                            .clone()
                                            .unwrap_or_else(|| "tool".into())
                                            .to_lowercase(),
                                    ),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_color(ink(PA, 0.8))
                                        .child(l.text.lines().next().unwrap_or("").to_string()),
                                ),
                        );
                    }
                    if !any {
                        inset = inset.child(
                            div()
                                .text_color(rgb(MUTED))
                                .child("$ no file edit or tool call yet"),
                        );
                    }
                    right = right.child(sec("Files and last calls")).child(inset);
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
            link("ri-open", "Open session ›", PK)
                .ml_auto()
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
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .child(
                    pill(
                        "lt-autopilot",
                        PK,
                        if autopilot {
                            "Turn autopilot off"
                        } else {
                            "Turn autopilot on"
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
                .child(th(format!(
                    "Autopilot is {} · {queued_n} queued",
                    if autopilot { "on" } else { "off" }
                ))),
        );
        {
            let all: Vec<Task> = [
                TaskTab::Queued,
                TaskTab::Running,
                TaskTab::Review,
                TaskTab::Done,
            ]
            .iter()
            .flat_map(|t| self.of(*t))
            .collect();
            let spent: f64 = all.iter().map(|t| t.cost).sum();
            let asking = all.iter().filter(|t| t.waiting_for.is_some()).count();
            left = left.child(
                div().mt(px(14.)).child(
                    th(format!(
                        "{} tasks · ${spent:.2} spent · {asking} wait for you",
                        all.len()
                    ))
                    .text_color(rgb(PA)),
                ),
            );
        }

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
                    let mut strip = div().flex_none().flex().gap(px(10.));
                    for (k, s) in t.steps.iter().enumerate() {
                        let c = step_color(s.state);
                        strip = strip.child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .px(px(12.))
                                .py(px(10.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(ink(c, 0.5))
                                .bg(gpui::linear_gradient(
                                    160.,
                                    gpui::linear_color_stop(ink(0xBFEFFF, 0.14), 0.),
                                    gpui::linear_color_stop(ink(0x1A5E92, 0.10), 1.),
                                ))
                                .child(
                                    div()
                                        .font_family(LABEL)
                                        .text_size(px(S_SMALL))
                                        .text_color(rgb(c))
                                        .child(format!(
                                            "{} · {}",
                                            k + 1,
                                            step_word(s.state).to_uppercase()
                                        )),
                                )
                                .child(prose(&s.name)),
                        );
                    }
                    right = right.child(strip);
                    if !t.prompt.trim().is_empty() {
                        right = right
                            .child(sec("Now").mt(px(10.)))
                            .child(prose(&t.prompt).max_h(px(LH_BODY * 2.)).overflow_hidden());
                    }
                    // The newest chat rows of the first session fill the rest of the panel.
                    let rows = t
                        .sessions
                        .first()
                        .and_then(|s| {
                            self.model
                                .read(cx)
                                .war
                                .as_ref()
                                .and_then(|w| w.session(&s.session))
                                .map(|p| chat_text(&p.tail, 4))
                        })
                        .unwrap_or_default();
                    if !rows.is_empty() {
                        right = right.child(sec("Latest output").mt(px(10.)));
                        for r in rows {
                            right =
                                right.child(prose(&r).max_h(px(LH_BODY * 2.)).overflow_hidden());
                        }
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
        let has_act = cur
            .as_ref()
            .is_some_and(|t| TaskAct::of(t.column).is_some());
        if let Some((t, a)) = cur
            .as_ref()
            .and_then(|t| TaskAct::of(t.column).map(|a| (t, a)))
        {
            let id = t.id.clone();
            right_tabs.push(
                link("rt-act", &format!("{} ›", a.label()), GD)
                    .ml_auto()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        task_act(&this.model, &id, a, Self::reload, cx);
                    }))
                    .into_any_element(),
            );
        }
        right_tabs.push(
            link("rt-open", "Open session ›", PK)
                .when(!has_act, |l| l.ml_auto())
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
    /// The "+N more" lines are opened: codebases, parts, sessions.
    more: [bool; 3],
    _obs: gpui::Subscription,
}

impl EventEmitter<HoloEvent> for HoloRepos {}

impl HoloRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: None,
            more: [false; 3],
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
        let shown = if n <= room || self.more[0] {
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
            left = left.child(
                link("lr-more", &format!("+ {} more ›", n - shown), PK)
                    .mt(px(12.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.more[0] = true;
                        cx.notify();
                    })),
            );
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
            let parts_shown = if self.more[1] {
                c.parts.len()
            } else {
                c.parts.len().min(3)
            };
            for p in c.parts.iter().take(parts_shown) {
                right = right.child(prose(&format!("{} · {}", p.name, p.paths.join(", "))));
            }
            if c.parts.len() > parts_shown {
                right = right.child(
                    link(
                        "lr-more-parts",
                        &format!("+ {} more parts ›", c.parts.len() - parts_shown),
                        PK,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.more[1] = true;
                        cx.notify();
                    })),
                );
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
            let shown = if sessions.len() <= room || self.more[2] {
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
                right = right.child(
                    link(
                        "lr-more-sessions",
                        &format!("+ {} more sessions ›", sessions.len() - shown),
                        PK,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.more[2] = true;
                        cx.notify();
                    })),
                );
            }
        }
        let right_tabs = vec![
            link("rr-open", "Open session ›", PK)
                .ml_auto()
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

/// The page color and the signal color of a look.
fn look_colors(look: aivory_protocol::Look) -> (u32, u32) {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => (0xF4F4F2, 0x3B82F6),
        Look::Bridge => (0x050505, 0xFFB000),
        Look::Operator => (0x0A1020, 0x2F7CFF),
        Look::Neon => (0x0B0620, 0xFF2BD6),
        Look::Navigator => (0x1E2A33, 0x7FB0CF),
        Look::Holo => (0x040A16, 0x7FD8FF),
        Look::Tactical => (0x262626, 0x8FD14F),
        Look::Command => (0x141A22, 0xFFB800),
    }
}

/// A one-line description of a look for the list.
fn look_line(look: aivory_protocol::Look) -> &'static str {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => "Clean light screens, one rail",
        Look::Bridge => "LCARS panels, orange and violet",
        Look::Operator => "Rain wall with blue instruments",
        Look::Neon => "A neon city, a tower per session",
        Look::Navigator => "3D files with a toolchest",
        Look::Holo => "See-through glass over rings",
        Look::Tactical => "A calm radar of sessions",
        Look::Command => "A village map and a command card",
    }
}

/// The five colors of a look: page, panel, text, signal, wait.
fn look_palette(look: aivory_protocol::Look) -> [u32; 5] {
    use aivory_protocol::Look;
    match look {
        Look::Minimal => [0xF4F4F2, 0xFFFFFF, 0x1A1C22, 0x3B82F6, 0xE08A1E],
        Look::Bridge => [0x050505, 0x2A2130, 0xF3E3CF, 0xFF9C00, 0x9C7BFF],
        Look::Operator => [0x0A1020, 0x07120D, 0x8FD3AE, 0x2F7CFF, 0x9BFFC0],
        Look::Neon => [0x0B0620, 0x0E1016, 0xE6E8EE, 0xFF2BD6, 0xE8DA3A],
        Look::Navigator => [0x1E2A33, 0xC6C6C6, 0x111111, 0x5A7299, 0xFFC21A],
        Look::Holo => [0x040A16, 0x0A182E, 0xDDF8FF, 0x4FD8FF, 0xFF9A4D],
        Look::Tactical => [0x262626, 0x021206, 0x8FFFAE, 0x39FF6A, 0xFFB000],
        Look::Command => [0x141A22, 0x1E2124, 0xE6E8DA, 0x5EA634, 0xFFC21A],
    }
}

/// A box of the sketch of a look at a place.
fn blk(x: f32, y: f32, w: f32, h: f32, c: u32, a: f32) -> gpui::Div {
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(w))
        .h(px(h))
        .bg(ink(c, a))
}

/// A tiny sketch of the frame of a look: its colors and the shape of its war room.
fn look_sketch(look: aivory_protocol::Look) -> gpui::Div {
    use aivory_protocol::Look;
    let [page, panel, text, signal, wait] = look_palette(look);
    let mut d = div()
        .relative()
        .flex_none()
        .h(px(170.))
        .w(px(340.))
        .overflow_hidden()
        .border_1()
        .border_color(ink(PK, 0.4))
        .bg(rgb(page));
    match look {
        Look::Minimal => {
            d = d
                .child(blk(0., 0., 44., 170., 0xE6E6E2, 1.))
                .child(blk(60., 14., 200., 60., panel, 1.).rounded(px(6.)))
                .child(blk(60., 86., 200., 60., panel, 1.).rounded(px(6.)));
        }
        Look::Bridge => {
            d = d
                .child(blk(0., 0., 26., 170., signal, 1.).rounded_tr(px(14.)))
                .child(blk(26., 0., 300., 14., wait, 1.))
                .child(blk(44., 30., 90., 100., panel, 1.).rounded(px(8.)))
                .child(blk(144., 30., 110., 100., panel, 1.).rounded(px(8.)));
        }
        Look::Operator => {
            for k in 0..9 {
                d = d.child(blk(
                    10. + k as f32 * 22.,
                    6.,
                    2.,
                    40. + (k % 4) as f32 * 14.,
                    text,
                    0.5,
                ));
            }
            d = d
                .child(
                    blk(14., 90., 110., 60., 0x0F1D40, 1.)
                        .border_1()
                        .border_color(rgb(signal)),
                )
                .child(
                    blk(140., 90., 110., 60., panel, 1.)
                        .border_1()
                        .border_color(rgb(wait)),
                );
        }
        Look::Neon => {
            for k in 0..8 {
                let h = 30. + ((k * 37) % 70) as f32;
                let c = if k % 3 == 0 { signal } else { 0x4FB8C4 };
                d = d.child(blk(14. + k as f32 * 26., 150. - h, 16., h, c, 0.85));
            }
            d = d.child(
                blk(200., 10., 70., 40., panel, 1.)
                    .border_1()
                    .border_color(rgb(signal)),
            );
        }
        Look::Navigator => {
            d = d
                .child(blk(14., 14., 220., 130., panel, 1.))
                .child(blk(14., 14., 220., 18., signal, 1.))
                .child(blk(244., 40., 40., 40., wait, 1.));
        }
        Look::Holo => {
            d = d
                .child(
                    blk(110., 20., 110., 110., signal, 0.0)
                        .rounded_full()
                        .border_1()
                        .border_color(ink(signal, 0.7)),
                )
                .child(
                    blk(136., 46., 58., 58., signal, 0.0)
                        .rounded_full()
                        .border_1()
                        .border_color(ink(signal, 0.9)),
                )
                .child(
                    blk(12., 16., 84., 66., signal, 0.12)
                        .border_1()
                        .border_color(ink(wait, 0.8)),
                )
                .child(
                    blk(236., 16., 84., 120., signal, 0.12)
                        .border_1()
                        .border_color(ink(signal, 0.7)),
                );
        }
        Look::Tactical => {
            d = d
                .child(
                    blk(80., 14., 130., 130., signal, 0.0)
                        .rounded_full()
                        .border_1()
                        .border_color(ink(signal, 0.8)),
                )
                .child(
                    blk(110., 44., 70., 70., signal, 0.0)
                        .rounded_full()
                        .border_1()
                        .border_color(ink(signal, 0.5)),
                )
                .child(blk(142., 76., 6., 6., wait, 1.))
                .child(
                    blk(8., 14., 54., 130., panel, 1.)
                        .border_1()
                        .border_color(ink(signal, 0.6)),
                );
        }
        Look::Command => {
            d = d
                .child(blk(0., 0., 250., 170., 0x5C7A2E, 1.))
                .child(blk(40., 40., 46., 34., 0x8A5A3A, 1.))
                .child(blk(120., 90., 46., 34., 0x8A5A3A, 1.))
                .child(
                    blk(250., 0., 80., 170., panel, 1.)
                        .border_l_2()
                        .border_color(rgb(0x6A7076)),
                )
                .child(blk(266., 60., 48., 48., signal, 1.));
        }
    }
    d
}

/// The palette of a look as five labeled swatches in a column.
fn look_palette_list(look: aivory_protocol::Look) -> gpui::Div {
    let mut col = div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(8.));
    for (name, c) in ["Page", "Panel", "Text", "Signal", "Wait"]
        .into_iter()
        .zip(look_palette(look))
    {
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .size(px(26.))
                        .bg(rgb(c))
                        .border_1()
                        .border_color(ink(PA, 0.5)),
                )
                .child(th(name))
                .child(
                    div().flex_1().flex().justify_end().child(
                        div()
                            .font_family(CODE)
                            .text_size(px(S_CODE))
                            .text_color(rgb(MUTED))
                            .child(format!("#{c:06X}")),
                    ),
                ),
        );
    }
    col
}

/// A small preview of a look: its page color with its signal color, framed in cyan when the
/// look is in use.
fn swatch(look: aivory_protocol::Look, used: bool, size: f32) -> gpui::Div {
    let (page, signal) = look_colors(look);
    div()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .border_1()
        .border_color(ink(PK, if used { 1. } else { 0.3 }))
        .bg(rgb(page))
        .flex()
        .items_end()
        .justify_end()
        .child(div().w(px(size * 0.5)).h(px(size * 0.35)).bg(rgb(signal)))
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
        // "Other": the full settings screen (providers, devices, machines, notifications).
        left_tabs.push(
            tab("ls-tab-Other", "All settings ›", PK, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(HoloEvent::AllSettings)))
                .into_any_element(),
        );
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
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(swatch(look, used, 28.))
                            .child(
                                row(
                                    format!("ls-look-{k}"),
                                    &format!("{:02}", k + 1),
                                    if used { PK } else { GY },
                                    name,
                                    look_line(look),
                                    if used { "In use" } else { "" },
                                    PK,
                                    look == shown,
                                )
                                .min_h(px(52.))
                                .py(px(4.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.picked = Some(look);
                                    cx.notify();
                                }))
                                .flex_1()
                                .min_w(px(0.)),
                            ),
                    );
                }
                let name = crate::look::name(shown);
                right = right
                    .child(
                        div()
                            .flex()
                            .gap(px(24.))
                            .child(look_sketch(shown))
                            .child(look_palette_list(shown)),
                    )
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
                                    "Turn autopilot off"
                                } else {
                                    "Turn autopilot on"
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
                right = right.child(big("Providers and models"));
                if let Some(w) = &war {
                    for u in w.usage.iter() {
                        right = right.child(prose(&format!(
                            "{} {} · {}% used",
                            u.provider, u.label, u.pct
                        )));
                    }
                }
                right = right
                    .child(prose(
                        "Models, effort and permission mode of each provider are in the full settings.",
                    ))
                    .child(
                        button("ls-all", Tone::Plain, "Open all settings")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HoloEvent::AllSettings))),
                    );
            }
        }
        let right_tabs: Vec<AnyElement> = Vec::new();
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
