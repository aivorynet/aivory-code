//! The overlay style: one table for every dialog, menu, picker, confirm box and toast.
//!
//! `style()` returns the [`OverlayStyle`] of the active look: scrim, panel, frame, edge, radii,
//! the colors of text, fields, chips and the primary button, the type sizes and `caps`.
//! `fonts()` returns (body, mono, heading). `color(v)` turns a 0xRRGGBB or 0xRRGGBBAA value
//! into a color. A look fills its arm of `style()` (one `OverlayStyle` literal) and its arm of
//! `fonts()`; every overlay reads those two functions, never the Minimal tokens, and the shared
//! helpers `menu_frame`, `menu_row`, `toast_card`, `button`, `chip` and `field_box` apply them.
//! Minimal, Navigator and Bridge are tuned in full; the other looks use their kit colors.

use aivory_protocol::Look;
use gpui::prelude::*;
use gpui::{Div, FontWeight, Rgba, Stateful, Styled, div, px, rgb, rgba};

use crate::theme::tk;

/// The colors and sizes of one look (the boards in `docs/design/looks/newsession/`).
pub struct OverlayStyle {
    pub scrim: u32,
    pub panel: u32,
    pub edge: Option<u32>,
    pub radius: f32,
    /// Bridge: the orange frame on the left and on top.
    pub frame: Option<u32>,
    /// Navigator: the blue title bar.
    pub title_bar: Option<u32>,
    pub label: u32,
    pub text: u32,
    pub bright: u32,
    pub muted: u32,
    pub field: u32,
    pub field_edge: Option<u32>,
    pub field_radius: f32,
    pub sel_bg: u32,
    pub sel_edge: u32,
    pub sel_text: u32,
    pub chip_on_bg: u32,
    pub chip_on_text: u32,
    pub chip_off_bg: u32,
    pub chip_off_edge: Option<u32>,
    pub chip_radius: f32,
    pub free: u32,
    pub primary: u32,
    pub primary_text: u32,
    pub s_title: f32,
    pub s_entry: f32,
    pub s_label: f32,
    pub caps: bool,
}

