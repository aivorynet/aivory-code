//! The Tactical look kit: the military-terminal palette (see DESIGN.md), the type scale, and the
//! pieces every Tactical screen uses: a framed panel with a caption on its top border, order
//! buttons, menu buttons, list rows and the screen frame (title row, key row, body).
//!
//! One type scale for the look: screen title > section title > entry text > label.
//! Entry text is at most 0.8 x the section title.

use gpui::prelude::*;
use gpui::{AnyElement, Div, FontWeight, Hsla, SharedString, Stateful, div, px};

use crate::ui;

/// The base: a very dark olive slate, not black. Phosphor green on pure black made every
/// element glow the same; on this base the text reads as paper and color is left for state.
pub const BG: u32 = 0x0F120E;
/// Bright text and the focus mark: warm bone white.
pub const INK: u32 = 0xE9EBDA;
/// Mid tone of lines and quiet marks.
pub const MID: u32 = 0x7E8A6C;
/// Labels and captions (olive grey, 6:1 on the base).
pub const LABEL: u32 = 0x9AA586;
/// Borders of panels and buttons.
pub const DIM: u32 = 0x3F4A37;
/// Hairlines and dim fills.
pub const DEEP: u32 = 0x252D20;
/// Amber: an agent needs you. The caution light of a cockpit, and nothing else.
pub const AMBER: u32 = 0xE5A63A;
/// Alias of `AMBER`: older code calls the attention color `WHITE`.
pub const WHITE: u32 = AMBER;
/// Desaturated cyan: information, live work, the selection (the radar scope).
pub const CYAN: u32 = 0x7FBAC0;
/// Muted olive green: nominal, done.
pub const OK: u32 = 0x9DB86A;
/// Red: a fault or an alarm, text and marks only.
pub const ALARM: u32 = 0xE0604A;
/// Text on a bright fill.
pub const DARK: u32 = 0x10140D;
/// Entry text.
pub const TEXT: u32 = 0xCDD3BC;
/// The glass of a panel.
pub const PANEL: u32 = 0x171C14;
/// The dim inset of code, diffs and tool calls.
pub const INSET: u32 = 0x0B0E09;

pub const SANS: &str = "B612";
/// B612 Mono sets the labels, times and codes. B612 sets the titles and the entry text.
pub const MONO: &str = "B612 Mono";

/// Screen title.
pub const S_TITLE: f32 = 32.;
/// Section title (the chip of a contact, a panel heading).
pub const S_SECTION: f32 = 20.;
/// Entry text: rows, questions, list items. At most 0.8 x `S_SECTION`.
pub const S_ENTRY: f32 = 15.;
/// Labels, captions, buttons.
pub const S_LABEL: f32 = 12.;

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

/// A short tick at a corner of a panel (a thin L, no glow).
fn tick(right: bool, bottom: bool) -> Div {
    let d = div().absolute().size(px(9.)).border_color(c(MID, 1.));
    let d = if right {
        d.right(px(-1.))
    } else {
        d.left(px(-1.))
    };
    let d = if bottom {
        d.bottom(px(-1.))
    } else {
        d.top(px(-1.))
    };
    let d = if bottom {
        d.border_b_1()
    } else {
        d.border_t_1()
    };
    if right {
        d.border_r_1()
    } else {
        d.border_l_1()
    }
}

/// A framed panel: a thin border, glass, corner ticks and a stencil plate on the top border
/// that holds the panel code and name (`SEC-02 / SESSIONS`). No glow. The caller sets the size
/// and the padding (24 px or more).
pub fn tbox(caption: impl Into<SharedString>) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .border_1()
        .border_color(c(DIM, 1.))
        .bg(c(PANEL, 1.))
        .child(tick(false, false))
        .child(tick(true, false))
        .child(tick(false, true))
        .child(tick(true, true))
        .child(
            div()
                .absolute()
                .left(px(14.))
                .top(px(-11.))
                .px(px(8.))
                .py(px(1.))
                .bg(c(DEEP, 1.))
                .border_1()
                .border_color(c(DIM, 1.))
                .font_family(MONO)
                .text_size(ui::ts(S_LABEL))
                .text_color(c(LABEL, 1.))
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

/// A path or a command: small mono text on a dim inset with a rule at the left. Code never
/// looks like chat text.
pub fn code(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .border_l(px(2.))
        .border_color(c(DIM, 1.))
        .bg(c(INSET, 1.))
        .font_family(MONO)
        .text_size(ui::ts(S_LABEL))
        .line_height(ui::ts(S_LABEL) * 1.5)
        .text_color(c(TEXT, 1.))
        .child(text.into())
}

/// The kind of an order button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// The first order: filled cyan.
    On,
    /// A normal order: an outline.
    Plain,
}

