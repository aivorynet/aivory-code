//! The Neon session screen: one session with its chat and its code, in the grammar of the look.
//! The screen title and a tab menu of the live sessions (waits first) on top, the chat in the
//! middle (display lines, the decision, the composer with its controls), the changes on the
//! right (diff, console, files, tests). The decision is the one bright panel; the others dim
//! and light up on hover. Glow sits on frames and the title bar, never on text. Every control
//! goes through `act`.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::{DiffLineKind, SessionDiff};
use aivory_protocol::{AskKind, Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FontWeight, SharedString, Subscription, Window,
    div, px, relative, rgb, rgba,
};
use gpui_base::input::{InputEvent, InputState};

use super::neon_kit::*;
use crate::actions::{Action, act};
use crate::compose::{self, Tools};
use crate::compose_controls::{ControlStyle, Controls};
use crate::media::{self, MediaStyle};
use crate::model::Model;
use crate::session_console::{ConsoleEvent, ConsoleLook, SessionConsole, take_want_code};
use crate::theme::{PaneTheme, tk};
use crate::ui;

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
    /// Show another session.
    Open(String),
    /// Open a file of the session in the code lens.
    OpenFile {
        session: String,
        path: String,
    },
}

impl EventEmitter<NightFocusEvent> for NightFocus {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Chat,
    Diff,
    Console,
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
    controls: Controls,
    /// The console output of the session: a calm tab next to the diff.
    console: Entity<SessionConsole>,
    _subs: Vec<Subscription>,
}

