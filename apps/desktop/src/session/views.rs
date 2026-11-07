//! The tabs of the deep dive (waterfall, DAG, transcript, diff) and the evidence panel.

use std::ops::Range;
use std::rc::Rc;

use aivory_protocol::layout;
use aivory_protocol::{DiffLineKind, Pane};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, BoxShadow, ContentMask, Context, Div, FontWeight, PathBuilder, SharedString,
    Stateful, Window, canvas, div, list, point, px, relative, rgb, rgba, uniform_list,
};

use super::logic::{self, Axis};
use super::{Load, SessionEvent, SessionScreen};
use crate::console::{Frame, Hooks, Style};
use crate::theme::*;
use crate::ui;

/// Height of a waterfall row and of a diff line.
const ROW_H: f32 = 34.;
const DIFF_LINE_H: f32 = 21.;
/// rgba(13,13,14,.05): row borders and grid lines.
const FAINT: u32 = 0x0D0D0E0D;
/// The waterfall columns: operation and duration.
const OP_W: f32 = 420.;
const DUR_W: f32 = 84.;

/// The small mono label of the design (`.k`, `.ev-k`).
pub fn key(text: &str) -> Div {
    div()
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(tk().ink_3))
        .whitespace_nowrap()
        .child(text.to_uppercase())
}

/// One calm line in the middle of a tab.
fn note(text: impl Into<SharedString>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(32.))
        .text_size(crate::ui::ts(crate::theme::T_BODY))
        .text_color(rgb(tk().ink_3))
        .child(text.into())
        .into_any_element()
}

/// The note of a tab whose data is not here: loading, not served by this daemon, or failed.
fn load_note<T>(load: &Load<T>, what: &str) -> Option<AnyElement> {
    match load {
        Load::Ready(_) => None,
        Load::Loading => Some(note("Loading…")),
        Load::Unavailable => Some(note(format!(
            "{what} is not available from this daemon yet."
        ))),
        Load::Failed(e) => Some(note(format!("{what} cannot be loaded: {e}"))),
    }
}

impl SessionScreen {
    // ---- waterfall ----

    pub(super) fn waterfall_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(n) = load_note(&self.trace, "The span trace") {
            return n;
        }
        let Some(axis) = &self.axis else {
            return note("Loading…");
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(waterfall_header(axis))
            .child(
                uniform_list(
                    "wf-rows",
                    self.rows.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| {
                        range
                            .map(|ix| this.waterfall_row(ix, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.waterfall_scroll)
                .flex_1()
                .min_h(px(0.)),
            )
            .into_any_element()
    }

    fn waterfall_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let (Some(row), Some(trace), Some(axis)) =
            (self.rows.get(ix).copied(), self.trace.ready(), &self.axis)
        else {
            return div().into_any_element();
        };
        let span = &trace.spans[row.index];
        let color = logic::kind_color(span.kind);
        let selected = self.selected_span().is_some_and(|s| s.id == span.id);
        let (x, w) = axis.bar(span, self.now);
        let id = span.id.clone();
        let toggle_id = span.id.clone();
        let twist = div()
            .id(SharedString::from(format!("tw-{}", span.id)))
            .size(px(16.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .when(row.has_children, |d| {
                let icon = if row.collapsed {
                    "icons/twist-right.svg"
                } else {
                    "icons/twist-down.svg"
                };
                d.cursor_pointer()
                    .hover(|s| s.bg(rgb(0xE6E9EE)))
                    .tooltip(ui::tip(if row.collapsed {
                        "Show the children"
                    } else {
                        "Hide the children"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if !this.collapsed.remove(&toggle_id) {
                            this.collapsed.insert(toggle_id.clone());
                        }
                        cx.notify();
                    }))
                    .child(ui::icon(icon, 10., rgb(tk().ink_3)))
            });
        let operation = div()
            .w(px(OP_W))
            .flex_none()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(7.))
            .pl(px(row.depth as f32 * 18.))
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .child(twist)
            .child(
                div()
                    .size(px(9.))
                    .flex_none()
                    .rounded(px(2.))
                    .bg(rgb(color)),
            )
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(span.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(logic::kind_tag(span.kind)),
            );
        let mut timeline = div().flex_1().min_w(px(0.)).h_full().relative();
        for tick in &axis.ticks {
            timeline = timeline.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(tick.frac))
                    .w(px(1.))
                    .bg(rgba(FAINT)),
            );
        }
        if let Some(now) = axis.now {
            timeline = timeline.child(now_line(now));
        }
        timeline = timeline.child(
            div()
                .absolute()
                .top(px(11.))
                .h(px(12.))
                .left(relative(x))
                .w(relative(w))
                .min_w(px(3.))
                .rounded(px(4.))
                .overflow_hidden()
                .bg(rgb(color))
                .when(span.end.is_none(), |d| d.child(hatch())),
        );
        div()
            .id(SharedString::from(format!("wf-{}", span.id)))
            .relative()
            .flex()
            .items_center()
            .h(px(ROW_H))
            .w_full()
            .pl(px(32.))
            .pr(px(24.))
            .border_b_1()
            .border_color(rgba(FAINT))
            .when(row.dim, |d| d.opacity(0.35))
            .when(selected, |d| {
                d.bg(rgb(0xEEF3FE)).child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w(px(3.))
                        .bg(rgb(tk().primary)),
                )
            })
            .when(!selected, |d| d.hover(|s| s.bg(rgb(0xF8F9FB))))
            .on_click(cx.listener(move |this, _, _, cx| this.pick_span(&id, cx)))
            .child(operation)
            .child(timeline)
            .child(
                div()
                    .w(px(DUR_W))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .whitespace_nowrap()
                    .child(logic::fmt_dur(span.end.unwrap_or(self.now) - span.start)),
            )
            .into_any_element()
    }

