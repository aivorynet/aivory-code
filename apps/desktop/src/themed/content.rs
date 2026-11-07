//! The content components of the catalog (ADR 0024 section 4): text, readout, badge, list,
//! row, card, log, files, diff, chart, gauge and ruler. They read the data of the snapshot and
//! draw it with the tokens of the theme. Text clips with an ellipsis and shows the full text on
//! hover; a theme cannot turn that off.

use gpui::prelude::*;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, Styled, Window, div, px, rgba};
use serde_json::Value;

use super::data::as_text;
use super::host::{Node, ThemedView};
use super::render::{Frame, Scope, id, item_key, style};
use crate::theme::tk;
use crate::ui;

/// Text longer than this gets the full text on hover.
const HOVER_AT: usize = 18;

impl ThemedView {
    pub(super) fn text_node(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let t = f.text(node, "value", sc);
        let color = f.color(node, "color", sc).unwrap_or_else(|| f.tone("text"));
        let key = Self::key_of(node, sc);
        let lines = f.num(node, "lines", sc).map_or(1, |n| n as usize);
        let mut d = div()
            .id(id(&key))
            .min_w(px(0.))
            .text_size(f.ty(node, sc, "md"))
            .text_color(rgba(color));
        d = if lines <= 1 {
            d.truncate()
        } else {
            d.line_clamp(lines)
        };
        if f.text(node, "weight", sc) == "bold" {
            d = d.font_weight(FontWeight::BOLD);
        }
        if f.flag(node, "mono", sc) {
            d = d.font_family(tk().mono);
        }
        d = match f.text(node, "align", sc).as_str() {
            "center" => d.text_center(),
            "end" => d.text_right(),
            _ => d,
        };
        if t.chars().count() > HOVER_AT {
            d = d.tooltip(ui::tip(t.clone()));
        }
        style(d.child(t), node, f, sc).into_any_element()
    }