/// The console colors of the Neon look (`0xRRGGBBAA`): quiet ink on the panel.
fn console_theme() -> PaneTheme {
    let c = |rgb: u32| (rgb << 8) | 0xFF;
    PaneTheme {
        bg: c(0x0A0E22),
        text: c(INK),
        dim: c(MIST),
        line: c(DIM),
        hover: c(PANEL),
        user_bg: c(PANEL),
        add: c(GREEN),
        add_bg: 0x0A0E22FF,
        del: c(RED),
        del_bg: 0x0A0E22FF,
        sub: c(MIST),
        tool: c(MIST),
        accent: c(CYAN),
        link: c(CYAN),
        code_bg: c(PANEL),
        top: c(0x0A0E22),
        ..PaneTheme::NAVY
    }
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
                size: 12.,
                pad_x: 14.,
                max_w: None,
            })
        });
        let open_file = cx.subscribe(&console, |_, _, e: &ConsoleEvent, cx| {
            let ConsoleEvent::OpenFile { session, path } = e;
            cx.emit(NightFocusEvent::OpenFile {
                session: session.clone(),
                path: path.clone(),
            });
        });
        // The diff is the first tab; the flag from Tasks ("Open diff") is used up here.
        take_want_code(&id);
        Self {
            model,
            id,
            input,
            diff: None,
            busy: false,
            side: Side::Chat,
            file: None,
            tools: Tools::default(),
            controls: Controls::default(),
            console,
            _subs: vec![observe, events, open_file],
        }
    }

    /// Show another session: the data resets.
    pub(super) fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.diff = None;
            self.file = None;
            self.controls = Controls::default();
            self.console.update(cx, |c, cx| c.show(id, cx));
            cx.notify();
        }
        if take_want_code(id) {
            self.side = Side::Diff;
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
        let prefs = self.controls.current(&pane);
        let action = match &pane.ask {
            Some(ask) => Action::Answer {
                id: pane.id.clone(),
                ask: ask.id.clone(),
                option: None,
                text: Some(text),
            },
            _ => {
                let images = self.tools.images();
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
                "Session",
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
        let k = f32::from(ui::sw(1.));
        let narrow = vw < 1100.;
        let mut room = vw - 96. - 90. * k;
        let mut shown = 0;
        for (_, title, _) in &live {
            let w = (title.chars().count() as f32 * 9.6 + 40.) * k;
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
                    .min_h(ui::sw(40.))
                    .px(px(20.))
                    .flex()
                    .items_center()
                    .child(label(format!("+{}", live.len() - shown), MIST))
                    .into_any_element(),
            );
        }
        let (pause, stop) = (p.id.clone(), p.id.clone());
        let actions = vec![
            button("nfo-back", "◂ War room", CYAN, false)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NightFocusEvent::Back)))
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
            .min_h(px(96.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(PAD))
            .pb(px(PAD));
        // A media line breaks the stream: the rows around it, and a preview between them.
        let media_style = MediaStyle {
            card: rgba(0x8C96BC22),
            text: rgb(INK),
            muted: rgb(MIST),
            radius: 0.,
        };
        // Each part is a run of lines and the media line that follows it.
        type Part<'a> = (
            Vec<&'a aivory_protocol::Line>,
            Option<&'a aivory_protocol::Line>,
        );
        let mut parts: Vec<Part> = Vec::new();
        let mut run = Vec::new();
        for l in &p.tail {
            if l.media.is_some() {
                parts.push((std::mem::take(&mut run), Some(l)));
            } else {
                run.push(l);
            }
        }
        parts.push((run, None));
        let mut budget = fit;
        let mut blocks: Vec<AnyElement> = Vec::new();
        for (at, (run, shot)) in parts.into_iter().enumerate().rev() {
            if budget == 0 {
                break;
            }
            if let Some(m) = shot.and_then(|l| l.media.as_ref()) {
                blocks.push(
                    div()
                        .flex_none()
                        .py(px(4.))
                        .child(media::preview(m, ("nfo-media", at), media_style))
                        .into_any_element(),
                );
                budget = budget.saturating_sub(4);
            }
            let owned: Vec<aivory_protocol::Line> = run.into_iter().cloned().collect();
            let rows = stream_rows(&owned, budget);
            budget = budget.saturating_sub(rows.len().max(1));
            for r in rows.into_iter().rev() {
                blocks.push(r.into_any_element());
            }
        }
        blocks.reverse();
        chat = chat.children(blocks);
        let waits = p.ask.is_some();
        let mut center = frame(if waits { YELLOW } else { CYAN }, "nfo-center", waits)
            .flex_1()
            .min_w(px(if narrow { 0. } else { 380. }))
            .min_h(px(0.))
            .overflow_hidden()
            .child(section(
                "Chat",
                CYAN,
                Some(format!("{} · {}", tag_word(&p), p.context_pct) + "% CTX"),
            ))
            .child(chat);
        if let Some(ask) = &p.ask {
            let mut card = div()
                .id("nfo-ask")
                .flex_none()
                .max_h(px((vh * 0.22).clamp(96., 300.)))
                .overflow_y_scroll()
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
        self.controls.sync(&p);
        let chip = |id: SharedString, text: String, on: bool| {
            div()
                .id(id)
                .min_h(ui::sw(28.))
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
        let providers = self.model.read(cx).providers.clone();
        let style = ControlStyle::console(PANEL, INK, YELLOW, Some(CHAKRA.into()));
        chips = chips.child(self.controls.row(
            "nfo-ctl",
            &p,
            &providers,
            &style,
            |this: &mut Self| &mut this.controls,
            cx,
        ));
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
                        .min_h(ui::sw(48.))
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
            Side::Diff | Side::Chat => match &current {
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
            Side::Console => {}
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
            .px(px(if narrow { 0. } else { PAD }))
            .flex()
            .flex_wrap()
            .border_b_1()
            .border_color(rgba(0x8C96BC30));
        for (s, name, id) in [
            (Side::Chat, "Chat", "nfo-s-chat"),
            (Side::Diff, "Diff", "nfo-s-diff"),
            (Side::Console, "Console", "nfo-s-console"),
            (Side::Files, "Files", "nfo-s-files"),
            (Side::Tests, "Tests", "nfo-s-tests"),
        ] {
            if s == Side::Chat && !narrow {
                continue;
            }
            let on = self.side == s || (!narrow && s == Side::Diff && self.side == Side::Chat);
            tabs = tabs.child(menu_tab(id, name, None, CYAN, on).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.side = s;
                    cx.notify();
                },
            )));
        }
        let (sid, open_path) = (p.id.clone(), current.as_ref().map(|f| f.path.clone()));
        tabs = tabs.child(div().flex_1().min_w(px(0.))).child(
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
        let (tabs_in, tabs_out) = if narrow {
            (None, Some(tabs))
        } else {
            (Some(tabs), None)
        };
        let right_w = ((vw - 64.) * 0.4).clamp(320., 720.);
        let right = frame(CYAN, "nfo-changes", false)
            .when(narrow, |d| d.flex_1().min_w(px(0.)))
            .when(!narrow, |d| d.w(px(right_w)).flex_none())
            .min_h(px(0.))
            .overflow_hidden()
            .child(section(
                "Changes",
                CYAN,
                Some(format!("{} FILES · +{add} −{del}", files.len())),
            ))
            .children(tabs_in)
            .child(div().h(px(12.)))
            .child(if self.side == Side::Console {
                div()
                    .id("nfo-console")
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.console.clone())
                    .into_any_element()
            } else {
                body.into_any_element()
            });

        // A file dropped anywhere on the chat attaches; Ctrl+V pastes an image.
        let center = compose::media_target(center, |this: &mut Self| Some(&mut this.tools), cx);

        screen(
            "Session",
            sub,
            state_color,
            (menu, actions),
            if narrow {
                // One column: Chat, Diff, Console, Files and Tests are tabs.
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .children(tabs_out)
                    .child(if self.side == Side::Chat {
                        center.into_any_element()
                    } else {
                        right.into_any_element()
                    })
                    .font_weight(FontWeight::NORMAL)
                    .into_any_element()
            } else {
                div()
                    .size_full()
                    .flex()
                    .gap(px(GAP))
                    .child(center)
                    .child(right)
                    .font_weight(FontWeight::NORMAL)
                    .into_any_element()
            },
        )
    }
}

