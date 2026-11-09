//! The desktop shell of every screen: rainbow strip, left rail, 60 px header, 30 px status bar.

use std::time::Duration;

use aivory_protocol::{LinkState, UsageWindow, WarRoomCounts};
use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, Context, DispatchPhase, Entity, FontWeight, KeyDownEvent, SharedString,
    Subscription, Task, Window, anchored, canvas, deferred, div, point, px, rgb, rgba, svg,
};

use crate::bridge_screens::{BridgeInbox, BridgeRepos, BridgeSettings, BridgeTasks};
use crate::focus::{FocusEvent, FocusScreen};
use crate::inbox::{InboxEvent, InboxScreen};
use crate::lens::{LensEvent, LensScreen};
use crate::model::{Link, Model};
use crate::repos::{ReposEvent, ReposScreen};
use crate::session::{SessionEvent, SessionScreen};
use crate::settings::SettingsScreen;
use crate::tasks::{TasksEvent, TasksScreen};
use crate::theme::*;
use crate::ui;
use crate::warroom::{BridgeDetail, BridgeDetailEvent, WarRoomEvent, WarRoomScreen};

mod bridge;
mod command;
mod holo;
mod hud;
mod irix;
mod irix_focus;
mod irix_screens;
mod neon;
mod neon_focus;
mod neon_kit;
mod neon_lens;
mod neon_screens;
mod neon_session;
mod open_fx;
mod operator;
mod operator_focus;
mod operator_lens;
mod operator_screens;
mod tactical;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    WarRoom,
    Session(String),
    /// The Focus view of one session: chat and code.
    Focus(String),
    Inbox,
    Tasks,
    Repos,
    Settings,
    /// The code lens (opens from Repos and from the deep dive).
    Lens,
}

impl Screen {
    fn label(&self) -> &'static str {
        match self {
            Screen::WarRoom => "War room",
            Screen::Session(_) => "Session",
            Screen::Focus(_) => "Focus",
            Screen::Inbox => "Inbox",
            Screen::Tasks => "Tasks",
            Screen::Repos => "Repos",
            Screen::Settings => "Settings",
            Screen::Lens => "Lens",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Screen::WarRoom | Screen::Session(_) | Screen::Focus(_) => "icons/warroom.svg",
            Screen::Inbox => "icons/inbox.svg",
            Screen::Tasks => "icons/tasks.svg",
            Screen::Repos | Screen::Lens => "icons/repos.svg",
            Screen::Settings => "icons/settings.svg",
        }
    }

    /// The rail item that is on for this screen.
    fn rail(&self) -> Screen {
        match self {
            Screen::Session(_) | Screen::Focus(_) => Screen::WarRoom,
            Screen::Lens => Screen::Repos,
            other => other.clone(),
        }
    }
}

/// The Bridge list screens and their event subscriptions.
#[derive(Default)]
struct BridgeScreens {
    inbox: Option<(Entity<BridgeInbox>, Subscription)>,
    tasks: Option<(Entity<BridgeTasks>, Subscription)>,
    repos: Option<(Entity<BridgeRepos>, Subscription)>,
    settings: Option<(Entity<BridgeSettings>, Subscription)>,
    focus: Option<(Entity<crate::bridge_focus::BridgeFocus>, Subscription)>,
    lens: Option<(Entity<crate::bridge_lens::BridgeLens>, Subscription)>,
    /// The codebase and file that the Bridge Lens shows next.
    lens_target: Option<(String, String, Option<String>)>,
}

pub struct Shell {
    screen: Screen,
    /// The window was maximized or in full screen at the last frame. A change to maximized
    /// opens the full screen when the Display setting asks for it.
    max_or_full: bool,
    /// Redraws the shell when the F11 hint ends.
    hint_timer: Option<Task<()>>,
    /// Where Back of the deep dive goes: the screen that opened it.
    back: Screen,
    model: Entity<Model>,
    war_room: Entity<WarRoomScreen>,
    inbox: Entity<InboxScreen>,
    tasks: Entity<TasksScreen>,
    repos: Entity<ReposScreen>,
    settings: Entity<SettingsScreen>,
    /// The deep dive of the session on screen: its id, the view, and the event subscription.
    session: Option<(String, Entity<SessionScreen>, Subscription)>,
    /// The Bridge session view: its id, the view, and the event subscription.
    bridge_detail: Option<(String, Entity<BridgeDetail>, Subscription)>,
    /// The Neon session view: its id, the view, and the event subscription.
    night_detail: Option<(String, Entity<neon_session::NightSession>, Subscription)>,
    /// The Bridge screens: made when they first show, then kept.
    bridge_screens: BridgeScreens,
    /// The Holo list screens, Focus and Lens.
    holo_screens: holo::HoloScreens,
    /// The Tactical list screens.
    tactical_screens: tactical::TacticalScreens,
    /// The Command Inbox and the production queue of the sidebar.
    command: command::CommandScreens,
    /// The Neon list screens: made when they first show, then kept.
    night_screens: neon_screens::NightScreens,
    /// The Bridge Settings screen hands over to the full Settings screen.
    bridge_full_settings: bool,
    /// The Focus view: kept while it is on screen, so its editor state survives a session switch.
    focus: Option<(Entity<FocusScreen>, Subscription)>,
    /// The lens on screen, or behind a deep dive that it opened. Back returns to `lens_back`.
    lens: Option<(Entity<LensScreen>, Subscription)>,
    lens_back: Screen,
    _subscriptions: Vec<Subscription>,
    /// The look chips in the status bar are open.
    look_menu: bool,
    /// Navigator look: the screens whose window is minimized, and the open window menu.
    irix_min: std::collections::HashSet<String>,
    irix_menu: bool,
    /// Navigator look: the Tasks, Repos and Focus screens.
    irix_screens: irix_screens::IrixScreens,
    /// The Operator look: the start of its animation clock, the steer line of the session view.
    operator_t0: Option<std::time::Instant>,
    operator_steer: Option<Entity<gpui_base::input::InputState>>,
    operator_sub: Option<Subscription>,
    /// The Operator look: its list screens, and the full Settings screen inside its frame.
    operator_screens: operator_screens::OperatorScreens,
    operator_full_settings: bool,
    _clock: Task<()>,
    /// The New session dialog over every screen (key N), and a request to open it.
    new_dialog: Option<(Entity<crate::new_session::NewSessionDialog>, Subscription)>,
    new_pending: bool,
    /// The view of a custom theme layout (ADR 0024), made when a layout first shows.
    themed: Option<(Entity<crate::themed::host::ThemedView>, Subscription)>,
    /// The decision dialog of a theme is open for this session.
    themed_dialog: Option<String>,
}

