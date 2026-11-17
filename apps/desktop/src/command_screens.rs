//! The windows of the Command look over the village map (boards `docs/design/looks/rts/`): Tasks
//! (building plans), Repos (villages), Setup and Unit (one session). Every control calls the
//! daemon through `actions.rs`. "FULL SCREEN VIEW" shows the full screen inside the window.

use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::rpc::{AutopilotPatch, SessionRef, TaskListParams, method};
use aivory_protocol::session::{DiffLineKind, SessionDiff};
use aivory_protocol::{Pane, SessionState, Task, TaskColumn, TaskList};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, Stateful, Subscription, Task as GpuiTask,
    Window, div, img, px,
};
use gpui_base::input::{InputEvent, TextareaState};

use crate::actions::{Action, act};
use crate::bridge_screens::{TaskAct, task_act};
use crate::command_kit::{self as cmd, Btn, c};
use crate::compose;
use crate::compose_controls::{self, ControlStyle, Controls};
use crate::model::Model;
use crate::rts::{Sprite, sprites};
use crate::session_console::{ConsoleEvent, ConsoleLook, SessionConsole};
use crate::ui;

/// What a window asks the shell to do.
#[derive(Debug, Clone)]
pub enum CmdEvent {
    OpenSession(String),
    /// Open a file of a session in the code lens.
    OpenFile {
        session: String,
        path: String,
    },
    /// Show the full screen of this view inside the window.
    Full,
    NewUnit,
    Close,
    /// Close the window and go to a village on the map.
    Village(String),
}

/// A sprite in a box of a size, scaled to fit.
fn icon(s: &Sprite, size: f32) -> Div {
    let k = (size / s.w().max(s.h())).min(1.);
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(img(s.render.clone()).w(px(s.w() * k)).h(px(s.h() * k)))
}

/// A sprite at up to twice its size (a portrait), scaled to fit the box.
fn icon_big(s: &Sprite, size: f32) -> Div {
    let k = (size / s.w().max(s.h())).min(5.);
    div()
        .w_full()
        .h(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(img(s.render.clone()).w(px(s.w() * k)).h(px(s.h() * k)))
}

/// A list well that scrolls.
fn list(id: &'static str) -> Stateful<Div> {
    cmd::well()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

fn row(id: impl Into<gpui::ElementId>, h: f32) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .h(px(h))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(14.))
        .border_b_2()
        .border_color(c(cmd::RULE, 1.))
        .hover(|s| s.bg(c(cmd::ROW_HOVER, 1.)))
}

fn tick<V: 'static>(
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

// ================================================================================== Tasks ====

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskTab {
    All,
    Queued,
    Building,
    Waits,
    Review,
}

impl TaskTab {
    fn has(self, t: &Task) -> bool {
        match self {
            Self::All => true,
            Self::Queued => t.column == TaskColumn::Planned,
            Self::Building => t.column == TaskColumn::Working,
            Self::Waits => t.column == TaskColumn::Waiting,
            Self::Review => t.column == TaskColumn::Review,
        }
    }
}

/// The line of a task, and whether it waits for the owner.
fn task_status(t: &Task) -> (String, u32) {
    let of = if t.steps.is_empty() {
        String::new()
    } else {
        format!(
            " · STEP {} OF {}",
            usize::from(t.step).saturating_add(1).min(t.steps.len()),
            t.steps.len()
        )
    };
    match t.column {
        TaskColumn::Planned => ("A PLAN AT THE VILLAGE EDGE".into(), cmd::LABEL),
        TaskColumn::Working => (format!("BUILDS{of}"), cmd::GREEN),
        TaskColumn::Waiting => (format!("WAITS FOR YOU{of}"), cmd::AMBER),
        TaskColumn::Review => ("THE HOUSE STANDS · REVIEW".into(), cmd::GREEN),
        TaskColumn::Done => ("DONE · A COTTAGE".into(), cmd::LABEL),
    }
}

pub struct CommandTasks {
    model: Entity<Model>,
    data: Option<Rc<TaskList>>,
    tab: TaskTab,
    busy: bool,
    _tick: GpuiTask<()>,
    _obs: Subscription,
}

impl EventEmitter<CmdEvent> for CommandTasks {}

