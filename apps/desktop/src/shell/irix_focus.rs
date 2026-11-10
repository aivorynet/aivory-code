//! The Navigator Focus view: one session with its chat and its code, in a grey window with
//! bevelled keys. The chat and the field are at the left; the diff, the files and the tests
//! are at the right. Every control goes through `act` or the shell.

use std::time::{Duration, Instant};

use aivory_protocol::Pane;
use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffFile, DiffLineKind, SessionDiff};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FontWeight, ScrollHandle, Subscription,
    Task as GpuiTask, Window, div, px, relative, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use super::irix_screens::{led_for, ticker};
use crate::actions::{Action, act};
use crate::compose::{self, Tools};
use crate::compose_controls::{ControlStyle, Controls};
use crate::console::display::{DKind, DLine, last};
use crate::media::{self, MediaStyle};
use crate::model::Model;
use crate::session_console::{ConsoleEvent, ConsoleLook, SessionConsole};
use crate::theme::*;
use crate::ui;
use crate::warroom::bridge_data::{provider_name, test_lines, tests_of, turn_of, wait_text};
use crate::warroom::skins::navigator::panels::{
    ALERT, BTN, BTN_HOT, FIELD, KEY_PLAN, LED_GREEN, LED_OFF, MUTED, S_ENTRY, S_LABEL, SCREEN_TEXT,
    Series, TEXT, TITLE_ASK, bevel, heading, kv, list_row, menu_bar, note, page, push, screen,
    scroll_area, section, seg, strip_chart, sunken,
};

/// How old a diff gets before the view asks again.
const TTL: Duration = Duration::from_secs(8);
/// The chat shows this many items at most.
const CHAT_LINES: usize = 16;
/// The diff shows this many lines at most.
const DIFF_LINES: usize = 200;
/// The width of the pane at the right.
const SIDE_W: f32 = 520.;

#[derive(Debug, Clone)]
pub enum IrixFocusEvent {
    /// Show another session.
    Open(String),
    /// Back to the war room.
    Back,
    /// Open a file of the session in the code lens.
    OpenFile { session: String, path: String },
}

impl EventEmitter<IrixFocusEvent> for IrixFocus {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Only in a narrow window: the chat takes the place of the details.
    Chat,
    Diff,
    Files,
    Console,
    Tests,
}

pub struct IrixFocus {
    model: Entity<Model>,
    id: String,
    input: Entity<InputState>,
    controls: Controls,
    tools: Tools,
    console: Entity<SessionConsole>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    side: Side,
    /// The window was narrow at the last frame: a change to narrow opens the chat tab.
    was_narrow: bool,
    /// The path of the file that the diff shows. The first file when `None`.
    file: Option<String>,
    /// The tokens that the session uses, one sample per second.
    tokens: Series,
    diff_scroll: ScrollHandle,
    files_scroll: ScrollHandle,
    _subs: Vec<Subscription>,
    _tick: GpuiTask<()>,
}

