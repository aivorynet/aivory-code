//! The Overview column of war room v2: tokens, spend, live agents, agents per minute, per
//! provider its tokens, cost and usage windows with their reset times, and the Sankeys.

use aivory_protocol::layout::{self, FlowBox, FlowColumn, Sankey};
use aivory_protocol::{SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    Bounds, Context, Div, FontWeight, PathBuilder, Pixels, SharedString, canvas, div, fill, point,
    px, relative, rgb,
};

use super::{Overlay, WarRoomScreen};
use crate::theme::*;
use crate::ui;

impl WarRoomScreen {
    /// The Overview column (`.rs`). The providers show tokens, cost, and their windows
    /// with the reset times; the Sankeys stay under them.
    pub(super) fn render_overview(&self, war: &WarRoom, width: f32, cx: &mut Context<Self>) -> Div {
        let counts = war.counts();
        // The eased values: a change counts to the new number.
        let spent = self.shown.1;
        let tokens = self.shown.0.round() as u64;
        let start = war.activity.first().map_or(war.now, |a| a.at);
        let since = self.model.read(cx).clock(start);
        let minutes = (war.now - start).max(0) / 60_000;
        let span = if minutes >= 60 {
            format!("{} h {} min", minutes / 60, minutes % 60)
        } else {
            format!("{minutes} min")
        };
        let big = |text: String, size: f32| {
            div()
                .font_family(if size >= 40. {
                    tk().display_figure
                } else {
                    tk().display
                })
                .font_weight(FontWeight::NORMAL)
                .text_size(ui::ts(size))
                .line_height(px(size * 1.1))
                .text_color(rgb(tk().ink))
                .child(text)
        };
        let unit = |text: &'static str| {
            div()
                .text_size(ui::ts(T_SMALL))
                .text_color(rgb(tk().ink_3))
                .child(text)
        };
        let stat = |value: String, label: &'static str| {
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .child(big(value, 24.))
                .child(unit(label))
        };
        let top = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("Since {since} · {span}")),
            )
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .map(|d| {
                        // Without token counts the spend is the big number.
                        if tokens > 0 {
                            d.child(big(fmt_tokens(tokens), 46.)).child(unit("tokens"))
                        } else {
                            d.child(big(format!("${spent:.2}"), 46.))
                                .child(unit("spent"))
                        }
                    }),
            )
            .child(
                div()
                    .mt(px(16.))
                    .grid()
                    .grid_cols(2)
                    .gap(px(16.))
                    .map(|d| {
                        if tokens > 0 {
                            d.child(stat(format!("${spent:.2}"), "spent"))
                        } else {
                            d.child(stat(counts.live.to_string(), "sessions"))
                        }
                    })
                    .child(stat(counts.agents.to_string(), "agents live")),
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
        let mut providers = div()
            .flex()
            .flex_col()
            .child(overview_head("Providers", None));
        let mut names: Vec<&str> = Vec::new();
        for name in war
            .sessions
            .iter()
            .map(|s| s.provider.as_str())
            .chain(war.usage.iter().map(|w| w.provider.as_str()))
        {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let rows: Vec<(&str, u64, f64)> = names
            .iter()
            .map(|&name| {
                let of = war.sessions.iter().filter(|s| s.provider == name);
                let tokens = of.clone().map(|s| s.tokens).sum();
                // `+ 0.0`: an empty sum is -0.0, which prints as "$-0.00".
                let cost = of.map(|s| s.cost).sum::<f64>() + 0.0;
                (name, tokens, cost)
            })
            .collect();
        let by_tokens = rows.iter().any(|r| r.1 > 0);
        let clock = |at: aivory_protocol::UnixMs| self.model.read(cx).clock(at);
        for (name, tokens, cost) in rows {
            let color = provider_color(name);
            let value = if by_tokens {
                fmt_tokens(tokens)
            } else {
                format!("${cost:.2}")
            };
            let mut block = div().flex().flex_col().gap(px(10.)).py(px(10.)).child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .text_size(ui::ts(T_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ui::dot(rgb(color)))
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui::ts(T_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .child(if by_tokens {
                                format!("{value} tokens")
                            } else {
                                value
                            }),
                    )
                    .when(by_tokens, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(ui::ts(T_SMALL))
                                .text_color(rgb(tk().ink_3))
                                .child(format!("${cost:.2}")),
                        )
                    }),
            );
            let windows: Vec<_> = war.usage.iter().filter(|w| w.provider == name).collect();
            if windows.is_empty() {
                block = block.child(
                    div()
                        .pl(px(17.))
                        .text_size(ui::ts(T_SMALL))
                        .text_color(rgb(tk().ink_3))
                        .child("No usage limit known"),
                );
            }
            for w in windows {
                let mut note = Vec::new();
                if w.used > 0 {
                    note.push(format!("{} tokens", fmt_tokens(w.used)));
                }
                if let Some(at) = w.resets_at {
                    note.push(format!("resets {}, {}", clock(at), until(at - war.now)));
                }
                if w.estimate {
                    note.push("estimate".to_string());
                }
                block = block.child(
                    div()
                        .pl(px(17.))
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .text_size(ui::ts(T_SMALL))
                                .child(div().text_color(rgb(tk().ink_2)).child(w.label.clone()))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(if w.pct >= 90 {
                                            tk().q_text
                                        } else {
                                            tk().ink
                                        }))
                                        .child(format!("{}%", w.pct)),
                                ),
                        )
                        .child(
                            div().h(px(4.)).rounded_full().bg(rgb(tk().track)).child(
                                div()
                                    .h_full()
                                    .w(relative(f32::from(w.pct.min(100)) / 100.))
                                    .rounded_full()
                                    .bg(rgb(color)),
                            ),
                        )
                        .when(!note.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(ui::ts(T_CAPTION))
                                    .text_color(rgb(tk().ink_3))
                                    .child(note.join(" · ")),
                            )
                        }),
                );
            }
            providers = providers.child(block);
        }
        // A scroll box with a visible scrollbar, as the page has.
        div()
            .flex_none()
            .w(px(width))
            .h_full()
            .relative()
            .bg(rgb(tk().card))
            .shadow(ui::side_shadow(-8.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .pt(px(40.))
                    .px(px(36.))
                    .pb(px(28.))
                    .child(top),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .relative()
                    .child(
                        div()
                            .id("overview")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.overview_scroll)
                            .pt(px(16.))
                            .px(px(36.))
                            .pb(px(64.))
                            .flex()
                            .flex_col()
                            .gap(px(44.))
                            .child(providers)
                            .child(self.who_works_where(war, cx)),
                    )
                    .child(
                        div().absolute().inset_0().child(
                            gpui_base::Scrollbar::vertical(&self.overview_scroll)
                                .mode(gpui_base::ScrollbarMode::Always),
                        ),
                    ),
            )
    }

    /// Agents per minute (working plus waiting), the newest bar in ink.
    pub(super) fn spark(&self, war: &WarRoom) -> Div {
        let samples = &war.activity;
        let max = samples
            .iter()
            .map(|a| a.active + a.waiting)
            .max()
            .unwrap_or(0)
            .max(1) as f32;
        let mut bars = div().mt(px(28.)).h(px(56.)).flex().items_end().gap(px(2.));
        let empty = aivory_protocol::ACTIVITY_SAMPLES.saturating_sub(samples.len());
        for _ in 0..empty {
            bars = bars.child(div().flex_1().h(px(3.)).rounded(px(3.)).bg(rgb(tk().track)));
        }
        for (i, a) in samples.iter().enumerate() {
            let v = (a.active + a.waiting) as f32 / max;
            bars = bars.child(
                div()
                    .flex_1()
                    .min_h(px(3.))
                    .h(relative(v))
                    .rounded(px(3.))
                    .bg(rgb(if i + 1 == samples.len() {
                        tk().ink
                    } else {
                        tk().track_2
                    }))
                    // The current minute breathes while agents work.
                    .when(i + 1 == samples.len() && a.active + a.waiting > 0, |d| {
                        d.opacity(0.55 + 0.45 * self.breath(0.))
                    }),
            );
        }
        bars
    }

    /// "Who works where": one Sankey per codebase with live sessions (agent, part, state).
    fn who_works_where(&self, war: &WarRoom, cx: &mut Context<Self>) -> Div {
        let active = layout::active_codebases(war);
        let mut flows = div().flex().flex_col().child(overview_head(
            "Who works where",
            Some("agent → part → state"),
        ));
        for id in &active {
            if let Some(card) = self.sankey_card(war, id, cx) {
                flows = flows.child(card);
            }
        }
        let quiet: Vec<&str> = war
            .codebases
            .iter()
            .map(|c| c.id.as_str())
            .filter(|id| !active.contains(id))
            .collect();
        if !quiet.is_empty() {
            flows = flows.child(
                div()
                    .mt(px(8.))
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("{}: no live sessions", quiet.join(", "))),
            );
        }
        flows
    }

    fn sankey_card(&self, war: &WarRoom, id: &str, cx: &mut Context<Self>) -> Option<Div> {
        let cb = war.codebase(id)?;
        let items = layout::flow_items(&war.sessions, id);
        let sankey = layout::sankey(&items, cb.part_names(), &FlowBox::CARD);
        let live = war
            .sessions
            .iter()
            .filter(|s| s.codebase == id && s.state.is_live())
            .count();
        let tokens: u64 = war
            .sessions
            .iter()
            .filter(|s| s.codebase == id)
            .map(|s| s.tokens)
            .sum();
        let target = id.to_string();
        let on = self
            .part_filter
            .as_ref()
            .is_some_and(|(cb, part)| cb == id && part.is_none());
        Some(
            div()
                .min_w(px(0.))
                .pt(px(14.))
                .pb(px(18.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .id(SharedString::from(format!("sk-{id}")))
                                .font_family(tk().display_heading)
                                .text_size(ui::ts(16.))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(rgb(tk().ink_2))
                                .cursor_pointer()
                                .when(on, |d| d.underline())
                                .hover(|s| s.underline())
                                .tooltip(ui::tip("Show only this codebase"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open = Overlay::None;
                                    this.part_filter = if on {
                                        None
                                    } else {
                                        Some((target.clone(), None))
                                    };
                                    cx.notify();
                                }))
                                .child(id.to_string()),
                        )
                        .child(
                            div()
                                .text_size(ui::ts(T_SMALL))
                                .text_color(rgb(tk().ink_3))
                                .whitespace_nowrap()
                                .child(format!(
                                    "{live} {}{}",
                                    if live == 1 { "session" } else { "sessions" },
                                    if tokens > 0 {
                                        format!(" · {} tokens", fmt_tokens(tokens))
                                    } else {
                                        String::new()
                                    }
                                )),
                        ),
                )
                .child(flow_box(sankey, self.tick)),
        )
    }
}

