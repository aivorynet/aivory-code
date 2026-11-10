//! The renderer of a theme layout: the walk over the compiled tree, the layout and frame
//! components of the catalog (ADR 0024 section 4), and the dialogs that the app owns. The
//! content components are in `content`, the session components in `sessions`.
//!
//! Props that name a token (`gap`, `pad`, `radius`, `bg`, `color`, `size`) read the tables of
//! the theme; an unknown name gives the default. A number is a size in px (`width`, `height`).

use gpui::prelude::*;
use gpui::{
    AnyElement, ClickEvent, Context, Div, ElementId, Pixels, SharedString, Stateful, Styled,
    Window, div, px, rgba,
};
use serde_json::Value;

use super::data::{DataPath, ThemeSnapshot, as_text};
use super::host::{Compiled, Cond, KeyBind, Node, Prop, TYPES, Test, ThemedEvent, ThemedView};
use crate::shell::Screen;
use crate::theme::tk;
use crate::ui;

/// The inputs of one frame.
pub struct Frame<'a> {
    pub theme: &'a Compiled,
    pub snap: &'a ThemeSnapshot,
    /// The window width in px.
    pub width: f32,
}

/// Where the walk is: the current `$item` and the key prefix of the instance.
#[derive(Clone, Copy)]
pub struct Scope<'a> {
    pub item: Option<&'a Value>,
    pub prefix: &'a str,
}

impl<'a> Frame<'a> {
    pub fn val(&self, p: &Prop, sc: &Scope<'a>) -> Option<Value> {
        match p {
            Prop::Lit(v) => Some(v.clone()),
            Prop::Path(path) => self.snap.get(path, sc.item).cloned(),
            Prop::Map { path, table, other } => {
                let key = as_text(self.snap.get(path, sc.item));
                table.get(&key).or(other.as_ref()).cloned()
            }
        }
    }

    pub fn value(&self, node: &Node, name: &str, sc: &Scope<'a>) -> Option<Value> {
        self.val(node.prop(name)?, sc)
    }

    pub fn text(&self, node: &Node, name: &str, sc: &Scope<'a>) -> String {
        as_text(self.value(node, name, sc).as_ref())
    }

    pub fn num(&self, node: &Node, name: &str, sc: &Scope<'a>) -> Option<f64> {
        match self.value(node, name, sc)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn flag(&self, node: &Node, name: &str, sc: &Scope<'a>) -> bool {
        match self.value(node, name, sc) {
            Some(Value::Bool(b)) => b,
            Some(Value::String(s)) => s == "true",
            Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.),
            _ => false,
        }
    }

    /// A list that a prop points at.
    pub fn list(&self, node: &Node, name: &str, sc: &Scope<'a>) -> &'a [Value] {
        match node.prop(name) {
            Some(Prop::Path(path)) => self
                .snap
                .get(path, sc.item)
                .and_then(Value::as_array)
                .map_or(&[], Vec::as_slice),
            _ => &[],
        }
    }

    pub fn color(&self, node: &Node, name: &str, sc: &Scope<'a>) -> Option<u32> {
        let v = self.text(node, name, sc);
        if v.is_empty() {
            return None;
        }
        self.theme
            .color(&v)
            .or_else(|| self.theme.color(&format!("state.{v}")))
    }

    /// The color of a session state name (`needsYou`).
    pub fn state_color(&self, state: &str) -> u32 {
        self.theme
            .color(&format!("state.{state}"))
            .unwrap_or_else(|| self.theme.color("muted").unwrap_or(0x888888FF))
    }

    pub fn tone(&self, name: &str) -> u32 {
        self.theme.color(name).unwrap_or(0x888888FF)
    }

    /// A gap, a margin: a name of `space`, or a number.
    pub fn space(&self, node: &Node, name: &str, sc: &Scope<'a>) -> Option<f32> {
        match self.value(node, name, sc)? {
            Value::Number(n) => n.as_f64().map(|n| n as f32),
            Value::String(s) => self.theme.space.get(&s).copied(),
            _ => None,
        }
    }

    pub fn radius(&self, node: &Node, sc: &Scope<'a>) -> Option<f32> {
        match self.value(node, "radius", sc)? {
            Value::Number(n) => n.as_f64().map(|n| n as f32),
            Value::String(s) => self.theme.radii.get(&s).copied(),
            _ => None,
        }
    }

    /// A text size: the token `xs` to `xl`, scaled by the interface text size.
    pub fn ty(&self, node: &Node, sc: &Scope<'a>, default: &str) -> Pixels {
        let name = self.text(node, "size", sc);
        let i = TYPES
            .iter()
            .position(|t| *t == name)
            .or_else(|| TYPES.iter().position(|t| *t == default))
            .unwrap_or(2);
        ui::ts(self.theme.types[i])
    }

    pub fn test(&self, c: &Cond, sc: &Scope<'a>) -> bool {
        let v = self.snap.get(&c.path, sc.item);
        let same = |a: &Value, b: &Value| match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        };
        match &c.test {
            Test::NonEmpty => non_empty(v),
            Test::Empty => !non_empty(v),
            Test::Eq(x) => v.is_some_and(|v| same(v, x)),
            Test::Ne(x) => !v.is_some_and(|v| same(v, x)),
            Test::In(list) => v.is_some_and(|v| list.iter().any(|x| same(v, x))),
        }
    }

