//! The Command look kit (boards `docs/design/looks/rts/`): an original pixel-art building game.
//! Riveted steel windows and buttons (9-slices cut from the boards), dark recessed wells,
//! the VT323 pixel font in four sizes (30 title, 24 section, 21 entry, 17 label), green for the
//! selection and work, amber for a wait, red for a fault.

use gpui::prelude::*;
use gpui::{Div, FontWeight, Hsla, SharedString, Stateful, div, img, px};

use crate::rts::{Frame9, frame};
use crate::ui;

/// Window and well colors.
pub const PANEL: u32 = 0x1E2124;
pub const WELL: u32 = 0x0A0C0A;
pub const CARD: u32 = 0x1E2A16;
pub const ROW_HOVER: u32 = 0x161A16;
pub const RULE: u32 = 0x1C201C;
pub const RULE_2: u32 = 0x5A5E58;
pub const STEEL: u32 = 0x3C4247;
pub const STEEL_HI: u32 = 0x6E767C;
pub const STEEL_LO: u32 = 0x101214;

/// Text: bright, entry, quiet, label, hint.
pub const WHITE: u32 = 0xF2F2E6;
pub const INK: u32 = 0xE6E8DA;
pub const INK_2: u32 = 0xD2D6C0;
pub const INK_3: u32 = 0xC3C8B0;
pub const LABEL: u32 = 0xA8AE96;
pub const HINT: u32 = 0x7E8A70;
/// Text on a green fill.
pub const DARK: u32 = 0x0B0D07;

/// Green: the selection, a free slot, the main button; the lamp of a working session.
pub const GREEN: u32 = 0x8FE05A;
pub const LAMP_RUN: u32 = 0x59E03A;
pub const GO: u32 = 0x5EA634;
pub const GO_HI: u32 = 0xA6EE76;
/// Amber: waits for you.
pub const AMBER: u32 = 0xFFC21A;
/// Red: a fault.
pub const RED: u32 = 0xFF5A43;
/// The LED counter.
pub const LED: u32 = 0xA6F06A;

pub const SANS: &str = "VT323";

pub const S_TITLE: f32 = 30.;
pub const S_SECTION: f32 = 24.;
pub const S_ENTRY: f32 = 21.;
pub const S_LABEL: f32 = 17.;

/// The gap between the panels of a window.
pub const GAP: f32 = 14.;

/// A color `0xRRGGBB` with an alpha of 0 to 1.
pub fn c(rgb: u32, alpha: f32) -> Hsla {
    ui::hsla(rgb << 8 | (alpha.clamp(0., 1.) * 255.) as u32)
}

/// Text in the pixel face at a size.
pub fn data(text: impl Into<SharedString>, s: f32) -> Div {
    div()
        .text_size(ui::ts(s))
        .line_height(ui::ts(s) * 1.05)
        .font_family(SANS)
        .font_weight(FontWeight::NORMAL)
        .text_color(c(INK, 1.))
        .child(text.into())
}

pub fn title(text: impl Into<SharedString>) -> Div {
    data(text, S_TITLE).text_color(c(WHITE, 1.))
}

pub fn section(text: impl Into<SharedString>) -> Div {
    data(text, S_SECTION)
}

pub fn entry(text: impl Into<SharedString>) -> Div {
    data(text, S_ENTRY)
}

pub fn label(text: impl Into<SharedString>) -> Div {
    data(text, S_LABEL).text_color(c(LABEL, 1.))
}

/// Upper case for the all-caps face; code and paths keep their case.
pub fn up(text: &str) -> String {
    text.to_uppercase()
}

/// `N MIN`, `N H` or `NOW` since a time.
pub fn mins(now: i64, since: i64) -> String {
    let m = ((now - since) / 60_000).max(0);
    if m < 1 {
        "NOW".into()
    } else if m < 120 {
        format!("{m} MIN")
    } else {
        format!("{} H", m / 60)
    }
}

/// `1.84M`, `640K`, `900`.
pub fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

/// A dark recessed well: black top edge, light bottom edge.
pub fn well() -> Div {
    div()
        .relative()
        .bg(c(WELL, 1.))
        .border_t_2()
        .border_color(c(0x000000, 1.))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(2.))
                .bg(c(RULE_2, 1.)),
        )
}

