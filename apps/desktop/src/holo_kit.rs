//! Building blocks of the Holo look: the palette, the type scale, glass panels with chamfered
//! frames, the page backdrop with the slow far rings, and the list parts (tab, row, pill, island)
//! that the Holo screens use. The boards `HoloBloom` and `HoloSession` give the positions.
//!
//! The names of the colors and the helpers follow `bridge_kit`, so the Holo screens read like the
//! Bridge screens (title, menu, list, detail) in a different grammar.

use std::f32::consts::TAU;
use std::time::Duration;

use aivory_protocol::{Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    AnyElement, Div, Hsla, PathBuilder, SharedString, Stateful, Window, canvas, div, point, px,
    relative, rgb,
};

use crate::console::display::{DKind, DLine};
use crate::ui;

/// The color of a wait, and the one filled button: the thing that needs the owner.
pub const OR: u32 = 0xFF9A4D;
/// The frame lines.
pub const PU: u32 = 0x1D6E9E;
/// Data and links: the cyan of the look.
pub const PK: u32 = 0x4FD8FF;
/// A warm light tone for an error.
pub const GD: u32 = 0xFFB27A;
/// An alert.
pub const RD: u32 = 0xFF9A4D;
/// A session or item that needs you.
pub const RS: u32 = 0xFF9A4D;
/// The second working color (violet).
pub const PE: u32 = 0xA08CFF;
pub const SK: u32 = 0x8C7BFF;
pub const SA: u32 = 0xC86BFF;
/// A light cyan for the text of a link.
pub const PA: u32 = 0xCFF6FF;
/// Idle slate.
pub const GY: u32 = 0x4F7894;
/// Dark text on a filled button.
pub const BLACK: u32 = 0x03172A;
/// A hairline or an empty cell.
pub const RULE: u32 = 0x174764;
/// Text.
pub const TXT: u32 = 0xDDF8FF;
/// Entry text that is not white.
pub const DIM: u32 = 0x9FD9F2;
/// Secondary text.
pub const MUTED: u32 = 0x7FB0CF;
/// The lit tone of the warm text (the text on a wait).
pub const WARM: u32 = 0xFFE6D2;
/// The page behind the glass.
pub const PAGE: u32 = 0x02070F;

/// Display face: titles, section titles, labels and figures.
pub const LABEL: &str = "Rajdhani";
pub const FIGURE: &str = "Rajdhani";
/// Entry text: rows, chat lines, questions.
pub const BODY: &str = "IBM Plex Sans";
/// Code, commands, paths.
pub const CODE: &str = "JetBrains Mono";

/// The type scale of the look: screen title > section title > entry text > label.
/// Entry text stays under 0.8 x the section title.
pub const S_TITLE: f32 = 34.;
/// A figure or the title of a panel.
pub const S_BIG: f32 = 26.;
/// A section title above a list or a block.
pub const S_SECTION: f32 = 21.;
/// The lead entry: the question of a decision.
pub const S_LEAD: f32 = 17.;
/// Entry text: rows, chat lines, list items.
pub const S_BODY: f32 = 15.;
/// A label or a button text.
pub const S_SMALL: f32 = 13.;
/// Code and paths.
pub const S_CODE: f32 = 13.;
/// The line height of entry text.
pub const LH_BODY: f32 = 24.;
/// The inner padding of a panel, and the gap between blocks.
pub const PAD: f32 = 28.;
/// The gap between two panels.
pub const GAP: f32 = 48.;
/// The margin of the screen.
pub const MARGIN: f32 = 56.;
/// The height of the navigation bar.
pub const BAR_H: f32 = 76.;

/// The share 0 to 1 of an animation that started `elapsed` ago and lasts `ms`, eased out.
pub fn ease(elapsed: Duration, ms: f32) -> f32 {
    let x = (elapsed.as_secs_f32() * 1000. / ms).clamp(0., 1.);
    1. - (1. - x).powi(3)
}

/// The window. Holo lays out with flex boxes, so it keeps no art offsets.
#[derive(Debug, Clone, Copy)]
pub struct Art;

impl Art {
    pub fn of(_window: &Window) -> Self {
        Self
    }
}