pub fn style() -> OverlayStyle {
    let t = tk();
    match t.look {
        Look::Bridge => OverlayStyle {
            scrim: 0x000000FF,
            panel: 0x000000,
            edge: None,
            radius: 0.,
            frame: Some(0xFF9C00),
            title_bar: None,
            label: 0xFF9C00,
            text: 0xE9DCCB,
            bright: 0xF3E3CF,
            muted: 0xA88AA8,
            field: 0x120D14,
            field_edge: Some(0x6B5A70),
            field_radius: 0.,
            sel_bg: 0x160F12,
            sel_edge: 0xFF9C00,
            sel_text: 0xF3E3CF,
            chip_on_bg: 0xFFCC99,
            chip_on_text: 0x000000,
            chip_off_bg: 0x2A2130,
            chip_off_edge: None,
            chip_radius: 20.,
            free: 0xA88AA8,
            primary: 0xFF9C00,
            primary_text: 0x000000,
            s_title: 28.,
            s_entry: 18.,
            s_label: 14.,
            caps: true,
        },
        // Operator: an overlay is a blue instrument screen (steel bezel, glass), the primary
        // action is agent green. Sizes: the four of the look (30, 15, 13).
        Look::Operator => OverlayStyle {
            scrim: 0x010302F2,
            panel: 0x07120D,
            edge: Some(0x2F6B4A),
            radius: 0.,
            frame: None,
            title_bar: Some(0x0B1A12),
            label: 0x6DB596,
            text: 0x8FD3AE,
            bright: 0xCFFFE0,
            muted: 0x6DB596,
            field: 0x010A04,
            field_edge: Some(0x1D3A2A),
            field_radius: 0.,
            sel_bg: 0x0F2A1C,
            sel_edge: 0x9BFFC0,
            sel_text: 0xCFFFE0,
            chip_on_bg: 0x9BFFC0,
            chip_on_text: 0x04140B,
            chip_off_bg: 0x07120D,
            chip_off_edge: Some(0x1D3A2A),
            chip_radius: 0.,
            free: 0x6DB596,
            primary: 0x9BFFC0,
            primary_text: 0x04140B,
            s_title: 28.,
            s_entry: 14.,
            s_label: 12.,
            caps: false,
        },
        // The Neon: the colors of the Neon kit (`shell/neon_kit.rs`): ground 05060C, panel
        // 070A18, yellow F3E600, cyan 00E0F0, pale E9EEFF. Titles in Orbitron, a glow on the frame.
        Look::Neon => OverlayStyle {
            scrim: 0x05060CF0,
            panel: 0x070A18,
            edge: Some(0x00E0F0B0),
            radius: 0.,
            frame: None,
            title_bar: None,
            label: 0x8C96BC,
            text: 0xC9D2F0,
            bright: 0xE9EEFF,
            muted: 0x8C96BC,
            field: 0x05060C,
            field_edge: Some(0x00E0F055),
            field_radius: 0.,
            sel_bg: 0x141400,
            sel_edge: 0xF3E600,
            sel_text: 0xFFFFFF,
            chip_on_bg: 0x00E0F0,
            chip_on_text: 0x05060C,
            chip_off_bg: 0x070A18,
            chip_off_edge: Some(0x8C96BC66),
            chip_radius: 0.,
            free: 0x00E0F0,
            primary: 0xF3E600,
            primary_text: 0x05060C,
            s_title: 30.,
            s_entry: 14.,
            s_label: 12.,
            caps: true,
        },
        Look::Navigator => OverlayStyle {
            scrim: 0x141E2DE6,
            panel: 0xC6C6C6,
            edge: Some(0x6B6B6B),
            radius: 0.,
            frame: None,
            title_bar: Some(0x5A7299),
            label: 0x111111,
            text: 0x111111,
            bright: 0x111111,
            muted: 0x444444,
            field: 0xFFFFFF,
            field_edge: Some(0x6B6B6B),
            field_radius: 0.,
            sel_bg: 0x3D5A8A,
            sel_edge: 0x3D5A8A,
            sel_text: 0xFFFFFF,
            chip_on_bg: 0xB4B4B4,
            chip_on_text: 0x111111,
            chip_off_bg: 0xC6C6C6,
            chip_off_edge: Some(0x6B6B6B),
            chip_radius: 0.,
            free: 0x444444,
            primary: 0xC6C6C6,
            primary_text: 0x111111,
            s_title: 22.,
            s_entry: 14.,
            s_label: 12.,
            caps: false,
        },
        Look::Holo => OverlayStyle {
            scrim: 0x02060EF0,
            panel: 0x0A182E,
            edge: Some(0x7FD8FF66),
            radius: 0.,
            frame: None,
            title_bar: None,
            label: 0x93AECB,
            text: 0xDCEAF8,
            bright: 0xFFFFFF,
            muted: 0x8AA6C4,
            field: 0x061024,
            field_edge: Some(0x34506E),
            field_radius: 0.,
            sel_bg: 0x10243C,
            sel_edge: 0x7FD8FF,
            sel_text: 0xFFFFFF,
            chip_on_bg: 0x10283F,
            chip_on_text: 0xE6F2FF,
            chip_off_bg: 0x0A182E,
            chip_off_edge: Some(0x34506E),
            chip_radius: 0.,
            free: 0x7FD8FF,
            primary: 0x7FD8FF,
            primary_text: 0x04101E,
            s_title: 30.,
            s_entry: 15.,
            s_label: 12.,
            caps: true,
        },
        // Tactical (military terminal): olive slate panel on a dark scrim, bone text, a thin
        // olive edge. Cyan marks the selection and the primary button; green only says "free".
        Look::Tactical => OverlayStyle {
            scrim: 0x080A07F0,
            panel: 0x171C14,
            edge: Some(0x3F4A37),
            radius: 0.,
            frame: None,
            title_bar: None,
            label: 0x9AA586,
            text: 0xCDD3BC,
            bright: 0xE9EBDA,
            muted: 0x9AA586,
            field: 0x0B0E09,
            field_edge: Some(0x3F4A37),
            field_radius: 0.,
            sel_bg: 0x1B2A2B,
            sel_edge: 0x7FBAC0,
            sel_text: 0xE9EBDA,
            chip_on_bg: 0x7FBAC0,
            chip_on_text: 0x10140D,
            chip_off_bg: 0x171C14,
            chip_off_edge: Some(0x3F4A37),
            chip_radius: 0.,
            free: 0x9DB86A,
            primary: 0x7FBAC0,
            primary_text: 0x10140D,
            s_title: 32.,
            s_entry: 15.,
            s_label: 12.,
            caps: true,
        },
        Look::Command => OverlayStyle {
            scrim: 0x050605F2,
            panel: 0x1E2124,
            edge: Some(0x6A7076),
            radius: 0.,
            frame: None,
            title_bar: Some(0x0A0C0A),
            label: 0xA8AE96,
            text: 0xE6E8DA,
            bright: 0xF2F2E6,
            muted: 0x7E8A70,
            field: 0x050605,
            field_edge: Some(0x3A4038),
            field_radius: 0.,
            sel_bg: 0x1E2A16,
            sel_edge: 0xFFC21A,
            sel_text: 0xF2F2E6,
            chip_on_bg: 0x5EA634,
            chip_on_text: 0x0B0D07,
            chip_off_bg: 0x3C4247,
            chip_off_edge: Some(0x6E767C),
            chip_radius: 0.,
            free: 0x8FE05A,
            primary: 0x5EA634,
            primary_text: 0x0B0D07,
            s_title: 30.,
            s_entry: 22.,
            s_label: 16.,
            caps: true,
        },
        _ => OverlayStyle {
            scrim: if t.night { 0x000000E6 } else { 0x1A1C22E6 },
            panel: t.card,
            edge: Some(t.edge),
            radius: f32::from(t.r_card),
            frame: None,
            title_bar: None,
            label: t.ink_2,
            text: t.ink,
            bright: t.ink,
            muted: t.ink_3,
            field: t.page,
            field_edge: Some(t.field_line),
            field_radius: f32::from(t.r_button),
            sel_bg: t.track,
            sel_edge: t.ink,
            sel_text: t.ink,
            chip_on_bg: t.ink,
            chip_on_text: t.card,
            chip_off_bg: t.card,
            chip_off_edge: Some(t.edge),
            chip_radius: 999.,
            free: t.ok_text,
            primary: t.primary,
            primary_text: 0xFFFFFF,
            s_title: 22.,
            s_entry: 15.,
            s_label: 12.,
            caps: false,
        },
    }
}

