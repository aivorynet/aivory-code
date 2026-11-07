//! The war room v2 (`WarRoomV2.dc.html`): the header, then one centered page with Decisions,
//! Sessions (one group per codebase) and the quiet row; on the right the Overview or the focus
//! panel of one session. The page is one virtual list of rows, so hidden panes cost nothing.

mod band;
mod decisions;
mod focus;
mod new_session;
mod pack;
pub(crate) mod pane;

mod search;
pub(crate) mod skins;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::coord::{BlockOverrideParams, MailItem, MailListParams};
use aivory_protocol::layout::{self, GridGroup, PaneDensity};
use aivory_protocol::rpc::{
    AnswerParams, AutopilotPatch, HandoffParams, HistoryParams, Method, SendParams, SessionRef,
    SettingsSetParams, method,
};
use aivory_protocol::session::SessionDiff;
use aivory_protocol::{
    Autopilot, ComposerPrefs, ConsoleBackground, Display, Effort, Line, Mode, Pane, SessionState,
    WarRoom,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, ClipboardItem, Context, Div, Entity, EventEmitter, Focusable, FontWeight,
    KeystrokeEvent, ListAlignment, ListOffset, ListState, Pixels, Render, SharedString,
    Subscription, Window, deferred, div, list, px, rgb, rgba,
};
use gpui_base::input::{InputEvent, TextareaState};
use serde_json::json;

pub use pane::{effort_label, mode_label, models as provider_models};
pub(crate) use skins::bridge_data;
pub use skins::{BridgeDetail, BridgeDetailEvent};

use crate::actions::Action;
use crate::compose;
use crate::console::{BlockList, Pending};
use crate::model::Model;
use crate::theme::*;
use crate::ui;

#[derive(Debug, Clone)]
pub enum WarRoomEvent {
    /// Esc in the Command look: the shell closes the window over the map, else the map closes
    /// its own overlay.
    Escape,
    OpenSession(String),
    /// The New session dialog of the shell.
    NewSession,
    /// The Focus view of one session: chat and code.
    Focus(String),
    /// A click on a file path in a pane: show the file in the Lens.
    OpenFile {
        session: String,
        path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComposerMenu {
    Mode,
    Provider,
    Model,
    Effort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    None,
    Autopilot,
    NewSession,
    Search,
    /// The skin menu in the header.
    Skin,
    PerRow,
    Subagents(String),
    Composer(String, ComposerMenu),
}

/// A transcript in a pane: the main agent or one subagent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BodyKey {
    pub session: String,
    pub subagent: Option<String>,
    /// The copy in the focus panel or the context popup: a list renders in one place only.
    pub focus: bool,
}

/// The state filter of the Sessions section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    /// Needs you and errors.
    Needs,
    Working,
    Idle,
}

impl Filter {
    pub fn keeps(self, state: SessionState) -> bool {
        match self {
            Self::All => true,
            Self::Needs => matches!(state, SessionState::NeedsYou | SessionState::Error),
            Self::Working => state == SessionState::Working,
            Self::Idle => state == SessionState::Idle,
        }
    }
}

/// The tabs of the focus panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTab {
    Chat,
    Code,
}

/// The text field of a pane composer.
struct Composer {
    input: Entity<TextareaState>,
    placeholder: SharedString,
    _events: Subscription,
}

struct Body {
    /// The transcript as blocks. It holds the lines and the index of the first one.
    list: BlockList,
    loading: bool,
    /// A subagent transcript has its first page.
    loaded: bool,
}

/// What a pane render needs to know about the grid.
#[derive(Debug, Clone, Copy)]
pub struct Geom {
    pub columns: u32,
    pub pane_height: f32,
    pub density: PaneDensity,
    /// A pane under [`COMPACT`] wide: smaller text, a one-line input, fewer pickers.
    pub compact: bool,
    pub font_size: f32,
    pub font: &'static str,
    pub theme: PaneTheme,
}

#[derive(Debug, Clone, PartialEq)]
enum Row {
    Decisions,
    Sessions,
    Grid(pack::PackedRow),
    Quiet,
}

const PER_ROW: [u32; 7] = [1, 2, 3, 4, 8, 9, 16];
/// How long a read mailbox counts as fresh.
const BLOCKS_TTL: Duration = Duration::from_secs(15);
const HISTORY_PAGE: u32 = 200;

pub struct WarRoomScreen {
    model: Entity<Model>,
    board: skins::BoardState,
    bridge: skins::BridgeState,
    operator: skins::OperatorState,
    neon: skins::NeonState,
    navigator: skins::NavigatorState,
    holo: skins::HudState,
    tactical: skins::HudState,
    war: Option<Rc<WarRoom>>,
    list: ListState,
    rows: Vec<Row>,
    groups: Vec<GridGroup>,
    /// One per row, for [`Self::layout`].
    signatures: Vec<String>,
    per_row: Option<u32>,
    font_size: u8,
    font: &'static str,
    background: ConsoleBackground,
    open: Overlay,
    /// An overlay closed by a click outside, and when: the click on its own toggle must not
    /// open it again.
    closed: Option<(Overlay, Instant)>,
    hovered: Option<SharedString>,
    filter: Filter,
    /// A Sankey click: only this codebase (and part).
    part_filter: Option<(String, Option<String>)>,
    /// The header toggle of the Overview column.
    overview: bool,
    /// The session in the focus panel on the right.
    focus_session: Option<String>,
    focus_tab: FocusTab,
    focus_file: Option<usize>,
    /// "Answer one by one": an answer moves the focus panel to the next waiting session.
    walk: bool,
    /// The diff of a session and when it was read.
    diffs: HashMap<String, (Option<SessionDiff>, Instant)>,
    diffs_busy: HashSet<String>,
    /// Panes ticked for a bulk action.
    selected: HashSet<String>,
    /// The decision whose context popup is open.
    context: Option<String>,
    /// The inner page width of the last layout.
    page: f32,
    /// The Overview numbers as shown: they ease to the live values (tokens, spend).
    shown: (f64, f64),
    /// The most panes per row that keep [`PANE_MIN`] at this width.
    fit: u32,
    /// The scroll position of the Overview column (for its scrollbar).
    overview_scroll: gpui::ScrollHandle,
    tabs: HashMap<String, usize>,
    bodies: HashMap<BodyKey, Body>,
    composer: HashMap<String, ComposerPrefs>,
    /// Providers the daemon can run (the CLIs it detected), with their models.
    providers: Vec<(String, Vec<String>)>,
    new_session: new_session::NewSession,
    /// `resources/memory`, polled each 5 s for the Memory section of the Overview.
    memory: Option<aivory_protocol::Memory>,
    /// The mode for the next organizer start.
    organizer: aivory_protocol::OrganizerMode,
    _memory_poll: gpui::Task<()>,
    search: search::Search,
    /// The map of the Command look.
    rts: skins::command::RtsMap,
    focus: gpui::FocusHandle,
    inputs: HashMap<String, Composer>,
    /// The composer tools per session: chips, the dictation, the last result.
    tools: HashMap<String, compose::Tools>,
    /// The pane that a header pill scrolled to: it shows a ring for a moment.
    spot: Option<(SharedString, Instant)>,
    /// The open blocks in the mailbox of a session, and when they were read.
    blocks: HashMap<String, (Vec<MailItem>, Instant)>,
    blocks_busy: HashSet<String>,
    geom: Geom,
    width: Pixels,
    /// The animation step of the working panes (4 per second).
    tick: u32,
    /// The daemon time of the last snapshot and when it came: the status line counts seconds.
    clock: (aivory_protocol::UnixMs, Instant),
    /// `--scroll-row`: the row to scroll to at the first render.
    start_row: Option<usize>,
    _keys: Subscription,
    _ticker: gpui::Task<()>,
    /// `--fps`: frames in the current second, the start of that second, the longest frame.
    fps: Option<(u32, Instant, Instant, Duration)>,
    _observe: Subscription,
}