/// A well that holds `body` (the second argument is kept for the old callers).
pub fn panel(body: impl IntoElement, _inside: u32) -> Div {
    well().overflow_hidden().child(body)
}

/// The size of a window over the map: 92 % of the map width up to 1600, the map height minus
/// margins up to 1000.
pub fn win_size(vw: f32, vh: f32, side_w: f32) -> (f32, f32) {
    let map_w = (vw - side_w).max(200.);
    (
        (map_w * 0.92).min(1600.).floor(),
        (vh - 48.).clamp(300., 1000.).floor(),
    )
}

/// A riveted steel window of a size with a title line (`right` at its end) and `body` below it.
pub fn window(
    w: f32,
    h: f32,
    name: impl Into<SharedString>,
    right: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .child(
            img(frame(Frame9::Panel, w, h))
                .absolute()
                .left_0()
                .top_0()
                .size_full(),
        )
        .child(
            div()
                .absolute()
                .left(px(16.))
                .top(px(16.))
                .right(px(16.))
                .bottom(px(16.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .min_h(px(44.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .bg(c(WELL, 1.))
                        .border_b_2()
                        .border_color(c(RULE_2, 1.))
                        .child(title(name))
                        .child(right),
                )
                .child(div().flex_1().min_h(px(0.)).pt(px(10.)).child(body)),
        )
}

/// A steel button of a fixed size from the 9-slice image (`on` = lit green).
pub fn plate(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    w: f32,
    h: f32,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .cursor_pointer()
        .child(
            img(frame(
                if on { Frame9::ButtonOn } else { Frame9::Button },
                w,
                h,
            ))
            .absolute()
            .left_0()
            .top_0()
            .size_full(),
        )
        .child(
            data(text, 23.)
                .absolute()
                .left_0()
                .top_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(c(if on { 0xEEF8DE } else { INK }, 1.)),
        )
        .hover(|s| s.opacity(0.9))
}

/// The kinds of an answer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// Green: the main answer.
    Go,
    /// Steel: another answer.
    Plain,
    /// Steel with quiet text: the last answer (deny).
    Quiet,
}

/// A bevelled answer button that fits its text.
pub fn button(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    kind: Btn,
) -> Stateful<Div> {
    let (bg, hi, lo, ink) = match kind {
        Btn::Go => (GO, GO_HI, 0x2E5A16, DARK),
        Btn::Plain => (STEEL, STEEL_HI, STEEL_LO, INK),
        Btn::Quiet => (STEEL, STEEL_HI, STEEL_LO, 0xB8BDA8),
    };
    div()
        .id(id.into())
        .relative()
        .min_h(px(36.))
        .px(px(12.))
        .flex()
        .items_center()
        .flex_none()
        .cursor_pointer()
        .bg(c(bg, 1.))
        .border_b_2()
        .border_color(c(lo, 1.))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .h(px(2.))
                .bg(c(hi, 1.)),
        )
        .child(data(text, 22.).text_color(c(ink, 1.)))
        .hover(|s| s.opacity(0.88))
}

/// The mic button next to a text field.
pub fn mic(id: impl Into<gpui::ElementId>, listening: bool) -> Stateful<Div> {
    let color = if listening { AMBER } else { INK_2 };
    div()
        .id(id.into())
        .size(px(36.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border_2()
        .border_color(c(if listening { AMBER } else { RULE_2 }, 1.))
        .cursor_pointer()
        .hover(|s| s.border_color(c(WHITE, 1.)))
        .child(ui::icon("icons/mic.svg", 16., gpui::rgb(color)))
}

/// A thin bar: the filled share in green.
pub fn bar(share: f32) -> Div {
    div().h(px(8.)).w_full().bg(c(0x050605, 1.)).child(
        div()
            .h(px(8.))
            .w(gpui::relative(share.clamp(0., 1.)))
            .bg(c(LAMP_RUN, 1.)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_read_as_words() {
        assert_eq!(mins(600_000, 590_000), "NOW");
        assert_eq!(mins(600_000, 240_000), "6 MIN");
        assert_eq!(mins(3 * 3_600_000, 0), "3 H");
        assert_eq!(tokens(1_840_000), "1.84M");
    }
}