impl Shell {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let war_room = cx.new(|cx| WarRoomScreen::new(model.clone(), cx));
        let inbox = cx.new(|cx| InboxScreen::new(model.clone(), cx));
        let tasks = cx.new(|cx| TasksScreen::new(model.clone(), cx));
        let repos = cx.new(|cx| ReposScreen::new(model.clone(), cx));
        let settings = cx.new(|cx| SettingsScreen::new(model.clone(), cx));
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&war_room, |this, _, event: &WarRoomEvent, cx| match event {
                WarRoomEvent::OpenSession(id) => this.open_session(id, cx),
                WarRoomEvent::NewSession => this.open_new_session(cx),
                WarRoomEvent::Escape => {
                    if this.screen == Screen::WarRoom {
                        this.war_room.update(cx, |w, cx| {
                            w.rts_close();
                            cx.notify();
                        });
                    } else {
                        this.command_escape(cx);
                    }
                }
                WarRoomEvent::Focus(id) => this.open_focus(id, cx),
                WarRoomEvent::OpenFile { session, path } => {
                    this.open_session_file(session, path, cx)
                }
            }),
            cx.subscribe(&tasks, |this, _, event: &TasksEvent, cx| {
                match event {
                    TasksEvent::OpenSession(id) => this.open_session(id, cx),
                    TasksEvent::OpenDiff(id) => this.open_session_diff(id, cx),
                    TasksEvent::OpenInbox(sessions) => {
                        this.screen = Screen::Inbox;
                        this.inbox.update(cx, |i, cx| i.show_sessions(sessions, cx));
                    }
                }
                cx.notify();
            }),
            cx.subscribe(&inbox, |this, _, event: &InboxEvent, cx| match event {
                InboxEvent::OpenSession(id) => this.open_session(id, cx),
                InboxEvent::OpenFile { session, path } => this.open_session_file(session, path, cx),
            }),
            cx.subscribe(&repos, |this, _, event: &ReposEvent, cx| match event {
                ReposEvent::ShowInWarRoom(id) => {
                    this.go(Screen::WarRoom, cx);
                    let id = id.clone();
                    this.war_room.update(cx, |w, cx| w.reveal_group(&id, cx));
                }
                ReposEvent::OpenFile { codebase, path } => this.open_lens(codebase, path, None, cx),
                ReposEvent::OpenSessions(ids) => this.open_first(ids, cx),
            }),
        ];
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(10))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            max_or_full: false,
            hint_timer: None,
            // `AIVORY_FOCUS=<session>` starts in the Focus view, `AIVORY_SCREEN=inbox|tasks|
            // repos|settings` on that screen (screenshot runs).
            screen: match (
                std::env::var("AIVORY_FOCUS")
                    .ok()
                    .filter(|id| !id.is_empty()),
                std::env::var("AIVORY_SCREEN").as_deref(),
            ) {
                (Some(id), _) => Screen::Focus(id),
                (None, _) if session_env().is_some() => {
                    Screen::Session(session_env().unwrap_or_default())
                }
                (None, Ok("inbox")) => Screen::Inbox,
                (None, Ok("tasks")) => Screen::Tasks,
                (None, Ok("repos")) => Screen::Repos,
                (None, Ok("settings")) => Screen::Settings,
                _ => Screen::WarRoom,
            },
            back: Screen::WarRoom,
            model,
            war_room,
            inbox,
            tasks,
            repos,
            settings,
            session: None,
            bridge_detail: None,
            night_detail: None,
            bridge_screens: BridgeScreens::default(),
            holo_screens: holo::HoloScreens::default(),
            tactical_screens: Default::default(),
            command: Default::default(),
            night_screens: Default::default(),
            bridge_full_settings: false,
            focus: None,
            lens: None,
            lens_back: Screen::Repos,
            _subscriptions: subscriptions,
            look_menu: false,
            irix_min: Default::default(),
            irix_screens: Default::default(),
            irix_menu: false,
            operator_t0: None,
            operator_steer: None,
            operator_sub: None,
            operator_screens: Default::default(),
            operator_full_settings: false,
            _clock: clock,
            new_dialog: None,
            new_pending: false,
            themed: None,
            themed_dialog: None,
        }
    }

    /// Open the Focus view of a session.
    fn open_focus(&mut self, id: &str, cx: &mut Context<Self>) {
        self.screen = Screen::Focus(id.to_string());
        cx.notify();
    }

    /// The Focus view of the session on screen.
    fn focus_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<FocusScreen> {
        let Screen::Focus(id) = &self.screen else {
            unreachable!("only the focus screen asks for a focus view");
        };
        if let Some((view, _)) = &self.focus {
            if view.read(cx).session() != id {
                let id = id.clone();
                view.update(cx, |v, cx| v.show(&id, cx));
            }
            return view.clone();
        }
        let view = cx.new(|cx| FocusScreen::new(self.model.clone(), id.clone(), window, cx));
        let subscription = cx.subscribe(&view, |this, _, event: &FocusEvent, cx| match event {
            FocusEvent::Back => this.go(Screen::WarRoom, cx),
            FocusEvent::Open(id) => this.open_focus(id, cx),
            FocusEvent::DeepDive(id) => this.open_session(id, cx),
            FocusEvent::OpenFile { session, path } => this.open_session_file(session, path, cx),
        });
        self.focus = Some((view.clone(), subscription));
        view
    }

    /// Open the deep dive; Back returns to the screen on view now.
    fn open_session(&mut self, id: &str, cx: &mut Context<Self>) {
        // Holo and Tactical show a session inside the war room screen (their own session view).
        if matches!(
            tk().look,
            aivory_protocol::Look::Holo | aivory_protocol::Look::Tactical
        ) {
            self.war_room.update(cx, |w, cx| w.hud_open(id, cx));
            self.go(Screen::WarRoom, cx);
            return;
        }
        if !matches!(self.screen, Screen::Session(_)) {
            self.back = self.screen.clone();
        }
        self.screen = Screen::Session(id.to_string());
        cx.notify();
    }

    /// Open the deep dive on its Diff tab.
    fn open_session_diff(&mut self, id: &str, cx: &mut Context<Self>) {
        self.open_session(id, cx);
        let view = self.session_screen(cx);
        view.update(cx, |s, cx| s.show_diff(cx));
    }

    /// "Open both" opens the deep dive of the first session; the shell shows one deep dive.
    fn open_first(&mut self, ids: &[String], cx: &mut Context<Self>) {
        if let Some(id) = ids.first() {
            self.open_session(id, cx);
        }
    }

    /// Show a file that a session names. The Lens takes a path under the codebase root.
    fn open_session_file(&mut self, session: &str, path: &str, cx: &mut Context<Self>) {
        let found = self.model.read(cx).war.as_ref().and_then(|w| {
            let pane = w.session(session)?;
            let root = w.codebase(&pane.codebase)?.path.clone();
            Some((pane.codebase.clone(), root))
        });
        if let Some((codebase, root)) = found {
            let path = relative_to(path, &root);
            self.open_lens(&codebase, &path, Some(session.to_string()), cx);
        }
    }

    /// Show a file in the Lens; Back returns to the screen on view now.
    fn open_lens(
        &mut self,
        codebase: &str,
        path: &str,
        from_session: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let back_label = match &self.screen {
            Screen::Session(_) => "Session",
            Screen::Lens => self.lens_back.label(),
            other => other.label(),
        };
        if self.screen != Screen::Lens {
            self.lens_back = self.screen.clone();
        }
        // The Holo look has its own Lens screen: it takes the target in `holo_content`.
        if Self::holo_on() {
            self.holo_screens.lens_target =
                Some((codebase.to_string(), path.to_string(), from_session));
            self.lens = None;
            self.screen = Screen::Lens;
            cx.notify();
            return;
        }
        // The Operator look has its own Lens screen: it takes the target in `operator_content`.
        if tk().look == aivory_protocol::Look::Operator {
            self.operator_screens.lens_target =
                Some((codebase.to_string(), path.to_string(), from_session));
            self.lens = None;
            self.screen = Screen::Lens;
            cx.notify();
            return;
        }
        // The Bridge look has its own Lens screen: it takes the target in `bridge_content`.
        if matches!(
            tk().look,
            aivory_protocol::Look::Bridge | aivory_protocol::Look::Neon
        ) {
            self.bridge_screens.lens_target =
                Some((codebase.to_string(), path.to_string(), from_session));
            self.lens = None;
            self.screen = Screen::Lens;
            cx.notify();
            return;
        }
        // The Tactical look has its own Lens screen: it takes the target in `tactical_content`.
        if tk().look == aivory_protocol::Look::Tactical {
            self.tactical_screens.lens_target =
                Some((codebase.to_string(), path.to_string(), from_session));
            self.lens = None;
            self.screen = Screen::Lens;
            cx.notify();
            return;
        }
        let model = self.model.clone();
        let (codebase, path) = (codebase.to_string(), path.to_string());
        let view =
            cx.new(|cx| LensScreen::new(model, codebase, path, from_session, back_label, cx));
        let subscription = cx.subscribe(&view, |this, _, event: &LensEvent, cx| {
            match event {
                LensEvent::Back => {
                    let back = std::mem::replace(&mut this.lens_back, Screen::Repos);
                    this.go(back, cx);
                }
                LensEvent::OpenSession(id) => this.open_session(id, cx),
                LensEvent::OpenSessions(ids) => this.open_first(ids, cx),
            }
            cx.notify();
        });
        self.lens = Some((view, subscription));
        self.screen = Screen::Lens;
        cx.notify();
    }

    /// Show a screen. The war room and Settings read the settings again, so a change on one
    /// shows on the other.
    fn go(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.refresh_display(cx));
        match screen {
            Screen::Settings => self.settings.update(cx, |s, cx| s.refresh(cx)),
            Screen::WarRoom => self.war_room.update(cx, |w, cx| w.reload_settings(cx)),
            _ => {}
        }
        if screen != Screen::Lens {
            self.lens = None;
        }
        self.bridge_full_settings = false;
        self.tactical_screens.full = false;
        self.command.full = false;
        self.screen = screen;
        cx.notify();
    }

    /// The deep dive of the session on screen. A view is made when the id changes.
    fn session_screen(&mut self, cx: &mut Context<Self>) -> Entity<SessionScreen> {
        let Screen::Session(id) = &self.screen else {
            unreachable!("only the session screen asks for a session view");
        };
        if let Some((open, view, _)) = &self.session
            && open == id
        {
            return view.clone();
        }
        let id = id.clone();
        let opener = id.clone();
        let view = cx.new(|cx| SessionScreen::new(self.model.clone(), id.clone(), cx));
        let subscription = cx.subscribe(&view, move |this, _, event: &SessionEvent, cx| {
            match event {
                SessionEvent::Back => {
                    let back = std::mem::replace(&mut this.back, Screen::WarRoom);
                    this.go(back, cx);
                }
                SessionEvent::Open(id) => this.screen = Screen::Session(id.clone()),
                SessionEvent::OpenFile(path) => {
                    let codebase = this
                        .model
                        .read(cx)
                        .war
                        .as_ref()
                        .and_then(|w| w.session(&opener).map(|s| s.codebase.clone()));
                    if let Some(codebase) = codebase {
                        this.open_lens(&codebase, path, Some(opener.clone()), cx);
                    }
                }
            }
            cx.notify();
        });
        self.session = Some((id, view.clone(), subscription));
        view
    }

    /// The Neon view of the session on screen. A view is made when the session changes.
    fn night_detail_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<neon_session::NightSession> {
        use neon_session::{NightSession, NightSessionEvent};
        let Screen::Session(id) = &self.screen else {
            unreachable!("only the session screen asks for a session view");
        };
        let id = id.clone();
        if let Some((open, view, _)) = &mut self.night_detail {
            if *open != id {
                view.update(cx, |v, cx| v.show(&id, cx));
                *open = id;
            }
            return view.clone();
        }
        let view = cx.new(|cx| NightSession::new(self.model.clone(), id.clone(), window, cx));
        let subscription = cx.subscribe(&view, |this, _, event: &NightSessionEvent, cx| {
            match event {
                NightSessionEvent::Back => {
                    let back = std::mem::replace(&mut this.back, Screen::WarRoom);
                    this.go(back, cx);
                }
                NightSessionEvent::Switch(id) => this.screen = Screen::Session(id.clone()),
                NightSessionEvent::OpenFile { session, path } => {
                    this.open_session_file(session, path, cx)
                }
            }
            cx.notify();
        });
        self.night_detail = Some((id, view.clone(), subscription));
        view
    }

    /// The Bridge view of the session on screen. A view is made when the session changes.
    fn bridge_detail_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<BridgeDetail> {
        let Screen::Session(id) = &self.screen else {
            unreachable!("only the session screen asks for a session view");
        };
        let id = id.clone();
        if let Some((open, view, _)) = &mut self.bridge_detail {
            if *open != id {
                view.update(cx, |v, cx| v.show(&id, cx));
                *open = id;
            }
            return view.clone();
        }
        let view = cx.new(|cx| BridgeDetail::new(self.model.clone(), id.clone(), window, cx));
        let subscription = cx.subscribe(&view, |this, _, event: &BridgeDetailEvent, cx| {
            match event {
                BridgeDetailEvent::Back => {
                    let back = std::mem::replace(&mut this.back, Screen::WarRoom);
                    this.go(back, cx);
                }
                BridgeDetailEvent::Switch(id) => this.screen = Screen::Session(id.clone()),
                BridgeDetailEvent::Focus(id) => this.open_focus(id, cx),
                BridgeDetailEvent::OpenFile { session, path } => {
                    this.open_session_file(session, path, cx)
                }
            }
            cx.notify();
        });
        self.bridge_detail = Some((id, view.clone(), subscription));
        view
    }

    /// The rail. Under [`NARROW`] it is a compact strip of icons: no screen is lost.
    fn rail(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let inbox = self.model.read(cx).war.as_ref().map_or(0, |w| w.inbox_open);
        div()
            .flex_none()
            .w(if compact { RAIL_W_COMPACT } else { RAIL_W })
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .pt(px(if compact { 10. } else { 16. }))
            .pb(px(12.))
            .bg(rgb(tk().card))
            // No region border: a soft shadow sets the rail off the white page.
            .shadow(vec![gpui::BoxShadow {
                color: ui::hsla(0x00000040),
                offset: gpui::point(px(8.), px(0.)),
                blur_radius: px(24.),
                spread_radius: px(-20.),
                inset: false,
            }])
            .when(!compact, |d| {
                d.child(
                    svg()
                        .path("icons/logo.svg")
                        .w(px(40.))
                        .h(px(14.))
                        .mb(px(18.))
                        .text_color(rgb(tk().logo)),
                )
            })
            .child(self.rail_item(Screen::WarRoom, 0, compact, cx))
            .child(self.rail_item(Screen::Inbox, inbox, compact, cx))
            .child(self.rail_item(Screen::Tasks, 0, compact, cx))
            .child(self.rail_item(Screen::Repos, 0, compact, cx))
            .child(div().flex_1())
            .child(self.rail_item(Screen::Settings, 0, compact, cx))
    }

    fn rail_item(
        &self,
        screen: Screen,
        badge: u32,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on = self.screen.rail() == screen;
        let group = SharedString::from(format!("rail-{}", screen.label()));
        let ink = rgb(if on { tk().ink } else { tk().ink_3 });
        let icon = screen.icon();
        let label = screen.label();
        div()
            .id(group.clone())
            .group(group.clone())
            .relative()
            .w(px(if compact { 38. } else { 60. }))
            .h(px(if compact { 38. } else { 54. }))
            .whitespace_nowrap()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .rounded(tk().r_rail_item)
            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
            .text_color(ink)
            .cursor_pointer()
            .when(on, |d| {
                d.bg(rgb(tk().rail_on)).font_weight(FontWeight::MEDIUM)
            })
            .when(!on, |d| {
                d.hover(|s| s.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
            })
            .when(compact, |d| d.tooltip(ui::tip(label)))
            .on_click(cx.listener(move |this, _, _, cx| this.go(screen.clone(), cx)))
            .child(
                svg()
                    .path(icon)
                    .size(px(18.))
                    .text_color(ink)
                    .group_hover(group, |s| s.text_color(rgb(tk().ink))),
            )
            .when(!compact, |d| d.child(label))
            .when(badge > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(if compact { 0. } else { 6. }))
                        .right(px(if compact { 0. } else { 10. }))
                        .min_w(px(16.))
                        .h(px(16.))
                        .px(px(4.))
                        .rounded_full()
                        .bg(rgb(tk().q))
                        .text_color(rgb(tk().card))
                        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(badge.to_string()),
                )
            })
    }

    /// The notices of the model: a calm card at the bottom, above the status bar. The dot shows
    /// the state; a click on the card or the cross hides it.
    fn toasts(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let notices = self.model.read(cx).notices.clone();
        if notices.is_empty() {
            return None;
        }
        let mut col = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(STATUS_H + px(14.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .px(px(16.));
        for n in notices {
            let id = n.id;
            let color = match n.kind {
                crate::notice::Kind::Failure => tk().e,
                crate::notice::Kind::Info => tk().ok,
            };
            let mut card = div()
                .id(SharedString::from(format!("toast-{id}")))
                .max_w(px(640.))
                .py(px(8.))
                .pl(px(12.))
                .pr(px(6.))
                .flex()
                .items_center()
                .gap(px(10.))
                .bg(rgb(tk().card))
                .border_1()
                .border_color(rgba(tk().line_2))
                .rounded(tk().r_button)
                .shadow(ui::popover_shadow())
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .text_color(rgb(tk().ink))
                .child(ui::dot(rgb(color)))
                .child(div().min_w(px(0.)).child(n.text.clone()));
            if let Some(help) = n.help.clone() {
                card = card.child(
                    div()
                        .id(SharedString::from(format!("toast-help-{id}")))
                        .h(px(26.))
                        .px(px(10.))
                        .flex()
                        .flex_none()
                        .items_center()
                        .rounded(px(6.))
                        .bg(rgb(tk().track))
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(tk().track_2)))
                        .on_click(move |_, _, cx| cx.open_url(help.url))
                        .child(help.label),
                );
            }
            card = card.child(
                div()
                    .id(SharedString::from(format!("toast-x-{id}")))
                    .size(px(24.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .text_color(rgb(tk().ink_3))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
                    .tooltip(ui::tip("Hide"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.update(cx, |m, cx| m.dismiss_notice(id, cx))
                    }))
                    .child("×"),
            );
            col = col.child(card);
        }
        Some(col)
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let counts = model.war.as_ref().map(|w| w.counts()).unwrap_or_default();
        let (daemon_color, daemon_text): (u32, SharedString) = match &model.link {
            Link::Connecting => (tk().quiet, "aivoryd connecting".into()),
            Link::Running if model.demo => (tk().ok, "aivoryd running · demo".into()),
            Link::Running => (tk().ok, "aivoryd running".into()),
            Link::Offline(_) => (tk().e, "aivoryd offline".into()),
        };
        let mut bar = div()
            .flex_none()
            .h(STATUS_H)
            .flex()
            .items_center()
            .gap(px(20.))
            .px(px(16.))
            .bg(rgb(tk().card))
            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
            .text_color(rgb(tk().ink_2))
            .whitespace_nowrap()
            .overflow_hidden()
            .child(status_item(Some(daemon_color), daemon_text));
        if let Some(war) = &model.war {
            bar = bar.child(status_item(None, running_text(&counts).into()));
            for (i, window) in first_windows(&war.usage).into_iter().enumerate() {
                bar = bar.child(status_item(None, usage_text(window, i == 0).into()));
            }
        }
        bar = bar.child(div().flex_1());
        if let Some(war) = &model.war {
            for (state, name) in [(war.hub, "hub"), (war.relay, "relay")] {
                if let Some(color) = link_color(state) {
                    bar = bar.child(status_item(Some(color), name.into()));
                }
            }
        }
        let clock = match &model.war {
            Some(war) if model.demo => model.clock(war.now),
            _ => chrono::Local::now().format("%H:%M").to_string(),
        };
        bar.child(self.look_switch(cx))
            .child(status_item(None, clock.into()))
    }

    /// The look switch of the app frame (ADR 0022): the current look; a click shows all looks.
    fn look_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = tk().look;
        let chip = |id: SharedString, label: &'static str, on: bool| {
            div()
                .id(id)
                .h(px(20.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(tk().r_button)
                .cursor_pointer()
                .when(on, |d| d.bg(rgb(tk().track)).text_color(rgb(tk().ink)))
                .hover(|s| s.bg(rgb(tk().track_2)).text_color(rgb(tk().ink)))
                .child(label)
        };
        if !self.look_menu {
            return chip("look-switch".into(), crate::look::name(current), false)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.look_menu = true;
                    cx.notify();
                }))
                .into_any_element();
        }
        let mut list = div()
            .id("look-list")
            .occlude()
            .flex()
            .flex_col()
            .p(px(4.))
            .bg(rgb(tk().card))
            .border_1()
            .border_color(rgb(tk().edge))
            .rounded(tk().r_button)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.look_menu = false;
                cx.notify();
            }));
        for (look, name) in crate::look::LOOKS {
            list = list.child(
                chip(format!("look-{name}").into(), name, look == current).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.look_menu = false;
                        crate::actions::act(&this.model, crate::actions::Action::SetLook(look), cx);
                    },
                )),
            );
        }
        div()
            .relative()
            .child(
                chip("look-switch".into(), crate::look::name(current), true).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.look_menu = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .offset(point(px(0.), px(-4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(list),
                )
                .with_priority(3),
            )
            .into_any_element()
    }
}

