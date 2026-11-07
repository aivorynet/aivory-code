//! The session deep dive (`Session.dc.html`): what each agent did, when, at what cost, and the
//! diff. Header with the session controls, stats strip, the tabs Waterfall, DAG, Transcript and
//! Diff, quick pills with the composer, and the "Span evidence" panel on the right.

mod logic;
mod views;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use aivory_protocol::coord::{BlockOverrideParams, MailItem, MailListParams};
use aivory_protocol::rpc::{
    AnswerParams, HandoffParams, HistoryParams, RpcError, SendParams, SessionRef, code, method,
};
use aivory_protocol::{
    ComposerPrefs, Effort, Line, Mode, Pane, ProviderStatus, SessionDiff, SessionState,
    SessionTrace, UnixMs,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, BoxShadow, Context, Div, Entity, EventEmitter, Focusable, FontWeight,
    ListAlignment, SharedString, Subscription, Task, UniformListScrollHandle, Window, deferred,
    div, point, px, relative, rgb, rgba,
};
use gpui_base::input::{InputEvent, TextareaState};

use crate::compose;
use crate::console::{BlockList, Pending};
use crate::model::Model;
use crate::theme::*;
use crate::ui;
use logic::{Axis, CHIPS, KindFilter, Row, activity_text, stats_cells};

/// Lines of one `session/history` page.
const HISTORY_PAGE: u32 = 200;
const PLACEHOLDER: &str = "Message the session, or @ a subagent…";
const QUICK_PILLS: [&str; 4] = [
    "Continue",
    "Run tests",
    "Explain the last turn",
    "Ask another session",
];

#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Back to the war room.
    Back,
    /// Follow a new session (a handoff made it).
    Open(String),
    /// A click on a path in the Diff or the Transcript: show the file in the Lens.
    OpenFile(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Waterfall,
    Dag,
    Transcript,
    Diff,
}

/// Data from the daemon. `Unavailable` is the answer `NOT_AVAILABLE` of a live daemon that
/// has no spans or diff yet.
enum Load<T> {
    Loading,
    Ready(T),
    Unavailable,
    Failed(String),
}

impl<T> Load<T> {
    fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }

    /// A failed refresh keeps the data that the screen already has.
    fn merge(&mut self, result: anyhow::Result<T>) {
        match result {
            Ok(v) => *self = Load::Ready(v),
            Err(e) if is_not_available(&e) => *self = Load::Unavailable,
            Err(e) => {
                if !matches!(self, Load::Ready(_)) {
                    *self = Load::Failed(format!("{e:#}"));
                }
            }
        }
    }
}

