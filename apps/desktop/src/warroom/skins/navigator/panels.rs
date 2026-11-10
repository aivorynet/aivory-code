//! The workstation widgets of the Navigator: light grey beveled windows with a title bar,
//! raised push buttons, sunken fields and list boxes, in the desktop style of the early 90s.
//! The colors and sizes follow the mockups `NavigatorMix` and `NavigatorGroup`.

use gpui::prelude::*;
use std::rc::Rc;

use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, Div, ElementId, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, ScrollHandle, ScrollWheelEvent, SharedString, Stateful, Window,
    div, point, px, relative, rgb,
};

use crate::theme::*;
use crate::ui;

/// The face of a window.
pub(crate) const FACE: u32 = 0xBDBDBD;
/// The face of a push button, and of a button under the mouse.
pub(crate) const BTN: u32 = 0xC8C8C8;
pub(crate) const BTN_HOT: u32 = 0xD8D8D8;
/// The lit and the shaded bevel edges.
pub(crate) const LIT: u32 = 0xF2F2F2;
pub(crate) const SHADE: u32 = 0x6A6A6A;
/// The thin dark outline of a window.
pub(crate) const OUTLINE: u32 = 0x101010;
/// The title bar of a window, and of the dialog of an ask.
pub(crate) const TITLE: u32 = 0x5E7396;
pub(crate) const TITLE_ASK: u32 = 0x8E3A3A;
/// A sunken field.
pub(crate) const FIELD: u32 = 0xFFFFFF;
pub(crate) const TEXT: u32 = 0x111111;
pub(crate) const TEXT_2: u32 = 0x333333;
/// The dark screen of a live view, and its text.
pub(crate) const SCREEN: u32 = 0x050505;
pub(crate) const SCREEN_TEXT: u32 = 0xD6D6D6;
/// The LED colors.
pub(crate) const LED_RED: u32 = 0xFF3B30;
pub(crate) const LED_GREEN: u32 = 0x3CE05A;
pub(crate) const LED_AMBER: u32 = 0xF2B233;
pub(crate) const LED_OFF: u32 = 0x5A5A5A;
/// The state colors of the mode keys and the charts.
pub(crate) const KEY_PLAN: u32 = 0x3D5BD8;
pub(crate) const KEY_EDIT: u32 = 0xE6A019;
pub(crate) const KEY_AUTO: u32 = 0xE0352B;

/// A two-pixel bevel around `inner`: lit top and left, shaded bottom and right (raised), or
/// the other way round (sunken).
pub(crate) fn bevel(raised: bool, inner: impl IntoElement) -> Div {
    let (a, b) = if raised { (LIT, SHADE) } else { (SHADE, LIT) };
    div().bg(rgb(a)).pt(px(2.)).pl(px(2.)).child(
        div()
            .size_full()
            .bg(rgb(b))
            .pr(px(2.))
            .pb(px(2.))
            .child(inner),
    )
}

/// The width of the scroll bar.
const BAR_W: f32 = 16.;
/// The shortest thumb.
const THUMB_MIN: f32 = 24.;

