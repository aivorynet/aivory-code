//! Building blocks of the Bridge look: the palette, the 1440x900 art canvas, colored frame
//! blocks, segment and pill buttons, the numbered navigation frame, and a ring chart.
//! The mockups `BridgeIslands` and `BridgeDetail` give the positions; `Art` moves the right and
//! the bottom edge when the window is larger than the art.

use std::rc::Rc;
use std::time::Duration;

use aivory_protocol::{Pane, SessionState, WarRoom};

use crate::console::display::{DKind, DLine};
use gpui::prelude::*;
use gpui::{AnyElement, App, Div, SharedString, Stateful, Window, div, px, rgb};

/// Navigation and actions.
pub const OR: u32 = 0xFF9C00;
/// The frame.
pub const PU: u32 = 0x9C639C;
/// Data.
pub const PK: u32 = 0xCE9CCE;
/// The highlight: the screen on view, an error.
pub const GD: u32 = 0xFFCE63;
/// An alert: a session waits for you.
pub const RD: u32 = 0xE8604C;
/// The fleet island.
pub const RS: u32 = 0xE8706A;
pub const PE: u32 = 0x9C9CFF;
pub const SK: u32 = 0xB4B4FF;
pub const SA: u32 = 0xFFB4A0;
pub const PA: u32 = 0xF6EF95;
pub const GY: u32 = 0x848484;
pub const BLACK: u32 = 0x000000;
/// An empty cell, and the row rules.
pub const RULE: u32 = 0x2B2233;
/// Text on black.
pub const TXT: u32 = 0xF2EDE4;
/// The one muted tone of secondary text.
pub const MUTED: u32 = 0xB9A8D8;
pub const WHITE: u32 = 0xFFFFFF;
/// Entry text that is not the focus: dimmed, full bright on hover.
pub const DIM: u32 = 0xBDB6AD;

/// Antonio 600 for labels, Antonio 700 for figures (see `assets.rs`).
pub const LABEL: &str = "Antonio SB";
/// Chat and questions.
pub const BODY: &str = "Barlow Semi Condensed";
/// Code, commands, paths.
pub const CODE: &str = "IBM Plex Mono";

/// The type scale of the look: screen title > section title > entry text > label.
/// Entry text stays under 0.8 x the section title.
pub const S_TITLE: f32 = 44.;
/// A figure or an island title.
pub const S_BIG: f32 = 30.;
/// A section title above a list or a block (Antonio capitals).
pub const S_SECTION: f32 = 22.;
/// The lead entry: the question of a decision.
pub const S_LEAD: f32 = 17.;
/// Entry text: rows, chat lines, list items.
pub const S_BODY: f32 = 16.;
/// Code, paths and commands in an entry.
pub const S_CODE: f32 = 14.;
/// A label or a button text.
pub const S_SMALL: f32 = 16.;
/// The line height of entry text (1.5).
pub const LH_BODY: f32 = 24.;
/// The inner padding of an island, and the gap between two islands or two blocks.
pub const PAD: f32 = 32.;

/// The share 0 to 1 of an animation that started `elapsed` ago and lasts `ms`, eased out.
pub fn ease(elapsed: Duration, ms: f32) -> f32 {
    let x = (elapsed.as_secs_f32() * 1000. / ms).clamp(0., 1.);
    1. - (1. - x).powi(3)
}

/// The size of the mockups.
pub const ART_W: f32 = 1440.;
pub const ART_H: f32 = 900.;

/// A text size or a text height of the look, scaled like the interface text.
pub fn tsz(size: f32) -> gpui::Pixels {
    crate::ui::ts(size)
}

/// The scale factor of the interface text (the setting and the window fit).
pub fn kf() -> f32 {
    f32::from(crate::ui::sw(1.))
}

/// The window against the 1440x900 art. A left edge stays; a right edge moves with `dx`, a
/// bottom edge with `dy`.
#[derive(Debug, Clone, Copy)]
pub struct Art {
    pub dx: f32,
    pub dy: f32,
}

