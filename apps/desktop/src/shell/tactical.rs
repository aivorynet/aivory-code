//! The app frame of the Tactical look (ADR 0022): one bar on top in place of the rail and the
//! status bar (board `TacticalRadar`). The bar holds War room, Inbox, Tasks, Repos, Setup and
//! the look switch, with the counts of the fleet and the daemon clock. The list screens are the
//! Tactical views of `tactical_screens.rs`; "ALL FUNCTIONS" shows the full screen inside the
//! Tactical frame. Focus and Lens sit in the frame too.

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, SharedString, Subscription, Window, div, px,
};

use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::look::LOOKS;
use crate::tactical_focus::{TacFocusEvent, TacticalFocus};
use crate::tactical_kit::{self as tac, Frame, c, frame, navb};
use crate::tactical_lens::{TacLensEvent, TacticalLens};
use crate::tactical_screens::{
    TacEvent, TacticalInbox, TacticalRepos, TacticalSettings, TacticalTasks,
};
use crate::theme::tk;
use crate::ui;

/// The height of the bar.
const BAR_H: f32 = 44.;

/// The Tactical list screens and their event subscriptions.
#[derive(Default)]
pub(super) struct TacticalScreens {
    inbox: Option<(Entity<TacticalInbox>, Subscription)>,
    tasks: Option<(Entity<TacticalTasks>, Subscription)>,
    repos: Option<(Entity<TacticalRepos>, Subscription)>,
    settings: Option<(Entity<TacticalSettings>, Subscription)>,
    focus: Option<(Entity<TacticalFocus>, Subscription)>,
    lens: Option<(Entity<TacticalLens>, Subscription)>,
    /// The codebase, file and session that the Lens shows next.
    pub(super) lens_target: Option<(String, String, Option<String>)>,
    /// The full screen of the list screen on view is on.
    pub(super) full: bool,
}

fn handle(this: &mut Shell, event: &TacEvent, cx: &mut Context<Shell>) {
    match event {
        TacEvent::OpenSession(id) => this.open_session(id, cx),
        TacEvent::Full => {
            this.tactical_screens.full = true;
            cx.notify();
        }
    }
}

