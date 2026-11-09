//! The app frame in the Operator look (ADR 0022, mockups `OperatorCore` and `OperatorSession`):
//! the F-keys in the top line are the only navigation of the app; the rail and the status bar
//! do not show. The session view is drawn here: a switcher on the left, the console of the
//! session in the middle, blue facts on the right.

use std::time::Instant;

use aivory_protocol::{LineKind, Pane, SessionState};
use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, Context, DispatchPhase, Div, KeyDownEvent, Window, anchored, canvas,
    deferred, div, point, px, relative, rgb,
};
use gpui_base::input::{InputEvent, InputState};

use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::look::LOOKS;
use crate::ui;
use crate::warroom::skins::operator::kit::*;

/// The gap between the monitors.
const GAP: f32 = 20.;
/// The inner padding of a screen.
const PAD: f32 = 24.;

/// The height of the top line.
const TOP_H: f32 = 44.;

/// A session in the switcher.
struct Mate {
    id: String,
    name: String,
    state: SessionState,
    /// The state and its age as text: `working 3 min`.
    wait: String,
}

/// What the session view needs, copied once per frame.
struct Data {
    pane: Pane,
    mates: Vec<Mate>,
    now: i64,
}

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
            .h(px(TOP_H))
            .px(px(30.))
            .flex()
            .items_center()
            .gap(px(26.))
            .child(
                div()
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
            .child(nav("mx-f5", "F5 settings".into(), Screen::Settings, cx));
        if let Screen::Session(id) = &self.screen {
            top = top.child(
                div()
                    .text_size(ui::ts(MX_BODY))
                    .text_color(rgb(GREEN_HI))
                    .child(format!(
                        "session {}",
                        id.chars().take(6).collect::<String>()
                    )),
            );
        }
        let crumb = match &self.screen {
            Screen::Focus(_) => Some("focus"),
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
            .child(div().flex_1())
            .child(
                div()
                    .text_size(ui::ts(MX_BODY))
                    .text_color(rgb(GREEN_MID))
                    .child(format!("{live} agents · {idle} idle")),
            )
            .child(
                div()
                    .text_size(ui::ts(MX_BODY))
                    .text_color(rgb(BLUE))
                    .child(format!("{asks} decisions")),
            )
            .child(self.operator_look_switch(cx));

        // F1 to F5 go to the screens; Escape leaves a session; J and K step through sessions.
        let weak = cx.entity().downgrade();
        let steer_focus = self
            .operator_steer
            .as_ref()
            .map(|input| gpui::Focusable::focus_handle(input, cx));
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
                        "escape" if matches!(s.screen, Screen::Session(_) | Screen::Focus(_)) => {
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
            Screen::Focus(id) => {
                use super::operator_focus::{OperatorFocus, OperatorFocusEvent};
                if let Some((v, _)) = &self.operator_screens.focus {
                    v.update(cx, |v, cx| v.show(&id, cx));
                } else {
                    let v = cx.new(|cx| OperatorFocus::new(model, id.clone(), window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &OperatorFocusEvent, cx| match e {
                        OperatorFocusEvent::Open(id) => t.open_focus(id, cx),
                        OperatorFocusEvent::Detail(id) => t.open_session(id, cx),
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

    /// The steer line, made on first use. Enter sends the text to the session on screen.
    fn operator_steer_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<InputState> {
        if let Some(input) = &self.operator_steer {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("steer this session"));
        let events = cx.subscribe_in(
            &input,
            window,
            |this: &mut Shell, input, event: &InputEvent, window, cx| {
                let InputEvent::PressEnter { .. } = event else {
                    return;
                };
                let Screen::Session(id) = &this.screen else {
                    return;
                };
                let text = input.read(cx).value().trim().to_string();
                if text.is_empty() {
                    return;
                }
                let id = id.clone();
                act(
                    &this.model,
                    Action::Send {
                        id,
                        text,
                        composer: None,
                    },
                    cx,
                );
                input.update(cx, |s, cx| s.clean(window, cx));
            },
        );
        self.operator_steer = Some(input.clone());
        self.operator_sub = Some(events);
        input
    }

    /// The session view: no rain. The newest line is decoded by a scramble cursor.
    pub(super) fn operator_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::ui::animate(window, cx);
        let t = self
            .operator_t0
            .get_or_insert_with(Instant::now)
            .elapsed()
            .as_secs_f32();
        let Screen::Session(id) = self.screen.clone() else {
            return div().into_any_element();
        };
        let steer = self.operator_steer_input(window, cx);
        let data = self.model.read(cx).war.as_ref().and_then(|w| {
            let pane = w.session(&id)?.clone();
            let mates = w
                .sessions
                .iter()
                .filter(|s| s.state.is_live())
                .map(|s| Mate {
                    id: s.id.clone(),
                    name: name(s),
                    state: s.state,
                    wait: format!(
                        "{} {} min",
                        match s.state {
                            SessionState::Working => "working",
                            SessionState::NeedsYou => "waits",
                            SessionState::Error => "error",
                            _ => "idle",
                        },
                        s.minutes(w.now)
                    ),
                })
                .collect();
            Some(Data {
                pane,
                mates,
                now: w.now,
            })
        });
        let Some(data) = data else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(GREEN_DIM))
                .child("This session is gone. Esc goes back to the wall.")
                .into_any_element();
        };
        let switcher = self.operator_mates(&data, t, cx);
        let console = self.operator_console(&data, steer, t, cx);
        let facts = operator_facts(&data, t);
        div()
            .size_full()
            .flex()
            .gap(px(GAP))
            .px(px(28.))
            .pb(px(12.))
            .pt(px(8.))
            .child(switcher)
            .child(console)
            .child(facts)
            .into_any_element()
    }

    /// The switcher: calm text rows in one monitor. A lamp shows the state; the session on
    /// screen is lit.
    fn operator_mates(&mut self, data: &Data, _t: f32, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div()
            .id("mx-mates")
            .absolute()
            .inset_0()
            .px(px(PAD - 8.))
            .py(px(PAD - 8.))
            .flex()
            .flex_col()
            .overflow_y_scroll();
        list = list.child(
            div()
                .flex_none()
                .px(px(8.))
                .pb(px(10.))
                .text_size(ui::ts(T_SECTION))
                .text_color(rgb(GREEN_HI))
                .child("SESSIONS"),
        );
        for (i, m) in data.mates.iter().enumerate() {
            let here = m.id == data.pane.id;
            let go = m.id.clone();
            let lamp = match m.state {
                SessionState::Working => GREEN,
                SessionState::NeedsYou => BLUE,
                SessionState::Error => 0xFF8A80,
                _ => FOOT,
            };
            list = list.child(
                div()
                    .id(eid(format!("mx-mate-{i}")))
                    .flex_none()
                    .min_h(px(48.))
                    .px(px(8.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .when(here, |d| d.bg(hca(GREEN, 36)))
                    .when(!here, |d| d.opacity(DIM + 0.1).hover(|s| s.opacity(1.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.screen = Screen::Session(go.clone());
                        cx.notify();
                    }))
                    .child(div().flex_none().size(px(8.)).bg(rgb(lamp)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(ui::ts(T_ENTRY))
                                    .text_color(rgb(if here { GREEN_HI } else { 0xB9C4BB }))
                                    .child(m.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(ui::ts(T_LABEL))
                                    .text_color(rgb(FOOT))
                                    .child(m.wait.clone()),
                            ),
                    ),
            );
        }
        div().flex_none().w(px(230.)).flex().flex_col().child(
            bezel(GREEN_MID)
                .flex_1()
                .min_h(px(0.))
                .child(screen(false, list))
                .child(foot("S", GREEN, vec![("switcher · J K".into(), FOOT)], 30.)),
        )
    }

    /// The console of the session: header, transcript, and the steer line or the decision.
    fn operator_console(
        &mut self,
        data: &Data,
        steer: gpui::Entity<InputState>,
        t: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let pane = &data.pane;
        let lines = display_lines(pane, 26);
        let seed = hash_str(&pane.id);
        let stream = match lines.split_last() {
            Some((newest, old)) if pane.state == SessionState::Working => {
                decoded(&pane.id, old, newest, seed, t)
            }
            _ => tokenize(&lines),
        };
        let part = if pane.part.is_empty() {
            "main"
        } else {
            pane.part.as_str()
        };
        let header = div()
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .top(px(0.))
            .h(px(44.))
            .px(px(PAD))
            .flex()
            .items_center()
            .justify_between()
            .bg(hca(0x010A03, 230))
            .text_size(ui::ts(T_ENTRY))
            .text_color(rgb(GREEN_MID))
            .child(
                div()
                    .text_size(ui::ts(T_SECTION))
                    .text_color(rgb(GREEN_HI))
                    .child(format!(
                        "{}@{}/{} · {}",
                        pane.provider,
                        pane.codebase,
                        part,
                        name(pane)
                    )),
            )
            .child(format!("{} · {} min", pane.model, pane.minutes(data.now)));
        let body = div()
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .top(px(44.))
            .bottom(px(84.))
            .px(px(PAD))
            .pt(px(8.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_size(ui::ts(T_ENTRY))
            .line_height(relative(1.5))
            .child(div().flex_none().pb(px(6.)).child(stream.into_text()));

        let mut bar = div()
            .absolute()
            .left(px(PAD - 8.))
            .right(px(PAD - 8.))
            .bottom(px(PAD - 8.))
            .h(px(48.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(hca(0x04170A, 240))
            .text_size(ui::ts(T_ENTRY));
        if let Some(ask) = &pane.ask {
            bar = bar.text_color(rgb(BLUE)).bg(hca(BLUE_DIM, 235));
            for (i, option) in ask.options.iter().take(4).enumerate() {
                let (session, ask_id, text) = (pane.id.clone(), ask.id.clone(), option.clone());
                bar = bar.child(
                    div()
                        .id(eid(format!("mx-s-opt-{i}")))
                        .h(px(32.))
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .bg(hca(BLUE_RING, 90))
                        .hover(|h| h.bg(rgb(BLUE)).text_color(rgb(BLUE_INK)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(
                                &this.model,
                                Action::Answer {
                                    id: session.clone(),
                                    ask: ask_id.clone(),
                                    option: Some(text.clone()),
                                    text: None,
                                },
                                cx,
                            );
                        }))
                        .child(format!("{} {}", i + 1, option.to_uppercase())),
                );
            }
        } else {
            bar = bar
                .text_color(rgb(GREEN_MID))
                .child("steer>")
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(rgb(GREEN_HI))
                        .child(steer),
                )
                .child(
                    div()
                        .text_size(ui::ts(T_LABEL))
                        .text_color(rgb(GREEN_DIM))
                        .child("↵ send · J K switch · esc wall"),
                );
        }
        let state = match pane.state {
            SessionState::Working => "busy · text flows",
            SessionState::NeedsYou => "waits for you",
            SessionState::Error => "error",
            SessionState::Idle => "idle",
            _ => "stopped",
        };
        bezel(GREEN_MID)
            .flex_1()
            .min_w(px(0.))
            .child(
                glass(false)
                    .child(header)
                    .child(body)
                    .child(bar)
                    .child(glaze(false)),
            )
            .child(foot(
                &(data
                    .mates
                    .iter()
                    .position(|m| m.id == pane.id)
                    .map_or(0, |i| i + 1))
                .to_string(),
                GREEN,
                vec![(name(pane), GREEN_HI), (format!("· {state}"), FOOT)],
                30.,
            ))
    }
}

/// A section title on a blue screen.
fn section(text: &str) -> Div {
    div()
        .text_size(ui::ts(T_SECTION))
        .text_color(rgb(BLUE_HI))
        .child(text.to_uppercase())
}

/// A blue monitor of the facts column: dim until the pointer comes.
fn fact_monitor(id: &'static str, badge: &str, foot_text: &str, body: Div) -> gpui::Stateful<Div> {
    dimmed(id.into()).child(
        bezel(BLUE_RING)
            .flex_1()
            .min_h(px(0.))
            .child(screen(true, body))
            .child(foot(badge, FOOT, vec![(foot_text.into(), FOOT)], 30.)),
    )
}

/// The blue column: context, turns, files, helpers.
fn operator_facts(data: &Data, t: f32) -> Div {
    let pane = &data.pane;
    let grow = ease(t / 0.9);
    let full = || {
        div()
            .absolute()
            .inset_0()
            .px(px(PAD))
            .py(px(PAD - 8.))
            .overflow_hidden()
    };

    // Context: a ring dial with the percent in its middle.
    let dial_box = div()
        .relative()
        .flex_none()
        .size(px(120.))
        .child(dial(f32::from(pane.context_pct.min(100)) / 100. * grow))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(ui::ts(T_TITLE))
                .text_color(rgb(BLUE_HI))
                .child(format!("{}%", pane.context_pct)),
        );
    let tokens = if pane.tokens >= 1000 {
        format!("{}k", pane.tokens / 1000)
    } else {
        pane.tokens.to_string()
    };
    let fact = |k: &str, v: String| {
        div()
            .flex()
            .justify_between()
            .child(k.to_string())
            .child(div().text_color(rgb(BLUE_HI)).child(v))
    };
    let context = fact_monitor(
        "mx-f-context",
        "A",
        "context",
        full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .text_size(ui::ts(T_ENTRY))
            .child(section("context"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(18.))
                    .child(dial_box)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(fact("tokens", tokens))
                            .child(fact("cost", format!("${:.2}", pane.cost)))
                            .child(fact("lines", pane.lines_total.to_string())),
                    ),
            ),
    )
    .flex_none()
    .h(px(220.));

    // Turns: the lines of each of the last four turns. A turn starts at a turn mark or at a
    // prompt of the owner that follows another kind of line.
    let mut turns: Vec<u32> = Vec::new();
    let mut prev_user = false;
    for l in &pane.tail {
        let user = l.kind == LineKind::User;
        if l.kind == LineKind::Turn || (user && !prev_user) || turns.is_empty() {
            turns.push(0);
        }
        if l.kind != LineKind::Turn
            && let Some(last) = turns.last_mut()
        {
            *last += 1;
        }
        prev_user = user;
    }
    let skip = turns.len().saturating_sub(4);
    let turns: Vec<u32> = turns.into_iter().skip(skip).collect();
    let peak = turns.iter().copied().max().unwrap_or(0);
    let n = turns.len();
    let have = n >= 1 && peak > 0;
    let mut chart = div()
        .absolute()
        .left(px(PAD))
        .right(px(PAD))
        .top(px(62.))
        .bottom(px(34.));
    if have {
        chart = chart.flex().items_end().gap(px(14.));
        for (k, v) in turns.iter().enumerate() {
            let h = 96. * *v as f32 / peak as f32 * ease((t - k as f32 * 0.1) / 0.8);
            chart = chart.child(div().flex_1().h(px(h.max(2.))).bg(rgb(if k + 1 == n {
                BLUE
            } else {
                BLUE_RING
            })));
        }
    } else {
        chart = chart.text_color(rgb(BLUE_SOFT)).child("no turns yet");
    }
    let turns_mon = fact_monitor(
        "mx-f-turns",
        "B",
        "turns",
        full().child(section("lines per turn")).child(chart).child(
            div()
                .absolute()
                .left(px(PAD))
                .right(px(PAD))
                .bottom(px(10.))
                .flex()
                .justify_around()
                .text_size(ui::ts(T_LABEL))
                .children((0..if have { n } else { 0 }).map(|k| {
                    if k + 1 == n {
                        format!("T{} now", k + 1)
                    } else {
                        format!("T{}", k + 1)
                    }
                })),
        ),
    )
    .flex_none()
    .h(px(210.));

    // Files: the files of the edit tools with their added and removed lines.
    let mut files: Vec<(String, u32, u32)> = Vec::new();
    let mut current: Option<usize> = None;
    for l in &pane.tail {
        match l.kind {
            LineKind::Tool
                if matches!(
                    l.label.as_deref(),
                    Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
                ) =>
            {
                let path = l.text.split_whitespace().next().unwrap_or("");
                let short = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
                current = Some(match files.iter().position(|f| f.0 == short) {
                    Some(i) => i,
                    None => {
                        files.push((short, 0, 0));
                        files.len() - 1
                    }
                });
            }
            LineKind::Added => {
                if let Some(i) = current {
                    files[i].1 += 1;
                }
            }
            LineKind::Removed => {
                if let Some(i) = current {
                    files[i].2 += 1;
                }
            }
            _ => {}
        }
    }
    let most = files.iter().map(|f| f.1.max(f.2)).max().unwrap_or(1).max(1);
    let mut list = div()
        .mt(px(10.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .text_size(ui::ts(T_ENTRY));
    for (path, plus, minus) in files.iter().take(3) {
        let w = |n: u32| px(48. * n as f32 / most as f32 * grow);
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(path.clone()),
                )
                .child(
                    div()
                        .w(px(48.))
                        .flex()
                        .justify_end()
                        .child(div().h(px(8.)).w(w(*minus)).bg(rgb(BLUE_SOFT))),
                )
                .child(
                    div()
                        .w(px(48.))
                        .child(div().h(px(8.)).w(w(*plus)).bg(rgb(BLUE_HI))),
                ),
        );
    }
    if files.is_empty() {
        list = list.child(div().text_color(rgb(BLUE_SOFT)).child("no edits yet"));
    }
    let files_mon = fact_monitor(
        "mx-f-files",
        "C",
        "diff",
        full()
            .child(section(&format!("files changed · {}", files.len())))
            .child(list),
    )
    .flex_none()
    .h(px(150.));

    // Helpers: the subagents with their state. The state is the only real value, so a lamp
    // and a word show it; nothing moves.
    let mut subs = div()
        .mt(px(8.))
        .flex()
        .flex_col()
        .text_size(ui::ts(T_ENTRY));
    for u in pane.subagents.iter().take(3) {
        let (word, color) = match u.state {
            SessionState::Working => ("working", GREEN),
            SessionState::NeedsYou => ("waits", BLUE_HI),
            SessionState::Error => ("error", BLUE_HI),
            SessionState::Idle => ("idle", BLUE_SOFT),
            _ => ("done", BLUE_SOFT),
        };
        subs = subs.child(
            div()
                .h(px(30.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_none().size(px(8.)).bg(rgb(color)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(u.name.clone()),
                )
                .child(div().text_color(rgb(color)).child(word)),
        );
    }
    if pane.subagents.is_empty() {
        subs = subs.child(div().text_color(rgb(BLUE_SOFT)).child("no helpers"));
    }
    let subs_mon = fact_monitor(
        "mx-f-subs",
        "D",
        "helpers",
        full()
            .child(section(&format!("subagents · {}", pane.subagents.len())))
            .child(subs),
    )
    .flex_1()
    .min_h(px(0.));

    div()
        .flex_none()
        .w(px(360.))
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(context)
        .child(turns_mon)
        .child(files_mon)
        .child(subs_mon)
}