/// A heading of the Overview column with an optional note on the right.
fn overview_head(title: &'static str, note: Option<&'static str>) -> Div {
    div()
        .mb(px(16.))
        .flex()
        .items_baseline()
        .justify_between()
        .gap(px(8.))
        .font_family(tk().display_heading)
        .text_size(ui::ts(18.))
        .font_weight(FontWeight::NORMAL)
        .child(title)
        .when_some(note, |d, n| {
            d.child(
                div()
                    .text_size(ui::ts(T_CAPTION))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(rgb(tk().ink_3))
                    .child(n),
            )
        })
}

/// The 100 px flow box: links and nodes on a canvas, labels on top. The view box is stretched
/// on x only (`preserveAspectRatio="none"`); stroke widths do not scale.
fn flow_box(sankey: Sankey, tick: u32) -> Div {
    let w = sankey.width as f32;
    let labels: Vec<_> = sankey.labels.clone();
    let mut box_ = div().relative().h(px(sankey.height as f32)).w_full().child(
        canvas(
            move |_, _, _| sankey,
            move |bounds: Bounds<Pixels>, sankey, window, _| {
                let sx = bounds.size.width / px(w);
                let at = |x: f64, y: f64| {
                    point(
                        bounds.origin.x + px(x as f32) * sx,
                        bounds.origin.y + px(y as f32),
                    )
                };
                let part_x = sankey
                    .nodes
                    .iter()
                    .filter(|n| n.column == FlowColumn::Part)
                    .map(|n| n.x)
                    .fold(f64::MAX, f64::min);
                let provider_at = |y: f64| {
                    sankey
                        .nodes
                        .iter()
                        .find(|n| {
                            n.column == FlowColumn::Provider
                                && y >= n.y - 0.5
                                && y <= n.y + n.h + 0.5
                        })
                        .map(|n| provider_color(&n.key))
                };
                for (i, link) in sankey.links.iter().enumerate() {
                    // Agent to part takes the agent color; part to state the state color.
                    let first = link.x0 < part_x;
                    let (color, alpha) = match (first, link.state, provider_at(link.y0)) {
                        (true, SessionState::Working | SessionState::Idle, Some(c)) => (c, 0x38),
                        _ => link_color(link.state),
                    };
                    // A working ribbon breathes, each one in its own phase.
                    // Working ribbons breathe, each in its own phase; waiting ribbons pulse.
                    let alpha = match link.state {
                        SessionState::Working => {
                            let phase = ((tick as f32 * 0.35 + i as f32 * 0.9).sin() + 1.) / 2.;
                            (alpha as f32 * (0.6 + 0.8 * phase)).round().min(255.) as u32
                        }
                        SessionState::NeedsYou => {
                            let phase = ((tick as f32 * 0.8).sin() + 1.) / 2.;
                            (alpha as f32 * (0.55 + 0.9 * phase)).round().min(255.) as u32
                        }
                        _ => alpha,
                    };
                    let xm = (link.x0 + link.x1) / 2.;
                    let mut path = PathBuilder::stroke(px(link.width as f32));
                    path.move_to(at(link.x0, link.y0));
                    path.cubic_bezier_to(at(link.x1, link.y1), at(xm, link.y0), at(xm, link.y1));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, ui::hsla(color << 8 | alpha));
                    }
                }
                for node in &sankey.nodes {
                    let color = match (node.state, node.column) {
                        (Some(state), _) => ui::state_color(state),
                        (None, FlowColumn::Provider) => provider_color(&node.key),
                        (None, _) => tk().ink,
                    };
                    let origin = at(node.x, node.y);
                    let size = gpui::size(px(node.w as f32) * sx, px(node.h.max(2.) as f32));
                    window
                        .paint_quad(fill(Bounds { origin, size }, rgb(color)).corner_radii(px(2.)));
                }
            },
        )
        .absolute()
        .inset_0(),
    );
    // A part label ends before the state column, so a long name does not run into "working 5"
    // at a large interface text size.
    let state_frac = labels
        .iter()
        .find(|l| l.column == FlowColumn::State)
        .map_or(1., |l| (l.anchor_x / w as f64) as f32);
    for label in labels {
        let frac = (label.anchor_x / w as f64) as f32;
        let mut l = div()
            .absolute()
            .top(px(label.y as f32 - 5.))
            .text_size(ui::ts(T_CAPTION))
            .line_height(px(10.))
            .whitespace_nowrap()
            .text_color(rgb(tk().ink));
        l = match label.column {
            FlowColumn::Provider => {
                l.right(relative(1. - frac + 0.02))
                    .text_color(rgb(provider_color(
                        label.text.split_whitespace().next().unwrap_or(""),
                    )))
            }
            FlowColumn::Part => l
                .left(relative(frac + 0.015))
                .max_w(relative((state_frac - frac - 0.03).clamp(0.05, 0.34)))
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(FontWeight::MEDIUM),
            FlowColumn::State => {
                let l = l.left(relative(frac + 0.02));
                match label.state {
                    Some(SessionState::NeedsYou) => l
                        .text_color(rgb(tk().q_text))
                        .font_weight(FontWeight::SEMIBOLD),
                    Some(SessionState::Error) => l
                        .text_color(rgb(tk().e_text))
                        .font_weight(FontWeight::SEMIBOLD),
                    Some(SessionState::Idle) => l.text_color(rgb(tk().ink_3)),
                    _ => l,
                }
            }
        };
        // `claude 5`, as the artboard writes it.
        box_ = box_.child(l.child(label.text.replace(" ×", " ")));
    }
    box_
}