impl Art {
    pub fn of(window: &Window) -> Self {
        let s = window.viewport_size();
        Self::sized(f32::from(s.width), f32::from(s.height))
    }

    pub fn sized(w: f32, h: f32) -> Self {
        Self {
            dx: (w - ART_W).max(-480.),
            dy: (h - ART_H).max(-300.),
        }
    }

    /// The x of a right-anchored art x.
    pub fn rx(&self, x: f32) -> f32 {
        x + self.dx
    }

    /// The y of a bottom-anchored art y.
    pub fn by(&self, y: f32) -> f32 {
        y + self.dy
    }
}

/// `c` lit toward white for a hover.
pub fn lit(c: u32) -> u32 {
    let ch = |s: u32| {
        let v = (c >> s) & 255;
        (v + (255 - v) * 3 / 10) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// A colored rectangle at a place of the art.
pub fn rect(x: f32, y: f32, w: f32, h: f32, c: u32) -> Div {
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(w))
        .h(px(h))
        .bg(rgb(c))
}

/// A box at a place of the art, without a color.
pub fn area(x: f32, y: f32, w: f32, h: f32) -> Div {
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
}

/// A frame segment button: a number at the left, a name at the right, black on color.
#[allow(clippy::too_many_arguments)]
pub fn seg(
    id: impl Into<SharedString>,
    (x, y, w, h): (f32, f32, f32, f32),
    c: u32,
    size: f32,
    no: &str,
    name: &str,
) -> Stateful<Div> {
    rect(x, y, w, h, c)
        .id(id.into())
        .flex()
        .items_end()
        .justify_between()
        .gap(px(6.))
        .px(px(12.))
        .pb(px(4.))
        .font_family(LABEL)
        .text_size(px(size))
        .line_height(px(size))
        .text_color(rgb(BLACK))
        .whitespace_nowrap()
        .overflow_hidden()
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(lit(c))))
        .child(no.to_string())
        .child(div().min_w(px(0.)).truncate().child(name.to_uppercase()))
}

/// A column button in the flow of a list: the label sits at the lower right, black on color.
pub fn ibtn_flow(id: impl Into<SharedString>, c: u32, label: &str) -> Stateful<Div> {
    div()
        .id(id.into())
        .bg(rgb(c))
        .flex()
        .items_end()
        .justify_end()
        .pl(px(16.))
        .pr(px(16.))
        .pb(px(8.))
        .font_family(LABEL)
        .text_size(tsz(S_SMALL))
        .line_height(tsz(S_SMALL))
        .text_color(rgb(BLACK))
        .whitespace_nowrap()
        .overflow_hidden()
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(lit(c))))
        .child(div().min_w(px(0.)).truncate().child(label.to_uppercase()))
}

/// A pill button in a row of answers.
pub fn pill(id: impl Into<SharedString>, c: u32, label: &str) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .min_h(tsz(28.))
        .px(px(14.))
        .rounded_full()
        .bg(rgb(c))
        .flex()
        .items_center()
        .justify_center()
        .pt(px(2.))
        .font_family(LABEL)
        .text_size(tsz(S_SMALL))
        .line_height(tsz(S_SMALL))
        .text_color(rgb(BLACK))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(lit(c))))
        .child(label.to_uppercase())
}

/// A small label in lilac capitals.
pub fn th(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .font_family(LABEL)
        .text_size(tsz(S_SMALL))
        .line_height(tsz(S_SMALL + 2.))
        .text_color(rgb(MUTED))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(SharedString::from(text.to_uppercase()))
}

/// A section title: Antonio capitals, larger than every entry below it.
pub fn sec(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .font_family(LABEL)
        .text_size(tsz(S_SECTION))
        .line_height(tsz(S_SECTION + 6.))
        .text_color(rgb(MUTED))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(SharedString::from(text.to_uppercase()))
}