impl CommandTasks {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        let tick = tick(cx, Duration::from_secs(5), |t: &mut Self, cx| t.refresh(cx));
        let mut this = Self {
            model,
            data: None,
            tab: TaskTab::All,
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
}

impl Render for CommandTasks {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tasks: Vec<Task> = self.data.as_ref().map_or(Vec::new(), |d| d.tasks.clone());
        let s = sprites();
        let mut tabs = div().flex_none().flex().gap(px(8.)).pb(px(10.));
        for (t, name) in [
            (TaskTab::All, "ALL"),
            (TaskTab::Queued, "QUEUED"),
            (TaskTab::Building, "BUILDING"),
            (TaskTab::Waits, "WAITS"),
            (TaskTab::Review, "REVIEW"),
        ] {
            let n = tasks.iter().filter(|x| t.has(x)).count();
            tabs = tabs.child(
                cmd::plate(
                    format!("ct-tab-{name}"),
                    format!("{name} {n}"),
                    134.,
                    40.,
                    self.tab == t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    cx.notify();
                })),
            );
        }
        tabs = tabs.child(div().flex_1()).child(
            cmd::plate("ct-new", "+ NEW SESSION", 200., 40., false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::NewUnit))),
        );
        let mut rows = list("ct-list");
        let shown: Vec<&Task> = tasks.iter().filter(|t| self.tab.has(t)).collect();
        if self.data.is_none() {
            rows = rows.child(cmd::label("READING THE PLANS…").p(px(12.)));
        } else if shown.is_empty() {
            rows = rows.child(cmd::label("NO TASK IN THIS LIST").p(px(12.)));
        }
        for (k, t) in shown.into_iter().enumerate() {
            let (line, col) = task_status(t);
            let sp = match t.column {
                TaskColumn::Done | TaskColumn::Review => {
                    s.cottage(t.agents.first().map_or("", String::as_str))
                }
                _ => s.site(k as u32),
            };
            let open = t.sessions.first().map(|x| x.session.clone());
            rows = rows.child(
                row(("ct-row", k), 72.)
                    .when(open.is_some(), |d| d.cursor_pointer())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(id) = &open {
                            cx.emit(CmdEvent::OpenSession(id.clone()));
                        }
                    }))
                    .child(icon(sp, 56.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                cmd::entry(cmd::up(&t.title))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis(),
                            )
                            .child(cmd::label(t.codebase.clone())),
                    )
                    .child(
                        cmd::data(line, cmd::S_LABEL)
                            .flex_none()
                            .text_color(c(col, 1.)),
                    )
                    .when_some(TaskAct::of(t.column), |d, a| {
                        let id = t.id.clone();
                        d.child(
                            cmd::plate(
                                format!("ct-act-{k}"),
                                a.label().to_uppercase(),
                                96.,
                                36.,
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    task_act(&this.model, &id, a, Self::reload, cx);
                                },
                            )),
                        )
                    }),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .child(rows.child(div().flex_none().h(px(24.))))
            .child(
                div()
                    .flex_none()
                    .pt(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(cmd::label(
                        "EACH TASK IS A HOUSE · AUTOPILOT STARTS A PLAN WHEN A SLOT IS FREE",
                    ))
                    .child(
                        cmd::plate("ct-full", "FULL SCREEN VIEW", 220., 36., false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::Full))),
                    ),
            )
    }
}

// ================================================================================== Repos ====

pub struct CommandRepos {
    model: Entity<Model>,
    only_units: bool,
    _obs: Subscription,
}

impl EventEmitter<CmdEvent> for CommandRepos {}

impl CommandRepos {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            only_units: false,
            _obs: obs,
        }
    }
}