/// Link color and alpha byte: working ink 9 %, idle 18 %, needs you and error 50 %.
fn link_color(state: SessionState) -> (u32, u32) {
    match state {
        SessionState::NeedsYou => (tk().q, 0x80),
        SessionState::Error => (tk().e, 0x80),
        SessionState::Idle => (tk().idle_ring, 0x2E),
        _ => (tk().ink, 0x17),
    }
}

/// `9.66M`, `851k`, `842`.
pub(super) fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{}k", (n as f64 / 1000.).round())
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_read_short() {
        assert_eq!(fmt_tokens(9_660_000), "9.66M");
        assert_eq!(fmt_tokens(1_429_880_000), "1.43B");
        assert_eq!(fmt_tokens(851_400), "851k");
        assert_eq!(fmt_tokens(842), "842");
    }
}

/// `in 2 h 12 min`, `in 3 d 6 h`, `in 4 min`, `now`.
pub(super) fn until(ms: i64) -> String {
    let m = ms.max(0) / 60_000;
    match m {
        0 => "now".to_string(),
        1..=59 => format!("in {m} min"),
        60..=1439 => format!("in {} h {} min", m / 60, m % 60),
        _ => format!("in {} d {} h", m / 1440, m % 1440 / 60),
    }
}

#[cfg(test)]
mod until_tests {
    use super::until;

    #[test]
    fn reset_times_read_short() {
        assert_eq!(until(-5), "now");
        assert_eq!(until(4 * 60_000), "in 4 min");
        assert_eq!(until(132 * 60_000), "in 2 h 12 min");
        assert_eq!(until((3 * 1440 + 410) * 60_000), "in 3 d 6 h");
    }
}