fn is_not_available(e: &anyhow::Error) -> bool {
    e.downcast_ref::<RpcError>()
        .is_some_and(|r| r.code == code::NOT_AVAILABLE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Menu {
    Mode,
    Provider,
    Model,
    Effort,
    Target,
    Handoff,
}

/// The paged main transcript.
struct Transcript {
    /// The lines as blocks. It holds the lines and the index of the first one.
    list: BlockList,
    total: u32,
    loading: bool,
    loaded: bool,
}

/// A text field and the subscription that listens to it.
struct Field {
    input: Entity<TextareaState>,
    _events: Subscription,
}

pub struct SessionScreen {
    model: Entity<Model>,
    id: String,
    tab: Tab,
    trace: Load<SessionTrace>,
    diff: Load<SessionDiff>,
    /// The picked span. `None` shows the first span (the run).
    selected: Option<String>,
    collapsed: HashSet<String>,
    kind: KindFilter,
    query: String,
    file: usize,
    /// The visible waterfall rows and the minute axis, made in `render`.
    rows: Vec<Row>,
    axis: Option<Axis>,
    now: UnixMs,
    utc: bool,
    transcript: Transcript,
    composer: Option<Field>,
    search: Option<Field>,
    /// The composer tools: chips, the dictation, the last result.
    tools: compose::Tools,
    /// The open blocks in the mailbox of the session.
    blocks: Vec<MailItem>,
    prefs: Option<ComposerPrefs>,
    /// Providers the daemon can run, with their models.
    providers: Vec<(String, Vec<String>)>,
    /// The subagent that gets the message: an index in `Pane::subagents`. `None` is main.
    target: Option<usize>,
    menu: Option<Menu>,
    /// A menu closed by a click outside, and when: the click on its own button must not open
    /// it again.
    closed: Option<(Menu, Instant)>,
    waterfall_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    _observe: Subscription,
    _poll: Task<()>,
}

impl EventEmitter<SessionEvent> for SessionScreen {}

impl SessionScreen {
    pub fn new(model: Entity<Model>, id: String, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let list = BlockList::new(ListAlignment::Bottom);
        let weak = cx.entity().downgrade();
        list.list.set_scroll_handler(move |event, _, cx| {
            if event.visible_range.start == 0 {
                let weak = weak.clone();
                cx.defer(move |cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |this, cx| this.load_older(cx));
                    }
                });
            }
        });
        let mut this = Self {
            model,
            id,
            tab: Tab::Waterfall,
            trace: Load::Loading,
            diff: Load::Loading,
            selected: None,
            collapsed: HashSet::new(),
            kind: KindFilter::All,
            query: String::new(),
            file: 0,
            rows: Vec::new(),
            axis: None,
            now: 0,
            utc: false,
            transcript: Transcript {
                list,
                total: 0,
                loading: false,
                loaded: false,
            },
            composer: None,
            search: None,
            tools: compose::Tools::default(),
            blocks: Vec::new(),
            prefs: None,
            providers: Vec::new(),
            target: None,
            menu: None,
            closed: None,
            waterfall_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            _observe: observe,
            _poll: poll,
        };
        this.refresh(cx);
        this.load_history(None, cx);
        this.load_providers(cx);
        this
    }

    fn pane(&self, cx: &gpui::App) -> Option<Pane> {
        self.model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| w.session(&self.id))
            .cloned()
    }

    /// Daemon time: the demo clock in demo mode, else the clock of this machine.
    fn clock_now(&self, cx: &gpui::App) -> (UnixMs, bool) {
        let m = self.model.read(cx);
        match (&m.war, m.demo) {
            (Some(war), true) => (war.now, true),
            _ => (chrono::Utc::now().timestamp_millis(), m.demo),
        }
    }

    /// Fetch spans and diff again. A daemon that answered `NOT_AVAILABLE` is not asked again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.trace, Load::Unavailable) {
            let task = self.model.read(cx).call::<method::SessionSpans>(
                SessionRef {
                    id: self.id.clone(),
                },
                cx,
            );
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.trace.merge(result);
                    cx.notify();
                });
            })
            .detach();
        }
        if !matches!(self.diff, Load::Unavailable) {
            let task = self.model.read(cx).call::<method::SessionDiffMethod>(
                SessionRef {
                    id: self.id.clone(),
                },
                cx,
            );
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.diff.merge(result);
                    cx.notify();
                });
            })
            .detach();
        }
        let mail = self.model.read(cx).call::<method::MailListMethod>(
            MailListParams {
                session: self.id.clone(),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = mail.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(list) = result {
                    let open = crate::block::open_blocks(&list.items);
                    if open != this.blocks {
                        this.blocks = open;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        // New transcript lines: fetch the newest page again.
        let total = self.pane(cx).map_or(0, |p| p.lines_total);
        if self.transcript.loaded && total != self.transcript.total {
            self.load_history(None, cx);
        }
    }

    fn load_providers(&mut self, cx: &mut Context<Self>) {
        let settings = self
            .model
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
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn load_older(&mut self, cx: &mut Context<Self>) {
        let t = &self.transcript;
        if t.loading || t.list.start() == 0 || !t.loaded {
            return;
        }
        let before = t.list.start();
        self.load_history(Some(before), cx);
    }

    /// `before: None` loads the newest page (first load, and when new lines arrive).
    fn load_history(&mut self, before: Option<u32>, cx: &mut Context<Self>) {
        if self.transcript.loading {
            return;
        }
        self.transcript.loading = true;
        let task = self.model.read(cx).call::<method::SessionHistory>(
            HistoryParams {
                id: self.id.clone(),
                subagent: None,
                before,
                limit: Some(HISTORY_PAGE),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let page = task.await;
            let _ = this.update(cx, |this, cx| {
                let t = &mut this.transcript;
                t.loading = false;
                t.loaded = true;
                let Ok(page) = page else {
                    return;
                };
                t.total = page.total;
                let start = t.list.start();
                let have = t.list.lines();
                if before.is_some() {
                    if page.start < start {
                        let n = (start - page.start) as usize;
                        let mut lines: Vec<Line> = page.lines.into_iter().take(n).collect();
                        lines.extend_from_slice(have);
                        t.list.set(lines, page.start, Pending::No);
                    }
                } else {
                    let end = start as usize + have.len();
                    if !have.is_empty() && page.start as usize <= end && start <= page.start {
                        // The new page continues the lines that are here: append the rest.
                        let skip = end - page.start as usize;
                        let mut lines = have.to_vec();
                        lines.extend(page.lines.into_iter().skip(skip));
                        t.list.set(lines, start, Pending::No);
                    } else {
                        t.list.set(page.lines, page.start, Pending::No);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The text fields, made on first use (they need the window).
    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.is_none() {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(PLACEHOLDER)
                    .submit_on_enter(true)
                    .auto_grow(2, 5)
            });
            let events =
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.submit(window, cx);
                    }
                });
            self.composer = Some(Field {
                input,
                _events: events,
            });
        }
        if self.search.is_none() {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Search tool, agent, file…")
                    .submit_on_enter(true)
                    .auto_grow(1, 1)
            });
            let events =
                cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.query = input.read(cx).value().to_string();
                        cx.notify();
                    }
                });
            self.search = Some(Field {
                input,
                _events: events,
            });
        }
    }

    /// Show the Diff tab (the Tasks screen opens the diff of a task).
    pub fn show_diff(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Diff;
        cx.notify();
    }

    /// The human lifts a block: the session may edit the keys again.
    fn override_block(&mut self, key: String, cx: &mut Context<Self>) {
        let task = self.model.read(cx).call_notify::<method::BlockOverride>(
            BlockOverrideParams {
                session: self.id.clone(),
                keys: vec![key],
            },
            cx,
        );
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            if task.await.is_some() {
                model.update(cx, |m, cx| {
                    m.notify_user(crate::notice::Draft::info("The block is lifted."), cx)
                });
                let _ = this.update(cx, |this, cx| {
                    this.blocks.clear();
                    this.refresh(cx);
                });
            }
        })
        .detach();
    }

    /// The banner of the first open block, under the header.
    fn block_banner(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let block = self.blocks.first()?.clone();
        let key = block.key.clone();
        Some(crate::block::banner(
            &block,
            "deep",
            crate::block::Colors {
                bg: (tk().q_bg << 8) | 0xFF,
                border: (0xF1D9B5 << 8) | 0xFF,
                text: (tk().q_text << 8) | 0xFF,
                hover: (0xFDF1DE << 8) | 0xFF,
            },
            false,
            Box::new(cx.listener(move |this, _, _, cx| this.override_block(key.clone(), cx))),
        ))
    }

    fn call_ref<M>(&self, cx: &mut Context<Self>)
    where
        M: aivory_protocol::rpc::Method<Params = SessionRef> + 'static,
        M::Result: Send + 'static,
    {
        self.model.read(cx).send::<M>(
            SessionRef {
                id: self.id.clone(),
            },
            cx,
        );
    }

    fn toggle(&mut self, menu: Menu, cx: &mut Context<Self>) {
        if let Some((closed, at)) = self.closed.take()
            && closed == menu
            && at.elapsed() < Duration::from_millis(400)
        {
            cx.notify();
            return;
        }
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    /// Paint `el` above everything; a click outside closes the open menu.
    fn overlay(&self, el: Div, cx: &mut Context<Self>) -> AnyElement {
        deferred(
            el.occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if let Some(menu) = this.menu.take() {
                        this.closed = Some((menu, Instant::now()));
                    }
                    cx.notify();
                })),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn prefs(&self, pane: &Pane) -> ComposerPrefs {
        self.prefs.clone().unwrap_or_else(|| pane.composer.clone())
    }

    fn provider_ids(&self) -> Vec<String> {
        if self.providers.is_empty() {
            aivory_protocol::PROVIDERS
                .iter()
                .map(|p| (*p).to_string())
                .collect()
        } else {
            self.providers.iter().map(|(id, _)| id.clone()).collect()
        }
    }

    fn models(&self, provider: &str) -> Vec<String> {
        match self.providers.iter().find(|(id, _)| id == provider) {
            Some((_, models)) if !models.is_empty() => models.clone(),
            _ => fallback_models(provider)
                .iter()
                .map(|m| (*m).to_string())
                .collect(),
        }
    }

    /// The entries of a menu with the one that is on.
    fn menu_items(&self, menu: Menu, pane: &Pane, prefs: &ComposerPrefs) -> Vec<(String, bool)> {
        match menu {
            Menu::Mode => [Mode::Plan, Mode::Edit, Mode::Auto]
                .iter()
                .map(|m| (mode_label(*m).to_string(), *m == prefs.mode))
                .collect(),
            Menu::Provider => self
                .provider_ids()
                .into_iter()
                .map(|p| {
                    let on = p == prefs.provider;
                    (p, on)
                })
                .collect(),
            Menu::Model => self
                .models(&prefs.provider)
                .into_iter()
                .map(|m| {
                    let on = m == prefs.model;
                    (m, on)
                })
                .collect(),
            Menu::Effort => [Effort::Low, Effort::Medium, Effort::High, Effort::Max]
                .iter()
                .map(|e| (effort_label(*e).to_string(), *e == prefs.effort))
                .collect(),
            Menu::Target => std::iter::once(("main".to_string(), self.target.is_none()))
                .chain(
                    pane.subagents
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (s.name.clone(), self.target == Some(i))),
                )
                .collect(),
            Menu::Handoff => self
                .provider_ids()
                .into_iter()
                .filter(|p| *p != pane.provider)
                .map(|p| (p, false))
                .collect(),
        }
    }

    fn pick(&mut self, menu: Menu, index: usize, value: &str, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(cx) else {
            return;
        };
        let mut prefs = self.prefs(&pane);
        match menu {
            Menu::Mode => {
                prefs.mode = match value {
                    "Plan" => Mode::Plan,
                    "Auto" => Mode::Auto,
                    _ => Mode::Edit,
                }
            }
            Menu::Provider => {
                if prefs.provider != value {
                    prefs.provider = value.to_string();
                    prefs.model = self.models(value).first().cloned().unwrap_or_default();
                }
            }
            Menu::Model => prefs.model = value.to_string(),
            Menu::Effort => {
                prefs.effort = match value {
                    "low" => Effort::Low,
                    "medium" => Effort::Medium,
                    "max" => Effort::Max,
                    _ => Effort::High,
                }
            }
            Menu::Target => self.target = index.checked_sub(1),
            Menu::Handoff => {
                self.menu = None;
                self.handoff(value.to_string(), cx);
                return;
            }
        }
        if menu != Menu::Target {
            self.prefs = Some(prefs);
        }
        self.menu = None;
        cx.notify();
    }

    /// Fork the session (at a span, when the daemon can), and follow the fork.
    pub(crate) fn fork(&mut self, at: Option<String>, cx: &mut Context<Self>) {
        let task = self.model.read(cx).call::<method::SessionFork>(
            aivory_protocol::rpc::ForkParams {
                id: self.id.clone(),
                at,
                provider: None,
                prompt: None,
            },
            cx,
        );
        cx.spawn(async move |this, cx| match task.await {
            Ok(created) => {
                let _ = this.update(cx, |_, cx| cx.emit(SessionEvent::Open(created.id)));
            }
            Err(e) => {
                let _ = this.update(cx, |this, cx| {
                    this.model.update(cx, |m, cx| {
                        m.notify_user(crate::notice::failure("session/fork", &e), cx)
                    })
                });
            }
        })
        .detach();
    }

    /// Continue the session with another provider, and follow the new session.
    fn handoff(&mut self, provider: String, cx: &mut Context<Self>) {
        let task = self.model.read(cx).call::<method::SessionHandoff>(
            HandoffParams {
                id: self.id.clone(),
                provider,
                model: None,
            },
            cx,
        );
        cx.spawn(async move |this, cx| match task.await {
            Ok(created) => {
                let _ = this.update(cx, |_, cx| cx.emit(SessionEvent::Open(created.id)));
            }
            Err(e) => {
                let _ = this.update(cx, |this, cx| {
                    this.model.update(cx, |m, cx| {
                        m.notify_user(crate::notice::failure("session/handoff", &e), cx)
                    })
                });
            }
        })
        .detach();
    }

    /// Send the composer text: an answer to the open ask, a handoff when the provider
    /// changed, or a message to main or the picked subagent.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.composer.as_ref().map(|c| c.input.clone()) else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        let Some(pane) = self.pane(cx) else {
            return;
        };
        if text.is_empty() && self.tools.attachments.is_empty() {
            return;
        }
        let text = self.tools.message(&text, self.cwd(cx).as_deref());
        let prefs = self.prefs(&pane);
        if let Some(ask) = &pane.ask {
            self.model.read(cx).send::<method::SessionAnswer>(
                AnswerParams {
                    id: pane.id.clone(),
                    ask: ask.id.clone(),
                    option: None,
                    text: Some(text),
                },
                cx,
            );
        } else if prefs.provider != pane.provider {
            let handoff = self.model.read(cx).call_notify::<method::SessionHandoff>(
                HandoffParams {
                    id: pane.id.clone(),
                    provider: prefs.provider.clone(),
                    model: Some(prefs.model.clone()),
                },
                cx,
            );
            let model = self.model.clone();
            cx.spawn(async move |this, cx| {
                if let Some(created) = handoff.await {
                    let id = created.id.clone();
                    cx.update(|cx| {
                        model.read(cx).send::<method::SessionSend>(
                            SendParams {
                                id: created.id,
                                text,
                                subagent: None,
                                attachments: Vec::new(),
                                composer: None,
                            },
                            cx,
                        )
                    });
                    let _ = this.update(cx, |_, cx| cx.emit(SessionEvent::Open(id)));
                }
            })
            .detach();
            self.prefs = None;
        } else {
            let subagent = self
                .target
                .and_then(|i| pane.subagents.get(i))
                .map(|s| s.id.clone());
            self.model.read(cx).send::<method::SessionSend>(
                SendParams {
                    id: pane.id.clone(),
                    text,
                    subagent,
                    attachments: Vec::new(),
                    composer: (prefs != pane.composer).then_some(prefs),
                },
                cx,
            );
        }
        self.tools.sent();
        input.update(cx, |s, cx| s.clean(window, cx));
        cx.notify();
    }

    /// A quick pill: fill the composer and send it.
    fn quick(&mut self, text: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.composer.as_ref().map(|c| c.input.clone()) else {
            return;
        };
        input.update(cx, |s, cx| s.set_value(text, window, cx));
        self.submit(window, cx);
    }

    /// The folder of the session: file references are relative to it.
    fn cwd(&self, cx: &gpui::App) -> Option<std::path::PathBuf> {
        let pane = self.pane(cx)?;
        let war = self.model.read(cx).war.as_ref()?;
        Some(war.codebase(&pane.codebase)?.path.clone().into())
    }

    /// The paperclip: attach files; each becomes a chip.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx);
    }

    /// The screen capture: attach a PNG of the screen as a chip.
    fn capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools.capturing {
            return;
        }
        self.tools.capturing = true;
        cx.notify();
        compose::capture(|this: &mut Self| Some(&mut this.tools), window, cx);
    }

    /// A button of the tool line (stop, the offline model).
    fn tool_action(
        &mut self,
        action: compose::ToolAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.composer.as_ref().map(|c| c.input.clone()) else {
            return;
        };
        compose::handle_action(
            self,
            action,
            |this: &mut Self| Some(&mut this.tools),
            input,
            window,
            cx,
        );
    }

    /// The mic: dictate into the composer; a second click stops the dictation.
    fn toggle_rec(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.composer.as_ref().map(|c| c.input.clone()) else {
            return;
        };
        let Some(stop) = self.tools.start_listening() else {
            self.tools.stop_listening();
            cx.notify();
            return;
        };
        cx.notify();
        compose::dictate(
            |this: &mut Self| Some(&mut this.tools),
            stop,
            input,
            window,
            cx,
        );
    }

    /// The span that the evidence panel shows.
    fn selected_span(&self) -> Option<&aivory_protocol::Span> {
        let trace = self.trace.ready()?;
        self.selected
            .as_deref()
            .and_then(|id| trace.spans.iter().find(|s| s.id == id))
            .or_else(|| trace.spans.first())
    }

    fn pick_span(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected = Some(id.to_string());
        cx.notify();
    }

    fn open_in_transcript(&mut self, cx: &mut Context<Self>) {
        let ix = self
            .selected_span()
            .and_then(|span| logic::find_line(self.transcript.list.lines(), span));
        self.tab = Tab::Transcript;
        if let Some(ix) = ix {
            let item = self.transcript.list.item_of_line(ix);
            self.transcript.list.list.scroll_to_reveal_item(item);
        }
        cx.notify();
    }

    // ---- header, stats, tabs ----

    fn header(&self, pane: Option<&Pane>, now: UnixMs, cx: &mut Context<Self>) -> Div {
        let back = div()
            .id("back")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .py(px(6.))
            .pl(px(6.))
            .pr(px(10.))
            .rounded(tk().r_button)
            .text_size(crate::ui::ts(crate::theme::T_BODY))
            .text_color(rgb(tk().ink_2))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
            .when(ui::irix(), |d| {
                d.rounded(px(0.))
                    .bg(rgb(0xC8C8C8))
                    .text_color(rgb(0x111111))
            })
            .when(ui::irix(), |d| ui::bevel_edges(true, d))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionEvent::Back)))
            .child(ui::icon("icons/back.svg", 14., rgb(tk().ink_2)))
            .child("War room");
        let crumb =
            pane.map(|p| format!("{} / {} · {} {}", p.codebase, p.part, p.provider, p.model));
        let title = pane.map_or_else(|| self.id.clone(), |p| p.title.clone());
        let mut header = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(14.))
            .px(px(32.))
            .py(px(12.))
            .min_h(HEADER_H)
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line))
            .child(back)
            .when_some(crumb, |d, crumb| {
                d.child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().ink_3))
                        .whitespace_nowrap()
                        .child(crumb),
                )
            })
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_TITLE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            );
        if let Some(p) = pane {
            let dot = if p.state == SessionState::Done {
                ui::dot(rgb(tk().ok))
            } else {
                ui::state_dot(p.state, 8.)
            };
            header = header.child(
                div()
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .rounded_full()
                    .bg(rgb(tk().rail_on))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .when(ui::irix(), |d| d.rounded(px(0.)).bg(rgb(0xFFFFFF)))
                    .when(ui::irix(), |d| ui::bevel_edges(false, d))
                    .child(dot)
                    .child(format!("{} · {} min", p.state.label(), p.minutes(now))),
            );
        }
        header = header.child(div().flex_1());
        let button = |id: &'static str, label: &'static str, danger: bool| {
            let ink = if danger { tk().e_text } else { tk().ink };
            div()
                .id(id)
                .h(px(32.))
                .px(px(12.))
                .flex()
                .flex_none()
                .items_center()
                .rounded(tk().r_button)
                .border_1()
                .border_color(rgba(tk().line_2))
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .text_color(rgb(if danger { tk().e_text } else { tk().ink }))
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(move |s| s.border_color(rgb(ink)))
                .when(ui::irix(), |d| {
                    d.rounded(px(0.))
                        .border_0()
                        .bg(rgb(0xC8C8C8))
                        .text_color(rgb(if danger { 0xB3141C } else { 0x111111 }))
                })
                .when(ui::irix(), |d| ui::bevel_edges(true, d))
                .child(label)
        };
        let handoff = {
            let open = self.menu == Some(Menu::Handoff);
            let mut wrap = div().relative().flex_none().child(
                button("hb-handoff", "Handoff", false)
                    .when(open, |d| d.border_color(rgb(tk().ink)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(Menu::Handoff, cx))),
            );
            if open && let Some(p) = pane {
                let prefs = self.prefs(p);
                wrap = wrap.child(self.menu_popup(Menu::Handoff, p, &prefs, true, cx));
            }
            wrap
        };
        header
            .child(
                button("hb-pause", "Pause", false).on_click(cx.listener(|this, _, _, cx| {
                    this.call_ref::<method::SessionPause>(cx);
                })),
            )
            .child(
                button("hb-park", "Park", false).on_click(cx.listener(|this, _, _, cx| {
                    this.call_ref::<method::SessionPark>(cx);
                })),
            )
            .child(
                button("hb-fork", "Fork turn", false)
                    .on_click(cx.listener(|this, _, _, cx| this.fork(None, cx))),
            )
            .child(handoff)
            .child(
                button("hb-stop", "Stop", true).on_click(cx.listener(|this, _, _, cx| {
                    this.call_ref::<method::SessionStop>(cx);
                })),
            )
    }

    fn stats_strip(&self, pane: Option<&Pane>) -> Div {
        let cells = match (self.trace.ready(), pane) {
            (Some(t), _) => stats_cells(
                t.stats.subagents,
                Some(t.stats.turns),
                Some(t.stats.tool_calls),
                t.stats.cost,
                t.stats.context_pct,
            ),
            (None, Some(p)) => {
                stats_cells(p.subagents.len() as u32, None, None, p.cost, p.context_pct)
            }
            (None, None) => stats_cells(0, None, None, 0., 0),
        };
        let last = cells.len() - 1;
        let mut strip = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .px(px(32.))
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line));
        for (i, (key, value)) in cells.into_iter().enumerate() {
            strip = strip.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .pt(px(10.))
                    .pb(px(11.))
                    .when(i < last, |d| {
                        d.pr(px(20.))
                            .mr(px(20.))
                            .border_r_1()
                            .border_color(rgba(tk().line))
                    })
                    .child(views::key(key))
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .whitespace_nowrap()
                            .child(value),
                    ),
            );
        }
        strip
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> Div {
        let files = self.diff.ready().map_or(0, |d| d.files.len());
        let tabs = [
            (Tab::Waterfall, "Waterfall".to_string()),
            (Tab::Dag, "DAG".to_string()),
            (Tab::Transcript, "Transcript".to_string()),
            (Tab::Diff, format!("Diff · {files} files")),
        ];
        let mut track = div()
            .flex()
            .flex_none()
            .gap(px(2.))
            .p(px(2.))
            .bg(rgb(tk().track))
            .rounded(tk().r_button);
        if ui::irix() {
            track = ui::bevel_edges(false, track.rounded(px(0.)).bg(rgb(0xBDBDBD)).p(px(4.)));
        }
        for (tab, label) in tabs {
            let on = self.tab == tab;
            track = track.child(
                div()
                    .id(SharedString::from(format!("tab-{tab:?}")))
                    .h(px(30.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .whitespace_nowrap()
                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(rgb(tk().card))
                            .font_weight(FontWeight::MEDIUM)
                            .shadow(vec![BoxShadow {
                                color: ui::hsla(tk().line),
                                offset: point(px(0.), px(1.)),
                                blur_radius: px(2.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                    })
                    .when(!on, |d| d.hover(|s| s.text_color(rgb(tk().ink))))
                    .when(ui::irix(), |d| {
                        d.rounded(px(0.))
                            .shadow(Vec::new())
                            .text_color(rgb(0x111111))
                            .bg(rgb(if on { 0xA6A6A6 } else { 0xC8C8C8 }))
                    })
                    .when(ui::irix(), |d| ui::bevel_edges(!on, d))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        let mut bar = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .pl(px(32.))
            .pr(px(24.))
            .py(px(10.))
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line))
            .child(track);
        if self.tab == Tab::Waterfall {
            bar = bar.child(self.search_box()).child(self.chips(cx));
        }
        bar
    }

    fn search_box(&self) -> Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .w(px(240.))
            .px(px(10.))
            .border_1()
            .border_color(rgba(tk().line_2))
            .rounded(tk().r_button)
            .text_color(rgb(tk().ink_3))
            .when(ui::irix(), |d| {
                d.rounded(px(0.)).border_0().bg(rgb(0xFFFFFF))
            })
            .when(ui::irix(), |d| ui::bevel_edges(false, d))
            .child(ui::icon("icons/search.svg", 14., rgb(tk().ink_3)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .text_color(rgb(tk().ink))
                    .when_some(self.search.as_ref(), |d, s| d.child(s.input.clone())),
            )
    }

    fn chips(&self, cx: &mut Context<Self>) -> Div {
        let mut chips = div().flex().flex_none().gap(px(6.));
        for (filter, label) in CHIPS {
            let on = self.kind == filter;
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("chip-{label}")))
                    .h(px(28.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded_full()
                    .border_1()
                    .border_color(rgba(if on { 0x1A1A2EFF } else { 0x0D0D0E1F }))
                    .when(on, |d| d.bg(rgb(tk().ink)))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(if on { tk().card } else { tk().ink_2 }))
                    .cursor_pointer()
                    .when(!on, |d| d.hover(|s| s.border_color(rgba(0x0D0D0E4D))))
                    .when(ui::irix(), |d| {
                        d.rounded(px(0.))
                            .border_0()
                            .text_color(rgb(0x111111))
                            .bg(rgb(if on { 0xA6A6A6 } else { 0xC8C8C8 }))
                    })
                    .when(ui::irix(), |d| ui::bevel_edges(!on, d))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.kind = filter;
                        cx.notify();
                    }))
                    .child(div().size(px(8.)).rounded(px(2.)).bg(rgb(filter.color())))
                    .child(label),
            );
        }
        chips
    }

    // ---- composer ----

    fn composer_view(
        &mut self,
        pane: &Pane,
        now: UnixMs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let prefs = self.prefs(pane);
        let activity = activity_text(pane, now);
        let mut pills = div().flex().flex_wrap().gap(px(6.));
        for text in QUICK_PILLS {
            pills = pills.child(
                div()
                    .id(SharedString::from(format!("qa-{text}")))
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(rgb(tk().track))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xE6E9EE)).text_color(rgb(tk().ink)))
                    .when(ui::irix(), |d| {
                        d.rounded(px(0.))
                            .bg(rgb(0xC8C8C8))
                            .text_color(rgb(0x111111))
                    })
                    .when(ui::irix(), |d| ui::bevel_edges(true, d))
                    .on_click(cx.listener(move |this, _, window, cx| this.quick(text, window, cx)))
                    .child(text),
            );
        }
        pills = pills.child(
            div()
                .ml_auto()
                .max_w(px(520.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                .text_color(rgb(0x7C3AED))
                .child(activity),
        );
        let input = self.composer.as_ref().map(|c| c.input.clone());
        let focused = input
            .as_ref()
            .is_some_and(|i| i.focus_handle(cx).contains_focused(window, cx));
        let dd = |menu: Menu, label: String, dot: Option<u32>, cx: &mut Context<Self>| {
            let on = self.menu == Some(menu);
            let mut wrap = div().relative().flex_none().child(
                div()
                    .id(SharedString::from(format!("cm-{menu:?}")))
                    .h(px(22.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .rounded(px(4.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .whitespace_nowrap()
                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                    .when(on, |d| d.bg(rgb(0xEBECF0)))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xEBECF0)).text_color(rgb(tk().ink)))
                    .when_some(dot, |d, c| {
                        d.child(div().size(px(7.)).rounded_full().bg(rgb(c)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(menu, cx)))
                    .child(label),
            );
            if on {
                wrap = wrap.child(self.menu_popup(menu, pane, &prefs, false, cx));
            }
            wrap
        };
        let target = self
            .target
            .and_then(|i| pane.subagents.get(i))
            .map_or("main", |s| s.name.as_str());
        let row = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .ml(px(-4.))
            .child(dd(
                Menu::Mode,
                format!("{} ▾", mode_label(prefs.mode)),
                None,
                cx,
            ))
            .child(dd(
                Menu::Provider,
                format!("{} ▾", prefs.provider),
                Some(provider_color(&prefs.provider)),
                cx,
            ))
            .child(dd(Menu::Model, format!("{} ▾", prefs.model), None, cx))
            .child(dd(
                Menu::Effort,
                format!("{} ▾", effort_label(prefs.effort)),
                None,
                cx,
            ))
            .child(dd(Menu::Target, format!("to: {target} ▾"), None, cx));
        let listening = self.tools.listening.is_some();
        let icon_button = |id: &'static str, icon: &'static str, tip: &'static str| {
            div()
                .id(id)
                .tooltip(ui::tip(tip))
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0xEBECF0)))
                .child(ui::icon(icon, 15., rgb(tk().ink_3)))
        };
        let mic = icon_button(
            "mic",
            "icons/mic.svg",
            if listening {
                "Stop listening"
            } else {
                "Dictate"
            },
        )
        .when(listening, |d| {
            d.bg(rgb(tk().e_bg))
                .child(ui::icon("icons/mic.svg", 15., rgb(tk().e)))
        })
        .on_click(cx.listener(|this, _, window, cx| this.toggle_rec(window, cx)));
        let field = div()
            .relative()
            .flex()
            .rounded(tk().r_card)
            .border_1()
            .border_color(if focused {
                rgb(tk().primary)
            } else {
                rgb(0xCDD2DA)
            })
            .bg(rgb(tk().card))
            .when(ui::irix(), |d| {
                d.rounded(px(0.)).border_0().bg(rgb(0xFFFFFF))
            })
            .when(ui::irix(), |d| ui::bevel_edges(false, d))
            .when(focused, |d| {
                d.shadow(vec![BoxShadow {
                    color: ui::hsla(0x3574F024),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }])
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .justify_between()
                    .p(px(4.))
                    .child(
                        icon_button("cap", "icons/capture.svg", "Capture the screen")
                            .on_click(cx.listener(|this, _, window, cx| this.capture(window, cx))),
                    )
                    .child(
                        icon_button("att", "icons/attach.svg", "Attach files")
                            .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .pt(px(6.))
                    .pb(px(4.))
                    .child(
                        div()
                            .min_h(px(40.))
                            .px(px(2.))
                            .pt(px(2.))
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .line_height(relative(1.45))
                            .font_family(tk().sans)
                            .text_color(rgb(tk().ink))
                            .when_some(input, |d, i| d.child(i)),
                    )
                    .child(row),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .justify_between()
                    .p(px(4.))
                    .when(compose::VOICE_AVAILABLE, |d| d.child(mic))
                    .child(
                        div()
                            .id("send")
                            .tooltip(ui::tip("Send"))
                            .mt_auto()
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(tk().r_button)
                            .bg(rgb(tk().primary))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(tk().primary_hover)))
                            .when(ui::irix(), |d| d.rounded(px(0.)).bg(rgb(0xC8C8C8)))
                            .when(ui::irix(), |d| ui::bevel_edges(true, d))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                            .child(ui::icon(
                                "icons/send.svg",
                                14.,
                                rgb(if ui::irix() { 0x111111 } else { tk().card }),
                            )),
                    ),
            );
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pl(px(32.))
            .pr(px(24.))
            .pt(px(12.))
            .pb(px(14.))
            .bg(rgb(tk().card))
            .border_t_1()
            .border_color(rgba(tk().line))
            .child(pills)
            .children(compose::chips(
                &self.tools,
                "deep",
                compose::Palette::page(),
                |i| {
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.tools.remove(i);
                        cx.notify();
                    }))
                },
            ))
            .children(compose::status(
                &self.tools,
                "deep",
                compose::Palette::page(),
                |action| {
                    Box::new(
                        cx.listener(move |this, _, window, cx| {
                            this.tool_action(action, window, cx)
                        }),
                    )
                },
            ))
            .child(field)
    }

    /// The menu of a dropdown: above the composer button, or below the header button.
    fn menu_popup(
        &self,
        menu: Menu,
        pane: &Pane,
        prefs: &ComposerPrefs,
        below: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut m = ui::menu(150.);
        for (i, (label, on)) in self.menu_items(menu, pane, prefs).into_iter().enumerate() {
            let value = label.clone();
            m = m.child(ui::menu_item(
                SharedString::from(format!("mi-{menu:?}-{i}")),
                on,
                div().child(label).into_any_element(),
                None,
                Box::new(cx.listener(move |this, _, _, cx| this.pick(menu, i, &value, cx))),
            ));
        }
        let anchor = if below {
            div().absolute().top(relative(1.)).mt(px(6.)).right(px(0.))
        } else {
            div()
                .absolute()
                .bottom(relative(1.))
                .mb(px(6.))
                .left(px(0.))
        };
        self.overlay(anchor.child(m), cx)
    }
}