/// A color `0xRRGGBB` with an alpha of 0 to 1.
pub fn ink(rgb: u32, alpha: f32) -> Hsla {
    ui::hsla(rgb << 8 | (alpha.clamp(0., 1.) * 255.) as u32)
}

/// `c` lit toward white for a hover.
pub fn lit(c: u32) -> u32 {
    let ch = |s: u32| {
        let v = (c >> s) & 255;
        (v + (255 - v) * 3 / 10) << s
    };
    ch(16) | ch(8) | ch(0)
}

// ---- painting ---------------------------------------------------------------------------

/// Stroke line segments `[x0, y0, x1, y1]` of one color.
pub fn strokes(window: &mut Window, segs: &[[f32; 4]], color: u32, width: f32, alpha: f32) {
    for chunk in segs.chunks(100) {
        let mut path = PathBuilder::stroke(px(width));
        for s in chunk {
            path.move_to(point(px(s[0]), px(s[1])));
            path.line_to(point(px(s[2]), px(s[3])));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, ink(color, alpha));
        }
    }
}

/// Stroke a polyline, closed on request.
pub fn line(
    window: &mut Window,
    pts: &[[f32; 2]],
    color: u32,
    width: f32,
    alpha: f32,
    closed: bool,
) {
    let Some(first) = pts.first() else {
        return;
    };
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(point(px(first[0]), px(first[1])));
    for p in &pts[1..] {
        path.line_to(point(px(p[0]), px(p[1])));
    }
    if closed {
        path.close();
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, ink(color, alpha));
    }
}

/// A line with a soft glow under it.
pub fn glow_line(window: &mut Window, pts: &[[f32; 2]], color: u32, width: f32, closed: bool) {
    line(window, pts, color, width * 4., 0.12, closed);
    line(window, pts, color, width, 1., closed);
}

/// A ring or an arc of dashes: `on` and `off` are lengths in pixels along the circle.
#[allow(clippy::too_many_arguments)]
fn dashed_ring(
    window: &mut Window,
    c: [f32; 2],
    r: f32,
    turn: f32,
    (on, off): (f32, f32),
    color: u32,
    width: f32,
    alpha: f32,
) {
    let period = on + off;
    let n = ((TAU * r / period).round() as usize).max(1);
    let da = TAU / n as f32;
    let sweep = da * on / period;
    let mut path = PathBuilder::stroke(px(width));
    let mut count = 0;
    for k in 0..n {
        let a0 = turn + k as f32 * da;
        let steps = ((sweep / 0.12).ceil() as usize).max(1);
        for i in 0..=steps {
            let a = a0 + sweep * i as f32 / steps as f32;
            let p = point(px(c[0] + a.cos() * r), px(c[1] + a.sin() * r));
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        count += 1;
        if count == 40 {
            if let Ok(done) = std::mem::replace(&mut path, PathBuilder::stroke(px(width))).build() {
                window.paint_path(done, ink(color, alpha));
            }
            count = 0;
        }
    }
    if let Ok(done) = path.build() {
        window.paint_path(done, ink(color, alpha));
    }
}

/// A solid ring.
fn plain_ring(window: &mut Window, c: [f32; 2], r: f32, color: u32, width: f32, alpha: f32) {
    let steps = ((TAU * r / 14.) as usize).max(24);
    let pts: Vec<[f32; 2]> = (0..steps)
        .map(|i| {
            let a = TAU * i as f32 / steps as f32;
            [c[0] + a.cos() * r, c[1] + a.sin() * r]
        })
        .collect();
    line(window, &pts, color, width, alpha, true);
}

/// The slow far rings of the boards, around `c`. `scale` is the window height over 900. The
/// rings turn once in 80 to 120 seconds, so they are calm.
pub fn far_rings(window: &mut Window, c: [f32; 2], scale: f32, t: f32) {
    let spin = |period: f32, rev: bool| t / period * TAU * if rev { -1. } else { 1. };
    dashed_ring(
        window,
        c,
        360. * scale,
        spin(120., true),
        (2., 6.),
        PK,
        1.,
        0.4,
    );
    // The thick violet band with a glow.
    dashed_ring(
        window,
        c,
        420. * scale,
        spin(80., false),
        (200. * scale, 250. * scale),
        SK,
        14. * scale.max(0.6),
        0.16,
    );
    plain_ring(window, c, 480. * scale, PU, 1., 0.45);
    dashed_ring(
        window,
        c,
        560. * scale,
        spin(120., true),
        (50., 86.),
        SA,
        2.,
        0.25,
    );
    plain_ring(window, c, 650. * scale, PK, 1., 0.18);
}

/// The page of a Holo screen: the deep blue glow, the fine grid, the far rings and the outer
/// frame with the notch for the navigation. It fills its parent.
pub fn backdrop(t: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
            let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
            window.paint_quad(gpui::fill(b, ink(PAGE, 1.)));
            // The glow: nested discs, brightest in the middle.
            let c = [x + w / 2., y + h * 0.49];
            let reach = w.max(h) * 0.62;
            for k in 0..16 {
                let r = reach * (1. - k as f32 / 16.);
                let disc = gpui::Bounds::new(
                    point(px(c[0] - r), px(c[1] - r * 0.78)),
                    gpui::size(px(r * 2.), px(r * 1.56)),
                );
                window.paint_quad(gpui::fill(disc, ink(0x0B2846, 0.085)).corner_radii(px(r)));
            }
            // The grid.
            let mut segs = Vec::new();
            let mut gx = 0.;
            while gx <= w {
                segs.push([x + gx, y, x + gx, y + h]);
                gx += 60.;
            }
            let mut gy = 0.;
            while gy <= h {
                segs.push([x, y + gy, x + w, y + gy]);
                gy += 60.;
            }
            strokes(window, &segs, PK, 1., 0.028);
            far_rings(window, c, (h / 900.).max(0.5), t);
            // The outer frame: chamfered corners and a notch under the navigation.
            let (m, k) = (18., 26.);
            let mid = x + w / 2.;
            let frame = [
                [x + m + k, y + m],
                [mid - 220., y + m],
                [mid - 194., y + m + k],
                [mid + 194., y + m + k],
                [mid + 220., y + m],
                [x + w - m - k, y + m],
                [x + w - m, y + m + k],
                [x + w - m, y + h - m - k],
                [x + w - m - k, y + h - m],
                [x + m + k, y + h - m],
                [x + m, y + h - m - k],
                [x + m, y + m + k],
            ];
            line(window, &frame, PK, 4., 0.05, true);
            line(window, &frame, PU, 1.2, 0.8, true);
        },
    )
    .absolute()
    .inset_0()
}

