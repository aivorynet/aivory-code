//! The Operator look of the screens Inbox, Tasks, Repos and Settings, and the frame of Focus,
//! Lens and the full Settings. The grammar is the operator console: a title line with the
//! F-key, blue screens for information and decisions, green screens for agents, monospace
//! text. Every screen has a menu (blue, left), a list and a detail screen. The menu is dim
//! until the pointer comes (one focus: the detail). Every control calls the daemon through
//! `actions.rs`, exactly as the Bridge screens do.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{AutopilotPatch, InboxListParams, TaskListParams, method};
use aivory_protocol::{
    InboxFilter, InboxItem, InboxKind, InboxList, LineKind, SessionState, StepState, Task,
    TaskColumn, TaskList,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, Stateful, Task as GpuiTask, Window, div, px,
    relative, rgb,
};

use crate::actions::{Action, act};
use crate::model::Model;
use crate::ui;
use crate::warroom::skins::operator::kit::*;

/// The gap between the screens.
const GAP: f32 = 20.;
/// The inner padding of a screen.
const PAD: f32 = 24.;
/// The soft red of an error. Color is for state only.
const ERR: u32 = 0xFF8A80;

/// What a screen asks the shell to do.
#[derive(Debug, Clone)]
pub enum OperatorEvent {
    OpenSession(String),
    /// The full Settings screen.
    AllSettings,
}

/// The Operator screens, made when they first show, then kept.
#[derive(Default)]
pub(super) struct OperatorScreens {
    pub inbox: Option<(Entity<OperatorInbox>, gpui::Subscription)>,
    pub tasks: Option<(Entity<OperatorTasks>, gpui::Subscription)>,
    pub repos: Option<(Entity<OperatorRepos>, gpui::Subscription)>,
    pub settings: Option<(Entity<OperatorSettings>, gpui::Subscription)>,
    pub focus: Option<(
        Entity<super::operator_focus::OperatorFocus>,
        gpui::Subscription,
    )>,
    pub lens: Option<(
        Entity<super::operator_lens::OperatorLens>,
        gpui::Subscription,
    )>,
    /// The codebase, file and session that the Lens shows next.
    pub lens_target: Option<(String, String, Option<String>)>,
}

// ---- parts --------------------------------------------------------------------------------

/// The title line of a screen: the F-key and the title on the left, facts on the right.
pub(super) fn head_line(fkey: &str, title: &str, stats: Vec<(String, String)>) -> Div {
    let mut line = div()
        .flex_none()
        .h(px(68.))
        .flex()
        .items_center()
        .gap(px(28.))
        .child(
            div()
                .text_size(ui::ts(T_TITLE))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(GREEN_HI))
                .child(format!("{fkey} // {}", title.to_uppercase())),
        )
        .child(div().flex_1());
    for (k, v) in stats {
        line = line.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(k.to_uppercase()),
                )
                .child(
                    div()
                        .text_size(ui::ts(T_ENTRY))
                        .text_color(rgb(BLUE_HI))
                        .child(v),
                ),
        );
    }
    line
}

/// The layout of a screen: the title line over a row of screens.
pub(super) fn page(head: Div, columns: Vec<AnyElement>) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .px(px(28.))
        .pb(px(14.))
        .bg(rgb(PAGE))
        .font_family(FONT)
        .text_size(ui::ts(T_ENTRY))
        .text_color(rgb(GREEN))
        .child(head)
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .gap(px(GAP))
                .children(columns),
        )
}

/// A screen with a section title and a scrolling body. Blue = information, green = agent.
pub(super) fn mon(
    id: &'static str,
    blue: bool,
    title: &str,
    badge: &str,
    foot_text: &str,
    body: Div,
) -> Div {
    let inner = div()
        .id(id)
        .absolute()
        .inset_0()
        .px(px(PAD))
        .py(px(PAD - 6.))
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .text_size(ui::ts(T_ENTRY))
        .line_height(relative(1.45))
        .child(
            div()
                .flex_none()
                .pb(px(12.))
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(if blue { BLUE_HI } else { GREEN_HI }))
                .child(title.to_uppercase()),
        )
        .child(body);
    bezel(if blue { BLUE_RING } else { GREEN_MID })
        .min_h(px(0.))
        .child(screen(blue, inner))
        .child(foot(
            badge,
            if blue { BLUE } else { GREEN },
            vec![(foot_text.to_string(), FOOT)],
            30.,
        ))
}

