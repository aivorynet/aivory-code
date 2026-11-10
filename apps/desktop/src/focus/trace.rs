//! The Trace tab: the span waterfall of the session and the evidence of the picked span.

use std::ops::Range;
use std::time::{Duration, Instant};

use aivory_protocol::rpc::{RpcError, SessionRef, code, method};
use aivory_protocol::{MINUTE_MS, SessionTrace, Span, SpanKind, UnixMs};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, FontWeight, SharedString, UniformListScrollHandle, div, px, relative,
    rgb, rgba, uniform_list,
};

use super::FocusScreen;
use crate::theme::*;
use crate::ui;

const ROW_H: f32 = 28.;
const OP_W: f32 = 230.;
const DUR_W: f32 = 64.;
/// rgba(13,13,14,.05): row borders and grid lines.
const FAINT: u32 = 0x0D0D0E0D;
/// How long a read trace counts as fresh.
const TTL: Duration = Duration::from_secs(4);

/// What the daemon said about the spans.
pub(super) enum Load {
    Loading,
    Ready(Box<SessionTrace>),
    Unavailable,
    Failed(String),
}

/// One visible row: the index in the span list and the depth in the tree.
#[derive(Clone, Copy)]
pub(super) struct Row {
    index: usize,
    depth: usize,
}

/// The minute axis: where the bars start and end.
struct Axis {
    origin: UnixMs,
    total: i64,
}

impl Axis {
    fn new(spans: &[Span], now: UnixMs) -> Self {
        let origin = spans.iter().map(|s| s.start).min().unwrap_or(now);
        let end = spans
            .iter()
            .map(|s| s.end.unwrap_or(now))
            .max()
            .unwrap_or(now)
            .max(origin);
        let minutes = ((end - origin + MINUTE_MS - 1) / MINUTE_MS).max(1);
        Self {
            origin,
            total: minutes * MINUTE_MS,
        }
    }

    /// Where a span starts and how wide its bar is, as fractions of the axis.
    fn bar(&self, span: &Span, now: UnixMs) -> (f32, f32) {
        let end = span.end.unwrap_or(now);
        let x = ((span.start - self.origin) as f64 / self.total as f64).clamp(0., 1.);
        let w = ((end - span.start).max(0) as f64 / self.total as f64).max(0.0035);
        (x as f32, w.min(1. - x) as f32)
    }
}

/// The state of the Trace tab.
pub(super) struct TraceState {
    pub load: Load,
    busy: bool,
    at: Option<Instant>,
    selected: Option<String>,
    rows: Vec<Row>,
    axis: Option<Axis>,
    now: UnixMs,
    scroll: UniformListScrollHandle,
}

