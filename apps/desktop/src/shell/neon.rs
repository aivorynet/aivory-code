//! The app frame in the Neon look (ADR 0022, `NeonGrid`): a neon header with the
//! tabs in place of the rail and the status bar. The session screen is `neon_focus`.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, px, rgb};

use super::neon_kit::*;
use super::{Screen, Shell};
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
            .text_size(ui::ts(T_LABEL))
            .text_color(rgb(INK))
            .child(head)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
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
        let mut nav = div().flex().items_center().gap(ui::sw(6.));
        for (label, screen) in tabs {
            let on = here == screen;
            let target = screen.clone();
            let mut tab = div()
                .id(SharedString::from(format!("nc-tab-{label}")))
                .h(ui::sw(34.))
                .px(ui::sw(16.))
                .flex()
                .items_center()
                .gap(ui::sw(6.))
                .cursor_pointer()
                .font_weight(FontWeight::BOLD)
                .text_size(ui::ts(T_LABEL))
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
        // The launchers open a tool and not a screen: an outlined cyan button with a plus.
        for tab in crate::new_session::launch_buttons_open(
            "neon",
            self.launcher_open(cx),
            |id, label, on| {
                div()
                    .id(id)
                    .h(ui::sw(34.))
                    .px(ui::sw(14.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(T_LABEL))
                    .border_1()
                    .border_color(rgb(CYAN))
                    .text_color(rgb(CYAN))
                    .when(on, |d| d.bg(rgb(0x232C55)))
                    .hover(|h| h.bg(rgb(0x232C55)))
                    .child(format!("+ {}", label.to_uppercase()))
            },
        ) {
            nav = nav.child(tab);
        }
        // Each count opens the screen that shows it.
        let part = |id: &'static str, text: String, color: u32, to: Screen| {
            div()
                .id(id)
                .cursor_pointer()
                .text_color(rgb(color))
                .hover(|h| h.text_color(rgb(PALE)))
                .child(text)
                .on_click(
                    cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                        this.look_menu = false;
                        this.go(to.clone(), cx);
                    }),
                )
        };
        let sep = || div().text_color(rgb(MIST)).child("//");
        let mut line = div()
            .flex()
            .flex_shrink(1.)
            .min_w(px(0.))
            .overflow_hidden()
            .whitespace_nowrap()
            .items_center()
            .gap(ui::sw(8.))
            .font_weight(FontWeight::BOLD)
            .text_size(ui::ts(T_LABEL));
        if let Some(c) = counts {
            line = line
                .child(part(
                    "nc-live",
                    format!("{} LIVE", c.live),
                    MIST,
                    Screen::WarRoom,
                ))
                .child(sep())
                .child(part(
                    "nc-wait",
                    format!("{} WAIT FOR YOU", c.needs_you),
                    if c.needs_you > 0 { RED } else { MIST },
                    Screen::Inbox,
                ))
                .child(sep())
                .child(part(
                    "nc-fault",
                    format!("{} FAULT", c.error),
                    if c.error > 0 { YELLOW } else { MIST },
                    Screen::Inbox,
                ))
                .child(sep())
                .child(part(
                    "nc-idle",
                    format!("{} IDLE", c.idle),
                    MIST,
                    Screen::WarRoom,
                ));
        }
        div()
            .flex_none()
            .h(ui::sw(HEAD_H))
            .px(ui::sw(24.))
            .flex()
            .items_center()
            .gap(ui::sw(22.))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(T_SECTION))
                    .text_color(rgb(YELLOW))
                    .child("AIVORY")
                    .child(div().text_color(rgb(RED)).child("//"))
                    .child("CITY"),
            )
            .child(nav.flex_none())
            .child(div().flex_1().min_w(px(0.)))
            .child(line)
            .child(self.look_switch(cx))
    }
}
