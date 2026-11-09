//! The Operator look of the war room (`OperatorCore.dc.html`). A wall of monitors: blue screens
//! show information and decisions, green screens show agents. A screen is a console or rain,
//! never both. A busy agent on the wall is code rain made of its own output (speed = how busy
//! it is); an idle agent shows a static prompt with a blinking block; the agent behind the
//! decision is a console. The navigation (F-keys) is in `shell/operator.rs`.
//! `kit` holds the monitor frame and the token streams; the session view uses them too.

pub(crate) mod kit;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use aivory_protocol::{AskKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, Context, Div, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    ScrollWheelEvent, Subscription, Window, canvas, div, px, relative, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use self::kit::*;
use super::common::{asks, clip, live};
use crate::actions::{Action, act};
use crate::ui;
use crate::warroom::decisions::question;
use crate::warroom::{WarRoomEvent, WarRoomScreen};

/// The agent screens on the bottom row.
const PER_PAGE: usize = 4;
/// The plot of the fleet chart.
const PLOT_H: f32 = 110.;
const WIDE: f32 = 380.;
/// The gap between the monitors.
const GAP: f32 = 20.;
/// The inner padding of a screen.
const PAD: f32 = 24.;

/// What the Operator look keeps between frames.
#[derive(Default)]
pub(in crate::warroom) struct OperatorState {
    /// The start of the animation clock.
    t0: Option<Instant>,
    /// The session whose decision is on the blue screen.
    pick: Option<String>,
    /// The option of the operator menu that is lit.
    opt: usize,
    /// The page of agent screens.
    page: usize,
    /// The bounds of the scroll track, kept for the mouse.
    track: Rc<Cell<Bounds<Pixels>>>,
    /// The prompt line of the center screen, and the event subscription of its Enter key.
    steer: Option<Entity<InputState>>,
    steer_sub: Option<Subscription>,
    /// The session on the center screen: the target of the prompt line.
    focus_id: Option<String>,
    /// The prompt line has the keyboard: the keys of the wall stay quiet.
    typing: bool,
    /// Give the keyboard back to the wall in the next frame.
    blur: bool,
    /// The agent on the center screen. The owner picks it on the bottom row; none = no signal.
    active: Option<String>,
    /// The agent screen under the mouse and the start of its reveal.
    hover: Option<(String, Instant)>,
    /// The scroll of the center console, and "go to the newest line in the next frame".
    center_scroll: gpui::ScrollHandle,
    to_bottom: bool,
    /// When the agent came to the center screen: the reveal of the console runs from it.
    active_at: Option<Instant>,
    /// The screenshot hooks `AIVORY_MX_PICK=<n>` and `AIVORY_MX_HOVER=<n>` were read.
    hooks_read: bool,
    /// A scroll bar drag in progress; the whole wall follows the mouse until the button is up.
    drag: Option<Drag>,
    /// The bounds of the center console scroll track, kept for the mouse.
    center_track: Rc<Cell<Bounds<Pixels>>>,
}

/// A scroll bar drag.
#[derive(Clone, Copy)]
enum Drag {
    /// The page bar at the bottom.
    Pages,
    /// The thumb of the center console: the mouse y and the scroll offset at the start.
    Center { from_y: f32, from_off: f32 },
}

/// The agents for the screens: the ones that work now (the busiest first), then the other
/// live sessions, the newest first, so that no screen stays dark while sessions exist
/// (owner 2026-11-05). The sort is stable.
fn working<'a>(all: &[&'a Pane]) -> Vec<&'a Pane> {
    let mut list: Vec<&Pane> = all.to_vec();
    list.sort_by(|a, b| {
        let wa = a.state == SessionState::Working;
        let wb = b.state == SessionState::Working;
        wb.cmp(&wa)
            .then_with(|| {
                if wa && wb {
                    activity(b).total_cmp(&activity(a))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| b.since.cmp(&a.since))
    });
    list
}

impl WarRoomScreen {
    /// The wall: decision, queue, focus agent, fleet chart, budget, and the agent screens.
    pub(in crate::warroom) fn render_operator(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::ui::animate(window, cx);
        let t = self
            .operator
            .t0
            .get_or_insert_with(Instant::now)
            .elapsed()
            .as_secs_f32();
        if std::mem::take(&mut self.operator.blur) {
            self.focus.focus(window, cx);
        }
        let queue = asks(war);
        let at = self.operator_at(&queue);
        let current = at.map(|i| queue[i]);
        let all = live(war);
        let busy = working(&all);
        let pages = busy.len().div_ceil(PER_PAGE).max(1);
        if self.operator.page >= pages {
            self.operator.page = pages - 1;
        }
        let opt = self.operator.opt;
        let page = self.operator.page;
        // The center screen shows only the agent that the owner picked on the bottom row.
        let focus = self
            .operator
            .active
            .as_ref()
            .and_then(|id| all.iter().copied().find(|p| &p.id == id));
        if focus.is_none() {
            self.operator.active = None;
        }
        let waits = focus.is_some_and(|f| queue.iter().any(|q| q.id == f.id));
        let steer = self.operator_steer(window, cx);
        self.operator.focus_id = focus.map(|p| p.id.clone());
        let typing = gpui::Focusable::focus_handle(&steer, cx).is_focused(window);
        self.operator.typing = typing;

        let left = div()
            .flex_none()
            .w(px(WIDE))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(self.operator_decision(war, current, at, queue.len(), opt, t, cx))
            .child(
                dimmed("mx-dim-queue".into())
                    .flex_none()
                    .h(px(176.))
                    .child(self.operator_queue(war, &queue, at, cx)),
            );
        let center = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(self.operator_focus(focus, waits, &all, &steer, typing, t, cx));
        let right = div()
            .flex_none()
            .w(px(WIDE))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(
                dimmed("mx-dim-fleet".into())
                    .flex_none()
                    .h(px(276.))
                    .child(operator_fleet(war, all.len(), t)),
            )
            .child(
                dimmed("mx-dim-budget".into())
                    .flex_1()
                    .min_h(px(0.))
                    .child(operator_budget(war, &all, t)),
            );
        let top = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .gap(px(GAP))
            .child(left)
            .child(center)
            .child(right);

        let mut row = div()
            .id("mx-row")
            .flex_none()
            .h(px(250.))
            .flex()
            .gap(px(GAP))
            .on_scroll_wheel(cx.listener(Self::operator_wheel));
        for slot in 0..PER_PAGE {
            let idx = page * PER_PAGE + slot;
            row = row.child(match busy.get(idx) {
                Some(pane) => self.operator_tile(pane, idx, t, cx),
                None => operator_dark(idx, busy.is_empty() && slot == 0),
            });
        }
        // Screenshot hooks: put agent n on the center, or show the reveal of agent n.
        if !self.operator.hooks_read {
            self.operator.hooks_read = true;
            let hook = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok());
            if let Some(p) = hook("AIVORY_MX_PICK").and_then(|n| busy.get(n)) {
                self.operator.active = Some(p.id.clone());
                self.operator.active_at = Some(Instant::now());
            }
        }
        if let Some(p) = std::env::var("AIVORY_MX_HOVER")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .and_then(|n| busy.get(n))
            && self.operator.hover.as_ref().is_none_or(|(h, _)| h != &p.id)
        {
            self.operator.hover = Some((p.id.clone(), Instant::now()));
        }
        let active = self.operator.active.clone();
        let bar = self
            .operator_scroll(busy.len(), page, pages, cx)
            .into_any_element();
        let first = page * PER_PAGE;
        let lines: Vec<(LineFrom, u32, bool)> = busy
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let from = if i < first {
                    LineFrom::Left
                } else if i < first + PER_PAGE {
                    LineFrom::Slot(i - first)
                } else {
                    LineFrom::Right
                };
                (
                    from,
                    state_color(p.state).0,
                    active.as_deref() == Some(p.id.as_str()),
                )
            })
            .collect();
        let strip = div()
            .flex_none()
            .h(px(16.))
            .relative()
            .child(state_lines(lines, PER_PAGE, WIDE, GAP, t));

        div()
            .id("mx-wall")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .px(px(28.))
            .pt(px(10.))
            .pb(px(10.))
            .bg(rgb(PAGE))
            // A scroll bar drag goes on wherever the mouse goes, until the button is up.
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                let Some(drag) = this.operator.drag else {
                    return;
                };
                if e.pressed_button != Some(MouseButton::Left) {
                    this.operator.drag = None;
                    return;
                }
                match drag {
                    Drag::Pages => this.operator_seek(e.position.x, pages, cx),
                    Drag::Center { from_y, from_off } => {
                        this.operator_center_drag(f32::from(e.position.y) - from_y, from_off);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, _| this.operator.drag = None),
            )
            .font_family(FONT)
            .text_size(ui::ts(T_ENTRY))
            .child(top)
            .child(strip)
            .child(row)
            .child(bar)
            .into_any_element()
    }

    /// The prompt line of the center screen, made on first use. Enter sends the text to the
    /// session on the center screen.
    fn operator_steer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.operator.steer {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("write to this session"));
        let events = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| {
                let InputEvent::PressEnter { .. } = event else {
                    return;
                };
                let text = input.read(cx).value().trim().to_string();
                let Some(id) = this.operator.focus_id.clone() else {
                    return;
                };
                if text.is_empty() {
                    return;
                }
                act(
                    &this.model,
                    Action::Send {
                        id,
                        text,
                        composer: None,
                    },
                    cx,
                );
                input.update(cx, |s, cx| s.clean(window, cx));
                this.operator.blur = true;
                cx.notify();
            },
        );
        self.operator.steer = Some(input.clone());
        self.operator.steer_sub = Some(events);
        input
    }

    /// The index of the decision on the blue screen.
    fn operator_at(&self, queue: &[&Pane]) -> Option<usize> {
        if queue.is_empty() {
            return None;
        }
        Some(
            self.operator
                .pick
                .as_ref()
                .and_then(|id| queue.iter().position(|p| &p.id == id))
                .unwrap_or(0),
        )
    }

    /// Blue: the decision as an operator prompt menu. It is the brightest screen of the wall.
    #[allow(clippy::too_many_arguments)]
    fn operator_decision(
        &mut self,
        war: &WarRoom,
        pane: Option<&Pane>,
        at: Option<usize>,
        total: usize,
        opt: usize,
        t: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut body = div()
            .absolute()
            .inset_0()
            .px(px(PAD))
            .py(px(PAD - 6.))
            .flex()
            .flex_col()
            .overflow_hidden();
        let (Some(pane), Some(at)) = (pane, at) else {
            body = body
                .child(
                    div()
                        .text_size(ui::ts(T_SECTION))
                        .text_color(rgb(BLUE_HI))
                        .child("Incoming · 0"),
                )
                .child(
                    div()
                        .mt(px(14.))
                        .text_color(rgb(BLUE_SOFT))
                        .child("No agent waits for you."),
                );
            return bezel(BLUE_RING)
                .flex_1()
                .min_h(px(0.))
                .child(screen(true, body))
                .child(foot("1", FOOT, vec![("decisions".into(), FOOT)], 30.));
        };
        let Some(ask) = pane.ask.as_ref() else {
            return bezel(BLUE_RING);
        };
        let head = div()
            .flex_none()
            .h(px(30.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(ui::ts(T_SECTION))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(BLUE_HI))
                    .child(format!("Incoming {} of {}", at + 1, total)),
            )
            .child(div().relative().size(px(24.)).child(radar(t)));
        let mut top = div()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .pt(px(6.))
            .child(
                div()
                    .text_size(ui::ts(T_LABEL))
                    .text_color(rgb(BLUE_SOFT))
                    .child(format!(
                        "{}@{} · waits {} min",
                        pane.provider,
                        pane.codebase,
                        pane.minutes(war.now)
                    )),
            )
            .child(
                div()
                    .mt(px(10.))
                    .text_size(ui::ts(T_ENTRY))
                    .line_height(relative(1.5))
                    .text_color(rgb(BLUE_HI))
                    .child(clip(&question(pane, ask), 110)),
            );
        if ask.kind == AskKind::Permission {
            top = top.child(
                div()
                    .mt(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .border_l_2()
                    .border_color(rgb(BLUE_RING))
                    .bg(rgb(0x021024))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(ui::ts(T_LABEL))
                    .child(clip(&ask.text, 56)),
            );
        }
        let mut menu = div().flex_none().flex().flex_col().gap(px(2.));
        let opt = opt.min(ask.options.len().saturating_sub(1));
        for (i, option) in ask.options.iter().take(4).enumerate() {
            let on = i == opt;
            let (session, ask_id, text) = (pane.id.clone(), ask.id.clone(), option.clone());
            menu = menu.child(
                div()
                    .id(eid(format!("mx-opt-{i}")))
                    .h(px(32.))
                    .flex_none()
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .text_size(ui::ts(T_ENTRY))
                    .when(on, |d| {
                        d.bg(rgb(BLUE))
                            .text_color(rgb(BLUE_INK))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                    })
                    .when(!on, |d| d.hover(|h| h.bg(rgb(BLUE_DIM))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.operator.opt = 0;
                        this.skin_answer(&session, &ask_id, &text, cx);
                    }))
                    .child(if on { "▶" } else { " " })
                    .child(format!("{}  {}", i + 1, option)),
            );
        }
        let open = pane.id.clone();
        menu = menu.child(
            div()
                .id("mx-opt-write")
                .h(px(32.))
                .flex_none()
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .cursor_pointer()
                .text_size(ui::ts(T_ENTRY))
                .hover(|h| h.bg(rgb(BLUE_DIM)))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(WarRoomEvent::OpenSession(open.clone()));
                }))
                .child(" ")
                .child("W  Write an answer…"),
        );
        let body = body.child(head).child(top).child(menu);
        bezel(BLUE_RING)
            .flex_1()
            .min_h(px(0.))
            .child(screen(true, body))
            .child(foot(
                "1",
                BLUE,
                vec![(name(pane), BLUE_HI), ("· ↑ ↓ ↵ · 1-4 answer".into(), FOOT)],
                30.,
            ))
    }

    /// Blue: the other decisions, the oldest first. Dim until the pointer comes.
    fn operator_queue(
        &mut self,
        war: &WarRoom,
        queue: &[&Pane],
        at: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut body = div()
            .absolute()
            .inset_0()
            .px(px(PAD))
            .py(px(PAD - 8.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .text_size(ui::ts(T_SECTION))
                    .text_color(rgb(BLUE_HI))
                    .child("Queue"),
            );
        let mut shown = 0;
        for (i, pane) in queue.iter().enumerate() {
            if Some(i) == at || shown >= 2 {
                continue;
            }
            shown += 1;
            let id = pane.id.clone();
            body = body.child(
                div()
                    .id(eid(format!("mx-q-{i}")))
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .text_size(ui::ts(T_ENTRY))
                    .hover(|h| h.bg(rgb(BLUE_DIM)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.operator.pick = Some(id.clone());
                        this.operator.opt = 0;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(22.))
                            .text_color(rgb(BLUE_HI))
                            .child((i + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(name(pane)),
                    )
                    .child(div().child(format!("{}m", pane.minutes(war.now)))),
            );
        }
        let more = queue
            .len()
            .saturating_sub(shown + usize::from(at.is_some()));
        if more > 0 {
            body = body.child(
                div()
                    .flex_none()
                    .pt(px(2.))
                    .pl(px(30.))
                    .text_size(ui::ts(T_LABEL))
                    .text_color(rgb(BLUE_SOFT))
                    .child(format!("+{more} more")),
            );
        }
        if shown == 0 {
            body = body.child(
                div()
                    .mt(px(8.))
                    .text_color(rgb(BLUE_SOFT))
                    .child("queue is empty"),
            );
        }
        bezel(BLUE_RING)
            .flex_1()
            .min_h(px(0.))
            .child(screen(true, body))
            .child(foot(
                "2",
                FOOT,
                vec![("[ ] switch decision".into(), FOOT)],
                30.,
            ))
    }

    /// Green: the agent behind the decision, as a console. A click opens its session. A prompt
    /// line at the bottom writes to this session; it folds away until the pointer comes.
    #[allow(clippy::too_many_arguments)]
    fn operator_focus(
        &mut self,
        pane: Option<&Pane>,
        waits: bool,
        all: &[&Pane],
        steer: &Entity<InputState>,
        typing: bool,
        t: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(pane) = pane else {
            return bezel(GREEN_DIM)
                .flex_1()
                .min_h(px(0.))
                .child(screen(
                    false,
                    div().absolute().inset_0().child(interference(t)).child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .px(px(18.))
                                    .py(px(8.))
                                    .bg(hca(0x010603, 220))
                                    .border_1()
                                    .border_color(hca(GREEN_DIM, 160))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(ui::ts(MX_TITLE))
                                    .text_color(rgb(GREEN))
                                    .flex_col()
                                    .child("No signal")
                                    .child(
                                        div()
                                            .text_size(ui::ts(MX_SMALL))
                                            .text_color(rgb(GREEN_MID))
                                            .child("Pick an agent below to watch it here"),
                                    ),
                            ),
                    ),
                ))
                .child(foot("-", FOOT, vec![], 30.));
        };
        let no = all
            .iter()
            .position(|p| p.id == pane.id)
            .map_or(1, |i| i + 1);
        let id = pane.id.clone();
        let tail = display_lines(pane, 400);
        let stream = match tail.split_last() {
            Some((newest, old)) if pane.state == SessionState::Working => {
                decoded(&pane.id, old, newest, hash_str(&pane.id), t)
            }
            _ => tokenize(&tail),
        };
        // Go to the newest line once the console itself shows (not during the reveal).
        if self.operator.to_bottom
            && self
                .operator
                .active_at
                .is_none_or(|at| at.elapsed().as_secs_f32() >= 0.9)
        {
            self.operator.to_bottom = false;
            self.operator.center_scroll.scroll_to_bottom();
        }
        // The reveal: for 0.9 s after the pick the rain of the agent turns into its console.
        let arrive = self
            .operator
            .active_at
            .map(|at| at.elapsed().as_secs_f32() / 0.9)
            .filter(|p| *p < 1.);
        if arrive.is_some() {
            cx.notify();
        }
        let (tint, _) = state_color(pane.state);
        let source = display_lines(pane, 24).join(" ");
        let arrive_el = arrive.map(|p| {
            div()
                .absolute()
                .inset_0()
                .top(px(40.))
                .bottom(px(30.))
                .child(reveal(
                    &source,
                    tail.clone(),
                    activity(pane),
                    p,
                    t,
                    hash_str(&pane.id),
                    tint,
                    MX_BODY,
                    PAD,
                ))
        });
        let body = div()
            .absolute()
            .inset_0()
            .pb(px(30.))
            .child(
                div()
                    .id("mx-center-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.operator.center_scroll)
                    .pl(px(PAD))
                    .pr(px(PAD + 8.))
                    .pt(px(PAD + 40.))
                    .pb(px(PAD + 56.))
                    .bg(hca(0x010A03, 200))
                    .text_size(ui::ts(MX_BODY))
                    .line_height(relative(1.55))
                    .child(stream.into_text()),
            )
            .child(self.operator_center_bar(cx));
        // The bar on top: readable buttons that show on hover (and stay while you type).
        // They jump to the next screen; the chat line below is always open.
        let open_id = id.clone();
        let button = |key: &'static str, label: &'static str, main: bool| {
            div()
                .id(key)
                .h(px(26.))
                .px(px(12.))
                .flex()
                .items_center()
                .cursor_pointer()
                .border_1()
                .border_color(hca(if main { GREEN } else { GREEN_DIM }, 220))
                .bg(hca(if main { 0x0C3A1C } else { 0x021006 }, 245))
                .text_size(ui::ts(MX_TITLE))
                .text_color(rgb(GREEN_HI))
                .hover(|s| s.bg(hca(0x125A2A, 250)).border_color(rgb(GREEN)))
                .child(label)
        };
        let bar = div()
            .id("mx-center-bar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(40.))
            .px(px(PAD))
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(hca(0x010A03, 240))
            .border_b_1()
            .border_color(hca(GREEN_DIM, 200))
            .opacity(if typing { 1. } else { 0. })
            .group_hover("mx-center", |s| s.opacity(1.))
            .child(div().flex_1())
            .child(
                button("mx-bar-open", "Open session ›", false).on_click(cx.listener(
                    move |_, _, _, cx| {
                        cx.emit(WarRoomEvent::OpenSession(open_id.clone()));
                    },
                )),
            );
        // The chat line: always open while an agent is on the center screen. It lies over the
        // foot of the transcript and never changes the size of the screen.
        let overlay = div()
            .id("mx-chat")
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .bottom(px(0.))
            .h(px(64.))
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .right(px(14.))
                    .h(px(46.))
                    .bottom(px(10.))
                    // A clear band: a bright line on top, a darker field, a frame.
                    .border_1()
                    .border_t_2()
                    .border_color(hca(if typing { GREEN_HI } else { GREEN }, 230))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .bg(hca(0x03200E, 250))
                    .text_size(ui::ts(T_ENTRY))
                    .text_color(rgb(GREEN))
                    .child("chat>")
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_color(rgb(GREEN_HI))
                            .child(steer.clone()),
                    )
                    .child(
                        div()
                            .text_size(ui::ts(T_LABEL))
                            .text_color(rgb(GREEN_DIM))
                            .child("↵ send · esc back"),
                    ),
            );
        let mut parts = vec![(
            format!("{} · {} · {}", pane.provider, pane.codebase, name(pane)),
            GREEN_HI,
        )];
        if waits {
            parts.push(("· waits for decision".into(), FOOT));
        }
        bezel(GREEN_MID)
            .flex_1()
            .min_h(px(0.))
            .child(
                glass(false)
                    .group("mx-center")
                    .when(arrive.is_none(), |d| d.child(body))
                    .children(arrive_el)
                    .child(bar)
                    .child(overlay)
                    .child(glaze(false)),
            )
            .child(foot(&no.to_string(), GREEN, parts, 30.))
    }

    /// Green: one agent that works now, as code rain made of its own output. The speed of the
    /// rain is the activity of the agent. A click opens its session.
    fn operator_tile(&mut self, pane: &Pane, idx: usize, t: f32, cx: &mut Context<Self>) -> Div {
        let id = pane.id.clone();
        let source = display_lines(pane, 24).join(" ");
        let seed = hash_str(&pane.id);
        let act = activity(pane);
        let (tint, word) = state_color(pane.state);
        // Hover: the rain turns into the console output in 0.7 s.
        let reveal_p = self
            .operator
            .hover
            .as_ref()
            .filter(|(h, _)| h == &pane.id)
            .map(|(_, at)| (at.elapsed().as_secs_f32() / 0.7).min(1.))
            // The agent on the center screen keeps its console open on its own screen too.
            .or_else(|| (self.operator.active.as_deref() == Some(pane.id.as_str())).then_some(1.));
        let body = match reveal_p {
            Some(p) => reveal(
                &source,
                display_lines(pane, 40),
                act,
                p,
                t,
                seed,
                tint,
                MX_SMALL,
                12.,
            ),
            None => rain(&source, act, 0.6, t, seed, tint),
        };
        let on_center = self.operator.active.as_deref() == Some(pane.id.as_str());
        let hover_id = pane.id.clone();
        let mut parts = vec![(name(pane), 0xB9C4BB), (format!("· {word}"), tint)];
        if on_center {
            parts.push(("· on center".to_string(), GREEN));
        }
        bezel(if on_center { GREEN } else { GREEN_MID })
            .flex_1()
            .min_w(px(0.))
            .child(screen(
                false,
                div()
                    .id(eid(format!("mx-tile-{idx}")))
                    .absolute()
                    .inset_0()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                        if *on {
                            this.operator.hover = Some((hover_id.clone(), Instant::now()));
                        } else if this
                            .operator
                            .hover
                            .as_ref()
                            .is_some_and(|(h, _)| h == &hover_id)
                        {
                            this.operator.hover = None;
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.operator.active.as_deref() != Some(id.as_str()) {
                            this.operator.active_at = Some(Instant::now());
                        }
                        this.operator.active = Some(id.clone());
                        this.operator.to_bottom = true;
                        cx.notify();
                    }))
                    .child(body),
            ))
            .child(foot(&(idx + 1).to_string(), tint, parts, 30.))
    }

    /// The scroll bar under the agent screens.
    fn operator_scroll(
        &mut self,
        n: usize,
        page: usize,
        pages: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let first = (page * PER_PAGE + 1).min(n);
        let last = ((page + 1) * PER_PAGE).min(n);
        let button = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .flex_none()
                .w(px(24.))
                .h(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(rgb(GREEN_MID))
                .hover(|h| h.text_color(rgb(GREEN_HI)))
                .child(glyph)
        };
        let track = self.operator.track.clone();
        div()
            .id("mx-bar")
            .flex_none()
            .h(px(18.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(ui::ts(T_LABEL))
            .text_color(rgb(GREEN_MID))
            .on_scroll_wheel(cx.listener(Self::operator_wheel))
            .child(
                button("mx-prev", "◂").on_click(cx.listener(|this, _, _, cx| {
                    this.operator.page = this.operator.page.saturating_sub(1);
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id("mx-track")
                    .relative()
                    .flex_1()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            this.operator.drag = Some(Drag::Pages);
                            this.operator_seek(e.position.x, pages, cx);
                        }),
                    )
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(6.))
                            .bg(rgb(0x0B2414))
                            .border_1()
                            .border_color(rgb(0x124A25))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-1.))
                                    .left(relative(page as f32 / pages as f32))
                                    .w(relative(1. / pages as f32))
                                    .h(px(6.))
                                    .bg(rgb(GREEN_MID)),
                            ),
                    )
                    .child(
                        canvas(move |bounds, _, _| track.set(bounds), |_, _, _, _| ())
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                    ),
            )
            .child(
                button("mx-next", "▸").on_click(cx.listener(move |this, _, _, cx| {
                    this.operator.page = (this.operator.page + 1).min(pages - 1);
                    cx.notify();
                })),
            )
            .child(format!("working now {first}–{last} of {n}"))
    }

    /// The scroll geometry of the center console: (max offset, thumb share, thumb top share).
    fn operator_center_geo(&self) -> (f32, f32, f32) {
        let s = &self.operator.center_scroll;
        let max = f32::from(s.max_offset().y).max(0.);
        let view = f32::from(s.bounds().size.height).max(1.);
        let share = (view / (view + max)).clamp(0.08, 1.);
        let at = if max > 0. {
            (-f32::from(s.offset().y) / max).clamp(0., 1.)
        } else {
            0.
        };
        (max, share, at * (1. - share))
    }

    /// Drag the center thumb by `dy` pixels from a start offset.
    fn operator_center_drag(&mut self, dy: f32, from_off: f32) {
        let (max, share, _) = self.operator_center_geo();
        let track = f32::from(self.operator.center_track.get().size.height);
        let room = (track * (1. - share)).max(1.);
        let off = (from_off - dy * max / room).clamp(-max, 0.);
        self.operator
            .center_scroll
            .set_offset(gpui::point(px(0.), px(off)));
    }

    /// The vertical scroll bar of the center console: a clear track and a thumb that you can
    /// drag; a click on the track jumps there.
    fn operator_center_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (max, share, top) = self.operator_center_geo();
        let probe = self.operator.center_track.clone();
        let track = self.operator.center_track.clone();
        let scroll = self.operator.center_scroll.clone();
        div()
            .id("mx-center-track")
            .absolute()
            .top(px(46.))
            .bottom(px(36.))
            .right(px(4.))
            .w(px(10.))
            .bg(hca(0x0B2414, 230))
            .border_1()
            .border_color(hca(GREEN_DIM, 200))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let b = track.get();
                    let h = f32::from(b.size.height).max(1.);
                    let y = (f32::from(e.position.y) - f32::from(b.origin.y)) / h;
                    let (max, share, top) = this.operator_center_geo();
                    // Off the thumb: jump so that the thumb centres on the click.
                    if y < top || y > top + share {
                        let at = ((y - share / 2.) / (1. - share).max(0.01)).clamp(0., 1.);
                        scroll.set_offset(gpui::point(px(0.), px(-at * max)));
                    }
                    this.operator.drag = Some(Drag::Center {
                        from_y: f32::from(e.position.y),
                        from_off: f32::from(scroll.offset().y),
                    });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                canvas(move |b, _, _| probe.set(b), |_, _, _, _| ())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(1.))
                    .right(px(1.))
                    .top(relative(if max > 0. { top } else { 0. }))
                    .h(relative(if max > 0. { share } else { 1. }))
                    .bg(rgb(GREEN_MID))
                    .hover(|s| s.bg(rgb(GREEN))),
            )
    }

    /// A click or a drag on the track: go to the page at that x position.
    fn operator_seek(&mut self, x: Pixels, pages: usize, cx: &mut Context<Self>) {
        let b = self.operator.track.get();
        let w = f32::from(b.size.width);
        if w <= 0. {
            return;
        }
        let frac = ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0., 0.999);
        let page = ((frac * pages as f32) as usize).min(pages - 1);
        if page != self.operator.page {
            self.operator.page = page;
            cx.notify();
        }
    }

    /// The mouse wheel over the screens or the bar turns the pages.
    fn operator_wheel(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(war) = self.war.clone() else {
            return;
        };
        let pages = working(&live(&war)).len().div_ceil(PER_PAGE).max(1);
        let d = e.delta.pixel_delta(px(16.));
        let step = if d.y < px(0.) || d.x < px(0.) {
            1
        } else if d.y > px(0.) || d.x > px(0.) {
            -1
        } else {
            return;
        };
        let page = (self.operator.page as isize + step).clamp(0, pages as isize - 1) as usize;
        if page != self.operator.page {
            self.operator.page = page;
            cx.notify();
        }
    }

    /// Keys of the wall: arrows and digits pick and answer, brackets switch the decision,
    /// left and right turn the pages of agent screens.
    pub(in crate::warroom) fn operator_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(war) = self.war.clone() else {
            return;
        };
        // The prompt line has the keyboard: only Escape gives it back to the wall.
        if self.operator.typing {
            if key == "escape" {
                self.operator.blur = true;
                cx.notify();
            }
            return;
        }
        let queue = asks(&war);
        let at = self.operator_at(&queue);
        let pages = working(&live(&war)).len().div_ceil(PER_PAGE).max(1);
        let options = at
            .and_then(|i| queue[i].ask.as_ref())
            .map_or(0, |a| a.options.len().min(4));
        match key {
            "up" => self.operator.opt = self.operator.opt.saturating_sub(1),
            "down" => self.operator.opt = (self.operator.opt + 1).min(options.saturating_sub(1)),
            "[" | "]" if !queue.is_empty() => {
                let n = queue.len();
                let i = at.unwrap_or(0);
                let next = if key == "]" {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                };
                self.operator.pick = Some(queue[next].id.clone());
                self.operator.opt = 0;
            }
            "escape" => self.operator.active = None,
            "o" => {
                if let Some(id) = self.operator.active.clone() {
                    cx.emit(WarRoomEvent::OpenSession(id));
                }
            }
            "left" => self.operator.page = self.operator.page.saturating_sub(1),
            "right" => self.operator.page = (self.operator.page + 1).min(pages - 1),
            "w" => {
                if let Some(i) = at {
                    cx.emit(WarRoomEvent::OpenSession(queue[i].id.clone()));
                }
            }
            _ => {
                let chosen = match key {
                    "enter" => Some(self.operator.opt),
                    "1" | "2" | "3" | "4" => key.parse::<usize>().ok().map(|n| n - 1),
                    _ => None,
                };
                if let (Some(i), Some(c)) = (at, chosen) {
                    let pane = queue[i];
                    if let Some(ask) = &pane.ask
                        && let Some(option) = ask.options.get(c)
                    {
                        self.operator.opt = 0;
                        self.skin_answer(&pane.id, &ask.id, option, cx);
                    }
                }
            }
        }
        cx.notify();
    }
}