/// An order button `[1] ALLOW`.
pub fn order(id: impl Into<SharedString>, text: &str, kind: Btn) -> Stateful<Div> {
    let d = div()
        .id(id.into())
        .min_h(px(34.))
        .px(px(16.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .font_family(MONO)
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(S_LABEL))
        .child(text.to_string());
    match kind {
        Btn::On => d
            .border_color(c(CYAN, 1.))
            .bg(c(CYAN, 1.))
            .text_color(c(DARK, 1.))
            .hover(|h| h.bg(c(CYAN, 0.85))),
        Btn::Plain => d
            .border_color(c(DIM, 1.))
            .text_color(c(TEXT, 1.))
            .hover(|h| h.border_color(c(CYAN, 1.)).text_color(c(INK, 1.))),
    }
}

/// A menu button of a bar: flat; the one that is on has a cyan rule below it.
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
        .border_b(px(2.))
        .border_color(c(CYAN, if on { 1. } else { 0. }))
        .font_family(MONO)
        .text_size(ui::ts(S_LABEL))
        .when(on, |d| d.bg(c(DEEP, 1.)).text_color(c(INK, 1.)))
        .when(!on, |d| {
            d.text_color(c(LABEL, 1.))
                .hover(|h| h.text_color(c(INK, 1.)).bg(c(DEEP, 0.7)))
        })
        .child(text.to_string())
}

/// A list row: a state mark, a title, a line below it, and a note at the right. The picked
/// row has a cyan rule at the left.
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
        .min_h(px(62.))
        .px(px(14.))
        .py(px(8.))
        .flex()
        .items_center()
        .gap(px(14.))
        .cursor_pointer()
        .border_l(px(2.))
        .border_color(c(CYAN, if on { 1. } else { 0. }))
        .bg(c(CYAN, if on { 0.08 } else { 0. }))
        .hover(|h| h.bg(c(INK, 0.05)))
        .child(dot(mark))
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

/// A readout: a value and a short label, inline, with no box.
pub fn readout(value: &str, text: &str, color: u32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_baseline()
        .gap(px(8.))
        .child(
            div()
                .font_family(SANS)
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(S_SECTION))
                .text_color(c(color, 1.))
                .child(value.to_string()),
        )
        .child(
            div()
                .font_family(MONO)
                .text_size(ui::ts(S_LABEL))
                .text_color(c(LABEL, 1.))
                .child(text.to_string()),
        )
}

/// A state dot: round, 8 px.
pub fn dot(color: u32) -> Div {
    div()
        .flex_none()
        .size(px(8.))
        .rounded_full()
        .bg(c(color, 1.))
}

/// What a Tactical screen puts into the frame.
pub struct Frame {
    /// The screen title.
    pub title: String,
    /// The line beside the title: what the screen is for.
    pub kicker: String,
    /// Readouts at the right of the title: value, label, color.
    pub stats: Vec<(String, String, u32)>,
    /// The menu: buttons under the title.
    pub menu: Vec<AnyElement>,
    /// The body below the menu.
    pub body: AnyElement,
}

/// The layer of a Tactical screen: one title row (title, kicker, readouts), one key row, and
/// the body. The same frame on every screen.
pub fn frame(f: Frame) -> Div {
    let mut stats = div().flex().items_center().gap(px(24.));
    for (value, text, color) in f.stats {
        stats = stats.child(readout(&value, &text, color));
    }
    let short = ui::short();
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
                .px(px(28.))
                .pt(px(if short { 8. } else { 16. }))
                .pb(px(if short { 12. } else { 24. }))
                .gap(px(if short { 8. } else { 14. }))
                .when(!short, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .min_h(px(40. * ui::factor()))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(24.))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .flex()
                                    .items_baseline()
                                    .gap(px(16.))
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_family(SANS)
                                            .font_weight(FontWeight::BOLD)
                                            .text_size(ui::ts(S_TITLE))
                                            .line_height(ui::ts(S_TITLE) * 1.15)
                                            .text_color(c(INK, 1.))
                                            .whitespace_nowrap()
                                            .child(f.title.to_string()),
                                    )
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .truncate()
                                            .font_family(MONO)
                                            .text_size(ui::ts(S_LABEL))
                                            .text_color(c(LABEL, 1.))
                                            .child(f.kicker),
                                    ),
                            )
                            .child(stats),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .pb(px(6.))
                        .border_b_1()
                        .border_color(c(DEEP, 1.))
                        .children(f.menu),
                )
                .child(div().flex_1().min_h(px(0.)).flex().flex_col().child(f.body)),
        )
}
