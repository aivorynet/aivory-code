//! The Neon session screen in the language of `NeonGrid`: a title with a glow bar,
//! a dim switcher on the left (waits for you first), the chat stream in the middle with the
//! decision above the composer, and the files and the facts on the right. The decision is the
//! one bright panel; the others dim and light up while the mouse is over them. Glow sits on
//! frames and the title bar, never on text. The shell draws the header; Back goes to the war
//! room. The chat shows the tail of the transcript that the pane carries.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{SessionRef, method};
use aivory_protocol::session::SessionDiff;
use aivory_protocol::{AskKind, Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FontWeight, SharedString, Subscription, Window,
    div, px, relative, rgb, rgba,
};
use gpui_base::input::{InputEvent, InputState};

use super::neon_kit::*;
use crate::actions::{Action, act};
use crate::model::Model;
use crate::theme::tk;
use crate::ui;

/// How old a diff gets before the view asks again.
const TTL: Duration = Duration::from_secs(8);
/// The files that the panel lists.
const FILES: usize = 6;
/// The inner padding of a panel and the gap between panels.
const PAD: f32 = 24.;
const GAP: f32 = 32.;

#[derive(Debug, Clone)]
pub(super) enum NightSessionEvent {
    /// Back to the war room.
    Back,
    /// Show another session.
    Switch(String),
    /// Open a file of the session in the code lens.
    OpenFile { session: String, path: String },
}

impl EventEmitter<NightSessionEvent> for NightSession {}

pub(super) struct NightSession {
    model: Entity<Model>,
    id: String,
    input: Entity<InputState>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    busy: bool,
    _subs: Vec<Subscription>,
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

impl NightSession {
    pub(super) fn new(
        model: Entity<Model>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Tell the agent what to do"));
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
            _subs: vec![observe, events],
        }
    }

    /// Show another session: the data resets.
    pub(super) fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.diff = None;
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
        if text.is_empty() {
            return;
        }
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
                composer: None,
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

    /// The switcher: the live sessions, the ones that wait for you first. Dim until the
    /// mouse is over it.
    fn switcher(&self, now: i64, window: &Window, cx: &mut Context<Self>) -> Div {
        let list: Vec<(String, String, String, u32, String)> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                let mut v: Vec<&Pane> = w.sessions.iter().filter(|s| s.state.is_live()).collect();
                v.sort_by_key(|s| (s.ask.is_none(), s.since));
                v.iter()
                    .map(|s| {
                        let (_, color) = state_of(s);
                        let wait = if s.ask.is_some() {
                            format!("{}M", s.minutes(now))
                        } else {
                            String::new()
                        };
                        (
                            s.id.clone(),
                            s.title.clone(),
                            s.codebase.clone(),
                            color,
                            wait,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, (id, title, codebase, color, wait)) in list.iter().enumerate() {
            let go = id.clone();
            rows.push(
                row_tall(
                    SharedString::from(format!("nc-s-{id}")),
                    &format!("{:02}", i + 1),
                    *color,
                    (title, &codebase.to_uppercase(), wait),
                    *id == self.id,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(NightSessionEvent::Switch(go.clone()));
                }))
                .into_any_element(),
            );
        }
        // Whole rows only: 56 header, 32 padding, 54 panel head.
        let rows = cap_rows(rows, fit_rows(window, 56. + 64. + 54. + 16., TALL), TALL);
        frame(CYAN, "nc-switcher", false)
            .w(px(300.))
            .flex_none()
            .h_full()
            .child(section(
                "Sessions",
                CYAN,
                Some(format!("{} LIVE", list.len())),
            ))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
    }