impl EventEmitter<WarRoomEvent> for WarRoomScreen {}

impl WarRoomScreen {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |this, model, cx| {
            this.war = model.read(cx).war.clone().map(Rc::new);
            cx.notify();
        });
        let settings = model
            .read(cx)
            .call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            if let Ok(view) = settings.await {
                let _ = this.update(cx, |this, cx| {
                    this.apply_display(&view.settings.display);
                    this.providers = view
                        .providers
                        .iter()
                        .filter(|p| p.status != aivory_protocol::ProviderStatus::Off)
                        .map(|p| (p.id.clone(), p.models.clone()))
                        .collect();
                    cx.notify();
                });
            }
        })
        .detach();
        search::bind_keys(cx);
        let display = Display::default();
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if let Some(view) = weak.upgrade() {
                view.update(cx, |this, cx| this.on_keystroke(event, window, cx));
            }
        });
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    if this
                        .war
                        .as_ref()
                        .is_some_and(|w| w.sessions.iter().any(|s| s.state.is_live()))
                    {
                        this.tick = this.tick.wrapping_add(1);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            war: model.read(cx).war.clone().map(Rc::new),
            model,
            board: Default::default(),
            bridge: Default::default(),
            operator: Default::default(),
            neon: Default::default(),
            navigator: Default::default(),
            holo: Default::default(),
            tactical: Default::default(),
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            rows: Vec::new(),
            groups: Vec::new(),
            signatures: Vec::new(),
            per_row: None,
            font_size: 16,
            font: Display::FONTS[0],
            background: ConsoleBackground::Dark,
            open: Overlay::None,
            closed: None,
            hovered: None,
            filter: Filter::All,
            part_filter: None,
            overview: true,
            focus_session: None,
            focus_tab: FocusTab::Chat,
            focus_file: None,
            walk: false,
            diffs: HashMap::new(),
            diffs_busy: HashSet::new(),
            selected: HashSet::new(),
            context: None,
            page: 1128.,
            shown: (0., 0.),
            fit: 16,
            overview_scroll: gpui::ScrollHandle::new(),
            tabs: HashMap::new(),
            bodies: HashMap::new(),
            composer: HashMap::new(),
            providers: Vec::new(),
            new_session: Default::default(),
            memory: None,
            organizer: Default::default(),
            _memory_poll: cx.spawn(async move |this, cx| {
                loop {
                    let Ok(call) = this.update(cx, |this, cx| {
                        this.model
                            .read(cx)
                            .call::<method::ResourcesMemory>(Default::default(), cx)
                    }) else {
                        return;
                    };
                    if let Ok(m) = call.await
                        && this
                            .update(cx, |this, cx| {
                                this.memory = Some(m);
                                cx.notify();
                            })
                            .is_err()
                    {
                        return;
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(5))
                        .await;
                }
            }),
            search: Default::default(),
            rts: Default::default(),
            focus: cx.focus_handle(),
            inputs: HashMap::new(),
            tools: HashMap::new(),
            spot: None,
            blocks: HashMap::new(),
            blocks_busy: HashSet::new(),
            geom: Geom {
                columns: 3,
                pane_height: 520.,
                density: PaneDensity::Normal,
                compact: false,
                font_size: 16.,
                font: Display::FONTS[0],
                theme: PaneTheme::DARK,
            },
            width: px(1440.),
            tick: 0,
            start_row: None,
            clock: (0, Instant::now()),
            _keys: keys,
            _ticker: ticker,
            fps: None,
            _observe: observe,
        };
        this.apply_display(&display);
        this.start_row = this.model.read(cx).scroll_row;
        if this.model.read(cx).fps {
            let now = Instant::now();
            this.fps = Some((0, now, now, Duration::ZERO));
        }
        this
    }

    /// Count frames and log the rate once per second; keep the window redrawing.
    fn tick_fps(&mut self, window: &mut Window) {
        let Some((frames, second, last, longest)) = &mut self.fps else {
            return;
        };
        let now = Instant::now();
        *longest = (*longest).max(now - *last);
        *last = now;
        *frames += 1;
        if now - *second >= Duration::from_secs(1) {
            let panes = self.war.as_ref().map_or(0, |w| w.counts().live);
            eprintln!(
                "fps {frames} · longest frame {:.1} ms · {} columns · {panes} live panes",
                longest.as_secs_f64() * 1000.,
                self.geom.columns
            );
            *frames = 0;
            *second = now;
            *longest = Duration::ZERO;
        }
        window.request_animation_frame();
    }

    /// The keys of a pane whose prompt box has the focus: PageUp, PageDown, Home and End scroll
    /// the transcript, 1-9 answer an open ask, Ctrl+C copies the transcript (while the prompt
    /// box is empty), Escape leaves the pane.
    fn on_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open == Overlay::Search {
            let delta = match event.keystroke.key.as_str() {
                "down" => 1,
                "up" => -1,
                _ => 0,
            };
            if delta != 0 {
                let query = self
                    .search
                    .input
                    .as_ref()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                let hits = self
                    .war
                    .as_ref()
                    .map_or(0, |w| search::matches(&w.sessions, &query).len());
                self.search.selected = search::move_selection(self.search.selected, hits, delta);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if self.open == Overlay::None
            && self.board_on()
            && self.focus.is_focused(window)
            && let m = &event.keystroke.modifiers
            && !m.control
            && !m.alt
            && !m.shift
            && !m.platform
            && self.board_key(event.keystroke.key.as_str(), cx)
        {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.open != Overlay::None {
            return;
        }
        let Some((id, input)) = self
            .inputs
            .iter()
            .find(|(_, c)| c.input.focus_handle(cx).is_focused(window))
            .map(|(id, c)| (id.clone(), c.input.clone()))
        else {
            return;
        };
        let Some(pane) = self.war.as_ref().and_then(|w| w.session(&id)).cloned() else {
            return;
        };
        let tab = self.tabs.get(&id).copied().unwrap_or(0);
        let key = BodyKey {
            session: id.clone(),
            subagent: tab
                .checked_sub(1)
                .and_then(|i| pane.subagents.get(i))
                .map(|s| s.id.clone()),
            focus: self.focus_session.as_deref() == Some(id.as_str()),
        };
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let bare = !m.control && !m.alt && !m.shift && !m.platform;
        let empty = input.read(cx).value().is_empty();
        let list = self.bodies.get(&key).map(|b| b.list.list.clone());
        let page = list.as_ref().map_or(px(0.), |l| {
            l.viewport_bounds().size.height - px(self.geom.font_size * 3.)
        });
        let handled = match ks.key.as_str() {
            "pageup" if !m.control && !m.alt => {
                list.iter().for_each(|l| l.scroll_by(-page));
                true
            }
            "pagedown" if !m.control && !m.alt => {
                list.iter().for_each(|l| l.scroll_by(page));
                true
            }
            "home" if bare && empty => {
                list.iter().for_each(|l| {
                    l.scroll_to(ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.),
                    })
                });
                true
            }
            "end" if bare && empty => {
                list.iter().for_each(|l| l.scroll_to_end());
                true
            }
            "escape" if bare => {
                window.blur(cx);
                true
            }
            "c" if m.control && !m.shift && !m.alt && empty => {
                if let Some(body) = self.bodies.get(&key) {
                    cx.write_to_clipboard(ClipboardItem::new_string(body.list.text()));
                }
                true
            }
            digit if bare && empty && digit.len() == 1 => {
                let n = digit.parse::<usize>().unwrap_or(0);
                match (&pane.ask, n) {
                    (Some(ask), 1..=9) if n <= ask.options.len() => {
                        self.send::<method::SessionAnswer>(
                            AnswerParams {
                                id: pane.id.clone(),
                                ask: ask.id.clone(),
                                option: Some(ask.options[n - 1].clone()),
                                text: None,
                            },
                            cx,
                        );
                        self.answered(&pane.id, cx);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn apply_display(&mut self, d: &Display) {
        crate::ui::set_ui_scale(d.ui_scale);
        self.per_row = d.per_row.map(u32::from);
        self.font_size = d
            .font_size
            .clamp(Display::FONT_SIZE.0, Display::FONT_SIZE.1);
        self.font = Display::FONTS
            .iter()
            .find(|f| **f == d.font)
            .copied()
            .unwrap_or(Display::FONTS[0]);
        self.background = d.background;
        crate::look::apply(d, false);
    }

    fn save_display(&self, key: &str, value: serde_json::Value, cx: &mut Context<Self>) {
        self.send::<method::SettingsSet>(
            SettingsSetParams {
                key: format!("display.{key}"),
                value,
            },
            cx,
        );
    }

    pub(super) fn send<M>(&self, params: M::Params, cx: &mut Context<Self>)
    where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        self.model.read(cx).send::<M>(params, cx);
    }

    /// Send a user action to aivoryd (ADR 0022).
    pub(super) fn act(&self, action: Action, cx: &mut Context<Self>) {
        crate::actions::act(&self.model, action, cx);
    }

    pub(super) fn toggle(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        if overlay == Overlay::NewSession {
            cx.emit(WarRoomEvent::NewSession);
            return;
        }
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
    pub(super) fn overlay(&self, el: Div, cx: &mut Context<Self>) -> AnyElement {
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

    /// The last `resources/memory` reading (the Command sidebar shows it).
    pub fn memory(&self) -> Option<&aivory_protocol::Memory> {
        self.memory.as_ref()
    }

    /// Read the settings again (display prefs, providers), for example after Settings.
    pub fn reload_settings(&mut self, cx: &mut Context<Self>) {
        let settings = self
            .model
            .read(cx)
            .call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            if let Ok(view) = settings.await {
                let _ = this.update(cx, |this, cx| {
                    this.apply_display(&view.settings.display);
                    this.providers = view
                        .providers
                        .iter()
                        .filter(|p| p.status != aivory_protocol::ProviderStatus::Off)
                        .map(|p| (p.id.clone(), p.models.clone()))
                        .collect();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Scroll the grid to the group of a codebase.
    pub fn reveal_group(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(group) = self.groups.iter().position(|g| g.id == id) else {
            return;
        };
        if let Some(row) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Grid(items) if items.iter().any(|(g, _)| *g == group)))
        {
            self.list.scroll_to_reveal_item(row);
            cx.notify();
        }
    }

    fn placeholder(&self, pane: &Pane) -> SharedString {
        if self
            .tools
            .get(&pane.id)
            .is_some_and(|t| t.listening.is_some())
        {
            "Listening… speak now".into()
        } else if pane.ask.is_some() {
            "Or type your own answer…".into()
        } else {
            "Type a message…".into()
        }
    }

    /// Read the mailbox of a pane again when the last read is old. A pane that is on screen
    /// calls this; the banner of an open block shows from the result.
    fn ensure_blocks(&mut self, session: &str, cx: &mut Context<Self>) {
        let stale = self
            .blocks
            .get(session)
            .is_none_or(|(_, at)| at.elapsed() > BLOCKS_TTL);
        if !stale || !self.blocks_busy.insert(session.to_string()) {
            return;
        }
        let task = self.model.read(cx).call::<method::MailListMethod>(
            MailListParams {
                session: session.to_string(),
            },
            cx,
        );
        let id = session.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.blocks_busy.remove(&id);
                let open = result
                    .map(|list| crate::block::open_blocks(&list.items))
                    .unwrap_or_default();
                let changed = this.blocks.get(&id).map(|(old, _)| old) != Some(&open);
                this.blocks.insert(id, (open, Instant::now()));
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The human lifts a block: the session may edit the keys again.
    pub(super) fn override_block(&mut self, session: &str, key: String, cx: &mut Context<Self>) {
        let task = self.model.read(cx).call_notify::<method::BlockOverride>(
            BlockOverrideParams {
                session: session.to_string(),
                keys: vec![key],
            },
            cx,
        );
        let id = session.to_string();
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            if task.await.is_some() {
                model.update(cx, |m, cx| {
                    m.notify_user(crate::notice::Draft::info("The block is lifted."), cx)
                });
                let _ = this.update(cx, |this, cx| {
                    this.blocks.remove(&id);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The folder of a session, for the `@path` references.
    fn session_cwd(&self, session: &str) -> Option<std::path::PathBuf> {
        self.war.as_ref().and_then(|w| {
            let pane = w.session(session)?;
            Some(w.codebase(&pane.codebase)?.path.clone().into())
        })
    }

    /// The tools of a session, made on first use.
    fn tools_of(&mut self, session: &str) -> &mut compose::Tools {
        self.tools.entry(session.to_string()).or_default()
    }

    /// The paperclip: attach files; each becomes a chip.
    pub(super) fn attach(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id = session.to_string();
        compose::attach(move |this: &mut Self| this.tools.get_mut(&id), window, cx);
    }

    /// The screen capture: attach a PNG of the screen as a chip.
    pub(super) fn capture(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools_of(session).capturing {
            return;
        }
        self.tools_of(session).capturing = true;
        cx.notify();
        let id = session.to_string();
        compose::capture(move |this: &mut Self| this.tools.get_mut(&id), window, cx);
    }

    /// A button of the tool line (stop, the offline model).
    pub(super) fn tool_action(
        &mut self,
        session: &str,
        action: compose::ToolAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.inputs.get(session).map(|c| c.input.clone()) else {
            return;
        };
        let id = session.to_string();
        compose::handle_action(
            self,
            action,
            move |this: &mut Self| this.tools.get_mut(&id),
            input,
            window,
            cx,
        );
    }

    /// The mic: dictate into the composer of a session; a second click stops the dictation.
    pub(super) fn dictate(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.inputs.get(session).map(|c| c.input.clone()) else {
            return;
        };
        let Some(stop) = self.tools_of(session).start_listening() else {
            self.tools_of(session).stop_listening();
            cx.notify();
            return;
        };
        cx.notify();
        let id = session.to_string();
        compose::dictate(
            move |this: &mut Self| this.tools.get_mut(&id),
            stop,
            input,
            window,
            cx,
        );
    }

    /// The text field of a pane, made on first use. Enter sends, Shift+Enter breaks the line.
    fn composer_input(
        &mut self,
        pane: &Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        let placeholder = self.placeholder(pane);
        if let Some(c) = self.inputs.get_mut(&pane.id) {
            if c.placeholder != placeholder {
                c.placeholder = placeholder.clone();
                c.input.update(cx, |s, cx| {
                    s.set_placeholder(placeholder.clone(), window, cx)
                });
            }
            return c.input.clone();
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(placeholder.clone())
                .submit_on_enter(true)
                .auto_grow(1, 4)
        });
        let id = pane.id.clone();
        let events = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit(&id, window, cx);
                }
            },
        );
        self.inputs.insert(
            pane.id.clone(),
            Composer {
                input: input.clone(),
                placeholder,
                _events: events,
            },
        );
        input
    }

    /// Send the composer text: an answer to the open ask, a handoff when the provider
    /// changed, or a message to the main agent or the picked subagent.
    pub(super) fn submit(&mut self, session: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.inputs.get(session).map(|c| c.input.clone()) else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        let Some(pane) = self.war.as_ref().and_then(|w| w.session(session)).cloned() else {
            return;
        };
        let cwd = self.session_cwd(session);
        let has_chips = self
            .tools
            .get(session)
            .is_some_and(|t| !t.attachments.is_empty());
        if text.is_empty() && !has_chips {
            return;
        }
        let text = match self.tools.get(session) {
            Some(tools) => tools.message(&text, cwd.as_deref()),
            None => text,
        };
        let prefs = self.composer_prefs(&pane);
        if let Some(ask) = &pane.ask {
            self.send::<method::SessionAnswer>(
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
                    let new_id = created.id.clone();
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
                    // Follow the new session: its deep dive opens.
                    let _ = this.update(cx, |_, cx| cx.emit(WarRoomEvent::OpenSession(new_id)));
                }
            })
            .detach();
            self.composer.remove(&pane.id);
        } else {
            let tab = self.tabs.get(&pane.id).copied().unwrap_or(0);
            let subagent = tab
                .checked_sub(1)
                .and_then(|i| pane.subagents.get(i))
                .map(|s| s.id.clone());
            self.send::<method::SessionSend>(
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
        if let Some(tools) = self.tools.get_mut(session) {
            tools.sent();
        }
        input.update(cx, |s, cx| s.clean(window, cx));
        cx.notify();
    }

    /// The providers of the composer menu: the detected ones, else the design list.
    fn provider_ids(&self) -> Vec<String> {
        if self.providers.is_empty() {
            ["claude", "codex", "gemini", "opencode", "goose"]
                .iter()
                .map(|p| p.to_string())
                .collect()
        } else {
            self.providers.iter().map(|(id, _)| id.clone()).collect()
        }
    }

    fn models(&self, provider: &str) -> Vec<String> {
        match self.providers.iter().find(|(id, _)| id == provider) {
            Some((_, models)) if !models.is_empty() => models.clone(),
            _ => pane::models(provider)
                .iter()
                .map(|m| m.to_string())
                .collect(),
        }
    }

    fn composer_prefs(&self, pane: &Pane) -> ComposerPrefs {
        self.composer
            .get(&pane.id)
            .cloned()
            .unwrap_or_else(|| pane.composer.clone())
    }

    fn pick_composer(
        &mut self,
        session: &str,
        menu: ComposerMenu,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.war.as_ref().and_then(|w| w.session(session)) else {
            return;
        };
        let mut prefs = self.composer_prefs(pane);
        match menu {
            ComposerMenu::Mode => {
                prefs.mode = match value {
                    "Plan" => Mode::Plan,
                    "Auto" => Mode::Auto,
                    _ => Mode::Edit,
                }
            }
            ComposerMenu::Provider => {
                if prefs.provider != value {
                    prefs.provider = value.to_string();
                    prefs.model = self.models(value).first().cloned().unwrap_or_default();
                }
            }
            ComposerMenu::Model => prefs.model = value.to_string(),
            ComposerMenu::Effort => {
                prefs.effort = match value {
                    "low" => Effort::Low,
                    "medium" => Effort::Medium,
                    "max" => Effort::Max,
                    _ => Effort::High,
                }
            }
        }
        self.composer.insert(session.to_string(), prefs);
        self.open = Overlay::None;
        cx.notify();
    }

    /// Create or refresh the transcript of a pane: older pages stay, the tail follows the pane.
    fn ensure_body(&mut self, key: &BodyKey, pane: &Pane, cx: &mut Context<Self>) {
        if !self.bodies.contains_key(key) {
            let list = BlockList::new(ListAlignment::Top);
            let weak = cx.entity().downgrade();
            let scroll_key = key.clone();
            list.list.set_scroll_handler(move |event, _, cx| {
                if event.visible_range.start == 0 {
                    let weak = weak.clone();
                    let key = scroll_key.clone();
                    cx.defer(move |cx| {
                        if let Some(view) = weak.upgrade() {
                            view.update(cx, |this, cx| this.load_older(&key, cx));
                        }
                    });
                }
            });
            self.bodies.insert(
                key.clone(),
                Body {
                    list,
                    loading: false,
                    loaded: false,
                },
            );
        }
        let pending = match pane.state {
            SessionState::Working => Pending::Running,
            SessionState::NeedsYou => Pending::Waiting,
            _ => Pending::No,
        };
        if key.subagent.is_some() {
            let body = self.bodies.get_mut(key).expect("body exists");
            if !body.loaded && !body.loading {
                self.load_page(key, None, cx);
            } else if body.list.pending() != pending {
                let (lines, start) = (body.list.lines().to_vec(), body.list.start());
                body.list.set(lines, start, pending);
            }
            return;
        }
        let body = self.bodies.get_mut(key).expect("body exists");
        let tail_start = pane.lines_total.saturating_sub(pane.tail.len() as u32);
        if body.list.lines().is_empty() && body.list.start() == 0 {
            body.list.set(pane.tail.clone(), tail_start, pending);
            return;
        }
        let start = body.list.start();
        let keep = tail_start.saturating_sub(start) as usize;
        let have = body.list.lines();
        if tail_start < start || keep > have.len() {
            // The transcript was rewritten (or pages are missing): start over from the tail.
            body.list.set(pane.tail.clone(), tail_start, pending);
            return;
        }
        if have.len() == keep + pane.tail.len() && have[keep..] == pane.tail[..] {
            if body.list.pending() != pending {
                let lines = have.to_vec();
                body.list.set(lines, start, pending);
            }
            return;
        }
        let mut lines = have[..keep].to_vec();
        lines.extend_from_slice(&pane.tail);
        body.list.set(lines, start, pending);
    }

    fn load_older(&mut self, key: &BodyKey, cx: &mut Context<Self>) {
        let Some(body) = self.bodies.get(key) else {
            return;
        };
        let start = body.list.start();
        if body.loading || start == 0 || (key.subagent.is_some() && !body.loaded) {
            return;
        }
        self.load_page(key, Some(start), cx);
    }

    fn load_page(&mut self, key: &BodyKey, before: Option<u32>, cx: &mut Context<Self>) {
        let Some(body) = self.bodies.get_mut(key) else {
            return;
        };
        body.loading = true;
        let task = self.model.read(cx).call::<method::SessionHistory>(
            HistoryParams {
                id: key.session.clone(),
                subagent: key.subagent.clone(),
                before,
                limit: Some(HISTORY_PAGE),
            },
            cx,
        );
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let page = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(body) = this.bodies.get_mut(&key) else {
                    return;
                };
                body.loading = false;
                let Ok(page) = page else {
                    body.loaded = true;
                    return;
                };
                let pending = body.list.pending();
                if before.is_none() {
                    body.list.set(page.lines, page.start, pending);
                } else if page.start < body.list.start() {
                    let n = (body.list.start() - page.start) as usize;
                    let mut lines: Vec<Line> = page.lines.into_iter().take(n).collect();
                    lines.extend_from_slice(body.list.lines());
                    body.list.set(lines, page.start, pending);
                }
                body.loaded = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// Fold or unfold the result rows of a block.
    fn toggle_fold(&mut self, key: &BodyKey, abs: u32, cx: &mut Context<Self>) {
        if let Some(body) = self.bodies.get_mut(key) {
            body.list.toggle(abs);
            cx.notify();
        }
    }

    /// The width of the right column: the focus panel, the Overview, or nothing.
    fn right_width(&self, window: f32) -> f32 {
        if self.board_on() {
            match skins::width_class(window) {
                skins::WidthClass::Compact => 0.,
                class => skins::side_width(class),
            }
        } else if window < 1100. {
            0.
        } else if self.focus_session.is_some() {
            focus::focus_width(window)
        } else if self.overview {
            OVERVIEW_W
        } else {
            0.
        }
    }

    /// The Sessions filter and the Sankey filter.
    fn keeps(&self, pane: &Pane) -> bool {
        self.filter.keeps(pane.state)
            && self.part_filter.as_ref().is_none_or(|(cb, part)| {
                &pane.codebase == cb && part.as_ref().is_none_or(|p| &pane.part == p)
            })
    }

    /// Recompute the rows when the page changes shape; the list re-measures only then.
    fn layout(&mut self, war: &WarRoom, width: Pixels) {
        let vw = f32::from(width);
        let narrow = width < NARROW;
        let rail = if narrow {
            f32::from(RAIL_W_COMPACT)
        } else {
            f32::from(RAIL_W)
        };
        let right = self.right_width(vw);
        let grid = page_width(vw - rail - right, narrow);
        let auto = auto_columns(grid, f32::from(self.font_size));
        // A fixed count never makes a pane narrower than PANE_MIN.
        self.fit = fit_columns(grid);
        let columns = if narrow {
            1
        } else {
            match self.per_row {
                Some(n) => n.min(self.fit),
                None => auto,
            }
        };
        let pane_width = (grid - PANE_GAP * (columns as f32 - 1.)) / columns as f32;
        self.geom = Geom {
            columns,
            pane_height: layout::pane_height(columns) as f32,
            density: PaneDensity::for_columns(columns),
            compact: !narrow && pane_width < COMPACT,
            font_size: f32::from(self.font_size),
            font: self.font,
            theme: PaneTheme::for_look(self.background),
        };
        self.width = width;
        self.page = grid;
        let groups = grid_groups(war, columns, |s| self.keeps(s));
        let spans: Vec<u32> = groups.iter().map(|g| g.span).collect();
        let mut rows = vec![Row::Decisions, Row::Sessions];
        rows.extend(pack::pack(&spans, columns).into_iter().map(Row::Grid));
        if war.sessions.iter().any(|s| is_quiet(s, war.now)) {
            rows.push(Row::Quiet);
        }
        // One signature per row: only rows whose signature changed are measured again, so
        // the scroll position stays where it is.
        let global = format!(
            "{columns}|{}|{narrow}|{}|{}",
            self.font_size, self.geom.compact, grid as u32
        );
        let ids = |indexes: &[usize]| -> String {
            indexes
                .iter()
                .filter_map(|i| war.sessions.get(*i).map(|s| s.id.as_str()))
                .collect::<Vec<_>>()
                .join(",")
        };
        let signatures: Vec<String> = rows
            .iter()
            .map(|row| match row {
                Row::Decisions => {
                    let asks: Vec<String> = decisions::decisions(war)
                        .iter()
                        .filter_map(|s| s.ask.as_ref().map(|a| format!("{}:{}", s.id, a.id)))
                        .collect();
                    format!("{global}|dec|{}", asks.join(","))
                }
                Row::Sessions => format!("{global}|sessions|{}", self.part_filter.is_some()),
                Row::Grid(items) => {
                    let mut sig = format!("{global}|grid");
                    for (g, _) in items {
                        let g = &groups[*g];
                        sig.push_str(&format!("|{}:{}", g.id, ids(&g.sessions)));
                    }
                    sig
                }
                Row::Quiet => {
                    let quiet: Vec<&str> = war
                        .sessions
                        .iter()
                        .filter(|s| is_quiet(s, war.now))
                        .map(|s| s.id.as_str())
                        .collect();
                    format!("{global}|quiet|{}", quiet.join(","))
                }
            })
            .collect();
        let old = &self.signatures;
        let prefix = old
            .iter()
            .zip(&signatures)
            .take_while(|(a, b)| a == b)
            .count();
        let room = old.len().min(signatures.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(signatures.iter().rev())
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        if prefix < old.len().max(signatures.len()) {
            self.list.splice(
                prefix..old.len() - suffix,
                signatures.len() - suffix - prefix,
            );
        }
        self.groups = groups;
        self.rows = rows;
        self.signatures = signatures;
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(war) = self.war.clone() else {
            return div().into_any_element();
        };
        let narrow = self.width < NARROW;
        let (top, bottom, content) = match self.rows.get(ix).cloned() {
            Some(Row::Decisions) => (
                48.,
                0.,
                self.render_decisions(&war, self.page, cx)
                    .into_any_element(),
            ),
            Some(Row::Sessions) => (72., 0., self.sessions_head(&war, cx).into_any_element()),
            Some(Row::Grid(items)) => {
                let first = matches!(self.rows.get(ix.wrapping_sub(1)), Some(Row::Sessions));
                let last = !matches!(self.rows.get(ix + 1), Some(Row::Grid(_)));
                let mut row = div()
                    .w_full()
                    .grid()
                    .grid_cols(self.geom.columns as u16)
                    .gap_x(px(PANE_GAP))
                    .items_start();
                for (group, _) in items {
                    row = row.child(self.render_group(&war, group, window, cx));
                }
                (
                    if first { 0. } else { 56. },
                    if last && !matches!(self.rows.get(ix + 1), Some(Row::Quiet)) {
                        96.
                    } else {
                        0.
                    },
                    row.into_any_element(),
                )
            }
            Some(Row::Quiet) => (56., 96., self.render_quiet(&war, cx).into_any_element()),
            None => return div().into_any_element(),
        };
        // The page: centered, at most 1240 px wide, 56 px side padding.
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(PAGE_MAX))
                    .px(px(if narrow { 16. } else { PAGE_PAD }))
                    .pt(px(top))
                    .pb(px(bottom))
                    .child(content),
            )
            .into_any_element()
    }

    fn render_group(
        &mut self,
        war: &WarRoom,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(group) = self.groups.get(index).cloned() else {
            return div();
        };
        let sub = if group.count == 1 {
            "1 session".to_string()
        } else {
            format!("{} sessions", group.count)
        };
        let header = div()
            .mb(px(18.))
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w(px(0.))
            .overflow_hidden()
            .child(ui::state_dot(group.state, 8.))
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(tk().display_heading)
                    .text_size(ui::ts(18.))
                    .font_weight(FontWeight::NORMAL)
                    .whitespace_nowrap()
                    .child(group.label.clone()),
            )
            .child(
                div()
                    .min_w(px(0.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .when(group.span > 1 || self.geom.columns < 4, |d| d.child(sub)),
            );
        let mut grid = div()
            .grid()
            .grid_cols(group.span as u16)
            .gap(px(PANE_GAP))
            .min_w(px(0.));
        for &i in &group.sessions {
            if let Some(pane) = war.sessions.get(i) {
                grid = grid.child(self.render_pane(pane, war.now, window, cx));
            }
        }
        div()
            .col_span(group.span as u16)
            .flex()
            .flex_col()
            .min_w(px(0.))
            .child(header)
            .child(grid)
    }

    /// `Sessions`, then the tools: the state filter, the Sankey filter chip, panes per row and
    /// the Display popover.
    fn sessions_head(&self, war: &WarRoom, cx: &mut Context<Self>) -> Div {
        let live: Vec<&Pane> = war
            .sessions
            .iter()
            .filter(|s| layout::in_grid(s, war.now))
            .collect();
        let count = |f: Filter| live.iter().filter(|s| f.keeps(s.state)).count();
        let mut filters = ui::seg_track();
        for (f, label) in [
            (Filter::All, "All"),
            (Filter::Needs, "Needs you"),
            (Filter::Working, "Working"),
            (Filter::Idle, "Idle"),
        ] {
            let text = if f == Filter::All {
                label.to_string()
            } else {
                format!("{label} {}", count(f))
            };
            filters = filters.child(
                ui::seg_button(("flt", f as usize), self.filter == f, text, false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.filter = f;
                        cx.notify();
                    }),
                ),
            );
        }
        let chip = self.part_filter.as_ref().map(|(cb, part)| {
            let label = match part {
                Some(p) => format!("{cb} / {p}"),
                None => cb.clone(),
            };
            div()
                .h(px(28.))
                .pl(px(10.))
                .pr(px(4.))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .rounded_full()
                .bg(rgb(tk().track))
                .font_family(tk().mono)
                .text_size(ui::ts(T_CAPTION))
                .child(label)
                .child(
                    div()
                        .id("chip-clear")
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(tk().track_2)))
                        .tooltip(ui::tip("Clear filter"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.part_filter = None;
                            cx.notify();
                        }))
                        .child("×"),
                )
        });
        let tools = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(filters)
            .children(chip)
            .child(div().flex_1())
            .child(self.grid_controls(cx));
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(decisions::section_head(
                "Sessions",
                Some("click a title to open it on the right".to_string()),
            ))
            .child(tools)
            .child(div().h(px(36.)))
    }

    /// `Auto · 2 per row ▾`: a dropdown with Auto and the fixed counts.
    fn grid_controls(&self, cx: &mut Context<Self>) -> Div {
        let label = match self.per_row {
            None => format!("Auto · {} per row ▾", self.geom.columns),
            Some(n) if n > self.fit => format!("{} per row ▾", self.fit),
            Some(n) => format!("{n} per row ▾"),
        };
        let open = self.open == Overlay::PerRow;
        let mut wrap = div().relative().child(
            ui::button_sm("per-row", label, false)
                .when(open, |d| d.bg(rgb(tk().track_2)))
                .on_click(cx.listener(|this, _, _, cx| this.toggle(Overlay::PerRow, cx))),
        );
        if open {
            let mut menu = ui::menu(150.);
            for (i, n) in std::iter::once(None)
                .chain(PER_ROW.iter().copied().map(Some))
                .enumerate()
            {
                let text = match n {
                    None => "Auto".to_string(),
                    Some(n) => format!("{n} per row"),
                };
                let fits = n.is_none_or(|n| n <= self.fit);
                if !fits {
                    // Too many for this width: shown, but not offered.
                    menu = menu.child(
                        div()
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .text_size(ui::ts(T_BODY))
                            .text_color(rgb(tk().muted))
                            .child(text)
                            .child(
                                div()
                                    .text_size(ui::ts(T_CAPTION))
                                    .child("needs a wider window"),
                            ),
                    );
                    continue;
                }
                menu = menu.child(ui::menu_item(
                    ("per", i),
                    self.per_row == n,
                    div().child(text).into_any_element(),
                    None,
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.per_row = n;
                        this.save_display(
                            "perRow",
                            n.map_or(serde_json::Value::Null, |n| json!(n)),
                            cx,
                        );
                        this.open = Overlay::None;
                        cx.notify();
                    })),
                ));
            }
            wrap =
                wrap.child(self.overlay(div().absolute().top(px(40.)).right_0().child(menu), cx));
        }
        div().flex().flex_none().child(wrap)
    }

    /// The Autopilot switch in the header and its popover.
    pub(super) fn autopilot_button(&mut self, cx: &mut Context<Self>) -> Div {
        let autopilot = self.war.as_ref().map(|w| w.autopilot.clone());
        let open = self.open == Overlay::Autopilot;
        let mut ap = div().relative().child(
            ui::button_lead(
                "autopilot",
                ui::switch(autopilot.as_ref().is_some_and(|a| a.on)),
                "Autopilot",
            )
            .when(open, |d| d.bg(rgb(tk().track_2)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle(Overlay::Autopilot, cx))),
        );
        if open && let Some(a) = &autopilot {
            let queued = self.war.as_ref().map_or(0, |w| w.queued);
            ap = ap.child(
                self.overlay(
                    div()
                        .absolute()
                        .top(px(44.))
                        .right_0()
                        .child(self.autopilot_popover(a, queued, cx)),
                    cx,
                ),
            );
        }
        ap
    }

    fn render_header(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let new_session = self.new_session_button(window, cx);
        let search = self.search_button(window, cx);
        let ap = self.autopilot_button(cx);
        let shown = self.overview && self.focus_session.is_none();
        let overview = ui::button_lead(
            "overview",
            ui::icon("icons/panel.svg", 14., rgb(tk().ink_2)),
            "Overview",
        )
        .when(shown, |d| d.bg(rgb(tk().track_2)))
        .tooltip(ui::tip(if shown {
            "Hide the overview"
        } else {
            "Show the overview"
        }))
        .on_click(cx.listener(|this, _, _, cx| {
            this.overview = !(this.overview && this.focus_session.is_none());
            this.focus_session = None;
            cx.notify();
        }));
        div()
            .flex_none()
            .h(HEADER_H)
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(if self.width < NARROW { 16. } else { 48. }))
            .bg(rgb(tk().card))
            .child(
                div()
                    .font_family(tk().display)
                    .text_size(ui::ts(T_DISPLAY))
                    .font_weight(FontWeight::NORMAL)
                    .whitespace_nowrap()
                    .child("War room"),
            )
            .child(div().flex_1())
            .child(search)
            .child(self.view_switch(cx))
            .child(ap)
            .child(self.skin_menu(cx))
            .when(self.width >= px(1100.), |d| d.child(overview))
            .child(new_session)
    }

    fn autopilot_popover(&self, a: &Autopilot, queued: u32, cx: &mut Context<Self>) -> Div {
        fn step(v: u32, (lo, hi): (u32, u32), delta: i64) -> u32 {
            (i64::from(v) + delta).clamp(i64::from(lo), i64::from(hi)) as u32
        }
        let set = |patch: AutopilotPatch| -> ui::OnClick {
            Box::new(
                cx.listener(move |this, _, _, cx| this.act(Action::Autopilot(patch.clone()), cx)),
            )
        };
        let none = AutopilotPatch::default;
        let (park_lo, park_hi, park_step) = Autopilot::PARK_AFTER_MIN;
        let park = (park_lo, park_hi);
        let ps = i64::from(park_step);
        let on = set(AutopilotPatch {
            on: Some(!a.on),
            ..none()
        });
        let queued_switch = set(AutopilotPatch {
            start_queued: Some(!a.start_queued),
            ..none()
        });
        let park_dec = set(AutopilotPatch {
            park_after_min: Some(step(a.park_after_min, park, -ps)),
            ..none()
        });
        let park_inc = set(AutopilotPatch {
            park_after_min: Some(step(a.park_after_min, park, ps)),
            ..none()
        });
        let run_dec = set(AutopilotPatch {
            max_running: Some(step(a.max_running, Autopilot::MAX_RUNNING, -1)),
            ..none()
        });
        let run_inc = set(AutopilotPatch {
            max_running: Some(step(a.max_running, Autopilot::MAX_RUNNING, 1)),
            ..none()
        });
        let per_dec = set(AutopilotPatch {
            max_per_codebase: Some(step(a.max_per_codebase, Autopilot::MAX_PER_CODEBASE, -1)),
            ..none()
        });
        let per_inc = set(AutopilotPatch {
            max_per_codebase: Some(step(a.max_per_codebase, Autopilot::MAX_PER_CODEBASE, 1)),
            ..none()
        });
        ui::popover(320.)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(FontWeight::BOLD).child("Autopilot"))
                    .child(ui::switch_button("ap-on", a.on, on)),
            )
            .child(ui::popover_row(
                "Park idle after",
                ui::stepper(
                    "ap-park",
                    format!("{} min", a.park_after_min),
                    park_dec,
                    park_inc,
                ),
            ))
            .child(ui::popover_row(
                "Max running",
                ui::stepper("ap-run", a.max_running.to_string(), run_dec, run_inc),
            ))
            .child(ui::popover_row(
                "Max per codebase",
                ui::stepper("ap-per", a.max_per_codebase.to_string(), per_dec, per_inc),
            ))
            .child(ui::popover_row(
                format!("Start queued tasks · {queued} queued"),
                ui::switch_button("ap-queued", a.start_queued, queued_switch),
            ))
    }
}

