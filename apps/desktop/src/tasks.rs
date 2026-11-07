//! The Tasks screen (`Tasks.dc.html`): every task on a board of five columns, each task with its
//! playbook. Right the panel of the selected task.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::rpc::{
    HandoffParams, NewSessionParams, StartWhen, TaskCreateParams, TaskListParams, TaskUpdateParams,
    method,
};
use aivory_protocol::{
    AskKind, ComposerPrefs, Effort, GateState, MINUTE_MS, Mode, Playbook, ProviderPrefs,
    ProviderStatus, SessionState, StepState, Task, TaskColumn, UnixMs,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, BoxShadow, Context, Div, Entity, EventEmitter, FontWeight, Render, SharedString,
    Subscription, Window, deferred, div, point, px, relative, rgb, rgba, svg,
};
use gpui_base::input::{InputEvent, InputState, TextareaState};

use crate::model::Model;
use crate::theme::*;
use crate::ui;
use crate::vtext;

#[derive(Debug, Clone)]
#[allow(
    clippy::enum_variant_names,
    reason = "the events name what the shell opens: a session, a diff, the Inbox"
)]
pub enum TasksEvent {
    /// Open the deep dive of a session.
    OpenSession(String),
    /// Open the deep dive of a session on its Diff tab (review the diff, open the merge).
    OpenDiff(String),
    /// Switch to the Inbox.
    /// Open the Inbox with the ask of one of these sessions selected.
    OpenInbox(Vec<String>),
}

/// Under this window width the panel hides (`@media (max-width:1100px)`).
const MEDIUM: f32 = 1100.;
const PANEL_W: f32 = 360.;
/// The width of the folded Done column.
const FOLDED_W: f32 = 56.;
/// The text size of the folded Done label.
const STRIP_TEXT: f32 = 13.;
const NO_PART: &str = "general";
const NEW_TASK_W: f32 = 420.;

// Surfaces of the board that no other screen shares.
fn step_off() -> u32 {
    crate::theme::pick(0xE6E9EE, tk().track_2)
}
const STEP_WORKING: u32 = 0x6B8AFF;
fn strip_bg() -> u32 {
    crate::theme::pick(0xECEEF2, tk().track)
}
fn strip_hover() -> u32 {
    crate::theme::pick(0xE3E6EB, tk().track_2)
}
fn working_bg() -> u32 {
    crate::theme::pick(0xEEF2FF, tk().work_bg)
}
fn working_text() -> u32 {
    crate::theme::pick(0x4B5BDB, tk().work_text)
}
fn field_border() -> u32 {
    tk().field_line
}
fn button_border() -> u32 {
    tk().edge
}
fn button_border_hover() -> u32 {
    tk().muted
}
fn gate_ok() -> u32 {
    tk().ok_text
}
fn gate_ok_bg() -> u32 {
    crate::theme::pick(0xF3FBF5, tk().ok_bg)
}
const GATE_BAD: u32 = 0xB91C1C;
fn gate_bad_bg() -> u32 {
    crate::theme::pick(0xFEF6F6, tk().q_bg)
}
fn session_done() -> u32 {
    tk().ok
}

/// The card that the pointer drags: its task and the column it came from.
#[derive(Debug, Clone)]
struct DragCard {
    id: String,
    from: TaskColumn,
    title: SharedString,
}

/// The picture that follows the pointer while a card drags.
struct DragGhost(SharedString);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(260.))
            .px(px(12.))
            .py(px(8.))
            .bg(rgb(tk().card))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(tk().primary))
            .shadow(ui::popover_shadow())
            .text_size(crate::ui::ts(crate::theme::T_BODY))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(tk().ink))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(self.0.clone())
    }
}

/// What a drop of a card on a column does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Drop {
    /// The card stays: it was dropped on its own column.
    Stay,
    Move,
    /// The daemon would refuse: the text tells why, and where the card may go.
    Refused(String),
}

fn check_drop(from: TaskColumn, to: TaskColumn) -> Drop {
    if from == to {
        Drop::Stay
    } else if from.can_move_to(to) {
        Drop::Move
    } else {
        Drop::Refused(from.refusal(to))
    }
}

/// What the open popover is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    NewTask,
    Handoff,
}

/// The fields of the New task popover.
#[derive(Default)]
struct NewTask {
    title: Option<Entity<InputState>>,
    prompt: Option<Entity<TextareaState>>,
    codebase: Option<String>,
    part: Option<String>,
    playbook: Option<Playbook>,
    error: Option<String>,
    busy: bool,
    _events: Vec<Subscription>,
}

pub struct TasksScreen {
    model: Entity<Model>,
    tasks: Option<Rc<Vec<Task>>>,
    /// The codebase chip that is on. `None`: all codebases.
    filter: Option<String>,
    /// The pick of the user. [`resolve_selection`] decides what shows.
    selected: Option<String>,
    done_open: bool,
    open: Overlay,
    /// An overlay closed by a click outside, and when: the click on its own button must not
    /// open it again.
    closed: Option<(Overlay, Instant)>,
    new_task: NewTask,
    /// The providers the daemon can run, with their models.
    providers: Vec<(String, Vec<String>)>,
    prefs: BTreeMap<String, ProviderPrefs>,
    /// A start, add or hand-off call is in flight.
    acting: bool,
    error: Option<String>,
    refreshing: bool,
    dirty: bool,
    /// Bumped by every local change: an older list answer is stale.
    generation: u64,
    /// The `(epoch, cursor)` of the war room that the last refresh answered.
    seen: Option<(u64, u64)>,
    _observe: Subscription,
}

impl EventEmitter<TasksEvent> for TasksScreen {}