impl Render for CommandRepos {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let war = self.model.read(cx).war.clone();
        let s = sprites();
        let units = |id: &str| {
            war.as_ref().map_or(0, |w| {
                w.sessions
                    .iter()
                    .filter(|p| p.codebase == id && p.state.is_live())
                    .count()
            })
        };
        let books = war.as_ref().map_or(Vec::new(), |w| w.codebases.clone());
        let with = books.iter().filter(|b| units(&b.id) > 0).count();
        let per = war
            .as_ref()
            .map_or(1, |w| w.autopilot.max_per_codebase.max(1)) as usize;
        let mut tabs = div().flex_none().flex().gap(px(8.)).pb(px(10.));
        for (on, name) in [
            (false, format!("ALL {}", books.len())),
            (true, format!("WITH UNITS {with}")),
        ] {
            tabs = tabs.child(
                cmd::plate(
                    format!("cr-tab-{on}"),
                    name,
                    170.,
                    40.,
                    self.only_units == on,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.only_units = on;
                    cx.notify();
                })),
            );
        }
        tabs = tabs.child(div().flex_1()).child(
            cmd::plate("cr-add", "+ NEW VILLAGE", 170., 40., false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::Full))),
        );
        let mut rows = list("cr-list");
        if books.is_empty() {
            rows = rows.child(cmd::label("NO CODEBASE YET").p(px(12.)));
        }
        for (k, b) in books.iter().enumerate() {
            let n = units(&b.id);
            if self.only_units && n == 0 {
                continue;
            }
            let run = war.as_ref().map_or(0, |w| {
                w.sessions
                    .iter()
                    .filter(|p| {
                        p.codebase == b.id
                            && matches!(p.state, SessionState::Working | SessionState::NeedsYou)
                    })
                    .count()
            });
            let free = per.saturating_sub(run);
            let name = b.id.clone();
            let sub = if b.worktrees > 1 {
                format!("{} · {} worktrees", b.branch, b.worktrees)
            } else {
                b.branch.clone()
            };
            rows = rows.child(
                row(("cr-row", k), 72.)
                    .child(icon(&s.hall, 56.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(cmd::entry(b.id.clone()))
                            .child(cmd::label(sub)),
                    )
                    .child({
                        let (wait, err) = war.as_ref().map_or((0, 0), |w| {
                            let of = |st: SessionState| {
                                w.sessions
                                    .iter()
                                    .filter(|p| p.codebase == b.id && p.state == st)
                                    .count()
                            };
                            (of(SessionState::NeedsYou), of(SessionState::Error))
                        });
                        let (text, col) = if err > 0 {
                            (format!("{err} FAULT"), cmd::RED)
                        } else if wait > 0 {
                            (format!("{wait} WAIT FOR YOU"), cmd::AMBER)
                        } else if run > 0 {
                            (format!("{run} AT WORK"), cmd::GREEN)
                        } else {
                            ("QUIET".to_string(), cmd::HINT)
                        };
                        cmd::data(text, cmd::S_ENTRY)
                            .w(px(210.))
                            .flex_none()
                            .text_color(c(col, 1.))
                    })
                    .child(
                        cmd::data(
                            format!("{n} UNIT{}", if n == 1 { "" } else { "S" }),
                            cmd::S_ENTRY,
                        )
                        .w(px(90.))
                        .flex_none(),
                    )
                    .child(
                        cmd::data(
                            if free == 0 {
                                format!("{run} OF {per} RUN")
                            } else {
                                format!("{free} FREE")
                            },
                            cmd::S_ENTRY,
                        )
                        .w(px(100.))
                        .flex_none()
                        .text_color(c(if free > 0 { cmd::GREEN } else { cmd::WHITE }, 1.)),
                    )
                    .child(
                        cmd::plate(("cr-go", k), "OPEN MAP", 104., 40., false).on_click(
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(CmdEvent::Village(name.clone()));
                            }),
                        ),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .child(rows.child(div().flex_none().h(px(24.))))
            .child(
                div()
                    .flex_none()
                    .pt(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(cmd::label(
                        "OPEN MAP MOVES THE MAP · A VILLAGE GROWS WITH ITS UNITS",
                    ))
                    .child(
                        cmd::plate("cr-full", "FULL SCREEN VIEW", 220., 36., false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::Full))),
                    ),
            )
    }
}

// ================================================================================== Setup ====

pub struct CommandSetup {
    model: Entity<Model>,
    looks: bool,
    _obs: Subscription,
}

impl EventEmitter<CmdEvent> for CommandSetup {}

impl CommandSetup {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            looks: false,
            _obs: obs,
        }
    }

    /// One value with a bar, a minus and a plus button.
    #[allow(clippy::too_many_arguments)]
    fn stepper(
        &self,
        id: &'static str,
        name: &str,
        value: u32,
        shown: String,
        (lo, hi, step): (u32, u32, u32),
        set: fn(u32) -> Action,
        cx: &mut Context<Self>,
    ) -> Div {
        let share = (value.saturating_sub(lo)) as f32 / (hi - lo).max(1) as f32;
        div()
            .h(px(40.))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(cmd::entry(name.to_string()).w(px(190.)).flex_none())
            .child(
                cmd::button((id, 0usize), "-", Btn::Plain).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if value > lo {
                            act(&this.model, set(value.saturating_sub(step).max(lo)), cx);
                        }
                    },
                )),
            )
            .child(div().flex_1().child(cmd::bar(share)))
            .child(
                cmd::button((id, 1usize), "+", Btn::Plain).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if value < hi {
                            act(&this.model, set((value + step).min(hi)), cx);
                        }
                    },
                )),
            )
            .child(cmd::entry(shown).w(px(84.)).flex_none().text_right())
    }
}

