//! The Tactical look kit (boards `TacticalRadar` and `TacticalSession`): the phosphor palette,
//! the type scale, and the pieces every Tactical screen uses: a framed panel with corner
//! brackets and a caption on its top border, order buttons, menu buttons, list rows, the screen
//! frame (title, menu, body) and the scanlines.
//!
//! One type scale for the look: screen title > section title > entry text > label.
//! Entry text is at most 0.8 x the section title.

use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, Div, FontWeight, Hsla, SharedString, Stateful, canvas, div, fill, point,
    px, size,
};

use crate::ui;

pub const BG: u32 = 0x000400;
/// Bright phosphor: the focus color.
pub const INK: u32 = 0x39FF6A;
pub const MID: u32 = 0x22C24E;
pub const LABEL: u32 = 0x1FA845;
pub const DIM: u32 = 0x128A35;
pub const DEEP: u32 = 0x0B4A1E;
/// White: "needs you", and nothing else.
pub const WHITE: u32 = 0xE8FFE8;
/// Text on a green fill.
pub const DARK: u32 = 0x001A06;
/// Entry text.
pub const TEXT: u32 = 0x8FFFAE;
/// The glass of a panel.
pub const PANEL: u32 = 0x021206;

pub const SANS: &str = "B612";
/// The proportional B612 also sets the mono roles: B612 Mono gives ":" and "." a full cell,
/// so "14:40:02Z" and "0.8K" read with gaps.
pub const MONO: &str = "B612";

/// Screen title.
pub const S_TITLE: f32 = 32.;
/// Section title (the chip of a contact, a panel heading).
pub const S_SECTION: f32 = 20.;
/// Entry text: rows, questions, list items. At most 0.8 x `S_SECTION`.
pub const S_ENTRY: f32 = 15.;
/// Labels, captions, buttons.
pub const S_LABEL: f32 = 12.;
/// Big readouts (numbers).
pub const S_BIG: f32 = 28.;

/// A color `0xRRGGBB` with an alpha of 0 to 1.
pub fn c(rgb: u32, alpha: f32) -> Hsla {
    ui::hsla(rgb << 8 | (alpha.clamp(0., 1.) * 255.) as u32)
}

/// Cut `text` to `max` characters with a trailing "..". The B612 faces have no ellipsis glyph.
pub fn dots(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(2)).collect();
    out.push_str("..");
    out
}

/// `m:ss` of a number of milliseconds.
pub fn clock_ms(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    format!("{}:{:02}", s / 60, s % 60)
}

/// `14:32:08Z` of a time in Unix milliseconds.
pub fn utc(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    format!("{:02}:{:02}:{:02}Z", s / 3600 % 24, s / 60 % 60, s % 60)
}

fn corner(top_left: bool) -> Div {
    let d = div().absolute().size(px(12.)).border_color(c(INK, 1.));
    if top_left {
        d.left(px(-1.))
            .top(px(-1.))
            .border_t(px(2.))
            .border_l(px(2.))
    } else {
        d.right(px(-1.))
            .bottom(px(-1.))
            .border_b(px(2.))
            .border_r(px(2.))
    }
}

/// A framed panel: a thin border, glass, bright corners at the top left and bottom right, and a
/// caption on the top border. The caller sets the size and the padding (24 px or more).
pub fn tbox(caption: impl Into<SharedString>) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .border_1()
        .border_color(c(DIM, 1.))
        .bg(c(PANEL, 0.82))
        .child(corner(true))
        .child(corner(false))
        .child(
            div()
                .absolute()
                .left(px(14.))
                .top(px(-10.))
                .px(px(10.))
                .py(px(1.))
                .bg(c(BG, 1.))
                .font_family(MONO)
                .text_size(ui::ts(S_LABEL))
                .text_color(c(INK, 1.))
                .child(caption.into()),
        )
}

/// A small label in the label color.
pub fn label(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(MONO)
        .text_size(ui::ts(S_LABEL))
        .line_height(ui::ts(S_LABEL) * 1.5)
        .text_color(c(LABEL, 1.))
        .child(text.into())
}

/// Entry text.
pub fn entry(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(SANS)
        .text_size(ui::ts(S_ENTRY))
        .line_height(ui::ts(S_ENTRY) * 1.5)
        .text_color(c(TEXT, 1.))
        .child(text.into())
}

/// A section title.
pub fn section(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(SANS)
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(S_SECTION))
        .line_height(ui::ts(S_SECTION) * 1.4)
        .text_color(c(INK, 1.))
        .child(text.into())
}

/// A path or a command: mono entry text in a dashed box.
pub fn code(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .border_1()
        .border_dashed()
        .border_color(c(DIM, 1.))
        .font_family(MONO)
        .text_size(ui::ts(S_LABEL + 1.))
        .line_height(ui::ts(S_LABEL + 1.) * 1.5)
        .text_color(c(TEXT, 1.))
        .child(text.into())
}

/// The kind of an order button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// The first order: filled, with a glow.
    On,
    /// A normal order: an outline.
    Plain,
    /// The last order, the one that stops something: a dim outline.
    Dim,
}

