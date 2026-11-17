//! The parts of the Operator look (`OperatorCore`, `OperatorSession`) that the war room and the
//! session view share: the monitor frame, the token streams and the decode effect.
//!
//! Rules of the look: blue = information and decisions, green = agents. A screen is rain
//! or a console, never both. The code rain lives on the monitors of the wall; a console
//! screen shows text only.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::ops::Range;

use aivory_protocol::{LineKind, Pane};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, BoxShadow, Div, HighlightStyle, Hsla, SharedString, Stateful, StyledText,
    Window, canvas, div, linear_color_stop, linear_gradient, point, px, rgb,
};

use super::super::common::{Align, Painter, TextCache};
use crate::ui;

thread_local! {
    /// The shaped glyphs of the rain, kept between frames.
    static CACHE: RefCell<TextCache> = RefCell::default();
    /// The decode of the newest line per session: the hash of the line and the time its
    /// decode began. A line that was decoded once stays as it is.
    static DECODE: RefCell<HashMap<String, (u32, f32)>> = RefCell::default();
    /// The display lines per session and size, with the tail stamp they were made from.
    #[allow(clippy::type_complexity)]
    static LINES: RefCell<HashMap<(String, usize), ((usize, u32), Vec<String>)>> =
        RefCell::default();
}

/// Run `f` with a painter that uses the shared glyph cache.
fn painter(window: &mut Window, cx: &mut App, f: impl FnOnce(&mut Painter)) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let mut p = Painter::new(window, cx, &mut cache);
        f(&mut p);
    });
}

pub const FONT: &str = "IBM Plex Mono";

// The one type scale of the look (all sizes go through `ui::ts`). Use these, never a literal.
pub const MX_DISPLAY: f32 = 30.;
pub const MX_TITLE: f32 = 17.;
pub const MX_BODY: f32 = 14.;
pub const MX_SMALL: f32 = 12.;
/// The text of the black label on the bezel.
pub const MX_TAG: f32 = 10.;
pub const T_TITLE: f32 = MX_DISPLAY;
pub const T_SECTION: f32 = MX_TITLE;
pub const T_ENTRY: f32 = MX_BODY;
pub const T_LABEL: f32 = MX_SMALL;
/// Dim level of a screen that does not need the owner; it is 1 on hover.
pub const DIM: f32 = 0.6;
pub const PAGE: u32 = 0x020403;
pub const GREEN: u32 = 0x3DE07A;
pub const GREEN_MID: u32 = 0x2BA85A;
pub const GREEN_DIM: u32 = 0x1F7A40;
pub const GREEN_HI: u32 = 0xB8FFCC;
pub const WORD_HI: u32 = 0xD8FFE4;
pub const GREEN_PATH: u32 = 0x8FE9AE;
pub const BLUE: u32 = 0x7CCBFF;
pub const BLUE_HI: u32 = 0xCFF0FF;
pub const BLUE_RING: u32 = 0x2E8FD6;
pub const BLUE_SOFT: u32 = 0x5FA9DE;
pub const BLUE_DIM: u32 = 0x0F3A62;
pub const BLUE_INK: u32 = 0x03152A;
pub const FOOT: u32 = 0x7E8A80;
/// The state colors of the agent screens and their lines (owner 2026-11-09): gray idle,
/// blue working, red error, yellow waits for input.
pub const ST_IDLE: u32 = 0x8A968E;
pub const ST_WORK: u32 = 0x5CB8FF;
pub const ST_ERROR: u32 = 0xFF4D5E;
pub const ST_WAIT: u32 = 0xFFD23F;

/// The state color of a session, and its word for a foot.
pub fn state_color(state: aivory_protocol::SessionState) -> (u32, &'static str) {
    use aivory_protocol::SessionState as S;
    match state {
        S::Working => (ST_WORK, "working"),
        S::NeedsYou => (ST_WAIT, "waits for you"),
        S::Error => (ST_ERROR, "error"),
        S::Idle => (ST_IDLE, "idle"),
        S::Parked => (ST_IDLE, "parked"),
        S::Done => (ST_IDLE, "done"),
    }
}

/// An element id from a text.
pub fn eid(text: String) -> gpui::SharedString {
    text.into()
}

/// A color `0xRRGGBB` as a text color.
pub fn hc(color: u32) -> Hsla {
    ui::hsla((color << 8) | 0xFF)
}

/// A color `0xRRGGBB` with an alpha 0 to 255.
pub fn hca(color: u32, alpha: u8) -> Hsla {
    ui::hsla((color << 8) | u32::from(alpha))
}

/// A fast mix of two numbers. The look needs repeatable noise, not good random numbers.
pub fn hash(a: u32, b: u32) -> u32 {
    let mut x = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^ (x >> 12)
}

/// The hash of a text.
pub fn hash_str(text: &str) -> u32 {
    text.bytes()
        .fold(17u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)))
}

const GLYPHS: &[char] = &[
    'ｱ', 'ｲ', 'ｳ', 'ｴ', 'ｵ', 'ｶ', 'ｷ', 'ｸ', 'ｹ', 'ｺ', 'ｻ', 'ｼ', 'ｽ', 'ｾ', 'ｿ', 'ﾀ', 'ﾁ', 'ﾂ', 'ﾃ',
    'ﾄ', 'ﾅ', 'ﾆ', 'ﾇ', 'ﾈ', 'ﾉ', 'ﾊ', 'ﾋ', 'ﾌ', 'ﾍ', 'ﾎ', 'ﾏ', 'ﾐ', 'ﾑ', 'ﾒ', 'ﾓ', 'ﾔ', 'ﾕ', 'ﾖ',
    'ﾗ', 'ﾘ', 'ﾙ', 'ﾚ', 'ﾛ', 'ﾜ', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'Z', ':', '.',
    '=', '*', '+', '-', '<', '>', '|',
];