impl TasksScreen {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |this, model, cx| {
            let key = model.read(cx).war.as_ref().map(|w| (w.epoch, w.cursor));
            if key.is_some() && key != this.seen {
                this.seen = key;
                this.refresh(cx);
            }
        });
        let settings = model
            .read(cx)
            .call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            if let Ok(view) = settings.await {
                let _ = this.update(cx, |this, cx| {
                    this.providers = view
                        .providers
                        .iter()
                        .filter(|p| p.status != ProviderStatus::Off)
                        .map(|p| (p.id.clone(), p.models.clone()))
                        .collect();
                    this.prefs = view.settings.providers.clone();
                    cx.notify();
                });
            }
        })
        .detach();
        let mut this = Self {
            model,
            tasks: None,
            filter: None,
            selected: None,
            done_open: false,
            open: Overlay::None,
            closed: None,
            new_task: NewTask::default(),
            providers: Vec::new(),
            prefs: BTreeMap::new(),
            acting: false,
            error: None,
            refreshing: false,
            dirty: false,
            generation: 0,
            seen: None,
            _observe: observe,
        };
        this.refresh(cx);
        this
    }

    /// Load the list again. One call at a time: deltas that come meanwhile ask for one more.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.dirty = true;
            return;
        }
        self.refreshing = true;
        let generation = self.generation;
        let call = self
            .model
            .read(cx)
            .call::<method::TasksList>(TaskListParams::default(), cx);
        cx.spawn(async move |this, cx| {
            let list = call.await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                let stale = generation != this.generation;
                match list {
                    Ok(list) if !stale => this.tasks = Some(Rc::new(list.tasks)),
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("aivory: tasks/list failed: {e:#}");
                        this.model.update(cx, |m, cx| {
                            m.notify_user(crate::notice::failure("tasks/list", &e), cx)
                        });
                    }
                }
                if stale || std::mem::take(&mut this.dirty) {
                    this.refresh(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn now(&self, cx: &gpui::App) -> UnixMs {
        self.model
            .read(cx)
            .war
            .as_ref()
            .map_or_else(|| chrono::Utc::now().timestamp_millis(), |w| w.now)
    }

    fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.as_ref()?.iter().find(|t| t.id == id)
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.error = None;
        cx.notify();
    }

    fn set_filter(&mut self, filter: Option<String>, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    fn toggle(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        if let Some((closed, at)) = self.closed.take()
            && closed == overlay
            && at.elapsed() < Duration::from_millis(400)
        {
            cx.notify();
            return;
        }
        self.open = if self.open == overlay {
            Overlay::None
        } else {
            overlay
        };
        cx.notify();
    }

    /// Paint `el` above everything; a click outside closes the open overlay.
    fn overlay(&self, el: Div, cx: &mut Context<Self>) -> AnyElement {
        deferred(
            el.occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    let open = std::mem::replace(&mut this.open, Overlay::None);
                    this.closed = Some((open, Instant::now()));
                    cx.notify();
                })),
        )
        .with_priority(1)
        .into_any_element()
    }

    // ---- Calls ----

    /// The provider that starts a task: the first one the daemon can run.
    fn default_provider(&self) -> String {
        self.providers
            .first()
            .map_or_else(|| "claude".to_string(), |(id, _)| id.clone())
    }

    fn composer_for(&self, provider: &str) -> ComposerPrefs {
        let prefs = self.prefs.get(provider);
        let model = prefs
            .map(|p| p.model.clone())
            .filter(|m| !m.is_empty())
            .or_else(|| {
                self.providers
                    .iter()
                    .find(|(id, _)| id == provider)
                    .and_then(|(_, models)| models.first().cloned())
            })
            .unwrap_or_default();
        ComposerPrefs {
            mode: prefs.map_or(Mode::Edit, |p| p.mode),
            provider: provider.to_string(),
            model,
            effort: prefs.map_or(Effort::High, |p| p.effort),
        }
    }

    fn session_params(&self, task: &Task, provider: &str) -> NewSessionParams {
        NewSessionParams {
            codebase: task.codebase.clone(),
            part: Some(task.part.clone()),
            title: Some(task.title.clone()),
            prompt: task.prompt.clone(),
            composer: self.composer_for(provider),
            task: Some(task.id.clone()),
        }
    }

    /// End of a start, add or hand-off call: show a failure, load the list again.
    fn acted(&mut self, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        self.acting = false;
        if let Err(e) = result {
            eprintln!("aivory: task action failed: {e:#}");
            self.error = Some(format!("{e:#}"));
            let text = format!("The task action failed. {}", crate::notice::reason(&e));
            self.model.update(cx, |m, cx| {
                m.notify_user(crate::notice::Draft::failure(text), cx)
            });
        }
        self.generation += 1;
        self.refresh(cx);
        cx.notify();
    }

    /// Start a planned task: a session that does the task, then the task moves to Working.
    fn start_now(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(task) = self.task(id) else { return };
        if self.acting {
            return;
        }
        let provider = self.default_provider();
        let params = self.session_params(task, &provider);
        let update = TaskUpdateParams {
            id: id.to_string(),
            column: Some(TaskColumn::Working),
            ..Default::default()
        };
        self.acting = true;
        self.error = None;
        let call = self.model.read(cx).call::<method::SessionNew>(params, cx);
        cx.spawn(async move |this, cx| {
            let result = async {
                call.await?;
                let next = this.update(cx, |this, cx| {
                    this.model.read(cx).call::<method::TasksUpdate>(update, cx)
                })?;
                next.await.map(|_| ())
            }
            .await;
            let _ = this.update(cx, |this, cx| this.acted(result, cx));
        })
        .detach();
        cx.notify();
    }

    /// Add one more session to a task.
    fn add_session(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(task) = self.task(id) else { return };
        if self.acting {
            return;
        }
        let provider = task
            .agents
            .first()
            .cloned()
            .unwrap_or_else(|| self.default_provider());
        let params = self.session_params(task, &provider);
        self.acting = true;
        self.error = None;
        let call = self.model.read(cx).call::<method::SessionNew>(params, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await.map(|_| ());
            let _ = this.update(cx, |this, cx| this.acted(result, cx));
        })
        .detach();
        cx.notify();
    }

    /// Continue the main session of a task with another provider.
    fn hand_off(&mut self, id: &str, provider: String, cx: &mut Context<Self>) {
        self.open = Overlay::None;
        let Some(session) = self
            .task(id)
            .and_then(|t| t.sessions.first())
            .map(|s| s.session.clone())
        else {
            return;
        };
        if self.acting {
            return;
        }
        self.acting = true;
        self.error = None;
        let call = self.model.read(cx).call::<method::SessionHandoff>(
            HandoffParams {
                id: session,
                provider,
                model: None,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await.map(|_| ());
            let _ = this.update(cx, |this, cx| this.acted(result, cx));
        })
        .detach();
        cx.notify();
    }

    fn run_primary(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(task) = self.task(id) else { return };
        let session = task.sessions.first().map(|s| s.session.clone());
        let sessions: Vec<String> = task.sessions.iter().map(|s| s.session.clone()).collect();
        match Primary::of(task.column) {
            Primary::Start => self.start_now(id, cx),
            Primary::Answer => cx.emit(TasksEvent::OpenInbox(sessions)),
            Primary::Review | Primary::Merge => {
                if let Some(session) = session {
                    cx.emit(TasksEvent::OpenDiff(session));
                }
            }
            Primary::Session => {
                if let Some(session) = session {
                    cx.emit(TasksEvent::OpenSession(session));
                }
            }
        }
    }

    /// A card dropped on a column: `tasks/update` moves it. A move that the rules refuse shows
    /// the refusal, and a refusal of the daemon shows its text.
    fn move_task(&mut self, card: &DragCard, to: TaskColumn, cx: &mut Context<Self>) {
        let from = self.task(&card.id).map_or(card.from, |t| t.column);
        match check_drop(from, to) {
            Drop::Stay => return,
            Drop::Refused(text) => {
                let text = format!(
                    "The task cannot move there. {}",
                    crate::notice::sentence(&text)
                );
                self.model.update(cx, |m, cx| {
                    m.notify_user(crate::notice::Draft::failure(text), cx)
                });
                return;
            }
            Drop::Move => {}
        }
        self.selected = Some(card.id.clone());
        let call = self.model.read(cx).call_notify::<method::TasksUpdate>(
            TaskUpdateParams {
                id: card.id.clone(),
                column: Some(to),
                ..Default::default()
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            call.await;
            let _ = this.update(cx, |this, cx| {
                this.generation += 1;
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    // ---- New task ----

    /// The fields of the popover, made on first use.
    fn ensure_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_task.title.is_some() {
            return;
        }
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("A short name"));
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What should the agents do?")
                .submit_on_enter(true)
                .auto_grow(3, 8)
        });
        let events = vec![
            cx.subscribe_in(&title, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event
                    && let Some(prompt) = this.new_task.prompt.clone()
                {
                    prompt.update(cx, |s, cx| s.focus(window, cx));
                }
            }),
            cx.subscribe_in(
                &prompt,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.create_task(StartWhen::Queue, window, cx);
                    }
                },
            ),
        ];
        self.new_task.title = Some(title);
        self.new_task.prompt = Some(prompt);
        self.new_task._events = events;
    }

    fn open_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle(Overlay::NewTask, cx);
        if self.open == Overlay::NewTask {
            self.ensure_new_task(window, cx);
            if let Some(title) = self.new_task.title.clone() {
                title.update(cx, |s, cx| s.focus(window, cx));
            }
        }
    }

    /// Queue the task for the autopilot, or start it now.
    fn create_task(&mut self, when: StartWhen, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(title_in), Some(prompt_in)) =
            (self.new_task.title.clone(), self.new_task.prompt.clone())
        else {
            return;
        };
        if self.new_task.busy {
            return;
        }
        let mut title = title_in.read(cx).value().trim().to_string();
        let mut prompt = prompt_in.read(cx).value().trim().to_string();
        if title.is_empty() {
            title = title_from_prompt(&prompt);
        }
        if title.is_empty() {
            self.new_task.error = Some("Add a title or a prompt.".into());
            cx.notify();
            return;
        }
        if prompt.is_empty() {
            prompt.clone_from(&title);
        }
        let Some(codebase) = self.new_task.codebase.clone() else {
            self.new_task.error = Some("Pick a codebase.".into());
            cx.notify();
            return;
        };
        let part = self
            .new_task
            .part
            .clone()
            .unwrap_or_else(|| NO_PART.to_string());
        let params = TaskCreateParams {
            title,
            codebase: codebase.clone(),
            part,
            playbook: self.new_task.playbook.unwrap_or(Playbook::Feature),
            prompt,
            when,
            provider: None,
            mode: None,
            priority: None,
        };
        self.new_task.busy = true;
        self.new_task.error = None;
        let call = self.model.read(cx).call::<method::TasksCreate>(params, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.new_task.busy = false;
                match result {
                    Ok(task) => {
                        title_in.update(cx, |s, cx| s.clean(window, cx));
                        prompt_in.update(cx, |s, cx| s.clean(window, cx));
                        this.open = Overlay::None;
                        if this.filter.as_ref().is_some_and(|f| *f != codebase) {
                            this.filter = None;
                        }
                        this.selected = Some(task.id);
                        this.generation += 1;
                        this.refresh(cx);
                    }
                    Err(e) => this.new_task.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- Views ----

    fn header(
        &mut self,
        tasks: &[Task],
        chips: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut chip_row = div().flex().flex_wrap().gap(px(6.));
        let all = std::iter::once((SharedString::from("All codebases"), None)).chain(
            chips
                .iter()
                .map(|c| (SharedString::from(c.clone()), Some(c.clone()))),
        );
        for (ix, (label, value)) in all.enumerate() {
            let on = self.filter == value;
            chip_row = chip_row.child(
                div()
                    .id(("task-chip", ix))
                    .h(px(28.))
                    .px(px(11.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(if on { rgb(tk().ink) } else { rgba(0x0D0D0E1F) })
                    .when(on, |d| d.bg(rgb(tk().ink)))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(if on { tk().card } else { tk().ink_2 }))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(!on, |d| d.hover(|s| s.border_color(rgba(0x0D0D0E4D))))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_filter(value.clone(), cx)))
                    .child(label),
            );
        }
        let open = self.open == Overlay::NewTask;
        let mut new = div().relative().flex_none().child(
            ui::dark_button("new-task", "New task")
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .on_click(cx.listener(|this, _, window, cx| this.open_new_task(window, cx))),
        );
        if open {
            let popover = self.new_task_popover(window, cx);
            new =
                new.child(self.overlay(div().absolute().top(px(42.)).right_0().child(popover), cx));
        }
        div()
            .flex_none()
            .min_h(HEADER_H)
            .py(px(12.))
            .px(px(32.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .font_family(crate::theme::tk().display)
                    .text_size(crate::ui::ts(crate::theme::T_DISPLAY))
                    .font_weight(FontWeight::NORMAL)
                    .child("Tasks"),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child(header_text(tasks)),
            )
            .child(div().flex_1())
            .child(chip_row)
            .child(new)
    }

    fn new_task_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.ensure_new_task(window, cx);
        let codebases: Vec<(String, Vec<String>)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                w.codebases
                    .iter()
                    .map(|c| (c.id.clone(), c.part_names().map(str::to_string).collect()))
                    .collect()
            })
            .unwrap_or_default();
        let nt = &mut self.new_task;
        if nt.codebase.is_none() {
            nt.codebase = self
                .filter
                .clone()
                .filter(|f| codebases.iter().any(|(id, _)| id == f))
                .or_else(|| codebases.first().map(|(id, _)| id.clone()));
        }
        let parts: Vec<String> = codebases
            .iter()
            .find(|(id, _)| Some(id) == nt.codebase.as_ref())
            .map(|(_, parts)| parts.clone())
            .unwrap_or_default();
        if nt.part.as_ref().is_none_or(|p| !parts.contains(p)) {
            nt.part = parts.first().cloned();
        }
        let playbook = nt.playbook.unwrap_or(Playbook::Feature);
        let nt = &self.new_task;

        let mut cb_chips = div().flex().flex_wrap().gap(px(6.));
        for (ix, (id, _)) in codebases.iter().enumerate() {
            let value = id.clone();
            cb_chips = cb_chips.child(
                option_chip(("nt-cb", ix), id.clone(), nt.codebase.as_ref() == Some(id)).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.new_task.codebase = Some(value.clone());
                        this.new_task.part = None;
                        cx.notify();
                    }),
                ),
            );
        }
        let mut part_chips = div().flex().flex_wrap().gap(px(6.));
        for (ix, part) in parts.iter().enumerate() {
            let value = part.clone();
            part_chips = part_chips.child(
                option_chip(
                    ("nt-part", ix),
                    part.clone(),
                    nt.part.as_ref() == Some(part),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.new_task.part = Some(value.clone());
                    cx.notify();
                })),
            );
        }
        let mut seg = ui::seg_track();
        for (ix, (value, label)) in PLAYBOOKS.into_iter().enumerate() {
            seg = seg.child(
                ui::seg_button(("nt-pb", ix), playbook == value, label, false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.new_task.playbook = Some(value);
                        cx.notify();
                    }),
                ),
            );
        }
        let field = |label: &'static str, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().text_color(rgb(tk().ink_2)).child(label))
                .child(control)
        };
        let boxed = |h: Option<f32>| {
            div()
                .px(px(10.))
                .py(px(8.))
                .rounded(tk().r_button)
                .border_1()
                .border_color(rgb(field_border()))
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .when_some(h, |d, h| d.min_h(px(h)))
        };
        let title = nt.title.clone().expect("title exists");
        let prompt = nt.prompt.clone().expect("prompt exists");
        let busy = nt.busy;
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(32.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(tk().r_button)
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(busy, |d| d.opacity(0.6))
                .when(primary, |d| {
                    d.bg(rgb(tk().primary))
                        .text_color(rgb(tk().card))
                        .hover(|s| s.bg(rgb(tk().primary_hover)))
                })
                .when(!primary, |d| {
                    d.border_1()
                        .border_color(rgb(button_border()))
                        .hover(|s| s.border_color(rgb(button_border_hover())))
                })
                .child(label)
        };
        ui::popover(NEW_TASK_W)
            .child(div().font_weight(FontWeight::BOLD).child("New task"))
            .when(codebases.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().ink_3))
                        .child("No codebase yet. Start one agent in a repository, or add a repo."),
                )
            })
            .child(field(
                "Title",
                boxed(None)
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .child(title)
                    .into_any_element(),
            ))
            .child(field("Codebase", cb_chips.into_any_element()))
            .when(!parts.is_empty(), |d| {
                d.child(field("Part", part_chips.into_any_element()))
            })
            .child(field(
                "Playbook",
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.))
                    .child(seg)
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(playbook.steps().join(" → ")),
                    )
                    .into_any_element(),
            ))
            .child(field(
                "Prompt",
                boxed(Some(64.))
                    .line_height(relative(1.45))
                    .child(prompt)
                    .into_any_element(),
            ))
            .when_some(nt.error.clone(), |d, e| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().e_text))
                        .child(e),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(button("nt-queue", "Queue it", false).on_click(cx.listener(
                        |this, _, window, cx| this.create_task(StartWhen::Queue, window, cx),
                    )))
                    .child(button("nt-start", "Start now", true).on_click(cx.listener(
                        |this, _, window, cx| this.create_task(StartWhen::Now, window, cx),
                    ))),
            )
    }

    /// One column: its head and its cards. The folded Done column is a strip.
    fn column(
        &self,
        column: TaskColumn,
        count: usize,
        tasks: &[&Task],
        selected: Option<&str>,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = column == TaskColumn::Done && !self.done_open && !narrow;
        if folded {
            return self.done_strip(count, cx);
        }
        let fold = column == TaskColumn::Done && !narrow;
        let mut col = div()
            .id(SharedString::from(format!("column-{}", column.label())))
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h(px(220.))
            .rounded(px(12.))
            .when(narrow, |d| d.w_full().flex_none())
            .when(!narrow, |d| d.flex_1().min_w(px(0.)))
            .drag_over::<DragCard>(move |s, card, _, _| {
                if card.from != column && card.from.can_move_to(column) {
                    s.bg(rgb(strip_bg()))
                } else {
                    s
                }
            })
            .on_drop(
                cx.listener(move |this, card: &DragCard, _, cx| this.move_task(card, column, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(2.))
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(column.label()),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(count.to_string()),
                    )
                    .child(div().flex_1().h(px(1.)).bg(rgba(tk().line)))
                    .when(fold, |d| {
                        d.child(
                            div()
                                .id("fold-done")
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(strip_bg())))
                                .tooltip(ui::tip("Fold the Done column"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.done_open = false;
                                    cx.notify();
                                }))
                                .child(ui::icon("icons/twist-right.svg", 12., rgb(tk().ink_3))),
                        )
                    }),
            );
        for task in tasks {
            col = col.child(self.card(task, selected == Some(task.id.as_str()), cx));
        }
        col.into_any_element()
    }

    /// The folded Done column (`.colx`): `Done · 5` turned 90 degrees clockwise, top to bottom.
    fn done_strip(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let label = format!("Done · {count}");
        let (w, h) = vtext::measure(&label, STRIP_TEXT);
        div()
            .id("done-strip")
            .flex_none()
            .w(px(FOLDED_W))
            .min_h(px(420.))
            .pt(px(16.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .rounded(px(12.))
            .bg(rgb(strip_bg()))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(strip_hover())))
            .drag_over::<DragCard>(|s, card, _, _| {
                if card.from.can_move_to(TaskColumn::Done) {
                    s.bg(rgb(strip_hover()))
                } else {
                    s
                }
            })
            .on_drop(cx.listener(|this, card: &DragCard, _, cx| {
                this.move_task(card, TaskColumn::Done, cx)
            }))
            .tooltip(ui::tip(
                "Show the Done column. Drop a card here to finish it.",
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.done_open = true;
                cx.notify();
            }))
            .child(
                svg()
                    .path(vtext::asset_path(&label, STRIP_TEXT as u32))
                    .flex_none()
                    .w(px(w))
                    .h(px(h))
                    .text_color(rgb(tk().ink_2)),
            )
            .into_any_element()
    }

    fn card(&self, task: &Task, on: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = task.id.clone();
        let hot = task.waiting_for.is_some();
        let mut bar = div().flex().gap(px(4.));
        for seg in segments(task) {
            bar = bar.child(
                div()
                    .flex_1()
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(rgb(seg.color())),
            );
        }
        // Overlapping circles (`.ags`): each one 12 px after the one before.
        let shown_agents = task.agents.len().min(MAX_CIRCLES);
        let mut circles = div()
            .relative()
            .flex_none()
            .h(px(CIRCLE))
            .w(px(circles_width(shown_agents)));
        for (ix, provider) in task.agents.iter().take(MAX_CIRCLES).enumerate() {
            circles = circles.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(ix as f32 * CIRCLE_STEP))
                    .size(px(CIRCLE))
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(tk().card))
                    .bg(rgb(provider_color(provider))),
            );
        }
        div()
            .id(SharedString::from(format!("task-{}", task.id)))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .bg(rgb(tk().card))
            .rounded(px(12.))
            .border_1()
            .border_color(if on {
                rgb(tk().primary)
            } else {
                rgba(0x00000000)
            })
            .shadow(card_shadows(on, hot))
            .cursor_pointer()
            .when(!on, |d| d.hover(|s| s.border_color(rgba(0x0D0D0E24))))
            .on_drag(
                DragCard {
                    id: task.id.clone(),
                    from: task.column,
                    title: task.title.clone().into(),
                },
                |card, _, _, cx| cx.new(|_| DragGhost(card.title.clone())),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(relative(1.35))
                    .child(task.title.clone()),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("{} / {}", task.codebase, task.part)),
            )
            .when_some(task.waiting_for, |d, ask| {
                d.child(
                    div()
                        .self_start()
                        .px(px(8.))
                        .py(px(1.))
                        .rounded_full()
                        .bg(rgb(tk().q_bg))
                        .text_color(rgb(tk().q_text))
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .font_weight(FontWeight::MEDIUM)
                        .child(wait_text(ask)),
                )
            })
            .child(bar)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_2))
                    .child(circles)
                    .child(agents_text(task.agents.len())),
            )
            .into_any_element()
    }

    fn board(
        &self,
        shown: &[&Task],
        selected: Option<&str>,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut board = div()
            .id("task-board")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .when(narrow, |d| d.flex_col().p(px(16.)))
            .when(!narrow, |d| {
                d.pl(px(32.)).pr(px(24.)).pt(px(20.)).pb(px(24.))
            })
            .gap(px(14.))
            .items_start();
        let counts = column_counts(shown);
        for (column, count) in TaskColumn::ALL.into_iter().zip(counts) {
            let tasks: Vec<&Task> = shown
                .iter()
                .copied()
                .filter(|t| t.column == column)
                .collect();
            board = board.child(self.column(column, count, &tasks, selected, narrow, cx));
        }
        board.into_any_element()
    }

    fn panel(&mut self, task: &Task, now: UnixMs, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.read(cx);
        let started = started_text(
            task.started,
            now,
            task.started.is_some_and(|at| same_day(at, now, model.demo)),
            &task.started.map(|at| model.clock(at)).unwrap_or_default(),
        );
        let started = match task.started {
            Some(_) => format!("started {started}"),
            None => started,
        };
        let primary = Primary::of(task.column);
        let has_session = !task.sessions.is_empty();
        let primary_enabled = match primary {
            Primary::Start | Primary::Answer => true,
            _ => has_session,
        };
        let id = task.id.clone();

        let mut stepper = div().flex().flex_col();
        let last = task.steps.len().saturating_sub(1);
        for (ix, step) in task.steps.iter().enumerate() {
            let (bg, fg, ring) = match step.state {
                StepState::Done => (tk().ink, tk().card, None),
                StepState::Working => (working_bg(), working_text(), Some(STEP_WORKING)),
                StepState::Waiting => (tk().q_bg, tk().q_text, Some(tk().q)),
                StepState::NotStarted => (tk().track, tk().ink_3, None),
            };
            let who = match (&step.actor, step.state) {
                (_, StepState::NotStarted) | (None, _) => "not started".to_string(),
                (Some(actor), _) => actor.clone(),
            };
            let time = match step.state {
                StepState::Done => step
                    .duration_ms
                    .map(|ms| duration_text(ms as i64 / MINUTE_MS))
                    .unwrap_or_default(),
                StepState::Working => "running".to_string(),
                StepState::Waiting => "waits for you".to_string(),
                StepState::NotStarted => String::new(),
            };
            stepper = stepper.child(
                div()
                    .relative()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .when(ix < last, |d| d.pb(px(14.)))
                    .when(ix < last, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(10.))
                                .top(px(20.))
                                .bottom_0()
                                .w(px(2.))
                                .bg(rgb(if step.state == StepState::Done {
                                    tk().ink
                                } else {
                                    step_off()
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(rgb(bg))
                            .text_color(rgb(fg))
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when_some(ring, |d, c| d.border_2().border_color(rgb(c)))
                            .child(if step.state == StepState::Done {
                                "✓".to_string()
                            } else {
                                (ix + 1).to_string()
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .pt(px(2.))
                            .child(
                                div()
                                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(step.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                    .text_color(rgb(tk().ink_3))
                                    .child(who),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .pt(px(4.))
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(time),
                    ),
            );
        }

        let mut sessions = div().flex().flex_col().gap(px(10.));
        if task.sessions.is_empty() {
            sessions = sessions.child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().muted))
                    .child("no session yet"),
            );
        }
        for (ix, s) in task.sessions.iter().enumerate() {
            let session = s.session.clone();
            sessions = sessions.child(
                div()
                    .id(("task-session", ix))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .border_1()
                    .border_color(rgba(tk().line))
                    .rounded(px(10.))
                    .cursor_pointer()
                    .hover(|st| st.border_color(rgba(0x0D0D0E3D)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(TasksEvent::OpenSession(session.clone()))
                    }))
                    .child(session_dot(s.state))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(s.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .child(s.role.clone()),
                    ),
            );
        }

        let mut gates = div().flex().flex_wrap().gap(px(8.));
        for gate in &task.gates {
            let (mark, fg, bg) = match gate.state {
                GateState::Ok => ("✓", gate_ok(), Some(gate_ok_bg())),
                GateState::Pending => ("○", tk().ink_3, None),
                GateState::Failed => ("✕", GATE_BAD, Some(gate_bad_bg())),
            };
            gates = gates.child(
                div()
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(rgba(0x0D0D0E1A))
                    .when_some(bg, |d, bg| d.bg(rgb(bg)))
                    .text_color(rgb(fg))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .whitespace_nowrap()
                    .child(format!("{mark} {}", gate.label)),
            );
        }

        let busy = self.acting;
        let action = |id: &'static str, label: &'static str, primary: bool, enabled: bool| {
            div()
                .id(id)
                .h(px(32.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .whitespace_nowrap()
                .when(!enabled || busy, |d| d.opacity(0.5))
                .when(enabled, |d| d.cursor_pointer())
                .when(primary, |d| {
                    d.bg(rgb(tk().primary))
                        .border_color(rgb(tk().primary))
                        .text_color(rgb(tk().card))
                        .font_weight(FontWeight::MEDIUM)
                        .when(enabled, |d| d.hover(|s| s.bg(rgb(tk().primary_hover))))
                })
                .when(!primary, |d| {
                    d.bg(rgb(tk().card))
                        .border_color(rgb(button_border()))
                        .when(enabled, |d| {
                            d.hover(|s| s.border_color(rgb(button_border_hover())))
                        })
                })
                .child(label)
        };
        let primary_id = id.clone();
        let add_id = id.clone();
        let hand_id = id.clone();
        let hand_open = self.open == Overlay::Handoff;
        let mut hand = div().relative().child(
            action("task-handoff", "Hand off", false, has_session).on_click(cx.listener(
                move |this, _, _, cx| {
                    if has_session {
                        this.toggle(Overlay::Handoff, cx);
                    }
                },
            )),
        );
        if hand_open {
            let current = task.agents.first().cloned().unwrap_or_default();
            let mut menu = ui::menu(150.);
            for (ix, provider) in self.handoff_providers().iter().enumerate() {
                let value = provider.clone();
                let task_id = hand_id.clone();
                menu = menu.child(ui::menu_item(
                    ("handoff", ix),
                    *provider == current,
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_none()
                                .size(px(7.))
                                .rounded_full()
                                .bg(rgb(provider_color(provider))),
                        )
                        .child(provider.clone())
                        .into_any_element(),
                    None,
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.hand_off(&task_id, value.clone(), cx)
                    })),
                ));
            }
            hand =
                hand.child(self.overlay(div().absolute().bottom(px(36.)).left_0().child(menu), cx));
        }

        let actions = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(
                action("task-primary", primary.label(), true, primary_enabled).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if primary_enabled {
                            this.run_primary(&primary_id, cx);
                        }
                    }),
                ),
            )
            .child(
                action("task-add-session", "Add a session", false, true)
                    .on_click(cx.listener(move |this, _, _, cx| this.add_session(&add_id, cx))),
            )
            .child(hand);

        div()
            .id("task-panel")
            .flex_none()
            .w(px(PANEL_W))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(20.))
            .pt(px(22.))
            .pb(px(28.))
            .px(px(24.))
            .bg(rgb(tk().card))
            .border_l_1()
            .border_color(rgba(tk().line))
            .line_height(relative(1.3))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(kicker(format!(
                        "{} · {} playbook",
                        task.column.label(),
                        playbook_name(task.playbook)
                    )))
                    .child(
                        div()
                            .mt(px(2.))
                            .text_size(crate::ui::ts(crate::theme::T_HEADING))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(relative(1.3))
                            .child(task.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .text_color(rgb(tk().ink_3))
                            .child(format!(
                                "{} / {} · {started} · ${:.2}",
                                task.codebase, task.part, task.cost
                            )),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(14.))
                    .py(px(12.))
                    .rounded(px(10.))
                    .bg(rgb(tk().page))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .line_height(relative(1.55))
                    .child(task.prompt.clone()),
            )
            .child(section("Playbook", stepper))
            .child(section("Sessions", sessions))
            .child(section("Gates", gates))
            .when_some(self.error.clone(), |d, e| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().e_text))
                        .child(e),
                )
            })
            .child(actions)
            .into_any_element()
    }

    /// The providers of the Hand off menu: the ones the daemon can run, else the design list.
    fn handoff_providers(&self) -> Vec<String> {
        if self.providers.is_empty() {
            ["claude", "codex", "gemini", "opencode", "goose"]
                .iter()
                .map(|p| p.to_string())
                .collect()
        } else {
            self.providers.iter().map(|(id, _)| id.clone()).collect()
        }
    }
}