/// The state word of a session for the chat note.
fn tag_word(p: &Pane) -> &'static str {
    state_of(p).0
}

/// The word and the color of the state of a session.
pub(super) fn state_of(p: &Pane) -> (&'static str, u32) {
    match (&p.ask, p.state) {
        (_, SessionState::Error) => ("FAULT", YELLOW),
        (Some(_), _) => ("WAITS FOR YOU", RED),
        (None, SessionState::Idle) => ("IDLE", MIST),
        (None, _) => ("WORKING", CYAN),
    }
}

/// The words of an answer button: the three permission answers carry their key.
pub(super) fn answer_label(kind: AskKind, option: &str, at: usize) -> String {
    match (kind, option) {
        (AskKind::Permission, "Allow") => "1 Allow".to_string(),
        (AskKind::Permission, "Always allow") => "2 Always".to_string(),
        (AskKind::Permission, "Deny") => "3 Deny".to_string(),
        _ => format!("{} {option}", at + 1),
    }
}

/// `12k` for 12,345.
pub(super) fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.),
    }
}

/// A bar on a dark track; `frac` is 0 to 1.
pub(super) fn bar(frac: f32, color: u32, h: f32) -> Div {
    div().w_full().h(px(h)).bg(rgba(0x1A2142CC)).child(
        div()
            .h_full()
            .w(relative(frac.clamp(0., 1.)))
            .bg(rgb(color)),
    )
}

/// A small filled tag with dark text.
pub(super) fn tag(text: &str, color: u32) -> Div {
    div()
        .px(px(10.))
        .py(px(3.))
        .bg(rgb(color))
        .text_color(rgb(DARK))
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(T_LABEL))
        .child(text.to_string())
}

/// The transcript as display lines (`console::display`): one speaker label per message, no
/// raw markdown marker, code and table rows in the mono face. Top-aligned: the list starts at
/// a message. Shared with the Focus view.
pub(super) fn stream_rows(tail: &[aivory_protocol::Line], max: usize) -> Vec<Div> {
    use crate::console::display::{DKind, last};
    let mono = |color: u32| {
        div()
            .font_family(tk().mono)
            .text_size(ui::ts(T_LABEL + 1.))
            .line_height(ui::ts(20.))
            .text_color(rgb(color))
    };
    let mut out = Vec::new();
    for d in last(tail, max) {
        let who = match d.kind {
            DKind::You => Some(("YOU", CYAN)),
            DKind::Agent | DKind::Heading | DKind::Bullet | DKind::Code | DKind::Table => {
                Some(("AGENT", YELLOW))
            }
            DKind::Error => Some(("ERROR", RED)),
            _ => None,
        };
        let mut cell = div().flex_none().flex().flex_col().gap(px(4.));
        if d.first {
            cell = cell.mt(px(14.));
            if let Some((name, color)) = who {
                cell = cell.child(label(name, color).font_weight(FontWeight::BOLD));
            }
        }
        let text = d.text.clone();
        cell = cell.child(match d.kind {
            DKind::You => div()
                .px(px(16.))
                .py(px(8.))
                .bg(rgba(0x00E0F01A))
                .border_l_2()
                .border_color(rgb(CYAN))
                .text_size(ui::ts(T_ENTRY))
                .line_height(ui::ts(21.))
                .text_color(rgb(0xFFFFFF))
                .child(text),
            DKind::Agent => entry(text),
            DKind::Heading => entry(text)
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(YELLOW)),
            DKind::Bullet => entry(text).pl(px(12.)),
            DKind::Code => mono(PALE)
                .px(px(16.))
                .bg(rgba(0x00000099))
                .border_l_2()
                .border_color(rgba(0xF3E60088))
                .child(text),
            DKind::Table => mono(PALE).px(px(16.)).bg(rgba(0x0A0E22FF)).child(text),
            DKind::Tool => mono(MIST)
                .line_clamp(1)
                .child(format!("● {}  {text}", d.label.as_deref().unwrap_or(""))),
            DKind::Output => mono(DIM).line_clamp(2).child(format!("  └ {text}")),
            DKind::Error => mono(YELLOW).child(text),
            DKind::Meta => div()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(DIM))
                .child(text),
        });
        out.push(cell);
    }
    out
}

/// The test result lines of a transcript (`ok. 12 passed; 0 failed`), the last few.
pub(super) fn test_lines(tail: &[aivory_protocol::Line]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = tail
        .iter()
        .filter(|l| {
            let t = l.text.to_lowercase();
            ((t.contains("passed") || t.contains("failed")) && t.contains("test"))
                || t.contains(" passed;")
        })
        .map(|l| {
            let t = l.text.lines().next().unwrap_or("").to_string();
            let bad = !t.contains(" 0 failed") && t.to_lowercase().contains("failed");
            (t, bad)
        })
        .collect();
    let skip = out.len().saturating_sub(2);
    out.drain(..skip);
    out
}