impl IrixFocus {
    pub fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Tell the agent what to do"));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) && !crate::complete::is_open(cx) {
                this.send(window, cx);
            }
        });
        let weak = cx.weak_entity();
        crate::complete::attach(
            &input,
            model.clone(),
            move |cx| weak.upgrade().map(|v| v.read(cx).session().to_string()),
            |_| (None, None),
            cx,
        );
        let console = cx.new(|cx| SessionConsole::new(model.clone(), id.clone(), cx));
        console.update(cx, |c, _| {
            c.set_look(ConsoleLook {
                theme: Some(PaneTheme::DARK),
                size: 12.,
                pad_x: 10.,
                max_w: None,
            })
        });
        let console_events = cx.subscribe(&console, |_, _, e: &ConsoleEvent, cx| {
            let ConsoleEvent::OpenFile { session, path } = e;
            cx.emit(IrixFocusEvent::OpenFile {
                session: session.clone(),
                path: path.clone(),
            });
        });
        // The diff is the first tab; "Open diff" from Tasks lands on it too.
        let _ = crate::session_console::take_want_code(&id);
        let tick = ticker(cx, Duration::from_secs(1), |t: &mut Self, cx| {
            if let Some(p) = t.pane(cx) {
                t.tokens.sample(p.tokens as f64);
                cx.notify();
            }
        });
        Self {
            model,
            id,
            input,
            controls: Controls::default(),
            tools: Tools::default(),
            console,
            diff: None,
            busy: false,
            side: Side::Diff,
            was_narrow: false,
            file: None,
            tokens: Series::default(),
            diff_scroll: ScrollHandle::new(),
            files_scroll: ScrollHandle::new(),
            _subs: vec![observe, events, console_events],
            _tick: tick,
        }
    }

    pub fn session(&self) -> &str {
        &self.id
    }

    /// Show another session: the data starts over.
    pub fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.diff = None;
            self.file = None;
            self.tokens = Series::default();
            self.controls = Controls::default();
            self.tools = Tools::default();
            self.side = Side::Diff;
            let id = id.to_string();
            self.console.update(cx, |c, cx| c.show(&id, cx));
            let _ = crate::session_console::take_want_code(&id);
            cx.notify();
        }
    }

    fn pane(&self, cx: &gpui::App) -> Option<Pane> {
        self.model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| w.session(&self.id))
            .cloned()
    }

    /// Send the field: an answer to the open ask, else a message to the agent.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).value().trim().to_string();
        let Some(pane) = self.pane(cx) else {
            return;
        };
        let images = self.tools.images();
        let text = self.tools.message(&typed, None);
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        let action = match &pane.ask {
            Some(ask) => Action::Answer {
                id: pane.id.clone(),
                ask: ask.id.clone(),
                option: None,
                text: Some(text),
            },
            _ => {
                let prefs = self.controls.current(&pane);
                crate::compose_controls::submit(&self.model, &pane, text, images, prefs, cx);
                self.tools.sent();
                self.input.update(cx, |s, cx| s.clean(window, cx));
                return;
            }
        };
        act(&self.model, action, cx);
        self.tools.sent();
        self.input.update(cx, |s, cx| s.clean(window, cx));
    }

    /// Read the diff again when the last read is old.
    fn ensure(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.diff.as_ref().is_some_and(|(_, t)| t.elapsed() <= TTL) {
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
}

/// The label at the left of a chat row.
fn who(color: u32, text: &str) -> Div {
    div()
        .flex_none()
        .w(ui::sw(96.))
        .text_size(ui::ts(S_LABEL))
        .line_height(relative(1.9))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(color))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text.to_uppercase())
}

/// The colors of the attachment chips: the grey keys of the look.
fn chip_palette() -> compose::Palette {
    compose::Palette {
        text: TEXT << 8 | 0xFF,
        dim: MUTED << 8 | 0xFF,
        line: 0x3C3C4340,
        chip_bg: BTN << 8 | 0xFF,
        hover: BTN_HOT << 8 | 0xFF,
        fail: ALERT << 8 | 0xFF,
        ok: LED_GREEN << 8 | 0xFF,
    }
}

/// One display line of the chat: the label of the speaker on the first line of a message,
/// the words at the right.
fn chat_line(d: &DLine) -> Option<Div> {
    if d.kind == DKind::Meta {
        return None;
    }
    let color = match d.kind {
        DKind::You => 0x1A4FA0,
        DKind::Error => ALERT,
        _ => MUTED,
    };
    let label = if d.first {
        who(color, d.speaker().unwrap_or(""))
    } else {
        div().flex_none().w(ui::sw(96.))
    };
    let mono = matches!(
        d.kind,
        DKind::Code | DKind::Table | DKind::Tool | DKind::Output
    );
    let mut text = div()
        .flex_1()
        .min_w_0()
        .text_size(ui::ts(if mono { S_LABEL } else { S_ENTRY }))
        .line_height(relative(1.5))
        .text_color(rgb(match d.kind {
            DKind::Output => MUTED,
            DKind::Error => ALERT,
            _ => TEXT,
        }));
    if mono {
        text = text
            .font_family(tk().mono)
            .whitespace_nowrap()
            .overflow_hidden();
    }
    if d.kind == DKind::Heading {
        text = text.font_weight(FontWeight::BOLD);
    }
    let body = match d.kind {
        DKind::Output => format!("└ {}", d.text),
        DKind::Bullet => d.text.clone(),
        _ => d.text.clone(),
    };
    Some(
        div()
            .flex_none()
            .flex()
            .gap(px(12.))
            .items_start()
            .child(label)
            .child(text.child(if body.is_empty() {
                " ".to_string()
            } else {
                body
            })),
    )
}