impl Render for TasksScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now(cx);
        let width = window.viewport_size().width;
        let Some(tasks) = self.tasks.clone() else {
            let header = self.header(&[], &[], window, cx);
            return div().size_full().flex().flex_col().child(header).child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().muted))
                    .child("Waits for aivoryd"),
            );
        };
        let order: Vec<String> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| w.codebases.iter().map(|c| c.id.clone()).collect())
            .unwrap_or_default();
        let chips = chip_labels(&tasks, &order);
        if self.filter.as_ref().is_some_and(|f| !chips.contains(f)) {
            self.filter = None;
        }
        let shown = visible(&tasks, self.filter.as_deref());
        let selected = resolve_selection(&tasks, self.filter.as_deref(), self.selected.as_deref());
        let narrow = width < NARROW;
        let board = self.board(&shown, selected.as_deref(), narrow, cx);
        let panel = match selected
            .as_deref()
            .and_then(|id| tasks.iter().find(|t| t.id == id))
        {
            Some(task) if width >= px(MEDIUM) => Some(self.panel(task, now, cx)),
            _ => None,
        };
        let header = self.header(&tasks, &chips, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .line_height(relative(1.3))
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(board)
                    .when_some(panel, |d, panel| d.child(panel)),
            )
    }
}

// ---- Small views ----

