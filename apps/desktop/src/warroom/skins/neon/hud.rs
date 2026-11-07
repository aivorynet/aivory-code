//! The HUD pieces of the Neon: chamfered frames with a double border, corner brackets
//! and scanlines, neon buttons that fill on hover, tags, barcodes and hex codes.

use gpui::prelude::*;
use gpui::{Bounds, Div, ElementId, FontWeight, Pixels, Stateful, canvas, div, px, rgb};

use super::city::{BG, CHAKRA, chamfer};
use crate::ui;
use crate::warroom::skins::common::{Painter, TextCache};

/// A stable 64-bit hash (FNV-1a) of an id.
pub(super) fn hash(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01B3)
    })
}

/// The short hex code of an id, for example `0x3F2A`.
pub(super) fn hex(s: &str) -> String {
    format!("0x{:04X}", hash(s) & 0xFFFF)
}

/// A wait in minutes as a short code: `NEW`, `6M`, `2H05`.
pub(super) fn wait_code(minutes: i64) -> String {
    match minutes.max(0) {
        0 => "NEW".into(),
        m if m < 60 => format!("{m}M"),
        m => format!("{}H{:02}", m / 60, m % 60),
    }
}

/// The rect of `b` as `[x, y, w, h]`.
fn rect(b: Bounds<Pixels>) -> [f32; 4] {
    [
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
    ]
}

/// A chamfered frame behind a HUD block: the dark ground, scanlines, a double border and
/// corner brackets on the two square corners. Put it first in the block. The glow sits on the
/// frame only. A `lit` frame (the one that needs you) is at full strength; the others are
/// dim and light up while the mouse is over them.
pub(super) fn frame(color: u32, cut: f32, fill: f32, lit: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, cx| {
            let k = if lit || b.contains(&window.mouse_position()) {
                1.
            } else {
                0.5
            };
            let mut cache = TextCache::default();
            let mut pen = Painter::new(window, cx, &mut cache);
            let [x, y, w, h] = rect(b);
            let outer = chamfer([x, y, w, h], cut);
            pen.polygon(&outer, BG, fill);
            pen.polygon(&outer, color, 0.03 * k);
            let mut lines = Vec::new();
            let mut ly = y + 2.;
            while ly < y + h - 1. {
                lines.push([x + 1., ly, x + w - 1., ly]);
                ly += 4.;
            }
            pen.strokes(&lines, 0xFFFFFF, 1., 0.012);
            // The glow: a wide faint stroke under the thin border.
            pen.polyline(&outer, color, 6., 0.1 * k, true);
            pen.polyline(&outer, color, 1., 0.8 * k, true);
            let inner = chamfer([x + 4., y + 4., w - 8., h - 8.], (cut - 1.6).max(0.));
            pen.polyline(&inner, color, 1., 0.18 * k, true);
            let a = 14.;
            let brackets = [
                [x, y + 1., x + a, y + 1.],
                [x + 1., y, x + 1., y + a],
                [x + w, y + h - 1., x + w - a, y + h - 1.],
                [x + w - 1., y + h, x + w - 1., y + h - a],
            ];
            pen.strokes(&brackets, color, 2.5, k);
        },
    )
    .absolute()
    .inset_0()
}

/// The ground of a button: an outline in its color, filled while the mouse is over it.
fn button_ground(color: u32, solid: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, cx| {
            let over = b.contains(&window.mouse_position());
            let mut cache = TextCache::default();
            let mut pen = Painter::new(window, cx, &mut cache);
            let shape = chamfer(rect(b), 7.);
            if over || solid {
                pen.polygon(&shape, color, 1.);
            } else {
                pen.polygon(&shape, BG, 0.75);
                pen.polygon(&shape, color, 0.06);
            }
            pen.polyline(&shape, color, 1., 1., true);
        },
    )
    .absolute()
    .inset_0()
}

/// A neon button with chamfered corners. A `solid` button is filled at rest.
pub(super) fn button(
    id: impl Into<ElementId>,
    label: &str,
    color: u32,
    solid: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .h(px(36.))
        .px(px(20.))
        .flex()
        .flex_none()
        .items_center()
        .cursor_pointer()
        .font_family(CHAKRA)
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(13.))
        .text_color(rgb(if solid { BG } else { color }))
        .hover(|s| s.text_color(rgb(BG)))
        .child(button_ground(color, solid))
        .child(div().child(label.to_uppercase()))
}

/// A small filled tag with dark text.
pub(super) fn tag(label: &str, color: u32) -> Div {
    div()
        .relative()
        .h(px(20.))
        .px(px(8.))
        .flex()
        .flex_none()
        .items_center()
        .font_family(CHAKRA)
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(12.))
        .text_color(rgb(BG))
        .child(
            canvas(
                |_, _, _| (),
                move |b, _, window, cx| {
                    let mut cache = TextCache::default();
                    let mut pen = Painter::new(window, cx, &mut cache);
                    pen.polygon(&chamfer(rect(b), 5.), color, 1.);
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(div().child(label.to_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_short() {
        assert_eq!(hex("s1"), hex("s1"));
        assert_ne!(hex("s1"), hex("s2"));
        assert_eq!(hex("anything").len(), 6);
        assert_eq!(wait_code(0), "NEW");
        assert_eq!(wait_code(6), "6M");
        assert_eq!(wait_code(125), "2H05");
    }
}