    // ---- DAG ----

    pub(super) fn dag_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(n) = load_note(&self.trace, "The DAG") {
            return n;
        }
        let Some(trace) = self.trace.ready() else {
            return note("Loading…");
        };
        if trace.nodes.is_empty() {
            return note("This session has no steps in a DAG yet.");
        }
        let (card_w, card_h) = layout::DAG_CARD;
        let pos = layout::dag(&trace.nodes, &trace.edges);
        let (mut width, mut height) = (1200_f64, 440_f64);
        for (x, y) in &pos {
            width = width.max(x + card_w);
            height = height.max(y + card_h);
        }
        let index = |id: &str| trace.nodes.iter().position(|n| n.id == id);
        // From the middle of the right side of a card to the middle of the left side of the next.
        let edges: Vec<Edge> = trace
            .edges
            .iter()
            .filter_map(|e| {
                let (a, b) = (index(&e.from)?, index(&e.to)?);
                let (color, dashed) = logic::edge_style(trace.nodes[b].state);
                Some(Edge {
                    from: ((pos[a].0 + card_w) as f32, (pos[a].1 + card_h / 2.) as f32),
                    to: (pos[b].0 as f32, (pos[b].1 + card_h / 2.) as f32),
                    color,
                    dashed,
                })
            })
            .collect();
        let selected = self.selected_span().map(|s| s.id.clone());
        let mut plane = div()
            .relative()
            .w(px(width as f32 + 64.))
            .h(px(height as f32 + 64.))
            .child(
                canvas(
                    move |_, _, _| edges,
                    |bounds, edges, window, _| {
                        let at = |(x, y): (f32, f32)| {
                            point(
                                bounds.origin.x + px(MARGIN + x),
                                bounds.origin.y + px(MARGIN + y),
                            )
                        };
                        for e in &edges {
                            let xm = (e.from.0 + e.to.0) / 2.;
                            let mut path = PathBuilder::stroke(px(1.5));
                            if e.dashed {
                                path = path.dash_array(&[px(4.), px(4.)]);
                            }
                            path.move_to(at(e.from));
                            path.cubic_bezier_to(at(e.to), at((xm, e.from.1)), at((xm, e.to.1)));
                            if let Ok(path) = path.build() {
                                window.paint_path(path, ui::hsla(e.color << 8 | 0xFF));
                            }
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );
        for (node, (x, y)) in trace.nodes.iter().zip(pos) {
            let on = node.span.is_some() && node.span == selected;
            let (text, bg) = logic::status_colors(node.state);
            let span = node.span.clone();
            plane = plane.child(
                div()
                    .id(SharedString::from(format!("dag-{}", node.id)))
                    .absolute()
                    .left(px(MARGIN + x as f32))
                    .top(px(MARGIN + y as f32))
                    .w(px(card_w as f32))
                    .h(px(card_h as f32))
                    .px(px(14.))
                    .py(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .overflow_hidden()
                    .bg(rgb(tk().card))
                    .border_1()
                    .border_color(if on {
                        rgb(tk().primary)
                    } else {
                        rgba(0x0D0D0E1F)
                    })
                    .rounded(tk().r_card)
                    .shadow(node_shadow(on))
                    .when(span.is_some(), |d| {
                        d.cursor_pointer()
                            .when(!on, |d| d.hover(|s| s.border_color(rgba(0x0D0D0E52))))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(id) = &span {
                            this.pick_span(id, cx);
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(node.id.clone())
                            .child(
                                div()
                                    .px(px(7.))
                                    .py(px(1.))
                                    .rounded_full()
                                    .bg(rgb(bg))
                                    .text_color(rgb(text))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(logic::status_label(node.state)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(relative(1.3))
                            .child(node.title.clone()),
                    )
                    .child(
                        div()
                            .mt_auto()
                            .flex()
                            .justify_between()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_2))
                            .child(node.agent.clone())
                            .child(logic::node_duration(node)),
                    ),
            );
        }
        div()
            .id("dag-scroll")
            .size_full()
            .overflow_scroll()
            .child(plane)
            .into_any_element()
    }

    // ---- transcript ----

    pub(super) fn transcript_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.transcript;
        if t.list.lines().is_empty() {
            return note(if t.loaded {
                "The transcript is empty."
            } else {
                "Loading…"
            });
        }
        div()
            .size_full()
            .bg(rgba(self.model.read(cx).console_theme().bg))
            .child(
                list(
                    t.list.list.clone(),
                    cx.processor(|this, ix: usize, _, cx| this.transcript_row(ix, cx)),
                )
                .size_full(),
            )
            .into_any_element()
    }

    /// One item of the transcript: the console look of the panes, in a centered column.
    fn transcript_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let style = Style {
            pad_x: 0.,
            pad_top: 24.,
            pad_bottom: 32.,
            ..Style::plain(12.5, self.model.read(cx).console_theme())
        };
        let weak = cx.entity().downgrade();
        let toggle = {
            let weak = weak.clone();
            Rc::new(move |abs: u32, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    this.transcript.list.toggle(abs);
                    cx.notify();
                });
            })
        };
        let open_file = {
            let weak = weak.clone();
            Rc::new(move |path: String, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |_, cx| cx.emit(SessionEvent::OpenFile(path)));
            })
        };
        let load_older = Rc::new(move |_: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.load_older(cx));
        });
        let hooks = Hooks {
            toggle: Some(toggle),
            open_file: Some(open_file),
            load_older: Some(load_older),
        };
        let id: SharedString = format!("dive-{}", self.id).into();
        let frame = Frame {
            style: &style,
            hooks: &hooks,
            id: &id,
            tick: 0,
        };
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(980.))
                    .px(px(32.))
                    .child(self.transcript.list.item(ix, &frame)),
            )
            .into_any_element()
    }

    // ---- diff ----

    pub(super) fn diff_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(n) = load_note(&self.diff, "The diff") {
            return n;
        }
        let Some(diff) = self.diff.ready() else {
            return note("Loading…");
        };
        if diff.files.is_empty() {
            return note("No changes in the worktree of this session yet.");
        }
        let ix = self.file.min(diff.files.len() - 1);
        let file = &diff.files[ix];
        let mut files = div()
            .id("diff-files")
            .w(px(260.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .py(px(12.))
            .bg(rgb(0xFAFBFC))
            .border_r_1()
            .border_color(rgba(tk().line));
        for (i, f) in diff.files.iter().enumerate() {
            let on = i == ix;
            files = files.child(
                div()
                    .id(SharedString::from(format!("df-{i}")))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .h(px(ROW_H))
                    .px(px(16.))
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(rgb(0xEEF3FE)).child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(3.))
                                .bg(rgb(tk().primary)),
                        )
                    })
                    .when(!on, |d| d.hover(|s| s.bg(rgb(tk().track))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.file = i;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(f.path.clone()),
                    )
                    .child(
                        div()
                            .text_color(rgb(0x15803D))
                            .child(format!("+{}", f.added)),
                    )
                    .child(
                        div()
                            .text_color(rgb(0xB91C1C))
                            .child(format!("−{}", f.removed)),
                    ),
            );
        }
        let turn = logic::file_turn(file).map_or(String::new(), |t| format!(" · turn {t}"));
        let lines = file.lines.len();
        let body = div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(40.))
                    .px(px(20.))
                    .bg(rgb(tk().card))
                    .border_b_1()
                    .border_color(rgba(tk().line))
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .child({
                        let path = file.path.clone();
                        div()
                            .id("diff-path")
                            .flex_1()
                            .min_w(px(0.))
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(tk().link)))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SessionEvent::OpenFile(path.clone()))
                            }))
                            .child(file.path.clone())
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(format!("+{} −{}{turn}", file.added, file.removed)),
                    ),
            )
            .child(
                uniform_list(
                    "diff-lines",
                    lines,
                    cx.processor(move |this, range: Range<usize>, _, _| {
                        range.map(|n| this.diff_line(ix, n)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.diff_scroll)
                .flex_1()
                .min_h(px(0.)),
            );
        div()
            .size_full()
            .flex()
            .child(files)
            .child(body)
            .into_any_element()
    }

    fn diff_line(&self, file: usize, ix: usize) -> AnyElement {
        let Some(line) = self
            .diff
            .ready()
            .and_then(|d| d.files.get(file))
            .and_then(|f| f.lines.get(ix))
        else {
            return div().into_any_element();
        };
        let (bg, fg) = match line.kind {
            DiffLineKind::Hunk => (Some(0xF3F5F8), tk().ink_3),
            DiffLineKind::Added => (Some(0xF0FAF3), 0x15803D),
            DiffLineKind::Removed => (Some(0xFEF3F3), 0xB91C1C),
            DiffLineKind::Context => (None, tk().ink),
        };
        let number = |n: Option<u32>| {
            div()
                .w(px(48.))
                .flex_none()
                .flex()
                .justify_end()
                .pr(px(10.))
                .text_color(rgb(tk().muted))
                .child(n.map(|n| n.to_string()).unwrap_or_default())
        };
        div()
            .flex()
            .w_full()
            .h(px(DIFF_LINE_H))
            .items_center()
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .when_some(bg, |d, c| d.bg(rgb(c)))
            .child(number(line.old))
            .child(number(line.new))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(fg))
                    .child(line.text.clone()),
            )
            .into_any_element()
    }

    // ---- evidence ----

    /// The right panel: the picked span.
    pub(super) fn evidence(&self, pane: Option<&Pane>, cx: &mut Context<Self>) -> Stateful<Div> {
        let panel = div()
            .id("evidence")
            .w(px(360.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(22.))
            .pt(px(20.))
            .pb(px(28.))
            .bg(rgb(tk().card));
        let Some(span) = self.selected_span() else {
            return panel.child(key("Span evidence")).child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .text_color(rgb(tk().ink_3))
                    .child("The evidence of a span shows here when the daemon serves spans."),
            );
        };
        let provider = pane.map_or("", |p| p.provider.as_str());
        let (text, bg) = logic::status_colors(span.state);
        let mut grid = div()
            .grid()
            .grid_cols(2)
            .gap_x(px(16.))
            .gap_y(px(14.))
            .pt(px(16.))
            .border_t_1()
            .border_color(rgba(tk().line));
        for (k, v) in logic::evidence_fields(span, provider, self.now, self.utc) {
            grid = grid.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .min_w(px(0.))
                    .child(key(k))
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(v),
                    ),
            );
        }
        let block = |label: &'static str, body: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .pt(px(16.))
                .border_t_1()
                .border_color(rgba(tk().line))
                .child(key(label))
                .child(
                    div()
                        .px(px(12.))
                        .py(px(10.))
                        .bg(rgb(tk().page))
                        .rounded(tk().r_button)
                        .font_family(tk().mono)
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .line_height(relative(1.6))
                        .child(body),
                )
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(30.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(tk().r_button)
                .border_1()
                .border_color(rgb(if primary { tk().primary } else { 0xC9CCD6 }))
                .bg(rgb(if primary { tk().primary } else { tk().card }))
                .text_color(rgb(if primary { tk().card } else { tk().ink }))
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .when(primary, |d| d.font_weight(FontWeight::MEDIUM))
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(move |s| {
                    if primary {
                        s.bg(rgb(tk().primary_hover))
                            .border_color(rgb(tk().primary_hover))
                    } else {
                        s.border_color(rgb(tk().muted))
                    }
                })
                .child(label)
        };
        panel
            .child(
                div()
                    .child(key(&format!("{} evidence", logic::kind_tag(span.kind))))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .mt(px(4.))
                                    .text_size(crate::ui::ts(crate::theme::T_TITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_height(relative(1.35))
                                    .child(span.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px(px(9.))
                                    .py(px(3.))
                                    .rounded_full()
                                    .bg(rgb(bg))
                                    .text_color(rgb(text))
                                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(logic::status_label(span.state)),
                            ),
                    ),
            )
            .child(grid)
            .child(block("Input", span.input.clone()))
            .child(block("Output", span.output.clone()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        button("ev-open", "Open in transcript", true)
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_transcript(cx))),
                    )
                    .child(
                        button("ev-fork", "Fork from here", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                let at = this.selected_span().map(|s| s.id.clone());
                                this.fork(at, cx)
                            },
                        )),
                    ),
            )
    }
}

