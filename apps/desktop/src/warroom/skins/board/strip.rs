//! The strip under the gates (Run log, Queue, Live) and the pointer handling of the line band:
//! hover on a line highlights it and shows its session; a click opens the Focus view.

use aivory_protocol::WarRoom;
use gpui::prelude::*;
use gpui::{Bounds, Context, Div, FontWeight, MouseMoveEvent, Pixels, canvas, div, point, px, rgb};

use super::{Gates, NEEDS, WORKING, doing, short_id, split, state_tone, stream};
use crate::theme::*;
use crate::ui;
use crate::warroom::{WarRoomEvent, WarRoomScreen};

/// The most height of the strip under the gates.
const STRIP_H: f32 = 168.;

/// A card with the one soft shadow.
fn panel(grow: f32) -> Div {
    div()
        .bg(rgb(tk().card))
        .rounded(tk().r_card)
        .shadow(ui::card_shadow())
        .flex_basis(px(0.))
        .flex_grow(grow)
        .min_w(px(0.))
        .min_h(px(0.))
        .px(px(14.))
        .py(px(12.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .overflow_hidden()
}

/// The head of a strip panel: a title, a value, and a note on the right.
fn head(title: &'static str, value: Option<String>, right: Option<String>) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(ui::ts(T_CAPTION))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(tk().ink_3))
        .child(title)
        .when_some(value, |d, v| {
            d.child(
                div()
                    .font_family(tk().mono)
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(tk().ink))
                    .child(v),
            )
        })
        .when_some(right, |d, r| {
            d.child(div().ml_auto().font_weight(FontWeight::NORMAL).child(r))
        })
}

/// A line chart of the numbers, as wide as its box and 34 px high.
fn sparkline(values: Vec<f32>) -> Div {
    let host = div().flex_none().w_full().h(px(34.));
    if values.len() < 2 {
        return host;
    }
    host.child(
        canvas(
            move |_, _, _| values,
            move |bounds: Bounds<Pixels>, values, window, _| {
                let top = values.iter().copied().fold(1., f32::max);
                let n = values.len() - 1;
                let mut path = gpui::PathBuilder::stroke(px(1.6));
                for (i, v) in values.iter().enumerate() {
                    let x = bounds.origin.x + bounds.size.width * (i as f32 / n as f32);
                    let y = bounds.origin.y + bounds.size.height * (1. - 0.9 * v / top) - px(1.);
                    if i == 0 {
                        path.move_to(point(x, y));
                    } else {
                        path.line_to(point(x, y));
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(tk().work));
                }
            },
        )
        .size_full(),
    )
}

impl WarRoomScreen {
    /// The line band with its pointer handling and the hover note.
    pub(super) fn render_band(
        &self,
        war: &WarRoom,
        band: Div,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let note = self.board.hover_line.as_ref().and_then(|(id, x, y)| {
            let pane = war.session(id)?;
            let what = doing(&pane.tail).unwrap_or_else(|| "No tool yet".to_string());
            Some((pane.title.clone(), what, pane.state, *x, *y))
        });
        let host = div()
            .id("line-band")
            .relative()
            .flex_none()
            .child(band)
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let hit = this.line_at(e.position);
                if hit.as_ref().map(|h| &h.0) != this.board.hover_line.as_ref().map(|h| &h.0) {
                    this.board.hover_line = hit;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.board.hover_line.take().is_some() {
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some((id, ..)) = &this.board.hover_line {
                    cx.emit(WarRoomEvent::Focus(id.clone()));
                }
            }))
            .when(self.board.hover_line.is_some(), |d| d.cursor_pointer());
        let Some((title, what, state, x, y)) = note else {
            return host;
        };
        // The note sits beside the pointer, inside the band.
        let left = (x + 14.).max(0.);
        let top = (y - 40.).clamp(0., stream::HEIGHT - 44.);
        host.child(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .max_w(px(360.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .bg(rgb(tk().card))
                .shadow(ui::popover_shadow())
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(ui::ts(T_SMALL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .child(ui::dot(rgb(state_tone(state))))
                        .child(title),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_family(tk().mono)
                        .text_size(ui::ts(T_CAPTION))
                        .text_color(rgb(tk().ink_3))
                        .child(what),
                ),
        )
    }

    /// The session of the line under a window point, with the point in band coordinates.
    fn line_at(&self, at: gpui::Point<Pixels>) -> Option<(String, f32, f32)> {
        let bounds = self.board.band.get()?;
        let war = self.war.clone()?;
        let x = f32::from(at.x - bounds.origin.x);
        let y = f32::from(at.y - bounds.origin.y);
        let gates = split(&war, self.board.module.as_deref());
        let ribbons = stream::ribbons(&gates, None);
        let i = stream::hit(&ribbons, f32::from(bounds.size.width), x, y)?;
        Some((ribbons[i].session.clone(), x, y))
    }

    /// The strip: Run log, Queue, Live.
    pub(super) fn render_strip(
        &self,
        war: &WarRoom,
        gates: &Gates<'_>,
        _cx: &mut Context<Self>,
    ) -> Div {
        // Run log: the newest lines at the bottom, as many as fit.
        let mut log = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .font_family(tk().mono)
            .text_size(ui::ts(T_CAPTION));
        for entry in self.board.log.iter().rev().take(5).rev() {
            let color = war
                .session(&entry.session)
                .map_or(tk().ink_3, |s| state_tone(s.state));
            log = log.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(rgb(tk().ink_2))
                    .child(
                        div()
                            .flex_none()
                            .w(px(70.))
                            .text_color(rgb(tk().ink_3))
                            .child(entry.ts.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(36.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(color))
                            .child(short_id(&entry.session)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(entry.text.clone()),
                    ),
            );
        }
        let epm = self.board.events.len();

        // Queue: the tasks that wait for a free slot, and the slots in use.
        let used = gates.lists[WORKING].len();
        let queue = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .font_family(tk().display_figure)
                    .text_size(ui::ts(26.))
                    .line_height(px(28.))
                    .child(war.queued.to_string()),
            )
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(if war.queued == 0 {
                        "No task waits for a slot."
                    } else if war.autopilot.on {
                        "Autopilot starts them when a slot is free."
                    } else {
                        "Autopilot is off. Start them in Tasks."
                    }),
            );

        // Live: the activity of the last hour and three numbers.
        let spark: Vec<f32> = war
            .activity
            .iter()
            .rev()
            .take(40)
            .rev()
            .map(|a| a.active as f32)
            .collect();
        let waits: Vec<i64> = gates.lists[NEEDS]
            .iter()
            .map(|s| s.minutes(war.now))
            .collect();
        let avg = if waits.is_empty() {
            "none".to_string()
        } else {
            format!(
                "{:.1} min",
                waits.iter().sum::<i64>() as f64 / waits.len() as f64
            )
        };
        let metric = |k: &'static str, v: String, color: u32| {
            div()
                .flex()
                .items_baseline()
                .child(
                    div()
                        .text_size(ui::ts(T_CAPTION))
                        .text_color(rgb(tk().ink_3))
                        .child(k),
                )
                .child(
                    div()
                        .ml_auto()
                        .font_family(tk().mono)
                        .font_weight(FontWeight::BOLD)
                        .text_size(ui::ts(T_SMALL))
                        .text_color(rgb(color))
                        .child(v),
                )
        };
        let live = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(sparkline(spark))
            .child(metric(
                "Agents working",
                gates.lists[WORKING]
                    .iter()
                    .map(|s| s.agents())
                    .sum::<u32>()
                    .to_string(),
                tk().ink,
            ))
            .child(metric(
                "Spent today",
                format!("${:.2}", self.shown.1),
                tk().ink,
            ))
            .child(metric("Average wait for you", avg, tk().q_text));

        div()
            .flex_none()
            .h(px(STRIP_H))
            .flex()
            .gap(px(super::GAP))
            .child(
                panel(4.)
                    .child(head("RUN LOG", Some(format!("{epm}/min")), None))
                    .child(log),
            )
            .child(
                panel(3.)
                    .child(head(
                        "QUEUE",
                        None,
                        Some(format!("{used}/{} slots", war.autopilot.max_running)),
                    ))
                    .child(queue),
            )
            .child(panel(3.).child(head("LIVE", None, None)).child(live))
    }
}