impl Shell {
    /// The navigation bar of the Tactical look.
    pub(super) fn tactical_bar(&self, cx: &mut Context<Self>) -> Div {
        let war = self.model.read(cx).war.as_ref();
        let (live, waits, inbox, auto, now) = war.map_or((0, 0, 0, false, 0), |w| {
            let n = w.counts();
            (
                n.live,
                n.needs_you + n.error,
                w.inbox_open,
                w.autopilot.on,
                w.now,
            )
        });
        let here = self.screen.rail();
        let entry = |id: &'static str, name: String, screen: Screen, cx: &mut Context<Self>| {
            navb(id, &name, here == screen).on_click(cx.listener(move |this, _, _, cx| {
                this.look_menu = false;
                this.go(screen.clone(), cx);
            }))
        };
        div()
            .flex_none()
            .h(px(BAR_H))
            .mx(px(20.))
            .flex()
            .items_center()
            .gap(px(22.))
            .border_b_1()
            .border_color(c(tac::DEEP, 1.))
            .font_family(tac::MONO)
            .child(
                div()
                    .font_family(tac::SANS)
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(15.))
                    .text_color(c(tac::INK, 1.))
                    .child("AIVORY // TACTICAL"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .child(entry("tac-war", "OPS".into(), Screen::WarRoom, cx))
                    .child(entry(
                        "tac-inbox",
                        if inbox > 0 {
                            format!("INBOX ({inbox})")
                        } else {
                            "INBOX".into()
                        },
                        Screen::Inbox,
                        cx,
                    ))
                    .child(entry("tac-tasks", "TASKS".into(), Screen::Tasks, cx))
                    .child(entry("tac-repos", "REPOS".into(), Screen::Repos, cx))
                    .child(entry("tac-set", "SETUP".into(), Screen::Settings, cx)),
            )
            .child(div().flex_1())
            .child(tac::label(format!("{live} CONTACTS")))
            .child(
                tac::label(format!("{waits} AWAIT ORDERS"))
                    .text_color(c(if waits > 0 { tac::WHITE } else { tac::LABEL }, 1.)),
            )
            .child(tac::label(format!(
                "AUTOPILOT: {}",
                if auto { "ON" } else { "OFF" }
            )))
            .child(
                div()
                    .text_size(ui::ts(14.))
                    .text_color(c(tac::INK, 1.))
                    .child(tac::utc(now)),
            )
            .child(
                navb(
                    "tac-look",
                    &format!("LOOK: {} v", crate::look::name(tk().look).to_uppercase()),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.look_menu = !this.look_menu;
                    cx.notify();
                })),
            )
    }

    /// The list of looks under the bar.
    pub(super) fn tactical_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.look_menu {
            return None;
        }
        let here = tk().look;
        let mut list = div()
            .absolute()
            .top(px(BAR_H))
            .right(px(20.))
            .w(px(200.))
            .occlude()
            .flex()
            .flex_col()
            .p(px(6.))
            .gap(px(2.))
            .bg(c(tac::PANEL, 0.98))
            .border_1()
            .border_color(c(tac::DIM, 1.))
            .font_family(tac::MONO);
        for (i, (look, name)) in LOOKS.iter().enumerate() {
            let look = *look;
            list = list.child(
                navb(
                    SharedString::from(format!("tac-look-{i}")),
                    &name.to_uppercase(),
                    look == here,
                )
                .h(px(32.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.look_menu = false;
                    act(&this.model, Action::SetLook(look), cx);
                    cx.notify();
                })),
            );
        }
        Some(list)
    }

    /// The Tactical views replace Inbox, Tasks, Repos and Settings; the full screens, Focus
    /// and Lens keep their own content inside the Tactical frame.
    pub(super) fn tactical_content(
        &mut self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.model.clone();
        match self.screen.clone() {
            Screen::Inbox if !self.tactical_screens.full => {
                if self.tactical_screens.inbox.is_none() {
                    let v = cx.new(|cx| TacticalInbox::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &TacEvent, cx| handle(t, e, cx));
                    self.tactical_screens.inbox = Some((v, sub));
                }
                self.tactical_screens
                    .inbox
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Tasks if !self.tactical_screens.full => {
                if self.tactical_screens.tasks.is_none() {
                    let v = cx.new(|cx| TacticalTasks::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &TacEvent, cx| handle(t, e, cx));
                    self.tactical_screens.tasks = Some((v, sub));
                }
                self.tactical_screens
                    .tasks
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Repos if !self.tactical_screens.full => {
                if self.tactical_screens.repos.is_none() {
                    let v = cx.new(|cx| TacticalRepos::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &TacEvent, cx| handle(t, e, cx));
                    self.tactical_screens.repos = Some((v, sub));
                }
                self.tactical_screens
                    .repos
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Settings if !self.tactical_screens.full => {
                if self.tactical_screens.settings.is_none() {
                    let v = cx.new(|cx| TacticalSettings::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &TacEvent, cx| handle(t, e, cx));
                    self.tactical_screens.settings = Some((v, sub));
                }
                self.tactical_screens
                    .settings
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Inbox => self.tactical_full("INBOX", "ALL FUNCTIONS OF THE INBOX", content, cx),
            Screen::Tasks => self.tactical_full("TASKS", "ALL FUNCTIONS OF THE TASKS", content, cx),
            Screen::Repos => self.tactical_full("REPOS", "ALL FUNCTIONS OF THE REPOS", content, cx),
            Screen::Settings => self.tactical_full("SETUP", "ALL SETTINGS", content, cx),
            Screen::Focus(id) => {
                if let Some((v, _)) = &self.tactical_screens.focus {
                    v.update(cx, |v, cx| v.show(&id, cx));
                } else {
                    let v = cx.new(|cx| TacticalFocus::new(model, id, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &TacFocusEvent, cx| match e {
                        TacFocusEvent::Back => t.go(Screen::WarRoom, cx),
                        TacFocusEvent::Open(id) => t.open_focus(id, cx),
                        TacFocusEvent::Dossier(id) => t.open_session(id, cx),
                        TacFocusEvent::OpenFile { session, path } => {
                            t.open_session_file(session, path, cx)
                        }
                    });
                    self.tactical_screens.focus = Some((v, sub));
                }
                self.tactical_screens
                    .focus
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Lens => {
                if self.tactical_screens.lens.is_none() {
                    let v = cx.new(|cx| TacticalLens::new(model, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &TacLensEvent, cx| match e {
                        TacLensEvent::Back => {
                            let back = std::mem::replace(&mut t.lens_back, Screen::Repos);
                            t.go(back, cx);
                        }
                        TacLensEvent::OpenSession(id) => t.open_session(id, cx),
                    });
                    self.tactical_screens.lens = Some((v, sub));
                }
                let target = self.tactical_screens.lens_target.take();
                if let (Some((v, _)), Some((codebase, path, session))) =
                    (&self.tactical_screens.lens, target)
                {
                    v.update(cx, |v, cx| v.show(&codebase, &path, session, cx));
                }
                self.tactical_screens
                    .lens
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            _ => content,
        }
    }

    /// A full screen of a list view in the Tactical frame, with a way back to the view.
    fn tactical_full(
        &mut self,
        title: &'static str,
        kicker: &str,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu = vec![
            navb("tac-full-back", "< TACTICAL VIEW", false)
                .border_1()
                .border_color(c(tac::INK, 1.))
                .text_color(c(tac::INK, 1.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tactical_screens.full = false;
                    cx.notify();
                }))
                .into_any_element(),
        ];
        Self::tactical_wrap(title, kicker, menu, content)
    }

    /// A screen with its own content under a Tactical title and menu.
    fn tactical_wrap(
        title: &'static str,
        kicker: &str,
        menu: Vec<AnyElement>,
        content: AnyElement,
    ) -> AnyElement {
        frame(Frame {
            title,
            kicker: kicker.to_string(),
            stats: Vec::new(),
            menu,
            body: div()
                .size_full()
                .relative()
                .overflow_hidden()
                .border_1()
                .border_color(c(tac::DIM, 1.))
                .child(content)
                .into_any_element(),
        })
        .into_any_element()
    }
}
