//! Small widgets of the accepted design that more than one screen uses.

use aivory_protocol::SessionState;
use gpui::prelude::*;
use gpui::{
    AnyElement, AnyView, App, BoxShadow, ClickEvent, Context, Div, ElementId, FontWeight, Hsla,
    Pixels, Rgba, SharedString, Stateful, Svg, Window, div, point, px, rgb, rgba, svg,
};

use crate::theme::*;

pub type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The interface text scale in percent (Settings → Display → Interface text size).
static UI_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(115);

pub fn set_ui_scale(percent: u16) {
    UI_SCALE.store(u32::from(percent), std::sync::atomic::Ordering::Relaxed);
}

/// The interface text scale in percent.
pub fn ui_scale() -> u16 {
    UI_SCALE.load(std::sync::atomic::Ordering::Relaxed) as u16
}

/// The window fit in per mille: the text grows on a large window and shrinks a little on a
/// small one. The shell sets it from the window size at each frame ([`set_fit`]).
static FIT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1000);

/// The window size that the designs use at fit 1.
const FIT_BASE: (f32, f32) = (1600., 1000.);
/// The smallest and the largest fit.
const FIT_RANGE: (f32, f32) = (0.85, 1.4);

/// The fit for a window of `w` x `h` logical px.
pub fn fit_for(w: f32, h: f32) -> f32 {
    (w / FIT_BASE.0)
        .min(h / FIT_BASE.1)
        .clamp(FIT_RANGE.0, FIT_RANGE.1)
}

/// A window under this height (logical px) is short: the screens hide their big title rows.
pub const SHORT_H: f32 = 820.;

static SHORT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `true` when the window is short ([`SHORT_H`]): a screen drops its big title row so the chat
/// and the composer keep their room.
pub fn short() -> bool {
    SHORT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set the fit from the window size. `true`: the fit changed (the frame must draw again).
pub fn set_fit(w: f32, h: f32) -> bool {
    SHORT.store(h < SHORT_H, std::sync::atomic::Ordering::Relaxed);
    let next = (fit_for(w, h) * 1000.).round() as u32;
    FIT.swap(next, std::sync::atomic::Ordering::Relaxed) != next
}

/// The text scale (the setting and the window fit) as a factor.
fn factor() -> f32 {
    UI_SCALE.load(std::sync::atomic::Ordering::Relaxed) as f32 / 100.
        * FIT.load(std::sync::atomic::Ordering::Relaxed) as f32
        / 1000.
}

/// A width that holds interface text (a table column): `w` px at 100 %, scaled like the text.
pub fn sw(w: f32) -> Pixels {
    px(w * factor())
}

/// A text size of the interface: `size` px at 100 %, scaled by the interface text size.
pub fn ts(size: f32) -> Pixels {
    px(size * factor())
}

pub fn hsla(rgba_hex: u32) -> Hsla {
    rgba(rgba_hex).into()
}

pub fn icon(path: &'static str, size: f32, color: Rgba) -> Svg {
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// The view of a tooltip: a small dark label.
struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(rgb(tk().dark))
            .text_color(rgb(tk().card))
            .text_size(ts(crate::theme::T_SMALL))
            .font_family(tk().sans)
            .whitespace_nowrap()
            .child(self.0.clone())
    }
}

/// The builder for `.tooltip(...)` of an element with an id. Every button that shows only an
/// icon gets one, so the look says what the click does.
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}

/// An 8 px filled dot.
pub fn dot(color: Rgba) -> Div {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

/// The state dot: idle is a hollow ring.
pub fn state_dot(state: SessionState, size: f32) -> Div {
    let d = div().flex_none().size(px(size)).rounded_full();
    match state {
        SessionState::Idle => d.border(px(1.5)).border_color(rgb(tk().idle_ring)),
        _ => d.bg(rgb(state_color(state))),
    }
}

/// The dot and node color of a state.
pub fn state_color(state: SessionState) -> u32 {
    match state {
        SessionState::NeedsYou => tk().q,
        SessionState::Error => tk().e,
        SessionState::Working => tk().ink,
        SessionState::Idle => tk().quiet,
        SessionState::Parked | SessionState::Done => tk().quiet,
    }
}

/// The 26x16 switch.
pub fn switch(on: bool) -> Div {
    div()
        .flex_none()
        .relative()
        .w(px(26.))
        .h(px(16.))
        .rounded_full()
        .bg(rgb(if on { tk().switch_on } else { tk().switch_off }))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(if on { 12. } else { 2. }))
                .size(px(12.))
                .rounded_full()
                .bg(rgb(tk().card)),
        )
}