/// `.k`: the mono kicker of a panel section.
fn kicker(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(tk().ink_3))
        .child(text.into().to_uppercase())
}

fn section(label: &'static str, body: Div) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(kicker(label))
        .child(body)
}

/// The dot of a session row: the state, and done is green.
fn session_dot(state: SessionState) -> Div {
    match state {
        SessionState::Done => ui::dot(rgb(session_done())),
        other => ui::state_dot(other, 8.),
    }
}

/// A selectable chip of the New task popover.
fn option_chip(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    on: bool,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(if on {
            rgb(tk().primary)
        } else {
            rgba(tk().line_2)
        })
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(tk().muted)))
        .child(label.into())
}

/// The shadows of a card: a selected card shows a ring, a waiting card an orange top line.
fn card_shadows(on: bool, hot: bool) -> Vec<BoxShadow> {
    let shadow = |color: u32, y: f32, blur: f32, spread: f32, inset: bool| BoxShadow {
        color: ui::hsla(color),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset,
    };
    let drops = [
        shadow(0x0D0D0E0A, 1., 2., 0., false),
        shadow(0x0D0D0E0D, 8., 24., 0., false),
    ];
    if hot {
        let mut v = vec![shadow(Q_RGBA, 2., 0., 0., true)];
        v.extend(drops);
        v
    } else if on {
        vec![shadow(0x3574F01F, 0., 0., 3., false)]
    } else {
        drops.to_vec()
    }
}