/// A dark screen of the bottom row: no agent works for this slot.
fn operator_dark(idx: usize, say: bool) -> Div {
    let mut body = div().absolute().inset_0().p(px(PAD));
    if say {
        body = body.text_color(rgb(GREEN_DIM)).child("No agent works now.");
    }
    bezel(GREEN_DIM)
        .flex_1()
        .min_w(px(0.))
        .child(screen(false, body))
        .child(foot(&(idx + 1).to_string(), FOOT, vec![], 30.))
}

/// Blue: agents by state for the last 60 minutes, stacked bars that grow in.
fn operator_fleet(war: &WarRoom, live_count: usize, t: f32) -> Div {
    let samples: Vec<_> = war.activity.iter().rev().take(36).rev().copied().collect();
    let peak = samples
        .iter()
        .map(|s| s.active + s.waiting + s.idle)
        .max()
        .unwrap_or(1)
        .max(1);
    let unit = PLOT_H / peak as f32;
    let mut bars = div()
        .absolute()
        .left(px(PAD))
        .right(px(PAD))
        .top(px(58.))
        .h(px(PLOT_H))
        .flex()
        .items_end()
        .gap(px(3.));
    for (k, s) in samples.iter().enumerate() {
        let grow = ease((t - k as f32 * 0.02) / 0.8);
        bars = bars.child(
            div()
                .flex_1()
                .h(px(PLOT_H))
                .flex()
                .flex_col()
                .justify_end()
                .child(div().h(px(s.idle as f32 * unit * grow)).bg(rgb(BLUE_DIM)))
                .child(div().h(px(s.waiting as f32 * unit * grow)).bg(rgb(BLUE_HI)))
                .child(
                    div()
                        .h(px(s.active as f32 * unit * grow))
                        .bg(rgb(BLUE_RING)),
                ),
        );
    }
    let empty = samples.is_empty().then(|| {
        div()
            .absolute()
            .left(px(PAD))
            .top(px(80.))
            .text_color(rgb(BLUE_SOFT))
            .child("no samples yet")
    });
    let head = div()
        .absolute()
        .left(px(PAD))
        .right(px(PAD))
        .top(px(PAD - 8.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(BLUE_HI))
                .child("Agents by state"),
        )
        .child(
            div()
                .text_size(ui::ts(T_TITLE))
                .text_color(rgb(BLUE_HI))
                .child(live_count.to_string()),
        );
    let legend = div()
        .absolute()
        .left(px(PAD))
        .right(px(PAD))
        .bottom(px(8.))
        .flex()
        .justify_between()
        .text_size(ui::ts(T_LABEL))
        .child("■ at work")
        .child(div().text_color(rgb(BLUE_HI)).child("■ waits for you"))
        .child(div().text_color(rgb(0x3F6E96)).child("■ idle"))
        .child("60 min");
    bezel(BLUE_RING)
        .flex_1()
        .min_h(px(0.))
        .child(
            glass(true)
                .child(head)
                .child(bars)
                .children(empty)
                .child(legend)
                .child(glaze(true)),
        )
        .child(foot("A", FOOT, vec![("last 60 min".into(), FOOT)], 30.))
}

