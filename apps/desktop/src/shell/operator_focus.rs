//! The Operator Focus screen: one session as an operator console. A strip of session tabs on
//! top, the chat of the session as a green console in the middle (agent text parsed to display
//! lines, no raw markdown), the composer as an operator prompt at its foot, and blue screens on
//! the right with the facts, the controls and the changes of the session. Every control goes
//! through `actions.rs`, as in the other Focus screens.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffLineKind, SessionDiff};
use aivory_protocol::{ComposerPrefs, Effort, Mode, Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, ScrollHandle, Subscription, Window, div, px,
    relative, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use super::operator_screens::{btn, head_line, page};
use crate::actions::{Action, act};
use crate::compose::{self, Tools};
use crate::model::Model;
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
    /// The session view of the session.
    Detail(String),
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
    prefs: Option<ComposerPrefs>,
    _subs: Vec<Subscription>,
}

impl OperatorFocus {
    pub fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("tell the agent what to do"));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.send(window, cx);
            }
        });
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
            prefs: None,
            _subs: vec![observe, events],
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
            self.prefs = None;
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
        let composer = self.prefs.take().filter(|p| *p != pane.composer);
        self.tools.sent();
        let action = match &pane.ask {
            Some(ask) => Action::Answer {
                id: pane.id.clone(),
                ask: ask.id.clone(),
                option: None,
                text: Some(text),
            },
            _ => Action::Send {
                id: pane.id.clone(),
                text,
                composer,
            },
        };
        act(&self.model, action, cx);
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = self.pane(cx) else {
            return page(
                head_line("F1", "Focus", Vec::new()),
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
        let mut head = head_line(
            "F1",
            "Focus",
            vec![
                ("session".into(), name(&p)),
                ("agent".into(), p.provider.clone()),
                ("state".into(), state_word.into()),
            ],
        )
        .gap(px(12.));
        let (sid, did) = (p.id.clone(), p.id.clone());
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
            .child(btn("mf-deep".into(), "Deep dive ›", false).on_click(
                cx.listener(move |_, _, _, cx| cx.emit(OperatorFocusEvent::Detail(did.clone()))),
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
            .h(px(40.))
            .mb(px(14.))
            .flex()
            .items_center()
            .gap(px(4.))
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(0x0E3A1C));
        // Whole names only: the tabs that fit, the open session always, and a "+N" tab.
        let mut budget = 880.0_f32;
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
                    .h(px(36.))
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
        let transcript = div()
            .id("mf-chat")
            .flex_1()
            .min_h(px(0.))
            .px(px(PAD))
            .pt(px(PAD - 6.))
            .pb(px(10.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .text_size(ui::ts(T_ENTRY))
            .line_height(relative(1.5))
            .child(div().flex_none().child(tokenize(&lines).into_text()));

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
                .flex_none()
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

        let prefs = self.prefs.clone().unwrap_or_else(|| p.composer.clone());
        let chip = |id: String, text: String, on: bool| btn(id, &text, on).h(px(30.));
        let mut chips = div()
            .flex_none()
            .mx(px(PAD - 8.))
            .mb(px(8.))
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(
                chip(
                    "mf-mode".into(),
                    format!("mode: {}", crate::warroom::mode_label(prefs.mode)),
                    false,
                )
                .on_click(cx.listener({
                    let base = prefs.clone();
                    move |this, _, _, cx| {
                        let mut next = base.clone();
                        next.mode = match base.mode {
                            Mode::Plan => Mode::Edit,
                            Mode::Edit => Mode::Auto,
                            Mode::Auto => Mode::Plan,
                        };
                        this.prefs = Some(next);
                        cx.notify();
                    }
                })),
            )
            .child(
                chip("mf-model".into(), format!("model: {}", prefs.model), false).on_click(
                    cx.listener({
                        let base = prefs.clone();
                        move |this, _, _, cx| {
                            let list = crate::warroom::provider_models(&base.provider);
                            if list.is_empty() {
                                return;
                            }
                            let at = list
                                .iter()
                                .position(|m| *m == base.model)
                                .map_or(0, |i| i + 1);
                            let mut next = base.clone();
                            next.model = list[at % list.len()].to_string();
                            this.prefs = Some(next);
                            cx.notify();
                        }
                    }),
                ),
            )
            .child(
                chip(
                    "mf-effort".into(),
                    format!("effort: {}", crate::warroom::effort_label(prefs.effort)),
                    false,
                )
                .on_click(cx.listener({
                    let base = prefs.clone();
                    move |this, _, _, cx| {
                        let mut next = base.clone();
                        next.effort = match base.effort {
                            Effort::Low => Effort::Medium,
                            Effort::Medium => Effort::High,
                            Effort::High => Effort::Max,
                            Effort::Max => Effort::Low,
                        };
                        this.prefs = Some(next);
                        cx.notify();
                    }
                })),
            )
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
        let composer = div()
            .flex_none()
            .h(px(52.))
            .mx(px(PAD - 8.))
            .mb(px(PAD - 8.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(hca(0x04170A, 240))
            .text_size(ui::ts(T_ENTRY))
            .text_color(rgb(GREEN_MID))
            .child("you>")
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_color(rgb(GREEN_HI))
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .text_size(ui::ts(T_LABEL))
                    .text_color(rgb(GREEN_DIM))
                    .child(if p.ask.is_some() && p.controlled {
                        "↵ answers the ask"
                    } else {
                        "↵ send"
                    }),
            );
        let center = bezel(GREEN_MID)
            .flex_1()
            .min_w(px(0.))
            .child(
                glass(false)
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .flex_col()
                            .child(transcript)
                            .children(ask_block)
                            .child(chips)
                            .child(composer),
                    )
                    .child(glaze(false)),
            )
            .child(foot(
                "1",
                GREEN,
                vec![
                    (format!("{} · {}", p.provider, p.codebase), GREEN_HI),
                    (format!("· {state_word} {mins} min"), FOOT),
                ],
                30.,
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
            .h(px(250.))
            .child(screen(true, facts_body))
            .child(foot("A", FOOT, vec![("facts".into(), FOOT)], 30.));

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
            .absolute()
            .inset_0()
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
                    .h(px(34.))
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
        let changes = bezel(BLUE_RING)
            .flex_1()
            .min_h(px(0.))
            .child(screen(true, changes))
            .child(foot(
                "B",
                FOOT,
                vec![("diff of the session".into(), FOOT)],
                30.,
            ));
        let right = div()
            .flex_none()
            .w(px(400.))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(facts)
            .child(changes)
            .into_any_element();

        // `page` puts the title line over one row; the strip joins the first column.
        let body: AnyElement = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(strip)
            .child(div().flex_1().min_h(px(0.)).flex().child(center))
            .into_any_element();
        page(head, vec![body, right])
    }
}