impl Render for IrixFocus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = self.pane(cx) else {
            return page()
                .child(heading(
                    "Session",
                    "This session is not in the war room any more.",
                ))
                .child(
                    menu_bar().child(
                        push("if-gone", "War room", true)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(IrixFocusEvent::Back))),
                    ),
                );
        };
        self.ensure(cx);
        let vw = f32::from(window.viewport_size().width);
        let narrow = vw < f32::from(ui::sw(1100.));
        if !narrow && self.side == Side::Chat {
            self.side = Side::Diff;
        }
        if narrow && !self.was_narrow {
            self.side = Side::Chat;
        }
        self.was_narrow = narrow;
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let mins = p.minutes(now);
        let waits = p.ask.is_some();

        // ---- title and facts ------------------------------------------------------------------
        let mut meta = provider_name(&p.provider);
        if !p.model.is_empty() {
            meta.push_str(&format!(" · {}", p.model));
        }
        meta.push_str(&format!(" · {}", p.codebase));
        if let Some(t) = turn_of(&p) {
            meta.push_str(&format!(" · turn {t:02}"));
        }
        let title = if p.title.is_empty() { &p.id } else { &p.title };
        let state_word = match p.state {
            aivory_protocol::SessionState::Error => "Error",
            aivory_protocol::SessionState::Working => "Working",
            aivory_protocol::SessionState::Idle => "Idle",
            aivory_protocol::SessionState::NeedsYou => "Waits for you",
            aivory_protocol::SessionState::Parked => "Parked",
            aivory_protocol::SessionState::Done => "Done",
        };
        let head = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(24.))
            .child(heading(title, &meta).flex_1().min_w(ui::sw(240.)))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(crate::warroom::skins::navigator::panels::led(led_for(&p)))
                    .child(kv(
                        state_word,
                        if waits {
                            format!("waits {}", wait_text(mins))
                        } else {
                            format!("in state {}", wait_text(mins))
                        },
                        if waits { ALERT } else { TEXT },
                    )),
            );

        // ---- keys: the sessions, the deep dive, back --------------------------------------------
        let sessions: Vec<(String, String, u32)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                w.sessions
                    .iter()
                    .filter(|s| s.state.is_live())
                    .map(|s| {
                        let name = if s.title.is_empty() { &s.id } else { &s.title };
                        (s.id.clone(), name.clone(), led_for(s))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut keys = menu_bar().flex_wrap();
        // Whole names only: the tabs that fit, the open session always among them, then "+N".
        let mut used = 0.;
        let mut shown: Vec<usize> = Vec::new();
        let here = sessions.iter().position(|(sid, _, _)| *sid == self.id);
        if let Some(h) = here {
            used += sessions[h].1.chars().count() as f32 * 8.5 + 40.;
            shown.push(h);
        }
        for (k, (_, name, _)) in sessions.iter().enumerate() {
            let w = name.chars().count() as f32 * 8.5 + 40.;
            if Some(k) != here && used + w <= (vw - 220.).clamp(320., 1100.) {
                used += w;
                shown.push(k);
            }
        }
        shown.sort_unstable();
        for k in &shown {
            let (sid, name, _) = &sessions[*k];
            let sid = sid.clone();
            keys = keys.child(seg(format!("if-s-{k}"), name, sid == self.id).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(IrixFocusEvent::Open(sid.clone()))),
            ));
        }
        if let Some((sid, _, _)) = sessions
            .iter()
            .enumerate()
            .find(|(k, _)| !shown.contains(k))
            .map(|(_, v)| v)
        {
            let sid = sid.clone();
            let more = sessions.len() - shown.len();
            keys = keys.child(seg("if-s-more", &format!("+{more}"), false).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(IrixFocusEvent::Open(sid.clone()))),
            ));
        }
        keys = keys.child(div().flex_1()).child(
            push("if-back", "War room", false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(IrixFocusEvent::Back))),
        );

        // ---- left: the chat, the ask, the field ---------------------------------------------------
        let shown = last(&p.tail, CHAT_LINES);
        let mut lines = div().p(px(16.)).flex().flex_col().gap(px(2.));
        let media_style = MediaStyle {
            card: rgb(BTN),
            text: rgb(TEXT),
            muted: rgb(MUTED),
            radius: 0.,
        };
        for (n, d) in shown.iter().enumerate() {
            if d.kind == DKind::Meta {
                let found = p
                    .tail
                    .iter()
                    .rev()
                    .find(|l| l.media.is_some() && l.text.trim_end() == d.text);
                if let Some(m) = found.and_then(|l| l.media.as_ref()) {
                    lines = lines.child(
                        div()
                            .flex_none()
                            .pl(ui::sw(108.))
                            .py(px(4.))
                            .child(media::preview(m, ("if-media", n), media_style)),
                    );
                }
                continue;
            }
            if let Some(r) = chat_line(d) {
                // A new message starts a little lower.
                lines = lines.child(r.when(d.first, |r| r.mt(px(8.))));
            }
        }
        if shown.is_empty() {
            lines = lines.child(note("No rows yet."));
        }
        // Top-aligned, from the start of a message; what does not fit is cut at the bottom.
        let chat = sunken(div().size_full().overflow_hidden().child(lines))
            .flex_1()
            .min_h(px(96.));

        let ask_card = p.ask.as_ref().map(|ask| {
            let mut buttons = div().flex().flex_wrap().gap(px(10.));
            if p.controlled {
                for (k, option) in ask.options.iter().enumerate() {
                    let (sid, aid, opt) = (p.id.clone(), ask.id.clone(), option.clone());
                    buttons = buttons.child(push(format!("if-ans-{k}"), option, k == 0).on_click(
                        cx.listener(move |this, _, _, cx| {
                            act(
                                &this.model,
                                Action::Answer {
                                    id: sid.clone(),
                                    ask: aid.clone(),
                                    option: Some(opt.clone()),
                                    text: None,
                                },
                                cx,
                            );
                        }),
                    ));
                }
            } else {
                buttons = buttons.child(note("Answer it in its terminal."));
            }
            bevel(
                true,
                div()
                    .flex()
                    .flex_col()
                    .bg(rgb(crate::warroom::skins::navigator::panels::FACE))
                    .child(
                        div()
                            .min_h(ui::sw(26.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .bg(rgb(TITLE_ASK))
                            .text_size(ui::ts(S_ENTRY))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child("The agent waits for you"),
                    )
                    .child(
                        div()
                            .p(px(16.))
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(
                                div()
                                    .max_h(px(S_ENTRY * 1.45 * 3.))
                                    .overflow_hidden()
                                    .font_family(tk().mono)
                                    .text_size(ui::ts(S_ENTRY))
                                    .line_height(relative(1.45))
                                    .text_color(rgb(TEXT))
                                    .child(ask.text.clone()),
                            )
                            .child(buttons),
                    ),
            )
            .flex_none()
        });

        let chips = compose::chips(&self.tools, "if", chip_palette(), |i| {
            Box::new(cx.listener(move |this, _, _, cx| {
                this.tools.remove(i);
                cx.notify();
            }))
        });
        let field = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                sunken(
                    div()
                        .min_h(ui::sw(34.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .text_size(ui::ts(S_ENTRY))
                        .text_color(rgb(TEXT))
                        .child(div().flex_1().min_w_0().child(self.input.clone())),
                )
                .flex_1()
                .min_w_0(),
            )
            .child(
                push("if-attach", "Attach", false).on_click(cx.listener(|_, _, window, cx| {
                    compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx)
                })),
            )
            .child(
                push("if-send", "Send", true)
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            );

        self.controls.sync(&p);
        let providers = self.model.read(cx).providers.clone();
        // The grey keys of the look: square, with the plan blue for the pick.
        let style = ControlStyle {
            pill: rgb(BTN),
            pill_hover: rgb(BTN_HOT),
            pill_text: rgb(TEXT),
            on: rgb(KEY_PLAN),
            on_text: rgb(0xFFFFFF),
            menu: rgb(FIELD),
            menu_text: rgb(TEXT),
            menu_hover: rgb(BTN_HOT),
            font: None,
            radius: Some(0.),
            text: None,
        };
        let composer = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(self.controls.row(
                "if-ctl",
                &p,
                &providers,
                &style,
                |this: &mut Self| &mut this.controls,
                cx,
            ))
            .children(chips)
            .child(field);

        let left = compose::media_target(div(), |this: &mut Self| Some(&mut this.tools), cx)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section("Chat"))
            .child(chat)
            .children(ask_card)
            .child(composer);

        // ---- right: diff, files, tests, activity ----------------------------------------------------
        let files: Vec<DiffFile> = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let (add, del) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        let current: Option<&DiffFile> = self
            .file
            .as_ref()
            .and_then(|path| files.iter().find(|f| &f.path == path))
            .or(files.first());

        let mut tabs = menu_bar().flex_wrap();
        let mut tab_list = vec![];
        if narrow {
            tab_list.push(("if-t-chat", "Chat", Side::Chat));
        }
        tab_list.extend([
            ("if-t-diff", "Diff", Side::Diff),
            ("if-t-files", "Files", Side::Files),
            ("if-t-console", "Console", Side::Console),
            ("if-t-tests", "Tests", Side::Tests),
        ]);
        let mut chat_tabs = menu_bar().flex_wrap();
        for (id, label, side) in tab_list {
            let key = |bar: Div, id: String| {
                bar.child(seg(id, label, self.side == side).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.side = side;
                        cx.notify();
                    },
                )))
            };
            tabs = key(tabs, id.to_string());
            chat_tabs = key(chat_tabs, format!("{id}-c"));
        }
        let (sid, open_path) = (p.id.clone(), current.map(|f| f.path.clone()));
        tabs = tabs
            .child(div().flex_1())
            .child(push("if-lens", "Open in Lens", false).on_click(cx.listener(
                move |_, _, _, cx| {
                    if let Some(path) = &open_path {
                        cx.emit(IrixFocusEvent::OpenFile {
                            session: sid.clone(),
                            path: path.clone(),
                        });
                    }
                },
            )));

        let content: AnyElement = match self.side {
            Side::Chat => div().into_any_element(),
            Side::Diff => {
                let mut col = div().size_full().flex().flex_col().gap(px(10.));
                match current {
                    Some(f) => {
                        col = col.child(
                            div()
                                .flex_none()
                                .truncate()
                                .font_family(tk().mono)
                                .text_size(ui::ts(S_LABEL))
                                .text_color(rgb(TEXT))
                                .child(format!(
                                    "{} · +{} −{} · file {} of {}",
                                    f.path,
                                    f.added,
                                    f.removed,
                                    files
                                        .iter()
                                        .position(|x| x.path == f.path)
                                        .map_or(1, |i| i + 1),
                                    files.len()
                                )),
                        );
                        let mut body = div().p(px(10.)).flex().flex_col();
                        for l in f.lines.iter().take(DIFF_LINES) {
                            let (mark, c) = match l.kind {
                                DiffLineKind::Added => ('+', 0x6FD36F),
                                DiffLineKind::Removed => ('−', 0xFF7B72),
                                DiffLineKind::Hunk => ('@', 0x8AA4D6),
                                DiffLineKind::Context => (' ', SCREEN_TEXT),
                            };
                            let text = if l.kind == DiffLineKind::Hunk {
                                l.text.clone()
                            } else {
                                format!("{mark} {}", l.text.trim_end())
                            };
                            body = body.child(
                                div()
                                    .flex_none()
                                    .truncate()
                                    .font_family(tk().mono)
                                    .text_size(ui::ts(S_LABEL))
                                    .line_height(relative(1.6))
                                    .text_color(rgb(c))
                                    .child(text),
                            );
                        }
                        col = col.child(
                            screen(div().size_full().child(scroll_area(
                                "if-diff",
                                &self.diff_scroll,
                                false,
                                body,
                            )))
                            .flex_1()
                            .min_h_0(),
                        );
                    }
                    None => col = col.child(note("No change yet.")),
                }
                col.into_any_element()
            }
            Side::Files => {
                let mut rows: Vec<AnyElement> = Vec::new();
                for (k, f) in files.iter().enumerate() {
                    let path = f.path.clone();
                    let (name, dir) = match f.path.rsplit_once('/') {
                        Some((d, n)) => (n.to_string(), d.to_string()),
                        None => (f.path.clone(), String::new()),
                    };
                    rows.push(
                        list_row(
                            format!("if-f-{k}"),
                            None,
                            &name,
                            &dir,
                            &format!("+{} −{}", f.added, f.removed),
                            current.is_some_and(|c| c.path == f.path),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.file = Some(path.clone());
                            this.side = Side::Diff;
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                }
                if rows.is_empty() {
                    rows.push(
                        div()
                            .p(px(16.))
                            .child(note("No change yet."))
                            .into_any_element(),
                    );
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(note(format!(
                        "{} files changed · +{add} −{del}",
                        files.len()
                    )))
                    .child(
                        sunken(div().size_full().child(scroll_area(
                            "if-files",
                            &self.files_scroll,
                            false,
                            div().flex().flex_col().children(rows),
                        )))
                        .flex_1()
                        .min_h_0(),
                    )
                    .into_any_element()
            }
            Side::Console => screen(div().size_full().child(self.console.clone()))
                .size_full()
                .into_any_element(),
            Side::Tests => {
                let counts = tests_of(&p.tail);
                let label = match counts {
                    Some((ok, bad)) => format!("{ok} of {} pass", ok + bad),
                    None => "No test run yet.".to_string(),
                };
                let mut col = div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(note(label));
                if let Some((ok, bad)) = counts {
                    let total = (ok + bad).max(1);
                    let lit = (ok as usize * 12 / total as usize).min(12);
                    let mut blocks = div().flex_none().flex().gap(px(4.)).h(px(20.));
                    for k in 0..12 {
                        blocks = blocks.child(div().flex_1().h_full().bg(rgb(if k < lit {
                            LED_GREEN
                        } else if bad > 0 {
                            ALERT
                        } else {
                            LED_OFF
                        })));
                    }
                    col = col.child(sunken(blocks.p(px(3.)).h(px(26.))));
                    for l in test_lines(&p.tail, 6) {
                        col = col.child(
                            div()
                                .flex_none()
                                .truncate()
                                .font_family(tk().mono)
                                .text_size(ui::ts(S_LABEL))
                                .line_height(relative(1.6))
                                .text_color(rgb(MUTED))
                                .child(l),
                        );
                    }
                }
                col.into_any_element()
            }
        };

        let right = div()
            .when(narrow, |d| d.flex_1().min_w_0())
            .when(!narrow, |d| {
                d.w(px(
                    (vw * 0.4).clamp(ui::sw(SIDE_W * 0.7).into(), ui::sw(760.).into())
                ))
                .flex_none()
            })
            .h_full()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section(&format!("Changes · {} files", files.len())))
            .child(tabs)
            .child(div().flex_1().min_h_0().flex().flex_col().child(content))
            .child(section("Activity"))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(32.))
                    .child(kv("Context", format!("{}%", p.context_pct), TEXT))
                    .child(kv("Tokens", p.tokens.to_string(), TEXT))
                    .child(kv("Spend", format!("${:.2}", p.cost), TEXT)),
            )
            .child(strip_chart(
                &self.tokens.list(),
                0x3CE05A,
                64.,
                1.,
                "tokens per s",
            ));

        let body = div().flex_1().min_h_0().flex().gap(px(32.));
        let body = if !narrow {
            body.child(left).child(right)
        } else if self.side == Side::Chat {
            // The tabs stay above the chat, so the owner can switch to the details.
            body.flex_col().child(chat_tabs.flex_none()).child(left)
        } else {
            body.child(right)
        };
        page().child(head).child(keys).child(body)
    }
}
