//! The Operator Focus screen: one session as an operator console. A strip of session tabs on
//! top, the chat of the session as a green console in the middle (agent text parsed to display
//! lines, no raw markdown), the composer as an operator prompt at its foot, and blue screens on
//! the right with the facts, the controls and the changes of the session. Every control goes
//! through `actions.rs`, as in the other Focus screens.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffLineKind, SessionDiff};
use aivory_protocol::{Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, ScrollHandle, Subscription, Window, div, px,
    relative, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use super::operator_screens::{btn, head_line, page};
use crate::actions::{Action, act};
use crate::compose::{self, Tools};
use crate::compose_controls::{ControlStyle, Controls};
use crate::media::{self, MediaStyle};
use crate::model::Model;
use crate::session_console::{ConsoleEvent, ConsoleLook, SessionConsole, take_want_code};
use crate::theme::PaneTheme;
use crate::ui;
use crate::warroom::skins::operator::kit::*;

/// How old a diff gets before the view asks again.
const TTL: Duration = Duration::from_secs(8);
/// The inner padding of a screen.
const PAD: f32 = 24.;
/// The gap between the screens.
const GAP: f32 = 20.;
/// The soft red of an error.
const ERR: u32 = 0xFF8A80;

#[derive(Debug, Clone)]
pub enum OperatorFocusEvent {
    /// Show another session.
    Open(String),
    /// Open a file of the session in the code lens.
    OpenFile { session: String, path: String },
    /// Back to the war room.
    Back,
}

impl EventEmitter<OperatorFocusEvent> for OperatorFocus {}

pub struct OperatorFocus {
    model: Entity<Model>,
    id: String,
    input: Entity<InputState>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    /// The file whose diff shows. The first file when `None`.
    file: Option<String>,
    scroll: ScrollHandle,
    /// The session and line count that the chat was last scrolled to the end for.
    scrolled: String,
    /// The chips of the composer: attachments, and the mode, model and effort of the next
    /// message when the owner changed them.
    tools: Tools,
    controls: Controls,
    /// The console output of the session, shown next to the diff.
    console: Entity<SessionConsole>,
    tab: Tab,
    _subs: Vec<Subscription>,
}

/// The tab of the blue changes screen.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Chat,
    Diff,
    Console,
}

/// The console colors of the blue screen (`0xRRGGBBAA`).
fn console_theme() -> PaneTheme {
    let c = |rgb: u32| (rgb << 8) | 0xFF;
    PaneTheme {
        bg: c(BLUE_INK),
        text: c(BLUE_HI),
        dim: c(BLUE_SOFT),
        line: c(BLUE_DIM),
        hover: c(BLUE_DIM),
        user_bg: c(BLUE_DIM),
        add: c(BLUE_HI),
        add_bg: (BLUE_DIM << 8) | 0x99,
        del: c(BLUE_SOFT),
        del_bg: (BLUE_INK << 8) | 0x99,
        sub: c(BLUE_SOFT),
        tool: c(BLUE),
        accent: c(BLUE),
        link: c(BLUE),
        code_bg: c(BLUE_DIM),
        top: c(BLUE_INK),
        ..PaneTheme::NAVY
    }
}