/// The logo mark: the W of AIVORY as a glowing line.
pub fn mark() -> impl IntoElement {
    div().flex_none().w(px(30.)).h(px(22.)).child(
        canvas(
            |_, _, _| (),
            |b, _, window, _| {
                let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
                let pts = [
                    [x + 2., y + 2.],
                    [x + 10., y + 20.],
                    [x + 15., y + 9.],
                    [x + 20., y + 20.],
                    [x + 28., y + 2.],
                ];
                glow_line(window, &pts, PK, 3., false);
            },
        )
        .size_full(),
    )
}

// ---- glass ---------------------------------------------------------------------------------

/// A see-through panel: a 16 px chamfer at the top left and the bottom right, a fill of
/// alpha 0.04 to 0.12, a thin frame stroke and bright corner accents. `hot` is the warm panel
/// of the one thing that needs you. The caller adds children and padding.
pub fn glass(accent: u32, hot: bool) -> Div {
    let (top, bottom) = if hot {
        (ink(0xFF9A4D, 0.12), ink(0x280E00, 0.12))
    } else {
        (ink(0x5ABEF0, 0.09), ink(0x0A2846, 0.10))
    };
    div().relative().child(
        canvas(
            |_, _, _| (),
            move |b, _, window, _| {
                let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
                let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                let k = 16.;
                let poly = [
                    [x + k, y],
                    [x + w, y],
                    [x + w, y + h - k],
                    [x + w - k, y + h],
                    [x, y + h],
                    [x, y + k],
                ];
                let mut path = PathBuilder::fill();
                let pts: Vec<_> = poly.iter().map(|p| point(px(p[0]), px(p[1]))).collect();
                path.add_polygon(&pts, true);
                if let Ok(path) = path.build() {
                    window.paint_path(
                        path,
                        gpui::linear_gradient(
                            180.,
                            gpui::linear_color_stop(top, 0.),
                            gpui::linear_color_stop(bottom, 1.),
                        ),
                    );
                }
                line(window, &poly, accent, 1., 0.45, true);
                let tl = [[x, y + 48.], [x, y + k], [x + k, y], [x + 80., y]];
                glow_line(window, &tl, accent, 2.5, false);
                if hot {
                    let br = [
                        [x + w - 80., y + h],
                        [x + w - k, y + h],
                        [x + w, y + h - k],
                        [x + w, y + h - 48.],
                    ];
                    glow_line(window, &br, accent, 2.5, false);
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

/// The tab that sits on the top edge of a panel: a filled label.
pub fn glass_tab(text: &str, hot: bool) -> Div {
    let c = if hot { OR } else { PK };
    div()
        .absolute()
        .left(px(16.))
        .top(px(-11.))
        .h(px(22.))
        .pl(px(10.))
        .pr(px(12.))
        .flex()
        .items_center()
        .rounded_l(px(3.))
        .rounded_r(px(10.))
        .bg(ink(c, 0.88))
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(ui::ts(S_SMALL))
        .text_color(rgb(BLACK))
        .whitespace_nowrap()
        .child(text.to_uppercase())
}

/// A panel with its tab and the inner padding of the look.
pub fn panel(title: &str, hot: bool) -> Div {
    glass(if hot { OR } else { 0x2AA9E0 }, hot)
        .flex()
        .flex_col()
        .p(px(PAD))
        .child(glass_tab(title, hot))
        .text_color(rgb(if hot { WARM } else { TXT }))
        .font_family(BODY)
        .text_size(ui::ts(S_BODY))
}

// ---- small parts -------------------------------------------------------------------------

/// A small label in capitals.
pub fn th(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_size(ui::ts(S_SMALL))
        .line_height(ui::ts(S_SMALL + 5.))
        .text_color(rgb(MUTED))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(SharedString::from(text.to_uppercase()))
}

/// A section title: larger than every entry below it.
pub fn sec(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_size(ui::ts(S_SECTION))
        .line_height(ui::ts(S_SECTION + 8.))
        .text_color(rgb(PA))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(SharedString::from(text.to_uppercase()))
}

/// A button in the grammar of the boards. `Primary` is the one warm filled button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Primary,
    Plain,
    Ghost,
}

pub fn button(id: impl Into<SharedString>, tone: Tone, label: &str) -> Stateful<Div> {
    let base = div()
        .id(id.into())
        .flex_none()
        .min_h(px(36.))
        .px(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(ui::ts(S_SMALL + 1.))
        .whitespace_nowrap()
        .cursor_pointer();
    match tone {
        Tone::Primary => base
            .bg(ink(OR, 0.92))
            .border_1()
            .border_color(rgb(OR))
            .text_color(rgb(0x1A0A00))
            .hover(|s| s.bg(rgb(lit(OR)))),
        Tone::Plain => base
            .bg(ink(PK, 0.06))
            .border_1()
            .border_color(ink(PK, 0.7))
            .text_color(rgb(PA))
            .hover(|s| s.bg(ink(PK, 0.2))),
        Tone::Ghost => base
            .border_1()
            .border_color(ink(0x2A5878, 0.9))
            .text_color(rgb(0x7FB3D3))
            .hover(|s| s.bg(ink(PK, 0.1)).text_color(rgb(PA))),
    }
    .child(label.to_uppercase())
}

/// A pill button in a row of answers: the warm color is the primary button, slate is the ghost.
pub fn pill(id: impl Into<SharedString>, c: u32, label: &str) -> Stateful<Div> {
    let tone = if c == OR {
        Tone::Primary
    } else if c == GY {
        Tone::Ghost
    } else {
        Tone::Plain
    };
    button(id, tone, label)
}

/// A text link such as `OPEN SESSION ›`.
pub fn link(id: impl Into<SharedString>, label: &str, c: u32) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .min_h(px(28.))
        .flex()
        .items_center()
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_size(ui::ts(S_SMALL + 1.))
        .text_color(ink(c, 0.8))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.text_color(rgb(lit(c))))
        .child(label.to_uppercase())
}

/// A menu entry above a list: dim, bright and underlined when on.
pub(crate) fn tab(id: impl Into<SharedString>, label: &str, color: u32, on: bool) -> Stateful<Div> {
    let c = if color == OR { OR } else { PK };
    div()
        .id(id.into())
        .flex_none()
        .min_h(px(34.))
        .px(px(4.))
        .flex()
        .items_center()
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_size(ui::ts(S_SMALL + 1.))
        .whitespace_nowrap()
        .cursor_pointer()
        .border_b_2()
        .border_color(ink(c, if on { 1. } else { 0. }))
        .text_color(if on {
            gpui::Hsla::from(rgb(TXT))
        } else {
            ink(MUTED, 0.75)
        })
        .hover(|s| s.text_color(rgb(TXT)))
        .child(label.to_uppercase())
}

/// Everything the island frame needs.
pub struct Island {
    pub stats: Vec<(String, String, u32)>,
    pub left_title: &'static str,
    pub left_color: u32,
    pub left_tabs: Vec<AnyElement>,
    pub left: AnyElement,
    pub right_title: String,
    pub right_tabs: Vec<AnyElement>,
    pub right: AnyElement,
}

/// The layer of a list screen: the title and the readouts on top, then two glass panels. The
/// left panel lists, the right panel shows the one selected thing; each has a menu of tabs.
pub fn island(art: Art, i: Island) -> Div {
    island_with(art, i, false)
}

/// Like `island`; with `one` only the left panel shows, over the full width (narrow windows).
pub fn island_with(_art: Art, i: Island, one: bool) -> Div {
    let hot = i.left_color == RS || i.left_color == OR;
    let mut stats = div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap_x(px(GAP / 2.))
        .gap_y(px(4.));
    for (label, value, color) in i.stats {
        stats = stats.child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(th(label))
                .child(
                    div()
                        .font_family(FIGURE)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(ui::ts(S_BIG))
                        .line_height(ui::ts(S_BIG + 4.))
                        .text_color(rgb(if color == TXT { PA } else { color }))
                        .child(value),
                ),
        );
    }
    let body = |id: &'static str, child: AnyElement| {
        div()
            .id(id)
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(child)
    };
    let menu = |tabs: Vec<AnyElement>| {
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap_x(px(20.))
            .pb(px(16.))
            .children(tabs)
    };
    let left = glass(0x2AA9E0, false)
        .when(one, |d| d.flex_1().min_w(px(0.)))
        .when(!one, |d| d.w(relative(0.46)))
        .h_full()
        .flex()
        .flex_col()
        .p(px(PAD))
        .pt(px(PAD + 6.))
        .child(glass_tab(i.left_title, hot))
        .child(menu(i.left_tabs))
        .child(body("holo-left", i.left));
    let right = glass(0x2AA9E0, false)
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex()
        .flex_col()
        .p(px(PAD))
        .pt(px(PAD + 6.))
        .child(glass_tab(&i.right_title, false))
        .child(menu(i.right_tabs))
        .child(body("holo-right", i.right));
    div()
        .size_full()
        .flex()
        .flex_col()
        .px(px(MARGIN))
        .pt(px(8.))
        .pb(px(if ui::short() { 16. } else { 40. }))
        .gap(px(if ui::short() { 12. } else { 36. }))
        .font_family(BODY)
        .text_color(rgb(TXT))
        .when(!ui::short(), |d| {
            d.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(24.))
                    .items_end()
                    .justify_between()
                    .child(
                        div()
                            .font_family(LABEL)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(ui::ts(S_TITLE))
                            .line_height(ui::ts(S_TITLE + 6.))
                            .text_color(rgb(TXT))
                            .child(i.left_title.to_uppercase()),
                    )
                    .child(stats),
            )
        })
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .gap(px(GAP))
                .child(left)
                .children((!one).then_some(right)),
        )
}

