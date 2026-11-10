//! The building blocks of the Neon look: the palette, the type scale, the neon frame
//! with a glow on the frame only, titles, menu tabs, rows, buttons and the scanlines. The app
//! frame (`neon`), the session screen (`neon_focus`) and the list screens
//! (`neon_screens`) use them, so all screens speak one grammar.

use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, BoxShadow, Div, FontWeight, Pixels, SharedString, Stateful, Window, canvas,
    div, fill, point, px, rgb, rgba, size,
};

use crate::theme::tk;
use crate::ui;

pub(super) const BG: u32 = 0x05060C;
pub(super) const PANEL: u32 = 0x141A33;
pub(super) const YELLOW: u32 = 0xF3E600;
pub(super) const RED: u32 = 0xFF3355;
pub(super) const CYAN: u32 = 0x00E0F0;
pub(super) const GREEN: u32 = 0x7CFFB2;
pub(super) const PALE: u32 = 0xE9EEFF;
pub(super) const INK: u32 = 0xC9D2F0;
pub(super) const MIST: u32 = 0x8C96BC;
pub(super) const DIM: u32 = 0x5A6388;
pub(super) const DARK: u32 = 0x06070D;
pub(super) const ORBITRON: &str = "Orbitron";
pub(super) const CHAKRA: &str = "Chakra Petch";

// The type scale of the look: screen title > section title > entry text > label. Entry text
// stays under 0.8 x the section title (14 <= 0.8 x 18).
/// The title of a screen (Orbitron capitals).
pub(super) const T_SCREEN: f32 = 32.;
/// A section title above a list or a block (Orbitron capitals).
pub(super) const T_SECTION: f32 = 18.;
/// Entry text: rows, chat lines, questions, list items (Chakra Petch).
pub(super) const T_ENTRY: f32 = 14.;
/// A label, a note, a count.
pub(super) const T_LABEL: f32 = 12.;

/// Fine soft scanlines over a screen. The overlay takes no mouse input.
pub(super) fn scanlines() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |b: Bounds<Pixels>, _, window, _| {
            let h = f32::from(b.size.height);
            let mut y = 1.;
            while y < h {
                let at = Bounds::new(
                    point(b.origin.x, b.origin.y + px(y)),
                    size(b.size.width, px(1.)),
                );
                window.paint_quad(fill(at, rgba(0x00000026)));
                y += 4.;
            }
        },
    )
    .absolute()
    .inset_0()
}

/// A soft glow around a frame or a title bar.
pub(super) fn glow(color: u32, alpha: u32, blur: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: ui::hsla(color << 8 | alpha),
        offset: point(px(0.), px(0.)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// A panel with a thin neon frame and a glow on the frame. A `bright` panel is the one that
/// needs you. All others dim and light up while the mouse is over them. `group` names the
/// panel and must differ per panel on one screen.
pub(super) fn frame(color: u32, group: &'static str, bright: bool) -> Div {
    let panel = div()
        .relative()
        .flex()
        .flex_col()
        .bg(rgba(0x070A18E8))
        .border_1()
        .font_family(CHAKRA)
        .text_size(ui::ts(T_ENTRY))
        .text_color(rgb(INK));
    if bright {
        panel
            .border_color(rgba(color << 8 | 0xD0))
            .shadow(glow(color, 0x48, 22.))
    } else {
        panel
            .border_color(rgba(color << 8 | 0x66))
            .shadow(glow(color, 0x14, 14.))
            .group(group)
            .opacity(0.68)
            .group_hover(group, |s| s.opacity(1.))
    }
}

/// A section title: Orbitron capitals in neon, with a note on the right.
pub(super) fn section(title: &str, color: u32, note: Option<String>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .px(px(24.))
        .pt(px(20.))
        .pb(px(12.))
        .child(
            div()
                .font_family(ORBITRON)
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(color))
                .child(title.to_uppercase()),
        )
        .children(note.map(|n| {
            div()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(MIST))
                .child(n)
        }))
}

/// Entry text: a row text, a chat line, a question.
pub(super) fn entry(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(ui::ts(T_ENTRY))
        .line_height(ui::ts(21.))
        .text_color(rgb(PALE))
        .child(text.into())
}

/// A label: small, in the color given.
pub(super) fn label(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .text_size(ui::ts(T_LABEL))
        .line_height(ui::ts(18.))
        .text_color(rgb(color))
        .child(text.into())
}

/// A block of code or a command in the mono font.
pub(super) fn code(text: &str, color: u32) -> Div {
    div()
        .px(px(16.))
        .py(px(12.))
        .bg(rgba(0x00000099))
        .border_l_2()
        .border_color(rgba(color << 8 | 0xAA))
        .font_family(tk().mono)
        .text_size(ui::ts(T_ENTRY - 1.))
        .line_height(ui::ts(20.))
        .text_color(rgb(PALE))
        .child(text.to_string())
}

/// A neon button: an outline that fills while the mouse is over it. A `solid` button is
/// filled at rest.
pub(super) fn button(
    id: impl Into<SharedString>,
    text: &str,
    color: u32,
    solid: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .min_h(ui::sw(36.))
        .px(px(20.))
        .flex()
        .flex_none()
        .items_center()
        .cursor_pointer()
        .font_family(CHAKRA)
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(T_LABEL + 1.))
        .border_1()
        .border_color(rgb(color))
        .when(solid, |d| d.bg(rgb(color)).text_color(rgb(DARK)))
        .when(!solid, |d| {
            d.text_color(rgb(color))
                .hover(move |h| h.bg(rgb(color)).text_color(rgb(DARK)))
        })
        .child(text.to_uppercase())
}