const Q_RGBA: u32 = 0xF97316FF;

/// The provider circles of a card: 18 px wide, 12 px apart, at most eight.
const CIRCLE: f32 = 18.;
const CIRCLE_STEP: f32 = 12.;
const MAX_CIRCLES: usize = 8;

fn circles_width(count: usize) -> f32 {
    match count {
        0 => 0.,
        n => CIRCLE + (n - 1) as f32 * CIRCLE_STEP,
    }
}

const PLAYBOOKS: [(Playbook, &str); 3] = [
    (Playbook::Feature, "Feature"),
    (Playbook::Fix, "Fix"),
    (Playbook::Port, "Port"),
];

// ---- Pure parts ----

/// One of the five segments of the playbook bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    Done,
    Working,
    Waiting,
    Off,
}

impl Segment {
    fn color(self) -> u32 {
        match self {
            Self::Done => tk().ink,
            Self::Working => STEP_WORKING,
            Self::Waiting => tk().q,
            Self::Off => step_off(),
        }
    }
}

/// The playbook bar of a card (`stepStates()` of the design): the steps before the current one
/// are done; the current one works, waits for the user, or is done when the task is done.
fn segments(task: &Task) -> [Segment; 5] {
    let mut out = [Segment::Off; 5];
    for (i, seg) in out.iter_mut().enumerate() {
        let at = usize::from(task.step);
        *seg = if i < at {
            Segment::Done
        } else if i > at {
            Segment::Off
        } else if task.column == TaskColumn::Done {
            Segment::Done
        } else if task.waiting_for.is_some() {
            Segment::Waiting
        } else if task.column == TaskColumn::Planned {
            Segment::Off
        } else {
            Segment::Working
        };
    }
    out
}