/// One garbled character.
pub fn glyph(a: u32, b: u32) -> char {
    GLYPHS[hash(a, b) as usize % GLYPHS.len()]
}

/// 0 to 1, fast at first and calm at the end. The charts grow with it.
pub fn ease(x: f32) -> f32 {
    let x = x.clamp(0., 1.);
    1. - (1. - x).powi(3)
}

// ---- text and scroll ---------------------------------------------------------------------------

/// A text size on the Operator screens: the board size at the default interface scale (115 %),
/// following the owner's text setting but not the window fit. A larger window shows more text
/// instead of larger text (owner 2026-11-14).
pub fn ts(size: f32) -> gpui::Pixels {
    px(size * f32::from(ui::ui_scale()) / 115.)
}

/// "Follow the bottom" of a console: new lines scroll into view while the owner is at the
/// bottom; a scroll up by the owner stops it, a scroll back to the bottom starts it again.
#[derive(Default)]
pub struct Follow {
    last_max: f32,
    loose: bool,
}

impl Follow {
    /// Follow again from the next frame (a new session on the screen).
    pub fn reset(&mut self) {
        self.loose = false;
        self.last_max = -1.;
    }

    /// Call once per frame with the handle of the scroll area.
    pub fn update(&mut self, handle: &gpui::ScrollHandle) {
        let max = f32::from(handle.max_offset().y);
        let off = -f32::from(handle.offset().y);
        let at_bottom = max - off <= 8.;
        if at_bottom {
            self.loose = false;
        } else if (max - self.last_max).abs() < 0.5 {
            // The content did not grow, yet the view is not at the bottom: the owner scrolled.
            self.loose = true;
        }
        if !self.loose && !at_bottom {
            handle.scroll_to_bottom();
        }
        self.last_max = max;
    }
}

// ---- chat input --------------------------------------------------------------------------------

