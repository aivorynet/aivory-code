//! The app frame of the Command look (ADR 0022, boards `docs/design/looks/rts/`): the map of
//! the war room always fills the window at the left; the command sidebar at the right replaces
//! the rail and the status bar (one navigation only). Every other screen opens as a riveted steel
//! window over the dimmed map: Inbox (the Command view of `command_inbox.rs`), Tasks, Repos,
//! Setup, the session (Unit) and Lens. The X button or a click beside the window closes it.
//!
//! The sidebar: the LED counter (tokens left), the radar of the whole map (a click moves the
//! view), the screen buttons, the NEW UNIT build tile (the New session dialog, N), the sessions
//! that wait for you, and the RAM.

use aivory_protocol::SessionState;
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, MouseButton, MouseDownEvent, Subscription, Window, div, img,
    px,
};

use super::{Screen, Shell};
use crate::command_inbox::{CommandInbox, CommandInboxEvent};
use crate::command_kit::{self as cmd, c};
use crate::command_screens::{CmdEvent, CommandRepos, CommandSetup, CommandTasks, CommandUnit};
use crate::rts::{Frame9, frame, sprites};
use crate::theme::tk;
use crate::warroom::skins::command::{SIDE_W, Unit, rts_waits};

/// The Command views.
#[derive(Default)]
pub(super) struct CommandScreens {
    inbox: Option<(Entity<CommandInbox>, Subscription)>,
    tasks: Option<(Entity<CommandTasks>, Subscription)>,
    repos: Option<(Entity<CommandRepos>, Subscription)>,
    setup: Option<(Entity<CommandSetup>, Subscription)>,
    unit: Option<(String, Entity<CommandUnit>, Subscription)>,
    /// Show the full screen of this view inside the window.
    pub(super) full: bool,
}

/// What a Command window asks the shell to do.
fn handle(this: &mut Shell, event: &CmdEvent, cx: &mut Context<Shell>) {
    match event {
        CmdEvent::OpenSession(id) => this.open_session(id, cx),
        CmdEvent::OpenFile { session, path } => this.open_session_file(session, path, cx),
        CmdEvent::Full => {
            this.command.full = true;
            cx.notify();
        }
        CmdEvent::NewUnit => this.open_new_session(cx),
        CmdEvent::Close => this.go_back(cx),
        CmdEvent::Village(name) => {
            this.go(Screen::WarRoom, cx);
            let name = name.clone();
            this.war_room.update(cx, |w, cx| w.rts_village(&name, cx));
        }
    }
}

impl Shell {
    /// Esc closes the window over the map (the New session dialog handles its own Esc).
    pub(super) fn command_escape(&mut self, cx: &mut Context<Self>) {
        if tk().look == aivory_protocol::Look::Command
            && self.screen != Screen::WarRoom
            && self.new_dialog.is_none()
        {
            self.go(Screen::WarRoom, cx);
        }
    }