/// A row of a list: a state dot, a title with a line below, and a note at the right. `no` and
/// `chip` give the dot its color; the number is not shown.
#[allow(clippy::too_many_arguments)]
pub(crate) fn row(
    id: impl Into<SharedString>,
    _no: &str,
    chip: u32,
    title: &str,
    sub: &str,
    note: &str,
    note_color: u32,
    on: bool,
) -> Stateful<Div> {
    let dim = chip != RS && chip != OR && chip != RD && chip != GD;
    div()
        .id(id.into())
        .flex_none()
        .min_h(px(52.))
        .py(px(8.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(14.))
        .border_b_1()
        .border_color(ink(PK, 0.1))
        .cursor_pointer()
        .hover(|s| s.bg(ink(PK, 0.07)))
        .when(on, |d| {
            d.bg(ink(PK, 0.12)).border_l_2().border_color(rgb(PK))
        })
        .child(
            div()
                .flex_none()
                .size(px(8.))
                .rounded_full()
                .bg(ink(chip, if dim { 0.7 } else { 1. })),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_family(BODY)
                        .text_size(ui::ts(S_BODY))
                        .line_height(ui::ts(LH_BODY))
                        .text_color(ink(TXT, if on { 1. } else { 0.85 }))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .truncate()
                        .font_family(BODY)
                        .text_size(ui::ts(S_SMALL))
                        .line_height(ui::ts(S_SMALL + 6.))
                        .text_color(ink(MUTED, 0.8))
                        .child(sub.to_string()),
                ),
        )
        .child(
            div()
                .flex_none()
                .font_family(LABEL)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_size(ui::ts(S_SMALL))
                .text_color(rgb(note_color))
                .child(note.to_uppercase()),
        )
}