    /// The decision of the session with its answers; they call the daemon through `act`.
    /// This is the one bright panel.
    fn decision(&self, p: &Pane, now: i64, cx: &mut Context<Self>) -> Option<Div> {
        let ask = p.ask.as_ref()?;
        let word = match ask.kind {
            AskKind::Permission => "PERMISSION",
            AskKind::Plan => "PLAN",
            _ => "QUESTION",
        };
        let mut card = frame(YELLOW, "nc-decision", true)
            .flex_none()
            .p(px(PAD))
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(tag(word, RED))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(ui::ts(T_LABEL))
                            .text_color(rgb(RED))
                            .child(format!("WAITS {} MIN", p.minutes(now))),
                    ),
            );
        card = card.child(if ask.kind == AskKind::Permission {
            code(&ask.text, YELLOW).line_clamp(3)
        } else {
            entry(ask.text.clone()).line_clamp(4)
        });
        for (i, step) in ask.steps.iter().take(3).enumerate() {
            card = card.child(
                div()
                    .truncate()
                    .child(label(format!("{}  {step}", i + 1), MIST)),
            );
        }
        let mut row = div().flex().flex_wrap().gap(px(12.));
        if p.controlled {
            for (i, option) in ask.options.iter().enumerate() {
                let (session, ask_id, pick) = (p.id.clone(), ask.id.clone(), option.clone());
                let text = answer_label(ask.kind, option, i);
                let deny = text.to_lowercase().contains("deny");
                let color = if i == 0 {
                    YELLOW
                } else if deny {
                    RED
                } else {
                    MIST
                };
                row = row.child(button(format!("nc-a-{i}"), &text, color, i == 0).on_click(
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
            row = row.child(label("Answer in its terminal", MIST));
        }
        Some(card.child(row))
    }
}

impl Render for NightSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(p) = self.pane(cx) else {
            return div()
                .size_full()
                .bg(rgb(BG))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MIST))
                .child("SESSION GONE")
                .into_any_element();
        };
        self.ensure(cx);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let (state_word, state_color) = state_of(&p);

        // ---- middle: the title, the chat, the decision, the composer ----------------------
        let id = p.id.clone();
        let title = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .font_family(ORBITRON)
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_size(ui::ts(T_SCREEN - 4.))
                    .line_height(ui::ts(36.))
                    .text_color(rgb(YELLOW))
                    .line_clamp(2)
                    .child(p.title.to_uppercase()),
            )
            .child(div().w(px(88.)).h(px(3.)).bg(rgb(state_color)).shadow(glow(
                state_color,
                0xC0,
                14.,
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(tag(state_word, state_color))
                    .child(div().flex_1().min_w(px(0.)).child(label(
                        format!("{} · {} / {}", p.provider, p.codebase, p.model),
                        MIST,
                    )))
                    .child(
                        button("nc-back", "◂ War room", CYAN, false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(NightSessionEvent::Back))),
                    ),
            );

        // As many display lines as fit, from a message on, top-aligned (no half-cut line).
        let vh = f32::from(window.viewport_size().height);
        let spare =
            vh - 56. - 64. - 150. - 70. - if p.ask.is_some() { 250. } else { 0. } - 64. - 96.;
        let fit = ((spare / 22.).floor() as usize).max(6);
        let mut chat = div()
            .id("nc-chat")
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
        let chat = frame(CYAN, "nc-chat-panel", false)
            .flex_1()
            .min_h(px(0.))
            .child(section(
                "Stream",
                CYAN,
                Some(format!("LAST {} OF {} LINES", p.tail.len(), p.lines_total)),
            ))
            .child(chat);

        // The composer is the bright panel while no decision waits.
        let composer = frame(CYAN, "nc-composer", p.ask.is_none())
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(16.))
            .px(px(PAD))
            .h(px(64.))
            .child(div().text_color(rgb(CYAN)).child("›"))
            .child(div().flex_1().min_w(px(0.)).child(self.input.clone()))
            .child(
                button("nc-send", "Send", YELLOW, true)
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            );

        let middle = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(title)
            .child(chat)
            .children(self.decision(&p, now, cx))
            .child(composer);

        // ---- right: files, then the facts ---------------------------------------------------
        let files = self.diff.as_ref().and_then(|(d, _)| d.as_ref());
        let most = files
            .map(|d| {
                d.files
                    .iter()
                    .map(|f| f.added + f.removed)
                    .max()
                    .unwrap_or(1)
            })
            .unwrap_or(1)
            .max(1) as f32;
        let mut file_panel = frame(CYAN, "nc-files", false)
            .flex_none()
            .pb(px(16.))
            .child(section(
                "Files",
                CYAN,
                files.map(|d| format!("{} CHANGED", d.files.len())),
            ));
        match files {
            None => {
                file_panel =
                    file_panel.child(div().px(px(PAD)).child(label("Reading the worktree", MIST)));
            }
            Some(d) if d.files.is_empty() => {
                file_panel =
                    file_panel.child(div().px(px(PAD)).child(label("No changes yet", MIST)));
            }
            Some(d) => {
                for (i, f) in d.files.iter().take(FILES).enumerate() {
                    let (session, path) = (id.clone(), f.path.clone());
                    let name = f
                        .path
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(&f.path)
                        .to_string();
                    let add = f.added as f32 / most;
                    let del = f.removed as f32 / most;
                    file_panel = file_panel.child(
                        div()
                            .id(SharedString::from(format!("nc-f-{i}")))
                            .px(px(PAD))
                            .py(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .cursor_pointer()
                            .hover(|h| h.bg(rgba(0x00E0F014)))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .gap(px(12.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .truncate()
                                            .text_size(ui::ts(T_ENTRY))
                                            .text_color(rgb(PALE))
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .flex()
                                            .gap(px(8.))
                                            .font_family(tk().mono)
                                            .text_size(ui::ts(T_LABEL))
                                            .child(
                                                div()
                                                    .text_color(rgb(GREEN))
                                                    .child(format!("+{}", f.added)),
                                            )
                                            .child(
                                                div()
                                                    .text_color(rgb(RED))
                                                    .child(format!("−{}", f.removed)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .h(px(3.))
                                            .w(relative(add.min(1.) * 0.5))
                                            .bg(rgb(GREEN)),
                                    )
                                    .child(
                                        div().h(px(3.)).w(relative(del.min(1.) * 0.5)).bg(rgb(RED)),
                                    ),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(NightSessionEvent::OpenFile {
                                    session: session.clone(),
                                    path: path.clone(),
                                });
                            })),
                    );
                }
            }
        }

        let ctx_color = if p.context_pct >= 85 { RED } else { CYAN };
        let fact = |name: &str, value: String| {
            div()
                .flex()
                .justify_between()
                .child(label(name.to_string(), MIST))
                .child(entry(value))
        };
        let mut facts = frame(YELLOW, "nc-facts", false)
            .flex_none()
            .pb(px(PAD))
            .child(section("Facts", YELLOW, None));
        let mut list = div()
            .px(px(PAD))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(fact("Context", format!("{}%", p.context_pct)))
            .child(bar(f32::from(p.context_pct) / 100., ctx_color, 4.))
            .child(fact("Tokens", short_count(p.tokens)))
            .child(fact("Cost", format!("${:.2}", p.cost)))
            .child(fact("Subagents", p.subagents.len().to_string()));
        for (text, bad) in test_lines(&p.tail) {
            list = list.child(
                div()
                    .truncate()
                    .child(label(text, if bad { RED } else { GREEN }).font_family(tk().mono)),
            );
        }
        facts = facts.child(list);
        // The subagents with their real state and task.
        let mut subs = frame(CYAN, "nc-subs", false)
            .flex_none()
            .pb(px(PAD))
            .child(section(
                "Subagents",
                CYAN,
                Some(format!("{}", p.subagents.len())),
            ));
        if p.subagents.is_empty() {
            subs = subs.child(div().px(px(PAD)).child(label("No subagent runs", MIST)));
        }
        for s in p.subagents.iter().take(5) {
            let (word, color) = match s.state {
                SessionState::Working => ("WORKING", CYAN),
                SessionState::NeedsYou => ("WAITS FOR YOU", RED),
                SessionState::Error => ("FAULT", YELLOW),
                SessionState::Done => ("DONE", GREEN),
                SessionState::Idle => ("IDLE", MIST),
                SessionState::Parked => ("PARKED", MIST),
            };
            subs = subs.child(
                div()
                    .px(px(PAD))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(12.))
                            .child(entry(s.name.clone()))
                            .child(label(word, color).font_weight(FontWeight::BOLD)),
                    )
                    .child(label(s.task.clone(), MIST).line_clamp(2)),
            );
        }
        let right = div()
            .w(px(340.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(file_panel)
            .child(facts)
            .child(subs);

        div()
            .size_full()
            .flex()
            .gap(px(GAP))
            .p(px(GAP))
            .bg(rgb(BG))
            .font_family(CHAKRA)
            .child(self.switcher(now, window, cx))
            .child(middle)
            .child(right)
            .into_any_element()
    }
}
