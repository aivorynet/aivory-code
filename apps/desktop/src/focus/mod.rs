//! The session screen: one session with its chat, code, console and trace. The strip on top
//! lists the live sessions, the chat is in the center. The right panel has three tabs: Code
//! (a full editor with the hunks of the agent, each tagged with its turn), Console (the output
//! of the session) and Trace (the span waterfall). Esc goes back to the war room.

mod chat;
mod editor;
mod highlight;
mod items;
mod rows;
mod strip;
mod trace;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::code::{CodeFileParams, LineRange};
use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::SessionDiff;
use aivory_protocol::{Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, FollowMode, KeystrokeEvent,
    ListAlignment, ListState, Pixels, Subscription, Task, Window, div, px, rgb, rgba,
};
use gpui_base::input::{InputEvent, TextareaState};

use crate::compose;
use crate::model::Model;
use crate::theme::*;
use crate::ui;

use highlight::Span;
use items::Item;
use rows::{Hunk, Row, SplitRow};

#[derive(Debug, Clone)]
pub enum FocusEvent {
    /// Back to the war room.
    Back,
    /// Show another session in the Focus view.
    Open(String),
    /// Open a file of a session in the code lens.
    OpenFile { session: String, path: String },
}

/// How the editor shows a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum View {
    /// The whole file with the hunks in place.
    File,
    /// Only the hunks.
    Diff,
    /// The old and the new side.
    Split,
}

/// The tabs of the right panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Code,
    Console,
    Trace,
    /// Only in a narrow window: the chat takes the place of the right panel.
    Chat,
}

/// The text of a file as the editor shows it.
pub(super) struct TextFile {
    pub text: String,
    /// The colors per line.
    pub spans: Vec<Vec<Span>>,
}

/// A file text that the daemon gave, or is asked for.
#[derive(Default)]
pub(super) struct TextState {
    pub file: Option<Rc<TextFile>>,
    pub busy: bool,
    pub failed: bool,
    /// The stamp of the file diff when the text was read: a change reads it again.
    pub stamp: u64,
}

/// Lines of a file that the composer refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodeRef {
    pub path: String,
    pub from: u32,
    pub to: u32,
    /// The chip follows the selection in the editor until the user keeps it.
    pub live: bool,
}

impl CodeRef {
    pub fn label(&self) -> String {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        if self.from == self.to {
            format!("{name}:{}", self.from)
        } else {
            format!("{name}:{}-{}", self.from, self.to)
        }
    }

    /// The reference in the prompt.
    pub fn reference(&self) -> String {
        if self.from == self.to {
            format!("@{}#L{}", self.path, self.from)
        } else {
            format!("@{}#L{}-{}", self.path, self.from, self.to)
        }
    }
}

/// Where the editor scrolls to when its rows are ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Jump {
    Hunk(usize),
    Top,
}

/// The rows of the open file in the open view, and what they were made from.
pub(super) struct Rows {
    pub key: (String, View, Option<u32>, u64, bool),
    pub hunks: Vec<Hunk>,
    pub rows: Vec<Row>,
    pub split: Vec<SplitRow>,
    /// The text of the file in this view, when it is read.
    pub text: Option<Rc<TextFile>>,
}

