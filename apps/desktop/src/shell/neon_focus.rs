//! The Neon Focus screen: one session with its code, in the grammar of the look. The
//! screen title and a tab menu of the live sessions on top, the chat in the middle (display
//! lines, the decision, the composer), the changes on the right (diff, files, tests). The
//! decision is the one bright panel; the others dim and light up on hover. Every control goes
//! through `act`.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffLineKind, SessionDiff};
use aivory_protocol::{AskKind, ComposerPrefs, Effort, Mode, Pane};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FontWeight, SharedString, Subscription, Window,
    div, px, rgb, rgba,
};
use gpui_base::input::{InputEvent, InputState};

use super::neon_kit::*;
use super::neon_session::{answer_label, bar, short_count, state_of, stream_rows, tag, test_lines};
use crate::actions::{Action, act};
use crate::compose::{self, Tools};
use crate::model::Model;
use crate::theme::tk;
use crate::ui;
use crate::warroom::{effort_label, mode_label, provider_models};

/// How old a diff gets before the view asks again.
const TTL: Duration = Duration::from_secs(8);
/// The diff shows this many lines at most.
const DIFF_LINES: usize = 120;
/// Session tabs in the menu.
const TABS: usize = 6;
const PAD: f32 = 24.;
const GAP: f32 = 32.;

#[derive(Debug, Clone)]
pub(super) enum NightFocusEvent {
    Back,
    /// Show another session in Focus.
    Open(String),
    /// The session view.
    Detail(String),
    /// Open a file of the session in the code lens.
    OpenFile {
        session: String,
        path: String,
    },
}

impl EventEmitter<NightFocusEvent> for NightFocus {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Diff,
    Files,
    Tests,
}

pub(super) struct NightFocus {
    model: Entity<Model>,
    id: String,
    input: Entity<InputState>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    side: Side,
    file: Option<String>,
    tools: Tools,
    prefs: Option<ComposerPrefs>,
    _subs: Vec<Subscription>,
}

impl NightFocus {
    pub(super) fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Reply, or tell the agent what to do")
        });
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
            side: Side::Diff,
            file: None,
            tools: Tools::default(),
            prefs: None,
            _subs: vec![observe, events],
        }
    }

    /// Show another session: the data resets.
    pub(super) fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.diff = None;
            self.file = None;
            self.prefs = None;
            cx.notify();
        }
    }

    pub(super) fn session(&self) -> &str {
        &self.id
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
        let text = self.tools.message(&typed, None);
        if text.trim().is_empty() {
            return;
        }
        let prefs = self.prefs.clone().unwrap_or_else(|| pane.composer.clone());
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
                composer: (prefs != pane.composer).then_some(prefs),
            },
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

