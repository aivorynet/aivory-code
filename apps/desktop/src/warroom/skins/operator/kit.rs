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
pub const MX_DISPLAY: f32 = 16.;
pub const MX_TITLE: f32 = 11.5;
pub const MX_BODY: f32 = 10.5;
pub const MX_SMALL: f32 = 9.5;
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

// ---- monitors -----------------------------------------------------------------------------

/// The frame of a monitor with depth: a light top edge, a dark bottom edge, a drop shadow and
/// a faint glow of the screen in `tint`.
pub fn bezel(tint: u32) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .rounded(px(14.))
        .px(px(11.))
        .pt(px(11.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(rgb(0x1E241F), 0.),
            linear_color_stop(rgb(0x0F120F), 1.),
        ))
        .shadow(vec![
            BoxShadow {
                color: hca(0x000000, 190),
                offset: point(px(0.), px(7.)),
                blur_radius: px(16.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: hca(tint, 34),
                offset: point(px(0.), px(0.)),
                blur_radius: px(24.),
                spread_radius: px(1.),
                inset: false,
            },
        ])
        .child(
            div()
                .absolute()
                .top(px(1.))
                .left(px(12.))
                .right(px(12.))
                .h(px(1.))
                .bg(hca(0xFFFFFF, 56)),
        )
        .child(
            div()
                .absolute()
                .bottom(px(1.))
                .left(px(12.))
                .right(px(12.))
                .h(px(1.))
                .bg(hca(0x000000, 220)),
        )
}

/// The glass of a monitor: blue (information) or green (agent). Add the content, then
/// `glaze` as the last child.
pub fn glass(blue: bool) -> Div {
    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .rounded(px(CORNER))
        .overflow_hidden()
        .bg(rgb(if blue { BLUE_INK } else { 0x010A03 }))
        .text_color(rgb(if blue { BLUE } else { GREEN }))
        .child(fx(blue))
}

/// A glass with one content element and its glaze.
pub fn screen(blue: bool, content: impl IntoElement) -> Div {
    glass(blue).child(content).child(glaze(blue))
}

/// The radius of the glass. The glaze rounds the content to it.
const CORNER: f32 = 6.;

/// What lies over the content of a glass: an inner shadow, a faint reflection of the room and
/// the four rounded corners. The corners cut the square content to the round glass, so
/// nothing sticks out of the bezel.
pub fn glaze(blue: bool) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            painter(window, cx, |p| {
                for i in 0..6 {
                    let a = 0.34 * (1. - i as f32 / 6.);
                    let o = i as f32 * 1.5;
                    p.rect((x, y + o, w, 1.5), 0x000000, a);
                    p.rect((x, y + h - o - 1.5, w, 1.5), 0x000000, a * 0.6);
                    p.rect((x + o, y, 1.5, h), 0x000000, a * 0.7);
                    p.rect((x + w - o - 1.5, y, 1.5, h), 0x000000, a * 0.7);
                }
                let tint = if blue { 0xBFE6FF } else { 0xD8FFE4 };
                p.polygon(
                    &[
                        [x + 2., y + 2.],
                        [x + w * 0.58, y + 2.],
                        [x + w * 0.22, y + h * 0.5],
                        [x + 2., y + h * 0.62],
                    ],
                    tint,
                    0.035,
                );
                // The corners: a square minus a quarter disc, in the color of the bezel.
                let r = CORNER;
                for (cx0, cy0, a0, color) in [
                    (x + r, y + r, PI, 0x1D231E),
                    (x + w - r, y + r, 1.5 * PI, 0x1D231E),
                    (x + w - r, y + h - r, 0., 0x111511),
                    (x + r, y + h - r, FRAC_PI_2, 0x111511),
                ] {
                    let (sx, sy) = (
                        if cx0 < x + w / 2. { -1. } else { 1. },
                        if cy0 < y + h / 2. { -1. } else { 1. },
                    );
                    let mut pts = vec![[cx0 + sx * r, cy0 + sy * r]];
                    for k in 0..=6 {
                        let a = a0 + FRAC_PI_2 * k as f32 / 6.;
                        pts.push([cx0 + a.cos() * r, cy0 + a.sin() * r]);
                    }
                    p.polygon(&pts, color, 1.);
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

/// The foot of a monitor: a round badge and some parts of text.
pub fn foot(_badge: &str, badge_color: u32, parts: Vec<(String, u32)>, height: f32) -> Div {
    // Only a thin bar in the state color: the screens are no sequence, so no number.
    let mut row = div()
        .flex_none()
        .h(px(height))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(4.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(ui::ts(T_LABEL))
        .text_color(rgb(FOOT))
        .child(div().flex_none().w(px(3.)).h(px(12.)).bg(rgb(badge_color)));
    for (text, color) in parts {
        row = row.child(div().text_color(rgb(color)).child(text));
    }
    row
}

// ---- token streams ------------------------------------------------------------------------

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

/// A faint glow along the edge of a screen and fine dark scanlines. It is the first child of
/// a glass, so the content of the screen paints over it.
fn fx(blue: bool) -> AnyElement {
    let tint = if blue { BLUE } else { GREEN };
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            painter(window, cx, |p| {
                for i in 0..4 {
                    let a = 0.08 * (1. - i as f32 / 4.);
                    let o = i as f32 * 2.;
                    p.rect((x, y + o, w, 2.), tint, a);
                    p.rect((x, y + h - o - 2., w, 2.), tint, a);
                    p.rect((x + o, y, 2., h), tint, a);
                    p.rect((x + w - o - 2., y, 2., h), tint, a);
                }
                let mut line = y;
                while line < y + h {
                    p.rect((x, line, w, 1.), 0x000000, 0.22);
                    line += 3.;
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
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
            let size = f32::from(ui::ts(8.5));
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
            let size = f32::from(ui::ts(text_size));
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
            let band = (t / 9.).fract() * (h + 200.) - 100.;
            painter(window, cx, |p| {
                // A little darker, with the faint green cast of the look.
                p.rect((x0, y0, w, h), 0x000000, 0.35);
                p.rect((x0, y0, w, h), GREEN_DIM, 0.10);
                for i in 0..4 {
                    let bh = 200. - i as f32 * 45.;
                    p.rect((x0, y0 + band - bh / 2., w, bh), 0x000000, 0.08);
                }
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
pub fn idle(t: f32) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let band = (t / 14.).fract() * (h + 120.) - 60.;
            let step = ((t * 6.) as u32) % 64;
            painter(window, cx, |p| {
                p.rect((x0, y0, w, h), 0x021208, 1.0);
                let mut y = 0.;
                while y < h {
                    p.rect((x0, y0 + y, w, 1.), 0x000000, 0.22);
                    y += 4.;
                }
                p.rect((x0, y0 + band - 30., w, 60.), GREEN_DIM, 0.05);
                for i in 0..40u32 {
                    let hx = hash(i, step);
                    let fx = (hx % 997) as f32 / 997. * w;
                    let fy = band - 30. + (hash(hx, 7) % 60) as f32;
                    if fy >= 0. && fy < h {
                        p.rect((x0 + fx, y0 + fy, 2. + (hx % 14) as f32, 1.), GREEN, 0.10);
                    }
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
    Slot(usize),
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
                        LineFrom::Slot(s) => {
                            let sx = x0 + *s as f32 * (tile_w + gap) + tile_w / 2.;
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
                            let sx = if left { x0 } else { x0 + w };
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
                    if let LineFrom::Slot(_) = from {
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