/// A clickable switch with an id.
pub fn switch_button(id: impl Into<ElementId>, on: bool, on_click: OnClick) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .child(switch(on))
        .on_click(on_click)
}

/// `− value +` with the design's 28 px buttons.
pub fn stepper(id: &str, value: impl Into<SharedString>, dec: OnClick, inc: OnClick) -> Div {
    let button = |suffix: &str, label: &'static str, f: OnClick| {
        div()
            .id(SharedString::from(format!("{id}-{suffix}")))
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(ts(crate::theme::T_BODY))
            .text_color(rgb(tk().ink_2))
            .cursor_pointer()
            .hover(|s| s.text_color(rgb(tk().ink)).bg(rgb(tk().track)))
            .on_click(f)
            .child(label)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .rounded(tk().r_button)
        .border_1()
        .border_color(rgba(tk().line_2))
        .text_size(ts(crate::theme::T_SMALL))
        .child(button("dec", "−", dec))
        .child(
            div()
                .min_w(px(56.))
                .flex()
                .justify_center()
                .child(value.into()),
        )
        .child(button("inc", "+", inc))
}

/// The card shadow of popovers and menus.
pub fn popover_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: hsla(0x0D0D0E0F),
            offset: point(px(0.), px(2.)),
            blur_radius: px(4.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: hsla(0x0D0D0E1A),
            offset: point(px(0.), px(16.)),
            blur_radius: px(40.),
            spread_radius: px(0.),
            inset: false,
        },
    ]
}

/// The Navigator look draws buttons and fields as bevelled workstation widgets.
pub fn irix() -> bool {
    tk().look == aivory_protocol::Look::Navigator
}

/// The Navigator bevel as four edge bars drawn over the widget: lit top and left and shaded
/// bottom and right when raised, the other way round when sunken. Works on any element.
pub fn bevel_edges<E: ParentElement + Styled>(raised: bool, e: E) -> E {
    let (a, b) = if raised {
        (0xF2F2F2, 0x6A6A6A)
    } else {
        (0x6A6A6A, 0xF2F2F2)
    };
    let bar = |c: u32| div().absolute().bg(rgb(c));
    e.child(bar(a).top_0().left_0().right_0().h(px(2.)))
        .child(bar(a).top_0().left_0().bottom_0().w(px(2.)))
        .child(bar(b).bottom_0().left_0().right_0().h(px(2.)))
        .child(bar(b).top_0().right_0().bottom_0().w(px(2.)))
}

fn shadow(color: u32, y: f32, blur: f32, spread: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(color),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

/// The one soft shadow of a white card on the white page (`--card-sh`): no border.
pub fn card_shadow() -> Vec<BoxShadow> {
    vec![
        shadow(0x0000000A, 0., 0., 1.),
        shadow(0x0000000D, 2., 6., 0.),
        shadow(0x0000000D, 10., 28., 0.),
    ]
}

/// The card shadow under the pointer.
pub fn card_shadow_hover() -> Vec<BoxShadow> {
    vec![
        shadow(0x0000000F, 0., 0., 1.),
        shadow(0x0000000F, 4., 10., 0.),
        shadow(0x00000014, 16., 36., 0.),
    ]
}

/// A 2 px ring with no glow (an open tile, a focused field).
pub fn ring(rgb_color: u32, width: f32) -> Vec<BoxShadow> {
    vec![shadow(rgb_color << 8 | 0xFF, 0., 0., width)]
}

/// The soft shadow of a white side column against the white page (`.side-light`).
pub fn side_shadow(x: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0x0000002E),
        offset: point(px(x), px(0.)),
        blur_radius: px(24.),
        spread_radius: px(-16.),
        inset: false,
    }]
}

/// A pill button (`.btn`): graphite when primary, soft grey otherwise.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    pill_button(id, label, primary, false)
}

/// A pill button with a switch or an icon before the label.
pub fn button_lead(
    id: impl Into<ElementId>,
    lead: impl IntoElement,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    pill_with(id, Some(lead.into_any_element()), label, false, false).pl(px(14.))
}

/// The small pill button (`.btn.sm`).
pub fn button_sm(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    pill_button(id, label, primary, true)
}

fn pill_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    small: bool,
) -> Stateful<Div> {
    pill_with(id, None, label, primary, small)
}