/// The fonts of the look: body, mono (paths) and heading.
pub fn fonts() -> (&'static str, &'static str, &'static str) {
    let t = tk();
    match t.look {
        Look::Command => (
            crate::command_kit::SANS,
            crate::command_kit::SANS,
            crate::command_kit::SANS,
        ),
        Look::Tactical => (
            crate::tactical_kit::SANS,
            crate::tactical_kit::MONO,
            crate::tactical_kit::SANS,
        ),
        Look::Bridge => (
            crate::bridge_kit::BODY,
            crate::bridge_kit::BODY,
            crate::bridge_kit::BODY,
        ),
        Look::Operator => ("IBM Plex Mono", "IBM Plex Mono", "IBM Plex Mono"),
        Look::Holo => (
            crate::holo_kit::BODY,
            crate::holo_kit::BODY,
            crate::holo_kit::BODY,
        ),
        Look::Neon => ("Chakra Petch", "Chakra Petch", "Orbitron"),
        _ => (t.sans, t.mono, t.display_heading),
    }
}

pub fn color(v: u32) -> Rgba {
    if v > 0xFFFFFF { rgba(v) } else { rgb(v) }
}

/// `true` when the active look keeps the plain Minimal overlays (the `tk()` tokens).
pub fn plain() -> bool {
    tk().look == Look::Minimal
}

/// The border color of a framed overlay part.
fn edge(st: &OverlayStyle) -> Rgba {
    color(
        st.edge
            .or(st.field_edge)
            .or(st.frame)
            .unwrap_or(st.sel_edge),
    )
}