pub struct FocusScreen {
    model: Entity<Model>,
    session: String,
    focus: FocusHandle,
    // The strip.
    chip_hover: Option<String>,
    pop_hover: bool,
    /// The chip whose popup is open.
    pop_for: Option<String>,
    /// The prompt field of the New session popover, while it is open.
    new_input: Option<Entity<TextareaState>>,
    chip_bounds: HashMap<String, Bounds<Pixels>>,
    strip_scroll: gpui::ScrollHandle,
    /// The session that the strip last scrolled to.
    strip_for: Option<String>,
    strip_tries: u8,
    /// The asks that this view answered: the card shows the answer.
    answered: HashMap<String, String>,
    // The chat.
    list: ListState,
    items: Vec<Item>,
    ask_card: Option<chat::AskCard>,
    input: Entity<TextareaState>,
    tools: compose::Tools,
    refs: Vec<CodeRef>,
    controls: crate::compose_controls::Controls,
    // The right panel.
    tab: Tab,
    /// The window was narrow at the last frame: a change to narrow opens the chat tab.
    was_narrow: bool,
    console: Entity<crate::session_console::SessionConsole>,
    trace: trace::TraceState,
    // The editor.
    diff: Option<(Option<SessionDiff>, Instant)>,
    diff_busy: bool,
    file: Option<String>,
    view: View,
    /// "After turn n": the file as the end of that turn left it. `None`: the working tree.
    turn: Option<u32>,
    texts: HashMap<(String, Option<u32>), TextState>,
    rows: Option<Rows>,
    code: ListState,
    jump: Option<Jump>,
    /// The hunk that J and K stand on, as an index in the open file.
    cursor: usize,
    reviewed: HashSet<(String, usize)>,
    sel: Option<(String, u32, u32)>,
    dragging: bool,
    _keys: Subscription,
    _observe: Subscription,
    _input: Subscription,
    _console: Subscription,
    _ticker: Task<()>,
}

impl EventEmitter<FocusEvent> for FocusScreen {}

/// How long a read diff counts as fresh: short while the agent writes.
const DIFF_LIVE: Duration = Duration::from_secs(3);
const DIFF_IDLE: Duration = Duration::from_secs(20);