fn pill_with(
    id: impl Into<ElementId>,
    lead: Option<AnyElement>,
    label: impl Into<SharedString>,
    primary: bool,
    small: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(if small { 32. } else { 36. }))
        .px(px(if small { 14. } else { 18. }))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .rounded_full()
        .text_size(ts(if small {
            crate::theme::T_SMALL
        } else {
            crate::theme::T_BODY
        }))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .when(primary && !irix(), |d| {
            d.bg(rgb(tk().dark))
                .text_color(rgb(tk().card))
                .hover(|s| s.bg(rgb(tk().dark_hover)))
        })
        .when(!primary && !irix(), |d| {
            d.bg(rgb(tk().track))
                .text_color(rgb(tk().ink))
                .hover(|s| s.bg(rgb(tk().track_2)))
        })
        .when(irix(), |d| {
            d.rounded(px(0.))
                .bg(rgb(0xC8C8C8))
                .text_color(rgb(0x111111))
                .hover(|s| s.bg(rgb(0xD8D8D8)))
                .when(primary, |d| {
                    d.border_2()
                        .border_color(rgb(0x111111))
                        .font_weight(FontWeight::BOLD)
                })
                .when(!primary, |d| d.font_weight(FontWeight::NORMAL))
        })
        .when(irix(), |d| bevel_edges(true, d))
        .children(lead)
        .child(label.into())
}

/// The frame of a popover (`.pop`): 320 px by default.
pub fn popover(width: f32) -> Div {
    div()
        .w(px(width))
        .p(px(16.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .bg(rgb(tk().card))
        .rounded(tk().r_card)
        .border_1()
        .border_color(rgba(tk().line))
        .shadow(popover_shadow())
        .text_size(TEXT)
        .text_color(rgb(tk().ink))
}

/// A row of a popover: label left, control right.
pub fn popover_row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(div().text_color(rgb(tk().ink_2)).child(label.into()))
        .child(control)
}

/// The frame of a dropdown menu (`.menu`).
pub fn menu(min_width: f32) -> Div {
    div()
        .min_w(px(min_width))
        .p(px(4.))
        .flex()
        .flex_col()
        .bg(rgb(tk().card))
        .rounded(tk().r_button)
        .border_1()
        .border_color(rgba(crate::theme::pick(0x0D0D0E1F, tk().line_2)))
        .shadow(popover_shadow())
        .text_color(rgb(tk().ink))
}

/// A menu item (`.mi`); `on` shows the check mark.
pub fn menu_item(
    id: impl Into<ElementId>,
    on: bool,
    left: AnyElement,
    right: Option<SharedString>,
    on_click: OnClick,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .rounded(px(5.))
        .text_size(ts(crate::theme::T_BODY))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(tk().menu_hover)))
        .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
        .child(left)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when_some(right, |d, r| {
                    d.child(
                        div()
                            .text_size(ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .font_weight(FontWeight::NORMAL)
                            .child(r),
                    )
                })
                .when(on, |d| {
                    d.child(div().text_color(rgb(tk().primary)).child("✓"))
                }),
        )
        .on_click(on_click)
}

/// The graphite primary header button (`.btn.pri`).
pub fn dark_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button(id, label, true)
}

/// A segmented control button (`.sb`).
pub fn seg_button(
    id: impl Into<ElementId>,
    on: bool,
    label: impl Into<SharedString>,
    mono: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .min_w(px(30.))
        .h(px(30.))
        .px(px(if mono { 9. } else { 13. }))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .whitespace_nowrap()
        .text_size(ts(crate::theme::T_SMALL))
        .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
        .cursor_pointer()
        .when(on, |d| {
            d.bg(rgb(tk().card))
                .shadow(vec![shadow(0x0000001F, 2., 8., 0.)])
        })
        .when(!on, |d| d.hover(|s| s.text_color(rgb(tk().ink))))
        .when(irix(), |d| {
            d.rounded(px(0.)).text_color(rgb(0x111111)).bg(rgb(if on {
                0xA8A8A8
            } else {
                0xC8C8C8
            }))
        })
        .when(irix(), |d| bevel_edges(!on, d))
        .child(label.into())
}

/// The track of a segmented control (`.seg`).
pub fn seg_track() -> Div {
    div()
        .flex()
        .flex_none()
        .gap(px(2.))
        .p(px(3.))
        .bg(rgb(tk().track))
        .rounded_full()
}