    /// The Command view of a screen: its own window content, or the full screen on request.
    pub(super) fn command_content(
        &mut self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.command.full {
            return content;
        }
        let model = self.model.clone();
        match self.screen.clone() {
            Screen::Inbox => {
                if self.command.inbox.is_none() {
                    let v = cx.new(|cx| CommandInbox::new(model, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &CommandInboxEvent, cx| match e {
                        CommandInboxEvent::OpenSession(id) => t.open_session(id, cx),
                    });
                    self.command.inbox = Some((v, sub));
                }
                self.command
                    .inbox
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Tasks => {
                if self.command.tasks.is_none() {
                    let v = cx.new(|cx| CommandTasks::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &CmdEvent, cx| handle(t, e, cx));
                    self.command.tasks = Some((v, sub));
                }
                self.command
                    .tasks
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Repos => {
                if self.command.repos.is_none() {
                    let v = cx.new(|cx| CommandRepos::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &CmdEvent, cx| handle(t, e, cx));
                    self.command.repos = Some((v, sub));
                }
                self.command
                    .repos
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Settings => {
                if self.command.setup.is_none() {
                    let v = cx.new(|cx| CommandSetup::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &CmdEvent, cx| handle(t, e, cx));
                    self.command.setup = Some((v, sub));
                }
                self.command
                    .setup
                    .as_ref()
                    .map_or(content, |(v, _)| v.clone().into_any_element())
            }
            Screen::Session(id) => {
                if self.command.unit.as_ref().is_none_or(|(k, ..)| *k != id) {
                    let session = id.clone();
                    let v = cx.new(|cx| CommandUnit::new(model, session, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &CmdEvent, cx| handle(t, e, cx));
                    self.command.unit = Some((id, v, sub));
                }
                self.command
                    .unit
                    .as_ref()
                    .map_or(content, |(_, v, _)| v.clone().into_any_element())
            }
            _ => content,
        }
    }

    /// The title of the window of a screen.
    fn command_title(&self, cx: &mut Context<Self>) -> String {
        let war = self.model.read(cx).war.clone();
        match &self.screen {
            Screen::Inbox => format!(
                "INBOX · {} ORDERS WAIT",
                war.as_ref().map_or(0, |w| w.inbox_open)
            ),
            Screen::Tasks => "TASKS · BUILDING PLANS".into(),
            Screen::Repos => format!(
                "VILLAGES · {} CODEBASES",
                war.as_ref().map_or(0, |w| w.codebases.len())
            ),
            Screen::Settings => "SETTINGS".into(),
            Screen::Session(id) => {
                let t = war
                    .as_ref()
                    .and_then(|w| w.session(id))
                    .map_or(String::new(), |p| cmd::up(&p.title));
                format!("UNIT · {t}")
            }
            Screen::Lens => "LENS".into(),
            Screen::WarRoom => String::new(),
        }
    }

    /// The whole Command screen: the map, a window over it for every screen but the war room,
    /// and the sidebar.
    pub(super) fn command_screen(
        &mut self,
        content: AnyElement,
        toasts: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let map_w = (vw - SIDE_W).max(200.);
        let side = self.command_side(vh, cx);
        let mut root = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(c(0x1E2124, 1.))
            .font_family(cmd::SANS)
            .text_color(c(cmd::INK, 1.));
        if self.screen == Screen::WarRoom {
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(map_w))
                    .h(px(vh))
                    .child(content),
            );
        } else {
            let (ww, wh) = cmd::win_size(vw, vh, SIDE_W);
            // The setup window holds a few rows: it takes the height of its rows, not the screen.
            let wh = if self.screen == Screen::Settings && !self.command.full {
                wh.min(660.)
            } else {
                wh
            };
            let title = self.command_title(cx);
            // The own Command views draw on the steel; a full screen keeps its page color.
            let own = !self.command.full
                && matches!(
                    self.screen,
                    Screen::Inbox
                        | Screen::Tasks
                        | Screen::Repos
                        | Screen::Settings
                        | Screen::Session(_)
                );
            let inner = if own {
                content
            } else {
                div()
                    .size_full()
                    .relative()
                    .overflow_hidden()
                    .bg(c(crate::theme::tk().page, 1.))
                    .child(content)
                    .into_any_element()
            };
            root = root
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(px(map_w))
                        .h(px(vh))
                        .child(self.war_room.clone()),
                )
                .child(
                    div()
                        .id("cmd-dim")
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(px(map_w))
                        .h(px(vh))
                        .bg(c(cmd::WELL, 1.))
                        .occlude()
                        .on_click(cx.listener(|this, _, _, cx| this.go(Screen::WarRoom, cx))),
                )
                .child(
                    div()
                        .id("cmd-window")
                        .absolute()
                        .left(px(((map_w - ww) / 2.).floor()))
                        .top(px(((vh - wh) / 2.).floor()))
                        .occlude()
                        .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                            if ev.keystroke.key == "escape" {
                                this.go(Screen::WarRoom, cx);
                            }
                        }))
                        .child(cmd::window(
                            ww,
                            wh,
                            title,
                            cmd::button("cmd-x", "X", cmd::Btn::Plain).on_click(
                                cx.listener(|this, _, _, cx| this.go(Screen::WarRoom, cx)),
                            ),
                            inner,
                        )),
                );
        }
        root.child(
            div()
                .absolute()
                .right_0()
                .top_0()
                .w(px(SIDE_W))
                .h(px(vh))
                .child(side),
        )
        .children(toasts)
    }

    /// The command sidebar.
    fn command_side(&mut self, vh: f32, cx: &mut Context<Self>) -> Div {
        let here = self.screen.rail();
        let war = self.model.read(cx).war.clone();
        let s = sprites();
        // Under 860 px of height the radar is hidden and the rest moves up.
        let compact = vh < 860.;
        let dy = if compact { 206. } else { 0. };
        let mut side = div().relative().w(px(SIDE_W)).h(px(vh)).child(
            img(frame(Frame9::Panel, SIDE_W, vh))
                .absolute()
                .left_0()
                .top_0()
                .size_full(),
        );
        let well = |x: f32, y: f32, w: f32, h: f32| {
            cmd::well()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(w))
                .h(px(h))
        };
        // LED counter: the tokens left in the first usage window
        let usage = war.as_ref().and_then(|w| w.usage.first().cloned());
        let led = match &usage {
            Some(u) if u.pct > 0 && u.used > 0 => {
                let total = u.used * 100 / u64::from(u.pct);
                cmd::tokens(total.saturating_sub(u.used))
            }
            Some(u) => format!("{}%", 100 - u.pct.min(100)),
            None => "----".into(),
        };
        let led_label = match &usage {
            Some(u) if u.pct > 0 && u.used > 0 => "TOKENS LEFT",
            Some(_) => "USAGE LEFT",
            None => "USAGE",
        };
        side = side.child(
            well(24., 22., 272., 40.)
                .bg(c(0x040704, 1.))
                .child(
                    cmd::data(led_label, cmd::S_ENTRY)
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .text_color(c(cmd::INK_3, 1.)),
                )
                .child(
                    cmd::data(led, cmd::S_TITLE)
                        .absolute()
                        .right(px(12.))
                        .top(px(4.))
                        .text_color(c(cmd::LED, 1.)),
                ),
        );
        // radar: the whole map, the units, the view
        let (lay, pan, view, ground) = {
            let m = self.war_room.read(cx).rts_map();
            (m.layout.clone(), m.pan, m.view, m.ground.clone())
        };
        let mut radar = well(24., 72., 272., 196.).overflow_hidden();
        if let Some(g) = ground {
            let (rw, rh) = g.radar_size;
            let (ox, oy) = (4. + (264. - rw) / 2., 4. + (188. - rh) / 2.);
            let sc = rw / lay.w.max(1.);
            radar = radar.child(
                img(g.radar.clone())
                    .absolute()
                    .left(px(ox))
                    .top(px(oy))
                    .w(px(rw))
                    .h(px(rh)),
            );
            for v in &lay.villages {
                radar = radar.child(
                    div()
                        .absolute()
                        .left(px((ox + v.cx * sc).floor() - 3.))
                        .top(px((oy + v.cy * sc).floor() - 3.))
                        .size(px(6.))
                        .bg(c(0xF2F6E8, 1.)),
                );
                for u in &v.items {
                    if u.is_house() {
                        continue;
                    }
                    let col = match u.state {
                        SessionState::NeedsYou => cmd::AMBER,
                        SessionState::Error => cmd::RED,
                        SessionState::Working => cmd::LAMP_RUN,
                        _ => cmd::HINT,
                    };
                    radar = radar.child(
                        div()
                            .absolute()
                            .left(px((ox + u.x * sc).floor()))
                            .top(px((oy + u.y * sc).floor()))
                            .size(px(4.))
                            .bg(c(col, 1.)),
                    );
                }
            }
            radar = radar.child(
                div()
                    .absolute()
                    .left(px((ox + pan.0 * sc).floor()))
                    .top(px((oy + pan.1 * sc).floor()))
                    .w(px((view.0 * sc).min(rw)))
                    .h(px((view.1 * sc).min(rh)))
                    .border_2()
                    .border_color(c(0xF2F6E8, 1.)),
            );
            let (lw, lh) = (lay.w, lay.h);
            radar = radar.child(
                div()
                    .id("cmd-radar")
                    .absolute()
                    .left(px(ox))
                    .top(px(oy))
                    .w(px(rw))
                    .h(px(rh))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            let vw = f32::from(window.viewport_size().width);
                            let x = (f32::from(ev.position.x) - (vw - SIDE_W) - 24. - ox) / rw * lw;
                            let y = (f32::from(ev.position.y) - 72. - oy) / rh * lh;
                            this.war_room.update(cx, |w, cx| w.rts_jump(x, y, cx));
                            cx.notify();
                        }),
                    ),
            );
        } else {
            radar = radar.child(
                cmd::label("DRAWING THE MAP…")
                    .absolute()
                    .left(px(10.))
                    .top(px(10.)),
            );
        }
        if !compact {
            side = side.child(radar);
        }
        // The unit command card (C2, C3): the portrait of the selected worker, then a grid of
        // square command buttons with an icon, a name and the hotkey letter. FOCUS is the one
        // green button that opens the selected session.
        const CELL: f32 = 86.;
        const STEP: f32 = 93.;
        let on_map = self.screen == Screen::WarRoom;
        let selected = self.war_room.read(cx).rts_selected();
        let pane = selected
            .as_ref()
            .and_then(|id| war.as_ref().and_then(|w| w.session(id)).cloned());
        let list_open = self.war_room.read(cx).rts_list_open();
        let waits_n = war.as_ref().map_or(0, |w| rts_waits(w).len());
        let mut portrait = well(24., 278. - dy, 272., 116.).overflow_hidden();
        let slot = div()
            .absolute()
            .left(px(10.))
            .top(px(8.))
            .w(px(64.))
            .h(px(88.))
            .bg(c(0x2A4A1E, 1.))
            .child(
                img(s.bot.render.clone())
                    .absolute()
                    .left(px(9.))
                    .top_0()
                    .w(px(s.bot.rgba.width() as f32))
                    .h(px(s.bot.rgba.height() as f32)),
            );
        match &pane {
            Some(p) => {
                let (word, col) = match p.state {
                    SessionState::Working => ("WORKING", cmd::LAMP_RUN),
                    SessionState::NeedsYou => ("WAITS FOR YOU", cmd::AMBER),
                    SessionState::Error => ("STOPPED ON AN ERROR", cmd::RED),
                    SessionState::Idle => ("IDLE", cmd::HINT),
                    SessionState::Parked => ("PARKED", cmd::HINT),
                    SessionState::Done => ("DONE", cmd::HINT),
                };
                let now = war.as_ref().map_or(0, |w| w.now);
                portrait = portrait.child(slot).child(
                    div()
                        .absolute()
                        .left(px(86.))
                        .top(px(8.))
                        .w(px(176.))
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            cmd::data(cmd::up(&p.title), cmd::S_ENTRY)
                                .max_h(px(38.))
                                .overflow_hidden()
                                .line_height(px(19.))
                                .text_color(c(cmd::WHITE, 1.)),
                        )
                        .child(
                            cmd::label(cmd::up(&format!("{} · {}", p.codebase, p.provider)))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis(),
                        )
                        .child(cmd::data(word, cmd::S_ENTRY).text_color(c(col, 1.)))
                        .child(cmd::label(format!(
                            "{} · CTX {}%",
                            cmd::mins(now, p.since),
                            p.context_pct
                        )))
                        .child(cmd::bar(f32::from(p.context_pct) / 100.).mt(px(3.))),
                );
            }
            None => {
                portrait = portrait.child(slot.opacity(0.35)).child(
                    div()
                        .absolute()
                        .left(px(86.))
                        .top(px(22.))
                        .w(px(176.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(cmd::data("NO UNIT SELECTED", cmd::S_ENTRY))
                        .child(cmd::label("CLICK A WORKER"))
                        .child(cmd::label("ON THE MAP")),
                );
            }
        }
        side = side.child(portrait);

        let cell = |id: &'static str,
                    label: &str,
                    icon: &'static str,
                    key: Option<&'static str>,
                    on: bool,
                    enabled: bool,
                    badge: Option<String>| {
            let ink = if on { 0xEEF8DE } else { cmd::INK };
            let key = key.filter(|_| on_map);
            div()
                .id(id)
                .relative()
                .w(px(CELL))
                .h(px(CELL))
                .flex_none()
                .when(enabled, |d| d.cursor_pointer().hover(|st| st.opacity(0.9)))
                .when(!enabled, |d| d.opacity(0.4))
                .child(
                    img(frame(
                        if on { Frame9::ButtonOn } else { Frame9::Button },
                        CELL,
                        CELL,
                    ))
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(15.))
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(crate::ui::icon(icon, 30., gpui::rgb(ink))),
                )
                .child(
                    cmd::data(label.to_string(), cmd::S_LABEL)
                        .absolute()
                        .left_0()
                        .bottom(px(9.))
                        .w_full()
                        .flex()
                        .justify_center()
                        .whitespace_nowrap()
                        .text_color(c(ink, 1.)),
                )
                .when_some(key, |d, k| {
                    d.child(
                        cmd::data(k, cmd::S_LABEL)
                            .absolute()
                            .left(px(9.))
                            .top(px(6.))
                            .text_color(c(if on { 0xEEF8DE } else { cmd::AMBER }, 1.)),
                    )
                })
                .when_some(badge, |d, b| {
                    d.child(
                        cmd::data(b, cmd::S_LABEL)
                            .absolute()
                            .right(px(9.))
                            .top(px(6.))
                            .text_color(c(cmd::AMBER, 1.)),
                    )
                })
        };
        let grid_y = 402. - dy;
        let at = |col: usize, row: usize| (24. + col as f32 * STEP, grid_y + row as f32 * STEP);
        let live = pane
            .as_ref()
            .is_some_and(|p| p.controlled && p.state.is_live());
        let parked = pane
            .as_ref()
            .is_some_and(|p| p.state == SessionState::Parked);
        // row 1: the commands of the selected worker
        let (x, y) = at(0, 0);
        side = side.child(
            cell(
                "cmd-focus",
                "FOCUS",
                "icons/open.svg",
                Some("F"),
                pane.is_some() && on_map,
                pane.is_some(),
                None,
            )
            .absolute()
            .left(px(x))
            .top(px(y))
            .when(pane.is_some(), |d| {
                d.on_click(cx.listener(|this, _, _, cx| {
                    this.war_room
                        .update(cx, |w, cx| w.rts_unit(Unit::Focus, cx))
                }))
            }),
        );
        let (x, y) = at(1, 0);
        side = side.child(
            cell(
                "cmd-park",
                if parked { "RESUME" } else { "PARK" },
                if parked {
                    "icons/play.svg"
                } else {
                    "icons/pause.svg"
                },
                Some("P"),
                false,
                live || parked,
                None,
            )
            .absolute()
            .left(px(x))
            .top(px(y))
            .when(live || parked, |d| {
                d.on_click(cx.listener(|this, _, _, cx| {
                    this.war_room.update(cx, |w, cx| {
                        w.rts_unit(Unit::Park, cx);
                        w.rts_unit(Unit::Resume, cx);
                    })
                }))
            }),
        );
        let (x, y) = at(2, 0);
        side = side.child(
            cell(
                "cmd-stop",
                "STOP",
                "icons/stop.svg",
                Some("S"),
                false,
                live,
                None,
            )
            .absolute()
            .left(px(x))
            .top(px(y))
            .when(live, |d| {
                d.on_click(cx.listener(|this, _, _, cx| {
                    this.war_room.update(cx, |w, cx| w.rts_unit(Unit::Stop, cx))
                }))
            }),
        );
        // row 2: New session and Organizer (they light up while their dialog is open), LIST
        let launch = crate::new_session::launch_buttons_open(
            "command",
            self.launcher_open(cx),
            |id, label, on| {
                let (name, icon, key) = if label == "Organizer" {
                    ("ORG", "icons/panel.svg", None)
                } else {
                    ("NEW", "icons/branch.svg", Some("N"))
                };
                cell(id, name, icon, key, on, true, None)
            },
        );
        let [organizer, new] = launch;
        let (x, y) = at(0, 1);
        side = side.child(new.absolute().left(px(x)).top(px(y)));
        let (x, y) = at(1, 1);
        side = side.child(organizer.absolute().left(px(x)).top(px(y)));
        let (x, y) = at(2, 1);
        side = side.child(
            cell(
                "cmd-list",
                "LIST",
                "icons/mail.svg",
                Some("L"),
                list_open && on_map,
                true,
                (waits_n > 0).then(|| waits_n.to_string()),
            )
            .absolute()
            .left(px(x))
            .top(px(y))
            .on_click(cx.listener(|this, _, _, cx| {
                this.go(Screen::WarRoom, cx);
                this.war_room.update(cx, |w, cx| w.rts_toggle_list(cx));
            })),
        );
        // rows 3 and 4: the screens
        for (k, (id, name, icon, screen)) in [
            ("cmd-war", "MAP", "icons/warroom.svg", Screen::WarRoom),
            ("cmd-inbox", "INBOX", "icons/inbox.svg", Screen::Inbox),
            ("cmd-tasks", "TASKS", "icons/tasks.svg", Screen::Tasks),
            ("cmd-repos", "REPOS", "icons/repos.svg", Screen::Repos),
            ("cmd-set", "SETUP", "icons/settings.svg", Screen::Settings),
        ]
        .into_iter()
        .enumerate()
        {
            let (x, y) = at(k % 3, 2 + k / 3);
            let badge = (screen == Screen::Inbox)
                .then(|| war.as_ref().map_or(0, |w| w.inbox_open))
                .filter(|n| *n > 0)
                .map(|n| n.to_string());
            side = side.child(
                cell(id, name, icon, None, here == screen, true, badge)
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .on_click(cx.listener(move |this, _, _, cx| this.go(screen.clone(), cx))),
            );
        }
        let (x, y) = at(2, 3);
        side = side.child(
            cell(
                "cmd-find",
                "FIND",
                "icons/search.svg",
                Some("/"),
                false,
                true,
                None,
            )
            .absolute()
            .left(px(x))
            .top(px(y))
            .on_click(cx.listener(|this, _, _, cx| {
                this.go(Screen::WarRoom, cx);
                this.war_room.update(cx, |w, cx| w.rts_find_open(cx));
            })),
        );
        let wt = grid_y + 3. * STEP + CELL;
        let ram_top = (vh - 106.).max(wt + 12.);
        // RAM
        let (used, total) = self
            .war_room
            .read(cx)
            .memory()
            .filter(|m| m.total_mb > 0)
            .map_or((0., 0.), |m| {
                (m.used_mb as f32 / 1024., m.total_mb as f32 / 1024.)
            });
        side.child(
            well(24., ram_top, 272., 82.)
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(cmd::label("RAM"))
                        .child(cmd::data(
                            format!("{used:.1} / {total:.0} GB"),
                            cmd::S_LABEL,
                        )),
                )
                .child(cmd::bar(if total > 0. { used / total } else { 0. })),
        )
    }
}