/// The tasks of the codebase filter (`None`: all).
fn visible<'a>(tasks: &'a [Task], filter: Option<&str>) -> Vec<&'a Task> {
    tasks
        .iter()
        .filter(|t| filter.is_none_or(|f| t.codebase == f))
        .collect()
}

/// The task count of each column, in board order.
fn column_counts(shown: &[&Task]) -> [usize; 5] {
    TaskColumn::ALL.map(|c| shown.iter().filter(|t| t.column == c).count())
}

/// `14 tasks · 7 active · 3 queued for autopilot`. Always over all tasks.
fn header_text(tasks: &[Task]) -> String {
    let active = tasks
        .iter()
        .filter(|t| matches!(t.column, TaskColumn::Working | TaskColumn::Waiting))
        .count();
    let queued = tasks
        .iter()
        .filter(|t| t.column == TaskColumn::Planned)
        .count();
    format!(
        "{} tasks · {active} active · {queued} queued for autopilot",
        tasks.len()
    )
}

/// The codebases that have a task: in war-room order, the others after them by name.
fn chip_labels(tasks: &[Task], order: &[String]) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for id in order {
        if tasks.iter().any(|t| &t.codebase == id) && !labels.contains(id) {
            labels.push(id.clone());
        }
    }
    let mut rest: Vec<String> = tasks
        .iter()
        .map(|t| t.codebase.clone())
        .filter(|c| !labels.contains(c))
        .collect();
    rest.sort();
    rest.dedup();
    labels.extend(rest);
    labels
}