/// Ask for the next animation frame, but at most about 30 times per second.
/// A full-rate redraw (the display rate) doubles the paint work and the CPU use
/// of the animated looks, and the eye does not need it.
/// One view has at most one waiting timer: a render for another reason (new data, a hover)
/// does not start a second redraw chain.
pub fn animate<T: 'static>(_window: &mut Window, cx: &mut Context<T>) {
    thread_local! {
        static WAITING: std::cell::RefCell<std::collections::HashSet<gpui::EntityId>> =
            std::cell::RefCell::new(std::collections::HashSet::new());
    }
    let id = cx.entity_id();
    if !WAITING.with(|w| w.borrow_mut().insert(id)) {
        return;
    }
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(33))
            .await;
        WAITING.with(|w| w.borrow_mut().remove(&id));
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

#[cfg(test)]
mod fit_tests {
    use super::fit_for;

    #[test]
    fn the_fit_follows_the_window_within_its_range() {
        assert_eq!(fit_for(1600., 1000.), 1.);
        assert_eq!(fit_for(3200., 2000.), 1.4);
        assert_eq!(fit_for(800., 600.), 0.85);
        // The smaller side decides.
        assert_eq!(fit_for(2400., 1000.), 1.);
    }
}

/// The scroll thumb of a scroll area, at its right edge. Put it as a sibling after the scroll
/// element, inside a relative parent of the same size. It reads the scroll handle while it
/// paints, after the layout of this frame, so it always follows the text (also while the text
/// grows and the area keeps to the bottom). It hides when all of the text fits.
pub fn thumb(handle: &gpui::ScrollHandle, color: Rgba, width: f32) -> impl IntoElement {
    let handle = handle.clone();
    gpui::canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            if let Some((y, h)) = thumb_span(
                f32::from(handle.max_offset().y),
                -f32::from(handle.offset().y),
                f32::from(handle.bounds().size.height),
                f32::from(b.size.height),
            ) {
                let x = f32::from(b.origin.x) + f32::from(b.size.width) - width - 2.;
                let rect = gpui::Bounds::new(
                    gpui::point(px(x), b.origin.y + px(y)),
                    gpui::size(px(width), px(h)),
                );
                window.paint_quad(gpui::fill(rect, color).corner_radii(px(width / 2.)));
            }
        },
    )
    .absolute()
    .top_0()
    .bottom_0()
    .right_0()
    .w(px(width + 4.))
}

/// The top and the height of a thumb in a track of `track` px, for a view of `view` px that
/// is `off` px down a text with `max` px more below the first view. `None` when all fits.
pub fn thumb_span(max: f32, off: f32, view: f32, track: f32) -> Option<(f32, f32)> {
    if max <= 1. || view <= 0. || track <= 0. {
        return None;
    }
    let h = (track * view / (view + max)).clamp(18_f32.min(track), track);
    let at = (off / max).clamp(0., 1.);
    Some(((track - h) * at, h))
}

#[cfg(test)]
mod thumb_tests {
    #[test]
    fn the_thumb_follows_the_offset_and_hides_when_all_fits() {
        assert_eq!(super::thumb_span(0., 0., 300., 300.), None);
        let (top, h) = super::thumb_span(300., 0., 300., 300.).unwrap();
        assert_eq!((top, h), (0., 150.));
        let (top, _) = super::thumb_span(300., 300., 300., 300.).unwrap();
        assert_eq!(top, 150.);
        let (top, _) = super::thumb_span(300., 150., 300., 300.).unwrap();
        assert_eq!(top, 75.);
    }
}

/// For a scroll bar that a view builds from the scroll handle while it renders: the handle
/// changes after the layout (new text, a jump to the bottom), and then no new frame comes. This
/// empty element checks the handle while it paints and asks for one more frame when it moved
/// away from the values (`max`, `off`) that the bar used, so the bar follows the text.
pub fn scroll_watch(handle: &gpui::ScrollHandle, max: f32, off: f32) -> impl IntoElement {
    let handle = handle.clone();
    gpui::canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let now_max = f32::from(handle.max_offset().y);
            let now_off = -f32::from(handle.offset().y);
            if (now_max - max).abs() > 0.5 || (now_off - off).abs() > 0.5 {
                window.refresh();
            }
        },
    )
    .absolute()
    .size_0()
}

/// A scroll element (it grows with `flex_1`) with its [`thumb`] on top: the wrapper takes the
/// place of the scroll element in its column.
pub fn with_thumb(
    scroll: impl IntoElement,
    handle: &gpui::ScrollHandle,
    color: Rgba,
    width: f32,
) -> Div {
    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex()
        .flex_col()
        .child(scroll)
        .child(thumb(handle, color, width))
}
