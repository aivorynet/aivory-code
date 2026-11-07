//! The app frame in the Holo and Tactical looks (ADR 0022): one thin navigation bar on top in
//! place of the rail and the status bar. The bar holds War room, Inbox, Tasks, Repos,
//! Settings and the look switch. The session views are part of the war room screen.

use gpui::prelude::*;
use gpui::{Context, Div, Hsla, SharedString, div, px};

use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::look::LOOKS;
use crate::theme::*;
use crate::ui;

/// The height of the bar.
const BAR_H: f32 = 36.;

/// A color `0xRRGGBB` with an alpha of 0 to 1.
fn ink(rgb: u32, alpha: f32) -> Hsla {
    ui::hsla(rgb << 8 | (alpha.clamp(0., 1.) * 255.) as u32)
}

impl Shell {
    /// The navigation bar of the Holo and Tactical looks.
    pub(super) fn hud_bar(&self, cx: &mut Context<Self>) -> Div {
        let t = tk();
        let (line, accent, text, dim) = (t.line_2, t.logo, t.ink, t.muted);
        let inbox = self.model.read(cx).war.as_ref().map_or(0, |w| w.inbox_open);
        let here = self.screen.rail();
        let entry = |id: &'static str, label: String, screen: Screen, cx: &mut Context<Self>| {
            let on = here == screen;
            div()
                .id(id)
                .h_full()
                .px(px(14.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(ui::ts(T_SMALL))
                .border_b_2()
                .border_color(ink(accent, if on { 1. } else { 0. }))
                .text_color(ink(if on { text } else { dim }, 1.))
                .hover(move |h| h.text_color(ink(text, 1.)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.look_menu = false;
                    this.go(screen.clone(), cx);
                }))
        };
        div()
            .flex_none()
            .h(px(BAR_H))
            .px(px(40.))
            .flex()
            .items_center()
            .gap(px(4.))
            .border_b_1()
            .border_color(ink(line >> 8, 0.4))
            .font_family(t.sans)
            .child(
                div()
                    .pr(px(20.))
                    .text_size(ui::ts(T_SMALL))
                    .text_color(ink(accent, 1.))
                    .child("AIVORY"),
            )
            .child(entry("hud-war", "War room".into(), Screen::WarRoom, cx))
            .child(entry(
                "hud-inbox",
                format!("Inbox ({inbox})"),
                Screen::Inbox,
                cx,
            ))
            .child(entry("hud-tasks", "Tasks".into(), Screen::Tasks, cx))
            .child(entry("hud-repos", "Repos".into(), Screen::Repos, cx))
            .child(entry("hud-set", "Settings".into(), Screen::Settings, cx))
            .child(div().flex_1())
            .child(
                div()
                    .id("hud-look")
                    .h_full()
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(ink(dim, 1.))
                    .hover(move |h| h.text_color(ink(text, 1.)))
                    .child(format!("Look: {} ▾", crate::look::name(t.look)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.look_menu = !this.look_menu;
                        cx.notify();
                    })),
            )
    }

    /// The list of looks under the bar.
    pub(super) fn hud_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.look_menu {
            return None;
        }
        let t = tk();
        let mut list = div()
            .absolute()
            .top(px(BAR_H))
            .right(px(40.))
            .w(px(190.))
            .occlude()
            .flex()
            .flex_col()
            .p(px(4.))
            .bg(ink(t.card, 0.96))
            .border_1()
            .border_color(ink(t.edge, 0.8))
            .font_family(t.sans);
        for (i, (look, name)) in LOOKS.iter().enumerate() {
            let look = *look;
            let on = look == t.look;
            let (accent, text) = (t.logo, t.ink);
            list = list.child(
                div()
                    .id(SharedString::from(format!("hud-look-{i}")))
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .text_color(ink(if on { accent } else { text }, 1.))
                    .hover(move |h| h.bg(ink(accent, 0.14)))
                    .child(*name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.look_menu = false;
                        act(&this.model, Action::SetLook(look), cx);
                        cx.notify();
                    })),
            );
        }
        Some(list)
    }
}