impl Shell {
    /// The layout name of the screen on view, for a custom theme. The Lens has none.
    fn layout_name(&self) -> Option<&'static str> {
        Some(match &self.screen {
            Screen::WarRoom => "warroom",
            Screen::Session(_) => "session",
            Screen::Focus(_) => "focus",
            Screen::Inbox => "inbox",
            Screen::Tasks => "tasks",
            Screen::Repos => "repos",
            Screen::Settings => "settings",
            Screen::Lens => return None,
        })
    }

    /// The view of the custom theme when the theme has a layout for the screen on view. A
    /// screen without a layout stays the compiled Minimal screen with the theme tokens.
    fn themed_screen(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::themed::host::{ThemedEvent, ThemedView};
        if !crate::themed::active() {
            self.themed_dialog = None;
            return None;
        }
        let name = self.layout_name()?;
        if !crate::themed::host::has_layout(name) {
            self.themed_dialog = None;
            return None;
        }
        let view = match &self.themed {
            Some((v, _)) => v.clone(),
            None => {
                let settings: gpui::AnyView = self.settings.clone().into();
                let view = cx.new(|_| ThemedView::new(self.model.clone(), settings));
                let sub = cx.subscribe(&view, |this, _, ev: &ThemedEvent, cx| match ev {
                    ThemedEvent::Screen(Screen::Focus(id)) => this.open_focus(id, cx),
                    ThemedEvent::Screen(s) => this.go(s.clone(), cx),
                    ThemedEvent::Decision(id) => {
                        if crate::themed::host::has_layout("decision") {
                            this.themed_dialog = Some(id.clone());
                            cx.notify();
                        } else {
                            this.go(Screen::Inbox, cx);
                        }
                    }
                    ThemedEvent::CloseDecision => {
                        this.themed_dialog = None;
                        cx.notify();
                    }
                });
                self.themed = Some((view.clone(), sub));
                view
            }
        };
        let selected = match &self.screen {
            Screen::Session(id) | Screen::Focus(id) => Some(id.clone()),
            _ => None,
        };
        let dialog = self.themed_dialog.clone();
        view.update(cx, |v, _| {
            v.show(name, selected.as_deref(), dialog.as_deref());
        });
        Some(view.into_any_element())
    }

    /// Open the New session dialog at the next frame (it needs the window).
    fn open_new_session(&mut self, cx: &mut Context<Self>) {
        if self.new_dialog.is_none() {
            self.new_pending = true;
            cx.notify();
        }
    }

    /// N (no text field has the focus) or Ctrl+N opens the New session dialog.
    fn new_session_keys(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_key_event(move |e: &KeyDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || e.is_held {
                        return;
                    }
                    // In the Command look Esc closes the window over the map.
                    if e.keystroke.key == "escape" {
                        let _ = weak.update(cx, |s, cx| s.command_escape(cx));
                        return;
                    }
                    if e.keystroke.key != "n" {
                        return;
                    }
                    let m = &e.keystroke.modifiers;
                    let plain = !m.modified() && window.focused(cx).is_none();
                    let ctrl = m.control && !m.alt && !m.shift;
                    if plain || ctrl {
                        let _ = weak.update(cx, |s, cx| s.open_new_session(cx));
                    }
                });
            },
        )
        .absolute()
        .size_0()
        .into_any_element()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.new_pending {
            self.new_pending = false;
            let model = self.model.clone();
            let dialog = cx.new(|cx| crate::new_session::NewSessionDialog::new(model, window, cx));
            let sub = cx.subscribe(&dialog, |this, _, ev, cx| match ev {
                crate::new_session::DialogEvent::Close => {
                    this.new_dialog = None;
                    cx.notify();
                }
            });
            self.new_dialog = Some((dialog, sub));
        }
        let looks = self.render_looks(window, cx).into_any_element();
        let keys = self.new_session_keys(cx);
        let full = window.is_fullscreen();
        let max_or_full = full || window.is_maximized();
        let (max_full, hint) = {
            let m = self.model.read(cx);
            (m.maximize_full_screen, m.full_screen_hint)
        };
        // The maximize button opens the borderless full screen. Leaving the full screen
        // restores the maximized window, and that is no new change, so it stays.
        if max_full && max_or_full && !self.max_or_full && !full {
            window.toggle_fullscreen();
        }
        self.max_or_full = max_or_full;
        let hint = hint.filter(|t| t.elapsed() < FULL_SCREEN_HINT);
        if let Some(t) = hint
            && self.hint_timer.is_none()
        {
            let left = FULL_SCREEN_HINT.saturating_sub(t.elapsed());
            self.hint_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(left).await;
                let _ = this.update(cx, |this, cx| {
                    this.hint_timer = None;
                    cx.notify();
                });
            }));
        }
        let hint = hint.map(|_| Self::full_screen_hint());
        let exit = full.then(Self::fullscreen_exit);
        div()
            .size_full()
            .relative()
            // F11 on the root element: the root is on every key dispatch path, also when a
            // text field has the focus. Capture phase, so no child can stop it.
            .capture_key_down(|e, window, cx| {
                if e.keystroke.key == "f11" && !e.is_held {
                    window.toggle_fullscreen();
                    cx.stop_propagation();
                }
            })
            .child(looks)
            .child(keys)
            .children(exit)
            .children(hint)
            .children(self.new_dialog.as_ref().map(|(d, _)| d.clone()))
    }
}