/// A menu screen: dim until the pointer comes.
pub(super) fn menu_col(id: &'static str, inner: Div) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .w(px(270.))
        .flex()
        .flex_col()
        .opacity(0.72)
        .hover(|s| s.opacity(1.))
        .child(inner.flex_1())
        .into_any_element()
}

/// An entry of a menu: an arrow, the name, the count.
pub(super) fn menu_item(id: String, text: &str, count: Option<usize>, on: bool) -> Stateful<Div> {
    div()
        .id(eid(id))
        .flex_none()
        .h(px(40.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(10.))
        .cursor_pointer()
        .when(on, |d| {
            d.bg(rgb(BLUE))
                .text_color(rgb(BLUE_INK))
                .font_weight(gpui::FontWeight::SEMIBOLD)
        })
        .when(!on, |d| d.hover(|h| h.bg(rgb(BLUE_DIM))))
        .child(if on { "▶" } else { " " })
        .child(div().flex_1().child(text.to_uppercase()))
        .children(count.map(|n| format!("{n:02}")))
}

/// A row of a list: number, title, a sub line, and a word on the right.
pub(super) fn list_row(
    id: String,
    no: usize,
    title: &str,
    sub: &str,
    right: &str,
    color: u32,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(eid(id))
        .flex_none()
        .h(px(52.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_pointer()
        .when(on, |d| d.bg(rgb(BLUE_DIM)))
        .when(!on, |d| d.hover(|h| h.bg(hca(BLUE_DIM, 150))))
        .child(
            div()
                .flex_none()
                .w(px(24.))
                .text_color(rgb(color))
                .child(format!("{no:02}")),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(rgb(if on { BLUE_HI } else { BLUE }))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(sub.to_string()),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(color))
                .child(right.to_string()),
        )
}

/// A button: an operator command. `strong` = lit.
pub(super) fn btn(id: String, text: &str, strong: bool) -> Stateful<Div> {
    div()
        .id(eid(id))
        .flex_none()
        .h(px(34.))
        .px(px(16.))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(ui::ts(T_ENTRY))
        .when(strong, |d| {
            d.bg(rgb(BLUE))
                .text_color(rgb(BLUE_INK))
                .font_weight(gpui::FontWeight::SEMIBOLD)
        })
        .when(!strong, |d| {
            d.bg(hca(BLUE_RING, 70))
                .text_color(rgb(BLUE_HI))
                .hover(|h| h.bg(rgb(BLUE)).text_color(rgb(BLUE_INK)))
        })
        .child(text.to_uppercase())
}

/// A line of body text.
pub(super) fn prose(text: &str) -> Div {
    div().text_color(rgb(BLUE)).child(text.to_string())
}

/// A small caption in the body.
pub(super) fn caption(text: &str) -> Div {
    div()
        .flex_none()
        .mt(px(10.))
        .text_size(ui::ts(T_LABEL))
        .text_color(rgb(BLUE_SOFT))
        .child(text.to_uppercase())
}

/// A block of code or a path.
pub(super) fn code(text: &str) -> Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .border_l_2()
        .border_color(rgb(BLUE_RING))
        .bg(rgb(0x021024))
        .text_color(rgb(BLUE_HI))
        .child(text.to_string())
}

/// The lamp color of a session state.
fn state_color(state: SessionState) -> u32 {
    match state {
        SessionState::Working => GREEN,
        SessionState::NeedsYou => BLUE_HI,
        SessionState::Error => ERR,
        _ => BLUE_SOFT,
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

fn minutes(now: i64, since: i64) -> String {
    let m = ((now - since) / 60_000).max(0);
    if m < 1 {
        "now".to_string()
    } else {
        format!("{m} min")
    }
}

/// The last prose rows of a transcript as `You: text` lines.
fn chat_text(tail: &[aivory_protocol::Line], n: usize) -> Vec<String> {
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

// ================================================================================= Inbox ====

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

pub struct OperatorInbox {
    model: Entity<Model>,
    data: Option<Rc<InboxList>>,
    tab: InboxTab,
    side: Side,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: gpui::Subscription,
}

impl EventEmitter<OperatorEvent> for OperatorInbox {}

impl OperatorInbox {
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
        InboxKind::Error => ERR,
        _ => BLUE_HI,
    }
}

impl Render for OperatorInbox {
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
            .map_or("none".to_string(), |s| minutes(now, s));
        let head = head_line(
            "F2",
            "Inbox",
            vec![
                ("waiting".into(), count(InboxFilter::Answers).to_string()),
                ("errors".into(), count(InboxFilter::Errors).to_string()),
                ("oldest".into(), oldest),
                ("done today".into(), done.to_string()),
            ],
        );

        // The menu.
        let mut menu = div().flex().flex_col().gap(px(2.));
        for (t, label, n) in [
            (InboxTab::All, "All", all),
            (InboxTab::Waiting, "Waiting", count(InboxFilter::Answers)),
            (InboxTab::Errors, "Errors", count(InboxFilter::Errors)),
            (InboxTab::Reviews, "Reviews", count(InboxFilter::Reviews)),
            (InboxTab::Done, "Done", done),
        ] {
            menu = menu.child(
                menu_item(format!("mi-tab-{label}"), label, Some(n), self.tab == t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.tab = t;
                        this.selected = None;
                        cx.notify();
                    }),
                ),
            );
        }
        let menu = menu_col(
            "mi-menu",
            mon("mi-menu-s", true, "Menu", "1", "filter", menu),
        );

        // The list.
        let mut list = div().flex().flex_col();
        for (k, it) in items.iter().take(9).enumerate() {
            let id = it.id.clone();
            list = list.child(
                list_row(
                    format!("mi-row-{k}"),
                    k + 1,
                    &it.title,
                    &format!("{} · {}", it.codebase, it.kind.label()),
                    &minutes(now, it.since),
                    kind_color(it.kind),
                    cur.as_ref().is_some_and(|c| c.id == it.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        if items.is_empty() {
            list = list.child(prose("Nothing waits for you."));
        }
        let list = mon(
            "mi-list",
            true,
            &format!("Incoming · {}", items.len()),
            "2",
            "oldest first",
            list,
        )
        .w(px(430.))
        .flex_none()
        .into_any_element();

        // The detail: the decision, its details, and the way to the session.
        let mut body = div().flex().flex_col().gap(px(12.));
        let mut title = "Detail".to_string();
        let open: Option<String>;
        if let Some(it) = &cur {
            title = it.kind.heading().to_string();
            open = Some(it.session.clone());
            let at = items.iter().position(|i| i.id == it.id).unwrap_or(0);
            body = body
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(format!(
                            "{} {:02} of {:02}",
                            if self.tab == InboxTab::Done {
                                "DONE"
                            } else {
                                "DECISION"
                            },
                            at + 1,
                            items.len()
                        ))
                        .child(format!("waits {}", minutes(now, it.since))),
                )
                .child(
                    div()
                        .text_color(rgb(BLUE_HI))
                        .line_height(relative(1.5))
                        .child(it.title.clone()),
                )
                .child(
                    div()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(format!(
                            "{} · {} / {}",
                            it.agents
                                .first()
                                .map_or(String::new(), |a| a.provider.clone()),
                            it.codebase,
                            it.part
                        )),
                );
            if let Some(result) = &it.result {
                body = body.child(prose(&format!("Done · {result}")));
            } else {
                let mut options = div().flex().flex_col().gap(px(4.));
                for (k, o) in it.options.iter().take(5).enumerate() {
                    let (id, opt) = (it.id.clone(), o.clone());
                    options = options.child(
                        btn(format!("mi-opt-{k}"), &format!("{}  {}", k + 1, o), k == 0).on_click(
                            cx.listener(move |this, _, _, cx| {
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
                            }),
                        ),
                    );
                }
                body = body.child(options);
            }
            let mut tabs = div().flex().gap(px(6.)).mt(px(6.));
            for (s, label) in [
                (Side::Details, "Details"),
                (Side::Files, "Files"),
                (Side::Chat, "Chat"),
            ] {
                tabs = tabs.child(
                    btn(format!("mi-side-{label}"), label, self.side == s).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        },
                    )),
                );
            }
            body = body.child(tabs);
            match self.side {
                Side::Details => {
                    body = body
                        .child(caption("Why it asks"))
                        .child(prose(if it.preview.is_empty() {
                            &it.text
                        } else {
                            &it.preview
                        }))
                        .child(caption("Command or question"))
                        .child(code(&it.text));
                    if !it.steps.is_empty() {
                        body = body.child(caption("Plan")).children(
                            it.steps
                                .iter()
                                .take(6)
                                .enumerate()
                                .map(|(k, s)| prose(&format!("{}. {s}", k + 1))),
                        );
                    }
                }
                Side::Files => {
                    body = body.child(caption(&format!("Files changed · {}", it.files.len())));
                    for f in it.files.iter().take(12) {
                        body = body.child(prose(f));
                    }
                }
                Side::Chat => {
                    body = body.child(caption("Last rows of the chat"));
                    for r in chat_text(&it.tail, 8) {
                        body = body.child(prose(&r));
                    }
                }
            }
            body = body.child(div().mt(px(8.)).child(
                btn("mi-open".into(), "Open session ›", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        if let Some(id) = &open {
                            cx.emit(OperatorEvent::OpenSession(id.clone()));
                        }
                    },
                )),
            ));
        } else {
            body = body.child(prose("Nothing waits for you."));
        }
        let detail = mon("mi-detail", true, &title, "3", "decision", body)
            .flex_1()
            .min_w(px(0.))
            .into_any_element();
        page(head, vec![menu, list.into_any_element(), detail])
    }
}

