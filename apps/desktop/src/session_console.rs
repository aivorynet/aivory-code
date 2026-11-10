//! The console output of one session: the transcript in the console look of the war-room
//! panes (agent text, tool calls with their output, diffs, prompts). Every look embeds it in
//! its session screen, next to the chat and the changed code, with its own colors.
//!
//! The view loads the newest `session/history` page, loads older pages on demand, and loads
//! the newest page again when the war room reports new lines for the session.

use std::rc::Rc;

use aivory_protocol::Line;
use aivory_protocol::rpc::{HistoryParams, method};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, EventEmitter, ListAlignment, SharedString, Subscription, Window, div,
    list, px, rgba,
};

use crate::console::draw::{Frame, Hooks, Style};
use crate::console::{BlockList, Pending};
use crate::model::Model;
use crate::theme::PaneTheme;

/// Lines of one `session/history` page.
const PAGE: u32 = 200;

#[derive(Debug, Clone)]
pub enum ConsoleEvent {
    /// A file name in the output was clicked.
    OpenFile { session: String, path: String },
}

/// How a look draws the console.
#[derive(Debug, Clone, Copy)]
pub struct ConsoleLook {
    /// The colors of the console. `None`: the console colors of the Display setting.
    pub theme: Option<PaneTheme>,
    /// The text size in px.
    pub size: f32,
    /// The side padding of the lines in px.
    pub pad_x: f32,
    /// The largest width of the text column. `None`: the full width.
    pub max_w: Option<f32>,
}

impl Default for ConsoleLook {
    fn default() -> Self {
        Self {
            theme: None,
            size: 12.5,
            pad_x: 16.,
            max_w: None,
        }
    }
}

pub struct SessionConsole {
    model: gpui::Entity<Model>,
    id: String,
    list: BlockList,
    total: u32,
    loading: bool,
    loaded: bool,
    look: ConsoleLook,
    _observe: Subscription,
}

impl EventEmitter<ConsoleEvent> for SessionConsole {}

impl SessionConsole {
    pub fn new(model: gpui::Entity<Model>, id: String, cx: &mut Context<Self>) -> Self {
        // New lines of the session: load the newest page again.
        let observe = cx.observe(&model, |this: &mut Self, _, cx| {
            let total = this.lines_total(cx);
            if this.loaded && total != this.total {
                this.load(None, cx);
            }
        });
        let mut this = Self {
            model,
            id,
            list: BlockList::new(ListAlignment::Bottom),
            total: 0,
            loading: false,
            loaded: false,
            look: ConsoleLook::default(),
            _observe: observe,
        };
        this.load(None, cx);
        this
    }

    /// Show another session.
    pub fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id == id {
            return;
        }
        self.id = id.to_string();
        self.list = BlockList::new(ListAlignment::Bottom);
        self.total = 0;
        self.loading = false;
        self.loaded = false;
        self.load(None, cx);
        cx.notify();
    }

    /// The colors and sizes of the look.
    pub fn set_look(&mut self, look: ConsoleLook) {
        self.look = look;
    }

    fn lines_total(&self, cx: &App) -> u32 {
        self.model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| w.session(&self.id))
            .map_or(0, |p| p.lines_total)
    }

    fn load_older(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.list.start() == 0 || !self.loaded {
            return;
        }
        let before = self.list.start();
        self.load(Some(before), cx);
    }

    /// `before: None` loads the newest page (first load, and when new lines arrive).
    fn load(&mut self, before: Option<u32>, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let id = self.id.clone();
        let task = self.model.read(cx).call::<method::SessionHistory>(
            HistoryParams {
                id: id.clone(),
                subagent: None,
                before,
                limit: Some(PAGE),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let page = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                // The view moved to another session while the page was on its way.
                if this.id != id {
                    return;
                }
                this.loaded = true;
                let Ok(page) = page else {
                    return;
                };
                this.total = page.total;
                let start = this.list.start();
                let have = this.list.lines();
                if before.is_some() {
                    if page.start < start {
                        let n = (start - page.start) as usize;
                        let mut lines: Vec<Line> = page.lines.into_iter().take(n).collect();
                        lines.extend_from_slice(have);
                        this.list.set(lines, page.start, Pending::No);
                    }
                } else {
                    let end = start as usize + have.len();
                    if !have.is_empty() && page.start as usize <= end && start <= page.start {
                        // The new page continues the lines that are here: append the rest.
                        let skip = end - page.start as usize;
                        let mut lines = have.to_vec();
                        lines.extend(page.lines.into_iter().skip(skip));
                        this.list.set(lines, start, Pending::No);
                    } else {
                        this.list.set(page.lines, page.start, Pending::No);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn theme(&self, cx: &App) -> PaneTheme {
        self.look
            .theme
            .unwrap_or_else(|| self.model.read(cx).console_theme())
    }

    fn row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let look = self.look;
        let style = Style {
            pad_x: 0.,
            pad_top: 12.,
            pad_bottom: 16.,
            ..Style::plain(look.size, self.theme(cx))
        };
        let weak = cx.entity().downgrade();
        let toggle = {
            let weak = weak.clone();
            Rc::new(move |abs: u32, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    this.list.toggle(abs);
                    cx.notify();
                });
            })
        };
        let open_file = {
            let weak = weak.clone();
            let session = self.id.clone();
            Rc::new(move |path: String, _: &mut Window, cx: &mut App| {
                let session = session.clone();
                let _ = weak.update(cx, |_, cx| {
                    cx.emit(ConsoleEvent::OpenFile { session, path })
                });
            })
        };
        let load_older = Rc::new(move |_: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.load_older(cx));
        });
        let hooks = Hooks {
            toggle: Some(toggle),
            open_file: Some(open_file),
            load_older: Some(load_older),
        };
        let id: SharedString = format!("console-{}", self.id).into();
        let frame = Frame {
            style: &style,
            hooks: &hooks,
            id: &id,
            tick: 0,
        };
        let column = div().w_full().px(px(look.pad_x));
        let column = match look.max_w {
            Some(w) => column.max_w(px(w)),
            None => column,
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(column.child(self.list.item(ix, &frame)))
            .into_any_element()
    }
}

impl Render for SessionConsole {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme(cx);
        if self.list.lines().is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(theme.bg))
                .text_color(rgba(theme.dim))
                .text_size(px(self.look.size))
                .child(if self.loaded {
                    "No console output yet."
                } else {
                    "Loading…"
                })
                .into_any_element();
        }
        div()
            .id(SharedString::from(format!("console-view-{}", self.id)))
            .size_full()
            .relative()
            .bg(rgba(theme.bg))
            .child(
                list(
                    self.list.list.clone(),
                    cx.processor(|this, ix: usize, _, cx| this.row(ix, cx)),
                )
                .size_full(),
            )
            // The scroll bar follows the text (it reads the list while it paints).
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(gpui_base::Scrollbar::vertical(&self.list.list)),
            )
            .into_any_element()
    }
}

thread_local! {
    /// The session whose screen opens with its code (the diff) on view.
    static WANT_CODE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Ask the next session screen of `id` to show its code first ("Open diff" of a task).
pub fn want_code(id: &str) {
    WANT_CODE.with(|w| *w.borrow_mut() = Some(id.to_string()));
}

/// `true` once when the session screen of `id` should show its code first.
pub fn take_want_code(id: &str) -> bool {
    WANT_CODE.with(|w| {
        let mut w = w.borrow_mut();
        if w.as_deref() == Some(id) {
            *w = None;
            true
        } else {
            false
        }
    })
}