/// A vertical scroll area with a scroll bar in the desktop style: a sunken trough and a raised
/// thumb at the right edge. The bar shows only when the content is taller than the area.
/// A press or a drag in the trough moves the thumb; the mouse wheel still scrolls.
/// With `stick`, the area stays at the bottom while it was at the bottom (a log).
pub(crate) fn scroll_area(
    id: impl Into<ElementId>,
    handle: &ScrollHandle,
    stick: bool,
    body: impl IntoElement,
) -> Div {
    let id: ElementId = id.into();
    let max = f32::from(handle.max_offset().y);
    let off = -f32::from(handle.offset().y);
    let view = f32::from(handle.bounds().size.height);
    let over = max > 1. && view > 0.;
    if stick && (!over || off >= max - 2.) {
        handle.scroll_to_bottom();
    }
    let mut area = div()
        .relative()
        .size_full()
        .overflow_hidden()
        .child(
            div()
                .id(id.clone())
                .size_full()
                .overflow_y_scroll()
                .track_scroll(handle)
                .child(div().when(over, |d| d.pr(px(BAR_W))).child(body)),
        )
        // The thumb follows the text: one more frame when the handle moved after this render.
        .child(crate::ui::scroll_watch(handle, max, off));
    if !over {
        return area;
    }
    let inner = (view - 4.).max(1.);
    let thumb = (inner * view / (view + max)).clamp(THUMB_MIN.min(inner), inner);
    let travel = (inner - thumb).max(1.);
    let top = (off / max).clamp(0., 1.) * travel;
    let set = {
        let h = handle.clone();
        move |y: f32, window: &mut Window| {
            let frac = ((y - f32::from(h.bounds().top()) - 2. - thumb / 2.) / travel).clamp(0., 1.);
            h.set_offset(point(h.offset().x, px(-frac * max)));
            window.refresh();
        }
    };
    let (down, drag) = (set.clone(), set);
    let wheel = handle.clone();
    let trough = div()
        .id(SharedString::from(format!("{id:?}-bar")))
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .w(px(BAR_W))
        .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, window, cx| {
            down(f32::from(ev.position.y), window);
            cx.stop_propagation();
        })
        .on_mouse_move(move |ev: &MouseMoveEvent, window, cx| {
            if ev.dragging() {
                drag(f32::from(ev.position.y), window);
                cx.stop_propagation();
            }
        })
        .on_scroll_wheel(move |ev: &ScrollWheelEvent, window, _| {
            let dy = f32::from(ev.delta.pixel_delta(px(40.)).y);
            let to = (f32::from(wheel.offset().y) + dy).clamp(-max, 0.);
            wheel.set_offset(point(wheel.offset().x, px(to)));
            window.refresh();
        })
        .child(
            bevel(
                false,
                div().size_full().relative().bg(rgb(0xA9A9A9)).child(
                    bevel(true, div().size_full().bg(rgb(BTN)))
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(top))
                        .h(px(thumb)),
                ),
            )
            .size_full(),
        );
    area = area
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(trough);
    area
}

/// A click handler that a title-bar box calls.
pub(crate) type Click = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
/// A press on a title bar: a drag starts.
pub(crate) type Press = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// What the title-bar boxes of a window do: the window menu (left box), minimize and close
/// (right boxes), and the drag of the title bar. A box without a handler does nothing.
#[derive(Clone, Default)]
pub(crate) struct Controls {
    /// A unique prefix for the element ids of the window.
    pub key: String,
    pub menu: Option<Click>,
    pub min: Option<Click>,
    pub close: Option<Click>,
    pub drag: Option<Press>,
}

/// A small raised square of a title bar with a glyph and a tooltip: 0 window menu (three short lines), 1 minimize (a low bar), 2 close (a cross). It lights up under the mouse.
fn corner_box(id: String, glyph: u8, tip: &'static str, on: Option<Click>) -> Stateful<Div> {
    let group = SharedString::from(format!("{id}-g"));
    let mark = match glyph {
        1 => div().w(px(7.)).h(px(2.)).bg(rgb(OUTLINE)),
        2 => div()
            .text_size(px(12.))
            .line_height(px(11.))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(OUTLINE))
            .child("×"),
        _ => div()
            .w(px(8.))
            .h(px(7.))
            .flex()
            .flex_col()
            .justify_between()
            .child(div().h(px(1.)).w_full().bg(rgb(OUTLINE)))
            .child(div().h(px(1.)).w_full().bg(rgb(OUTLINE)))
            .child(div().h(px(1.)).w_full().bg(rgb(OUTLINE))),
    };
    let inner = div()
        .size(px(11.))
        .flex()
        .justify_center()
        .when(glyph == 1, |d| d.items_end().pb(px(2.)))
        .when(glyph != 1, |d| d.items_center())
        .bg(rgb(FACE))
        .group_hover(group.clone(), |s| s.bg(rgb(BTN_HOT)))
        .child(mark);
    let mut b = div()
        .id(SharedString::from(id))
        .group(group)
        .flex_none()
        .tooltip(crate::ui::tip(tip))
        // A press on a box must not start the drag of the title bar.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(bevel(true, inner));
    if let Some(f) = on {
        b = b
            .cursor_pointer()
            .on_click(move |e, window, cx| f(e, window, cx));
    }
    b
}

