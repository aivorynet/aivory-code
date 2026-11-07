//! The app frame in the Neon look (ADR 0022, `NeonGrid`): a neon header with the
//! tabs in place of the rail and the status bar. The session screen is `neon_session`.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, px, rgb};

use super::neon_kit::*;
use super::{Screen, Shell};
use crate::theme::{T_CAPTION, T_SMALL};
use crate::ui;

/// The height of the header.
const HEAD_H: f32 = 56.;

impl Shell {
    /// The whole Neon frame: header, the screen, the toasts.
    pub(super) fn neon_screen(
        &mut self,
        content: AnyElement,
        toasts: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let head = self.nc_header(cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .font_family("Chakra Petch")
            .text_size(ui::ts(T_SMALL))
            .text_color(rgb(INK))
            .child(head)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .child(content)
                    .when(self.screen != Screen::WarRoom, |d| d.child(scanlines())),
            )
            .children(toasts)
    }

    /// The header: logo, one tab per screen, the live counts and the look switch.
    fn nc_header(&self, cx: &mut Context<Self>) -> Div {
        let model = self.model.read(cx);
        let inbox = model.war.as_ref().map_or(0, |w| w.inbox_open);
        let counts = model.war.as_ref().map(|w| w.counts());
        let here = self.screen.rail();
        let tabs: [(&'static str, Screen); 5] = [
            ("War room", Screen::WarRoom),
            ("Inbox", Screen::Inbox),
            ("Tasks", Screen::Tasks),
            ("Repos", Screen::Repos),
            ("Settings", Screen::Settings),
        ];
        let mut nav = div().flex().items_center().gap(px(6.));
        for (label, screen) in tabs {
            let on = here == screen;
            let target = screen.clone();
            let mut tab = div()
                .id(SharedString::from(format!("nc-tab-{label}")))
                .h(px(34.))
                .px(px(16.))
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_SMALL))
                .bg(rgb(if on { YELLOW } else { PANEL }))
                .text_color(rgb(if on { DARK } else { INK }))
                .hover(|h| h.bg(rgb(if on { YELLOW } else { 0x232C55 })))
                .child(label.to_uppercase());
            if screen == Screen::Inbox && inbox > 0 {
                tab = tab.child(
                    div()
                        .text_color(rgb(if on { DARK } else { RED }))
                        .child(inbox.to_string()),
                );
            }
            nav = nav.child(tab.on_click(cx.listener(
                move |this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                    this.look_menu = false;
                    this.go(target.clone(), cx);
                },
            )));
        }
        let part = |text: String, color: u32| div().text_color(rgb(color)).child(text);
        let mut line = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .font_weight(FontWeight::BOLD)
            .text_size(ui::ts(T_CAPTION));
        if let Some(c) = counts {
            line = line
                .child(part(format!("{} LIVE", c.live), MIST))
                .child(part("//".into(), MIST))
                .child(part(
                    format!("{} WAIT FOR YOU", c.needs_you),
                    if c.needs_you > 0 { RED } else { MIST },
                ))
                .child(part("//".into(), MIST))
                .child(part(
                    format!("{} FAULT", c.error),
                    if c.error > 0 { YELLOW } else { MIST },
                ))
                .child(part("//".into(), MIST))
                .child(part(format!("{} IDLE", c.idle), MIST));
        }
        div()
            .flex_none()
            .h(px(HEAD_H))
            .px(px(24.))
            .flex()
            .items_center()
            .gap(px(22.))
            .child(
                div()
                    .flex()
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(26.))
                    .text_color(rgb(YELLOW))
                    .child("AIVORY")
                    .child(div().text_color(rgb(RED)).child("//"))
                    .child("CITY"),
            )
            .child(nav)
            .child(div().flex_1())
            .child(line)
            .child(self.look_switch(cx))
    }
}