impl FocusScreen {
    pub fn new(
        model: Entity<Model>,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if let Some(view) = weak.upgrade() {
                view.update(cx, |this, cx| this.on_keystroke(event, window, cx));
            }
        });
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Reply, or ask about the selected lines")
                .submit_on_enter(true)
                .auto_grow(2, 5)
        });
        {
            let weak = cx.entity().downgrade();
            crate::complete::attach(
                &input,
                model.clone(),
                move |cx| weak.upgrade().map(|v| v.read(cx).session().to_string()),
                |_| (None, None),
                cx,
            );
        }
        let console = cx.new(|cx| {
            crate::session_console::SessionConsole::new(model.clone(), session.clone(), cx)
        });
        let console_events = cx.subscribe(
            &console,
            |this, _, event: &crate::session_console::ConsoleEvent, cx| {
                let crate::session_console::ConsoleEvent::OpenFile { path, .. } = event;
                this.tab = Tab::Code;
                this.open_file(path, Jump::Top, cx);
            },
        );
        let want_code = crate::session_console::take_want_code(&session);
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                this.submit(window, cx);
            }
        });
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |this, cx| this.refresh_diff(false, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        let mut this = Self {
            model,
            session,
            focus: cx.focus_handle(),
            chip_hover: None,
            pop_hover: false,
            pop_for: None,
            new_input: None,
            chip_bounds: HashMap::new(),
            strip_scroll: gpui::ScrollHandle::new(),
            strip_for: None,
            strip_tries: 0,
            answered: HashMap::new(),
            list,
            items: Vec::new(),
            ask_card: None,
            input,
            tools: Default::default(),
            refs: Vec::new(),
            controls: Default::default(),
            tab: Tab::Code,
            was_narrow: false,
            console,
            trace: Default::default(),
            diff: None,
            diff_busy: false,
            file: None,
            view: match std::env::var("AIVORY_FOCUS_VIEW").as_deref() {
                // Screenshot runs start in a view.
                _ if want_code => View::Diff,
                Ok("diff") => View::Diff,
                Ok("split") => View::Split,
                _ => View::File,
            },
            turn: None,
            texts: HashMap::new(),
            rows: None,
            code: ListState::new(0, ListAlignment::Top, px(600.)),
            jump: None,
            cursor: 0,
            reviewed: HashSet::new(),
            sel: None,
            dragging: false,
            _keys: keys,
            _observe: observe,
            _input: events,
            _console: console_events,
            _ticker: ticker,
        };
        this.refresh_diff(true, cx);
        this
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// Show another session; the view keeps its layout.
    pub fn show(&mut self, session: &str, cx: &mut Context<Self>) {
        if self.session == session {
            return;
        }
        self.session = session.to_string();
        if crate::session_console::take_want_code(session) {
            self.tab = Tab::Code;
            self.view = View::Diff;
        }
        self.console.update(cx, |c, cx| c.show(session, cx));
        self.trace = Default::default();
        self.strip_for = None;
        self.strip_tries = 0;
        self.diff = None;
        self.diff_busy = false;
        self.file = None;
        self.turn = None;
        self.texts.clear();
        self.rows = None;
        self.jump = Some(Jump::Top);
        self.cursor = 0;
        self.reviewed.clear();
        self.sel = None;
        self.refs.clear();
        self.controls = Default::default();
        self.items.clear();
        self.ask_card = None;
        self.list.reset(0);
        self.refresh_diff(true, cx);
        cx.notify();
    }

    /// The live pane of the session on screen.
    pub(super) fn pane(&self, cx: &gpui::App) -> Option<Pane> {
        self.war(cx)?.session(&self.session).cloned()
    }

    pub(super) fn war<'a>(&self, cx: &'a gpui::App) -> Option<&'a WarRoom> {
        self.model.read(cx).war.as_ref()
    }

    /// Read the diff of the session again when the last read is old.
    pub(super) fn refresh_diff(&mut self, force: bool, cx: &mut Context<Self>) {
        let working = self
            .pane(cx)
            .is_some_and(|p| p.state == SessionState::Working);
        let ttl = if working { DIFF_LIVE } else { DIFF_IDLE };
        let stale = self.diff.as_ref().is_none_or(|(_, at)| at.elapsed() > ttl);
        if !(stale || force) || self.diff_busy {
            return;
        }
        self.diff_busy = true;
        let id = self.session.clone();
        let task = self
            .model
            .read(cx)
            .call::<method::SessionDiffMethod>(SessionRef { id: id.clone() }, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.session != id {
                    return;
                }
                this.diff_busy = false;
                let new = result.ok();
                let changed = this.diff.as_ref().map(|(d, _)| d) != Some(&new);
                this.diff = Some((new, Instant::now()));
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Ask the daemon for the text of a file, once per diff stamp.
    pub(super) fn ensure_text(
        &mut self,
        path: &str,
        turn: Option<u32>,
        stamp: u64,
        cx: &mut Context<Self>,
    ) {
        let key = (path.to_string(), turn);
        let entry = self.texts.entry(key.clone()).or_default();
        // A turn snapshot never changes: only the working tree reads again.
        let stale = entry.file.is_none() && !entry.failed
            || (turn.is_none() && entry.stamp != stamp && entry.file.is_some());
        if entry.busy || !stale {
            return;
        }
        entry.busy = true;
        entry.stamp = stamp;
        let Some(pane) = self.pane(cx) else { return };
        let params = |session: Option<String>| CodeFileParams {
            codebase: pane.codebase.clone(),
            path: path.to_string(),
            session,
            rev: turn.map(|n| format!("turn:{n}")),
            lines: Some(LineRange { from: 1, to: 1 }),
        };
        let first = self
            .model
            .read(cx)
            .call::<method::CodeFileMethod>(params(Some(pane.id.clone())), cx);
        let retry_params = params(None);
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let mut result = first.await;
            if result.is_err() && turn.is_none() {
                // The session knows no worktree: read the main checkout.
                let retry = this
                    .update(cx, |this, cx| {
                        this.model
                            .read(cx)
                            .call::<method::CodeFileMethod>(retry_params, cx)
                    })
                    .ok();
                if let Some(retry) = retry {
                    result = retry.await;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.session != session {
                    return;
                }
                let entry = this.texts.entry(key).or_default();
                entry.busy = false;
                match result {
                    Ok(file) => {
                        let lang = highlight::lang_of(&file.path);
                        let spans = highlight::lex(lang, &file.text);
                        entry.file = Some(Rc::new(TextFile {
                            text: file.text,
                            spans,
                        }));
                        entry.failed = false;
                    }
                    Err(_) => entry.failed = true,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The composer text with the references and the attachments, or `None` when it is empty.
    fn message(&self, cx: &gpui::App) -> Option<String> {
        let text = self.input.read(cx).value().trim().to_string();
        let mut refs: Vec<String> = self.refs.iter().map(CodeRef::reference).collect();
        refs.extend(self.tools.references(None));
        if text.is_empty() && refs.is_empty() && self.tools.images().is_empty() {
            return None;
        }
        Some(match (text.is_empty(), refs.is_empty()) {
            (_, true) => text,
            (true, false) => refs.join(" "),
            (false, false) => format!("{text} {}", refs.join(" ")),
        })
    }

    /// Send the composer: an answer to the open ask, else a message to the agent.
    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(cx) else { return };
        let Some(text) = self.message(cx) else { return };
        let prefs = self.controls.current(&pane);
        if let Some(ask) = &pane.ask {
            self.answer(&pane.id, &ask.id, None, Some(text), cx);
        } else {
            let images = self.tools.images();
            crate::compose_controls::submit(&self.model, &pane, text, images, prefs, cx);
        }
        self.tools.sent();
        self.refs.clear();
        self.sel = None;
        self.input.update(cx, |s, cx| s.clean(window, cx));
        cx.notify();
    }

    /// Answer an ask: an option, a text, or both. The card then shows the answer.
    pub(super) fn answer(
        &mut self,
        session: &str,
        ask: &str,
        option: Option<String>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let shown = option.clone().or_else(|| text.clone()).unwrap_or_default();
        self.model.read(cx).send::<method::SessionAnswer>(
            aivory_protocol::rpc::AnswerParams {
                id: session.to_string(),
                ask: ask.to_string(),
                option,
                text,
            },
            cx,
        );
        self.answered.insert(ask.to_string(), shown);
        cx.notify();
    }

    /// Esc, J and K.
    fn on_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if crate::complete::is_open(cx) {
            return;
        }
        let key = event.keystroke.key.as_str();
        let plain = !event.keystroke.modifiers.modified();
        let typing = self.input.focus_handle(cx).is_focused(window);
        match key {
            "escape" => {
                if self.controls.close()
                    || self.pop_for.take().is_some()
                    || self.new_input.take().is_some()
                {
                    self.chip_hover = None;
                    self.pop_hover = false;
                } else if typing {
                    self.focus.focus(window, cx);
                } else if self.sel.take().is_some() {
                    self.refs.retain(|r| !r.live);
                } else {
                    cx.emit(FocusEvent::Back);
                }
                cx.notify();
            }
            "j" | "k" if plain && !typing => {
                self.step_hunk(if key == "j" { 1 } else { -1 }, cx);
            }
            _ => {}
        }
    }

    /// The changed files of the diff, in order.
    pub(super) fn changed(&self) -> Vec<aivory_protocol::session::DiffFile> {
        self.diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| {
                d.files
                    .iter()
                    .filter(|f| !f.lines.is_empty())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Open a file in the editor. `jump` says where it scrolls.
    pub(super) fn open_file(&mut self, path: &str, jump: Jump, cx: &mut Context<Self>) {
        if self.file.as_deref() != Some(path) {
            self.turn = None;
            self.cursor = 0;
        }
        self.file = Some(path.to_string());
        self.jump = Some(jump);
        cx.notify();
    }

    /// J and K: the next or the previous hunk, over the files in order.
    pub(super) fn step_hunk(&mut self, delta: i32, cx: &mut Context<Self>) {
        let files = self.changed();
        let counts: Vec<usize> = files
            .iter()
            .map(|f| {
                let fallback = self.fallback_turns(&f.path);
                rows::hunks(f, &fallback).len()
            })
            .collect();
        let flat: Vec<(usize, usize)> = counts
            .iter()
            .enumerate()
            .flat_map(|(f, n)| (0..*n).map(move |h| (f, h)))
            .collect();
        if flat.is_empty() {
            return;
        }
        let here = self
            .file
            .as_ref()
            .and_then(|p| files.iter().position(|f| &f.path == p))
            .map(|f| (f, self.cursor));
        let at = here.and_then(|h| flat.iter().position(|x| *x == h));
        let next = match at {
            Some(i) => (i as i32 + delta).rem_euclid(flat.len() as i32) as usize,
            None if delta > 0 => 0,
            None => flat.len() - 1,
        };
        let (f, h) = flat[next];
        let path = files[f].path.clone();
        self.open_file(&path, Jump::Hunk(h), cx);
        self.cursor = h;
    }

    /// The turn of each edit row of the chat that touches `path`: the turn of a hunk that
    /// carries no blame.
    pub(super) fn fallback_turns(&self, path: &str) -> Vec<u32> {
        items::edit_turns(&self.items, path)
    }
}

impl Focusable for FocusScreen {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FocusScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(pane) = self.pane(cx) else {
            return self.gone(cx).into_any_element();
        };
        let strip = self.strip(&pane, cx);
        let narrow = f32::from(window.viewport_size().width) < ui::sw(1100.).into();
        if narrow && !self.was_narrow {
            self.tab = Tab::Chat;
        }
        self.was_narrow = narrow;
        let popup = self.popup(cx);
        let body = if narrow {
            if self.tab == Tab::Chat {
                // The chat fills the window; the tabs sit above it.
                let tabs = self.tab_bar(true, cx);
                let chat = self.chat(&pane, window, cx).min_w(px(0.));
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(tabs)
                    .child(chat)
            } else {
                self.right_panel(&pane, true, window, cx)
            }
        } else {
            if self.tab == Tab::Chat {
                self.tab = Tab::Code;
            }
            let chat = self.chat(&pane, window, cx);
            let right = self.right_panel(&pane, false, window, cx);
            div().flex_1().min_h(px(0.)).flex().child(chat).child(right)
        };
        div()
            .id("focus")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(tk().page))
            .track_focus(&self.focus)
            .child(strip)
            .child(body)
            .children(popup)
            .into_any_element()
    }
}

impl FocusScreen {
    /// The tab bar of the right panel. `chat`: the Chat tab shows too (narrow window).
    fn tab_bar(&mut self, chat: bool, cx: &mut Context<Self>) -> gpui::Div {
        let mut bar = div()
            .flex_none()
            .min_h(ui::sw(36.))
            .px(px(10.))
            .py(px(3.))
            .gap(px(4.))
            .flex()
            .flex_wrap()
            .items_center()
            .border_b_1()
            .border_color(rgba(tk().line))
            .bg(rgb(tk().card));
        let mut tabs = vec![];
        if chat {
            tabs.push((Tab::Chat, "Chat"));
        }
        tabs.extend([
            (Tab::Code, "Code"),
            (Tab::Console, "Console"),
            (Tab::Trace, "Trace"),
        ]);
        for (tab, label) in tabs {
            let on = self.tab == tab;
            bar = bar.child(
                div()
                    .id(label)
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .text_size(ui::ts(T_SMALL))
                    .cursor_pointer()
                    .when(on, |d| d.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
                    .when(!on, |d| {
                        d.text_color(rgb(tk().ink_3))
                            .hover(|s| s.bg(rgb(tk().track)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        bar
    }

    /// The right panel: the tabs and the open tab. `narrow`: the panel fills the window.
    fn right_panel(
        &mut self,
        pane: &Pane,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let bar = self.tab_bar(narrow, cx);
        let body = match self.tab {
            Tab::Code => self.editor(pane, window, cx),
            Tab::Console => div()
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .bg(rgb(tk().card))
                .child(self.console.clone()),
            Tab::Trace => self.trace_tab(cx),
            Tab::Chat => div(),
        };
        let panel = if narrow {
            div().flex_1().min_w(px(0.))
        } else {
            let vw = f32::from(window.viewport_size().width);
            div().flex_none().w(px(
                (vw * 0.45).clamp(ui::sw(420.).into(), ui::sw(900.).into())
            ))
        };
        panel
            .h_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().flex_1().min_h(px(0.)).flex().child(body))
    }

    /// The view of a session that left the war room.
    fn gone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .text_size(ui::ts(T_BODY))
            .text_color(rgb(tk().ink_3))
            .child("This session is not in the war room any more.")
            .child(
                ui::button("focus-gone", "War room", true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FocusEvent::Back))),
            )
    }
}
