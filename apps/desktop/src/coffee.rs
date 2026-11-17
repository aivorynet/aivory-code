//! The coffee mug of aivory.net/coffee: pixel art on a 32 x 32 grid, drawn with plain
//! rectangles so it keeps its colors in every look. It sits next to each "Buy Paul a coffee"
//! link in the app.

use gpui::{Div, ParentElement, Styled, div, px, rgb};

const INK: u32 = 0x0d0d0e;
const COFFEE: u32 = 0x6b3f22;
const HEART: u32 = 0xd92d4a;
const STEAM: u32 = 0x8a8580;
const WHITE: u32 = 0xffffff;

/// One cell run: x, y, width, height (grid units), color, opacity.
const CELLS: &[(f32, f32, f32, f32, u32, f32)] = &[
    // steam
    (10., 7., 1., 2., STEAM, 0.55),
    (11., 5., 1., 2., STEAM, 0.55),
    (10., 3., 1., 2., STEAM, 0.55),
    (14., 7., 1., 2., STEAM, 0.55),
    (15., 5., 1., 2., STEAM, 0.55),
    (14., 3., 1., 2., STEAM, 0.55),
    (18., 7., 1., 2., STEAM, 0.55),
    (19., 5., 1., 2., STEAM, 0.55),
    (18., 3., 1., 2., STEAM, 0.55),
    // mug body, coffee and shine
    (6., 11., 16., 15., INK, 1.),
    (7., 11., 14., 2., COFFEE, 1.),
    (8., 14., 1., 9., WHITE, 0.18),
    // handle
    (22., 14., 4., 2., INK, 1.),
    (24., 16., 2., 5., INK, 1.),
    (22., 21., 4., 2., INK, 1.),
    // heart
    (11., 16., 2., 1., HEART, 1.),
    (15., 16., 2., 1., HEART, 1.),
    (10., 17., 8., 2., HEART, 1.),
    (11., 19., 6., 1., HEART, 1.),
    (12., 20., 4., 1., HEART, 1.),
    (13., 21., 2., 1., HEART, 1.),
    // saucer
    (3., 26., 24., 2., INK, 1.),
    (5., 28., 20., 1., INK, 0.45),
];

/// The mug, `size` px wide and high.
pub fn mug(size: f32) -> Div {
    let k = size / 32.;
    let mut out = div().relative().flex_none().w(px(size)).h(px(size));
    for &(x, y, w, h, color, opacity) in CELLS {
        out = out.child(
            div()
                .absolute()
                .left(px(x * k))
                .top(px(y * k))
                .w(px(w * k))
                .h(px(h * k))
                .bg(rgb(color))
                .opacity(opacity),
        );
    }
    out
}
