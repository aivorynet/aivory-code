//! The app frame in the Operator look (ADR 0022, mockup `OperatorCore`): the F-keys in the top
//! line are the only navigation of the app; the rail and the status bar do not show. The one
//! session screen is `operator_focus`.

use aivory_protocol::SessionState;
use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, Context, DispatchPhase, Div, KeyDownEvent, Window, anchored, canvas,
    deferred, div, point, px, rgb,
};

use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::look::LOOKS;
use crate::ui;
use crate::warroom::skins::operator::kit::*;

/// The height of the top line.
const TOP_H: f32 = 44.;

impl Shell {
    /// The whole Operator window: the top line with the F-keys, the screen, the notices.
    pub(super) fn operator_screen(
        &mut self,
        content: AnyElement,
        toasts: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (live, idle, asks) = self.model.read(cx).war.as_ref().map_or((0, 0, 0), |w| {
            let live = w.sessions.iter().filter(|s| s.state.is_live());
            (
                live.clone().count(),
                live.clone()
                    .filter(|s| s.state == SessionState::Idle)
                    .count(),
                live.filter(|s| s.ask.is_some()).count(),
            )
        });
        let inbox = self.model.read(cx).war.as_ref().map_or(0, |w| w.inbox_open);
        let here = self.screen.rail();
        let nav = |id: &'static str, text: String, screen: Screen, cx: &mut Context<Self>| {
            let on = here == screen;
            div()
                .id(id)
                .flex_none()
                .cursor_pointer()
                .text_size(ui::ts(MX_BODY))
                .text_color(rgb(if on { GREEN_HI } else { GREEN_MID }))
                .hover(|h| h.text_color(rgb(GREEN_HI)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.look_menu = false;
                    this.go(screen.clone(), cx);
                }))
                .child(text)
        };
        let mut top = div()
            .flex_none()
            .min_h(ui::sw(TOP_H))
            .px(px(30.))
            .py(px(6.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(26.))
            .gap_y(px(4.))
            .child(
                div()
                    .flex_none()
                    .text_size(ui::ts(MX_TITLE))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(GREEN_HI))
                    .child("AIVORY//OPERATOR"),
            )
            .child(nav("mx-f1", "F1 war room".into(), Screen::WarRoom, cx))
            .child(nav(
                "mx-f2",
                format!("F2 inbox ({inbox})"),
                Screen::Inbox,
                cx,
            ))
            .child(nav("mx-f3", "F3 tasks".into(), Screen::Tasks, cx))
            .child(nav("mx-f4", "F4 repos".into(), Screen::Repos, cx))
            .child(nav("mx-f5", "F5 settings".into(), Screen::Settings, cx))
            .children(crate::new_session::launch_buttons(
                "operator",
                |id, label| {
                    div()
                        .id(id)
                        .flex_none()
                        .cursor_pointer()
                        .text_size(ui::ts(MX_BODY))
                        .text_color(rgb(GREEN_MID))
                        .hover(|h| h.text_color(rgb(GREEN_HI)))
                        .child(if label == "Organizer" {
                            "F6 organizer"
                        } else {
                            "F7 new task"
                        })
                },
            ));
        if let Screen::Session(id) = &self.screen {
            top = top.child(
                div()
                    .flex_none()
                    .text_size(ui::ts(MX_BODY))
                    .text_color(rgb(GREEN_HI))
                    .child(format!(
                        "session {}",
                        id.chars().take(6).collect::<String>()
                    )),
            );
        }
        let crumb = match &self.screen {
            Screen::Session(_) => Some("session"),
            Screen::Lens => Some("lens"),
            _ => None,
        };
        if let Some(crumb) = crumb {
            top = top.child(
                div()
                    .text_size(ui::ts(MX_BODY))
                    .text_color(rgb(GREEN_HI))
                    .child(crumb),
            );
        }
        top = top
            .child(div().flex_1().min_w(px(0.)))
            .child(
                div()
                    .flex_none()
                    .text_size(ui::ts(MX_BODY))
                    .text_color(rgb(GREEN_MID))
                    .child(format!("{live} agents · {idle} idle")),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(ui::ts(MX_BODY))
                    .text_color(rgb(BLUE))
                    .child(format!("{asks} decisions")),
            )
            .child(self.operator_look_switch(cx));

        // F1 to F5 go to the screens; Escape leaves a session; J and K step through sessions.
        let weak = cx.entity().downgrade();
        let steer_focus = self
            .operator_screens
            .focus
            .as_ref()
            .map(|(v, _)| v.read(cx).input_focus(cx));
        let keys = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_key_event(move |e: &KeyDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || e.is_held {
                        return;
                    }
                    let key = e.keystroke.key.as_str();
                    let screen = match key {
                        "f1" => Some(Screen::WarRoom),
                        "f2" => Some(Screen::Inbox),
                        "f3" => Some(Screen::Tasks),
                        "f4" => Some(Screen::Repos),
                        "f5" => Some(Screen::Settings),
                        _ => None,
                    };
                    if let Some(screen) = screen {
                        let _ = weak.update(cx, |s, cx| s.go(screen, cx));
                        return;
                    }
                    let typing = steer_focus.as_ref().is_some_and(|f| f.is_focused(window));
                    if e.keystroke.modifiers.modified() || (typing && key != "escape") {
                        return;
                    }
                    let _ = weak.update(cx, |s, cx| match key {
                        "escape" if matches!(s.screen, Screen::Session(_)) => {
                            s.go(Screen::WarRoom, cx)
                        }
                        "j" => s.operator_step(true, cx),
                        "k" => s.operator_step(false, cx),
                        _ => {}
                    });
                });
            },
        )
        .absolute()
        .inset_0();

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(PAGE))
            .font_family(FONT)
            .text_size(ui::ts(MX_BODY))
            .text_color(rgb(GREEN))
            .child(top)
            .child(div().relative().flex_1().min_h(px(0.)).child(content))
            .children(toasts)
            .child(keys)
    }

    /// The Operator versions of Inbox, Tasks, Repos and Settings replace the standard screens.
    /// Focus, Lens and the full Settings keep their content inside a Operator frame.
    pub(super) fn operator_content(
        &mut self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::operator_screens::*;
        fn handle(this: &mut Shell, event: &OperatorEvent, cx: &mut Context<Shell>) {
            match event {
                OperatorEvent::OpenSession(id) => this.open_session(id, cx),
                OperatorEvent::AllSettings => {
                    this.operator_full_settings = true;
                    cx.notify();
                }
            }
        }
        if self.screen != Screen::Settings {
            self.operator_full_settings = false;
        }
        let model = self.model.clone();
        match self.screen.clone() {
            Screen::Inbox => {
                if self.operator_screens.inbox.is_none() {
                    let v = cx.new(|cx| OperatorInbox::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &OperatorEvent, cx| handle(t, e, cx));
                    self.operator_screens.inbox = Some((v, sub));
                }
                self.operator_screens
                    .inbox
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Tasks => {
                if self.operator_screens.tasks.is_none() {
                    let v = cx.new(|cx| OperatorTasks::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &OperatorEvent, cx| handle(t, e, cx));
                    self.operator_screens.tasks = Some((v, sub));
                }
                self.operator_screens
                    .tasks
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Repos => {
                if self.operator_screens.repos.is_none() {
                    let v = cx.new(|cx| OperatorRepos::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &OperatorEvent, cx| handle(t, e, cx));
                    self.operator_screens.repos = Some((v, sub));
                }
                self.operator_screens
                    .repos
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Settings if !self.operator_full_settings => {
                if self.operator_screens.settings.is_none() {
                    let v = cx.new(|cx| OperatorSettings::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &OperatorEvent, cx| handle(t, e, cx));
                    self.operator_screens.settings = Some((v, sub));
                }
                self.operator_screens
                    .settings
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Settings => self.operator_frame(
                "F5",
                "Settings",
                vec![("view".into(), "all settings".into())],
                true,
                Some(("‹ Settings", Screen::Settings, true)),
                content,
                cx,
            ),
            Screen::Session(id) => {
                use super::operator_focus::{OperatorFocus, OperatorFocusEvent};
                if let Some((v, _)) = &self.operator_screens.focus {
                    v.update(cx, |v, cx| v.show(&id, cx));
                } else {
                    let v = cx.new(|cx| OperatorFocus::new(model, id.clone(), window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &OperatorFocusEvent, cx| match e {
                        OperatorFocusEvent::Open(id) => t.open_session(id, cx),
                        OperatorFocusEvent::OpenFile { session, path } => {
                            t.open_session_file(session, path, cx)
                        }
                        OperatorFocusEvent::Back => t.go(Screen::WarRoom, cx),
                    });
                    self.operator_screens.focus = Some((v, sub));
                }
                self.operator_screens
                    .focus
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Lens => {
                use super::operator_lens::{OperatorLens, OperatorLensEvent};
                if self.operator_screens.lens.is_none() {
                    let v = cx.new(|cx| OperatorLens::new(model, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &OperatorLensEvent, cx| match e {
                        OperatorLensEvent::Back => {
                            let back = std::mem::replace(&mut t.lens_back, Screen::Repos);
                            t.go(back, cx);
                        }
                        OperatorLensEvent::OpenSession(id) => t.open_session(id, cx),
                    });
                    self.operator_screens.lens = Some((v, sub));
                }
                let target = self.operator_screens.lens_target.take();
                if let (Some((v, _)), Some((codebase, path, session))) =
                    (&self.operator_screens.lens, target)
                {
                    v.update(cx, |v, cx| v.show(&codebase, &path, session, cx));
                }
                self.operator_screens
                    .lens
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            _ => content,
        }
    }

    /// A screen of the standard app inside a Operator frame: the title line with the F-key, a
    /// back button, and a monitor that clips the content to its round glass.
    #[allow(clippy::too_many_arguments)]
    fn operator_frame(
        &mut self,
        fkey: &str,
        title: &str,
        stats: Vec<(String, String)>,
        blue: bool,
        back: Option<(&'static str, Screen, bool)>,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use super::operator_screens::{btn, head_line, page};
        let mut head = head_line(fkey, title, stats);
        if let Some((text, to, settings_inner)) = back {
            head = head.child(
                btn("mx-frame-back".into(), text, false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if settings_inner {
                            this.operator_full_settings = false;
                            cx.notify();
                        } else {
                            this.go(to.clone(), cx);
                        }
                    },
                )),
            );
        }
        let frame = bezel(if blue { BLUE_RING } else { GREEN_MID })
            .flex_1()
            .min_w(px(0.))
            .child(
                glass(blue)
                    .child(div().absolute().inset_0().flex().flex_col().child(content))
                    .child(glaze(blue)),
            )
            .child(foot(
                "1",
                if blue { BLUE } else { GREEN },
                vec![(title.to_lowercase(), FOOT)],
                30.,
            ));
        page(head, vec![frame.into_any_element()]).into_any_element()
    }

    /// The look switch of the top line: the look; a click shows all looks.
    fn operator_look_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = |id: String, label: &'static str, on: bool| {
            div()
                .id(eid(id))
                .flex_none()
                .flex()
                .items_center()
                .whitespace_nowrap()
                .cursor_pointer()
                .text_size(ui::ts(MX_BODY))
                .text_color(rgb(if on { GREEN_HI } else { GREEN_MID }))
                .hover(|h| h.text_color(rgb(GREEN_HI)))
                .child(label)
        };
        if !self.look_menu {
            return text(
                "mx-look".into(),
                crate::look::name(crate::theme::tk().look),
                false,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.look_menu = true;
                cx.notify();
            }))
            .child(" ▾")
            .into_any_element();
        }
        let current = crate::theme::tk().look;
        let mut list = div()
            .id("mx-look-list")
            .occlude()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .py(px(8.))
            .min_w(px(120.))
            .bg(rgb(PAGE))
            .border_1()
            .border_color(rgb(GREEN_DIM))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.look_menu = false;
                cx.notify();
            }));
        for (look, name) in LOOKS {
            list = list.child(
                text(format!("mx-look-{name}"), name, look == current).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.look_menu = false;
                        act(&this.model, Action::SetLook(look), cx);
                    },
                )),
            );
        }
        div()
            .relative()
            .child(
                text("mx-look".into(), crate::look::name(current), true)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.look_menu = false;
                        cx.notify();
                    }))
                    .child(" ▾"),
            )
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .offset(point(px(0.), px(22.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(list),
                )
                .with_priority(3),
            )
            .into_any_element()
    }

    /// J and K: the next or the previous live session.
    fn operator_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Screen::Session(id) = &self.screen else {
            return;
        };
        let ids: Vec<String> = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| {
                w.sessions
                    .iter()
                    .filter(|s| s.state.is_live())
                    .map(|s| s.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        let at = ids.iter().position(|i| i == id).unwrap_or(0);
        let next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        self.screen = Screen::Session(ids[next].clone());
        cx.notify();
    }
}