impl Render for WarRoomScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.width = window.viewport_size().width;
        let header = if self.board_on() {
            self.render_board_header(window, cx)
        } else {
            self.render_header(window, cx)
        };
        let Some(war) = self.war.clone() else {
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
        self.tick_fps(window);
        if self.clock.0 != war.now {
            self.clock = (war.now, Instant::now());
        }
        // Keys (Ctrl K, Escape) reach the war room while nothing else has the focus.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        // The page width excludes the rail and the right column.
        self.layout(&war, window.viewport_size().width);
        if let Some(row) = self.start_row.take()
            && row < self.list.item_count()
        {
            self.list.scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: px(0.),
            });
        }
        // Drop transcripts, selections and the focus of sessions that left the war room.
        self.bodies
            .retain(|k, _| war.sessions.iter().any(|s| s.id == k.session));
        self.inputs
            .retain(|id, _| war.sessions.iter().any(|s| &s.id == id));
        self.selected.retain(|id| {
            war.sessions
                .iter()
                .any(|s| &s.id == id && s.state.is_live())
        });
        self.ease_counters(&war, window);
        if let Some(skin) = self.render_skin(&war, window, cx) {
            let context = self.render_context(cx);
            return div()
                .size_full()
                .relative()
                .flex()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(
                    |this, ev: &gpui::KeyDownEvent, window, cx| match tk().look {
                        // The camera keys work when the 3D view has the focus, not a text field.
                        aivory_protocol::Look::Navigator if this.focus.is_focused(window) => {
                            this.nav_key(&ev.keystroke.key, cx)
                        }
                        aivory_protocol::Look::Bridge => this.bridge_key(&ev.keystroke.key, cx),
                        aivory_protocol::Look::Tactical => this.hud_key(&ev.keystroke.key, cx),
                        aivory_protocol::Look::Command => this.rts_key(&ev.keystroke, cx),
                        aivory_protocol::Look::Holo => {
                            this.holo_key(&ev.keystroke.key, ev.keystroke.modifiers.alt, cx)
                        }
                        aivory_protocol::Look::Operator => this.operator_key(&ev.keystroke.key, cx),
                        _ => {}
                    },
                ))
                .on_action(cx.listener(|this, _: &search::CloseOverlay, _, cx| {
                    this.context = None;
                    this.open = Overlay::None;
                    if tk().look == aivory_protocol::Look::Command {
                        cx.emit(WarRoomEvent::Escape);
                    }
                    cx.notify();
                }))
                .child(skin)
                .children(context);
        }
        let vw = f32::from(self.width);
        let board = self.board_on();
        let right = self.right_width(vw);
        let (side, side_overlay) = if board {
            self.render_board_side(&war, cx)
        } else if right > 0. && self.focus_session.is_some() {
            (
                self.render_focus(&war, right, window, cx)
                    .map(|panel| panel.into_any_element()),
                None,
            )
        } else if right > 0. {
            (
                Some(self.render_overview(&war, right, cx).into_any_element()),
                None,
            )
        } else {
            (None, None)
        };
        let context = self.render_context(cx);
        let selection = self.selection_bar(cx);
        let center = if board {
            self.render_board(&war, window, cx)
        } else {
            div()
                .size_full()
                .relative()
                .child(
                    list(
                        self.list.clone(),
                        cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
                    )
                    .size_full(),
                )
                .child(
                    div().absolute().inset_0().child(
                        gpui_base::Scrollbar::vertical(&self.list)
                            .mode(gpui_base::ScrollbarMode::Always),
                    ),
                )
                .into_any_element()
        };
        div()
            .size_full()
            .relative()
            .flex()
            .when(board, |d| d.bg(rgb(tk().board_bg)))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &search::ToggleSearch, window, cx| {
                this.toggle_search(window, cx)
            }))
            .on_action(cx.listener(|this, _: &search::CloseOverlay, _, cx| {
                if this.context.take().is_none() && this.open == Overlay::None {
                    this.board.side_open = false;
                    this.close_focus(cx);
                }
                this.open = Overlay::None;
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h(px(0.)).relative().child(center)),
            )
            .children(side)
            .children(side_overlay)
            .children(selection)
            .children(context)
    }
}