/// Blue: the usage windows of the providers and the spend of the live sessions.
fn operator_budget(war: &WarRoom, all: &[&Pane], t: f32) -> Div {
    let mut body = div()
        .absolute()
        .inset_0()
        .px(px(PAD))
        .py(px(PAD - 8.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .overflow_hidden()
        .text_size(ui::ts(T_ENTRY))
        .child(
            div()
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(BLUE_HI))
                .child("Budget"),
        );
    for (k, u) in war.usage.iter().take(3).enumerate() {
        let reset = u.resets_at.map_or(String::new(), |r| {
            let m = (r - war.now).max(0) / 60_000;
            format!(" · resets {}h {:02}m", m / 60, m % 60)
        });
        let est = if u.estimate {
            "estimate"
        } else {
            "from the provider"
        };
        let frac = f32::from(u.pct.min(100)) / 100. * ease((t - 0.2 - k as f32 * 0.1) / 1.3);
        body = body.child(
            div()
                .flex_none()
                .child(
                    div()
                        .flex()
                        .gap(px(16.))
                        .child(div().flex_1().min_w(px(0.)).child(format!(
                            "{} {}",
                            u.provider,
                            u.label.to_lowercase()
                        )))
                        .child(
                            div()
                                .flex_none()
                                .w(px(72.))
                                .flex()
                                .justify_end()
                                .text_color(rgb(BLUE_HI))
                                .child(format!("{}%", u.pct)),
                        ),
                )
                .child(
                    div()
                        .mt(px(4.))
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(BLUE_SOFT))
                        .child(format!("{est}{reset}")),
                )
                .child(
                    div()
                        .mt(px(6.))
                        .h(px(8.))
                        .bg(rgb(BLUE_DIM))
                        .child(div().h(px(8.)).w(relative(frac)).bg(rgb(BLUE))),
                ),
        );
    }
    let spend: f64 = all.iter().map(|p| p.cost).sum();
    body = body.child(
        div()
            .flex_none()
            .flex()
            .gap(px(16.))
            .child(div().flex_1().child("spend, live sessions"))
            .child(
                div()
                    .flex_none()
                    .w(px(72.))
                    .flex()
                    .justify_end()
                    .text_color(rgb(BLUE_HI))
                    .child(format!("${spend:.2}")),
            ),
    );
    bezel(BLUE_RING)
        .flex_1()
        .min_h(px(0.))
        .child(screen(true, body))
        .child(foot(
            "B",
            FOOT,
            vec![("windows and spend".into(), FOOT)],
            30.,
        ))
}