/// How long the F11 hint stays after the owner turns on "Maximize is full screen".
const FULL_SCREEN_HINT: Duration = Duration::from_secs(5);

impl Shell {
    /// The hint at the top: F11 leaves the full screen.
    fn full_screen_hint() -> AnyElement {
        div()
            .absolute()
            .top(px(12.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(14.))
                    .py(px(8.))
                    .rounded_full()
                    .bg(rgb(tk().ink))
                    .text_color(rgb(tk().card))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .child("Maximize now opens full screen. Press F11 to go back."),
            )
            .into_any_element()
    }

    /// In full screen the window has no title bar. A pill at the top edge shows on hover and
    /// switches the window back; F11 does the same.
    fn fullscreen_exit() -> AnyElement {
        let group = "fullscreen-exit";
        div()
            .id("fullscreen-exit")
            .group(group)
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(36.))
            .flex()
            .justify_center()
            .items_start()
            .child(
                div()
                    .id("fullscreen-exit-pill")
                    .mt(px(6.))
                    .px(px(12.))
                    .h(px(26.))
                    .rounded_full()
                    .bg(rgb(tk().ink))
                    .text_color(rgb(tk().card))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(0.92))
                    .on_click(|_, window, _| window.toggle_fullscreen())
                    .child("Exit full screen  F11"),
            )
            .into_any_element()
    }
}