/// The pick of the user if the task exists, else the first working task, else the first task.
fn resolve_selection(tasks: &[Task], filter: Option<&str>, picked: Option<&str>) -> Option<String> {
    if let Some(id) = picked
        && tasks.iter().any(|t| t.id == id)
    {
        return Some(id.to_string());
    }
    let shown = visible(tasks, filter);
    shown
        .iter()
        .copied()
        .find(|t| t.column == TaskColumn::Working)
        .or_else(|| shown.first().copied())
        .or_else(|| tasks.first())
        .map(|t| t.id.clone())
}

/// `3 agents`, `1 agent`, `not started`.
fn agents_text(count: usize) -> String {
    match count {
        0 => "not started".to_string(),
        1 => "1 agent".to_string(),
        n => format!("{n} agents"),
    }
}

/// The orange wait reason of a card.
fn wait_text(ask: AskKind) -> &'static str {
    ask.tag()
}

fn playbook_name(playbook: Playbook) -> &'static str {
    match playbook {
        Playbook::Feature => "feature",
        Playbook::Fix => "fix",
        Playbook::Port => "port",
    }
}

/// `2 min`, `1 h`, `2 h 5 min`.
fn duration_text(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// `queued`, `23 min ago`, `2 h ago`, `today 11:02`, `3 d ago`. `clock` is the time of `started`.
fn started_text(started: Option<UnixMs>, now: UnixMs, today: bool, clock: &str) -> String {
    let Some(at) = started else {
        return "queued".to_string();
    };
    let minutes = (now - at).max(0) / MINUTE_MS;
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 180 {
        format!("{} ago", duration_text(minutes))
    } else if today {
        format!("today {clock}")
    } else {
        format!("{} d ago", (minutes / (24 * 60)).max(1))
    }
}

/// Both times fall on the same calendar day (UTC in demo mode, local time otherwise).
fn same_day(a: UnixMs, b: UnixMs, utc: bool) -> bool {
    let day = |at: UnixMs| {
        chrono::DateTime::from_timestamp_millis(at).map(|t| {
            if utc {
                t.date_naive()
            } else {
                t.with_timezone(&chrono::Local).date_naive()
            }
        })
    };
    day(a).is_some() && day(a) == day(b)
}

/// The first line of a prompt as a title, cut at 60 characters.
fn title_from_prompt(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default().trim();
    line.chars().take(60).collect()
}

/// The button that fits the column of a task (`primary` of the design).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Primary {
    Start,
    Answer,
    Review,
    Merge,
    Session,
}