/// An order button `[1] ALLOW`.
pub fn order(id: impl Into<SharedString>, text: &str, kind: Btn) -> Stateful<Div> {
    let (line, fg, bg) = match kind {
        Btn::On => (INK, DARK, Some(INK)),
        Btn::Plain => (INK, INK, None),
        Btn::Dim => (DIM, MID, None),
    };
    let mut d = div()
        .id(id.into())
        .min_h(px(36.))
        .px(px(16.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .border_color(c(line, 1.))
        .font_family(MONO)
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(S_LABEL + 1.))
        .text_color(c(fg, 1.))
        .child(text.to_string());
    d = match bg {
        Some(b) => d.bg(c(b, 1.)).hover(move |h| h.bg(c(b, 0.85))),
        None => d.hover(move |h| h.bg(c(line, 0.18))),
    };
    d
}

/// A menu button of a bar: flat; the one that is on is filled.
pub fn navb(id: impl Into<SharedString>, text: &str, on: bool) -> Stateful<Div> {
    div()
        .id(id.into())
        .min_h(px(28.))
        .px(px(12.))
        .whitespace_nowrap()
        .flex()
        .flex_none()
        .items_center()
        .cursor_pointer()
        .font_family(MONO)
        .text_size(ui::ts(S_LABEL))
        .when(on, |d| d.bg(c(INK, 1.)).text_color(c(DARK, 1.)))
        .when(!on, |d| {
            d.text_color(c(LABEL, 1.))
                .hover(|h| h.text_color(c(INK, 1.)).bg(c(INK, 0.12)))
        })
        .child(text.to_string())
}

/// A list row of 52 px: a state mark, a title, a line below it, and a note at the right. The row
/// is dim until it is hovered or selected.
pub fn trow(
    id: impl Into<SharedString>,
    mark: u32,
    title: &str,
    sub: &str,
    note: &str,
    note_color: u32,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .min_h(px(52.))
        .px(px(14.))
        .py(px(6.))
        .flex()
        .items_center()
        .gap(px(14.))
        .cursor_pointer()
        .border_l(px(2.))
        .border_color(c(INK, if on { 1. } else { 0. }))
        .bg(c(INK, if on { 0.12 } else { 0. }))
        .opacity(if on { 1. } else { 0.7 })
        .hover(|h| h.bg(c(INK, 0.08)).opacity(1.))
        .child(div().flex_none().size(px(8.)).bg(c(mark, 1.)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_family(SANS)
                        .text_size(ui::ts(S_ENTRY))
                        .line_height(ui::ts(S_ENTRY) * 1.3)
                        .text_color(c(if on { INK } else { TEXT }, 1.))
                        .child(title.to_string()),
                )
                .when(!sub.is_empty(), |d| {
                    d.child(
                        div()
                            .truncate()
                            .font_family(MONO)
                            .text_size(ui::ts(S_LABEL))
                            .text_color(c(LABEL, 1.))
                            .child(sub.to_string()),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .font_family(MONO)
                .text_size(ui::ts(S_LABEL))
                .text_color(c(note_color, 1.))
                .child(note.to_string()),
        )
}

/// A readout: a big number and a two-word label in a bordered box.
pub fn readout(value: &str, text: &str, color: u32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .border_1()
        .border_color(c(if color == WHITE { WHITE } else { DIM }, 1.))
        .child(
            div()
                .px(px(14.))
                .py(px(6.))
                .font_family(SANS)
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(S_BIG))
                .text_color(c(color, 1.))
                .child(value.to_string()),
        )
        .child(
            div()
                .px(px(12.))
                .py(px(6.))
                .border_l_1()
                .border_color(c(DIM, 1.))
                .font_family(MONO)
                .text_size(ui::ts(S_LABEL))
                .line_height(ui::ts(S_LABEL) * 1.5)
                .text_color(c(LABEL, 1.))
                .child(text.to_string()),
        )
}

/// Scanlines over everything below: a black line of 1 px every 3 px.
pub fn scan() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |b, _, window, _| {
            let (x, w) = (f32::from(b.origin.x), f32::from(b.size.width));
            let mut y = f32::from(b.origin.y);
            let end = y + f32::from(b.size.height);
            while y < end {
                let line = Bounds::new(point(px(x), px(y)), size(px(w), px(1.)));
                window.paint_quad(fill(line, c(0x000000, 0.2)));
                y += 3.;
            }
        },
    )
    .absolute()
    .inset_0()
}

/// What a Tactical screen puts into the frame.
pub struct Frame {
    /// The screen title.
    pub title: &'static str,
    /// The line below the title: what the screen is for.
    pub kicker: String,
    /// Readouts at the right of the title: value, label, color.
    pub stats: Vec<(String, String, u32)>,
    /// The menu: buttons under the title.
    pub menu: Vec<AnyElement>,
    /// The body below the menu.
    pub body: AnyElement,
}

/// The layer of a Tactical screen: a title with readouts, a menu row, and the body. Scanlines
/// lie over it.
pub fn frame(f: Frame) -> Div {
    let mut stats = div().flex().flex_wrap().gap(px(16.));
    for (value, text, color) in f.stats {
        stats = stats.child(readout(&value, &text, color));
    }
    div()
        .size_full()
        .relative()
        .overflow_hidden()
        .bg(c(BG, 1.))
        .font_family(SANS)
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .px(px(32.))
                .pt(px(if ui::short() { 12. } else { 28. }))
                .pb(px(if ui::short() { 12. } else { 28. }))
                .gap(px(if ui::short() { 10. } else { 20. }))
                .when(!ui::short(), |d| {
                    d.child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_wrap()
                            .gap(px(12.))
                            .items_end()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .font_family(SANS)
                                            .font_weight(FontWeight::BOLD)
                                            .text_size(ui::ts(S_TITLE))
                                            .line_height(ui::ts(S_TITLE) * 1.2)
                                            .text_color(c(INK, 1.))
                                            .child(f.title.to_string()),
                                    )
                                    .child(label(f.kicker)),
                            )
                            .child(stats),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .pb(px(12.))
                        .border_b_1()
                        .border_color(c(DEEP, 1.))
                        .children(f.menu),
                )
                .child(div().flex_1().min_h(px(0.)).flex().flex_col().child(f.body)),
        )
        .child(scan())
}
