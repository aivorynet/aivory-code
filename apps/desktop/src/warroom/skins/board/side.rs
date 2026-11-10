//! The right sidebar of the board: the Overview. Big numbers on top (tokens, spend, agents
//! live) with the activity graph and its time axis, then the providers with usage this week.
//! Regular and Wide dock the sidebar; Compact shows it as an overlay.

use aivory_protocol::WarRoom;
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Stateful, div, px, relative, rgb};

use super::{WidthClass, active_providers, side_width, width_class};
use crate::theme::*;
use crate::ui;
use crate::warroom::WarRoomScreen;
use crate::warroom::band::{fmt_tokens, until};

/// The width of the number column of the provider table.
const NUM_W: f32 = 76.;

/// A section head: a title in the heading face.
fn section(title: &'static str) -> Div {
    div()
        .flex_none()
        .pt(px(28.))
        .pb(px(8.))
        .mb(px(6.))
        .border_b_1()
        .border_color(ui::hsla(tk().line))
        .font_family(tk().display_heading)
        .text_size(ui::ts(T_TITLE))
        .text_color(rgb(tk().ink))
        .child(title)
}

/// The number style of the provider table: mono, right aligned.
fn num(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .min_w(px(NUM_W))
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .font_family(tk().mono)
        .text_size(ui::ts(T_SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(tk().ink))
        .child(text.into())
}

/// A big number in the display face with its unit.
fn big(value: String, size: f32, unit: &'static str) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(8.))
        .child(
            div()
                .font_family(if size >= 40. {
                    tk().display_figure
                } else {
                    tk().display
                })
                .text_size(ui::ts(size))
                .line_height(px(size * 1.1))
                .text_color(rgb(tk().ink))
                .child(value),
        )
        .child(
            div()
                .text_size(ui::ts(T_SMALL))
                .text_color(rgb(tk().ink_3))
                .child(unit),
        )
}

impl WarRoomScreen {
    /// The sidebar of the board: `(docked, overlay)`. Compact gives an overlay only while the
    /// user opened it.
    pub(in crate::warroom) fn render_board_side(
        &mut self,
        war: &WarRoom,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        let class = width_class(f32::from(self.width));
        let compact = class == WidthClass::Compact;
        if compact && !self.board.side_open {
            return (None, None);
        }
        let panel = div()
            .flex_none()
            .w(px(side_width(class)))
            .h_full()
            .bg(rgb(tk().card))
            .rounded_tl(px(18.))
            .when(compact, |d| d.rounded_bl(px(18.)))
            .shadow(ui::side_shadow(-8.))
            .overflow_hidden()
            .relative()
            .child(self.overview_panel(war, cx))
            .children(super::parts::more_below(&self.board.side_scroll))
            .child(ui::thumb(
                &self.board.side_scroll,
                gpui::rgba(tk().ink_3 << 8 | 0x80),
                3.,
            ));
        if !compact {
            return (Some(panel.into_any_element()), None);
        }
        let overlay = div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("board-scrim")
                    .absolute()
                    .inset_0()
                    .bg(ui::hsla(tk().ink << 8 | 0x1F))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.board.side_open = false;
                        cx.notify();
                    })),
            )
            .child(div().absolute().top_0().bottom_0().right_0().child(panel));
        (None, Some(overlay.into_any_element()))
    }

    /// Memory: RAM used of the machine over the last 30 min (one bar per 30 s), with the share
    /// of the agents, and the agent processes that use the most.
    fn memory_view(&self) -> Div {
        let Some(m) = self.memory.as_ref().filter(|m| m.total_mb > 0) else {
            return div()
                .text_size(ui::ts(T_SMALL))
                .text_color(rgb(tk().ink_3))
                .child("Reading the RAM…");
        };
        let gb = |mb: u32| format!("{:.1} GB", f64::from(mb) / 1024.);
        let free = m.total_mb.saturating_sub(m.used_mb);
        let low = free * 100 < m.total_mb * 15;
        let mut out = div().flex().flex_col().gap(px(8.)).child(
            div()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(10.))
                .gap_y(px(2.))
                .child(big(gb(m.used_mb), 24., "used"))
                .child(
                    div()
                        .text_size(ui::ts(T_SMALL))
                        .text_color(rgb(if low { tk().q_text } else { tk().ink_3 }))
                        .child(format!(
                            "{} free of {} · agents {}",
                            gb(free),
                            gb(m.total_mb),
                            gb(m.agents_mb)
                        )),
                ),
        );
        // One bar per 6 samples: the height is RAM used, the dark part the agents.
        const H: f32 = 56.;
        let mut bars = div()
            .h(px(H))
            .flex()
            .items_end()
            .gap(px(1.))
            .border_b_1()
            .border_color(ui::hsla(tk().line));
        for chunk in m.samples.chunks(6) {
            let used = chunk.iter().map(|s| s.used_mb).max().unwrap_or(0);
            let agents = chunk.iter().map(|s| s.agents_mb).max().unwrap_or(0);
            let total = m.total_mb.max(1) as f32;
            let h = H * used as f32 / total;
            let a = (H * agents as f32 / total).min(h);
            bars = bars.child(
                div()
                    .flex_1()
                    .max_w(px(6.))
                    .h(px(h))
                    .flex()
                    .flex_col()
                    .justify_end()
                    .bg(rgb(tk().track_2))
                    .child(div().h(px(a)).bg(rgb(tk().ink_3))),
            );
        }
        out = out.child(bars).child(
            div()
                .flex()
                .justify_between()
                .text_size(ui::ts(T_CAPTION))
                .text_color(rgb(tk().ink_3))
                .child("30 min")
                .child("now"),
        );
        for a in &m.top {
            out = out.child(
                div()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(ui::ts(T_SMALL))
                    .child(div().flex_1().child(format!("{} · pid {}", a.name, a.pid)))
                    .child(num(gb(a.mb))),
            );
        }
        out
    }

    /// Organizer: the four modes and the start button. The organizer is a session of
    /// aivoryd that watches RAM and the other sessions (`organizer/start`).
    fn organizer_view(&self, cx: &mut Context<Self>) -> Div {
        let mut modes = div().flex().flex_wrap().gap(px(6.));
        for (i, mode) in aivory_protocol::OrganizerMode::ALL.into_iter().enumerate() {
            let on = self.organizer == mode;
            modes = modes.child(
                div()
                    .id(("organizer-mode", i))
                    .h(px(28.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(if on { tk().ink } else { tk().edge }))
                    .bg(rgb(if on { tk().ink } else { tk().card }))
                    .text_color(rgb(if on { tk().card } else { tk().ink }))
                    .text_size(ui::ts(T_SMALL))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.organizer = mode;
                        cx.notify();
                    }))
                    .child(mode.label()),
            );
        }
        let running = self.war.as_ref().is_some_and(|w| {
            w.sessions
                .iter()
                .any(|s| s.state.is_live() && s.title.starts_with("Organizer ·"))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(if running {
                        "An organizer runs. Its reports show in its session."
                    } else {
                        aivory_protocol::ORGANIZER_INFO
                    }),
            )
            .child(modes)
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child(self.organizer.about()),
            )
            .child(
                ui::dark_button("organizer-start", "Start organizer").on_click(cx.listener(
                    |this, _, _, cx| {
                        let call = this
                            .model
                            .read(cx)
                            .call_notify::<aivory_protocol::rpc::method::OrganizerStart>(
                                aivory_protocol::OrganizerStartParams {
                                    mode: this.organizer,
                                    codebase: None,
                                    composer: None,
                                },
                                cx,
                            );
                        cx.spawn(async move |_, _| {
                            let _ = call.await;
                        })
                        .detach();
                    },
                )),
            )
    }

    /// The Overview: big numbers, the activity graph, the providers.
    fn overview_panel(&self, war: &WarRoom, cx: &mut Context<Self>) -> Stateful<Div> {
        let start = war.activity.first().map_or(war.now, |a| a.at);
        let since = self.model.read(cx).clock(start);
        let minutes = (war.now - start).max(0) / 60_000;
        let span = if minutes >= 60 {
            format!("{} h {} min", minutes / 60, minutes % 60)
        } else {
            format!("{minutes} min")
        };
        let counts = war.counts();
        let tokens = self.shown.0.round() as u64;
        let top = div()
            .flex()
            .flex_col()
            .pt(px(28.))
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("Since {since} · {span}")),
            )
            .child(div().mt(px(8.)).child(if tokens > 0 {
                big(fmt_tokens(tokens), 46., "tokens")
            } else {
                big(format!("${:.2}", self.shown.1), 46., "spent")
            }))
            .child(
                div()
                    .mt(px(14.))
                    .flex()
                    .gap(px(24.))
                    .when(tokens > 0, |d| {
                        d.child(big(format!("${:.2}", self.shown.1), 24., "spent"))
                    })
                    .child(big(counts.agents.to_string(), 24., "agents live")),
            )
            .child(self.spark(war))
            .child(
                div()
                    .mt(px(8.))
                    .flex()
                    .justify_between()
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(since)
                    .child("now"),
            );

        let mut out = div()
            .id("board-overview")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.board.side_scroll)
            .px(px(24.))
            .pb(px(32.))
            .flex()
            .flex_col()
            .child(top)
            .child(section("Memory"))
            .child(self.memory_view())
            .child(section("Organizer"))
            .child(self.organizer_view(cx))
            .child(section("Providers"));
        let names = active_providers(war);
        if names.is_empty() {
            return out.child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child("No usage this week"),
            );
        }
        out = out.child(
            div()
                .h(px(22.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(ui::ts(T_CAPTION))
                .text_color(rgb(tk().ink_3))
                .child(div().flex_1().child("Provider"))
                .child(div().w(px(NUM_W)).flex().justify_end().child("Tokens"))
                .child(div().w(px(NUM_W)).flex().justify_end().child("Cost")),
        );
        for name in names {
            let of = war.sessions.iter().filter(|s| s.provider == name);
            let tokens: u64 = of.clone().map(|s| s.tokens).sum();
            let cost = of.map(|s| s.cost).sum::<f64>() + 0.0;
            let color = provider_color(&name);
            out = out.child(
                div()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(ui::ts(T_SMALL))
                    .child(ui::dot(rgb(color)))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name.clone()),
                    )
                    .child(num(fmt_tokens(tokens)))
                    .child(num(format!("${cost:.2}"))),
            );
            for w in war.usage.iter().filter(|w| w.provider == name) {
                let mut note = Vec::new();
                if let Some(at) = w.resets_at {
                    note.push(format!(
                        "resets {}, {}",
                        self.model.read(cx).clock(at),
                        until(at - war.now)
                    ));
                }
                if w.estimate {
                    note.push("estimate".to_string());
                }
                let label = w.label.clone();
                out = out.child(
                    div()
                        .id(SharedString::from(format!("win-{name}-{label}")))
                        .pl(px(16.))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(ui::ts(T_CAPTION))
                        .text_color(rgb(tk().ink_3))
                        .when(!note.is_empty(), |d| d.tooltip(ui::tip(note.join(" · "))))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(label),
                        )
                        .child(
                            div()
                                .w(px(NUM_W))
                                .h(px(4.))
                                .rounded_full()
                                .bg(rgb(tk().track))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(f32::from(w.pct.min(100)) / 100.))
                                        .rounded_full()
                                        .bg(rgb(if w.pct >= 90 { tk().q } else { color })),
                                ),
                        )
                        .child(num(format!("{}%", w.pct))),
                );
            }
        }
        out
    }
}