impl WarRoomScreen {
    /// `Parked and done`: one pill per quiet session with Resume.
    fn render_quiet(&self, war: &WarRoom, cx: &mut Context<Self>) -> Div {
        let mut quiet = div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .mr(px(4.))
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child("Parked and done"),
            );
        for s in war.sessions.iter().filter(|s| is_quiet(s, war.now)) {
            let id = s.id.clone();
            let open_id = s.id.clone();
            quiet = quiet.child(
                div()
                    .h(px(40.))
                    .pl(px(14.))
                    .pr(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .bg(rgb(tk().card))
                    .rounded_full()
                    .shadow(ui::card_shadow())
                    .text_size(ui::ts(T_SMALL))
                    .child(ui::state_dot(s.state, 8.))
                    .child(
                        div()
                            .id(SharedString::from(format!("quiet-title-{}", s.id)))
                            .cursor_pointer()
                            .hover(|st| st.underline())
                            .tooltip(ui::tip("Open the deep dive"))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(WarRoomEvent::OpenSession(open_id.clone()))
                            }))
                            .child(s.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(tk().mono)
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(format!(
                                "{} / {} · {} min ago",
                                s.codebase,
                                s.part,
                                s.minutes(war.now)
                            )),
                    )
                    // A stale idle session still runs; only parked and done sessions resume.
                    .when(s.state != SessionState::Idle, |d| {
                        d.child(
                            ui::button_sm(
                                SharedString::from(format!("resume-{}", s.id)),
                                "Resume",
                                false,
                            )
                            .bg(rgb(tk().card))
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.act(Action::Resume(id.clone()), cx),
                            )),
                        )
                    }),
            );
        }
        quiet
    }

    /// The bulk bar under the grid while panes are selected: message, pause, park, stop.
    fn selection_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.selected.is_empty() {
            return None;
        }
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(30.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded_full()
                .bg(rgba(0xFFFFFF14))
                .text_color(rgb(tk().card))
                .text_size(ui::ts(T_SMALL))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0xFFFFFF2E)))
                .child(label)
        };
        let bulk = |which: Bulk| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                let ids: Vec<String> = this.selected.drain().collect();
                for id in ids {
                    let r = SessionRef { id };
                    match which {
                        Bulk::Pause => this.act(Action::Pause(r.id), cx),
                        Bulk::Park => this.act(Action::Park(r.id), cx),
                        Bulk::Stop => this.act(Action::Stop(r.id), cx),
                    }
                }
                cx.notify();
            })
        };
        let bar = div()
            .absolute()
            .bottom(px(48.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .py(px(6.))
                    .pl(px(20.))
                    .pr(px(6.))
                    .rounded_full()
                    .bg(rgb(tk().dark))
                    .text_color(rgb(tk().card))
                    .shadow(ui::popover_shadow())
                    .occlude()
                    .child(
                        div()
                            .mr(px(8.))
                            .text_size(ui::ts(T_SMALL))
                            .child(format!("{} selected", self.selected.len())),
                    )
                    .child(button("bulk-msg", "Message all").on_click(
                        cx.listener(|this, _, window, cx| this.message_selected(window, cx)),
                    ))
                    .child(button("bulk-pause", "Pause").on_click(bulk(Bulk::Pause)))
                    .child(button("bulk-park", "Park").on_click(bulk(Bulk::Park)))
                    .child(button("bulk-stop", "Stop").on_click(bulk(Bulk::Stop)))
                    .child(button("bulk-clear", "Clear").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.selected.clear();
                            cx.notify();
                        },
                    ))),
            );
        Some(deferred(bar).with_priority(1).into_any_element())
    }

    /// Message all: the text of the first selected composer goes to every selected session.
    fn message_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.selected.iter().cloned().collect();
        let text = ids
            .iter()
            .filter_map(|id| self.inputs.get(id))
            .map(|c| c.input.read(cx).value().trim().to_string())
            .find(|t| !t.is_empty());
        let Some(text) = text else {
            // Nothing typed yet: put the cursor into the first selected composer.
            if let Some(c) = ids.first().and_then(|id| self.inputs.get(id)) {
                c.input.focus_handle(cx).focus(window, cx);
            }
            return;
        };
        for id in &ids {
            self.act(
                Action::Send {
                    id: id.clone(),
                    text: text.clone(),
                    composer: None,
                },
                cx,
            );
            if let Some(c) = self.inputs.get(id) {
                c.input.update(cx, |s, cx| s.clean(window, cx));
            }
        }
        self.selected.clear();
        cx.notify();
    }
}