    pub(super) fn readout(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let row = f.text(node, "dir", sc) == "row";
        let mut d = div().flex().gap(px(if row { 8. } else { 1. }));
        d = if row {
            d.flex_row().items_baseline()
        } else {
            d.flex_col()
        };
        let unit = f.text(node, "unit", sc);
        let value = format!("{}{unit}", f.text(node, "value", sc));
        let color = f.color(node, "color", sc).unwrap_or_else(|| f.tone("text"));
        style(d.min_w(px(0.)), node, f, sc)
            .child(
                div()
                    .truncate()
                    .text_size(ui::ts(f.theme.types[0]))
                    .text_color(rgba(f.tone("muted")))
                    .child(f.text(node, "label", sc)),
            )
            .child(
                div()
                    .truncate()
                    .text_size(f.ty(node, sc, "lg"))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgba(color))
                    .child(value),
            )
            .into_any_element()
    }

    pub(super) fn badge(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let state = f.text(node, "state", sc);
        let color = if state.is_empty() {
            f.color(node, "tone", sc)
                .unwrap_or_else(|| f.tone("accent"))
        } else {
            f.state_color(&state)
        };
        let label = {
            let v = f.text(node, "value", sc);
            if v.is_empty() { state } else { v }
        };
        style(
            div()
                .flex_none()
                .px(px(8.))
                .py(px(2.))
                .rounded_full()
                .bg(rgba((color & 0xFFFFFF00) | 0x28))
                .text_color(rgba(color))
                .text_size(ui::ts(f.theme.types[0]))
                .child(label),
            node,
            f,
            sc,
        )
        .into_any_element()
    }

    pub(super) fn list_node(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let items = f.list(node, "items", sc);
        let handle = self.scrolls.entry(key.clone()).or_default().clone();
        let mut d = div()
            .id(id(&key))
            .flex()
            .flex_col()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&handle);
        if let Some(g) = f.space(node, "gap", sc) {
            d = d.gap(px(g));
        }
        if items.is_empty() {
            let empty = f.text(node, "empty", sc);
            if !empty.is_empty() {
                d = d.child(
                    div()
                        .p(px(8.))
                        .text_size(f.ty(node, sc, "sm"))
                        .text_color(rgba(f.tone("muted")))
                        .child(empty),
                );
            }
        }
        let limit = f.num(node, "limit", sc).map_or(usize::MAX, |n| n as usize);
        for (i, item) in items.iter().take(limit).enumerate() {
            let prefix = format!("{key}#{}/", item_key(item, i));
            let inner = Scope {
                item: Some(item),
                prefix: &prefix,
            };
            for c in &node.children {
                d = d.child(self.draw(c, f, &inner, window, cx));
            }
        }
        style(d, node, f, sc).into_any_element()
    }

    /// The press handler of a row or a card.
    fn on_press<E: StatefulInteractiveElement + Styled>(
        &self,
        e: E,
        node: &Node,
        sc: &Scope<'_>,
        cx: &mut Context<Self>,
    ) -> E {
        match node.press.clone() {
            Some(spec) => {
                let item = sc.item.cloned();
                e.cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.press(&spec, item.as_ref(), None, window, cx);
                    }))
            }
            None => e,
        }
    }

    pub(super) fn row(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let state = f.text(node, "state", sc);
        let title = f.text(node, "title", sc);
        let sub = f.text(node, "sub", sc);
        let meta = f.text(node, "meta", sc);
        let mut d = div()
            .id(id(&key))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(6.))
            .min_w(px(0.))
            .rounded(px(f.radius(node, sc).unwrap_or(6.)))
            .hover(|s| s.bg(rgba((f.tone("accent") & 0xFFFFFF00) | 0x1C)));
        if !state.is_empty() {
            d = d.child(ui::dot(gpui::rgba(f.state_color(&state))));
        }
        let mut col = div().flex().flex_col().flex_1().min_w(px(0.)).child(
            div()
                .truncate()
                .text_size(f.ty(node, sc, "md"))
                .text_color(rgba(f.tone("text")))
                .child(title),
        );
        if !sub.is_empty() {
            col = col.child(
                div()
                    .truncate()
                    .text_size(ui::ts(f.theme.types[1]))
                    .text_color(rgba(f.tone("muted")))
                    .child(sub),
            );
        }
        d = d.child(col);
        if !meta.is_empty() {
            d = d.child(
                div()
                    .flex_none()
                    .text_size(ui::ts(f.theme.types[1]))
                    .text_color(rgba(f.tone("muted")))
                    .child(meta),
            );
        }
        let d = self.on_press(d, node, sc, cx);
        style(d, node, f, sc).into_any_element()
    }

    pub(super) fn card(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let state = f.text(node, "state", sc);
        let edge = if state.is_empty() {
            f.tone("muted") & 0xFFFFFF00 | 0x40
        } else {
            f.state_color(&state)
        };
        let title = f.text(node, "title", sc);
        let sub = f.text(node, "sub", sc);
        let mut d = div()
            .id(id(&key))
            .flex()
            .flex_col()
            .gap(px(f.space(node, "gap", sc).unwrap_or(6.)))
            .p(px(f.space(node, "pad", sc).unwrap_or(12.)))
            .min_w(px(0.))
            .rounded(px(f.radius(node, sc).unwrap_or(8.)))
            .bg(rgba(f.tone("surface")))
            .border_1()
            .border_color(rgba(edge));
        if !title.is_empty() {
            d = d.child(
                div()
                    .truncate()
                    .text_size(f.ty(node, sc, "md"))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgba(f.tone("text")))
                    .child(title),
            );
        }
        if !sub.is_empty() {
            d = d.child(
                div()
                    .truncate()
                    .text_size(ui::ts(f.theme.types[1]))
                    .text_color(rgba(f.tone("muted")))
                    .child(sub),
            );
        }
        let kids = self.draw_children(node, f, sc, window, cx);
        let d = self.on_press(d.children(kids), node, sc, cx);
        // The fill and the corners of a card come from its own tokens; `style` sets the size.
        let mut out = d;
        if let Some(w) = f.num(node, "width", sc) {
            out = out.w(px(w as f32)).flex_none();
        }
        if let Some(h) = f.num(node, "height", sc) {
            out = out.h(px(h as f32)).flex_none();
        }
        if f.num(node, "grow", sc).is_some_and(|g| g > 0.) {
            out = out.flex_1();
        }
        out.into_any_element()
    }

    pub(super) fn log(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let key = Self::key_of(node, sc);
        let items = f.list(node, "items", sc);
        let field = {
            let t = f.text(node, "field", sc);
            if t.is_empty() { "text".to_string() } else { t }
        };
        let limit = f.num(node, "limit", sc).map_or(50, |n| n as usize);
        let from = items.len().saturating_sub(limit);
        let handle = self.scrolls.entry(key.clone()).or_default().clone();
        let mut d = div()
            .id(id(&key))
            .flex()
            .flex_col()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&handle)
            .font_family(tk().mono)
            .text_size(f.ty(node, sc, "sm"))
            .text_color(rgba(f.tone("muted")));
        for item in &items[from..] {
            let line = match item {
                Value::Object(o) => as_text(o.get(&field)),
                other => as_text(Some(other)),
            };
            d = d.child(div().truncate().child(line));
        }
        style(d, node, f, sc).into_any_element()
    }

    /// A file with the bars of added and removed lines.
    pub(super) fn files(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let added = f.num(node, "added", sc).unwrap_or(0.).max(0.);
        let removed = f.num(node, "removed", sc).unwrap_or(0.).max(0.);
        let bar = |n: f64, c: u32| div().h(px(6.)).w(px((n as f32 * 3.).min(120.))).bg(rgba(c));
        style(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .min_w(px(0.)),
            node,
            f,
            sc,
        )
        .child(
            div()
                .flex_1()
                .truncate()
                .font_family(tk().mono)
                .text_size(f.ty(node, sc, "sm"))
                .text_color(rgba(f.tone("text")))
                .child(f.text(node, "label", sc)),
        )
        .child(
            div()
                .flex_none()
                .text_size(ui::ts(f.theme.types[0]))
                .text_color(rgba(f.state_color("done")))
                .child(format!("+{added}")),
        )
        .child(
            div()
                .flex_none()
                .text_size(ui::ts(f.theme.types[0]))
                .text_color(rgba(f.state_color("error")))
                .child(format!("-{removed}")),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .gap(px(1.))
                .child(bar(added, f.state_color("done")))
                .child(bar(removed, f.state_color("error"))),
        )
        .into_any_element()
    }

    /// A text with `+` lines in green and `-` lines in red.
    pub(super) fn diff(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let text = f.text(node, "value", sc);
        let mut d = div()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .font_family(tk().mono)
            .text_size(f.ty(node, sc, "sm"));
        for line in text.lines() {
            let c = match line.chars().next() {
                Some('+') => f.state_color("done"),
                Some('-') => f.state_color("error"),
                _ => f.tone("muted"),
            };
            d = d.child(div().truncate().text_color(rgba(c)).child(line.to_string()));
        }
        style(d, node, f, sc).into_any_element()
    }

    /// Charts of the catalog. Every kind draws with plain boxes: `bar`, `strip`, `spark`, `line`
    /// and `area` as columns of different weight, `stacked` as columns of three states,
    /// `segments` as one bar cut by value.
    pub(super) fn chart(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let items = f.list(node, "items", sc);
        let kind = f.text(node, "kind", sc);
        let field = {
            let t = f.text(node, "field", sc);
            if t.is_empty() { "pct".to_string() } else { t }
        };
        let height = f.num(node, "height", sc).map_or(60., |h| h as f32);
        let color = f
            .color(node, "color", sc)
            .unwrap_or_else(|| f.tone("accent"));
        let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.) as f32;
        let mut d = div().flex().min_w(px(0.)).h(px(height));
        match kind.as_str() {
            "segments" => {
                d = d.flex_row().gap(px(2.)).items_stretch();
                for (i, it) in items.iter().enumerate() {
                    let n = num(it, &field).max(0.);
                    let c = if i % 2 == 0 {
                        color
                    } else {
                        f.tone("analysis")
                    };
                    d = d.child(div().flex_1().flex_basis(px(n.max(1.))).bg(rgba(c)));
                }
            }
            "stacked" => {
                d = d.flex_row().gap(px(1.)).items_end();
                let max = items
                    .iter()
                    .map(|i| num(i, "active") + num(i, "waiting") + num(i, "idle"))
                    .fold(1., f32::max);
                for it in items {
                    let seg = |k: &str, c: u32| {
                        div().w_full().h(px(height * num(it, k) / max)).bg(rgba(c))
                    };
                    d = d.child(
                        div()
                            .flex_1()
                            .min_w(px(1.))
                            .flex()
                            .flex_col_reverse()
                            .child(seg("active", f.state_color("working")))
                            .child(seg("waiting", f.state_color("needsYou")))
                            .child(seg("idle", f.state_color("idle"))),
                    );
                }
            }
            _ => {
                d = d
                    .flex_row()
                    .items_end()
                    .gap(px(if kind == "strip" { 0. } else { 1. }));
                let max = f
                    .num(node, "max", sc)
                    .map_or_else(
                        || items.iter().map(|i| num(i, &field)).fold(1., f32::max),
                        |m| m as f32,
                    )
                    .max(1.);
                let area = kind == "area" || kind == "bar";
                for it in items {
                    let h = (height * num(it, &field) / max).clamp(1., height);
                    let col = if area {
                        div().w_full().h(px(h)).bg(rgba(color))
                    } else {
                        div().w_full().h(px(h.min(3.))).bg(rgba(color))
                    };
                    d = d.child(
                        div()
                            .flex_1()
                            .min_w(px(1.))
                            .h(px(height))
                            .flex()
                            .flex_col_reverse()
                            .child(col)
                            .when(kind == "line", |c| c.pb(px(h))),
                    );
                }
            }
        }
        style(d, node, f, sc).into_any_element()
    }

    /// A gauge. The kinds `arc` and `dial` draw as a bar in this display.
    pub(super) fn gauge(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let max = f.num(node, "max", sc).unwrap_or(100.).max(1.);
        let value = f.num(node, "value", sc).unwrap_or(0.).clamp(0., max);
        let unit = f.text(node, "unit", sc);
        let color = f
            .color(node, "color", sc)
            .unwrap_or_else(|| f.tone("accent"));
        style(
            div().flex().flex_col().gap(px(3.)).min_w(px(0.)),
            node,
            f,
            sc,
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .text_size(ui::ts(f.theme.types[1]))
                .child(
                    div()
                        .truncate()
                        .text_color(rgba(f.tone("muted")))
                        .child(f.text(node, "label", sc)),
                )
                .child(
                    div()
                        .text_color(rgba(f.tone("text")))
                        .child(format!("{value}{unit}")),
                ),
        )
        .child(
            div()
                .h(px(6.))
                .w_full()
                .rounded_full()
                .bg(rgba((f.tone("muted") & 0xFFFFFF00) | 0x30))
                .child(
                    div()
                        .h_full()
                        .w(gpui::relative((value / max) as f32))
                        .rounded_full()
                        .bg(rgba(color)),
                ),
        )
        .into_any_element()
    }

    pub(super) fn ruler(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let steps = f
            .num(node, "steps", sc)
            .map_or(10, |n| (n as usize).clamp(2, 60));
        let mut d = div().flex().flex_row().h(px(8.)).w_full();
        for _ in 0..steps {
            d = d.child(
                div()
                    .flex_1()
                    .h_full()
                    .border_l_1()
                    .border_color(rgba(f.tone("muted"))),
            );
        }
        style(
            div()
                .flex()
                .flex_col()
                .border_b_1()
                .border_color(rgba(f.tone("muted")))
                .child(d),
            node,
            f,
            sc,
        )
        .into_any_element()
    }
}