/// The chat input of a green console, as a familiar text field (owner 2026-11-14: the old prompt
/// line looked like nothing and distracted): a quiet "To ..." line, then a rounded field box a
/// little darker than the screen with a thin green border (brighter while typing), the input,
/// and an Enter key at the right.
pub fn chat_field(to: String, input: AnyElement, enter: &'static str, typing: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .text_size(ts(MX_SMALL))
                .text_color(rgb(GREEN_MID))
                .child(to),
        )
        .child(
            div()
                .h(px(38.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(6.))
                .bg(rgb(0x020C06))
                .border_1()
                .border_color(hca(GREEN, if typing { 200 } else { 110 }))
                .text_size(ts(MX_BODY))
                .text_color(rgb(GREEN_HI))
                .child(div().flex_1().min_w(px(0.)).child(input))
                .child(
                    div()
                        .flex_none()
                        .h(px(22.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .bg(rgb(0x0D2A18))
                        .border_1()
                        .border_color(hca(GREEN, 80))
                        .text_size(ts(MX_SMALL))
                        .text_color(rgb(GREEN_MID))
                        .child(enter),
                ),
        )
}

// ---- room -----------------------------------------------------------------------------------

/// The dark, gray, blurred photo of the operator room with the desk top across the bottom
/// (assets/operator/room.jpg, 2400x800). The war room slides it with its pages.
pub fn room_image() -> std::sync::Arc<gpui::RenderImage> {
    static ROOM: std::sync::OnceLock<std::sync::Arc<gpui::RenderImage>> =
        std::sync::OnceLock::new();
    ROOM.get_or_init(|| {
        let bytes = include_bytes!("../../../../assets/operator/room.jpg");
        let px = image::load_from_memory(bytes)
            .map(|i| i.to_rgba8())
            .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
        crate::rts::to_render_owned(px)
    })
    .clone()
}

/// The room behind the monitors: the photo at the window height, shifted left by `shift`
/// (0 = the left end, 1 = the right end).
pub fn room(shift: f32) -> AnyElement {
    let image = room_image();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let h = f32::from(bounds.size.height);
            let w = h * 3.;
            let free = (w - f32::from(bounds.size.width)).max(0.);
            let x = f32::from(bounds.origin.x) - free * shift.clamp(0., 1.);
            let rect = gpui::Bounds::new(point(px(x), bounds.origin.y), gpui::size(px(w), px(h)));
            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                let _ = window.paint_image(
                    rect,
                    rect,
                    gpui::Corners::default(),
                    image.clone(),
                    0,
                    false,
                );
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

// ---- keys -------------------------------------------------------------------------------------

/// A physical key of the console (accepted board): flat dark cap, a hard dark edge below and a
/// soft shadow. The lit key has green text and a small LED.
pub fn keycap(on: bool) -> Div {
    let el = div()
        .flex_none()
        .h(px(26.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(5.))
        .border_1()
        .border_color(rgb(0x000000))
        .bg(rgb(0x2A2D2F))
        .shadow(vec![
            BoxShadow {
                color: hc(0x0A0B0B),
                offset: point(px(0.), px(3.)),
                blur_radius: px(0.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: hca(0x000000, 153),
                offset: point(px(0.), px(5.)),
                blur_radius: px(9.),
                spread_radius: px(0.),
                inset: false,
            },
        ])
        .text_size(px(MX_SMALL))
        .line_height(px(MX_SMALL))
        .text_color(rgb(if on { 0x3BD16F } else { 0xB9C3C8 }))
        .cursor_pointer()
        .hover(|h| h.bg(rgb(0x34383A)));
    if on {
        el.child(
            div()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(rgb(0x3BD16F))
                .shadow(vec![BoxShadow {
                    color: hca(0x3BD16F, 160),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(4.),
                    spread_radius: px(0.),
                    inset: false,
                }]),
        )
    } else {
        el
    }
}

// ---- mounts -----------------------------------------------------------------------------------

/// One steel slab, `w` long, at the height fraction `at` of its parent; `left` slabs reach
/// out of the left side of the parent, the others out of the right side. A heavy bar (bright top
/// edge, dark bottom edge, faint brushed lines) and a tall bracket plate where it meets the monitor.
fn slab(w: f32, at: f32, left: bool) -> Div {
    let shadow = |d: Div| {
        d.shadow(vec![BoxShadow {
            color: hca(0x000000, 160),
            offset: point(px(6.), px(8.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }])
    };
    let line = |y: f32| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(y))
            .h(px(1.))
            .bg(hca(0xFFFFFF, 12))
    };
    let bar = shadow(
        div()
            .absolute()
            .top(px(6.))
            .left_0()
            .right_0()
            .h(px(22.))
            .flex()
            .items_center()
            .bg(rgb(0x33383A))
            .border_t_1()
            .border_b_1()
            .border_color(rgb(0x111314)),
    )
    .child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .top_0()
            .h(px(1.))
            .bg(rgb(0x8A9496)),
    )
    .child(line(7.))
    .child(line(13.));
    // The bracket plate on the side of the monitor.
    let plate = shadow(
        div()
            .absolute()
            .top_0()
            .w(px(9.))
            .h(px(34.))
            .bg(rgb(0x2B2F31))
            .border_1()
            .border_color(rgb(0x111314)),
    )
    .child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .top_0()
            .h(px(1.))
            .bg(rgb(0x7F898B)),
    );
    let plate = if left {
        plate.right_0()
    } else {
        plate.left_0()
    };
    let el = div()
        .absolute()
        .top(gpui::relative(at))
        .mt(px(-17.))
        .w(px(w))
        .h(px(34.))
        .child(bar)
        .child(plate);
    if left {
        el.left(px(-w))
    } else {
        el.right(px(-w))
    }
}

/// The monitors hang on short horizontal steel slabs (no bolts): from the side walls into the
/// outer monitors and between neighbours (accepted board, v21 mounts). `left` and `right` are how far
/// the slabs reach out of each side (0 = none); the parent must be `relative`.
pub fn slabs(el: Div, left: f32, right: f32) -> Div {
    let mut el = el.relative();
    for at in [0.22, 0.78] {
        if left > 0. {
            el = el.child(slab(left, at, true));
        }
        if right > 0. {
            el = el.child(slab(right, at, false));
        }
    }
    el
}

// ---- monitors -----------------------------------------------------------------------------

/// The plastic of a monitor (accepted board 2026-11-14): solid dark gray, a thin rim (10 px on
/// the top and the sides, the chin is the foot), a neutral light line on the top edge from the
/// ceiling light and a cast shadow down onto the wall. No gradient, no colored glow.
pub const RIM: u32 = 0x2A2A2A;
pub fn bezel(_tint: u32) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .rounded(px(14.))
        .px(px(10.))
        .pt(px(10.))
        .bg(rgb(RIM))
        .border_1()
        .border_color(rgb(0x181818))
        .shadow(vec![
            BoxShadow {
                color: hca(0x000000, 204),
                offset: point(px(8.), px(42.)),
                blur_radius: px(50.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: hca(0x000000, 184),
                offset: point(px(2.), px(14.)),
                blur_radius: px(18.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: hca(0x000000, 230),
                offset: point(px(0.), px(2.)),
                blur_radius: px(3.),
                spread_radius: px(0.),
                inset: false,
            },
        ])
        .child(
            div()
                .absolute()
                .top(px(0.))
                .left(px(12.))
                .right(px(12.))
                .h(px(1.))
                .bg(hca(0xCDCDCD, 107)),
        )
}

/// The glass of a monitor: blue (information) or green (agent). A curved tube (round
/// corners), set into the plastic with a dark recess ring. Add the content, then `glaze` as
/// the last child.
pub fn glass(blue: bool) -> Div {
    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .rounded(px(CORNER))
        .overflow_hidden()
        .bg(rgb(if blue { 0x10243A } else { 0x041A0E }))
        .text_color(rgb(if blue { BLUE } else { GREEN }))
        .shadow(vec![
            BoxShadow {
                color: hc(0x383838),
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(4.),
                inset: false,
            },
            BoxShadow {
                color: hc(0x121212),
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(3.),
                inset: false,
            },
        ])
        .child(fx(blue))
}

/// A glass with one content element and its glaze.
pub fn screen(blue: bool, content: impl IntoElement) -> Div {
    glass(blue).child(content).child(glaze(blue))
}

/// The radius of the glass. The glaze rounds the content to it.
const CORNER: f32 = 16.;

/// What lies over the content of a glass (the CRT of paul-piper.de, adapted): scanlines every
/// 3 px, a fine aperture grille, the inner shadow of the tube, the glass sheen (light at the
/// top, darker at the bottom), the glare of the ceiling light strips and the round corners of
/// the tube. The corners cut the square content to the round glass.
pub fn glaze(_blue: bool) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            painter(window, cx, |p| {
                // scanlines: the dark middle of every third pixel row
                let mut line = y + 2.;
                while line < y + h {
                    p.rect((x, line, w, 1.), 0x000000, 0.18);
                    line += 3.;
                }
                // aperture grille: every second pixel column a little darker
                let mut col = x + 1.;
                while col < x + w {
                    p.rect((col, y, 1., h), 0x000000, 0.05);
                    col += 2.;
                }
                // the inner shadow of the tube: 60 px, strongest at the edge
                for i in 0..16 {
                    let o = i as f32 * 3.;
                    let a = 0.7 * (1. - i as f32 / 16.).powf(2.2) / 3.;
                    p.rect((x, y + o, w, 3.), 0x000000, a);
                    p.rect((x, y + h - o - 3., w, 3.), 0x000000, a);
                    p.rect((x + o, y, 3., h), 0x000000, a);
                    p.rect((x + w - o - 3., y, 3., h), 0x000000, a);
                }
                // the glass sheen: light from the top to 38 %, dark from 60 % to the bottom
                for i in 0..20 {
                    let f = i as f32 / 20.;
                    p.rect(
                        (x, y + h * 0.38 * f, w, h * 0.38 / 20. + 0.5),
                        0xFFFFFF,
                        0.09 - 0.07 * f,
                    );
                    p.rect(
                        (x, y + h * (0.6 + 0.4 * f), w, h * 0.4 / 20. + 0.5),
                        0x000000,
                        0.22 * f,
                    );
                }
                // glare: the ceiling light strips reflect as two soft bars near the top
                for (bx, by, bw, bh, a) in
                    [(0.07, 0.07, 0.44, 9., 0.13), (0.58, 0.05, 0.22, 6., 0.08)]
                {
                    for k in 0..4 {
                        let g = k as f32 * 2.;
                        p.rect(
                            (
                                x + w * bx - g,
                                y + h * by - g * 0.5,
                                w * bw + 2. * g,
                                bh + g,
                            ),
                            0xEBFAFF,
                            a / 4.,
                        );
                    }
                }
                // the corners: a square minus a quarter disc, in the color of the recess ring
                let r = CORNER;
                for (cx0, cy0, a0) in [
                    (x + r, y + r, PI),
                    (x + w - r, y + r, 1.5 * PI),
                    (x + w - r, y + h - r, 0.),
                    (x + r, y + h - r, FRAC_PI_2),
                ] {
                    let (sx, sy) = (
                        if cx0 < x + w / 2. { -1. } else { 1. },
                        if cy0 < y + h / 2. { -1. } else { 1. },
                    );
                    let mut pts = vec![[cx0 + sx * r, cy0 + sy * r]];
                    for k in 0..=8 {
                        let a = a0 + FRAC_PI_2 * k as f32 / 8.;
                        pts.push([cx0 + a.cos() * r, cy0 + a.sin() * r]);
                    }
                    p.polygon(&pts, 0x121212, 1.);
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// A monitor that does not need the owner: dim, full brightness on hover. The monitor goes
/// in as the child; give it `flex_1` to fill the height.
pub fn dimmed(id: SharedString) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .opacity(DIM)
        .hover(|s| s.opacity(1.))
}

/// The chin of a monitor (28 px): a black label of fixed width with the name of the screen
/// (gray, 10 px, right-aligned, at most 30 characters) next to the power LED in the state
/// color. The label keeps 6 px from the edges of the rim.
pub fn foot(_badge: &str, badge_color: u32, parts: Vec<(String, u32)>, _height: f32) -> Div {
    let text: String = parts
        .iter()
        .map(|(t, _)| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
        .to_uppercase()
        .chars()
        .take(30)
        .collect();
    let tag = div()
        .flex_none()
        .w(px(212.))
        .h(px(16.))
        .px(px(5.))
        .py(px(2.))
        .flex()
        .items_center()
        .justify_end()
        .overflow_hidden()
        .whitespace_nowrap()
        .bg(rgb(0x050505))
        .border_1()
        .border_color(rgb(0x000000))
        .text_size(px(MX_TAG))
        .line_height(px(MX_TAG))
        .text_color(rgb(0x7C8285))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(8.))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(hca(0xFFFFFF, 40), 0.),
                    linear_color_stop(hca(0xFFFFFF, 0), 1.),
                )),
        )
        .child(text);
    let led = div()
        .flex_none()
        .size(px(11.))
        .rounded_full()
        .bg(rgb(0x121212))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .relative()
                .size(px(6.))
                .rounded_full()
                .bg(rgb(badge_color))
                .shadow(vec![BoxShadow {
                    color: hca(badge_color, 140),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(4.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .child(
                    div()
                        .absolute()
                        .left(px(1.))
                        .top(px(1.))
                        .size(px(2.))
                        .rounded_full()
                        .bg(hca(0xFFFFFF, 204)),
                ),
        );
    div()
        .flex_none()
        .h(px(28.))
        .flex()
        .items_center()
        .justify_end()
        .gap(px(9.))
        .pr(px(3.))
        .child(div().relative().child(tag))
        .child(led)
}

/// A text with its colored runs.
pub struct Stream {
    pub text: String,
    pub runs: Vec<(Range<usize>, HighlightStyle)>,
}

impl Stream {
    pub fn into_text(self) -> StyledText {
        StyledText::new(self.text).with_highlights(self.runs)
    }
}

fn colored(color: u32) -> HighlightStyle {
    HighlightStyle {
        color: Some(hc(color)),
        ..Default::default()
    }
}

/// The newest `n` display lines of a session as console text. The agent markdown is parsed
/// (`console::display`), so no raw marker shows. The first line of a message carries the
/// speaker; the other lines of it are indented.
pub fn display_lines(pane: &Pane, n: usize) -> Vec<String> {
    // The parse is costly and the wall asks for the same lines in every frame: keep the result
    // per session and size until the tail changes.
    let stamp = (
        pane.tail.len(),
        pane.tail.last().map_or(0, |l| hash_str(&l.text)),
    );
    let key = (pane.id.clone(), n);
    if let Some(hit) = LINES.with(|c| {
        c.borrow()
            .get(&key)
            .filter(|(s, _)| *s == stamp)
            .map(|(_, v)| v.clone())
    }) {
        return hit;
    }
    let lines = parse_lines(pane, n);
    LINES.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 256 {
            c.clear();
        }
        c.insert(key, (stamp, lines.clone()));
    });
    lines
}

fn parse_lines(pane: &Pane, n: usize) -> Vec<String> {
    use crate::console::display::{DKind, last};
    last(&pane.tail, n)
        .iter()
        .map(|d| {
            let who = if d.first {
                match d.kind {
                    DKind::You => "you   ",
                    _ => "agent ",
                }
            } else {
                "      "
            };
            let text = d.text.replace('\n', " ");
            match d.kind {
                DKind::You | DKind::Agent | DKind::Bullet | DKind::Code | DKind::Table => {
                    format!("{who}{text}")
                }
                DKind::Heading => format!("{who}{}", text.to_uppercase()),
                DKind::Tool => format!("● {} {text}", d.label.as_deref().unwrap_or("")),
                DKind::Output => format!("  └ {text}"),
                DKind::Error => format!("! {text}"),
                DKind::Meta => text,
            }
        })
        .collect()
}

const KEYWORDS: &[&str] = &[
    "●", "└", ">", "Read", "Edit", "Write", "Bash", "Grep", "Task", "fn", "let", "pub", "use",
    "impl", "return", "if", "for", "ok", "ok.", "FAIL", "passed", "failed", "error",
];

fn word_color(word: &str) -> u32 {
    let mut chars = word.chars();
    let first = chars.next().unwrap_or(' ');
    if KEYWORDS.contains(&word) {
        WORD_HI
    } else if word.contains('/') || word.contains("::") || first == '"' {
        GREEN_PATH
    } else if first == '+' {
        WORD_HI
    } else if first == '-' {
        GREEN_MID
    } else if word.chars().any(|c| c.is_ascii_digit()) {
        GREEN_HI
    } else {
        GREEN
    }
}

/// The tokens of some lines: key words glow, paths are light, numbers are bright. The text is
/// exactly the text of the lines; nothing in it ever changes.
pub fn tokenize(lines: &[String]) -> Stream {
    let mut text = String::new();
    let mut runs = Vec::new();
    for line in lines {
        let mut word = String::new();
        let flush = |word: &mut String, text: &mut String, runs: &mut Vec<_>| {
            if word.is_empty() {
                return;
            }
            let start = text.len();
            text.push_str(word);
            runs.push((start..text.len(), colored(word_color(word))));
            word.clear();
        };
        for c in line.chars() {
            if c.is_whitespace() {
                flush(&mut word, &mut text, &mut runs);
                text.push(c);
            } else {
                word.push(c);
            }
        }
        flush(&mut word, &mut text, &mut runs);
        text.push('\n');
    }
    text.pop();
    Stream { text, runs }
}

/// Decode speed of the newest line in characters per second.
const DECODE_CPS: f32 = 42.;

/// A console where only the newest line is decoded by a sideways cursor of scrambled
/// characters. The cursor moves over the line to the right and leaves the real characters
/// behind it. When it has found the last character, the line is final: it is never decoded
/// again and its characters never change. The old lines are plain text. `key` names the
/// console (the session), `t` is in seconds.
pub fn decoded(key: &str, old: &[String], newest: &str, seed: u32, t: f32) -> Stream {
    let line_hash = hash_str(newest);
    let began = DECODE.with(|d| {
        let mut d = d.borrow_mut();
        let entry = d.entry(key.to_string()).or_insert((line_hash, t));
        if entry.0 != line_hash {
            *entry = (line_hash, t);
        }
        entry.1
    });
    let total = newest.chars().count();
    let found = (((t - began).max(0.) * DECODE_CPS) as usize).min(total);
    let mut shown: Vec<String> = old.to_vec();
    shown.push(newest.chars().take(found).collect());
    let mut s = tokenize(&shown);
    if found < total {
        let phase = (t * 14.) as u32;
        let start = s.text.len();
        for k in 0..(total - found).min(7) as u32 {
            s.text.push(glyph(seed ^ phase, k + 31 * found as u32));
        }
        // The head glows; the tail of the cursor fades.
        let head_end = start + s.text[start..].chars().next().map_or(0, char::len_utf8);
        s.runs.push((
            start..head_end,
            HighlightStyle {
                color: Some(hc(0xE8FFF0)),
                background_color: Some(hca(GREEN, 64)),
                ..Default::default()
            },
        ));
        if head_end < s.text.len() {
            s.runs.push((
                head_end..s.text.len(),
                HighlightStyle {
                    color: Some(hca(GREEN, 170)),
                    ..Default::default()
                },
            ));
        }
    }
    s
}

/// How busy a session is, 0.35 to 1: the share of work lines in its last lines. The rain of
/// its screen falls the faster the busier it is.
pub fn activity(pane: &Pane) -> f32 {
    let n = pane.tail.len().max(1);
    let busy = pane
        .tail
        .iter()
        .filter(|l| {
            matches!(
                l.kind,
                LineKind::Tool | LineKind::Output | LineKind::Added | LineKind::Removed
            )
        })
        .count();
    (0.35 + 0.65 * busy as f32 / n as f32).clamp(0., 1.)
}

/// Kept as the first child of a glass for the layout; the CRT effect paints in the glaze,
/// over the content (owner 2026-11-14: no colored glow around the screens).
fn fx(_blue: bool) -> AnyElement {
    div().absolute().inset_0().into_any_element()
}

/// Code rain: a few clear falling streams per screen. A stream is a piece of the output of the
/// agent (`source`), read from top to bottom, with a bright head at the bottom and a fading
/// tail above it. Streams have their own phase and black gaps between them. `activity` sets
/// the speed, `dim` the brightness (1 = full). A glyph never changes while its stream falls.
pub fn rain(source: &str, activity: f32, dim: f32, t: f32, seed: u32, tint: u32) -> AnyElement {
    // The real console text, spaces kept: a stream shows whole words of the output, read
    // from top to bottom; a space is a gap in the stream (owner 2026-11-09).
    let mut chars: Vec<char> = source
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    if chars.iter().all(|c| *c == ' ') {
        chars = "01".chars().collect();
    }
    // The word starts. Column k reads the k-th word on, so the columns from left to right
    // follow the text.
    let words: Vec<u32> = (0..chars.len())
        .filter(|&i| chars[i] != ' ' && (i == 0 || chars[i - 1] == ' '))
        .map(|i| i as u32)
        .collect();
    // The state shows in the whole stream (owner 2026-11-09): a working agent rains a bluer
    // green, an idle one rains dim; waits and errors keep the green with their tinted heads.
    let (body, dim) = match tint {
        ST_WORK => (0x48D8E6, dim),
        ST_IDLE => (0x2C8A50, dim * 0.5),
        _ => (0x3DF07A, dim),
    };
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            // Small glyphs, so that a screen shows many lines (owner 2026-11-05).
            let size = f32::from(ts(MX_SMALL));
            let lh = size * 1.15;
            let col_w = size * 1.05;
            let cols = ((w - 8.) / col_w).max(0.) as u32;
            let n = chars.len() as u32;
            painter(window, cx, |p| {
                for k in 0..cols {
                    // The density of the rain is the activity of the agent: busy agents rain
                    // in more columns. The columns are fixed by the seed.
                    if (hash(seed ^ 0x77, k) % 100) as f32 >= 55. + 45. * activity {
                        continue;
                    }
                    let r1 = hash(seed, k);
                    let r2 = hash(seed ^ 0x55AA, k);
                    let speed = (36. + 220. * activity) * (0.8 + (r1 % 40) as f32 / 100.);
                    let len = 12 + r2 % 14;
                    let tail = len as f32 * lh;
                    // One cycle: the stream falls through the screen, then a black gap.
                    let gap = h * (0.05 + (r1 >> 8) as f32 % 25. / 100.);
                    let span = h + tail + gap;
                    let pos = t * speed + (r2 % 1000) as f32 / 1000. * span;
                    let cycle = (pos / span) as u32;
                    let head = pos % span;
                    // The top glyph of the stream is the first letter of a word.
                    let word = words[((k + cycle * cols) as usize) % words.len().max(1)];
                    let start = (word + n - 1) % n;
                    let head_row = (head / lh) as i64;
                    for i in 0..len {
                        let y = (head_row - i64::from(i)) as f32 * lh;
                        if y < -lh || y > h {
                            continue;
                        }
                        // Read downward: the head is the newest character of the piece.
                        let c = chars[((start + len - i) % n) as usize];
                        if c == ' ' {
                            continue;
                        }
                        let (color, base) = match i {
                            0 | 1 => (tint, 1.),
                            _ if hash(seed ^ k, i).is_multiple_of(3) => {
                                (tint, 1. - i as f32 / len as f32)
                            }
                            _ => (body, 1. - i as f32 / len as f32),
                        };
                        let alpha = ((base * dim) * 5.).round() / 5.;
                        if alpha < 0.2 {
                            continue;
                        }
                        let family = if c.is_ascii() { FONT } else { "DotGothic16" };
                        p.text(
                            &c.to_string(),
                            size,
                            lh,
                            family,
                            false,
                            color,
                            alpha,
                            (x0 + 4. + k as f32 * col_w, y0 + y),
                            Align::Left,
                        );
                    }
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// A radar: a ring and a line that sweeps round once in 3 seconds, with a fading fan.
pub fn radar(t: f32) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let c = [x + w / 2., y + h / 2.];
            let r = w.min(h) / 2. - 1.;
            let ang = t * TAU / 3.;
            painter(window, cx, |p| {
                p.arc(c, r, (0., TAU), 1.5, BLUE_DIM, 1., &|q| q);
                let mut fan = vec![c];
                for i in 0..=8 {
                    let a = ang - 1.1 + 1.1 * i as f32 / 8.;
                    fan.push([c[0] + a.cos() * r, c[1] + a.sin() * r]);
                }
                p.polygon(&fan, BLUE_RING, 0.45);
                let tip = [c[0] + ang.cos() * r, c[1] + ang.sin() * r];
                p.strokes(&[[c[0], c[1], tip[0], tip[1]]], BLUE_HI, 1.5, 1.);
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// The title of a session for a foot.
pub fn name(pane: &Pane) -> String {
    let n = if pane.title.is_empty() {
        &pane.id
    } else {
        &pane.title
    };
    super::super::common::clip(n, 44)
}

/// The hover reveal of an agent screen: the rain turns into the console output of the agent.
/// Each character cell of the newest `lines` locks in at its own moment: a wave from the top
/// with some noise. Before its moment a cell is empty, then it flickers as a bright scrambled
/// glyph, then it shows the real character. `p` runs from 0 to 1 (1 = all text is final);
/// `t` drives the flicker. Under the text the rain fades out with `1 - p`.
#[allow(clippy::too_many_arguments)]
pub fn reveal(
    source: &str,
    lines: Vec<String>,
    act: f32,
    p: f32,
    t: f32,
    seed: u32,
    tint: u32,
    text_size: f32,
    pad: f32,
) -> AnyElement {
    let rain_dim = (0.6 * (1. - p * 1.6)).max(0.);
    let under = (rain_dim > 0.05).then(|| rain(source, act, rain_dim, t, seed, tint));
    let text = canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let size = f32::from(ts(text_size));
            let lh = (size * 1.5).round();
            let cw = size * 0.6;
            let cols = ((w - 2. * pad) / cw).max(1.) as usize;
            let rows = ((h - 2. * pad) / lh).max(1.) as usize;
            // Wrap the lines to the screen, keep the newest rows.
            let mut wrapped: Vec<Vec<char>> = Vec::new();
            for line in &lines {
                let chars: Vec<char> = line.chars().collect();
                if chars.is_empty() {
                    wrapped.push(Vec::new());
                }
                for part in chars.chunks(cols) {
                    wrapped.push(part.to_vec());
                }
            }
            let skip = wrapped.len().saturating_sub(rows);
            let shown = &wrapped[skip..];
            let frame = (t * 30.) as u32;
            painter(window, cx, |pt| {
                // A dark glass under the text, so the text reads clearly.
                pt.rect((x0, y0, w, h), 0x010A03, (p * 1.5).min(0.92));
                for (r, row) in shown.iter().enumerate() {
                    let y = y0 + pad + r as f32 * lh;
                    let wave = 0.55 * r as f32 / rows as f32;
                    if p >= 1. {
                        let s: String = row.iter().collect();
                        pt.text(
                            &s,
                            size,
                            lh,
                            FONT,
                            false,
                            GREEN_HI,
                            1.,
                            (x0 + pad, y),
                            Align::Left,
                        );
                        continue;
                    }
                    for (c, ch) in row.iter().enumerate() {
                        if ch.is_whitespace() {
                            continue;
                        }
                        let th =
                            wave + 0.4 * (hash(seed ^ r as u32, c as u32) % 1000) as f32 / 1000.;
                        if p < th {
                            continue;
                        }
                        let x = x0 + pad + c as f32 * cw;
                        if p >= th + 0.12 {
                            let s = ch.to_string();
                            let family = if ch.is_ascii() { FONT } else { "DotGothic16" };
                            pt.text(
                                &s,
                                size,
                                lh,
                                family,
                                false,
                                GREEN_HI,
                                1.,
                                (x, y),
                                Align::Left,
                            );
                        } else {
                            let g = glyph(hash(seed, frame), (r * 131 + c) as u32);
                            let family = if g.is_ascii() { FONT } else { "DotGothic16" };
                            pt.text(
                                &g.to_string(),
                                size,
                                lh,
                                family,
                                false,
                                0xF0FFF4,
                                1.,
                                (x, y),
                                Align::Left,
                            );
                        }
                    }
                }
            });
        },
    )
    .absolute()
    .inset_0();
    div()
        .absolute()
        .inset_0()
        .children(under)
        .child(text)
        .into_any_element()
}

/// Frames of TV snow, made once: every pixel a random gray, full contrast, like a television
/// without an antenna (owner reference: a canvas noise pen, 2026-11-09).
fn snow_frames() -> &'static [std::sync::Arc<gpui::RenderImage>] {
    static FRAMES: std::sync::OnceLock<Vec<std::sync::Arc<gpui::RenderImage>>> =
        std::sync::OnceLock::new();
    FRAMES.get_or_init(|| {
        const W: u32 = 480;
        const H: u32 = 300;
        (0..10u32)
            .map(|f| {
                let mut img = image::RgbaImage::new(W, H);
                for y in 0..H {
                    for x in 0..W {
                        // Black and white grain: every pixel its own random gray.
                        let r = hash(f.wrapping_mul(7919) ^ y.wrapping_mul(104_729), x);
                        let l = (r % 256) as u8;
                        img.put_pixel(x, y, image::Rgba([l, l, l, 255]));
                    }
                }
                std::sync::Arc::new(gpui::RenderImage::new(vec![image::Frame::new(img)]))
            })
            .collect()
    })
}

/// The empty center screen: no input, like a monitor that shows snow. One pre-made snow frame
/// per draw (24 frames a second), a slow dark hum bar that rolls down, and darker edges.
#[allow(dead_code, reason = "the idle screen replaced the snow")]
pub fn interference(t: f32) -> AnyElement {
    let frames = snow_frames();
    let frame = frames[(t * 24.) as usize % frames.len()].clone();
    let shade = canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            painter(window, cx, |p| {
                // A little darker, with the faint green cast of the look.
                p.rect((x0, y0, w, h), 0x000000, 0.35);
                p.rect((x0, y0, w, h), GREEN_DIM, 0.10);
                for i in 0..8 {
                    let o = i as f32 * 8.;
                    let a = 0.06;
                    p.rect((x0 + o, y0 + o, w - 2. * o, 8.), 0x000000, a);
                    p.rect((x0 + o, y0 + h - o - 8., w - 2. * o, 8.), 0x000000, a);
                    p.rect((x0 + o, y0 + o + 8., 8., h - 2. * o - 16.), 0x000000, a);
                    p.rect(
                        (x0 + w - o - 8., y0 + o + 8., 8., h - 2. * o - 16.),
                        0x000000,
                        a,
                    );
                }
            });
        },
    )
    .absolute()
    .inset_0();
    div()
        .absolute()
        .inset_0()
        .child(
            gpui::img(frame)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(gpui::ObjectFit::Fill),
        )
        .child(shade)
        .into_any_element()
}

/// The idle center screen: dark green, faint scanlines, and a slow noise band. Cheap: about a
/// hundred rectangles per frame.
pub fn idle(_t: f32) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            painter(window, cx, |p| {
                p.rect((x0, y0, w, h), 0x021208, 1.0);
                let mut y = 0.;
                while y < h {
                    p.rect((x0, y0 + y, w, 1.), 0x000000, 0.22);
                    y += 4.;
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Where the line of an agent starts: above its screen on the bottom row, or for an agent on
/// another page at the left or the right edge of the strip (earlier pages left, later right).
#[derive(Clone, Copy)]
pub enum LineFrom {
    /// A screen of the sliding row at a slot position that need not be whole (it moves).
    At(f32),
    Left,
    Right,
}

/// The lines from all agents up to the center screen, in the state color of each agent. The
/// canvas fills the thin strip between the center screen and the bottom row; `gap` is the
/// space above and below the strip, so the lines touch both screens. `wide` is the width of
/// a side column, `slots` the number of screens on the row. The end points spread over the
/// foot of the center screen in the order of the agents, so lines never cross in a bundle.
/// The line of the agent on the center screen is bright; a short light runs up each line.
pub fn state_lines(
    lines: Vec<(LineFrom, u32, bool)>,
    slots: usize,
    wide: f32,
    gap: f32,
    t: f32,
) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let n = slots.max(1) as f32;
            let tile_w = (w - (n - 1.) * gap) / n;
            let (c0, c1) = (x0 + wide + gap + 24., x0 + w - wide - gap - 24.);
            let (top, bottom) = (y0 - gap + 1., y0 + h + gap - 1.);
            let count = lines.len().max(1) as f32;
            let lefts = lines
                .iter()
                .filter(|l| matches!(l.0, LineFrom::Left))
                .count();
            let (mut li, mut ri) = (0usize, 0usize);
            let mut sheath: [Vec<Vec<[f32; 2]>>; 2] = [Vec::new(), Vec::new()];
            let mut shine: [Vec<Vec<[f32; 2]>>; 2] = [Vec::new(), Vec::new()];
            let mut cores: Vec<(u32, bool, Vec<Polyline>)> = Vec::new();
            let mut signal: Vec<Vec<[f32; 2]>> = Vec::new();
            painter(window, cx, |p| {
                for (i, (from, color, on)) in lines.iter().enumerate() {
                    let ex = c0 + (i as f32 + 0.5) / count * (c1 - c0);
                    // An agent on the row rises from its screen. An agent on another page
                    // comes in from the edge of the strip as one line of a flat bundle in
                    // the upper half of the gap, then turns up to its end point.
                    let pts: Vec<[f32; 2]> = match from {
                        LineFrom::At(s) => {
                            let s = *s;
                            let sx = x0 + s * (tile_w + gap) + tile_w / 2.;
                            let my = (bottom + top) / 2.;
                            curve([sx, bottom], [sx, my], [ex, my], [ex, top])
                        }
                        LineFrom::Left | LineFrom::Right => {
                            let left = matches!(from, LineFrom::Left);
                            let (k, of) = if left {
                                li += 1;
                                (li, lefts)
                            } else {
                                ri += 1;
                                (ri, lines.len() - lefts)
                            };
                            // The lane: the line that runs farthest runs lowest, so no line crosses another.
                            let lane = if left { k - 1 } else { of - k };
                            let band = (bottom - top) * 0.45;
                            let y = top + 6. + band * (lane as f32 + 0.5) / of.max(1) as f32;
                            // Inside the frame of the wall: the bezel at the edge stays clear.
                            let sx = if left { x0 + 8. } else { x0 + w - 8. };
                            let turn = if left { ex - 14. } else { ex + 14. };
                            let mut v = vec![[sx, y], [turn, y]];
                            v.extend(
                                curve([turn, y], [ex, y], [ex, y], [ex, top])
                                    .into_iter()
                                    .skip(1),
                            );
                            v
                        }
                    };
                    // A cable: a dark sheath, the colored core, a thin light along its top,
                    // and a dark plug where it meets a screen. The strokes are collected
                    // and drawn as a few paths after the loop (owner: the wall was slow).
                    sheath[usize::from(*on)].push(pts.clone());
                    let ci = match cores.iter().position(|c| c.0 == *color && c.1 == *on) {
                        Some(ci) => ci,
                        None => {
                            cores.push((*color, *on, Vec::new()));
                            cores.len() - 1
                        }
                    };
                    cores[ci].2.push(pts.clone());
                    shine[usize::from(*on)].push(pts.iter().map(|q| [q[0], q[1] - 0.7]).collect());
                    let end = pts[pts.len() - 1];
                    p.rect((end[0] - 3., end[1], 6., 4.), 0x1A1F1B, 1.);
                    p.rect((end[0] - 3., end[1], 6., 1.), 0x3A423C, 1.);
                    if let LineFrom::At(_) = from {
                        let st = pts[0];
                        p.rect((st[0] - 3., st[1] - 4., 6., 4.), 0x1A1F1B, 1.);
                    }
                    // A signal runs up the core now and then: a short brighter stretch.
                    let len = pts.len() as i32 - 1;
                    let head = (t * 0.35 + i as f32 * 0.137).fract() * len as f32 * 2.;
                    let at = head as i32;
                    if (3..len).contains(&at) {
                        signal.push(pts[(at - 3) as usize..=at as usize].to_vec());
                    }
                }
                // A soft shadow of every cable on the wall below it: three widening passes.
                for set in &sheath {
                    let shadow: Vec<Vec<[f32; 2]>> = set
                        .iter()
                        .map(|l| l.iter().map(|q| [q[0] + 3., q[1] + 8.]).collect())
                        .collect();
                    for (w, a) in [(10., 0.10), (7., 0.14), (4., 0.2)] {
                        p.polylines(&shadow, 0x000000, w, a);
                    }
                }
                p.polylines(&sheath[0], 0x020402, 4., 1.);
                p.polylines(&sheath[1], 0x020402, 5., 1.);
                for (color, on, set) in &cores {
                    p.polylines(
                        set,
                        *color,
                        if *on { 3. } else { 2.2 },
                        if *on { 1. } else { 0.7 },
                    );
                }
                p.polylines(&shine[0], 0xFFFFFF, 0.6, 0.18);
                p.polylines(&shine[1], 0xFFFFFF, 0.6, 0.35);
                p.polylines(&signal, 0xFFFFFF, 1.2, 0.4);
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// The points of one open line.
type Polyline = Vec<[f32; 2]>;

/// 17 points of a cubic curve.
fn curve(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> Vec<[f32; 2]> {
    (0..=16)
        .map(|j| {
            let u = j as f32 / 16.;
            let v = 1. - u;
            let f = |i: usize| {
                v * v * v * a[i] + 3. * v * v * u * b[i] + 3. * v * u * u * c[i] + u * u * u * d[i]
            };
            [f(0), f(1)]
        })
        .collect()
}