    /// The width class of the window.
    pub fn class(&self) -> usize {
        if self.width < 960. {
            0
        } else if self.width < 1440. {
            1
        } else {
            2
        }
    }
}

fn non_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.),
    }
}

pub fn id(key: &str) -> ElementId {
    ElementId::Name(SharedString::from(key.to_string()))
}

/// The common props of a node on a div: size, grow, padding, fill, corners, border.
pub fn style<E: Styled>(mut e: E, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> E {
    if let Some(w) = f.num(node, "width", sc) {
        e = e.w(px(w as f32)).flex_none();
    }
    if let Some(h) = f.num(node, "height", sc) {
        e = e.h(px(h as f32)).flex_none();
    }
    if f.num(node, "grow", sc).is_some_and(|g| g > 0.) {
        e = e.flex_1().min_w(px(0.)).min_h(px(0.));
    }
    if let Some(p) = f.space(node, "pad", sc) {
        e = e.p(px(p));
    }
    if let Some(c) = f.color(node, "bg", sc) {
        e = e.bg(rgba(c));
    }
    if let Some(r) = f.radius(node, sc) {
        e = e.rounded(px(r));
    }
    if let Some(c) = f.color(node, "border", sc) {
        e = e.border_1().border_color(rgba(c));
    }
    e
}

impl ThemedView {
    /// The state key of a node instance.
    pub(super) fn key_of(node: &Node, sc: &Scope<'_>) -> String {
        format!("{}{}", sc.prefix, node.key)
    }

    /// Draw one node. A node with a failed `when` draws nothing.
    pub(super) fn draw(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(c) = &node.when
            && !f.test(c, sc)
        {
            return div().into_any_element();
        }
        if let Some(a) = &node.key_action
            && let Some(combo) = a.get("combo").and_then(Value::as_str)
        {
            self.binds.push(KeyBind {
                combo: combo.to_string(),
                action: a.clone(),
                item: sc.item.cloned(),
            });
        }
        let el = match node.kind {
            "stack" => self.stack(node, f, sc, window, cx),
            "grid" => self.grid(node, f, sc, window, cx),
            "layer" => self.layer(node, f, sc, window, cx),
            "scroll" => self.scroll(node, f, sc, window, cx),
            "spacer" => self.spacer(node, f, sc),
            "repeat" => self.repeat(node, f, sc, window, cx),
            "responsive" => self.responsive(node, f, sc, window, cx),
            "sessions" => self.sessions(node, f, sc, window, cx),
            "panel" | "frame" => self.panel(node, f, sc, window, cx),
            "nav" => self.nav(node, f, sc, cx),
            "segment_button" | "button" => self.button(node, f, sc, cx),
            "switch" => self.switch(node, f, sc, cx),
            "tabs" => self.tabs(node, f, sc, window, cx),
            "look_switch" => self.look_switch(node, f, sc, cx),
            "key_hint" => self.key_hint(node, f, sc),
            "text" => self.text_node(node, f, sc),
            "readout" => self.readout(node, f, sc),
            "badge" => self.badge(node, f, sc),
            "image" => self.fallback_or_empty(node, f, sc, window, cx),
            "list" => self.list_node(node, f, sc, window, cx),
            "row" => self.row(node, f, sc, cx),
            "card" => self.card(node, f, sc, window, cx),
            "log" => self.log(node, f, sc),
            "files" => self.files(node, f, sc),
            "diff" => self.diff(node, f, sc),
            "decision" => self.decision(node, f, sc, window, cx),
            "decision_queue" => self.decision_queue(node, f, sc, window, cx),
            "transcript" => self.transcript(node, f, sc),
            "terminal" => self.terminal(node, f, sc),
            "composer" => self.composer(node, f, sc, window, cx),
            "settings_section" => self.settings_section(node, f, sc),
            "chart" => self.chart(node, f, sc),
            "gauge" => self.gauge(node, f, sc),
            "ruler" => self.ruler(node, f, sc),
            // Effects and scenes: the display draws the fallback node (ADR 0024 section 4).
            _ => self.fallback_or_empty(node, f, sc, window, cx),
        };
        // A press on a node that is no control itself.
        if let Some(spec) = &node.press
            && !matches!(
                node.kind,
                "button" | "segment_button" | "row" | "card" | "switch"
            )
        {
            let (spec, item) = (spec.clone(), sc.item.cloned());
            let key = Self::key_of(node, sc);
            return div()
                .id(id(&format!("{key}/press")))
                .cursor_pointer()
                .child(el)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.press(&spec, item.as_ref(), None, window, cx);
                }))
                .into_any_element();
        }
        el
    }

    pub(super) fn draw_children(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        node.children
            .iter()
            .map(|c| self.draw(c, f, sc, window, cx))
            .collect()
    }

    pub(super) fn fallback_or_empty(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &node.fallback {
            Some(fb) => self.draw(fb, f, sc, window, cx),
            None => div().into_any_element(),
        }
    }

    // ---- layout ----

    fn stack(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = f.text(node, "dir", sc) == "row";
        let mut d = div().flex().min_w(px(0.)).min_h(px(0.));
        d = if row { d.flex_row() } else { d.flex_col() };
        if let Some(g) = f.space(node, "gap", sc) {
            d = d.gap(px(g));
        }
        d = match f.text(node, "align", sc).as_str() {
            "center" => d.items_center(),
            "end" => d.items_end(),
            "start" => d.items_start(),
            _ => d,
        };
        d = match f.text(node, "justify", sc).as_str() {
            "center" => d.justify_center(),
            "end" => d.justify_end(),
            "between" => d.justify_between(),
            _ => d,
        };
        if f.flag(node, "wrap", sc) {
            d = d.flex_wrap();
        }
        style(d, node, f, sc)
            .children(self.draw_children(node, f, sc, window, cx))
            .into_any_element()
    }

    fn grid(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cols = f
            .num(node, "columns", sc)
            .map_or(2, |c| (c as usize).clamp(1, 12));
        let gap = f.space(node, "gap", sc).unwrap_or(8.);
        let kids = self.draw_children(node, f, sc, window, cx);
        let mut col = div().flex().flex_col().gap(px(gap)).min_w(px(0.));
        let mut it = kids.into_iter().peekable();
        while it.peek().is_some() {
            let mut row = div().flex().flex_row().gap(px(gap)).min_w(px(0.));
            for _ in 0..cols {
                let cell = div().flex_1().min_w(px(0.));
                row = row.child(match it.next() {
                    Some(k) => cell.child(k),
                    None => cell,
                });
            }
            col = col.child(row);
        }
        style(col, node, f, sc).into_any_element()
    }

    fn layer(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut kids = self.draw_children(node, f, sc, window, cx).into_iter();
        let mut d = div().relative().min_w(px(0.)).min_h(px(0.));
        if let Some(base) = kids.next() {
            d = d.child(base);
        }
        for k in kids {
            d = d.child(div().absolute().top_0().left_0().size_full().child(k));
        }
        style(d, node, f, sc).into_any_element()
    }

    fn scroll(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let handle = self.scrolls.entry(key.clone()).or_default().clone();
        let mut d = div()
            .id(id(&key))
            .flex()
            .flex_col()
            .min_w(px(0.))
            .min_h(px(0.))
            .track_scroll(&handle);
        if let Some(g) = f.space(node, "gap", sc) {
            d = d.gap(px(g));
        }
        let d = if f.text(node, "dir", sc) == "row" {
            d.flex_row().overflow_x_scroll()
        } else {
            d.overflow_y_scroll()
        };
        style(d, node, f, sc)
            .children(self.draw_children(node, f, sc, window, cx))
            .into_any_element()
    }

    fn spacer(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        match f.space(node, "size", sc) {
            Some(s) => div().flex_none().size(px(s)).into_any_element(),
            None => div().flex_1().into_any_element(),
        }
    }

    fn repeat(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = f.list(node, "items", sc);
        let limit = f.num(node, "limit", sc).map_or(usize::MAX, |n| n as usize);
        let row = f.text(node, "dir", sc) == "row";
        let mut d = div().flex().min_w(px(0.)).min_h(px(0.));
        d = if row { d.flex_row() } else { d.flex_col() };
        if let Some(g) = f.space(node, "gap", sc) {
            d = d.gap(px(g));
        }
        if f.flag(node, "wrap", sc) {
            d = d.flex_wrap();
        }
        let base = Self::key_of(node, sc);
        for (i, item) in items.iter().take(limit).enumerate() {
            let prefix = format!("{base}#{}/", item_key(item, i));
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

    fn responsive(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let class = f.class();
        // A class without a node uses the next smaller class.
        let pick = node
            .classes
            .iter()
            .filter(|(rank, _)| *rank <= class)
            .max_by_key(|(rank, _)| *rank)
            .or_else(|| node.classes.iter().min_by_key(|(rank, _)| *rank));
        match pick {
            Some((_, n)) => self.draw(n, f, sc, window, cx),
            None => div().into_any_element(),
        }
    }

    // ---- frame ----

    fn panel(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let variant = f.text(node, "variant", sc);
        let analysis = f.text(node, "role", sc) == "analysis";
        let surface = f.tone("surface");
        let line = f.tone("muted") & 0xFFFFFF00 | 0x40;
        let accent = f.tone("accent");
        let mut d = div()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_hidden();
        let radius = f.radius(node, sc).unwrap_or(8.);
        d = match variant.as_str() {
            "glass" => {
                let a = (f.theme.glass_alpha.clamp(0., 1.) * 255.) as u32;
                d.bg(rgba((surface & 0xFFFFFF00) | a))
                    .border_1()
                    .border_color(rgba(0xFFFFFF40))
                    .rounded(px(radius))
            }
            "chamfer" => d
                .bg(rgba(surface))
                .border_l(px(4.))
                .border_color(rgba(accent))
                .rounded_tr(px(radius * 2.))
                .rounded_bl(px(radius * 2.)),
            "window" => d
                .bg(rgba(surface))
                .border_1()
                .border_color(rgba(line))
                .rounded(px(radius)),
            "monitor" => d
                .bg(rgba(f.tone("bg")))
                .border_1()
                .border_color(rgba(accent & 0xFFFFFF00 | 0x80))
                .rounded(px(2.))
                .font_family(tk().mono),
            _ => d.bg(rgba(surface)).rounded(px(radius)),
        };
        if node.kind == "frame" {
            d = d.rounded(px(0.));
        }
        let title = if analysis {
            "Analysis".to_string()
        } else {
            f.text(node, "title", sc)
        };
        let seg: Vec<String> = if node.kind == "frame" {
            f.value(node, "segments", sc)
                .and_then(|v| v.as_array().cloned())
                .map(|a| a.iter().map(|x| as_text(Some(x))).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if !seg.is_empty() {
            let mut bar = div().flex().flex_row().gap(px(4.)).p(px(6.));
            for (i, s) in seg.into_iter().enumerate() {
                let c = if i % 2 == 0 {
                    accent
                } else {
                    f.tone("analysis")
                };
                bar = bar.child(
                    div()
                        .px(px(12.))
                        .py(px(3.))
                        .rounded(px(10.))
                        .bg(rgba(c))
                        .text_color(rgba(f.tone("bg")))
                        .text_size(ui::ts(f.theme.types[0]))
                        .child(s),
                );
            }
            d = d.child(bar);
        }
        if !title.is_empty() {
            let c = if analysis {
                f.tone("analysis")
            } else {
                f.tone("muted")
            };
            let mut head = div()
                .px(px(12.))
                .pt(px(8.))
                .pb(px(2.))
                .text_size(ui::ts(f.theme.types[1]))
                .text_color(rgba(c))
                .child(title);
            if variant == "window" {
                head = head
                    .py(px(6.))
                    .border_b_1()
                    .border_color(rgba(line))
                    .bg(rgba(f.tone("bg")));
            }
            d = d.child(head);
        }
        let pad = f.space(node, "pad", sc).unwrap_or(12.);
        let gap = f.space(node, "gap", sc).unwrap_or(8.);
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .gap(px(gap))
            .p(px(pad))
            .children(self.draw_children(node, f, sc, window, cx));
        // The size props go on the outer box; the fill and corners stay with the variant.
        let outer = d.child(body);
        let outer = if let Some(w) = f.num(node, "width", sc) {
            outer.w(px(w as f32)).flex_none()
        } else {
            outer
        };
        let outer = if let Some(h) = f.num(node, "height", sc) {
            outer.h(px(h as f32)).flex_none()
        } else {
            outer
        };
        let outer = if f.num(node, "grow", sc).is_some_and(|g| g > 0.) {
            outer.flex_1()
        } else {
            outer
        };
        outer.into_any_element()
    }

    /// The screens of the app, as the app lists them. A theme picks the variant and the place.
    fn nav(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const ITEMS: [(&str, &str, Screen); 5] = [
            ("warroom", "War room", Screen::WarRoom),
            ("inbox", "Inbox", Screen::Inbox),
            ("tasks", "Tasks", Screen::Tasks),
            ("repos", "Repos", Screen::Repos),
            ("settings", "Settings", Screen::Settings),
        ];
        let row = f.text(node, "dir", sc) != "column";
        let pill = f.text(node, "variant", sc) != "plain";
        let mut d = div().flex().gap(px(f.space(node, "gap", sc).unwrap_or(4.)));
        d = if row { d.flex_row() } else { d.flex_col() };
        let inbox = f
            .snap
            .get(
                &DataPath::parse("$inbox.open").unwrap_or_else(|| unreachable!()),
                None,
            )
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let key = Self::key_of(node, sc);
        for (name, label, screen) in ITEMS {
            let on = self.screen == name;
            let mut text = label.to_string();
            if name == "inbox" && inbox > 0 {
                text = format!("{label} {inbox}");
            }
            let accent = f.tone("accent");
            let mut item = div()
                .id(id(&format!("{key}/{name}")))
                .px(px(12.))
                .py(px(5.))
                .cursor_pointer()
                .text_size(f.ty(node, sc, "md"))
                .text_color(rgba(if on { f.tone("bg") } else { f.tone("text") }))
                .child(text);
            if pill {
                item = item.rounded(px(f.radius(node, sc).unwrap_or(14.)));
            }
            item = if on {
                item.bg(rgba(accent))
            } else {
                item.hover(|s| s.bg(rgba(accent & 0xFFFFFF00 | 0x30)))
            };
            d = d.child(item.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.emit(ThemedEvent::Screen(screen.clone()));
            })));
        }
        let accent = f.tone("accent");
        let (text_tone, size, radius) = (f.tone("text"), f.ty(node, sc, "md"), f.radius(node, sc));
        d = d.children(crate::new_session::launch_buttons(
            "themed",
            |bid, label| {
                let mut item = div()
                    .id(bid)
                    .px(px(12.))
                    .py(px(5.))
                    .cursor_pointer()
                    .text_size(size)
                    .text_color(rgba(text_tone))
                    .hover(|s| s.bg(rgba(accent & 0xFFFFFF00 | 0x30)))
                    .child(label);
                if pill {
                    item = item.rounded(px(radius.unwrap_or(14.)));
                }
                item
            },
        ));
        style(d, node, f, sc).into_any_element()
    }

    fn button(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let segment = node.kind == "segment_button";
        let primary = f.text(node, "variant", sc) == "primary";
        let selected = segment && f.flag(node, "selected", sc);
        let label = f.text(node, "label", sc);
        let accent = f.tone("accent");
        let (bg, fg) = if primary || selected {
            (accent, f.tone("bg"))
        } else {
            (f.tone("surface"), f.tone("text"))
        };
        let mut d = div()
            .id(id(&key))
            .flex()
            .items_center()
            .justify_center()
            .px(px(12.))
            .py(px(6.))
            .cursor_pointer()
            .rounded(px(f.radius(node, sc).unwrap_or(if segment {
                12.
            } else {
                8.
            })))
            .bg(rgba(bg))
            .text_color(rgba(fg))
            .text_size(f.ty(node, sc, "md"))
            .hover(|s| s.opacity(0.85))
            .child(label.clone());
        if label.chars().count() > 24 {
            d = d.tooltip(ui::tip(label));
        }
        if let Some(spec) = node.press.clone() {
            let item = sc.item.cloned();
            let composer = {
                let c = f.text(node, "composer", sc);
                (!c.is_empty()).then(|| format!("{}{c}", sc.prefix))
            };
            d = d.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.press(&spec, item.as_ref(), composer.as_deref(), window, cx);
            }));
        }
        style(d, node, f, sc).into_any_element()
    }

    fn switch(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let on = f.flag(node, "value", sc);
        let track = if on {
            f.tone("accent")
        } else {
            f.tone("muted")
        };
        let knob = div().size(px(14.)).rounded_full().bg(rgba(f.tone("bg")));
        let sw = div()
            .w(px(34.))
            .h(px(18.))
            .p(px(2.))
            .rounded_full()
            .bg(rgba(track))
            .flex()
            .when(on, |d| d.justify_end())
            .child(knob);
        let mut d = div()
            .id(id(&key))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .text_size(f.ty(node, sc, "md"))
            .child(sw)
            .child(f.text(node, "label", sc));
        if let Some(spec) = node.press.clone() {
            let item = sc.item.cloned();
            d = d.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.press(&spec, item.as_ref(), None, window, cx);
            }));
        }
        style(d, node, f, sc).into_any_element()
    }

    fn tabs(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let labels: Vec<String> = f
            .value(node, "items", sc)
            .and_then(|v| v.as_array().cloned())
            .map(|a| a.iter().map(|x| as_text(Some(x))).collect())
            .unwrap_or_default();
        let n = node.children.len().max(1);
        let current = (*self.tabs.get(&key).unwrap_or(&0)).min(n - 1);
        let mut bar = div().flex().flex_row().gap(px(4.));
        for (i, label) in labels.into_iter().enumerate() {
            let on = i == current;
            let k = key.clone();
            bar = bar.child(
                div()
                    .id(id(&format!("{key}/tab{i}")))
                    .px(px(12.))
                    .py(px(5.))
                    .cursor_pointer()
                    .rounded(px(10.))
                    .text_size(f.ty(node, sc, "md"))
                    .text_color(rgba(if on { f.tone("bg") } else { f.tone("text") }))
                    .when(on, |d| d.bg(rgba(f.tone("accent"))))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.tabs.insert(k.clone(), i);
                        cx.notify();
                    })),
            );
        }
        let body = node
            .children
            .get(current)
            .map(|c| self.draw(c, f, sc, window, cx));
        style(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .min_w(px(0.))
                .min_h(px(0.)),
            node,
            f,
            sc,
        )
        .child(bar)
        .children(body)
        .into_any_element()
    }

    /// The looks of the app. The user can always go back to Minimal.
    fn look_switch(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let mut d = div().flex().flex_row().gap(px(4.));
        for (look, name) in crate::look::LOOKS {
            d = d.child(
                div()
                    .id(id(&format!("{key}/{name}")))
                    .px(px(10.))
                    .py(px(4.))
                    .cursor_pointer()
                    .rounded(px(10.))
                    .text_size(f.ty(node, sc, "sm"))
                    .bg(rgba(f.tone("surface")))
                    .text_color(rgba(f.tone("text")))
                    .hover(|s| s.opacity(0.8))
                    .child(name)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        crate::look::leave_theme(&this.model, look, cx);
                    })),
            );
        }
        style(d, node, f, sc).into_any_element()
    }

    fn key_hint(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        let keys = f.text(node, "keys", sc);
        let label = f.text(node, "label", sc);
        style(
            div().flex().flex_row().items_center().gap(px(6.)),
            node,
            f,
            sc,
        )
        .child(
            div()
                .px(px(6.))
                .py(px(1.))
                .rounded(px(4.))
                .border_1()
                .border_color(rgba(f.tone("muted")))
                .font_family(tk().mono)
                .text_size(ui::ts(f.theme.types[0]))
                .text_color(rgba(f.tone("muted")))
                .child(keys),
        )
        .child(
            div()
                .text_size(ui::ts(f.theme.types[1]))
                .text_color(rgba(f.tone("muted")))
                .child(label),
        )
        .into_any_element()
    }

    // ---- dialogs of the app ----

    /// The decision dialog: a backdrop, the layout, and a Close button that the theme cannot
    /// remove.
    pub(super) fn dialog_frame(
        &mut self,
        f: &Frame<'_>,
        body: AnyElement,
        _session: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(rgba(0x00000099))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(640.))
                    .max_h(px(720.))
                    .overflow_hidden()
                    .rounded(px(12.))
                    .bg(rgba(f.tone("bg")))
                    .border_1()
                    .border_color(rgba(f.tone("muted") & 0xFFFFFF00 | 0x60))
                    .child(div().p(px(8.)).child(body))
                    .child(
                        div()
                            .id("themed-dialog-close")
                            .absolute()
                            .top(px(6.))
                            .right(px(6.))
                            .px(px(10.))
                            .py(px(3.))
                            .cursor_pointer()
                            .rounded(px(8.))
                            .bg(rgba(f.tone("surface")))
                            .text_size(ui::ts(12.))
                            .text_color(rgba(f.tone("text")))
                            .child("Close")
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                cx.emit(ThemedEvent::CloseDecision);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The confirm before `stop` and `autopilot`. The app writes the text; a theme cannot
    /// change it, and it cannot press the button.
    pub(super) fn confirm_dialog(
        &mut self,
        f: &Frame<'_>,
        confirm: &super::actions::Confirm,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = |label: &'static str, primary: bool| -> Stateful<Div> {
            div()
                .id(label)
                .px(px(14.))
                .py(px(6.))
                .cursor_pointer()
                .rounded(px(8.))
                .text_size(ui::ts(13.))
                .bg(rgba(if primary {
                    f.tone("accent")
                } else {
                    f.tone("surface")
                }))
                .text_color(rgba(if primary {
                    f.tone("bg")
                } else {
                    f.tone("text")
                }))
                .child(label)
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(rgba(0x000000AA))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(420.))
                    .p(px(18.))
                    .rounded(px(12.))
                    .bg(rgba(f.tone("bg")))
                    .border_1()
                    .border_color(rgba(f.tone("muted") & 0xFFFFFF00 | 0x80))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .text_size(ui::ts(14.))
                            .text_color(rgba(f.tone("text")))
                            .child(confirm.question.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .child(button("Cancel", false).on_click(cx.listener(
                                |this, _: &ClickEvent, _, cx| {
                                    this.confirm = None;
                                    cx.notify();
                                },
                            )))
                            .child(button("Confirm", true).on_click(cx.listener(
                                |this, _: &ClickEvent, _, cx| {
                                    if let Some(c) = this.confirm.take() {
                                        this.run(c.accept(), cx);
                                    }
                                    cx.notify();
                                },
                            ))),
                    ),
            )
            .into_any_element()
    }
}

/// The key of an item: its `id`, else its position.
pub fn item_key(item: &Value, index: usize) -> String {
    item.get("id")
        .and_then(Value::as_str)
        .map_or_else(|| index.to_string(), str::to_string)
}