impl WarRoomScreen {
    /// Move the shown Overview numbers a step toward the live ones; while they differ, the
    /// next frame comes at once.
    fn ease_counters(&mut self, war: &WarRoom, window: &mut Window) {
        let tokens = war.sessions.iter().map(|s| s.tokens).sum::<u64>() as f64;
        let spent: f64 = war.sessions.iter().map(|s| s.cost).sum();
        let step = |shown: f64, target: f64, close: f64| {
            let next = shown + (target - shown) * 0.18;
            if (target - next).abs() <= close {
                target
            } else {
                next
            }
        };
        let next = (
            step(self.shown.0, tokens, 1.),
            step(self.shown.1, spent, 0.005),
        );
        if next != (tokens, spent) {
            window.request_animation_frame();
        }
        self.shown = next;
    }

    /// The breathing phase of working things, 0 to 1, from the 4 Hz tick (about 4.5 s a cycle).
    pub(super) fn breath(&self, offset: f32) -> f32 {
        ((self.tick as f32 * 0.35 + offset).sin() + 1.) / 2.
    }

    /// The faster pulse of a session that waits for the user (about 2 s a cycle).
    pub(super) fn breath_fast(&self) -> f32 {
        ((self.tick as f32 * 0.8).sin() + 1.) / 2.
    }
}