impl Render for CommandSetup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use aivory_protocol::Autopilot as Ap;
        let war = self.model.read(cx).war.clone();
        let auto = war.as_ref().map(|w| w.autopilot.clone());
        let on = auto.as_ref().is_some_and(|a| a.on);
        let here = crate::theme::tk().look;
        let mut body = div()
            .id("cs-body")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                cmd::plate(
                    "cs-look",
                    format!("LOOK · {}", cmd::up(crate::look::name(here))),
                    960.,
                    32.,
                    self.looks,
                )
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.looks = !this.looks;
                    cx.notify();
                })),
            );
        if self.looks {
            let mut grid = div().flex().flex_wrap().gap(px(8.));
            for (k, (look, name)) in crate::look::LOOKS.into_iter().enumerate() {
                grid = grid.child(
                    cmd::plate(("cs-pick", k), cmd::up(name), 228., 40., look == here).on_click(
                        cx.listener(move |this, _, _, cx| {
                            act(&this.model, Action::SetLook(look), cx);
                            this.looks = false;
                            cx.notify();
                        }),
                    ),
                );
            }
            body = body.child(grid);
        }
        body = body
            .child(
                cmd::plate(
                    "cs-auto",
                    format!("AUTOPILOT · {}", if on { "ON" } else { "OFF" }),
                    960.,
                    32.,
                    false,
                )
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| {
                    act(
                        &this.model,
                        Action::Autopilot(AutopilotPatch {
                            on: Some(!on),
                            ..AutopilotPatch::default()
                        }),
                        cx,
                    );
                })),
            )
            // "OTHER": the full settings screen (devices, machines, all other settings).
            .child(
                cmd::plate(
                    "cs-full",
                    "ALL SETTINGS · AGENTS, NOTICES, DEVICES",
                    960.,
                    32.,
                    false,
                )
                .w_full()
                .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::Full))),
            );
        if let Some(a) = &auto {
            let (lo, hi) = Ap::MAX_RUNNING;
            let run = self.stepper(
                "cs-max",
                "MAX UNITS",
                a.max_running,
                a.max_running.to_string(),
                (lo, hi, 1),
                |v| {
                    Action::Autopilot(AutopilotPatch {
                        max_running: Some(v),
                        ..AutopilotPatch::default()
                    })
                },
                cx,
            );
            let (lo, hi) = Ap::MAX_PER_CODEBASE;
            let per = self.stepper(
                "cs-per",
                "UNITS PER VILLAGE",
                a.max_per_codebase,
                a.max_per_codebase.to_string(),
                (lo, hi, 1),
                |v| {
                    Action::Autopilot(AutopilotPatch {
                        max_per_codebase: Some(v),
                        ..AutopilotPatch::default()
                    })
                },
                cx,
            );
            let park = self.stepper(
                "cs-park",
                "PARK IDLE AFTER",
                a.park_after_min,
                format!("{} MIN", a.park_after_min),
                Ap::PARK_AFTER_MIN,
                |v| {
                    Action::Autopilot(AutopilotPatch {
                        park_after_min: Some(v),
                        ..AutopilotPatch::default()
                    })
                },
                cx,
            );
            let (lo, hi) = aivory_protocol::Display::UI_SCALE;
            let scale = u32::from(ui::ui_scale());
            let text = self.stepper(
                "cs-text",
                "TEXT SIZE",
                scale,
                format!("{scale} %"),
                (u32::from(lo), u32::from(hi), 5),
                |v| Action::SetUiScale(v as u16),
                cx,
            );
            body = body
                .child(cmd::label("AUTOPILOT LIMITS").mt(px(8.)).mb(px(2.)))
                .child(
                    cmd::well()
                        .px(px(14.))
                        .py(px(8.))
                        .flex()
                        .flex_col()
                        .child(run)
                        .child(per)
                        .child(park)
                        .child(text),
                );
        } else {
            body = body.child(cmd::label("WAITING FOR THE DAEMON"));
        }
        // Looks, providers and agents: three steel wells that fill the lower half.
        if let Some(w) = &war {
            let head =
                |t: &str| cmd::data(t.to_string(), cmd::S_ENTRY).text_color(c(cmd::AMBER, 1.));
            let mut looks = cmd::well()
                .flex_1()
                .min_w(px(0.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(head("LOOKS"));
            let mut cols = [
                div().flex_1().flex().flex_col().gap(px(3.)),
                div().flex_1().flex().flex_col().gap(px(3.)),
            ];
            for (k, (look, name)) in crate::look::LOOKS.into_iter().enumerate() {
                let on = look == here;
                let col = std::mem::replace(&mut cols[k / 4], div());
                cols[k / 4] = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .size(px(8.))
                                .bg(c(if on { cmd::LAMP_RUN } else { cmd::RULE_2 }, 1.)),
                        )
                        .child(
                            cmd::data(cmd::up(name), cmd::S_LABEL)
                                .text_color(c(if on { cmd::WHITE } else { cmd::LABEL }, 1.)),
                        ),
                );
            }
            let [c0, c1] = cols;
            looks = looks.child(div().flex().gap(px(10.)).child(c0).child(c1));
            let mut prov = cmd::well()
                .flex_1()
                .min_w(px(0.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(head("PROVIDERS"));
            for u in w.usage.iter().take(3) {
                prov = prov.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .child(cmd::data(
                                    cmd::up(&format!("{} {}", u.provider, u.label)),
                                    cmd::S_LABEL,
                                ))
                                .child(cmd::data(format!("{}%", u.pct), cmd::S_LABEL).text_color(
                                    c(if u.pct >= 80 { cmd::RED } else { cmd::LED }, 1.),
                                )),
                        )
                        .child(cmd::bar(f32::from(u.pct) / 100.)),
                );
            }
            let mut agents = cmd::well()
                .flex_1()
                .min_w(px(0.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(head("AGENTS"));
            for provider in ["claude", "codex", "gemini"] {
                let live = w
                    .sessions
                    .iter()
                    .filter(|s| s.provider == provider && s.state.is_live())
                    .count();
                agents = agents.child(
                    div()
                        .flex()
                        .justify_between()
                        .child(cmd::data(cmd::up(provider), cmd::S_ENTRY))
                        .child(
                            cmd::data(format!("{live} LIVE"), cmd::S_ENTRY)
                                .text_color(c(if live > 0 { cmd::LED } else { cmd::HINT }, 1.)),
                        ),
                );
            }
            body = body.child(
                div()
                    .mt(px(10.))
                    .flex_none()
                    .flex()
                    .gap(px(10.))
                    .child(looks)
                    .child(prov)
                    .child(agents),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(body)
            .child(
                div().flex_none().flex().justify_end().gap(px(10.)).child(
                    cmd::plate("cs-done", "DONE", 120., 38., true)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::Close))),
                ),
            )
    }
}