/// What a click on a nav segment does: the index 0 to 4 of War room to Settings.
pub type NavFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The numbered segments of the frame (top, height, color). The sixth, Detail, shows only
/// when a session is selected.
const NAV: [(&str, f32, f32, u32); 6] = [
    ("War room", 16., 34., OR),
    ("Inbox", 54., 24., OR),
    ("Tasks", 82., 24., OR),
    ("Repos", 110., 24., PK),
    ("Settings", 138., 24., PK),
    ("Detail", 166., 20., SK),
];

/// The index of the Detail segment in `NavFn` calls.
pub const NAV_DETAIL: usize = 5;

/// The numbered navigation. The screen on view is gold; `inbox` counts open items; `detail`
/// adds the Detail entry (a session is selected).
pub fn nav(
    active: Option<usize>,
    sub: Option<usize>,
    inbox: u32,
    detail: bool,
    go: &NavFn,
) -> Vec<AnyElement> {
    NAV.iter()
        .enumerate()
        .filter(|(i, _)| *i != NAV_DETAIL || detail)
        .map(|(i, &(name, y, h, c))| {
            let c = if active == Some(i) { GD } else { c };
            let name = if i == 1 && inbox > 0 {
                format!("{name} {inbox:02}")
            } else {
                name.to_string()
            };
            let go = go.clone();
            seg(
                format!("lc-nav-{i}"),
                (16., y, 150., h),
                c,
                S_SMALL,
                &if sub == Some(i) {
                    format!("{:02}·1", i + 1)
                } else {
                    format!("{:02}", i + 1)
                },
                &name,
            )
            .on_click(move |_, w, cx| go(i, w, cx))
            .into_any_element()
        })
        .collect()
}

/// The upper elbow: the column bends into a thin bar at y 234. The bars are returned apart,
/// so the caller can put text on them.
pub fn upper_elbow(art: Art) -> Vec<Div> {
    vec![
        rect(16., 188., 262., 62., PU).rounded_bl(px(54.)),
        rect(166., 188., 115., 46., BLACK).rounded_bl(px(26.)),
        rect(282., 234., 700. + art.dx, 16., PU),
    ]
}

/// A thin bar of the frame, 16 px high.
pub fn bar(x: f32, y: f32, w: f32, c: u32) -> Div {
    rect(x, y, w, 16., c)
}

/// Text on a bar: right-aligned, black, 14 px.
pub fn bar_text(el: Div, text: &str) -> Div {
    el.flex()
        .items_center()
        .justify_end()
        .pr(px(16.))
        .font_family(LABEL)
        .text_size(px(14.))
        .line_height(px(14.))
        .text_color(rgb(BLACK))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text.to_uppercase())
}

