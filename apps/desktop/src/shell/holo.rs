//! The app frame in the Holo look (ADR 0022, boards `HoloBloom` and `HoloSession`): one bar on
//! top with the logo, the five screens, the live counts and the look switch. It replaces the
//! rail and the status bar. Every screen sits on the same page: the deep blue glow, the grid,
//! the slow far rings and the chamfered outer frame. The war room and the session view paint
//! their own glass; the list screens, Focus and Lens come from `holo_screens`, `holo_focus`
//! and `holo_lens`.

use std::time::Instant;

use aivory_protocol::Look;
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, Entity, SharedString, Subscription, Window, div, px, rgb};

use super::{Screen, Shell};
use crate::actions::{Action, act};
use crate::holo_focus::{HoloFocus, HoloFocusEvent};
use crate::holo_kit::*;
use crate::holo_lens::{HoloLens, HoloLensEvent};
use crate::holo_screens::{HoloEvent, HoloInbox, HoloRepos, HoloSettings, HoloTasks};
use crate::look::LOOKS;
use crate::theme::tk;

/// The Holo screens and their event subscriptions: made when they first show, then kept.
#[derive(Default)]
pub(super) struct HoloScreens {
    inbox: Option<(Entity<HoloInbox>, Subscription)>,
    tasks: Option<(Entity<HoloTasks>, Subscription)>,
    repos: Option<(Entity<HoloRepos>, Subscription)>,
    settings: Option<(Entity<HoloSettings>, Subscription)>,
    focus: Option<(Entity<HoloFocus>, Subscription)>,
    lens: Option<(Entity<HoloLens>, Subscription)>,
    /// The codebase and file that the Holo Lens shows next.
    pub(super) lens_target: Option<(String, String, Option<String>)>,
    /// The Holo Settings screen hands over to the full Settings screen.
    full_settings: bool,
    /// The start of the slow rings.
    started: Option<Instant>,
    /// The open effect of a new screen.
    fx: super::open_fx::OpenFx,
    /// The step of `AIVORY_HOLO_CYCLE`.
    cycle: usize,
}

impl Shell {
    /// `AIVORY_HOLO_CYCLE=1`: the screen changes every 2.5 s (off-screen shots of the open
    /// effect). It runs before the content of the frame is made.
    pub(super) fn holo_cycle(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("AIVORY_HOLO_CYCLE").is_none() {
            return;
        }
        let now = Instant::now();
        let t = (now - *self.holo_screens.started.get_or_insert(now)).as_secs_f32();
        let step = (t / 2.5) as usize;
        if step != self.holo_screens.cycle {
            self.holo_screens.cycle = step;
            self.screen = [Screen::WarRoom, Screen::Inbox, Screen::Repos][step % 3].clone();
            cx.notify();
        }
    }

    /// True when the Holo look is on.
    pub(super) fn holo_on() -> bool {
        tk().look == Look::Holo
    }