#[derive(Debug, Clone, Copy)]
enum Bulk {
    Pause,
    Park,
    Stop,
}

/// The widest page (`.wrap`) and its side padding.
pub const PAGE_MAX: f32 = 1240.;
pub const PAGE_PAD: f32 = 56.;
/// The gap between panes.
pub const PANE_GAP: f32 = 24.;
/// The Overview column.
pub const OVERVIEW_W: f32 = 380.;

/// The inner width of the page for the room beside the rail and the right column.
pub fn page_width(room: f32, narrow: bool) -> f32 {
    let pad = if narrow { 32. } else { 2. * PAGE_PAD };
    (room.min(PAGE_MAX) - pad).max(240.)
}

/// The narrowest pane that still reads as a terminal.
pub const PANE_MIN: f32 = 260.;

/// The most panes per row that keep [`PANE_MIN`] (at least 1).
pub fn fit_columns(page: f32) -> u32 {
    (((page + PANE_GAP) / (PANE_MIN + PANE_GAP)).floor() as u32).clamp(1, 16)
}

/// Auto columns: `floor((pageW + 24) / (fontSize × 26 + 24))`, 1-16. The v2 artboard uses
/// × 30 at 13 px; × 26 keeps 2 panes per row at the 16 px console font with the Overview open.
pub fn auto_columns(page: f32, font_size: f32) -> u32 {
    (((page + PANE_GAP) / (font_size * 26. + PANE_GAP)).floor() as u32).clamp(1, 16)
}