/// A tab of the menu under a screen title. The tab on view has a lit bar.
pub(super) fn menu_tab(
    id: impl Into<SharedString>,
    text: &str,
    count: Option<usize>,
    color: u32,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .min_h(ui::sw(40.))
        .px(px(20.))
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .font_family(CHAKRA)
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(T_LABEL + 1.))
        .border_b_2()
        .border_color(rgba(if on { color << 8 | 0xFF } else { 0 }))
        .bg(rgba(if on { color << 8 | 0x1C } else { 0 }))
        .text_color(rgb(if on { color } else { MIST }))
        .hover(|h| h.text_color(rgb(PALE)))
        .child(text.to_uppercase())
        .children(count.map(|n| {
            div()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(if on { color } else { DIM }))
                .child(format!("{n:02}"))
        }))
}

/// A list row: a code, a title, a note under it, a word on the right. 54 px high.
pub(super) fn row(
    id: impl Into<SharedString>,
    code_text: &str,
    color: u32,
    texts: (&str, &str, &str),
    on: bool,
) -> Stateful<Div> {
    row_with(id, code_text, color, texts, on, (54., 1))
}

/// How many rows of height `row_h` fit in a list panel of this window. `reserved` is the room
/// of the screen that the list does not get: the header, the title, the menu, the panel head.
pub(super) fn fit_rows(window: &Window, reserved: f32, row_h: f32) -> usize {
    let h = f32::from(window.viewport_size().height);
    (((h - reserved) / row_h).floor() as usize).max(3)
}

/// Show whole rows only: when `rows` do not fit, the last place is a "+N more" row.
pub(super) fn cap_rows(mut rows: Vec<AnyElement>, fit: usize, row_h: f32) -> Vec<AnyElement> {
    if rows.len() > fit {
        let more = rows.len() - (fit - 1);
        rows.truncate(fit - 1);
        rows.push(
            div()
                .flex_none()
                .h(px(row_h))
                .px(px(24.))
                .flex()
                .items_center()
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(MIST))
                .child(format!("+{more} more"))
                .into_any_element(),
        );
    }
    rows
}

/// The shared row: `(height, title lines)`.
fn row_with(
    id: impl Into<SharedString>,
    code_text: &str,
    color: u32,
    (title, note, side): (&str, &str, &str),
    on: bool,
    (height, lines): (f32, usize),
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .h(px(height))
        .px(px(24.))
        .flex()
        .items_center()
        .gap(px(16.))
        .cursor_pointer()
        .border_l_2()
        .border_color(rgba(color << 8 | if on { 0xFF } else { 0x55 }))
        .bg(rgba(if on { color << 8 | 0x22 } else { 0 }))
        .hover(move |h| h.bg(rgba(color << 8 | 0x16)))
        .child(
            div()
                .w(px(28.))
                .flex_none()
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_LABEL))
                .text_color(rgb(color))
                .child(code_text.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .line_clamp(lines)
                        .text_size(ui::ts(T_ENTRY))
                        .line_height(ui::ts(21.))
                        .text_color(rgb(if on { 0xFFFFFF } else { PALE }))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(ui::ts(T_LABEL))
                        .line_height(ui::ts(18.))
                        .text_color(rgb(MIST))
                        .child(note.to_string()),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(ui::ts(T_LABEL))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(color))
                .child(side.to_string()),
        )
}

/// The body of a screen: the title with a glow bar, the menu, then the content. All screens
/// of the look use it, so each has a clear title and a clear menu.
pub(super) fn screen(
    title: &str,
    sub: String,
    color: u32,
    (menu, actions): (Vec<AnyElement>, Vec<AnyElement>),
    body: AnyElement,
) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgb(BG))
        .font_family(CHAKRA)
        .text_size(ui::ts(T_ENTRY))
        .text_color(rgb(INK))
        .child(
            div()
                .flex_none()
                .px(px(32.))
                .pt(px(if ui::short() { 12. } else { 24. }))
                .flex()
                .flex_wrap()
                .items_end()
                .justify_between()
                .gap_x(px(32.))
                .gap_y(px(12.))
                .when(!ui::short(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.))
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_family(ORBITRON)
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .text_size(ui::ts(T_SCREEN))
                                    .line_height(ui::ts(40.))
                                    .text_color(rgb(color))
                                    .child(title.to_uppercase()),
                            )
                            .child(
                                div()
                                    .w(px(88.))
                                    .h(px(3.))
                                    .bg(rgb(color))
                                    .shadow(glow(color, 0xC0, 14.)),
                            )
                            .child(label(sub, MIST).text_size(ui::ts(T_ENTRY))),
                    )
                })
                .child(div().flex().flex_wrap().gap(px(12.)).children(actions)),
        )
        .child(
            div()
                .flex_none()
                .mt(px(12.))
                .mx(px(32.))
                .flex()
                .flex_wrap()
                .border_b_1()
                .border_color(rgba(0x8C96BC30))
                .children(menu),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .px(px(32.))
                .py(px(if ui::short() { 10. } else { 20. }))
                .child(body),
        )
}