impl Shell {
    fn render_looks(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = window.viewport_size().width < NARROW;
        // A deep dive that is not on screen stops its polling.
        let bridge = tk().look == aivory_protocol::Look::Bridge;
        let operator = tk().look == aivory_protocol::Look::Operator;
        let night = tk().look == aivory_protocol::Look::Neon;
        if !matches!(self.screen, Screen::Session(_)) || bridge || operator || night {
            self.session = None;
        }
        if !matches!(self.screen, Screen::Session(_)) || !night {
            self.night_detail = None;
        }
        if !matches!(self.screen, Screen::Session(_)) || !bridge {
            self.bridge_detail = None;
        }
        if !matches!(self.screen, Screen::Focus(_)) {
            self.focus = None;
            self.irix_screens.focus = None;
        }
        if let Some(el) = self.themed_screen(cx) {
            let toasts = self.toasts(cx).map(IntoElement::into_any_element);
            return div()
                .size_full()
                .relative()
                .bg(rgb(tk().page))
                .font_family(tk().sans)
                .text_size(TEXT)
                .text_color(rgb(tk().ink))
                .child(el)
                .children(toasts);
        }
        let navigator = tk().look == aivory_protocol::Look::Navigator;
        let hud = matches!(
            tk().look,
            aivory_protocol::Look::Holo | aivory_protocol::Look::Tactical
        );
        if hud && let Screen::Session(id) = self.screen.clone() {
            self.war_room.update(cx, |w, _| w.hud_set_open(&id));
            self.screen = Screen::WarRoom;
        }
        let holo = Self::holo_on();
        if holo {
            self.holo_cycle(cx);
        }
        let content = match self.screen {
            Screen::WarRoom => self.war_room.clone().into_any_element(),
            Screen::Inbox => self.inbox.clone().into_any_element(),
            Screen::Tasks if navigator => self.irix_tasks_view(cx),
            Screen::Repos if navigator => self.irix_repos_view(cx),
            Screen::Tasks => self.tasks.clone().into_any_element(),
            Screen::Repos => self.repos.clone().into_any_element(),
            Screen::Settings => self.settings.clone().into_any_element(),
            Screen::Session(_) if operator => self.operator_session(window, cx),
            Screen::Session(_) if bridge => {
                self.bridge_detail_screen(window, cx).into_any_element()
            }
            Screen::Session(_) if night => self.night_detail_screen(window, cx).into_any_element(),
            Screen::Session(_) => self.session_screen(cx).into_any_element(),
            Screen::Focus(_) if tk().look == aivory_protocol::Look::Tactical => {
                div().into_any_element()
            }
            Screen::Focus(_) if operator => div().into_any_element(),
            Screen::Focus(_) if bridge || holo => div().into_any_element(),
            Screen::Focus(_) if navigator => self.irix_focus_view(window, cx),
            Screen::Focus(_) if night => div().into_any_element(),
            Screen::Focus(_) => self.focus_screen(window, cx).into_any_element(),
            Screen::Lens => match &self.lens {
                Some((lens, _)) => lens.clone().into_any_element(),
                None => div().into_any_element(),
            },
        };
        let content = if bridge {
            self.bridge_content(content, window, cx)
        } else if night {
            self.night_content(content, window, cx)
        } else if holo {
            self.holo_content(content, window, cx)
        } else {
            content
        };
        let content = if operator {
            self.operator_content(content, window, cx)
        } else {
            content
        };
        let content = if tk().look == aivory_protocol::Look::Tactical {
            self.tactical_content(content, window, cx)
        } else {
            content
        };
        let command = tk().look == aivory_protocol::Look::Command;
        let content = if command {
            self.command_content(content, window, cx)
        } else {
            content
        };
        let toasts = self.toasts(cx).map(IntoElement::into_any_element);
        // The Command look replaces the navigation: the command sidebar at the right.
        if command {
            return self.command_screen(content, toasts, window, cx);
        }
        // The Bridge look replaces the navigation: numbered frame segments, no rail, no status
        // bar (ADR 0022).
        if bridge {
            return self.bridge_screen(content, toasts, window, cx);
        }
        // The Neon look replaces the navigation: a neon header with tabs, no rail and no
        // status bar.
        if night {
            return self.neon_screen(content, toasts, cx);
        }
        // The Holo and Tactical looks replace the navigation: one bar on top, no rail and no
        // status bar.
        if holo {
            return self.holo_screen(content, toasts, window, cx);
        }
        if hud {
            let tactical = tk().look == aivory_protocol::Look::Tactical;
            let bar = if tactical {
                self.tactical_bar(cx)
            } else {
                self.hud_bar(cx)
            };
            let menu = if tactical {
                self.tactical_menu(cx)
            } else {
                self.hud_menu(cx)
            };
            return div()
                .size_full()
                .relative()
                .flex()
                .flex_col()
                .bg(rgb(tk().page))
                .font_family(tk().sans)
                .text_size(TEXT)
                .text_color(rgb(tk().ink))
                .child(bar)
                .child(div().relative().flex_1().min_h(px(0.)).child(content))
                .children(menu)
                .children(toasts);
        }
        // The Operator look replaces the navigation: the F-keys in the top line, no rail, no
        // status bar.
        if operator {
            return self.operator_screen(content, toasts, cx);
        }
        // The Navigator look replaces the navigation: the Toolchest bar on top, no rail and no
        // status bar; every screen but the war room sits in a grey window.
        if tk().look == aivory_protocol::Look::Navigator {
            let chest = self.irix_chest(cx);
            let menu = self.irix_look_menu(cx);
            let win_menu = self.irix_win_menu(cx);
            let body = if self.screen == Screen::WarRoom {
                content
            } else {
                self.irix_window(content, cx)
            };
            return div()
                .size_full()
                .relative()
                .flex()
                .flex_col()
                .bg(rgb(irix::DESKTOP))
                .font_family(tk().sans)
                .text_size(TEXT)
                .text_color(rgb(tk().ink))
                .child(chest)
                .child(div().relative().flex_1().min_h(px(0.)).child(body))
                .children(menu)
                .children(win_menu)
                .children(toasts);
        }
        let rail = self.rail(narrow, cx).into_any_element();
        let status = self.status_bar(cx).into_any_element();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(tk().page))
            .font_family(tk().sans)
            .text_size(TEXT)
            .text_color(rgb(tk().ink))
            .child(
                div().flex_1().min_h(px(0.)).flex().child(rail).child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(content),
                ),
            )
            .child(status)
            .children(toasts)
    }
}