/// The lower elbow of a screen frame at the bottom left, with its bar at the bottom.
pub fn lower_elbow(art: Art) -> Vec<Div> {
    vec![
        rect(16., art.by(790.), 262., 94., PU).rounded_bl(px(54.)),
        rect(166., art.by(790.), 115., 78., BLACK).rounded_bl(px(26.)),
    ]
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

/// The live sessions in the order of the Bridge switcher. The sort is stable, so a session
/// keeps its place among the sessions of its rank.
pub fn order(war: &WarRoom) -> Vec<&Pane> {
    let mut list: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
    list.sort_by_key(|p| (rank(p), p.since));
    list
}

/// The color of a session by state: red waits, gold errs, lilac works, grey idles.
pub fn state_color(p: &Pane) -> u32 {
    match rank(p) {
        0 => RD,
        1 => GD,
        2 => PK,
        _ => GY,
    }
}

/// The text of a button: the digit and the label of an answer option, `1 Allow`.
pub fn option_text(i: usize, option: &str) -> String {
    format!("{} {option}", i + 1)
}

/// The color of the answer at place `i` of `n`: the last of two or more is the lilac "no".
pub fn option_color(i: usize, n: usize) -> u32 {
    if n >= 2 && i + 1 == n { PK } else { OR }
}

/// The gap above the first line of a message.
pub const MSG_GAP: f32 = 16.;

/// The lines that a display line takes on screen: prose wraps to 3 lines at most; code, table
/// and tool lines take one.
fn display_lines(d: &DLine, chars: usize) -> usize {
    match d.kind {
        DKind::You | DKind::Agent | DKind::Bullet => {
            d.text.chars().count().div_ceil(chars.max(1)).clamp(1, 3)
        }
        _ => 1,
    }
}

/// The height of a display line, with the gap above a message.
pub fn display_h(d: &DLine, chars: usize) -> f32 {
    display_lines(d, chars) as f32 * LH_BODY * kf() + if d.first { MSG_GAP } else { 0. }
}

/// The lines that fit in `avail` px, newest at the end. The list starts at a message when one
/// starts inside the room; else it holds the newest whole lines of the last message.
pub fn fit_display(all: Vec<DLine>, avail: f32, chars: usize) -> Vec<DLine> {
    let mut h = 0.;
    let mut at = all.len();
    while at > 0 {
        let c = display_h(&all[at - 1], chars);
        if h + c > avail {
            break;
        }
        h += c;
        at -= 1;
    }
    let mut v: Vec<DLine> = all[at..].to_vec();
    // Start at a message.
    if let Some(k) = v.iter().position(|d| d.first) {
        v.drain(..k);
    }
    // When the newest message alone is too tall, the newest whole lines stay: the last
    // words of the agent matter more than the top of its message.
    v
}

/// The name above a message: the agent by its provider name, You, the tool, Error.
fn speaker_name(d: &DLine, agent: &str) -> Option<String> {
    match d.kind {
        DKind::Agent | DKind::Heading | DKind::Bullet | DKind::Code | DKind::Table => {
            Some(agent.to_string())
        }
        _ => d.speaker().map(str::to_string),
    }
}

fn speaker_color(d: &DLine) -> u32 {
    match d.kind {
        DKind::You => OR,
        DKind::Error => RD,
        DKind::Tool => crate::warroom::bridge_data::tool_color(d.label.as_deref().unwrap_or("")),
        _ => PK,
    }
}

/// One display line: a speaker label on the first line of a message only, then the text
/// styled by its kind. `label_w` is the width of the label column; `pill` draws the label as a
/// colored block, else as colored text.
pub fn display_row(d: &DLine, agent: &str, label_w: f32, pill: bool, chars: usize) -> Div {
    let n = display_lines(d, chars);
    let c = speaker_color(d);
    let label = match (d.first, speaker_name(d, agent)) {
        (true, Some(name)) => Some(name),
        _ => None,
    };
    let mut cell = div().flex_none().w(px(label_w)).h(tsz(LH_BODY));
    if let Some(name) = label {
        cell = if pill {
            cell.child(
                div()
                    .h(tsz(LH_BODY))
                    .px(px(10.))
                    .rounded_l(px(12.))
                    .bg(rgb(c))
                    .flex()
                    .items_center()
                    .font_family(LABEL)
                    .text_size(tsz(S_SMALL))
                    .text_color(rgb(BLACK))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(name.to_uppercase()),
            )
        } else {
            cell.flex()
                .items_center()
                .font_family(LABEL)
                .text_size(tsz(S_SMALL))
                .text_color(rgb(c))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(name.to_uppercase())
        };
    }
    let text = div()
        .flex_1()
        .min_w(px(0.))
        .max_h(tsz(n as f32 * LH_BODY))
        .overflow_hidden()
        .line_height(tsz(LH_BODY));
    let text = match d.kind {
        DKind::You => text
            .font_family(BODY)
            .text_size(tsz(S_BODY))
            .text_color(rgb(WHITE)),
        DKind::Agent | DKind::Bullet => text
            .font_family(BODY)
            .text_size(tsz(S_BODY))
            .text_color(rgb(DIM)),
        DKind::Heading => text
            .truncate()
            .font_family(LABEL)
            .text_size(tsz(S_SECTION - 4.))
            .text_color(rgb(PK)),
        DKind::Code => text
            .truncate()
            .font_family(CODE)
            .text_size(tsz(S_CODE))
            .text_color(rgb(PA)),
        DKind::Table => text
            .truncate()
            .font_family(CODE)
            .text_size(tsz(S_CODE))
            .text_color(rgb(if d.first { PK } else { DIM })),
        DKind::Tool => text
            .truncate()
            .font_family(CODE)
            .text_size(tsz(S_CODE))
            .text_color(rgb(DIM)),
        DKind::Output => text
            .truncate()
            .pl(px(16.))
            .font_family(CODE)
            .text_size(tsz(S_CODE))
            .text_color(rgb(MUTED)),
        DKind::Error => text
            .truncate()
            .font_family(CODE)
            .text_size(tsz(S_CODE))
            .text_color(rgb(RD)),
        DKind::Meta => text
            .truncate()
            .font_family(CODE)
            .text_size(tsz(S_CODE))
            .text_color(rgb(MUTED)),
    };
    let shown = if d.kind == DKind::Heading {
        d.text.to_uppercase()
    } else {
        d.text.clone()
    };
    div()
        .flex_none()
        .flex()
        .items_start()
        .gap(px(16.))
        .when(d.first, |r| r.mt(px(MSG_GAP)))
        .child(cell)
        .child(text.child(shown))
}

/// The newest display lines of a transcript tail that fit a box of `avail` px (rule 16).
pub fn chat_display(tail: &[aivory_protocol::Line], avail: f32, chars: usize) -> Vec<DLine> {
    fit_display(crate::console::display::lines(tail), avail, chars)
}

/// A name cut at a word border to at most `max` characters, so a tab never ends mid-word.
/// A first word that is longer than `max` stays whole.
pub fn short_name(name: &str, max: usize) -> String {
    let mut out = String::new();
    for w in name.split_whitespace() {
        let next = if out.is_empty() {
            w.to_string()
        } else {
            format!("{out} {w}")
        };
        if !out.is_empty() && next.chars().count() > max {
            break;
        }
        out = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_of_several_answers_is_lilac() {
        assert_eq!(option_color(0, 3), OR);
        assert_eq!(option_color(2, 3), PK);
        assert_eq!(option_color(0, 1), OR);
        assert_eq!(option_text(1, "Deny"), "2 Deny");
    }

    #[test]
    fn a_short_name_ends_at_a_word_border() {
        assert_eq!(
            short_name("Virtualize the session list", 14),
            "Virtualize the"
        );
        assert_eq!(
            short_name("Supercalifragilistic", 8),
            "Supercalifragilistic"
        );
        assert_eq!(short_name("Redis failover", 20), "Redis failover");
    }

    #[test]
    fn the_switcher_lists_asks_first() {
        let war = aivory_protocol::demo::Demo::new().war_room;
        let list = order(&war);
        assert!(!list.is_empty());
        assert!(list.windows(2).all(|w| rank(w[0]) <= rank(w[1])));
    }

    #[test]
    fn a_hover_lights_a_block_toward_white() {
        assert_eq!(lit(0x000000), 0x4C4C4C);
        assert_eq!(lit(0xFFFFFF), 0xFFFFFF);
        let c = lit(OR);
        assert!(c >> 16 == 0xFF && (c >> 8 & 255) > 0x99);
    }

    #[test]
    fn the_art_moves_the_right_and_bottom_edge() {
        let a = Art::sized(1440., 900.);
        assert_eq!((a.rx(1124.), a.by(790.)), (1124., 790.));
        let a = Art::sized(1600., 1000.);
        assert_eq!((a.rx(1124.), a.by(790.)), (1284., 890.));
    }
}
