//! The Operator look of the war room (`OperatorCore.dc.html`). A wall of monitors: blue screens
//! show information and decisions, green screens show agents. A screen is a console or rain,
//! never both. A busy agent on the wall is code rain made of its own output (speed = how busy
//! it is); an idle agent shows a static prompt with a blinking block; the agent behind the
//! decision is a console. The navigation (F-keys) is in `shell/operator.rs`.
//! `kit` holds the monitor frame and the token streams; the session view uses them too.

pub(crate) mod kit;

use std::time::Instant;

use aivory_protocol::{AskKind, Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, Entity, Subscription, Window, div, px, relative, rgb};
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
    /// The prompt line of the center screen, and the event subscription of its Enter key.
    steer: Option<Entity<InputState>>,
    steer_sub: Option<Subscription>,
    /// The session on the center screen: the target of the prompt line.
    focus_id: Option<String>,
    /// The prompt line has the keyboard: the keys of the wall stay quiet.
    typing: bool,
    /// Give the keyboard back to the wall in the next frame.
    blur: bool,
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
        // The agent behind the decision, else the busiest agent, else the first one.
        let focus = current
            .or_else(|| busy.first().copied())
            .or_else(|| all.first().copied());
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
            .child(self.operator_focus(focus, current.is_some(), &all, &steer, typing, t, cx));
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

        let mut row = div().flex_none().h(px(290.)).flex().gap(px(GAP));
        for slot in 0..PER_PAGE {
            let idx = page * PER_PAGE + slot;
            row = row.child(match busy.get(idx) {
                Some(pane) => self.operator_tile(pane, idx, t, cx),
                None => operator_dark(idx, busy.is_empty() && slot == 0),
            });
        }
        let bar = self.operator_scroll(busy.len(), page, pages, cx);

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .px(px(28.))
            .pt(px(10.))
            .pb(px(10.))
            .bg(rgb(PAGE))
            .font_family(FONT)
            .text_size(ui::ts(T_ENTRY))
            .child(top)
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
                        .child("INCOMING · 0"),
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
                    .child(format!("INCOMING · {} / {}", at + 1, total)),
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
                    .child(format!("{}  {}", i + 1, option.to_uppercase())),
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
                .child("W  WRITE AN ANSWER…"),
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
                    .child("QUEUE"),
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
            return bezel(GREEN_MID)
                .flex_1()
                .min_h(px(0.))
                .child(screen(
                    false,
                    div()
                        .absolute()
                        .inset_0()
                        .p(px(PAD))
                        .child("No agent runs."),
                ))
                .child(foot("-", FOOT, vec![], 30.));
        };
        let no = all
            .iter()
            .position(|p| p.id == pane.id)
            .map_or(1, |i| i + 1);
        let id = pane.id.clone();
        let tail = display_lines(pane, 24);
        let stream = match tail.split_last() {
            Some((newest, old)) if pane.state == SessionState::Working => {
                decoded(&pane.id, old, newest, hash_str(&pane.id), t)
            }
            _ => tokenize(&tail),
        };
        let body = div()
            .absolute()
            .inset_0()
            .px(px(PAD))
            .pt(px(PAD))
            .pb(px(PAD + 30.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_size(ui::ts(T_ENTRY))
            .line_height(relative(1.5))
            .child(div().flex_none().child(stream.into_text()));
        // The prompt overlay: its hover area is fixed; the bar slides in over the foot of the
        // transcript and never changes the size of the screen.
        let folded = !typing;
        let overlay = div()
            .id("mx-chat")
            .group("mx-chat")
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .bottom(px(0.))
            .h(px(64.))
            .child(
                div()
                    .absolute()
                    .left(px(PAD))
                    .bottom(px(5.))
                    .text_size(ui::ts(T_LABEL))
                    .text_color(rgb(GREEN_DIM))
                    .child("▲ write to this session"),
            )
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .right(px(14.))
                    .h(px(46.))
                    .bottom(px(if folded { -52. } else { 10. }))
                    .group_hover("mx-chat", |s| s.bottom(px(10.)))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .bg(hca(0x010A03, 245))
                    .text_size(ui::ts(T_ENTRY))
                    .text_color(rgb(GREEN_MID))
                    .child("you>")
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
                    .child(
                        div()
                            .id("mx-focus")
                            .absolute()
                            .inset_0()
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(WarRoomEvent::OpenSession(id.clone()));
                            }))
                            .child(body),
                    )
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
        let body = rain(&source, activity(pane), 0.6, t, seed);
        // The newest steps in front of the rain: written text, it never changes.
        let mut steps = div()
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .bottom(px(0.))
            .px(px(16.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .bg(hca(0x010A03, 226))
            .text_size(ui::ts(T_LABEL + 1.))
            .line_height(relative(1.35))
            .child(
                div()
                    .text_size(ui::ts(T_LABEL))
                    .text_color(rgb(GREEN_PATH))
                    .child(format!("{} · {}", pane.codebase, pane.provider)),
            );
        for line in step_lines(pane, 3) {
            steps = steps.child(
                div()
                    .flex_none()
                    .max_h(px(2. * 16.5))
                    .overflow_hidden()
                    .text_color(rgb(GREEN_HI))
                    .child(line),
            );
        }
        bezel(GREEN_MID)
            .flex_1()
            .min_w(px(0.))
            .child(screen(
                false,
                div()
                    .id(eid(format!("mx-tile-{idx}")))
                    .absolute()
                    .inset_0()
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(WarRoomEvent::OpenSession(id.clone()));
                    }))
                    .child(body)
                    .child(steps),
            ))
            .child(foot(
                &(idx + 1).to_string(),
                GREEN,
                vec![(name(pane), 0xB9C4BB), ("· working".to_string(), FOOT)],
                30.,
            ))
    }

    /// The scroll bar under the agent screens.
    fn operator_scroll(
        &mut self,
        n: usize,
        page: usize,
        pages: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let first = (page * PER_PAGE + 1).min(n);
        let last = ((page + 1) * PER_PAGE).min(n);
        let button = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .cursor_pointer()
                .text_color(rgb(GREEN_MID))
                .hover(|h| h.text_color(rgb(GREEN_HI)))
                .child(glyph)
        };
        div()
            .flex_none()
            .h(px(16.))
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(ui::ts(T_LABEL))
            .text_color(rgb(GREEN_MID))
            .child(
                button("mx-prev", "◂").on_click(cx.listener(|this, _, _, cx| {
                    this.operator.page = this.operator.page.saturating_sub(1);
                    cx.notify();
                })),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
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
                button("mx-next", "▸").on_click(cx.listener(move |this, _, _, cx| {
                    this.operator.page = (this.operator.page + 1).min(pages - 1);
                    cx.notify();
                })),
            )
            .child(format!("working now {first}–{last} of {n}"))
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
                .child("AGENTS BY STATE"),
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
        .child(foot("A", FOOT, vec![("fleet".into(), FOOT)], 30.))
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
                .child("BUDGET"),
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