// ================================================================================= Tasks ====

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

pub struct OperatorTasks {
    model: Entity<Model>,
    data: Option<Rc<TaskList>>,
    tab: TaskTab,
    side: TaskSide,
    selected: Option<String>,
    busy: bool,
    _tick: GpuiTask<()>,
}

impl EventEmitter<OperatorEvent> for OperatorTasks {}

impl OperatorTasks {
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
        StepState::Done => BLUE_SOFT,
        StepState::Working => GREEN,
        StepState::Waiting => BLUE_HI,
        StepState::NotStarted => FOOT,
    }
}

fn step_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "done",
        StepState::Working => "working",
        StepState::Waiting => "waits for you",
        StepState::NotStarted => "queued",
    }
}

impl Render for OperatorTasks {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (autopilot, queued_n) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map_or((false, 0), |w| (w.autopilot.on, w.queued));
        let n = |t| self.of(t).len();
        let head = head_line(
            "F3",
            "Tasks",
            vec![
                ("queued".into(), n(TaskTab::Queued).to_string()),
                ("running".into(), n(TaskTab::Running).to_string()),
                ("in review".into(), n(TaskTab::Review).to_string()),
                ("done".into(), n(TaskTab::Done).to_string()),
            ],
        );
        let mut menu = div().flex().flex_col().gap(px(2.));
        for (t, label) in [
            (TaskTab::Queued, "Queued"),
            (TaskTab::Running, "Running"),
            (TaskTab::Review, "Review"),
            (TaskTab::Done, "Done"),
        ] {
            menu = menu.child(
                menu_item(format!("mt-tab-{label}"), label, Some(n(t)), self.tab == t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.tab = t;
                        this.selected = None;
                        cx.notify();
                    }),
                ),
            );
        }
        menu = menu.child(caption("Autopilot")).child(
            div().mt(px(4.)).child(
                btn(
                    "mt-autopilot".into(),
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
            ),
        );
        menu = menu.child(
            div()
                .mt(px(6.))
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(BLUE_SOFT))
                .child(format!("{queued_n} queued for autopilot")),
        );
        let menu = menu_col(
            "mt-menu",
            mon("mt-menu-s", true, "Menu", "1", "column", menu),
        );

        let tasks = self.of(self.tab);
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.id == id))
            .or(tasks.first())
            .cloned();
        let mut list = div().flex().flex_col();
        for (k, t) in tasks.iter().take(9).enumerate() {
            let id = t.id.clone();
            let c = if t.waiting_for.is_some() {
                BLUE_HI
            } else {
                GREEN
            };
            list = list.child(
                list_row(
                    format!("mt-row-{k}"),
                    k + 1,
                    &t.title,
                    &format!(
                        "{} · {}",
                        t.agents.first().cloned().unwrap_or_default(),
                        t.codebase
                    ),
                    &format!("step {}", (t.step + 1).min(5)),
                    c,
                    cur.as_ref().is_some_and(|x| x.id == t.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        if tasks.is_empty() {
            list = list.child(prose("No task here."));
        }
        let list = mon(
            "mt-list",
            true,
            &format!(
                "{} · {}",
                ["Queued", "Running", "Review", "Done"][self.tab as usize],
                tasks.len()
            ),
            "2",
            "tasks",
            list,
        )
        .w(px(430.))
        .flex_none()
        .into_any_element();

        let mut body = div().flex().flex_col().gap(px(12.));
        let mut title = "Task".to_string();
        let open: Option<String>;
        if let Some(t) = &cur {
            open = t.sessions.first().map(|s| s.session.clone());
            title = t.title.clone();
            body = body.child(
                div()
                    .text_size(ui::ts(T_LABEL))
                    .text_color(rgb(BLUE_SOFT))
                    .child(format!(
                        "{} · {} / {} · ${:.2}",
                        t.agents.first().cloned().unwrap_or_default(),
                        t.codebase,
                        t.part,
                        t.cost
                    )),
            );
            let mut tabs = div().flex().gap(px(6.));
            for (s, label) in [(TaskSide::Plan, "Plan"), (TaskSide::Sessions, "Sessions")] {
                tabs = tabs.child(
                    btn(format!("mt-side-{label}"), label, self.side == s).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.side = s;
                            cx.notify();
                        },
                    )),
                );
            }
            body = body.child(tabs);
            match self.side {
                TaskSide::Plan => {
                    body = body.child(caption(&format!("Plan · {} steps", t.steps.len())));
                    for (k, s) in t.steps.iter().enumerate() {
                        body = body.child(
                            div()
                                .flex_none()
                                .h(px(38.))
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(div().flex_none().size(px(9.)).bg(rgb(step_color(s.state))))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(24.))
                                        .text_color(rgb(BLUE_SOFT))
                                        .child(format!("{:02}", k + 1)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(rgb(BLUE_HI))
                                        .child(s.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(ui::ts(T_LABEL))
                                        .text_color(rgb(step_color(s.state)))
                                        .child(step_word(s.state).to_uppercase()),
                                ),
                        );
                    }
                }
                TaskSide::Sessions => {
                    body = body.child(caption(&format!("Sessions · {}", t.sessions.len())));
                    for (k, s) in t.sessions.iter().enumerate() {
                        let id = s.session.clone();
                        body = body.child(
                            list_row(
                                format!("mt-s-{k}"),
                                k + 1,
                                &s.title,
                                &s.role,
                                "open ›",
                                GREEN,
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| cx.emit(OperatorEvent::OpenSession(id.clone())),
                            )),
                        );
                    }
                }
            }
            body = body.child(div().mt(px(8.)).child(
                btn("mt-open".into(), "Open session ›", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        if let Some(id) = &open {
                            cx.emit(OperatorEvent::OpenSession(id.clone()));
                        }
                    },
                )),
            ));
        } else {
            body = body.child(prose("No task here."));
        }
        let detail = mon("mt-detail", true, &title, "3", "plan", body)
            .flex_1()
            .min_w(px(0.))
            .into_any_element();
        page(head, vec![menu, list, detail])
    }
}