impl Default for TraceState {
    fn default() -> Self {
        Self {
            load: Load::Loading,
            busy: false,
            at: None,
            selected: None,
            rows: Vec::new(),
            axis: None,
            now: 0,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

fn kind_color(kind: SpanKind) -> u32 {
    match kind {
        SpanKind::Run => 0x1A1A2E,
        SpanKind::Turn => 0x4B5BDB,
        SpanKind::Llm => 0x8B5CF6,
        SpanKind::Tool => 0xE8A23B,
        SpanKind::Agent => 0x0EA5A4,
    }
}

fn kind_tag(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Run => "RUN",
        SpanKind::Turn => "TURN",
        SpanKind::Llm => "LLM",
        SpanKind::Tool => "TOOL",
        SpanKind::Agent => "AGENT",
    }
}

/// `36 s`, `6.2 min`, `2.5 h`.
fn fmt_dur(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 59_500 {
        return format!("{} s", (ms as f64 / 1000.).round() as i64);
    }
    let minutes = ms as f64 / MINUTE_MS as f64;
    if minutes >= 120. {
        format!("{:.1} h", minutes / 60.)
    } else {
        format!("{minutes:.1} min")
    }
}

/// Parents come before children: the depth is the length of the parent chain.
fn rows_of(spans: &[Span]) -> Vec<Row> {
    let index: std::collections::HashMap<&str, usize> = spans
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    (0..spans.len())
        .map(|i| {
            let mut depth = 0;
            let mut at = spans[i].parent.as_deref().and_then(|p| index.get(p));
            while let Some(&p) = at {
                depth += 1;
                if depth > spans.len() {
                    break;
                }
                at = spans[p].parent.as_deref().and_then(|q| index.get(q));
            }
            Row { index: i, depth }
        })
        .collect()
}

fn note(text: impl Into<SharedString>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(32.))
        .text_size(ui::ts(T_BODY))
        .text_color(rgb(tk().ink_3))
        .child(text.into())
        .into_any_element()
}

impl FocusScreen {
    /// Ask the daemon for the spans when the last read is old. A daemon that answered
    /// `NOT_AVAILABLE` is not asked again.
    pub(super) fn refresh_trace(&mut self, cx: &mut Context<Self>) {
        let t = &self.trace;
        if t.busy || matches!(t.load, Load::Unavailable) {
            return;
        }
        if t.at.is_some_and(|at| at.elapsed() < TTL) {
            return;
        }
        self.trace.busy = true;
        let id = self.session.clone();
        let task = self
            .model
            .read(cx)
            .call::<method::SessionSpans>(SessionRef { id: id.clone() }, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.session != id {
                    return;
                }
                let m = this.model.read(cx);
                let now = match (&m.war, m.demo) {
                    (Some(war), true) => war.now,
                    _ => chrono::Utc::now().timestamp_millis(),
                };
                let t = &mut this.trace;
                t.busy = false;
                t.at = Some(Instant::now());
                match result {
                    Ok(trace) => {
                        t.rows = rows_of(&trace.spans);
                        t.axis = Some(Axis::new(&trace.spans, now));
                        t.now = now;
                        t.load = Load::Ready(Box::new(trace));
                    }
                    Err(e) if is_not_available(&e) => t.load = Load::Unavailable,
                    Err(e) => {
                        if !matches!(t.load, Load::Ready(_)) {
                            t.load = Load::Failed(format!("{e:#}"));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Trace tab: the waterfall on top, the evidence of the picked span below.
    pub(super) fn trace_tab(&mut self, cx: &mut Context<Self>) -> Div {
        self.refresh_trace(cx);
        let frame = div()
            .flex_1()
            .flex_grow(1.7)
            .min_w(px(560.))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(tk().card));
        let body = match &self.trace.load {
            Load::Loading => note("Loading…"),
            Load::Unavailable => note("The span trace is not available from this daemon yet."),
            Load::Failed(e) => note(format!("The span trace cannot be loaded: {e}")),
            Load::Ready(t) if t.spans.is_empty() => note("The session has no spans yet."),
            Load::Ready(_) => div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div().flex_1().min_h(px(0.)).child(
                        uniform_list(
                            "trace-rows",
                            self.trace.rows.len(),
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                range.map(|ix| this.trace_row(ix, cx)).collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.trace.scroll)
                        .size_full(),
                    ),
                )
                .child(self.evidence())
                .into_any_element(),
        };
        frame.child(div().flex_1().min_h(px(0.)).child(body))
    }

    fn trace_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.trace;
        let (Some(row), Load::Ready(trace), Some(axis)) =
            (t.rows.get(ix).copied(), &t.load, &t.axis)
        else {
            return div().into_any_element();
        };
        let span = &trace.spans[row.index];
        let color = kind_color(span.kind);
        let selected = self.picked().is_some_and(|s| s.id == span.id);
        let (x, w) = axis.bar(span, t.now);
        let id = span.id.clone();
        let operation = div()
            .w(px(OP_W))
            .flex_none()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(7.))
            .pl(px(row.depth as f32 * 14.))
            .text_size(ui::ts(T_SMALL))
            .child(
                div()
                    .size(px(8.))
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
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(kind_tag(span.kind)),
            );
        let timeline = div().flex_1().min_w(px(0.)).h_full().relative().child(
            div()
                .absolute()
                .top(px(9.))
                .h(px(10.))
                .left(relative(x))
                .w(relative(w))
                .min_w(px(3.))
                .rounded(px(3.))
                .bg(rgb(color))
                .when(span.end.is_none(), |d| d.opacity(0.6)),
        );
        div()
            .id(SharedString::from(format!("tr-{}", span.id)))
            .flex()
            .items_center()
            .h(px(ROW_H))
            .w_full()
            .px(px(16.))
            .border_b_1()
            .border_color(rgba(FAINT))
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(tk().track)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(tk().track))))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.trace.selected = Some(id.clone());
                cx.notify();
            }))
            .child(operation)
            .child(timeline)
            .child(
                div()
                    .w(px(DUR_W))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .font_family(tk().mono)
                    .text_size(ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .whitespace_nowrap()
                    .child(fmt_dur(span.end.unwrap_or(self.trace.now) - span.start)),
            )
            .into_any_element()
    }

    /// The span that the evidence shows: the picked one, else the first.
    fn picked(&self) -> Option<&Span> {
        let Load::Ready(trace) = &self.trace.load else {
            return None;
        };
        self.trace
            .selected
            .as_deref()
            .and_then(|id| trace.spans.iter().find(|s| s.id == id))
            .or_else(|| trace.spans.first())
    }

    /// The facts of the picked span.
    fn evidence(&self) -> Div {
        let panel = div()
            .flex_none()
            .max_h(px(190.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(16.))
            .py(px(12.))
            .border_t_1()
            .border_color(rgba(tk().line))
            .text_size(ui::ts(T_SMALL));
        let Some(span) = self.picked() else {
            return panel;
        };
        let cell = |k: &str, v: String| {
            div()
                .flex()
                .gap(px(5.))
                .child(div().text_color(rgb(tk().ink_3)).child(k.to_string()))
                .child(div().font_family(tk().mono).child(v))
        };
        let mut facts = div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(2.));
        facts = facts
            .child(cell("agent", span.agent.clone()))
            .child(cell("model", span.model.clone()))
            .child(cell(
                "tokens",
                format!("{} in / {} out", span.tokens_in, span.tokens_out),
            ))
            .child(cell("cost", format!("${:.4}", span.cost)));
        if let Some(tool) = &span.tool {
            facts = facts.child(cell("tool", tool.clone()));
        }
        let text = |s: &str| {
            let s: String = s.chars().take(400).collect();
            div()
                .font_family(tk().mono)
                .text_color(rgb(tk().ink_2))
                .max_h(px(44.))
                .overflow_hidden()
                .child(s)
        };
        panel
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                "{} {}",
                kind_tag(span.kind),
                span.name
            )))
            .child(facts)
            .when(!span.input.is_empty(), |d| d.child(text(&span.input)))
            .when(!span.output.is_empty(), |d| d.child(text(&span.output)))
    }
}

fn is_not_available(e: &anyhow::Error) -> bool {
    e.downcast_ref::<RpcError>()
        .is_some_and(|r| r.code == code::NOT_AVAILABLE)
}