/// The groups of the grid: one per codebase with live sessions that pass `keep`, in codebase
/// order; codebases where every session is idle come last. In a group: needs you, error,
/// working, idle, the newest change first.
pub fn grid_groups(war: &WarRoom, columns: u32, keep: impl Fn(&Pane) -> bool) -> Vec<GridGroup> {
    let rank = |s: SessionState| match s {
        SessionState::NeedsYou => 0,
        SessionState::Error => 1,
        SessionState::Working => 2,
        _ => 3,
    };
    let mut out = Vec::new();
    for cb in &war.codebases {
        let live: Vec<usize> = (0..war.sessions.len())
            .filter(|&i| {
                let s = &war.sessions[i];
                s.codebase == cb.id && layout::in_grid(s, war.now)
            })
            .collect();
        let mut shown: Vec<usize> = live
            .iter()
            .copied()
            .filter(|&i| keep(&war.sessions[i]))
            .collect();
        if shown.is_empty() {
            continue;
        }
        shown.sort_by_key(|&i| {
            let s = &war.sessions[i];
            (rank(s.state), std::cmp::Reverse(s.since))
        });
        let state = if shown
            .iter()
            .any(|&i| war.sessions[i].state == SessionState::Error)
        {
            SessionState::Error
        } else {
            SessionState::Working
        };
        out.push(GridGroup {
            id: cb.id.clone(),
            label: cb.id.clone(),
            state,
            count: live.len() as i64,
            span: (shown.len() as u32).min(columns.max(1)),
            sessions: shown,
        });
    }
    out.sort_by_key(|g| {
        g.sessions
            .iter()
            .all(|&i| war.sessions[i].state == SessionState::Idle)
    });
    out
}

