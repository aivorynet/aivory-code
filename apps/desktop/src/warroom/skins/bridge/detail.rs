//! The Bridge session view (mockup `BridgeDetail`): the session in a header, the full chat with
//! small inline charts, the ask card and a composer, the files with added and removed bars,
//! the turn timeline and the tests. The frame and the session switcher belong to the shell
//! (`shell/bridge.rs`); J and K here switch the session, Esc goes back. The chat reads the
//! whole transcript: the tail of the pane when it holds every line, else `session/history`.

use std::time::{Duration, Instant};

use aivory_protocol::rpc::{HistoryParams, SessionRef, method};
use aivory_protocol::session::{SessionDiff, SessionTrace, SpanKind};
use aivory_protocol::{Line, Pane};
use gpui::prelude::*;
use gpui::{
    Context, Div, Entity, EventEmitter, FocusHandle, Focusable, KeystrokeEvent, ScrollHandle,
    SharedString, Subscription, Window, div, px, relative, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use super::data::{provider_name, test_lines, tests_of, turn_count, turn_of, wait_text};
use crate::actions::{Action, act};
use crate::bridge_kit::*;
use crate::model::Model;

/// How old a diff, a trace or a history page gets before the view asks again.
const TTL: Duration = Duration::from_secs(8);
/// How long the charts take to grow in.
const GROW_MS: f32 = 700.;

#[derive(Debug, Clone)]
pub enum BridgeDetailEvent {
    /// Back to the war room.
    Back,
    /// Show another session.
    Switch(String),
    /// The Focus view of the session.
    Focus(String),
    /// Open a file of the session in the code lens.
    OpenFile { session: String, path: String },
}

impl EventEmitter<BridgeDetailEvent> for BridgeDetail {}

pub struct BridgeDetail {
    model: Entity<Model>,
    id: String,
    focus: FocusHandle,
    input: Entity<InputState>,
    diff: Option<(Option<SessionDiff>, Instant)>,
    trace: Option<(Option<SessionTrace>, Instant)>,
    /// The page of `session/history`: lines, total, and when it was read.
    history: Option<(Vec<Line>, u32, Instant)>,
    busy: [bool; 3],
    scroll: ScrollHandle,
    /// The session and row count that the chat was last scrolled to the end for.
    scrolled: String,
    /// When the session opened: the charts grow in from here.
    opened: Instant,
    _subs: Vec<Subscription>,
}

impl BridgeDetail {
    pub fn new(
        model: Entity<Model>,
        id: String,
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
            focus: cx.focus_handle(),
            input,
            diff: None,
            trace: None,
            history: None,
            busy: [false; 3],
            scroll: ScrollHandle::new(),
            scrolled: String::new(),
            opened: Instant::now(),
            _subs: vec![observe, keys, events],
        }
    }

    /// Show another session: the data resets and the charts grow in again.
    pub fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id != id {
            self.id = id.to_string();
            self.diff = None;
            self.trace = None;
            self.history = None;
            self.scrolled.clear();
            self.opened = Instant::now();
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

    /// J and K switch the session, Esc goes back. Typing in the field keeps its keys.
    fn on_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let typing = self.input.focus_handle(cx).is_focused(window);
        let plain = !event.keystroke.modifiers.modified();
        match key {
            "escape" if typing => self.focus.focus(window, cx),
            "escape" => cx.emit(BridgeDetailEvent::Back),
            "j" | "k" if plain && !typing => {
                let next = {
                    let model = self.model.read(cx);
                    model.war.as_ref().and_then(|w| {
                        let list = order(w);
                        let at = list.iter().position(|p| p.id == self.id)?;
                        let n = list.len();
                        let to = if key == "j" {
                            (at + 1) % n
                        } else {
                            (at + n - 1) % n
                        };
                        Some(list[to].id.clone())
                    })
                };
                if let Some(id) = next {
                    cx.emit(BridgeDetailEvent::Switch(id));
                }
            }
            _ => {}
        }
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

    /// Read the diff, the trace and the history again when the last read is old.
    fn ensure(&mut self, pane: &Pane, cx: &mut Context<Self>) {
        let id = self.id.clone();
        if !self.busy[0] && self.diff.as_ref().is_none_or(|(_, t)| t.elapsed() > TTL) {
            self.busy[0] = true;
            let task = self
                .model
                .read(cx)
                .call::<method::SessionDiffMethod>(SessionRef { id: id.clone() }, cx);
            let want = id.clone();
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.busy[0] = false;
                    if this.id == want {
                        this.diff = Some((result.ok(), Instant::now()));
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        if !self.busy[1] && self.trace.as_ref().is_none_or(|(_, t)| t.elapsed() > TTL) {
            self.busy[1] = true;
            let task = self
                .model
                .read(cx)
                .call::<method::SessionSpans>(SessionRef { id: id.clone() }, cx);
            let want = id.clone();
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.busy[1] = false;
                    if this.id == want {
                        this.trace = Some((result.ok(), Instant::now()));
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        // The pane tail holds the whole transcript only in a short session.
        let short = pane.tail.len() as u32 >= pane.lines_total;
        let stale = self
            .history
            .as_ref()
            .is_none_or(|(_, total, t)| t.elapsed() > TTL || *total != pane.lines_total);
        if !short && stale && !self.busy[2] {
            self.busy[2] = true;
            let task = self.model.read(cx).call::<method::SessionHistory>(
                HistoryParams {
                    id: id.clone(),
                    subagent: None,
                    before: None,
                    limit: Some(400),
                },
                cx,
            );
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.busy[2] = false;
                    if this.id == id {
                        if let Ok(page) = result {
                            this.history = Some((page.lines, page.total, Instant::now()));
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// The lines of the whole transcript.
    fn lines(&self, pane: &Pane) -> Vec<Line> {
        match &self.history {
            Some((lines, _, _)) if (pane.tail.len() as u32) < pane.lines_total => lines.clone(),
            _ => pane.tail.clone(),
        }
    }
}

impl Focusable for BridgeDetail {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A turn of the session: its weight (tokens, or rows without a trace), start and end.
struct Turn {
    weight: u64,
    start: i64,
    end: i64,
}

/// The turns of a trace in start order.
fn turns_of(trace: &SessionTrace) -> Vec<Turn> {
    let mut turns: Vec<&aivory_protocol::session::Span> = trace
        .spans
        .iter()
        .filter(|s| s.kind == SpanKind::Turn)
        .collect();
    turns.sort_by_key(|s| s.start);
    turns
        .iter()
        .enumerate()
        .map(|(k, t)| {
            let tokens: u64 = trace
                .spans
                .iter()
                .filter(|s| s.kind == SpanKind::Llm)
                .filter(|s| trace.turn_of_span(&s.id) == Some(k as u32 + 1))
                .map(|s| {
                    s.tokens_in.saturating_sub(s.tokens_cache_read.unwrap_or(0)) + s.tokens_out
                })
                .sum();
            Turn {
                weight: if tokens == 0 {
                    t.tokens_in + t.tokens_out
                } else {
                    tokens
                },
                start: t.start,
                end: t.end.unwrap_or(t.start),
            }
        })
        .collect()
}

/// The turns of a transcript without a trace: one per user prompt, weighted by its rows.
fn turns_of_lines(lines: &[Line]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.kind == aivory_protocol::LineKind::User {
            if let Some(last) = out.last_mut() {
                last.end = i as i64;
                last.weight = (last.end - last.start) as u64;
            }
            out.push(Turn {
                weight: 0,
                start: i as i64,
                end: lines.len() as i64,
            });
        }
    }
    if let Some(last) = out.last_mut() {
        last.weight = (last.end - last.start) as u64;
    }
    out
}

/// `289k`, `1.03M`.
fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{:.2}M", n as f64 / 1_000_000.),
    }
}

/// A grey-text note with no data.
fn note(text: &str) -> Div {
    th(text.to_string())
}

impl Render for BridgeDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        let art = Art::of(window);
        let root = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus);
        let Some(p) = self.pane(cx) else {
            return root
                .child(
                    area(316., 56., 700., 60.)
                        .font_family(LABEL)
                        .text_size(px(S_TITLE))
                        .text_color(rgb(TXT))
                        .child("THIS SESSION IS GONE · ESC GOES BACK"),
                )
                .into_any_element();
        };
        self.ensure(&p, cx);
        let now = self.model.read(cx).war.as_ref().map_or(0, |w| w.now);
        let waits = p.ask.is_some();
        let mins = p.minutes(now);
        let id = p.id.clone();
        // The charts grow in when the session opens or switches.
        let grow = ease(self.opened.elapsed(), GROW_MS);
        if grow < 1. {
            window.request_animation_frame();
        }

        let lines = self.lines(&p);

        // ---- header ------------------------------------------------------------------------
        let state = if waits {
            pill(
                "lc-state",
                RD,
                &format!("Waits for you · {}", wait_text(mins)),
            )
        } else {
            pill(
                "lc-state",
                state_color(&p),
                &format!("{} · {}", state_label(&p), wait_text(mins)),
            )
        }
        .px(px(18.));
        let ctl =
            |name: &'static str, c: u32, action: fn(String) -> Action, cx: &mut Context<Self>| {
                let id = id.clone();
                pill(format!("lc-{name}"), c, name)
                    .w(px(88.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act(&this.model, action(id.clone()), cx);
                    }))
            };
        let focus_id = p.id.clone();
        let header =
            area(316., 52., 600. + art.dx / 2., 178.)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_family(LABEL)
                        .text_size(px(S_TITLE))
                        .line_height(px(50.))
                        .text_color(rgb(TXT))
                        .child(p.title.to_uppercase()),
                )
                .child(th(head_meta(&p)).mt(px(10.)))
                .child(
                    div()
                        .mt(px(18.))
                        .flex()
                        .gap(px(8.))
                        .child(state)
                        .child(ctl("Pause", OR, Action::Pause, cx))
                        .child(ctl("Park", OR, Action::Park, cx))
                        .child(ctl("Stop", PK, Action::Stop, cx))
                        .child(pill("lc-focus", PK, "Focus view").w(px(120.)).on_click(
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(BridgeDetailEvent::Focus(focus_id.clone()))
                            }),
                        )),
                )
                .child(
                    th("J K or the left column switch session · Esc back to the war room")
                        .mt(px(16.))
                        .text_color(rgb(MUTED)),
                );

        // ---- turns -------------------------------------------------------------------------
        let traced = self
            .trace
            .as_ref()
            .and_then(|(t, _)| t.as_ref())
            .map(turns_of)
            .unwrap_or_default();
        let by_rows = traced.is_empty();
        let turns = if by_rows {
            turns_of_lines(&lines)
        } else {
            traced
        };
        let first_turn = turns.len().saturating_sub(7);
        let last7: Vec<(usize, u64)> = turns
            .iter()
            .enumerate()
            .skip(first_turn)
            .map(|(i, t)| (i + 1, t.weight))
            .collect();
        let most = last7.iter().map(|t| t.1).max().unwrap_or(1).max(1) as f32;
        let mut bars = div()
            .mt(px(10.))
            .h(px(118.))
            .flex()
            .items_end()
            .gap(px(10.));
        for (k, (n, weight)) in last7.iter().enumerate() {
            let last = k + 1 == last7.len();
            bars = bars.child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_end()
                    .gap(px(4.))
                    .child(
                        div()
                            .w_full()
                            .h(px((*weight as f32 / most * 90.).max(3.) * grow))
                            .bg(rgb(if last { OR } else { PK }))
                            .rounded_t(px(8.)),
                    )
                    .child(th(format!("T{n}"))),
            );
        }
        let chart = area(art.rx(930.), 52., 300., 172.)
            .child(th(if by_rows {
                "Rows per turn"
            } else {
                "Tokens per turn"
            }))
            .child(if last7.is_empty() {
                div().mt(px(20.)).child(note("No turn yet"))
            } else {
                bars
            });
        let ring_box = area(art.rx(1260.), 62., 164., 168.)
            .flex()
            .flex_col()
            .items_center()
            .child(ring(
                136.,
                f32::from(p.context_pct) * grow,
                p.context_pct,
                PE,
            ))
            .child(th(format!("Tokens · {}", short_count(p.tokens))).mt(px(8.)));

        // ---- chat: the messages that matter, one speaker label per message -------------------
        let chat_h = art.by(854.) - 330.;
        // The composer is 40 tall; three gaps of 20 lie between the blocks.
        let avail = chat_h - 40. - 60. - if waits { 168. } else { 0. };
        let chars = ((600. + art.dx - 116. - 36.) / 7.).max(20.) as usize;
        let shown = chat_display(&lines, avail, chars);
        let key = format!("{}:{}", p.id, shown.len());
        if self.scrolled != key {
            self.scroll.scroll_to_bottom();
            self.scrolled = key;
        }
        let agent = provider_name(&p.provider);
        let mut list = div()
            .id("lc-d-chat")
            .flex_none()
            .overflow_hidden()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .pr(px(16.));
        for d in &shown {
            list = list.child(display_row(d, &agent, 116., true, chars));
        }
        if shown.is_empty() {
            list = list.child(note("No rows yet"));
        }

        // The ask card.
        let ask_card = p.ask.as_ref().map(|ask| {
            let card = div()
                .flex_none()
                .border_l(px(12.))
                .border_color(rgb(RD))
                .rounded_l(px(6.))
                .pl(px(24.))
                .py(px(16.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    sec(format!(
                        "{} asks · {}",
                        provider_name(&p.provider),
                        crate::warroom::skins::common::kind_word(ask.kind).to_lowercase()
                    ))
                    .text_color(rgb(RD)),
                )
                .child(
                    div()
                        .max_h(px(48.))
                        .overflow_hidden()
                        .font_family(CODE)
                        .text_size(px(S_BODY))
                        .text_color(rgb(WHITE))
                        .child(ask.text.clone()),
                );
            let mut row = div().flex().gap(px(8.));
            if p.controlled {
                let n = ask.options.len();
                for (k, option) in ask.options.iter().enumerate() {
                    let (sid, aid, opt) = (p.id.clone(), ask.id.clone(), option.clone());
                    row = row.child(
                        pill(
                            format!("lc-answer-{k}"),
                            option_color(k, n),
                            &option_text(k, option),
                        )
                        .px(px(18.))
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
                row = row.child(th("Answer it in its terminal"));
            }
            card.child(row)
        });

        // The composer: the field and Send.
        let composer = div()
            .flex_none()
            .h(px(40.))
            .flex()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .child(div().w(px(10.)).h_full().bg(rgb(PU)).rounded_l(px(5.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .bg(rgb(0x120D16))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .font_family(BODY)
                            .text_size(px(S_BODY))
                            .text_color(rgb(TXT))
                            .child(div().flex_1().min_w(px(0.)).child(self.input.clone())),
                    ),
            )
            .child(
                pill("lc-send", OR, "Send")
                    .h(px(40.))
                    .w(px(88.))
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
            );
        let chat_box = area(316., 330., 600. + art.dx, art.by(854.) - 330.)
            .flex()
            .flex_col()
            .gap(px(20.))
            .overflow_hidden()
            .child(list)
            .children(ask_card)
            .child(div().flex_1())
            .child(composer);

        // ---- what changed ------------------------------------------------------------------
        let files = self
            .diff
            .as_ref()
            .and_then(|(d, _)| d.as_ref())
            .map(|d| d.files.clone())
            .unwrap_or_default();
        let (add, del) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        let widest = files
            .iter()
            .map(|f| f.added.max(f.removed))
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let mut side = area(art.rx(972.), 330., 452., art.by(854.) - 330.)
            .flex()
            .flex_col()
            .gap(px(8.))
            .overflow_hidden()
            .child(head(
                OR,
                &format!("Files changed · {}", files.len()),
                &format!("+{add} −{del}"),
            ));
        for f in files.iter().take(4) {
            let (session, path) = (p.id.clone(), f.path.clone());
            let bar = |n: u32, c: u32, left: bool| {
                let w = (n as f32 / widest * 88.).max(if n > 0 { 3. } else { 0. }) * grow;
                div()
                    .flex()
                    .when(left, |d| d.justify_end())
                    .child(div().h(px(12.)).w(px(w)).bg(rgb(c)))
            };
            side = side.child(
                div()
                    .id(SharedString::from(format!("lc-f-{}", f.path)))
                    .flex_none()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(BridgeDetailEvent::OpenFile {
                            session: session.clone(),
                            path: path.clone(),
                        })
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family(CODE)
                            .text_size(px(S_CODE))
                            .text_color(rgb(DIM))
                            .child(f.path.clone()),
                    )
                    .child(div().w(px(90.)).child(bar(f.removed, RD, true)))
                    .child(div().w(px(90.)).child(bar(f.added, PK, false))),
            );
        }
        if files.is_empty() {
            side = side.child(note("No change yet"));
        } else {
            side = side.child(
                div()
                    .flex()
                    .justify_between()
                    .child(th("Removed").text_color(rgb(RD)))
                    .child(th("Added")),
            );
        }

        // Turns · time.
        let span_start = turns.first().map_or(0, |t| t.start);
        let span_end = turns.iter().map(|t| t.end).max().unwrap_or(span_start);
        let total = (span_end - span_start).max(1) as f32;
        let span_label = if by_rows {
            format!("{} rows", (span_end - span_start).max(0))
        } else {
            format!("{} min", (span_end - span_start).max(0) / 60_000)
        };
        side = side.child(head(PE, "Turns · time", &span_label).mt(px(16.)));
        let tl_first = turns.len().saturating_sub(5);
        for (k, t) in turns.iter().enumerate().skip(tl_first) {
            let x = (t.start - span_start) as f32 / total;
            let w = ((t.end - t.start) as f32 / total).max(0.02);
            let c = if k + 1 == turns.len() && waits {
                RD
            } else {
                PK
            };
            let end = if by_rows {
                format!("{}", t.end - t.start)
            } else {
                format!("{}m", ((t.end - t.start).max(0) + 30_000) / 60_000)
            };
            side = side.child(
                div()
                    .flex_none()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(th(format!("T{}", k + 1)).w(px(30.)))
                    .child(
                        div().relative().flex_1().h(px(12.)).child(
                            div()
                                .absolute()
                                .top_0()
                                .h(px(12.))
                                .left(relative(x.min(0.98)))
                                .w(relative(w.min(1. - x.min(0.98)) * grow))
                                .bg(rgb(c)),
                        ),
                    )
                    .child(th(end).w(px(46.)).flex().justify_end().text_color(rgb(TXT))),
            );
        }
        if turns.is_empty() {
            side = side.child(note("No turn yet"));
        }

        // Tests, read from the transcript.
        let counts = tests_of(&lines);
        let label = match counts {
            Some((ok, bad)) => format!("{ok} of {}", ok + bad),
            None => "no run yet".to_string(),
        };
        side = side.child(head(PK, "Tests", &label).mt(px(16.)));
        if let Some((ok, bad)) = counts {
            let total = (ok + bad).max(1) as usize;
            let on = ((ok as usize * 12 / total) as f32 * grow).round() as usize;
            let mut blocks = div().flex_none().flex().gap(px(4.)).h(px(18.));
            for k in 0..12 {
                blocks = blocks.child(div().flex_1().h_full().bg(rgb(if k < on {
                    PE
                } else if bad > 0 && grow >= 1. {
                    RD
                } else {
                    RULE
                })));
            }
            side = side.child(blocks);
            if let Some(last) = test_lines(&lines, 1).pop() {
                side = side.child(
                    div()
                        .truncate()
                        .font_family(CODE)
                        .text_size(px(S_CODE))
                        .text_color(rgb(MUTED))
                        .child(last),
                );
            }
        }

        root.child(header)
            .child(chart)
            .child(ring_box)
            .child(chat_box)
            .child(side)
            .into_any_element()
    }
}

/// `Claude · opus-5.5 · billing / payment · turn 07`.
fn head_meta(p: &Pane) -> String {
    let mut m = provider_name(&p.provider);
    if !p.model.is_empty() {
        m.push_str(&format!(" · {}", p.model));
    }
    m.push_str(&format!(" · {}", p.codebase));
    if !p.part.is_empty() {
        m.push_str(&format!(" / {}", p.part));
    }
    if let Some(t) = turn_of(p) {
        m.push_str(&format!(" · turn {t:02}"));
    } else if turn_count(&p.tail) > 0 {
        m.push_str(&format!(" · turn {:02}", turn_count(&p.tail)));
    }
    m
}

/// The state word of a session without an ask.
fn state_label(p: &Pane) -> &'static str {
    match p.state {
        aivory_protocol::SessionState::Error => "Error",
        aivory_protocol::SessionState::Working => "Working",
        aivory_protocol::SessionState::Idle => "Idle",
        aivory_protocol::SessionState::NeedsYou => "Waits for you",
        aivory_protocol::SessionState::Parked => "Parked",
        aivory_protocol::SessionState::Done => "Done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::LineKind;

    #[test]
    fn token_counts_are_short() {
        assert_eq!(short_count(950), "950");
        assert_eq!(short_count(289_400), "289k");
        assert_eq!(short_count(1_030_000), "1.03M");
    }

    #[test]
    fn turns_come_from_the_user_prompts() {
        let lines = vec![
            Line::new(LineKind::User, "a"),
            Line::new(LineKind::Agent, "b"),
            Line::new(LineKind::User, "c"),
            Line::new(LineKind::Agent, "d"),
            Line::new(LineKind::Agent, "e"),
        ];
        let t = turns_of_lines(&lines);
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].weight, t[1].weight), (2, 3));
    }
}
