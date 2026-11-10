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
use crate::warroom::skins::command::{SIDE_W, rts_waits};

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
        CmdEvent::Close => this.go(Screen::WarRoom, cx),
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
            Screen::Settings => "SETUP".into(),
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
                        .bg(c(0x000000, 0.55))
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
        side = side.child(
            well(24., 22., 272., 40.)
                .bg(c(0x040704, 1.))
                .child(
                    cmd::data("AIV", 20.)
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .text_color(c(cmd::INK_3, 1.)),
                )
                .child(
                    cmd::data(led, 32.)
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
        // the screen buttons
        let inbox = war.as_ref().map_or(0, |w| w.inbox_open);
        for (id, name, screen, x, y, w) in [
            (
                "cmd-war",
                "WAR ROOM".to_string(),
                Screen::WarRoom,
                24.,
                290. - dy,
                134.,
            ),
            (
                "cmd-inbox",
                if inbox > 0 {
                    format!("INBOX {inbox}")
                } else {
                    "INBOX".into()
                },
                Screen::Inbox,
                162.,
                290. - dy,
                134.,
            ),
            (
                "cmd-tasks",
                "TASKS".into(),
                Screen::Tasks,
                24.,
                334. - dy,
                134.,
            ),
            (
                "cmd-repos",
                "REPOS".into(),
                Screen::Repos,
                162.,
                334. - dy,
                134.,
            ),
            (
                "cmd-set",
                "SETUP".into(),
                Screen::Settings,
                24.,
                378. - dy,
                272.,
            ),
        ] {
            side = side.child(
                cmd::plate(id, name, w, 40., here == screen)
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .on_click(cx.listener(move |this, _, _, cx| this.go(screen.clone(), cx))),
            );
        }
        // Organizer and New task: two plates under the screen buttons
        for (k, b) in crate::new_session::launch_buttons("command", |id, label| {
            cmd::plate(id, label.to_uppercase(), 134., 40., false)
        })
        .into_iter()
        .enumerate()
        {
            side = side.child(
                b.absolute()
                    .left(px(24. + k as f32 * 138.))
                    .top(px(422. - dy)),
            );
        }
        // NEW UNIT: the build tile of a new session
        side = side.child(
            div()
                .absolute()
                .left(px(24.))
                .top(px(478. - dy))
                .w(px(272.))
                .h(px(112.))
                .id("cmd-new")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.open_new_session(cx)))
                .child(
                    img(frame(Frame9::Button, 272., 112.))
                        .absolute()
                        .left_0()
                        .top_0()
                        .size_full(),
                )
                .child(
                    // the robot at its own size (no scaling), in a recessed slot
                    div()
                        .absolute()
                        .left(px(14.))
                        .top(px(12.))
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
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(94.))
                        .top(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(cmd::data("NEW UNIT", 30.).text_color(c(0xB6F28A, 1.)))
                        .child(cmd::label("A NEW SESSION"))
                        .child(cmd::label("IN ANY VILLAGE"))
                        .child(cmd::label("PRESS N").text_color(c(cmd::HINT, 1.))),
                ),
        );
        // the sessions that wait for you
        let wt = 602. - dy;
        let ram_top = (vh - 106.).max(wt + 132.);
        let waits_h = ram_top - 12. - wt;
        // As many rows as the well holds (the label takes about 40 px, a row 46 px).
        let rows = (((waits_h - 40.) / 46.).floor() as usize).clamp(1, 4);
        let mut waits = well(24., wt, 272., waits_h)
            .p(px(10.))
            .overflow_hidden()
            .flex()
            .flex_col();
        let asks: Vec<_> = war
            .as_ref()
            .map(|w| {
                rts_waits(w)
                    .iter()
                    .filter_map(|id| w.session(id).cloned())
                    .collect()
            })
            .unwrap_or_default();
        waits = waits.child(cmd::label(format!("WAITS FOR YOU · {}", asks.len())));
        if asks.is_empty() {
            waits = waits.child(
                cmd::label("ALL CLEAR · NO ORDERS WAIT")
                    .mt(px(12.))
                    .text_color(c(cmd::HINT, 1.)),
            );
        }
        let now = war.as_ref().map_or(0, |w| w.now);
        for (k, p) in asks.iter().take(rows).enumerate() {
            let id = p.id.clone();
            let fault = p.state == SessionState::Error;
            waits = waits.child(
                div()
                    .id(("cmd-wait", k))
                    .min_h(px(46.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .when(k > 0, |d| d.border_t_2().border_color(c(cmd::RULE, 1.)))
                    .cursor_pointer()
                    .hover(|st| st.bg(c(cmd::ROW_HOVER, 1.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.go(Screen::WarRoom, cx);
                        this.war_room.update(cx, |w, cx| w.rts_goto(&id, cx));
                    }))
                    .child(
                        cmd::entry(cmd::up(&p.title))
                            .flex_1()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis(),
                    )
                    .child(
                        cmd::entry(if fault {
                            "FAULT".to_string()
                        } else {
                            cmd::mins(now, p.since)
                        })
                        .flex_none()
                        .text_color(c(if fault { cmd::RED } else { cmd::AMBER }, 1.)),
                    ),
            );
        }
        side = side.child(waits);
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
                        .child(cmd::data(format!("{used:.1} / {total:.0} GB"), 18.)),
                )
                .child(cmd::bar(if total > 0. { used / total } else { 0. })),
        )
    }
}