// ================================================================================= Repos ====

pub struct OperatorRepos {
    model: Entity<Model>,
    selected: Option<String>,
    _obs: gpui::Subscription,
}

impl EventEmitter<OperatorEvent> for OperatorRepos {}

impl OperatorRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: None,
            _obs: obs,
        }
    }
}

impl Render for OperatorRepos {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(war) = self.model.read(cx).war.clone() else {
            return page(
                head_line("F4", "Repos", Vec::new()),
                vec![
                    mon(
                        "mr-none",
                        true,
                        "Repos",
                        "1",
                        "no data",
                        prose("No data yet."),
                    )
                    .flex_1()
                    .into_any_element(),
                ],
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
        let cur = self
            .selected
            .as_ref()
            .and_then(|id| war.codebase(id))
            .or(war.codebases.first())
            .cloned();
        let head = head_line(
            "F4",
            "Repos",
            vec![
                ("codebases".into(), war.codebases.len().to_string()),
                ("live sessions".into(), war.counts().live.to_string()),
                ("waiting".into(), war.counts().needs_you.to_string()),
                (
                    "worktrees".into(),
                    war.codebases
                        .iter()
                        .map(|c| c.worktrees)
                        .sum::<u32>()
                        .to_string(),
                ),
            ],
        );
        let mut list = div().flex().flex_col();
        for (k, c) in war.codebases.iter().enumerate() {
            let id = c.id.clone();
            let w = waits(&c.id);
            list = list.child(
                list_row(
                    format!("mr-row-{k}"),
                    k + 1,
                    &c.id,
                    &format!("{} · {}", c.branch, c.path),
                    &format!("{} live", live(&c.id)),
                    if w > 0 { BLUE_HI } else { BLUE_SOFT },
                    cur.as_ref().is_some_and(|x| x.id == c.id),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        let list = mon("mr-list", true, "Codebases", "1", "click to show", list)
            .w(px(470.))
            .flex_none()
            .into_any_element();

        let mut body = div().flex().flex_col().gap(px(12.));
        let mut title = "Codebase".to_string();
        if let Some(c) = &cur {
            title = c.id.clone();
            let sessions: Vec<_> = war
                .sessions
                .iter()
                .filter(|s| s.codebase == c.id && s.state.is_live())
                .collect();
            body = body
                .child(
                    div()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(format!(
                            "{} · {} worktrees · {}",
                            c.branch, c.worktrees, c.daemon
                        )),
                )
                .child(code(&c.path))
                .child(caption(&format!("Parts · {}", c.parts.len())));
            for p in c.parts.iter().take(6) {
                body = body.child(prose(&format!("{} · {}", p.name, p.paths.join(", "))));
            }
            body = body.child(caption(&format!("Live sessions · {}", sessions.len())));
            for (k, s) in sessions.iter().take(6).enumerate() {
                let id = s.id.clone();
                body = body.child(
                    list_row(
                        format!("mr-s-{k}"),
                        k + 1,
                        &s.title,
                        &s.provider,
                        "open ›",
                        state_color(s.state),
                        false,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(OperatorEvent::OpenSession(id.clone()))
                    })),
                );
            }
            if sessions.is_empty() {
                body = body.child(prose("No live session in this codebase."));
            }
        }
        let detail = mon("mr-detail", true, &title, "2", "codebase", body)
            .flex_1()
            .min_w(px(0.))
            .into_any_element();
        page(head, vec![list, detail])
    }
}

// =============================================================================== Settings ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Looks,
    Autopilot,
    Providers,
}

pub struct OperatorSettings {
    model: Entity<Model>,
    section: Section,
    /// The row of the list that the detail shows. The current look for the Looks section.
    sel: Option<usize>,
    _obs: gpui::Subscription,
}

impl EventEmitter<OperatorEvent> for OperatorSettings {}

impl OperatorSettings {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            section: Section::Looks,
            sel: None,
            _obs: obs,
        }
    }
}