    /// A nav entry of the bar: dim, bright with a cyan underline when on.
    fn holo_entry(
        &self,
        id: &'static str,
        label: &str,
        screen: Screen,
        badge: u32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let on = self.screen.rail() == screen;
        let mut el = div()
            .id(id)
            .h(px(30.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .font_family(LABEL)
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_size(px(15.))
            .cursor_pointer()
            .border_b_2()
            .border_color(ink(PK, if on { 1. } else { 0. }))
            .text_color(if on {
                gpui::Hsla::from(rgb(TXT))
            } else {
                ink(MUTED, 0.8)
            })
            .hover(|s| s.text_color(rgb(TXT)))
            .child(label.to_uppercase());
        if badge > 0 {
            el = el.child(div().text_color(rgb(OR)).child(badge.to_string()));
        }
        el.on_click(cx.listener(move |this, _, _, cx| {
            this.look_menu = false;
            this.holo_screens.full_settings = false;
            if screen == Screen::WarRoom {
                this.war_room.update(cx, |w, cx| w.holo_close(cx));
            }
            this.go(screen.clone(), cx);
        }))
    }

    /// The bar: logo at the left, the screens in the middle, the live counts and the look switch
    /// at the right.
    fn holo_bar(&self, cx: &mut Context<Self>) -> Div {
        let counts = self
            .model
            .read(cx)
            .war
            .as_ref()
            .map(|w| (w.counts(), w.inbox_open));
        let (c, inbox) = counts.unwrap_or_default();
        let count = |n: u32, word: &str, color: u32| {
            div()
                .flex_none()
                .font_family(LABEL)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_size(px(S_SMALL))
                .text_color(rgb(color))
                .whitespace_nowrap()
                .child(format!("{n} {word}"))
        };
        let look_name = crate::look::name(tk().look);
        div()
            .flex_none()
            .h(px(BAR_H))
            .px(px(MARGIN))
            .pt(px(20.))
            .flex()
            .items_start()
            .font_family(LABEL)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(mark())
                    .child(
                        div()
                            .font_family(LABEL)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(S_BIG))
                            .text_color(rgb(TXT))
                            .child("AIVORY"),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(30.))
                    .child(self.holo_entry("holo-war", "War room", Screen::WarRoom, 0, cx))
                    .child(self.holo_entry("holo-inbox", "Inbox", Screen::Inbox, inbox, cx))
                    .child(self.holo_entry("holo-tasks", "Tasks", Screen::Tasks, 0, cx))
                    .child(self.holo_entry("holo-repos", "Repos", Screen::Repos, 0, cx))
                    .child(self.holo_entry("holo-set", "Settings", Screen::Settings, 0, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(22.))
                    .child(count(c.working, "working", MUTED))
                    .child(count(
                        c.needs_you + c.error,
                        "wait for you",
                        if c.needs_you + c.error > 0 { GD } else { MUTED },
                    ))
                    .child(count(c.idle, "idle", GY))
                    .child(
                        div()
                            .id("holo-look")
                            .flex_none()
                            .h(px(30.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .font_family(LABEL)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(S_SMALL + 1.))
                            .text_color(ink(MUTED, 0.9))
                            .border_1()
                            .border_color(ink(PK, 0.3))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(TXT)).border_color(ink(PK, 0.7)))
                            .child(format!("LOOK: {} ▾", look_name.to_uppercase()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.look_menu = !this.look_menu;
                                cx.notify();
                            })),
                    ),
            )
    }

    /// The list of looks under the bar.
    fn holo_look_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.look_menu {
            return None;
        }
        let mut list = div()
            .absolute()
            .top(px(BAR_H - 14.))
            .right(px(MARGIN))
            .w(px(200.))
            .occlude()
            .flex()
            .flex_col()
            .p(px(6.))
            .bg(ink(0x06182A, 0.97))
            .border_1()
            .border_color(ink(PK, 0.4))
            .font_family(LABEL);
        for (i, (look, name)) in LOOKS.iter().enumerate() {
            let look = *look;
            let on = look == tk().look;
            list = list.child(
                div()
                    .id(SharedString::from(format!("holo-look-{i}")))
                    .h(px(34.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_size(px(S_SMALL + 2.))
                    .text_color(if on { rgb(PK) } else { rgb(DIM) })
                    .hover(|s| s.bg(ink(PK, 0.12)))
                    .child(name.to_uppercase())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.look_menu = false;
                        act(&this.model, Action::SetLook(look), cx);
                        cx.notify();
                    })),
            );
        }
        Some(list)
    }

    /// The window in the Holo look: the page, the bar and the screen.
    pub(super) fn holo_screen(
        &mut self,
        content: AnyElement,
        toasts: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let now = Instant::now();
        let t = (now - *self.holo_screens.started.get_or_insert(now)).as_secs_f32();
        crate::ui::animate(window, cx);
        let bar = self.holo_bar(cx);
        let menu = self.holo_look_menu(cx);
        // A new screen, or a session view in the war room, opens with the pen's effect.
        let open = self.war_room.read(cx).hud_open_id();
        let key = format!("{:?}|{:?}", self.screen, open);
        let mut fx = std::mem::take(&mut self.holo_screens.fx);
        fx.screen(key, window, cx, |s: &mut Shell| &mut s.holo_screens.fx);
        let area = fx.wrap(content, window);
        self.holo_screens.fx = fx;
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(rgb(PAGE))
            .font_family(BODY)
            .text_size(px(S_BODY))
            .text_color(rgb(TXT))
            .child(backdrop(t))
            .child(bar)
            .child(area)
            .children(menu)
            .children(toasts)
    }

    /// The Holo screens in place of the Minimal ones. The war room and the session view are
    /// not here: the war room screen draws them. A screen is made when it first shows.
    pub(super) fn holo_content(
        &mut self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        fn handle(this: &mut Shell, event: &HoloEvent, cx: &mut Context<Shell>) {
            match event {
                HoloEvent::OpenSession(id) => this.open_session(id, cx),
                HoloEvent::AllSettings => {
                    this.holo_screens.full_settings = true;
                    cx.notify();
                }
            }
        }
        let model = self.model.clone();
        let mut content = Some(content);
        let made: Option<AnyElement> =
            match self.screen.clone() {
                Screen::Focus(id) => {
                    if let Some((v, _)) = &self.holo_screens.focus {
                        v.update(cx, |v, cx| v.show(&id, cx));
                    } else {
                        let v = cx.new(|cx| HoloFocus::new(model, id, window, cx));
                        let sub = cx.subscribe(&v, |t, _, e: &HoloFocusEvent, cx| match e {
                            HoloFocusEvent::Open(id) => t.open_focus(id, cx),
                            HoloFocusEvent::Detail(id) => t.open_session(id, cx),
                            HoloFocusEvent::OpenFile { session, path } => {
                                t.open_session_file(session, path, cx)
                            }
                        });
                        self.holo_screens.focus = Some((v, sub));
                    }
                    self.holo_screens
                        .focus
                        .as_ref()
                        .map(|(v, _)| v.clone().into_any_element())
                }
                Screen::Lens => {
                    if self.holo_screens.lens.is_none() {
                        let v = cx.new(|cx| HoloLens::new(model, window, cx));
                        let sub = cx.subscribe(&v, |t, _, e: &HoloLensEvent, cx| match e {
                            HoloLensEvent::Back => {
                                let back = std::mem::replace(&mut t.lens_back, Screen::Repos);
                                t.go(back, cx);
                            }
                            HoloLensEvent::OpenSession(id) => t.open_session(id, cx),
                        });
                        self.holo_screens.lens = Some((v, sub));
                    }
                    let target = self.holo_screens.lens_target.take();
                    if let (Some((v, _)), Some((codebase, path, session))) =
                        (&self.holo_screens.lens, target)
                    {
                        v.update(cx, |v, cx| v.show(&codebase, &path, session, cx));
                    }
                    self.holo_screens
                        .lens
                        .as_ref()
                        .map(|(v, _)| v.clone().into_any_element())
                }
                Screen::Inbox => {
                    if self.holo_screens.inbox.is_none() {
                        let v = cx.new(|cx| HoloInbox::new(model, cx));
                        let sub = cx.subscribe(&v, |t, _, e: &HoloEvent, cx| handle(t, e, cx));
                        self.holo_screens.inbox = Some((v, sub));
                    }
                    self.holo_screens
                        .inbox
                        .as_ref()
                        .map(|(v, _)| v.clone().into_any_element())
                }
                Screen::Tasks => {
                    if self.holo_screens.tasks.is_none() {
                        let v = cx.new(|cx| HoloTasks::new(model, cx));
                        let sub = cx.subscribe(&v, |t, _, e: &HoloEvent, cx| handle(t, e, cx));
                        self.holo_screens.tasks = Some((v, sub));
                    }
                    self.holo_screens
                        .tasks
                        .as_ref()
                        .map(|(v, _)| v.clone().into_any_element())
                }
                Screen::Repos => {
                    if self.holo_screens.repos.is_none() {
                        let v = cx.new(|cx| HoloRepos::new(model, cx));
                        let sub = cx.subscribe(&v, |t, _, e: &HoloEvent, cx| handle(t, e, cx));
                        self.holo_screens.repos = Some((v, sub));
                    }
                    self.holo_screens
                        .repos
                        .as_ref()
                        .map(|(v, _)| v.clone().into_any_element())
                }
                Screen::Settings if !self.holo_screens.full_settings => {
                    if self.holo_screens.settings.is_none() {
                        let v = cx.new(|cx| HoloSettings::new(model, cx));
                        let sub = cx.subscribe(&v, |t, _, e: &HoloEvent, cx| handle(t, e, cx));
                        self.holo_screens.settings = Some((v, sub));
                    }
                    self.holo_screens
                        .settings
                        .as_ref()
                        .map(|(v, _)| v.clone().into_any_element())
                }
                Screen::Settings => Some(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .px(px(MARGIN))
                        .pb(px(32.))
                        .gap(px(12.))
                        .child(link("holo-settings-back", "‹ Holo settings", PK).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.holo_screens.full_settings = false;
                                cx.notify();
                            }),
                        ))
                        .child(div().flex_1().min_h(px(0.)).children(content.take()))
                        .into_any_element(),
                ),
                _ => None,
            };
        made.or(content).unwrap_or_else(|| div().into_any_element())
    }
}