/// The corner radius of a menu or a toast: LCARS rounds it, the hard looks keep it square.
fn box_radius(st: &OverlayStyle) -> f32 {
    if tk().look == Look::Bridge {
        14.
    } else if st.radius == 0. {
        0.
    } else {
        st.radius.min(8.)
    }
}

/// The frame of a menu, a picker or a toast card, in the style of the active look: panel
/// color, edge, radius, body font, text color. Add rows with [`menu_row`].
pub fn menu_frame() -> Div {
    let st = style();
    let (sans, _, _) = fonts();
    div()
        .flex()
        .flex_col()
        .bg(color(st.panel))
        .border_1()
        .border_color(edge(&st))
        .rounded(px(box_radius(&st)))
        .font_family(sans)
        .text_color(color(st.text))
        .when(tk().look == Look::Neon, |d| {
            // The glow of the Neon sits on the frame.
            d.shadow(vec![gpui::BoxShadow {
                color: crate::ui::hsla(0x00E0F0 << 8 | 0x33),
                offset: gpui::point(px(0.), px(0.)),
                blur_radius: px(16.),
                spread_radius: px(0.),
                inset: false,
            }])
        })
}

/// The skin of the active look on any box that is already built (a popup, a picker, a
/// confirm card): panel color, edge, radius, body font and text color. Minimal is untouched.
pub fn skin<E: Styled>(d: E) -> E {
    if plain() {
        return d;
    }
    let st = style();
    let (sans, _, _) = fonts();
    d.bg(color(st.panel))
        .border_1()
        .border_color(edge(&st))
        .rounded(px(box_radius(&st)))
        .font_family(sans)
        .text_color(color(st.text))
}

/// A menu row in the style of the active look: hover and selection colors, radius, font.
pub fn menu_row(row: Stateful<Div>, on: bool) -> Stateful<Div> {
    let st = style();
    let (sans, _, _) = fonts();
    let hover = color(st.sel_bg);
    let hover_text = color(st.sel_text);
    row.font_family(sans)
        .rounded(px(if box_radius(&st) > 8. {
            10.
        } else {
            box_radius(&st).min(4.)
        }))
        .text_color(if on { color(st.bright) } else { color(st.text) })
        .hover(move |s| s.bg(hover).text_color(hover_text))
}

/// The look's button: primary (filled) or secondary (edge only). Text is upper case when the
/// look uses `caps`; the caller applies [`caps_text`] to the label.
pub fn button(row: Stateful<Div>, primary: bool) -> Stateful<Div> {
    let st = style();
    let (sans, _, _) = fonts();
    let radius = if tk().look == Look::Bridge {
        st.chip_radius
    } else {
        st.field_radius
    };
    let base = row
        .font_family(sans)
        .font_weight(FontWeight::MEDIUM)
        .rounded(px(radius));
    if primary {
        base.bg(color(st.primary))
            .text_color(color(st.primary_text))
    } else {
        base.border_1()
            .border_color(edge(&st))
            .bg(color(st.chip_off_bg))
            .text_color(color(st.text))
            .hover(move |s| s.border_color(color(st.sel_edge)))
    }
}

/// The look's choice chip, selected or not.
pub fn chip(row: Stateful<Div>, on: bool) -> Stateful<Div> {
    let st = style();
    let (sans, _, _) = fonts();
    let base = row.font_family(sans).rounded(px(st.chip_radius.min(20.)));
    if on {
        base.bg(color(st.chip_on_bg))
            .text_color(color(st.chip_on_text))
    } else {
        base.bg(color(st.chip_off_bg))
            .text_color(color(st.text))
            .when_some(st.chip_off_edge, |d, e| d.border_1().border_color(color(e)))
    }
}

/// The look's text field box: field color, edge, radius, body font.
pub fn field_box(d: Div) -> Div {
    let st = style();
    let (sans, _, _) = fonts();
    d.font_family(sans)
        .bg(color(st.field))
        .text_color(color(st.bright))
        .rounded(px(st.field_radius))
        .border_1()
        .border_color(color(st.field_edge.unwrap_or(st.field)))
}

/// The label in the case of the look (upper case for LCARS).
pub fn caps_text(s: &str) -> String {
    if style().caps {
        s.to_uppercase()
    } else {
        s.to_string()
    }
}