/// A block of prose in the right panel.
pub(crate) fn prose(text: &str) -> Div {
    div()
        .font_family(BODY)
        .text_size(ui::ts(S_BODY))
        .line_height(ui::ts(LH_BODY))
        .text_color(ink(TXT, 0.9))
        .child(text.to_string())
}

/// A block of code or a path.
pub(crate) fn code(text: &str, color: u32) -> Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(2.))
        .bg(ink(color, 0.05))
        .border_1()
        .border_color(ink(color, 0.25))
        .font_family(CODE)
        .text_size(ui::ts(S_CODE))
        .line_height(ui::ts(S_CODE + 9.))
        .text_color(rgb(if color == TXT { PA } else { color }))
        .child(text.to_string())
}

/// The big line of a decision or a title.
pub(crate) fn big(text: &str) -> Div {
    div()
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(ui::ts(S_BIG))
        .line_height(ui::ts(S_BIG + 6.))
        .text_color(rgb(TXT))
        .child(text.to_string())
}

/// The color of a verb pill of a tool step: edits violet, shells cyan, reads blue.
pub fn verb_colors(label: &str) -> (u32, u32) {
    match label.to_lowercase().as_str() {
        "edit" | "write" | "multiedit" | "patch" => (0x8C7BFF, 0x0A0420),
        "bash" | "shell" | "run" => (0x4FD8FF, 0x03172A),
        "task" | "agent" => (0xC86BFF, 0x1A0426),
        "grep" | "glob" | "search" => (0x2A6C92, 0xDDF8FF),
        _ => (0x1D6E9E, 0xDDF8FF),
    }
}