/// `AIVORY_SESSION=<id>` starts in the session view of that session (screenshot runs).
fn session_env() -> Option<String> {
    std::env::var("AIVORY_SESSION")
        .ok()
        .filter(|id| !id.is_empty())
}

fn status_item(dot_color: Option<u32>, text: SharedString) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .when_some(dot_color, |d, c| d.child(ui::dot(rgb(c))))
        .child(text)
        .into_any_element()
}

fn running_text(counts: &WarRoomCounts) -> String {
    format!("{} running · {} parked", counts.live, counts.quiet)
}

/// `None` for a link that is not configured: the status bar leaves it out.
fn link_color(state: LinkState) -> Option<u32> {
    match state {
        LinkState::Connected => Some(tk().ok),
        LinkState::Connecting => Some(tk().q),
        LinkState::Offline => Some(tk().quiet),
        LinkState::Off => None,
    }
}

/// The first usage window of each provider, in daemon order.
fn first_windows(usage: &[UsageWindow]) -> Vec<&UsageWindow> {
    let mut seen: Vec<&str> = Vec::new();
    usage
        .iter()
        .filter(|w| {
            let new = !seen.contains(&w.provider.as_str());
            seen.push(&w.provider);
            new
        })
        .collect()
}

/// `claude 5 h window 62%`, then `codex 31%`: the first item names its window, as the artboard.
fn usage_text(window: &UsageWindow, with_label: bool) -> String {
    let estimate = if window.estimate { " estimate" } else { "" };
    if with_label {
        format!(
            "{} {} {}%{estimate}",
            window.provider, window.label, window.pct
        )
    } else {
        format!("{} {}%{estimate}", window.provider, window.pct)
    }
}