/// Parked and done sessions, and idle sessions that went stale, sit in the quiet row.
/// Panes narrower than this use the compact layout.
pub const COMPACT: f32 = 460.;

fn is_quiet(pane: &Pane, now: aivory_protocol::UnixMs) -> bool {
    matches!(pane.state, SessionState::Parked | SessionState::Done)
        || (pane.state == SessionState::Idle && !layout::in_grid(pane, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    #[test]
    fn two_panes_fit_a_row_at_1440_with_the_default_font() {
        // 1440 - 76 rail = 1364 -> 1240 max - 112 padding = 1128 px.
        let page = page_width(1440. - 76., false);
        assert_eq!(page, 1128.);
        assert_eq!(auto_columns(page, 16.), 2);
        // The Overview column takes 380 px; two panes still fit.
        let narrow = page_width(1440. - 76. - OVERVIEW_W, false);
        assert_eq!(narrow, 872.);
        assert_eq!(auto_columns(narrow, 16.), 2);
    }

    #[test]
    fn eight_per_row_does_not_fit_a_1440_window() {
        // 1128 px: four 260 px panes fit, eight do not.
        assert_eq!(fit_columns(1128.), 4);
        assert_eq!(fit_columns(872.), 3);
        assert_eq!(fit_columns(100.), 1);
    }

    #[test]
    fn every_live_session_is_in_its_codebase_group_once() {
        let war = Demo::new().war_room;
        let groups = grid_groups(&war, 3, |_| true);
        let mut seen: Vec<usize> = groups.iter().flat_map(|g| g.sessions.clone()).collect();
        let n = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), n);
        let live = war
            .sessions
            .iter()
            .filter(|s| layout::in_grid(s, war.now))
            .count();
        assert_eq!(n, live);
        for g in &groups {
            assert!(g.sessions.iter().all(|&i| war.sessions[i].codebase == g.id));
            assert!(g.span <= 3);
        }
    }

    #[test]
    fn the_needs_you_filter_keeps_waits_and_errors() {
        assert!(Filter::Needs.keeps(SessionState::NeedsYou));
        assert!(Filter::Needs.keeps(SessionState::Error));
        assert!(!Filter::Needs.keeps(SessionState::Working));
        assert!(Filter::All.keeps(SessionState::Idle));
    }

    #[test]
    fn decisions_come_oldest_first() {
        let war = Demo::new().war_room;
        let list = decisions::decisions(&war);
        assert!(list.windows(2).all(|w| w[0].since <= w[1].since));
        assert!(list.iter().all(|s| s.ask.is_some() && s.state.is_live()));
    }
}