/// A verb pill: the tool label in a small rounded chip.
pub fn verb(label: &str) -> Div {
    let (bg, fg) = verb_colors(label);
    let mut name = label.to_uppercase();
    name.truncate(5);
    div()
        .flex_none()
        .w(px(52.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .bg(rgb(bg))
        .font_family(LABEL)
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(ui::ts(S_SMALL - 1.))
        .text_color(rgb(fg))
        .child(name)
}

/// The rows of a transcript from `console::display` lines: one label per message in the left
/// column (the speaker, or a verb pill for a tool), headings in the display face, code and
/// table lines in the mono face on a faint block. Raw markdown markers never show.
pub fn display_rows(lines: &[DLine]) -> Vec<Div> {
    let mut out = Vec::new();
    for d in lines {
        if d.kind == DKind::Meta {
            continue;
        }
        let tight = !d.first || matches!(d.kind, DKind::Code | DKind::Table | DKind::Output);
        let label: Option<Div> = match d.kind {
            DKind::You if d.first => Some(th("You").text_color(rgb(PA))),
            DKind::Agent | DKind::Heading | DKind::Bullet | DKind::Code if d.first => {
                Some(th("Agent").text_color(rgb(PK)))
            }
            DKind::Tool => Some(verb(d.label.as_deref().unwrap_or("tool"))),
            DKind::Error => Some(verb("err")),
            _ => None,
        };
        let text = d.text.clone();
        let body = match d.kind {
            DKind::You => div()
                .font_family(CODE)
                .text_size(ui::ts(S_CODE))
                .line_height(ui::ts(S_CODE + 9.))
                .text_color(rgb(DIM))
                .child(format!("> {text}")),
            DKind::Heading => div()
                .font_family(LABEL)
                .font_weight(gpui::FontWeight::BOLD)
                .text_size(ui::ts(S_LEAD))
                .line_height(ui::ts(S_LEAD + 8.))
                .text_color(rgb(PA))
                .child(text),
            DKind::Code | DKind::Table => div()
                .w_full()
                .overflow_hidden()
                .px(px(12.))
                .bg(ink(PK, 0.06))
                .font_family(CODE)
                .font_weight(if d.head {
                    gpui::FontWeight::BOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_size(ui::ts(S_CODE))
                .line_height(ui::ts(S_CODE + 8.))
                .text_color(rgb(PA))
                .whitespace_nowrap()
                .child(text),
            DKind::Tool => div()
                .overflow_hidden()
                .font_family(CODE)
                .text_size(ui::ts(S_CODE))
                .line_height(ui::ts(S_CODE + 9.))
                .text_color(rgb(PA))
                .whitespace_nowrap()
                .child(text),
            DKind::Output => div()
                .overflow_hidden()
                .font_family(CODE)
                .text_size(ui::ts(S_CODE - 1.))
                .line_height(ui::ts(S_CODE + 7.))
                .text_color(rgb(0x5F95B8))
                .whitespace_nowrap()
                .child(format!("└ {text}")),
            DKind::Error => div()
                .font_family(CODE)
                .text_size(ui::ts(S_CODE))
                .text_color(rgb(GD))
                .child(text),
            _ => div()
                .max_h(px(LH_BODY * 3.))
                .overflow_hidden()
                .line_height(ui::ts(LH_BODY))
                .text_size(ui::ts(S_BODY))
                .text_color(ink(0xBFE6F7, 0.95))
                .child(text),
        };
        out.push(
            div()
                .flex_none()
                .flex()
                .items_start()
                .gap(px(14.))
                .mt(px(if tight { 2. } else { 14. }))
                .child(div().flex_none().w(px(54.)).pt(px(3.)).children(label))
                .child(div().flex_1().min_w(px(0.)).child(body)),
        );
    }
    out
}

/// The text of a button: the digit and the label of an answer option, `1 Allow`.
pub fn option_text(i: usize, option: &str) -> String {
    format!("{} {option}", i + 1)
}

/// The color of the answer at place `i` of `n`: the first is the warm primary, the last of
/// three or more is the ghost.
pub fn option_color(i: usize, n: usize) -> u32 {
    if i == 0 {
        OR
    } else if n >= 3 && i + 1 == n {
        GY
    } else {
        PK
    }
}

/// The rank of a live session in the switcher: asks, errors, working, idle.
fn rank(p: &Pane) -> u8 {
    match p.state {
        SessionState::NeedsYou => 0,
        SessionState::Error => 1,
        SessionState::Working => 2,
        _ => 3,
    }
}

/// The live sessions in the order of the switcher. The sort is stable, so a session keeps its
/// place among the sessions of its rank.
pub fn order(war: &WarRoom) -> Vec<&Pane> {
    let mut list: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
    list.sort_by_key(|p| (rank(p), p.since));
    list
}

/// The color of a session by state: orange waits, slate idles, cyan works.
pub fn state_color(p: &Pane) -> u32 {
    match rank(p) {
        0 | 1 => OR,
        2 => PK,
        _ => GY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_answer_is_warm_and_the_last_of_three_is_slate() {
        assert_eq!(option_color(0, 3), OR);
        assert_eq!(option_color(1, 3), PK);
        assert_eq!(option_color(2, 3), GY);
        assert_eq!(option_text(1, "Deny"), "2 Deny");
    }

    #[test]
    fn the_switcher_lists_asks_first() {
        let war = aivory_protocol::demo::Demo::new().war_room;
        let list = order(&war);
        assert!(!list.is_empty());
        assert!(list.windows(2).all(|w| rank(w[0]) <= rank(w[1])));
    }

    #[test]
    fn a_hover_lights_toward_white() {
        assert_eq!(lit(0x000000), 0x4C4C4C);
        assert_eq!(lit(0xFFFFFF), 0xFFFFFF);
    }
}