impl OperatorFocus {
    /// The focus handle of the chat field (the shell keys J and K stay quiet while it has focus).
    pub fn input_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(&self.input, cx)
    }

    pub fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("tell the agent what to do"));
        let weak = cx.weak_entity();
        crate::complete::attach(
            &input,
            model.clone(),
            move |cx| weak.upgrade().map(|v| v.read(cx).id.clone()),
            |_| (None, None),
            cx,
        );
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) && !crate::complete::is_open(cx) {
                this.send(window, cx);
            }
        });
        let console = cx.new(|cx| SessionConsole::new(model.clone(), id.clone(), cx));
        console.update(cx, |c, _| {
            c.set_look(ConsoleLook {
                theme: Some(console_theme()),
                size: 11.,
                pad_x: 12.,
                max_w: None,
            })
        });
        let open_file = cx.subscribe(&console, |_, _, e: &ConsoleEvent, cx| {
            let ConsoleEvent::OpenFile { session, path } = e;
            cx.emit(OperatorFocusEvent::OpenFile {
                session: session.clone(),
                path: path.clone(),
            });
        });
        // The diff is the first tab; the flag from Tasks ("Open diff") is used up here.
        take_want_code(&id);
        let tab = Tab::Chat;
        Self {
            model,
            id,
            input,
            diff: None,
            busy: false,
            file: None,
            scroll: ScrollHandle::new(),
            scrolled: String::new(),
            tools: Tools::default(),
            controls: Controls::default(),
            console,
            tab,
            _subs: vec![observe, events, open_file],
        }
    }

    /// Show another session.
    pub fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.diff = None;
            self.file = None;
            self.scrolled.clear();
            self.tools = Tools::default();
            self.controls = Controls::default();
            self.console.update(cx, |c, cx| c.show(id, cx));
            cx.notify();
        }
        if take_want_code(id) {
            self.tab = Tab::Diff;
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
        let text = self.input.read(cx).value().trim().to_string();
        let Some(pane) = self.pane(cx) else {
            return;
        };
        if text.is_empty() && self.tools.attachments.is_empty() {
            return;
        }
        let text = self.tools.message(&text, None);
        let prefs = self.controls.current(&pane);
        let images = self.tools.images();
        self.tools.sent();
        let action = match &pane.ask {
            Some(ask) => Action::Answer {
                id: pane.id.clone(),
                ask: ask.id.clone(),
                option: None,
                text: Some(text),
            },
            _ => {
                crate::compose_controls::submit(&self.model, &pane, text, images, prefs, cx);
                self.input.update(cx, |s, cx| s.clean(window, cx));
                return;
            }
        };
        act(&self.model, action, cx);
        self.input.update(cx, |s, cx| s.clean(window, cx));
    }

    /// The mic: dictate into the prompt; a second press stops the listening.
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

    /// A button of the dictation status line (stop, download of the offline model, cancel).
    fn tool_action(
        &mut self,
        action: compose::ToolAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.input.clone();
        compose::handle_action(
            self,
            action,
            |this: &mut Self| Some(&mut this.tools),
            input,
            window,
            cx,
        );
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

/// A row of facts: a key and a value.
fn fact(k: &str, v: String) -> Div {
    div()
        .flex()
        .gap(px(16.))
        .child(
            div()
                .flex_1()
                .text_color(rgb(BLUE_SOFT))
                .child(k.to_string()),
        )
        .child(div().flex_none().text_color(rgb(BLUE_HI)).child(v))
}

/// A section title on a blue screen.
fn title(text: &str, color: u32) -> Div {
    div()
        .flex_none()
        .text_size(ui::ts(T_SECTION))
        .text_color(rgb(color))
        .child(text.to_uppercase())
}

impl Render for OperatorFocus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = self.pane(cx) else {
            return page(
                head_line("F1", "Session", Vec::new()),
                vec![
                    div()
                        .flex_1()
                        .text_color(rgb(GREEN_DIM))
                        .child("This session is gone. Esc goes back to the wall.")
                        .into_any_element(),
                ],
            );
        };
        self.ensure(cx);
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        // Under 1100 px the details move into the chat column as tabs.
        let narrow = vw < 1100.;
        let short = vh < 760.;
        let right_w = (vw * 0.38).clamp(320., 720.);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let mins = p.minutes(now);
        let state_word = match p.state {
            SessionState::Working => "working",
            SessionState::NeedsYou => "waits for you",
            SessionState::Error => "error",
            SessionState::Idle => "idle",
            SessionState::Parked => "parked",
            SessionState::Done => "done",
        };

        // The title line with the controls.
        let stats = if narrow {
            vec![("state".into(), state_word.into())]
        } else {
            vec![
                ("session".into(), name(&p)),
                ("agent".into(), p.provider.clone()),
                ("state".into(), state_word.into()),
            ]
        };
        let mut head = head_line("F1", "Session", stats)
            .h_auto()
            .min_h(px(68.))
            .py(px(6.))
            .flex_wrap()
            .gap(px(12.));
        let sid = p.id.clone();
        head = head
            .child(
                btn("mf-pause".into(), "Pause", false).on_click(cx.listener({
                    let sid = sid.clone();
                    move |this, _, _, cx| act(&this.model, Action::Pause(sid.clone()), cx)
                })),
            )
            .child(btn("mf-stop".into(), "Stop", false).on_click(
                cx.listener(move |this, _, _, cx| act(&this.model, Action::Stop(sid.clone()), cx)),
            ))
            .child(
                btn("mf-back".into(), "‹ War room", false)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OperatorFocusEvent::Back))),
            );

        // The strip of sessions: tabs with a state lamp.
        let sessions: Vec<(String, String, SessionState)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                w.sessions
                    .iter()
                    .filter(|s| s.state.is_live())
                    .map(|s| (s.id.clone(), name(s), s.state))
                    .collect()
            })
            .unwrap_or_default();
        let mut strip = div()
            .flex_none()
            .min_h(ui::sw(40.))
            .mb(px(14.))
            .flex()
            .items_center()
            .gap(px(4.))
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(0x0E3A1C));
        // Whole names only: the tabs that fit, the open session always, and a "+N" tab.
        let mut budget = (vw - if narrow { 80. } else { right_w + 110. }).max(240.);
        let mut shown = 0usize;
        for (k, (sid, nm, st)) in sessions.iter().enumerate() {
            let on = *sid == p.id;
            let width = nm.chars().count() as f32 * 7.8 + 60.;
            if !on && width > budget {
                continue;
            }
            budget -= width;
            shown += 1;
            let lamp = match st {
                SessionState::Working => GREEN,
                SessionState::NeedsYou => BLUE,
                SessionState::Error => ERR,
                _ => FOOT,
            };
            let go = sid.clone();
            strip = strip.child(
                div()
                    .id(eid(format!("mf-tab-{k}")))
                    .flex_none()
                    .min_h(ui::sw(36.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .text_size(ui::ts(T_ENTRY))
                    .when(on, |d| d.bg(hca(GREEN, 40)).text_color(rgb(GREEN_HI)))
                    .when(!on, |d| {
                        d.text_color(rgb(GREEN_MID))
                            .hover(|h| h.bg(hca(GREEN, 20)).text_color(rgb(GREEN_HI)))
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(OperatorFocusEvent::Open(go.clone()));
                    }))
                    .child(div().flex_none().size(px(8.)).bg(rgb(lamp)))
                    .child(nm.clone()),
            );
        }
        if shown < sessions.len() {
            strip = strip.child(
                div()
                    .flex_none()
                    .px(px(14.))
                    .text_size(ui::ts(T_ENTRY))
                    .text_color(rgb(FOOT))
                    .child(format!("+{}", sessions.len() - shown)),
            );
        }

        // The chat: a green console. The agent text is parsed to display lines.
        let lines = display_lines(&p, 80);
        let key = format!("{}:{}", p.id, p.lines_total);
        if self.scrolled != key {
            self.scroll.scroll_to_bottom();
            self.scrolled = key;
        }
        // The media of the newest lines: a thumbnail or a card under the console text.
        let media_style = MediaStyle {
            card: gpui::rgba(0x04170AF0),
            text: gpui::rgb(GREEN_HI),
            muted: gpui::rgb(GREEN_DIM),
            radius: 0.,
        };
        let mut shots: Vec<_> = p.tail.iter().filter_map(|l| l.media.as_ref()).collect();
        shots.drain(..shots.len().saturating_sub(4));
        let previews: Vec<_> = shots
            .into_iter()
            .enumerate()
            .map(|(n, m)| {
                div()
                    .flex_none()
                    .py(px(4.))
                    .child(media::preview(m, ("mf-media", n), media_style))
            })
            .collect();
        let transcript = div()
            .id("mf-chat")
            .flex_1()
            .min_h(px(96.))
            .px(px(PAD))
            .pt(px(PAD - 6.))
            .pb(px(10.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .text_size(ui::ts(T_ENTRY))
            .line_height(relative(1.5))
            .child(div().flex_none().child(tokenize(&lines).into_text()))
            .children(previews);

        let ask_block = p.ask.as_ref().map(|ask| {
            let mut buttons = div().flex().flex_wrap().gap(px(8.));
            if p.controlled {
                for (k, option) in ask.options.iter().enumerate() {
                    let (sid, aid, opt) = (p.id.clone(), ask.id.clone(), option.clone());
                    buttons = buttons.child(
                        btn(
                            format!("mf-answer-{k}"),
                            &format!("{}  {}", k + 1, option),
                            k == 0,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
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
                        })),
                    );
                }
            } else {
                buttons = buttons.child(
                    div()
                        .text_color(rgb(BLUE_SOFT))
                        .child("Answer it in its terminal."),
                );
            }
            div()
                .id("mf-ask")
                .flex_none()
                .max_h(px((vh * 0.22).clamp(96., 300.)))
                .overflow_y_scroll()
                .mx(px(PAD - 8.))
                .mb(px(10.))
                .px(px(16.))
                .py(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .bg(hca(BLUE_DIM, 235))
                .text_size(ui::ts(T_ENTRY))
                .line_height(relative(1.5))
                .text_color(rgb(BLUE_HI))
                .child(
                    div()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child("INCOMING · WAITS FOR YOU"),
                )
                .child(ask.text.clone())
                .child(buttons)
        });

        self.controls.sync(&p);
        let providers = self.model.read(cx).providers.clone();
        let style = ControlStyle::operator(Some(FONT.into()));
        // A key of the status line: plain dim text, inverse video when it is on, as in a
        // terminal. Same size and cell as the composer controls next to it.
        let chip = |id: String, text: String, on: bool| {
            let (bg, fg) = if on {
                (rgb(0x9BFFC0), rgb(0x020A04))
            } else {
                (gpui::rgba(0x00000000), rgb(0x6DB596))
            };
            div()
                .id(eid(id))
                .flex_none()
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .cursor_pointer()
                .bg(bg)
                .text_color(fg)
                .text_size(ui::ts(T_LABEL))
                .hover(|h| h.bg(rgb(0x0F2A1C)).text_color(rgb(0xB8FFCC)))
                .child(text)
        };
        let mut chips = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .text_size(ui::ts(T_LABEL))
            .child(self.controls.row(
                "mf-ctl",
                &p,
                &providers,
                &style,
                |this: &mut Self| &mut this.controls,
                cx,
            ))
            .child(
                chip("mf-attach".into(), "+ attach".to_string(), false).on_click(cx.listener(
                    |_, _, window, cx| {
                        compose::attach(
                            |this: &mut OperatorFocus| Some(&mut this.tools),
                            window,
                            cx,
                        );
                    },
                )),
            );
        let listening = self.tools.listening.is_some();
        chips = chips.child(
            chip(
                "mf-mic".into(),
                if listening { "● rec" } else { "mic" }.to_string(),
                listening,
            )
            .tooltip(ui::tip(if listening {
                "Stop the dictation"
            } else {
                "Dictate into the prompt"
            }))
            .on_click(cx.listener(|this, _, window, cx| this.dictate(window, cx))),
        );
        for (k, a) in self.tools.attachments.iter().enumerate() {
            let file = a
                .path
                .file_name()
                .map_or(String::new(), |n| n.to_string_lossy().to_string());
            chips = chips.child(
                chip(format!("mf-chip-{k}"), format!("{file} ×"), true).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.tools.remove(k);
                        cx.notify();
                    },
                )),
            );
        }
        // The prompt: one console line under a thin rule, like the shell of the session. No
        // field box; the input text is the bright green of the console.
        let composer = div()
            .flex_none()
            .min_h(ui::sw(30.))
            .pt(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_t_1()
            .border_color(hca(GREEN_DIM, 160))
            .text_size(ui::ts(T_ENTRY))
            .text_color(rgb(GREEN_MID))
            .child(
                div()
                    .flex_none()
                    .child(format!("{}@{}:~$", p.provider, p.codebase)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_color(rgb(GREEN_HI))
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(ui::ts(T_LABEL))
                    .text_color(rgb(GREEN_DIM))
                    .child(if p.ask.is_some() && p.controlled {
                        "↵ answer"
                    } else {
                        "↵"
                    }),
            );
        let column = div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(ui::with_thumb(
                transcript,
                &self.scroll,
                gpui::rgba(0x2BA85ACC),
                4.,
            ))
            .children(ask_block)
            .child(
                div()
                    .flex_none()
                    .mx(px(PAD - 8.))
                    .mb(px(PAD - 8.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(composer)
                    .child(chips)
                    .children(compose::status(
                        &self.tools,
                        "mf",
                        compose::Palette {
                            text: (GREEN_HI << 8) | 0xFF,
                            dim: (GREEN_DIM << 8) | 0xFF,
                            line: (GREEN_DIM << 8) | 0xFF,
                            chip_bg: 0x04170AFF,
                            hover: 0x125A2AFF,
                            fail: 0xF2C063FF,
                            ok: (GREEN_MID << 8) | 0xFF,
                        },
                        |action| {
                            Box::new(cx.listener(move |this, _, window, cx| {
                                this.tool_action(action, window, cx)
                            }))
                        },
                    )),
            );
        // A file dropped anywhere on the chat attaches; Ctrl+V pastes an image.
        let column =
            compose::media_target(column, |this: &mut OperatorFocus| Some(&mut this.tools), cx);
        let center = bezel(GREEN_MID)
            .flex_1()
            .min_w(px(0.))
            .child(glass(false).child(column).child(glaze(false)))
            .child(foot(
                "1",
                GREEN,
                vec![
                    (format!("{} · {}", p.provider, p.codebase), GREEN_HI),
                    (format!("· {state_word} {mins} min"), FOOT),
                ],
                f32::from(ui::sw(30.)),
            ))
            .into_any_element();

        // The blue column: facts, then the changes.
        let tokens = if p.tokens >= 1000 {
            format!("{}k", p.tokens / 1000)
        } else {
            p.tokens.to_string()
        };
        let facts_body = div()
            .absolute()
            .inset_0()
            .px(px(PAD))
            .py(px(PAD - 8.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .overflow_hidden()
            .text_size(ui::ts(T_ENTRY))
            .child(title("Facts", BLUE_HI))
            .child(fact("model", p.model.clone()))
            .child(fact("context", format!("{}%", p.context_pct)))
            .child(fact("tokens", tokens))
            .child(fact("cost", format!("${:.2}", p.cost)))
            .child(fact("in this state", format!("{mins} min")));
        let facts = bezel(BLUE_RING)
            .flex_none()
            .h(ui::sw(250.))
            .child(screen(true, facts_body))
            .child(foot(
                "A",
                FOOT,
                vec![("facts".into(), FOOT)],
                f32::from(ui::sw(30.)),
            ));

        let files: Vec<aivory_protocol::session::DiffFile> = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let cur = self
            .file
            .as_ref()
            .and_then(|f| files.iter().find(|x| &x.path == f))
            .or(files.first())
            .cloned();
        let mut changes = div()
            .id("mf-changes")
            .flex_1()
            .min_h(px(0.))
            .px(px(PAD))
            .py(px(PAD - 8.))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .text_size(ui::ts(T_ENTRY))
            .child(title(&format!("Changes · {}", files.len()), BLUE_HI).pb(px(10.)));
        if self.diff.is_none() {
            changes = changes.child(
                div()
                    .text_color(rgb(BLUE_SOFT))
                    .child("reading the changes…"),
            );
        } else if files.is_empty() {
            changes = changes.child(
                div()
                    .text_color(rgb(BLUE_SOFT))
                    .child("The agent has not changed a file yet."),
            );
        }
        for (k, f) in files.iter().take(8).enumerate() {
            let on = cur.as_ref().is_some_and(|c| c.path == f.path);
            let path = f.path.clone();
            changes = changes.child(
                div()
                    .id(eid(format!("mf-file-{k}")))
                    .flex_none()
                    .min_h(ui::sw(34.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(rgb(BLUE_DIM)))
                    .when(!on, |d| d.hover(|h| h.bg(hca(BLUE_DIM, 150))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.file = Some(path.clone());
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_color(rgb(if on { BLUE_HI } else { BLUE }))
                            .child(f.path.rsplit(['/', '\\']).next().unwrap_or("").to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui::ts(T_LABEL))
                            .text_color(rgb(BLUE_HI))
                            .child(format!("+{}", f.added)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui::ts(T_LABEL))
                            .text_color(rgb(BLUE_SOFT))
                            .child(format!("-{}", f.removed)),
                    ),
            );
        }
        if let Some(f) = &cur {
            let (session, path) = (p.id.clone(), f.path.clone());
            changes = changes
                .child(
                    div()
                        .flex_none()
                        .mt(px(12.))
                        .mb(px(6.))
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(f.path.to_uppercase()),
                )
                .child(div().flex_none().pb(px(8.)).child(
                    btn("mf-lens".into(), "Open in lens ›", false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            cx.emit(OperatorFocusEvent::OpenFile {
                                session: session.clone(),
                                path: path.clone(),
                            });
                        },
                    )),
                ));
            for l in f.lines.iter().take(40) {
                let (mark, color) = match l.kind {
                    DiffLineKind::Added => ("+ ", BLUE_HI),
                    DiffLineKind::Removed => ("- ", BLUE_SOFT),
                    DiffLineKind::Hunk => ("", FOOT),
                    DiffLineKind::Context => ("  ", BLUE_RING),
                };
                changes = changes.child(
                    div()
                        .flex_none()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(color))
                        .child(format!("{mark}{}", l.text)),
                );
            }
        }
        let tab_btn = |id: &str, text: &str, tab: Tab, on: bool, cx: &mut Context<Self>| {
            div()
                .id(eid(id.to_string()))
                .flex_none()
                .min_h(ui::sw(30.))
                .px(px(10.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(if on { BLUE_HI } else { BLUE_SOFT }))
                .when(on, |d| d.bg(rgb(BLUE_DIM)))
                .when(!on, |d| d.hover(|h| h.text_color(rgb(BLUE_HI))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }))
                .child(text.to_uppercase())
        };
        let tabs = div()
            .flex_none()
            .px(px(PAD - 8.))
            .pt(px(10.))
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .when(narrow, |d| {
                d.child(tab_btn(
                    "mf-tab-chat",
                    "Chat",
                    Tab::Chat,
                    self.tab == Tab::Chat,
                    cx,
                ))
            })
            .child(tab_btn(
                "mf-tab-diff",
                "Diff",
                Tab::Diff,
                self.tab == Tab::Diff || (!narrow && self.tab == Tab::Chat),
                cx,
            ))
            .child(tab_btn(
                "mf-tab-console",
                "Console",
                Tab::Console,
                self.tab == Tab::Console,
                cx,
            ));
        let (tabs_in, tabs_out) = if narrow {
            (None, Some(tabs.pb(px(10.))))
        } else {
            (Some(tabs), None)
        };
        let pane_body: AnyElement = match self.tab {
            Tab::Diff | Tab::Chat => changes.into_any_element(),
            Tab::Console => div()
                .id("mf-console")
                .flex_1()
                .min_h(px(0.))
                .child(self.console.clone())
                .into_any_element(),
        };
        let changes = bezel(BLUE_RING)
            .flex_1()
            .min_h(px(0.))
            .child(screen(
                true,
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .children(tabs_in)
                    .child(pane_body),
            ))
            .child(foot(
                "B",
                FOOT,
                vec![(
                    if self.tab != Tab::Console {
                        "diff of the session"
                    } else {
                        "console output"
                    }
                    .into(),
                    FOOT,
                )],
                f32::from(ui::sw(30.)),
            ));
        let right = div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .when(narrow, |d| d.flex_1().min_w(px(0.)))
            .when(!narrow, |d| d.flex_none().w(px(right_w)))
            .when(!short && !narrow, |d| d.child(facts))
            .child(changes)
            .into_any_element();

        // `page` puts the title line over one row; the strip joins the first column.
        if narrow {
            // One column: Chat, Diff and Console are tabs of the same column.
            let shown = if self.tab == Tab::Chat {
                div().flex_1().min_w(px(0.)).flex().child(center)
            } else {
                div().flex_1().min_w(px(0.)).flex().child(right)
            };
            let body: AnyElement = div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(strip)
                .children(tabs_out)
                .child(div().flex_1().min_h(px(0.)).flex().child(shown))
                .into_any_element();
            return page(head, vec![body]);
        }
        let body: AnyElement = div()
            .flex_1()
            .min_w(px(380.))
            .flex()
            .flex_col()
            .child(strip)
            .child(div().flex_1().min_h(px(0.)).flex().child(center))
            .into_any_element();
        page(head, vec![body, right])
    }
}
