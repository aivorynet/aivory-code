//! The app frame in the Navigator look (ADR 0022): the Toolchest bar of an early workstation
//! desktop in place of the rail and the status bar, and a grey window frame around every
//! screen except the war room, which draws its own windows.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, px, rgb};

use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::look::LOOKS;
use crate::theme::*;
use crate::ui;
use crate::warroom::skins::navigator::panels::{
    BTN_HOT, Click, Controls, FACE, OUTLINE, TEXT, TITLE, bevel, frame,
};

thread_local! {
    /// The Windows menu of the Toolchest is open.
    static WINDOWS_MENU: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The height of the Toolchest bar. The war room dialog centers on the window with it.
pub(super) const CHEST_H: f32 = 26.;
/// The desktop color behind the windows.
pub(super) const DESKTOP: u32 = 0x22384A;

impl Shell {
    /// The Toolchest: one entry per screen, and the look switch.
    pub(super) fn irix_chest(&self, cx: &mut Context<Self>) -> Div {
        let inbox = self.model.read(cx).war.as_ref().map_or(0, |w| w.inbox_open);
        let here = self.screen.rail();
        let (asks, open) = self.war_room.read(cx).nav_decisions();
        let entry = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .h(px(20.))
                .px(px(10.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(ui::ts(T_SMALL))
                .when(on, |d| d.font_weight(FontWeight::BOLD))
                .hover(|h| h.bg(rgb(BTN_HOT)))
                .child(label)
        };
        let go = |screen: Screen| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                this.look_menu = false;
                this.go(screen.clone(), cx);
            })
        };
        let row = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .size_full()
            .child(
                div()
                    .px(px(8.))
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(T_SMALL))
                    .child("Toolchest"),
            )
            .child(
                entry("tc-war", "War room".into(), here == Screen::WarRoom)
                    .on_click(go(Screen::WarRoom)),
            )
            .child(
                entry(
                    "tc-inbox",
                    format!("Inbox ({inbox})"),
                    here == Screen::Inbox,
                )
                .on_click(go(Screen::Inbox)),
            )
            .child(
                entry("tc-tasks", "Tasks".into(), here == Screen::Tasks)
                    .on_click(go(Screen::Tasks)),
            )
            .child(
                entry("tc-repos", "Repos".into(), here == Screen::Repos)
                    .on_click(go(Screen::Repos)),
            )
            .child(
                entry("tc-set", "Settings".into(), here == Screen::Settings)
                    .on_click(go(Screen::Settings)),
            )
            .child(
                entry("tc-windows", "Windows ▾".into(), WINDOWS_MENU.get()).on_click(cx.listener(
                    |this, _, _, cx| {
                        WINDOWS_MENU.set(!WINDOWS_MENU.get());
                        this.look_menu = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                // Pending decisions always show here; red when some wait.
                entry("tc-decisions", format!("Decisions ({asks})"), open)
                    .text_color(rgb(if asks > 0 { 0xB3141C } else { TEXT }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if asks > 0 {
                            this.go(Screen::WarRoom, cx);
                            this.war_room
                                .update(cx, |w, cx| w.nav_open_window("dialog", cx));
                        }
                    })),
            )
            .children(crate::new_session::launch_buttons("irix", |id, label| {
                entry(id, label.to_string(), false)
            }))
            .child(div().flex_1())
            .child(
                entry("tc-look", "Look ▾".into(), self.look_menu).on_click(cx.listener(
                    |this, _, _, cx| {
                        WINDOWS_MENU.set(false);
                        this.look_menu = !this.look_menu;
                        cx.notify();
                    },
                )),
            );
        div()
            .flex_none()
            .h(px(CHEST_H))
            .bg(rgb(FACE))
            .border_b_1()
            .border_color(rgb(OUTLINE))
            .font_family(tk().sans)
            .text_color(rgb(TEXT))
            .child(row)
    }

    /// The popups of the Toolchest: the Windows menu and the list of looks.
    pub(super) fn irix_look_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        let windows = self.irix_windows_menu(cx);
        let looks = self.irix_looks(cx);
        if windows.is_none() && looks.is_none() {
            return None;
        }
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .children(windows)
                .children(looks),
        )
    }

    /// The Windows menu: every window of the war room; a closed one opens again.
    fn irix_windows_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !WINDOWS_MENU.get() {
            return None;
        }
        let mut list = div().flex().flex_col().p(px(2.)).gap(px(2.)).bg(rgb(FACE));
        for (key, title, open) in self.war_room.read(cx).nav_window_list() {
            list = list.child(
                div()
                    .id(SharedString::from(format!("tc-win-{key}")))
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .when(open, |d| d.font_weight(FontWeight::BOLD))
                    .hover(|h| h.bg(rgb(BTN_HOT)))
                    .child(format!("{} {title}", if open { "✓" } else { "  " }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        WINDOWS_MENU.set(false);
                        this.go(Screen::WarRoom, cx);
                        this.war_room.update(cx, |w, cx| w.nav_open_window(key, cx));
                    })),
            );
        }
        Some(
            div()
                .absolute()
                .top(px(CHEST_H))
                .right(px(70.))
                .w(px(210.))
                .occlude()
                .border_1()
                .border_color(rgb(OUTLINE))
                .font_family(tk().sans)
                .text_color(rgb(TEXT))
                .child(bevel(true, list)),
        )
    }

    /// The list of looks under the Look entry.
    fn irix_looks(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.look_menu {
            return None;
        }
        let current = tk().look;
        let mut list = div().flex().flex_col().p(px(2.)).gap(px(2.)).bg(rgb(FACE));
        for (i, (look, name)) in LOOKS.iter().enumerate() {
            let look = *look;
            let on = look == current;
            list = list.child(
                div()
                    .id(SharedString::from(format!("tc-look-{i}")))
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .when(on, |d| d.bg(rgb(TITLE)).text_color(rgb(0xFFFFFF)))
                    .when(!on, |d| d.hover(|h| h.bg(rgb(BTN_HOT))))
                    .child(format!("Look: {name}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.look_menu = false;
                        act(&this.model, Action::SetLook(look), cx);
                        cx.notify();
                    })),
            );
        }
        Some(
            div()
                .absolute()
                .top(px(CHEST_H))
                .right(px(8.))
                .w(px(190.))
                .occlude()
                .border_1()
                .border_color(rgb(OUTLINE))
                .font_family(tk().sans)
                .text_color(rgb(TEXT))
                .child(bevel(true, list)),
        )
    }

    /// The window menu of a screen window: Restore, Minimize, Maximize.
    pub(super) fn irix_win_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        let hook = self.screen != Screen::WarRoom && std::env::var("AIVORY_NAV_MENU").is_ok();
        if !self.irix_menu && !hook {
            return None;
        }
        let key = self.screen.label().to_string();
        let mut list = div().flex().flex_col().p(px(2.)).bg(rgb(FACE));
        for (label, min) in [
            ("Restore", Some(false)),
            ("Minimize", Some(true)),
            ("Maximize", Some(false)),
            ("Close", None),
        ] {
            let key = key.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("tc-wm-{label}")))
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .hover(|h| h.bg(rgb(BTN_HOT)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // The window already fills the main area, so Maximize and Restore
                        // both show it whole.
                        match min {
                            Some(true) => {
                                this.irix_min.insert(key.clone());
                            }
                            Some(false) => {
                                this.irix_min.remove(&key);
                            }
                            // Close leaves the screen for the war room.
                            None => this.go(Screen::WarRoom, cx),
                        }
                        this.irix_menu = false;
                        cx.notify();
                    })),
            );
        }
        Some(
            div()
                .absolute()
                .top(px(CHEST_H + 34.))
                .left(px(18.))
                .w(px(120.))
                .occlude()
                .border_1()
                .border_color(rgb(OUTLINE))
                .font_family(tk().sans)
                .text_color(rgb(TEXT))
                .child(bevel(true, list)),
        )
    }

    /// A screen in a grey window on the desktop.
    pub(super) fn irix_window(&self, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let key = self.screen.label().to_string();
        let minimized = self.irix_min.contains(&key);
        let menu: Click = Rc::new(cx.listener(|this: &mut Self, _: &gpui::ClickEvent, _, cx| {
            this.irix_menu = !this.irix_menu;
            cx.notify();
        }));
        let k = key.clone();
        let min: Click = Rc::new(cx.listener(
            move |this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                if !this.irix_min.remove(&k) {
                    this.irix_min.insert(k.clone());
                }
                cx.notify();
            },
        ));
        let close: Click = Rc::new(cx.listener(|this: &mut Self, _: &gpui::ClickEvent, _, cx| {
            this.go(Screen::WarRoom, cx);
        }));
        let ctl = Controls {
            key: "screen".to_string(),
            menu: Some(menu),
            min: Some(min),
            close: Some(close),
            drag: None,
        };
        let body = if minimized {
            div()
        } else {
            div().size_full().bg(rgb(tk().page)).child(content)
        };
        // The Focus window carries the name of its session.
        let title = match &self.screen {
            Screen::Session(id) => {
                let name = self
                    .model
                    .read(cx)
                    .war
                    .as_ref()
                    .and_then(|w| w.session(id))
                    .map(|s| {
                        if s.title.is_empty() {
                            s.id.clone()
                        } else {
                            s.title.clone()
                        }
                    })
                    .unwrap_or_else(|| id.clone());
                format!("{} — {name}", self.screen.label())
            }
            other => other.label().to_string(),
        };
        let win = frame(title, TITLE, ctl, body).w_full();
        div()
            .size_full()
            .p(px(12.))
            .bg(rgb(DESKTOP))
            .child(if minimized {
                win.h(px(24.))
            } else {
                win.h_full()
            })
            .into_any_element()
    }
}