impl Primary {
    fn of(column: TaskColumn) -> Self {
        match column {
            TaskColumn::Planned => Self::Start,
            TaskColumn::Waiting => Self::Answer,
            TaskColumn::Review => Self::Review,
            TaskColumn::Done => Self::Merge,
            TaskColumn::Working => Self::Session,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Start => "Start now",
            Self::Answer => "Answer in the inbox",
            Self::Review => "Review the diff",
            Self::Merge => "Open the merge",
            Self::Session => "Open the session",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    fn demo_tasks() -> Vec<Task> {
        Demo::new().tasks
    }

    fn task<'a>(tasks: &'a [Task], id: &str) -> &'a Task {
        tasks.iter().find(|t| t.id == id).expect("demo task")
    }

    #[test]
    fn columns_count_as_the_artboard() {
        let tasks = demo_tasks();
        let all = visible(&tasks, None);
        assert_eq!(column_counts(&all), [3, 4, 3, 2, 2]);
    }

    #[test]
    fn the_codebase_filter_keeps_that_codebase() {
        let tasks = demo_tasks();
        let code = visible(&tasks, Some("aivory-code"));
        assert!(code.iter().all(|t| t.codebase == "aivory-code"));
        assert_eq!(column_counts(&code), [1, 1, 1, 1, 1]);
        assert!(visible(&tasks, Some("nobody")).is_empty());
    }

    #[test]
    fn header_counts_all_tasks_whatever_the_filter() {
        let tasks = demo_tasks();
        assert_eq!(
            header_text(&tasks),
            "14 tasks · 7 active · 3 queued for autopilot"
        );
        assert_eq!(
            header_text(&[]),
            "0 tasks · 0 active · 0 queued for autopilot"
        );
    }

    #[test]
    fn playbook_bar_follows_step_and_column() {
        use Segment::{Done, Off, Waiting, Working};
        let tasks = demo_tasks();
        assert_eq!(segments(task(&tasks, "t1")), [Off; 5]);
        assert_eq!(
            segments(task(&tasks, "t4")),
            [Done, Done, Working, Off, Off]
        );
        assert_eq!(segments(task(&tasks, "t8")), [Done, Waiting, Off, Off, Off]);
        assert_eq!(
            segments(task(&tasks, "t11")),
            [Done, Done, Done, Working, Off]
        );
        assert_eq!(segments(task(&tasks, "t13")), [Done; 5]);
    }

    #[test]
    fn chips_follow_the_war_room_order() {
        let tasks = demo_tasks();
        let order: Vec<String> = ["kronk", "aivory-code", "mobile"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let chips = chip_labels(&tasks, &order);
        assert_eq!(&chips[..2], ["kronk", "aivory-code"]);
        assert!(!chips.contains(&"mobile".to_string()));
        assert!(chips.contains(&"infra".to_string()));
        assert_eq!(chips.len(), 7);
    }

    #[test]
    fn selection_keeps_the_pick_else_the_first_working_task() {
        let tasks = demo_tasks();
        assert_eq!(
            resolve_selection(&tasks, None, Some("t9")).as_deref(),
            Some("t9")
        );
        assert_eq!(
            resolve_selection(&tasks, None, Some("gone")).as_deref(),
            Some("t4")
        );
        assert_eq!(
            resolve_selection(&tasks, Some("aivory-guard"), None).as_deref(),
            Some("t5")
        );
        assert_eq!(
            resolve_selection(&tasks, Some("infra"), None).as_deref(),
            Some("t14")
        );
        assert_eq!(resolve_selection(&[], None, None), None);
    }

    #[test]
    fn a_drop_follows_the_rules_of_the_daemon() {
        use TaskColumn::{Done, Planned, Review, Waiting, Working};
        assert_eq!(check_drop(Working, Working), Drop::Stay);
        assert_eq!(check_drop(Planned, Working), Drop::Move);
        assert_eq!(check_drop(Review, Planned), Drop::Move);
        // Nothing goes to Waiting: the daemon sets it when a session asks.
        for from in [Planned, Working, Review, Done] {
            assert!(matches!(check_drop(from, Waiting), Drop::Refused(_)));
        }
        // A task that never ran does not go to Review.
        let Drop::Refused(text) = check_drop(Planned, Review) else {
            panic!("refused");
        };
        assert!(text.contains("Planned") && text.contains("Working, Done"));
    }

    #[test]
    fn every_move_that_the_check_allows_is_in_the_table() {
        for from in TaskColumn::ALL {
            for to in TaskColumn::ALL {
                let allowed = matches!(check_drop(from, to), Drop::Stay | Drop::Move);
                assert_eq!(allowed, from == to || from.moves_to().contains(&to));
            }
        }
    }

    #[test]
    fn card_texts() {
        assert_eq!(agents_text(0), "not started");
        assert_eq!(agents_text(1), "1 agent");
        assert_eq!(agents_text(4), "4 agents");
        assert_eq!(wait_text(AskKind::Permission), "needs permission");
        assert_eq!(wait_text(AskKind::Plan), "plan to approve");
        assert_eq!(Primary::of(TaskColumn::Planned).label(), "Start now");
        assert_eq!(
            Primary::of(TaskColumn::Waiting).label(),
            "Answer in the inbox"
        );
        assert_eq!(Primary::of(TaskColumn::Review).label(), "Review the diff");
        assert_eq!(Primary::of(TaskColumn::Done).label(), "Open the merge");
        assert_eq!(Primary::of(TaskColumn::Working).label(), "Open the session");
    }

    #[test]
    fn started_reads_as_the_artboard() {
        let now = 1_000_000_000_000;
        let ago = |m: i64| Some(now - m * MINUTE_MS);
        assert_eq!(started_text(None, now, false, ""), "queued");
        assert_eq!(started_text(ago(23), now, true, "14:17"), "23 min ago");
        assert_eq!(started_text(ago(60), now, true, "13:40"), "1 h ago");
        assert_eq!(started_text(ago(218), now, true, "11:02"), "today 11:02");
        assert_eq!(started_text(ago(3000), now, false, "10:00"), "2 d ago");
        assert_eq!(duration_text(5), "5 min");
        assert_eq!(duration_text(125), "2 h 5 min");
    }

    #[test]
    fn a_title_comes_from_the_first_prompt_line() {
        assert_eq!(title_from_prompt("Fix the grid\nand more"), "Fix the grid");
        assert_eq!(title_from_prompt(""), "");
        assert_eq!(title_from_prompt(&"x".repeat(80)).len(), 60);
    }
}