impl Render for SessionScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);
        let pane = self.pane(cx);
        let (now, utc) = self.clock_now(cx);
        self.now = now;
        self.utc = utc;
        match self.trace.ready() {
            Some(trace) => {
                self.rows = logic::rows(&trace.spans, &self.collapsed, self.kind, &self.query);
                self.axis = Some(Axis::new(&trace.spans, now));
            }
            None => {
                self.rows.clear();
                self.axis = None;
            }
        }
        let wide = window.viewport_size().width >= px(1100.);
        let view = match self.tab {
            Tab::Waterfall => self.waterfall_view(cx),
            Tab::Dag => self.dag_view(cx),
            Tab::Transcript => self.transcript_view(cx),
            Tab::Diff => self.diff_view(cx),
        };
        let composer = pane
            .as_ref()
            .map(|p| self.composer_view(p, now, window, cx));
        let left = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .when(wide, |d| d.border_r_1().border_color(rgba(tk().line)))
            .child(self.tab_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .bg(rgb(tk().card))
                    .overflow_hidden()
                    .when(ui::irix(), |d| d.m(px(16.)).bg(rgb(0xFFFFFF)))
                    .when(ui::irix(), |d| ui::bevel_edges(false, d))
                    .child(view),
            )
            .when_some(composer, |d, c| d.child(c));
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(pane.as_ref(), now, cx))
            .children(self.block_banner(cx))
            .child(self.stats_strip(pane.as_ref()))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(left)
                    .when(wide, |d| {
                        let ev = self.evidence(pane.as_ref(), cx);
                        if ui::irix() {
                            d.child(div().flex_none().h_full().py(px(16.)).pr(px(16.)).child(
                                ui::bevel_edges(
                                    false,
                                    div().h_full().bg(rgb(0xFFFFFF)).child(ev.bg(rgb(0xFFFFFF))),
                                ),
                            ))
                        } else {
                            d.child(ev)
                        }
                    }),
            )
    }
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Plan => "Plan",
        Mode::Edit => "Edit",
        Mode::Auto => "Auto",
    }
}

fn effort_label(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Max => "max",
    }
}

/// The model list per provider, until the daemon reports its own.
fn fallback_models(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &["opus-5.5", "sonnet-5.5", "haiku-5.5"],
        "codex" => &["gpt-5.2-codex", "gpt-5.2"],
        "gemini" => &["3-pro", "3-flash"],
        "opencode" => &["kimi-k2", "qwen3-coder"],
        "goose" => &["goose-1.9"],
        _ => &[],
    }
}