// =================================================================================== Unit ====

pub struct CommandUnit {
    model: Entity<Model>,
    id: String,
    input: Entity<TextareaState>,
    tools: compose::Tools,
    /// The composer controls: mode, provider, model, effort.
    controls: Controls,
    tab: UnitTab,
    /// The radio (the last messages): its scroll bar follows the text.
    radio_scroll: gpui::ScrollHandle,
    console: Entity<SessionConsole>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    _obs: Subscription,
    _events: Subscription,
    _console_events: Subscription,
}

impl EventEmitter<CmdEvent> for CommandUnit {}

/// The tabs of a Unit window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitTab {
    Orders,
    Changes,
    Console,
}

/// How old a diff gets before the window asks again.
const DIFF_TTL: Duration = Duration::from_secs(8);
/// The diff shows this many lines at most.
const DIFF_LINES: usize = 400;

fn fact(name: &str, value: String) -> Div {
    div()
        .flex()
        .justify_between()
        .child(cmd::label(name.to_string()))
        .child(cmd::entry(value))
}

impl CommandUnit {
    pub fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let obs = cx.observe(&model, |_, _, cx| cx.notify());
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("NEW ORDERS FOR THIS UNIT")
                .auto_grow(1, 3)
                .submit_on_enter(true)
        });
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event
                && !crate::complete::is_open(cx)
            {
                this.send(window, cx);
            }
        });
        let weak = cx.weak_entity();
        crate::complete::attach(
            &input,
            model.clone(),
            move |cx| weak.upgrade().map(|v| v.read(cx).id.clone()),
            |_| (None, None),
            cx,
        );
        let console = cx.new(|cx| SessionConsole::new(model.clone(), id.clone(), cx));
        console.update(cx, |c, _| {
            c.set_look(ConsoleLook {
                theme: Some(crate::theme::PaneTheme {
                    bg: cmd::WELL << 8 | 0xFF,
                    text: cmd::INK << 8 | 0xFF,
                    dim: cmd::HINT << 8 | 0xFF,
                    ..crate::theme::PaneTheme::DARK
                }),
                size: cmd::S_LABEL,
                pad_x: 12.,
                max_w: None,
            })
        });
        let console_events = cx.subscribe(&console, |_, _, e: &ConsoleEvent, cx| {
            let ConsoleEvent::OpenFile { session, path } = e;
            cx.emit(CmdEvent::OpenFile {
                session: session.clone(),
                path: path.clone(),
            });
        });
        let tab = if crate::session_console::take_want_code(&id) {
            UnitTab::Changes
        } else {
            UnitTab::Orders
        };
        Self {
            model,
            id,
            input,
            tools: compose::Tools::default(),
            controls: Controls::default(),
            tab,
            radio_scroll: gpui::ScrollHandle::new(),
            console,
            diff: None,
            busy: false,
            _obs: obs,
            _events: events,
            _console_events: console_events,
        }
    }

    /// Read the diff again when the last read is old.
    fn ensure_diff(&mut self, cx: &mut Context<Self>) {
        if self.busy
            || self
                .diff
                .as_ref()
                .is_some_and(|(_, t)| t.elapsed() <= DIFF_TTL)
        {
            return;
        }
        self.busy = true;
        let id = self.id.clone();
        let task = self
            .model
            .read(cx)
            .call::<method::SessionDiffMethod>(SessionRef { id: id.clone() }, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.id == id {
                    this.diff = Some((result.ok(), Instant::now()));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The code changes of the unit: every file with its lines.
    fn changes(&self) -> AnyElement {
        let files = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let mut col = div()
            .id("cu-diff")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if files.is_empty() {
            let text = if self.diff.is_some() {
                "NO CHANGES YET"
            } else {
                "READING CHANGES"
            };
            return col.child(cmd::label(text)).into_any_element();
        }
        let mut left = DIFF_LINES;
        for f in &files {
            col = col.child(div().flex_none().mt(px(8.)).child(cmd::entry(format!(
                "{} +{} -{}",
                f.path, f.added, f.removed
            ))));
            for l in f.lines.iter().take(left) {
                let (mark, color) = match l.kind {
                    DiffLineKind::Added => ("+ ", cmd::GREEN),
                    DiffLineKind::Removed => ("- ", cmd::RED),
                    DiffLineKind::Hunk => ("", cmd::HINT),
                    DiffLineKind::Context => ("  ", cmd::INK_3),
                };
                col = col.child(
                    cmd::data(format!("{mark}{}", l.text.trim_end()), cmd::S_LABEL)
                        .flex_none()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(c(color, 1.)),
                );
            }
            left = left.saturating_sub(f.lines.len());
            if left == 0 {
                break;
            }
        }
        col.into_any_element()
    }

    fn pane(&self, cx: &Context<Self>) -> Option<Pane> {
        self.model.read(cx).war.as_ref()?.session(&self.id).cloned()
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(pane) = self.pane(cx) else {
            return;
        };
        if let Some(ask) = &pane.ask {
            act(
                &self.model,
                Action::Answer {
                    id: self.id.clone(),
                    ask: ask.id.clone(),
                    option: None,
                    text: Some(text),
                },
                cx,
            );
        } else {
            let prefs = self.controls.current(&pane);
            let images = self.tools.images();
            compose_controls::submit(&self.model, &pane, text, images, prefs, cx);
        }
        self.tools.sent();
        self.input.update(cx, |s, cx| s.clean(window, cx));
        cx.notify();
    }

    fn answer(&mut self, k: usize, cx: &mut Context<Self>) {
        let Some(p) = self.pane(cx) else {
            return;
        };
        let Some(ask) = p.ask else {
            return;
        };
        let Some(opt) = ask.options.get(k).cloned() else {
            return;
        };
        act(
            &self.model,
            Action::Answer {
                id: self.id.clone(),
                ask: ask.id,
                option: Some(opt),
                text: None,
            },
            cx,
        );
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

    fn side(&self, p: &Pane, cx: &mut Context<Self>) -> Div {
        let s = sprites();
        let sp = if matches!(p.state, SessionState::Done | SessionState::Parked) {
            s.cottage(&p.provider)
        } else {
            s.site(crate::rts::layout::hash(&p.id))
        };
        let id = self.id.clone();
        let mut side = div()
            .w(px(280.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                cmd::well()
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(icon_big(sp, 190.))
                    .child(cmd::entry(format!(
                        "{} · {}",
                        cmd::up(&p.provider),
                        cmd::up(&p.model)
                    )))
                    .child(cmd::label(format!("{} · {}", p.codebase, p.part))),
            )
            .child(
                cmd::well()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(fact("CONTEXT", format!("{}%", p.context_pct)))
                    .child(cmd::bar(f32::from(p.context_pct) / 100.))
                    .child(fact("COST", format!("${:.2}", p.cost)))
                    .child(fact("TOKENS", cmd::tokens(p.tokens)))
                    .child(fact("HELPERS", p.subagents.len().to_string())),
            );
        let mut helpers = cmd::well()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(cmd::label(if p.subagents.is_empty() {
                "NO HELPERS ON THIS UNIT"
            } else {
                "HELPERS"
            }));
        for h in p.subagents.iter().take(4) {
            helpers = helpers.child(
                cmd::entry(cmd::up(&h.name))
                    .flex_none()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis(),
            );
        }
        if p.subagents.len() > 4 {
            helpers = helpers.child(cmd::label(format!("+{} MORE", p.subagents.len() - 4)));
        }
        if let Some(t) = p.task.as_ref().filter(|t| !t.trim().is_empty()) {
            helpers = helpers.child(cmd::label("TASK").flex_none()).child(
                cmd::entry(cmd::up(t))
                    .flex_none()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis(),
            );
        }
        // The well always shows the last orders: tool lines first, else the latest chat turns.
        let mut orders: Vec<_> = p
            .tail
            .iter()
            .filter(|l| l.kind == aivory_protocol::LineKind::Tool)
            .rev()
            .take(5)
            .collect();
        if orders.is_empty() {
            orders = p
                .tail
                .iter()
                .filter(|l| {
                    matches!(
                        l.kind,
                        aivory_protocol::LineKind::User | aivory_protocol::LineKind::Agent
                    ) && {
                        let t = l.text.trim_start();
                        !t.is_empty() && !t.starts_with(['|', '`', '#'])
                    }
                })
                .rev()
                .take(5)
                .collect();
        }
        helpers = helpers.child(cmd::label("LAST ORDERS").mt(px(8.)));
        if orders.is_empty() {
            helpers = helpers.child(cmd::label("NO ORDERS YET"));
        }
        for l in orders.into_iter().rev() {
            let tag = match l.kind {
                aivory_protocol::LineKind::User => "YOU".to_string(),
                aivory_protocol::LineKind::Agent => "AI".to_string(),
                _ => cmd::up(l.label.as_deref().unwrap_or("TOOL")),
            };
            helpers = helpers.child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(8.))
                    .child(
                        cmd::data(tag, cmd::S_LABEL)
                            .flex_none()
                            .text_color(c(cmd::LED, 1.)),
                    )
                    .child(
                        cmd::data(
                            l.text.lines().next().unwrap_or("").to_string(),
                            cmd::S_LABEL,
                        )
                        .flex_1()
                        .min_w(px(0.))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis(),
                    ),
            );
        }
        side = side.child(helpers);
        if p.state == SessionState::Parked {
            side = side.child(cmd::plate("cu-resume", "RESUME", 280., 40., true).on_click(
                cx.listener(move |this, _, _, cx| act(&this.model, Action::Resume(id.clone()), cx)),
            ));
        } else if p.controlled && p.state.is_live() {
            let (a, b) = (id.clone(), id);
            side = side.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        cmd::plate("cu-park", "PARK", 136., 40., false).on_click(cx.listener(
                            move |this, _, _, cx| act(&this.model, Action::Park(a.clone()), cx),
                        )),
                    )
                    .child(
                        cmd::plate("cu-stop", "STOP", 136., 40., false).on_click(cx.listener(
                            move |this, _, _, cx| act(&this.model, Action::Stop(b.clone()), cx),
                        )),
                    ),
            );
        }
        side
    }
}