/// A window: the outline, the hard shadow, the title bar (`bar` color) and the body. The
/// caller places it with `absolute()` and sizes it.
pub(crate) fn frame(title: impl Into<SharedString>, bar: u32, ctl: Controls, body: Div) -> Div {
    let k = ctl.key.clone();
    let mut head = div()
        .id(SharedString::from(format!("{k}-head")))
        .h(px(22.))
        .flex_none()
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .bg(rgb(bar))
        .border_b_1()
        .border_color(rgb(OUTLINE))
        .text_color(rgb(0xFFFFFF))
        .font_weight(FontWeight::BOLD)
        .text_size(ui::ts(T_SMALL))
        .child(corner_box(
            format!("{k}-menu"),
            0,
            "Window menu",
            ctl.menu.clone(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_center()
                .child(title.into()),
        )
        .child(corner_box(
            format!("{k}-min"),
            1,
            "Minimize",
            ctl.min.clone(),
        ))
        .child(corner_box(
            format!("{k}-close"),
            2,
            "Close",
            ctl.close.clone(),
        ));
    if let Some(drag) = ctl.drag.clone() {
        head = head.on_mouse_down(MouseButton::Left, move |e, window, cx| drag(e, window, cx));
    }
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(rgb(FACE))
        .border_1()
        .border_color(rgb(OUTLINE))
        .shadow(vec![BoxShadow {
            color: ui::hsla(0x00000059),
            offset: point(px(3.), px(3.)),
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: false,
        }])
        .font_family(tk().sans)
        .text_color(rgb(TEXT))
        .child(head)
        .child(body.flex_1().min_h_0())
}

/// A raised push button. The default button (`primary`) has a bold label and a dark ring,
/// like the default button of a dialog.
pub(crate) fn button(id: impl Into<SharedString>, label: &str, primary: bool) -> Stateful<Div> {
    let face = div()
        .h(px(20.))
        .px(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(BTN))
        .group_hover("nav-btn", |s| s.bg(rgb(BTN_HOT)))
        .text_size(ui::ts(T_SMALL))
        .font_weight(if primary {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(rgb(TEXT))
        .whitespace_nowrap()
        .child(label.to_string());
    let body = bevel(true, face);
    let ring = div()
        .flex_none()
        .when(primary, |d| d.p(px(2.)).m(px(-2.)).bg(rgb(TEXT)))
        .child(body);
    div()
        .id(id.into())
        .group("nav-btn")
        .flex_none()
        .cursor_pointer()
        .child(ring)
}

/// A sunken box with a light field inside.
pub(crate) fn sunken(inner: Div) -> Div {
    bevel(false, inner.bg(rgb(FIELD)))
}

/// A sunken box with a dark screen inside.
pub(crate) fn screen(inner: Div) -> Div {
    bevel(false, inner.bg(rgb(0x000000)))
}

/// A round LED with a dark edge.
pub(crate) fn led(color: u32) -> Div {
    div()
        .size(px(11.))
        .flex_none()
        .rounded_full()
        .bg(rgb(color))
        .border_1()
        .border_color(rgb(0x333333))
}

// ---- the type scale and the widgets of the Navigator screens ---------------------------------

/// The one type scale of the Navigator screens: screen title > section title > entry > label.
pub(crate) const S_TITLE: f32 = 28.;
pub(crate) const S_SECTION: f32 = 18.;
pub(crate) const S_ENTRY: f32 = 14.;
pub(crate) const S_LABEL: f32 = 12.;
/// The height of a one-line list row, and of a two-line row.
pub(crate) const ROW_H: f32 = 36.;
pub(crate) const ROW_H2: f32 = 48.;
/// The inner padding of a panel, and the gap between panels.
pub(crate) const PAD: f32 = 24.;
pub(crate) const GAP_PANEL: f32 = 32.;
/// Quiet text, the selected row, and the color of a thing that waits for the owner.
pub(crate) const MUTED: u32 = 0x4F4F4F;
pub(crate) const SEL: u32 = 0x2F4A7A;
pub(crate) const ALERT: u32 = 0xB3141C;

/// A push button of a screen: 28 px high, entry text. `primary` has a bold label and a ring.
pub(crate) fn push(id: impl Into<SharedString>, label: &str, primary: bool) -> Stateful<Div> {
    let id: SharedString = id.into();
    let group = SharedString::from(format!("{id}-g"));
    let face = div()
        .min_h(crate::ui::sw(28.))
        .px(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(BTN))
        .group_hover(group.clone(), |s| s.bg(rgb(BTN_HOT)))
        .text_size(ui::ts(S_ENTRY))
        .font_weight(if primary {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(rgb(TEXT))
        .whitespace_nowrap()
        .child(label.to_string());
    let ring = div()
        .flex_none()
        .when(primary, |d| d.p(px(2.)).m(px(-2.)).bg(rgb(TEXT)))
        .child(bevel(true, face));
    div()
        .id(id)
        .group(group)
        .flex_none()
        .cursor_pointer()
        .child(ring)
}

/// A segmented key: raised when off, sunken and darker when on.
pub(crate) fn seg(id: impl Into<SharedString>, label: &str, on: bool) -> Stateful<Div> {
    let id: SharedString = id.into();
    let group = SharedString::from(format!("{id}-g"));
    let face = div()
        .min_h(crate::ui::sw(26.))
        .px(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(if on { 0xA6A6A6 } else { BTN }))
        .when(!on, |d| {
            d.group_hover(group.clone(), |s| s.bg(rgb(BTN_HOT)))
        })
        .text_size(ui::ts(S_ENTRY))
        .font_weight(if on {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(rgb(TEXT))
        .whitespace_nowrap()
        .child(label.to_string());
    div()
        .id(id)
        .group(group)
        .flex_none()
        .cursor_pointer()
        .child(bevel(!on, face))
}

/// One row of a list box. The selected row is dark blue; a row that waits for the owner has a
/// red note. `dot` is the LED of the state.
pub(crate) fn list_row(
    id: impl Into<SharedString>,
    dot: Option<u32>,
    title: &str,
    sub: &str,
    note: &str,
    on: bool,
    urgent: bool,
) -> Stateful<Div> {
    let two = !sub.is_empty();
    let (ink, quiet) = if on {
        (0xFFFFFF, 0xD6DDEB)
    } else {
        (TEXT, MUTED)
    };
    let mut body = div().flex_1().min_w_0().flex().flex_col().child(
        div()
            .truncate()
            .text_size(ui::ts(S_ENTRY))
            .line_height(relative(1.45))
            .font_weight(if on || urgent {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            })
            .text_color(rgb(ink))
            .child(title.to_string()),
    );
    if two {
        body = body.child(
            div()
                .truncate()
                .text_size(ui::ts(S_LABEL))
                .line_height(relative(1.45))
                .text_color(rgb(quiet))
                .child(sub.to_string()),
        );
    }
    div()
        .id(id.into())
        .h(px(if two { ROW_H2 } else { ROW_H }))
        .flex_none()
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(10.))
        .cursor_pointer()
        .border_b_1()
        .border_color(rgb(0xD0D0D0))
        .bg(rgb(if on { SEL } else { FIELD }))
        .when(!on, |d| d.hover(|s| s.bg(rgb(0xDDE4F0))))
        .children(dot.map(led))
        .child(body)
        .when(!note.is_empty(), |d| {
            d.child(
                div()
                    .flex_none()
                    .text_size(ui::ts(S_LABEL))
                    .font_weight(if urgent {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(rgb(if urgent && !on { ALERT } else { quiet }))
                    .whitespace_nowrap()
                    .child(note.to_string()),
            )
        })
}

/// A sunken white list box around rows.
pub(crate) fn list_box(rows: Vec<AnyElement>) -> Div {
    sunken(div().size_full().flex().flex_col().children(rows)).size_full()
}

/// The screen title with a quiet line of counts under it.
pub(crate) fn heading(title: &str, sub: &str) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .child(
            div()
                .truncate()
                .text_size(ui::ts(S_TITLE))
                .line_height(relative(1.3))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(TEXT))
                .child(title.to_string()),
        )
        .child(
            div()
                .truncate()
                .text_size(ui::ts(S_LABEL))
                .line_height(relative(1.6))
                .text_color(rgb(MUTED))
                .child(sub.to_string()),
        )
}

/// A section title with an etched line under it.
pub(crate) fn section(text: &str) -> Div {
    div()
        .flex_none()
        .pb(px(6.))
        .border_b_1()
        .border_color(rgb(SHADE))
        .text_size(ui::ts(S_SECTION))
        .line_height(relative(1.4))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(TEXT))
        .child(text.to_string())
}

/// A quiet line of entry size.
pub(crate) fn note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(ui::ts(S_ENTRY))
        .line_height(relative(1.45))
        .text_color(rgb(MUTED))
        .child(text.into())
}

/// A small label over a value.
pub(crate) fn kv(label: &str, value: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(ui::ts(S_LABEL))
                .line_height(relative(1.45))
                .text_color(rgb(MUTED))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(ui::ts(S_ENTRY))
                .line_height(relative(1.45))
                .text_color(rgb(color))
                .child(value.into()),
        )
}

/// The key row under the title: the keys of the screen in one line.
pub(crate) fn menu_bar() -> Div {
    div().flex_none().flex().items_center().gap(px(10.))
}

/// `1.2k`, `830`, `0.42`.
fn short_value(v: f32) -> String {
    if v >= 10_000. {
        format!("{:.0}k", v / 1000.)
    } else if v >= 1000. {
        format!("{:.1}k", v / 1000.)
    } else if v >= 10. {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// An osview strip chart on a dark screen: a grid, one bar per real sample with the newest at
/// the right, the largest value at the top left and the newest value at the top right. The
/// bars scale to the largest sample (at least `floor`). With no sample, or only zeros, the
/// screen shows a calm note instead of a flat block.
pub(crate) fn strip_chart(values: &[f32], color: u32, h: f32, floor: f32, caption: &str) -> Div {
    let peak = values.iter().copied().fold(floor.max(1e-6), f32::max);
    let live = values.iter().any(|v| *v > 0.);
    let line = |top: f32| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(top))
            .h(px(1.))
            .bg(rgb(0x1B3A22))
    };
    let mut screen_box = div().relative().size_full().overflow_hidden();
    for k in 1..4 {
        screen_box = screen_box.child(line(h * k as f32 / 4.));
    }
    if live {
        let mut bars = div()
            .absolute()
            .left(px(4.))
            .right(px(4.))
            .top(px(16.))
            .bottom(px(2.))
            .flex()
            .items_end()
            .gap(px(1.));
        let room = h - 20.;
        let few = values.len() <= 16;
        if few {
            bars = bars.gap(px(6.));
        }
        for v in values {
            let bh = (v / peak * room).max(1.);
            let bar = div().h(px(bh)).bg(rgb(color));
            bars = bars.child(if few {
                bar.w(px(26.)).flex_none()
            } else {
                bar.flex_1()
            });
        }
        screen_box = screen_box.child(bars).child(
            div()
                .absolute()
                .top(px(1.))
                .left(px(6.))
                .text_size(ui::ts(S_LABEL))
                .text_color(rgb(0x8FCF8F))
                .child(format!("{caption} · max {}", short_value(peak))),
        );
        screen_box = screen_box.child(
            div()
                .absolute()
                .top(px(1.))
                .right(px(6.))
                .text_size(ui::ts(S_LABEL))
                .text_color(rgb(color))
                .child(short_value(values.last().copied().unwrap_or(0.))),
        );
    } else {
        screen_box = screen_box.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(ui::ts(S_LABEL))
                .text_color(rgb(0x6FA877))
                .child(if values.is_empty() {
                    format!("{caption} · no samples yet")
                } else {
                    format!("{caption} · no change yet")
                }),
        );
    }
    screen(screen_box).h(px(h)).w_full()
}

/// The growth of a total, sampled: a rolling series for a strip chart. A total that falls
/// (a session ended) counts as no growth.
#[derive(Default)]
pub(crate) struct Series {
    last: Option<f64>,
    values: std::collections::VecDeque<f32>,
}

impl Series {
    /// Keep this many samples.
    const KEEP: usize = 90;

    /// Record the growth since the last call.
    pub fn sample(&mut self, total: f64) {
        let grew = self.last.map_or(0., |l| (total - l).max(0.));
        self.last = Some(total);
        self.values.push_back(grew as f32);
        while self.values.len() > Self::KEEP {
            self.values.pop_front();
        }
    }

    pub fn list(&self) -> Vec<f32> {
        self.values.iter().copied().collect()
    }
}

/// A screen of the Navigator in the grey window: padding, face color, full size.
pub(crate) fn page() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(GAP_PANEL))
        .p(px(PAD))
        .bg(rgb(FACE))
        .font_family(tk().sans)
        .text_color(rgb(TEXT))
}