/// The margin around the DAG plane (`.dag` has `margin: 32px`).
const MARGIN: f32 = 32.;

struct Edge {
    from: (f32, f32),
    to: (f32, f32),
    color: u32,
    dashed: bool,
}

fn node_shadow(selected: bool) -> Vec<BoxShadow> {
    let mut shadows = vec![BoxShadow {
        color: ui::hsla(0x0D0D0E0A),
        offset: point(px(0.), px(1.)),
        blur_radius: px(2.),
        spread_radius: px(0.),
        inset: false,
    }];
    if selected {
        shadows.push(BoxShadow {
            color: ui::hsla(0x3574F026),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(3.),
            inset: false,
        });
    }
    shadows
}

/// The header of the waterfall: Operation, the minute ticks, Duration.
fn waterfall_header(axis: &Axis) -> Div {
    let mut ticks = div().flex_1().min_w(px(0.)).h_full().relative();
    for (i, tick) in axis.ticks.iter().enumerate() {
        let label = div()
            .absolute()
            .bottom(px(8.))
            .left(relative(tick.frac))
            .font_weight(FontWeight::NORMAL)
            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
            .whitespace_nowrap();
        ticks = ticks.child(if i == 0 {
            label.child(tick.label.clone())
        } else {
            label
                .ml(px(-32.))
                .w(px(64.))
                .flex()
                .justify_center()
                .child(tick.label.clone())
        });
    }
    div()
        .flex_none()
        .flex()
        .items_end()
        .h(px(ROW_H))
        .pl(px(32.))
        .pr(px(24.))
        .bg(rgb(0xFAFBFC))
        .border_b_1()
        .border_color(rgba(tk().line))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(tk().ink_3))
        .child(div().w(px(OP_W)).flex_none().pb(px(8.)).child("OPERATION"))
        .child(ticks)
        .child(
            div()
                .w(px(DUR_W))
                .flex_none()
                .flex()
                .justify_end()
                .pb(px(8.))
                .child("DURATION"),
        )
}

/// The blue "now" line.
fn now_line(frac: f32) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(frac))
        .w(px(1.))
        .bg(rgba(0x3574F080))
}

/// Diagonal white stripes over a running bar (`repeating-linear-gradient(45deg, white 45 %)`).
fn hatch() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, (), window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let height = bounds.size.height;
                let mut path = PathBuilder::stroke(px(4.));
                let mut x = bounds.left() - height;
                while x < bounds.right() {
                    path.move_to(point(x, bounds.top()));
                    path.line_to(point(x + height, bounds.bottom()));
                    x += px(11.3);
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, ui::hsla(0xFFFFFF73));
                }
            });
        },
    )
    .absolute()
    .inset_0()
}