/// A path under the codebase `root` as a relative path with `/`; other paths stay as they are.
fn relative_to(path: &str, root: &str) -> String {
    let norm = |s: &str| s.replace('\\', "/");
    let (path, root) = (norm(path), norm(root));
    match path.strip_prefix(root.trim_end_matches('/')) {
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/').to_string(),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    #[test]
    fn status_bar_usage_matches_the_artboard() {
        let war = Demo::new().war_room;
        let texts: Vec<String> = first_windows(&war.usage)
            .into_iter()
            .enumerate()
            .map(|(i, w)| usage_text(w, i == 0))
            .collect();
        assert_eq!(texts, ["claude 5 h window 62%", "codex 31%", "gemini 18%"]);
    }

    #[test]
    fn a_session_path_becomes_relative_to_the_codebase() {
        let win = "C:/work/app/src/a.rs".replace(char::from(47), "\\");
        assert_eq!(relative_to(&win, "C:/work/app"), "src/a.rs");
        assert_eq!(
            relative_to("/home/x/app/src/a.rs", "/home/x/app/"),
            "src/a.rs"
        );
        assert_eq!(relative_to("src/a.rs", "/home/x/app"), "src/a.rs");
        assert_eq!(
            relative_to("/home/x/app2/a.rs", "/home/x/app"),
            "/home/x/app2/a.rs"
        );
    }

    #[test]
    fn the_lens_marks_repos_in_the_rail() {
        assert_eq!(Screen::Lens.rail(), Screen::Repos);
        assert_eq!(Screen::Lens.icon(), Screen::Repos.icon());
        assert_eq!(Screen::Session("a".into()).rail(), Screen::WarRoom);
    }

    #[test]
    fn demo_counts_fill_the_header_and_status_bar() {
        let c = Demo::new().war_room.counts();
        assert!(c.needs_you > 0 && c.error > 0 && c.live > 0);
        assert_eq!(running_text(&c), "21 running · 3 parked");
    }
}