impl Render for CommandUnit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = self.pane(cx) else {
            let empty: AnyElement = cmd::label("THIS UNIT IS GONE")
                .p(px(12.))
                .into_any_element();
            return div().size_full().child(empty);
        };
        self.ensure_diff(cx);
        let vp = window.viewport_size();
        let (ww, _) = cmd::win_size(
            f32::from(vp.width),
            f32::from(vp.height),
            crate::warroom::skins::command::SIDE_W,
        );
        // Under 980 px of window width the unit card hides and the chat takes the space.
        let narrow = ww < 980.;
        let side = if narrow {
            None
        } else {
            Some(self.side(&p, cx))
        };
        let mut tabs = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.));
        for (id, label, tab) in [
            ("cu-t-orders", "ORDERS", UnitTab::Orders),
            ("cu-t-changes", "CHANGES", UnitTab::Changes),
            ("cu-t-console", "CONSOLE", UnitTab::Console),
        ] {
            let kind = if self.tab == tab { Btn::Go } else { Btn::Plain };
            tabs = tabs.child(cmd::button(id, label, kind).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                },
            )));
        }
        if narrow {
            let id = self.id.clone();
            if p.state == SessionState::Parked {
                tabs = tabs.child(cmd::button("cu-resume", "RESUME", Btn::Go).on_click(
                    cx.listener(move |this, _, _, cx| {
                        act(&this.model, Action::Resume(id.clone()), cx)
                    }),
                ));
            } else if p.controlled && p.state.is_live() {
                let (a, b) = (id.clone(), id);
                tabs = tabs
                    .child(
                        cmd::button("cu-park", "PARK", Btn::Plain).on_click(cx.listener(
                            move |this, _, _, cx| act(&this.model, Action::Park(a.clone()), cx),
                        )),
                    )
                    .child(
                        cmd::button("cu-stop", "STOP", Btn::Plain).on_click(cx.listener(
                            move |this, _, _, cx| act(&this.model, Action::Stop(b.clone()), cx),
                        )),
                    );
            }
        }
        if self.tab != UnitTab::Orders {
            let body: AnyElement = if self.tab == UnitTab::Changes {
                self.changes()
            } else {
                self.console.clone().into_any_element()
            };
            let main = cmd::well()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(tabs)
                .child(div().flex_1().min_h(px(0.)).child(body));
            return div()
                .size_full()
                .flex()
                .gap(px(cmd::GAP))
                .children(side)
                .child(main);
        }
        let mut msgs = div()
            .id("cu-radio")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.radio_scroll)
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(cmd::label("RADIO · THE LAST MESSAGES"));
        for (k, d) in crate::console::display::last(&p.tail, 60)
            .into_iter()
            .enumerate()
        {
            use crate::console::display::DKind;
            if d.kind == DKind::Meta {
                let found = p
                    .tail
                    .iter()
                    .rev()
                    .find(|l| l.media.is_some() && l.text.trim_end() == d.text);
                if let Some(m) = found.and_then(|l| l.media.as_ref()) {
                    let rgba = |v: u32| gpui::rgba(v << 8 | 0xFF);
                    let style = crate::media::MediaStyle {
                        card: rgba(cmd::CARD),
                        text: rgba(cmd::INK),
                        muted: rgba(cmd::HINT),
                        radius: 0.,
                    };
                    msgs = msgs.child(crate::media::preview(m, ("cu-media", k), style));
                }
                continue;
            }
            if d.kind == DKind::Output {
                continue;
            }
            // The speaker shows once per message, on its first line.
            let (who, col) = if !d.first {
                (String::new(), cmd::INK)
            } else {
                match d.kind {
                    DKind::You => ("YOU".to_string(), cmd::AMBER),
                    DKind::Tool => (cmd::up(d.label.as_deref().unwrap_or("TOOL")), cmd::INK_3),
                    DKind::Error => ("ERROR".to_string(), cmd::RED),
                    _ => (cmd::up(&p.provider), cmd::GREEN),
                }
            };
            let (text, color) = match d.kind {
                DKind::Heading => (cmd::up(&d.text), cmd::WHITE),
                DKind::Code | DKind::Table | DKind::Tool => (d.text.clone(), cmd::INK_3),
                DKind::Error => (d.text.clone(), cmd::RED),
                _ => (d.text.clone(), cmd::INK),
            };
            msgs = msgs.child(
                div()
                    .id(("cu-line", k))
                    .flex()
                    .gap(px(8.))
                    .child(
                        cmd::data(who, cmd::S_ENTRY)
                            .flex_none()
                            .w(px(110.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(c(col, 1.)),
                    )
                    .child(
                        cmd::data(text, cmd::S_ENTRY)
                            .flex_1()
                            .min_w(px(0.))
                            .text_color(c(color, 1.)),
                    ),
            );
        }
        // Space under the last line, so the scroll area never cuts a row.
        msgs = msgs.child(div().flex_none().h(px(12.)));
        let mut main = cmd::well()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(tabs)
            .child(crate::ui::with_thumb(
                msgs,
                &self.radio_scroll,
                gpui::rgba(0x7FCB4FB3),
                4.,
            ));
        if let Some(a) = &p.ask {
            main = main.child(
                div()
                    .flex_none()
                    .pt(px(10.))
                    .border_t_2()
                    .border_color(c(cmd::RULE_2, 1.))
                    .child(
                        cmd::data(
                            format!("{} {}", cmd::up(a.kind.label()), a.text),
                            cmd::S_ENTRY,
                        )
                        .text_color(c(cmd::AMBER, 1.)),
                    ),
            );
            let n = a.options.len();
            let mut row = div().flex_none().flex().flex_wrap().gap(px(8.));
            for (k, o) in a.options.iter().enumerate() {
                let kind = if k == 0 {
                    Btn::Go
                } else if k + 1 == n && n > 1 {
                    Btn::Quiet
                } else {
                    Btn::Plain
                };
                row = row.child(
                    cmd::button(("cu-ans", k), format!("{} {}", k + 1, cmd::up(o)), kind)
                        .on_click(cx.listener(move |this, _, _, cx| this.answer(k, cx))),
                );
            }
            main = main.child(row);
        }
        let listening = self.tools.listening.is_some();
        let rgba = |v: u32| v << 8 | 0xFF;
        let palette = compose::Palette {
            text: rgba(cmd::INK),
            dim: rgba(cmd::HINT),
            line: rgba(cmd::RULE_2),
            chip_bg: rgba(cmd::CARD),
            hover: rgba(cmd::ROW_HOVER),
            fail: rgba(cmd::RED),
            ok: rgba(cmd::GREEN),
        };
        let chips = compose::chips(&self.tools, "cu", palette, |i| {
            Box::new(cx.listener(move |this, _, _, cx| {
                this.tools.remove(i);
                cx.notify();
            }))
        });
        main = main.children(chips);
        self.controls.sync(&p);
        let providers = self.model.read(cx).providers.clone();
        let style = ControlStyle::console(cmd::PANEL, cmd::INK, cmd::GREEN, Some(cmd::SANS.into()));
        main = main.child(self.controls.row(
            "cu-ctl",
            &p,
            &providers,
            &style,
            |this: &mut Self| &mut this.controls,
            cx,
        ));
        main = main.child(
            div()
                .flex_none()
                .min_h(px(52.))
                .flex()
                .items_center()
                .gap(px(12.))
                .pl(px(14.))
                .pr(px(8.))
                .py(px(7.))
                .bg(c(cmd::CARD, 1.))
                .font_family(cmd::SANS)
                .text_size(ui::ts(cmd::S_ENTRY))
                .text_color(c(cmd::INK, 1.))
                .child(div().flex_1().min_w(px(0.)).child(self.input.clone()))
                .children((!narrow).then(|| cmd::label("ENTER").text_color(c(cmd::HINT, 1.))))
                .child(
                    cmd::mic("cu-mic", listening)
                        .on_click(cx.listener(|this, _, window, cx| this.dictate(window, cx))),
                ),
        );
        // A file dropped anywhere on the chat attaches.
        let main = compose::media_target(main, |this: &mut Self| Some(&mut this.tools), cx);
        div()
            .size_full()
            .flex()
            .gap(px(cmd::GAP))
            .children(side)
            .child(main)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tabs_sort_the_tasks_by_column() {
        assert!(matches!(TaskTab::Queued, TaskTab::Queued));
        assert_ne!(TaskTab::All, TaskTab::Review);
    }
}