/// A scrolling box that fills the rest of a panel.
fn scroll_box(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

impl Render for NightFocus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = self.pane(cx) else {
            return screen(
                "Focus",
                "Session gone".to_string(),
                YELLOW,
                (Vec::new(), Vec::new()),
                div().into_any_element(),
            );
        };
        self.ensure(cx);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let (state_word, state_color) = state_of(&p);

        // ---- the menu: the live sessions as tabs, waits first -------------------------------
        let live: Vec<(String, String, u32)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                let mut v: Vec<&Pane> = w.sessions.iter().filter(|s| s.state.is_live()).collect();
                v.sort_by_key(|s| (s.ask.is_none(), s.since));
                v.iter()
                    .map(|s| (s.id.clone(), s.title.clone(), state_of(s).1))
                    .collect()
            })
            .unwrap_or_default();
        // Full names only: as many tabs as the width holds, then a "+N" tab.
        let vw = f32::from(window.viewport_size().width);
        let mut room = vw - 96. - 90.;
        let mut shown = 0;
        for (_, title, _) in &live {
            let w = title.chars().count() as f32 * 9.6 + 40.;
            if room < w || shown >= TABS {
                break;
            }
            room -= w;
            shown += 1;
        }
        let shown = shown.max(1).min(live.len());
        let mut menu: Vec<AnyElement> = Vec::new();
        for (i, (id, title, color)) in live.iter().take(shown).enumerate() {
            let go = id.clone();
            menu.push(
                menu_tab(
                    SharedString::from(format!("nfo-tab-{i}")),
                    title,
                    None,
                    *color,
                    *id == self.id,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(NightFocusEvent::Open(go.clone()));
                }))
                .into_any_element(),
            );
        }
        if live.len() > shown {
            menu.push(
                div()
                    .h(px(40.))
                    .px(px(20.))
                    .flex()
                    .items_center()
                    .child(label(format!("+{}", live.len() - shown), MIST))
                    .into_any_element(),
            );
        }
        let (pause, stop, detail) = (p.id.clone(), p.id.clone(), p.id.clone());
        let actions = vec![
            button("nfo-back", "◂ War room", CYAN, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NightFocusEvent::Back)))
                .into_any_element(),
            button("nfo-detail", "Session ›", CYAN, false)
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(NightFocusEvent::Detail(detail.clone()));
                }))
                .into_any_element(),
            button("nfo-pause", "Pause", MIST, false)
                .on_click(cx.listener(move |this, _, _, cx| {
                    act(&this.model, Action::Pause(pause.clone()), cx);
                }))
                .into_any_element(),
            button("nfo-stop", "Stop", RED, false)
                .on_click(cx.listener(move |this, _, _, cx| {
                    act(&this.model, Action::Stop(stop.clone()), cx);
                }))
                .into_any_element(),
        ];
        let sub = format!(
            "{} · {} / {} · {} · ${:.2}",
            p.title, p.codebase, p.model, state_word, p.cost
        );

        // ---- the chat: display lines, the decision, the composer -----------------------------
        // As many display lines as fit, from a message on, top-aligned (no half-cut line).
        let vh = f32::from(window.viewport_size().height);
        let spare =
            vh - 56. - 190. - 60. - 80. - if p.ask.is_some() { 270. } else { 0. } - 90. - 40.;
        let fit = ((spare / 22.).floor() as usize).max(6);
        let mut chat = div()
            .id("nfo-chat")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(PAD))
            .pb(px(PAD));
        for r in stream_rows(&p.tail, fit) {
            chat = chat.child(r);
        }
        let waits = p.ask.is_some();
        let mut center = frame(if waits { YELLOW } else { CYAN }, "nfo-center", waits)
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .child(section(
                "Chat",
                CYAN,
                Some(format!("{} · {}", tag_word(&p), p.context_pct) + "% CTX"),
            ))
            .child(chat);
        if let Some(ask) = &p.ask {
            let mut card = div()
                .flex_none()
                .mx(px(PAD))
                .mb(px(PAD))
                .pt(px(16.))
                .border_t_1()
                .border_color(rgba(0xF3E60066))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(tag(
                            match ask.kind {
                                AskKind::Permission => "PERMISSION",
                                AskKind::Plan => "PLAN",
                                _ => "QUESTION",
                            },
                            RED,
                        ))
                        .child(label(format!("WAITS {} MIN", p.minutes(now)), RED)),
                );
            card = card.child(if ask.kind == AskKind::Permission {
                code(&ask.text, YELLOW).line_clamp(3)
            } else {
                entry(ask.text.clone()).line_clamp(4)
            });
            let mut buttons = div().flex().flex_wrap().gap(px(12.));
            if p.controlled {
                for (i, option) in ask.options.iter().enumerate() {
                    let (session, ask_id, pick) = (p.id.clone(), ask.id.clone(), option.clone());
                    let text = answer_label(ask.kind, option, i);
                    let color = if i == 0 {
                        YELLOW
                    } else if text.to_lowercase().contains("deny") {
                        RED
                    } else {
                        MIST
                    };
                    buttons =
                        buttons.child(button(format!("nfo-a-{i}"), &text, color, i == 0).on_click(
                            cx.listener(move |this, _, _, cx| {
                                act(
                                    &this.model,
                                    Action::Answer {
                                        id: session.clone(),
                                        ask: ask_id.clone(),
                                        option: Some(pick.clone()),
                                        text: None,
                                    },
                                    cx,
                                );
                            }),
                        ));
                }
            } else {
                buttons = buttons.child(label("Answer in its terminal", MIST));
            }
            center = center.child(card.child(buttons));
        }
        let prefs = self.prefs.clone().unwrap_or_else(|| p.composer.clone());
        let chip = |id: SharedString, text: String, on: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .cursor_pointer()
                .border_1()
                .border_color(rgba(if on { 0xF3E600FF } else { 0x8C96BC55 }))
                .text_size(ui::ts(T_LABEL))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(if on { YELLOW } else { MIST }))
                .hover(|h| h.text_color(rgb(PALE)))
                .child(text.to_uppercase())
        };
        let mut chips = div().flex().flex_wrap().items_center().gap(px(8.));
        for m in [Mode::Plan, Mode::Edit, Mode::Auto] {
            let base = prefs.clone();
            chips = chips.child(
                chip(
                    SharedString::from(format!("nfo-mode-{}", mode_label(m))),
                    mode_label(m).to_string(),
                    prefs.mode == m,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.prefs = Some(ComposerPrefs {
                        mode: m,
                        ..base.clone()
                    });
                    cx.notify();
                })),
            );
        }
        // The model and the effort step to the next value on a click.
        let models = provider_models(&p.provider);
        let next_model = models
            .iter()
            .position(|m| *m == prefs.model)
            .map_or(models.first(), |i| models.get((i + 1) % models.len()))
            .map(|m| m.to_string());
        let base = prefs.clone();
        chips = chips.child(
            chip("nfo-model".into(), prefs.model.clone(), false).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(model) = &next_model {
                        this.prefs = Some(ComposerPrefs {
                            model: model.clone(),
                            ..base.clone()
                        });
                        cx.notify();
                    }
                },
            )),
        );
        let next_effort = match prefs.effort {
            Effort::Low => Effort::Medium,
            Effort::Medium => Effort::High,
            Effort::High => Effort::Max,
            Effort::Max => Effort::Low,
        };
        let base = prefs.clone();
        chips = chips.child(
            chip(
                "nfo-effort".into(),
                effort_label(prefs.effort).to_string(),
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.prefs = Some(ComposerPrefs {
                    effort: next_effort,
                    ..base.clone()
                });
                cx.notify();
            })),
        );
        chips = chips.child(
            chip("nfo-attach".into(), "Attach".to_string(), false).on_click(cx.listener(
                |_, _, window, cx| {
                    compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx)
                },
            )),
        );
        for (i, a) in self.tools.attachments.iter().enumerate() {
            chips = chips.child(
                chip(
                    SharedString::from(format!("nfo-att-{i}")),
                    format!("{} ×", a.name),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tools.remove(i);
                    cx.notify();
                })),
            );
        }
        center = center.child(
            div()
                .flex_none()
                .mx(px(PAD))
                .mb(px(PAD))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(chips)
                .child(
                    div()
                        .h(px(48.))
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .child(div().text_color(rgb(CYAN)).child("›"))
                        .child(div().flex_1().min_w(px(0.)).child(self.input.clone()))
                        .child(
                            button("nfo-send", "Send", YELLOW, true)
                                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                        ),
                ),
        );

        // ---- the changes: diff, files, tests ------------------------------------------------
        let files = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let current = self
            .file
            .as_ref()
            .and_then(|path| files.iter().find(|f| &f.path == path))
            .or(files.first())
            .cloned();
        let (add, del) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        let mut body = scroll_box("nfo-side").px(px(PAD)).pb(px(PAD)).gap(px(4.));
        match self.side {
            Side::Diff => match &current {
                Some(f) => {
                    body = body.child(div().pb(px(8.)).child(label(
                        format!("{}  +{} −{}", f.path, f.added, f.removed),
                        MIST,
                    )));
                    for l in f.lines.iter().take(DIFF_LINES) {
                        let (mark, c) = match l.kind {
                            DiffLineKind::Added => ('+', GREEN),
                            DiffLineKind::Removed => ('−', RED),
                            DiffLineKind::Hunk => ('@', DIM),
                            DiffLineKind::Context => (' ', MIST),
                        };
                        let text = if l.kind == DiffLineKind::Hunk {
                            l.text.clone()
                        } else {
                            format!("{mark} {}", l.text.trim_end())
                        };
                        body = body.child(
                            div()
                                .flex_none()
                                .overflow_hidden()
                                .font_family(tk().mono)
                                .text_size(ui::ts(T_LABEL + 1.))
                                .line_height(ui::ts(20.))
                                .text_color(rgb(c))
                                .child(text),
                        );
                    }
                }
                None => {
                    body = body.child(label("The agent has not changed a file yet.", MIST));
                }
            },
            Side::Files => {
                if files.is_empty() {
                    body = body.child(label("No change yet", MIST));
                }
                for (k, f) in files.iter().enumerate() {
                    let path = f.path.clone();
                    let name = f.path.rsplit(['/', '\\']).next().unwrap_or(&f.path);
                    body = body.child(
                        row(
                            format!("nfo-f-{k}"),
                            &format!("{:02}", k + 1),
                            CYAN,
                            (name, &f.path, &format!("+{} −{}", f.added, f.removed)),
                            current.as_ref().is_some_and(|c| c.path == f.path),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.file = Some(path.clone());
                            this.side = Side::Diff;
                            cx.notify();
                        })),
                    );
                }
            }
            Side::Tests => {
                let lines = test_lines(&p.tail);
                if lines.is_empty() {
                    body = body.child(label("No test run in the stream", MIST));
                }
                for (text, bad) in lines {
                    body = body.child(
                        div()
                            .font_family(tk().mono)
                            .text_size(ui::ts(T_LABEL + 1.))
                            .line_height(ui::ts(20.))
                            .text_color(rgb(if bad { RED } else { GREEN }))
                            .child(text),
                    );
                }
                body = body
                    .child(div().h(px(12.)))
                    .child(label(
                        format!(
                            "Context {}%  ·  {} tok",
                            p.context_pct,
                            short_count(p.tokens)
                        ),
                        MIST,
                    ))
                    .child(bar(
                        f32::from(p.context_pct) / 100.,
                        if p.context_pct >= 85 { RED } else { CYAN },
                        4.,
                    ));
            }
        }
        let mut tabs = div()
            .flex_none()
            .px(px(PAD))
            .flex()
            .border_b_1()
            .border_color(rgba(0x8C96BC30));
        for (s, name, id) in [
            (Side::Diff, "Diff", "nfo-s-diff"),
            (Side::Files, "Files", "nfo-s-files"),
            (Side::Tests, "Tests", "nfo-s-tests"),
        ] {
            tabs = tabs.child(menu_tab(id, name, None, CYAN, self.side == s).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.side = s;
                    cx.notify();
                }),
            ));
        }
        let (sid, open_path) = (p.id.clone(), current.as_ref().map(|f| f.path.clone()));
        tabs = tabs.child(div().flex_1()).child(
            menu_tab("nfo-lens", "Open in Lens ›", None, CYAN, false).on_click(cx.listener(
                move |_, _, _, cx| {
                    if let Some(path) = &open_path {
                        cx.emit(NightFocusEvent::OpenFile {
                            session: sid.clone(),
                            path: path.clone(),
                        });
                    }
                },
            )),
        );
        let right = frame(CYAN, "nfo-changes", false)
            .w(px(520.))
            .flex_none()
            .h_full()
            .child(section(
                "Changes",
                CYAN,
                Some(format!("{} FILES · +{add} −{del}", files.len())),
            ))
            .child(tabs)
            .child(div().h(px(12.)))
            .child(body);

        screen(
            "Focus",
            sub,
            state_color,
            (menu, actions),
            div()
                .size_full()
                .flex()
                .gap(px(GAP))
                .child(center)
                .child(right)
                .font_weight(FontWeight::NORMAL)
                .into_any_element(),
        )
    }
}

/// The state word of a session for the chat note.
fn tag_word(p: &Pane) -> &'static str {
    state_of(p).0
}