/// What a look is, in one calm sentence.
fn look_blurb(name: &str) -> &'static str {
    match name {
        "Minimal" => "The calm default: a light page, a rail on the left and round cards.",
        "Bridge" => {
            "Island frames on black with numbered frame segments. Informative, never overwhelming."
        }
        "Operator" => {
            "An operator console: blue screens for information, green screens for agents, F-keys in the top line."
        }
        "Neon" => {
            "A full-screen city. A district is a codebase, a tower is a session, neon panels float over it."
        }
        "Navigator" => {
            "A 3D file-system view is the navigation. IRIX-style windows and a toolchest bar."
        }
        "Holo" => "See-through glass panels over slow rings. One decision at a time.",
        "Tactical" => {
            "A radar: the bearing is the codebase, the range is the time since a session moved."
        }
        _ => "A look for the whole app.",
    }
}

impl Render for OperatorSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let (auto, max, park) = war.as_ref().map_or((false, 0, 0), |w| {
            (
                w.autopilot.on,
                w.autopilot.max_running,
                w.autopilot.park_after_min,
            )
        });
        let head = head_line(
            "F5",
            "Settings",
            vec![
                (
                    "look".into(),
                    crate::look::name(crate::theme::tk().look).to_string(),
                ),
                (
                    "autopilot".into(),
                    if auto { "on" } else { "off" }.to_string(),
                ),
                ("running at most".into(), max.to_string()),
                ("parks after".into(), format!("{park} min")),
            ],
        );
        let mut menu = div().flex().flex_col().gap(px(2.));
        for (s, label) in [
            (Section::Looks, "Looks"),
            (Section::Autopilot, "Autopilot"),
            (Section::Providers, "Providers"),
        ] {
            menu = menu.child(
                menu_item(format!("ms-tab-{label}"), label, None, self.section == s).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.section = s;
                        this.sel = None;
                        cx.notify();
                    }),
                ),
            );
        }
        menu = menu.child(
            menu_item("ms-all".into(), "All settings ›", None, false)
                .mt(px(14.))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(OperatorEvent::AllSettings))),
        );
        let menu = menu_col(
            "ms-menu",
            mon("ms-menu-s", true, "Menu", "1", "section", menu),
        );

        let mut list = div().flex().flex_col();
        let mut body = div().flex().flex_col().gap(px(12.));
        let list_title: &str;
        let detail_title: String;
        match self.section {
            Section::Looks => {
                list_title = "Looks";
                let current = crate::look::LOOKS
                    .iter()
                    .position(|(l, _)| *l == crate::theme::tk().look)
                    .unwrap_or(0);
                let sel = self
                    .sel
                    .unwrap_or(current)
                    .min(crate::look::LOOKS.len() - 1);
                for (k, (_, name)) in crate::look::LOOKS.into_iter().enumerate() {
                    list = list.child(
                        list_row(
                            format!("ms-look-{k}"),
                            k + 1,
                            name,
                            look_blurb(name),
                            if k == current { "ON" } else { "" },
                            BLUE_HI,
                            k == sel,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sel = Some(k);
                            cx.notify();
                        })),
                    );
                }
                let (look, name) = crate::look::LOOKS[sel];
                detail_title = name.to_string();
                body = body
                    .child(prose(look_blurb(name)))
                    .child(caption("Status"))
                    .child(prose(if sel == current {
                        "This look is on now."
                    } else {
                        "This look is off. Switching changes the frame, the colors and the fonts of every screen."
                    }))
                    .child(
                        div().mt(px(8.)).child(
                            btn(
                                "ms-switch".into(),
                                &if sel == current {
                                    format!("{name} is on")
                                } else {
                                    format!("Switch to {name}")
                                },
                                sel != current,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                act(&this.model, Action::SetLook(look), cx);
                            })),
                        ),
                    )
                    .child(prose(
                        "The choice is saved in the settings of the daemon.",
                    ));
            }
            Section::Autopilot => {
                list_title = "Autopilot";
                let rows = [
                    (
                        "Autopilot",
                        "starts the queued tasks for you",
                        if auto { "on" } else { "off" }.to_string(),
                    ),
                    (
                        "Running at most",
                        "sessions at the same time",
                        max.to_string(),
                    ),
                    ("Parks after", "minutes without work", format!("{park} min")),
                ];
                let sel = self.sel.unwrap_or(0).min(rows.len() - 1);
                for (k, (t, sub, v)) in rows.iter().enumerate() {
                    list = list.child(
                        list_row(
                            format!("ms-auto-row-{k}"),
                            k + 1,
                            t,
                            sub,
                            v,
                            BLUE_HI,
                            k == sel,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sel = Some(k);
                            cx.notify();
                        })),
                    );
                }
                detail_title = rows[sel].0.to_string();
                let patch = |p: AutopilotPatch| Action::Autopilot(p);
                match sel {
                    0 => {
                        body = body
                            .child(prose(&format!(
                                "Autopilot is {}. When it is on, the daemon starts queued tasks while fewer than {max} sessions run.",
                                if auto { "on" } else { "off" }
                            )))
                            .child(
                                div().child(
                                    btn(
                                        "ms-auto".into(),
                                        if auto { "Turn off" } else { "Turn on" },
                                        !auto,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        act(
                                            &this.model,
                                            patch(AutopilotPatch {
                                                on: Some(!auto),
                                                ..AutopilotPatch::default()
                                            }),
                                            cx,
                                        );
                                    })),
                                ),
                            );
                    }
                    1 => {
                        body = body
                            .child(prose(&format!("Up to {max} sessions run at once.")))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .child(btn("ms-less".into(), "Fewer", false).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            act(
                                                &this.model,
                                                patch(AutopilotPatch {
                                                    max_running: Some(max.saturating_sub(1).max(1)),
                                                    ..AutopilotPatch::default()
                                                }),
                                                cx,
                                            );
                                        }),
                                    ))
                                    .child(btn("ms-more".into(), "More", false).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            act(
                                                &this.model,
                                                patch(AutopilotPatch {
                                                    max_running: Some((max + 1).min(64)),
                                                    ..AutopilotPatch::default()
                                                }),
                                                cx,
                                            );
                                        }),
                                    )),
                            );
                    }
                    _ => {
                        body = body.child(prose(&format!(
                            "A session parks after {park} minutes without work. A parked session frees its slot and can resume later."
                        )));
                    }
                }
            }
            Section::Providers => {
                list_title = "Usage windows";
                let usage = war.as_ref().map(|w| w.usage.clone()).unwrap_or_default();
                let sel = self.sel.unwrap_or(0).min(usage.len().saturating_sub(1));
                for (k, u) in usage.iter().enumerate() {
                    list = list.child(
                        list_row(
                            format!("ms-use-{k}"),
                            k + 1,
                            &format!("{} {}", u.provider, u.label),
                            if u.estimate {
                                "estimate"
                            } else {
                                "from the provider"
                            },
                            &format!("{}%", u.pct),
                            if u.pct >= 80 { ERR } else { BLUE_HI },
                            k == sel,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sel = Some(k);
                            cx.notify();
                        })),
                    );
                }
                match usage.get(sel) {
                    Some(u) => {
                        detail_title = format!("{} {}", u.provider, u.label);
                        let now = war.as_ref().map_or(0, |w| w.now);
                        let reset = u.resets_at.map_or("unknown".to_string(), |r| {
                            let m = (r - now).max(0) / 60_000;
                            format!("{}h {:02}m", m / 60, m % 60)
                        });
                        let hot = u.pct >= 80;
                        body = body
                            .child(
                                div()
                                    .text_size(ui::ts(T_TITLE))
                                    .text_color(rgb(if hot { ERR } else { BLUE_HI }))
                                    .child(format!("{}%", u.pct)),
                            )
                            .child(
                                div().h(px(10.)).bg(rgb(BLUE_DIM)).child(
                                    div()
                                        .h(px(10.))
                                        .w(relative(f32::from(u.pct.min(100)) / 100.))
                                        .bg(rgb(if hot { ERR } else { BLUE })),
                                ),
                            )
                            .child(prose(&format!("Frees up in {reset}.")))
                            .child(prose(if u.estimate {
                                "The value is an estimate from local transcripts."
                            } else {
                                "The value comes from the provider."
                            }))
                            .child(prose(
                                "Models, effort and permission mode per provider are in All settings.",
                            ));
                    }
                    None => {
                        detail_title = "Usage".to_string();
                        list = list.child(prose("No usage data yet."));
                    }
                }
            }
        }
        let list = mon("ms-list", true, list_title, "2", "pick a row", list)
            .w(px(470.))
            .flex_none()
            .into_any_element();
        let detail = mon("ms-detail", true, &detail_title, "3", "detail", body)
            .flex_1()
            .min_w(px(0.))
            .into_any_element();
        page(head, vec![menu, list, detail])
    }
}
