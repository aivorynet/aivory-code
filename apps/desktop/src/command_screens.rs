//! The windows of the Command look over the village map (boards `docs/design/looks/rts/`): Tasks
//! (building plans), Repos (villages), Setup and Unit (one session). Every control calls the
//! daemon through `actions.rs`. "ALL FUNCTIONS" shows the full screen inside the window.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::rpc::{AutopilotPatch, TaskListParams, method};
use aivory_protocol::{Mode, Pane, SessionState, Task, TaskColumn, TaskList};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, Stateful, Subscription, Task as GpuiTask,
    Window, div, img, px,
};
use gpui_base::input::{InputEvent, TextareaState};

use crate::actions::{Action, act};
use crate::command_kit::{self as cmd, Btn, c};
use crate::compose;
use crate::model::Model;
use crate::rts::{Sprite, sprites};
use crate::ui;
use crate::warroom::skins::command::{last_lines, line_head};

/// What a window asks the shell to do.
#[derive(Debug, Clone)]
pub enum CmdEvent {
    OpenSession(String),
    Focus(String),
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
    Review,
}

impl TaskTab {
    fn has(self, t: &Task) -> bool {
        match self {
            Self::All => true,
            Self::Queued => t.column == TaskColumn::Planned,
            Self::Building => matches!(t.column, TaskColumn::Working | TaskColumn::Waiting),
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
            cmd::plate("ct-new", "+ NEW UNIT", 150., 40., false)
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
                    .child(cmd::data(line, 18.).flex_none().text_color(c(col, 1.))),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .child(rows)
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
                        cmd::plate("ct-full", "ALL FUNCTIONS", 180., 36., false)
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
                    .child(
                        cmd::data(format!("{n} UNIT{}", if n == 1 { "" } else { "S" }), 19.)
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
                            19.,
                        )
                        .w(px(100.))
                        .flex_none()
                        .text_color(c(if free > 0 { cmd::GREEN } else { cmd::WHITE }, 1.)),
                    )
                    .child(
                        cmd::plate(("cr-go", k), "GO TO", 104., 40., false).on_click(cx.listener(
                            move |_, _, _, cx| {
                                cx.emit(CmdEvent::Village(name.clone()));
                            },
                        )),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .child(rows)
            .child(
                div()
                    .flex_none()
                    .pt(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(cmd::label(
                        "GO TO MOVES THE MAP · A VILLAGE GROWS WITH ITS UNITS",
                    ))
                    .child(
                        cmd::plate("cr-full", "ALL FUNCTIONS", 180., 36., false)
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
            .h(px(44.))
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
        let mut body = div().size_full().flex().flex_col().gap(px(8.)).child(
            cmd::plate(
                "cs-look",
                format!("LOOK · {}", cmd::up(crate::look::name(here))),
                960.,
                44.,
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
                cmd::plate("cs-agents", "AGENTS AND MODELS", 960., 44., false)
                    .w_full()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::Full))),
            )
            .child(
                cmd::plate(
                    "cs-auto",
                    format!("AUTOPILOT · {}", if on { "ON" } else { "OFF" }),
                    960.,
                    44.,
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
            .child(
                cmd::plate("cs-notes", "NOTICES AND SOUND", 960., 44., false)
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
            body = body.child(
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
        body.child(div().flex_1()).child(
            div()
                .flex_none()
                .flex()
                .justify_end()
                .gap(px(10.))
                .child(
                    cmd::plate("cs-full", "ALL SETTINGS", 170., 44., false)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(CmdEvent::Full))),
                )
                .child(
                    cmd::plate("cs-done", "DONE", 120., 44., true)
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
    /// The mode for the next order; the mode of the session when `None`.
    mode: Option<Mode>,
    _obs: Subscription,
    _events: Subscription,
}

impl EventEmitter<CmdEvent> for CommandUnit {}

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
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.send(window, cx);
            }
        });
        Self {
            model,
            id,
            input,
            tools: compose::Tools::default(),
            mode: None,
            _obs: obs,
            _events: events,
        }
    }

    fn pane(&self, cx: &Context<Self>) -> Option<Pane> {
        self.model.read(cx).war.as_ref()?.session(&self.id).cloned()
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let composer = self.mode.take().and_then(|mode| {
            let mut prefs = self.pane(cx)?.composer;
            prefs.mode = mode;
            Some(prefs)
        });
        act(
            &self.model,
            Action::Send {
                id: self.id.clone(),
                text,
                composer,
            },
            cx,
        );
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
        let mode = self.mode.unwrap_or(p.composer.mode);
        let id = self.id.clone();
        let focus = self.id.clone();
        let mut side = div()
            .w(px(280.))
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
                    .child(icon(sp, 120.))
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
                    .child(fact("COST", format!("${:.2}", p.cost)))
                    .child(fact("TOKENS", cmd::tokens(p.tokens)))
                    .child(fact("HELPERS", p.subagents.len().to_string())),
            )
            .child(
                cmd::plate(
                    "cu-mode",
                    format!(
                        "MODE · {}",
                        match mode {
                            Mode::Plan => "PLAN",
                            Mode::Edit => "EDIT",
                            Mode::Auto => "AUTO",
                        }
                    ),
                    280.,
                    40.,
                    self.mode.is_some(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.mode = Some(match mode {
                        Mode::Plan => Mode::Edit,
                        Mode::Edit => Mode::Auto,
                        Mode::Auto => Mode::Plan,
                    });
                    cx.notify();
                })),
            )
            .child(
                cmd::plate("cu-focus", "FOCUS VIEW", 280., 40., false).on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(CmdEvent::Focus(focus.clone()))),
                ),
            );
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = self.pane(cx) else {
            let empty: AnyElement = cmd::label("THIS UNIT IS GONE")
                .p(px(12.))
                .into_any_element();
            return div().size_full().child(empty);
        };
        let side = self.side(&p, cx);
        let mut msgs = div()
            .id("cu-radio")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(cmd::label("RADIO · THE LAST MESSAGES"));
        for (k, l) in last_lines(&p.tail, 24).into_iter().enumerate() {
            let (who, col) = line_head(&l, &p.provider);
            msgs = msgs.child(
                div()
                    .id(("cu-line", k))
                    .flex()
                    .gap(px(8.))
                    .child(cmd::data(who, 20.).flex_none().text_color(c(col, 1.)))
                    .child(cmd::data(l.text.clone(), 20.).min_w(px(0.))),
            );
        }
        let mut main = cmd::well()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(msgs);
        if let Some(a) = &p.ask {
            main = main.child(
                div()
                    .flex_none()
                    .pt(px(10.))
                    .border_t_2()
                    .border_color(c(cmd::RULE_2, 1.))
                    .child(
                        cmd::data(format!("{} {}", cmd::up(a.kind.label()), a.text), 20.)
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
                .child(cmd::label("ENTER").text_color(c(cmd::HINT, 1.)))
                .child(
                    cmd::mic("cu-mic", listening)
                        .on_click(cx.listener(|this, _, window, cx| this.dictate(window, cx))),
                ),
        );
        div()
            .size_full()
            .flex()
            .gap(px(cmd::GAP))
            .child(side)
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
