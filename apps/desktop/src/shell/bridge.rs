//! The app frame in the Bridge look (ADR 0022, mockups `BridgeIslands` and `BridgeDetail`): the
//! numbered segments 01 War room to 05 Settings are the only navigation of the app. The
//! frame is a few colored pieces on black; the Minimal rail and status bar do not show.
//! The war room and the session view are drawn over the whole window; every other screen
//! sits in a calm box beside the frame.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, SharedString, Window, div, px, rgb};

use super::{Screen, Shell, first_windows};
use crate::actions::{Action, act};
use crate::bridge_kit::*;
use crate::model::Link;

/// The three shapes of the frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The war room: the planet view draws the header; only the navigation is the frame's.
    Planet,
    /// The other main screens: the header bars; the screen draws its islands below.
    Overview,
    /// The session view: header bars, the session switcher, the bottom bars.
    Detail,
    /// Every other screen: a column, the bottom bars, and a box for the screen.
    Plain,
}

/// Makes the look button for a bar at x, y with width and height.
type LookMaker = Rc<dyn Fn(f32, f32, f32, f32) -> AnyElement>;

/// What the frame needs from the model, copied once per frame.
struct Facts {
    inbox: u32,
    status: String,
    idle: Vec<String>,
    /// The live sessions in switcher order: id, short name, color.
    switcher: Vec<(String, String, u32)>,
}

/// A segment of the war room board: label, top, height, color, screen.
type PlanetSeg = (&'static str, f32, f32, Option<u32>, Option<usize>);

/// The navigation of the war room board: segments on the swept frame, labels at the lower right.
fn planet_nav(art: Art, level: u8, inbox: u32, go: &NavFn, look: AnyElement) -> Div {
    let top = [0xFF9900, 0xCC99CC, 0xFF9966][usize::from(level.min(2))];
    let segs: [PlanetSeg; 6] = [
        ("AIV code", 0., 104., Some(top), Some(0)),
        ("01 War room", 108., 70., None, Some(0)),
        ("02 Inbox", 330., 48., None, Some(1)),
        ("03 Tasks", 382., 150., Some(0x9C9CFF), Some(2)),
        ("04 Repos", 536., 54., Some(0xFF9900), Some(3)),
        ("05 Settings", 594., 306. + art.dy, Some(0xFF9966), Some(4)),
    ];
    let mut out = div().absolute().inset_0();
    for (k, (name, y, h, color, to)) in segs.into_iter().enumerate() {
        let label = if k == 2 && inbox > 0 {
            format!("{name} {inbox:02}")
        } else {
            name.to_string()
        };
        let go = go.clone();
        let mut el = rect(0., y, 188., h, 0x000000)
            .id(("pl-nav", k))
            .flex()
            .justify_end()
            .pr(px(12.))
            .cursor_pointer()
            .font_family(LABEL)
            .text_size(px(18.))
            .line_height(px(24.))
            .text_color(rgb(0x000000))
            .whitespace_nowrap()
            .on_click(move |_, w, cx| {
                if let Some(i) = to {
                    go(i, w, cx);
                }
            })
            .child(label.to_uppercase());
        el = match color {
            Some(c) => el.bg(rgb(c)),
            None => el.bg(gpui::transparent_black()),
        };
        // The first war room label sits at the top of its segment, as in the board.
        el = if k == 1 || k == 5 {
            el.items_start().pt(px(6.))
        } else {
            el.items_end().pb(px(6.))
        };
        out = out.child(el);
    }
    out.child(look)
}

impl Shell {
    fn bridge_facts(&self, cx: &Context<Self>) -> Facts {
        let model = self.model.read(cx);
        let status = match &model.link {
            Link::Connecting => "aivoryd connecting".to_string(),
            Link::Running if model.demo => "aivoryd running · demo".to_string(),
            Link::Running => "aivoryd running".to_string(),
            Link::Offline(_) => "aivoryd offline".to_string(),
        };
        let Some(war) = &model.war else {
            return Facts {
                inbox: 0,
                status,
                idle: Vec::new(),
                switcher: Vec::new(),
            };
        };
        let live = order(war);
        let status = match first_windows(&war.usage).first() {
            Some(u) => format!("{status} · {} {}%", u.provider, u.pct),
            None => status,
        };
        Facts {
            inbox: war.inbox_open,
            status,
            idle: live
                .iter()
                .filter(|p| p.state == aivory_protocol::SessionState::Idle)
                .map(|p| p.id.clone())
                .collect(),
            switcher: live
                .iter()
                .map(|p| {
                    let name = if p.title.is_empty() { &p.id } else { &p.title };
                    (p.id.clone(), name.clone(), state_color(p))
                })
                .collect(),
        }
    }

    /// The whole Bridge window: the frame, the screen, the look switch, the notices.
    pub(super) fn bridge_screen(
        &mut self,
        content: AnyElement,
        toasts: Option<AnyElement>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let art = Art::of(window);
        let facts = self.bridge_facts(cx);
        let shape = match self.screen {
            Screen::WarRoom => Shape::Planet,
            Screen::Inbox | Screen::Tasks | Screen::Repos | Screen::Focus(_) | Screen::Lens => {
                Shape::Overview
            }
            Screen::Settings if !self.bridge_full_settings => Shape::Overview,
            Screen::Session(_) => Shape::Detail,
            _ => Shape::Plain,
        };
        // The session that the Detail entry opens: the open one, else the war room pick.
        let detail_id: Option<String> = match &self.screen {
            Screen::Session(id) => Some(id.clone()),
            _ => self.war_room.read(cx).bridge_selected(),
        };
        let weak = cx.entity().downgrade();
        let detail_go = detail_id.clone();
        let go: NavFn = Rc::new(move |i, _, cx| {
            if i == NAV_DETAIL {
                if let Some(id) = &detail_go {
                    let id = id.clone();
                    weak.update(cx, |s, cx| s.open_session(&id, cx)).ok();
                }
                return;
            }
            let screen = match i {
                0 => Screen::WarRoom,
                1 => Screen::Inbox,
                2 => Screen::Tasks,
                3 => Screen::Repos,
                _ => Screen::Settings,
            };
            weak.update(cx, |s, cx| s.go(screen, cx)).ok();
        });
        let active = match self.screen.rail() {
            Screen::WarRoom => Some(0),
            Screen::Inbox => Some(1),
            Screen::Tasks => Some(2),
            Screen::Repos => Some(3),
            Screen::Settings => Some(4),
            _ => None,
        };
        let active = if matches!(self.screen, Screen::Session(_)) {
            Some(NAV_DETAIL)
        } else {
            active
        };
        let at = match &self.screen {
            Screen::Session(id) => facts.switcher.iter().position(|s| &s.0 == id),
            _ => None,
        };
        let title = match (&self.screen, at) {
            (Screen::Session(_), Some(i)) => format!("Session {:02}", i + 1),
            (screen, _) => screen.label().to_string(),
        };

        let sub = match self.screen {
            Screen::Focus(_) => Some(0),
            Screen::Lens => Some(3),
            _ => None,
        };
        let level = self.war_room.read(cx).bridge_level();
        let mut frame: Vec<AnyElement> = if shape == Shape::Planet {
            Vec::new()
        } else {
            nav(active, sub, facts.inbox, detail_id.is_some(), &go)
        };
        let look = self.bridge_look_button(cx);
        match shape {
            Shape::Overview | Shape::Detail => {
                frame.extend(
                    upper_elbow(art)
                        .into_iter()
                        .map(IntoElement::into_any_element),
                );
                if shape == Shape::Overview {
                    frame.push(
                        bar_text(bar(art.rx(986.), 234., 316., PK), &facts.status)
                            .into_any_element(),
                    );
                    frame.push(look(art.rx(1306.), 234., 118., 16.));
                } else {
                    frame.push(bar(art.rx(986.), 234., 316., PK).into_any_element());
                }
            }
            Shape::Plain => {
                frame.push(rect(16., 188., 150., art.by(786.) - 188., PU).into_any_element());
            }
            Shape::Planet => {}
        }
        if shape == Shape::Detail {
            frame.push(
                bar(art.rx(1306.), 234., 118., PU)
                    .rounded_r(px(8.))
                    .into_any_element(),
            );
            frame.push(
                rect(16., 266., 262., 62., OR)
                    .rounded_tl(px(54.))
                    .into_any_element(),
            );
            frame.push(
                rect(166., 282., 115., 46., BLACK)
                    .rounded_tl(px(26.))
                    .into_any_element(),
            );
            frame.push(bar(282., 266., 760. + art.dx, OR).into_any_element());
            frame.push(bar(art.rx(1046.), 266., 230., PK).into_any_element());
            frame.push(
                bar(art.rx(1280.), 266., 144., OR)
                    .rounded_r(px(8.))
                    .into_any_element(),
            );
            frame.extend(self.bridge_switcher(art, &facts, at, cx));
        }
        if shape != Shape::Overview && shape != Shape::Planet {
            frame.extend(
                lower_elbow(art)
                    .into_iter()
                    .map(IntoElement::into_any_element),
            );
            frame.push(bar(282., art.by(868.), 520. + art.dx, PU).into_any_element());
            frame.push(self.bridge_park_bar(art, &facts, cx));
            frame.push(
                bar_text(bar(art.rx(1040.), art.by(868.), 234., PU), &facts.status)
                    .into_any_element(),
            );
            frame.push(look(art.rx(1278.), art.by(868.), 146., 16.));
        }

        let body = match shape {
            Shape::Planet | Shape::Overview | Shape::Detail => {
                div().absolute().inset_0().child(content)
            }
            Shape::Plain => div()
                .absolute()
                .left(px(190.))
                .top(px(64.))
                .right(px(16.))
                .bottom(px(44.))
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(content),
        };
        let name = div()
            .flex_none()
            .font_family(LABEL)
            .text_size(px(S_TITLE))
            .line_height(px(48.))
            .text_color(rgb(OR))
            .whitespace_nowrap()
            .child(title.to_uppercase());
        // A screen with a box gets a thin bar with a round cap in front of its title.
        let heading = if shape == Shape::Plain {
            div()
                .absolute()
                .left(px(190.))
                .right(px(16.))
                .top(px(6.))
                .h(px(48.))
                .flex()
                .items_center()
                .gap(px(14.))
                .child(div().flex_1().h(px(16.)).bg(rgb(PU)).rounded_l(px(8.)))
                .child(name)
        } else {
            div()
                .absolute()
                .right(px(16.))
                .top(px(6.))
                .h(px(48.))
                .child(name)
        };
        let planet = (shape == Shape::Planet).then(|| {
            planet_nav(
                art,
                level,
                facts.inbox,
                &go,
                look(art.rx(1379.), 232., 61., 22.),
            )
        });
        let menu = self.look_menu.then(|| {
            let y = if matches!(shape, Shape::Overview | Shape::Planet) {
                254.
            } else {
                art.by(832.)
            };
            self.bridge_look_menu(y, cx)
        });
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(BLACK))
            .font_family(BODY)
            .text_color(rgb(TXT))
            .children(frame)
            .child(body)
            .children((shape != Shape::Planet).then_some(heading))
            .children(planet)
            .children(menu)
            .children(toasts)
    }

    /// The look button of a bar: it opens the row of looks. Returns a maker, so the war room
    /// bar and the bottom bar can share it.
    fn bridge_look_button(&self, cx: &mut Context<Self>) -> LookMaker {
        let weak = cx.entity().downgrade();
        Rc::new(move |x, y, w, h| {
            let weak = weak.clone();
            let label = if w < 100. { "Look" } else { "Look · Bridge" };
            bar_text(rect(x, y, w, h, OR).rounded_r(px(8.)), label)
                .id("lc-look")
                .cursor_pointer()
                .hover(|s| s.bg(rgb(lit(OR))))
                .on_click(move |_, _, cx| {
                    weak.update(cx, |s, cx| {
                        s.look_menu = !s.look_menu;
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element()
        })
    }

    /// The row of looks above the look button.
    fn bridge_look_menu(&self, y: f32, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .absolute()
            .right(px(16.))
            .top(px(y))
            .flex()
            .gap(px(6.));
        for (look, name) in crate::look::LOOKS {
            let c = if look == crate::theme::tk().look {
                GD
            } else {
                OR
            };
            row = row.child(
                pill(format!("lc-look-{name}"), c, name).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.look_menu = false;
                        act(&this.model, Action::SetLook(look), cx);
                    },
                )),
            );
        }
        row.child(
            pill("lc-look-x", PK, "×").on_click(cx.listener(|this, _, _, cx| {
                this.look_menu = false;
                cx.notify();
            })),
        )
        .into_any_element()
    }

    /// The grey bar of the bottom: park every idle session.
    fn bridge_park_bar(&self, art: Art, facts: &Facts, cx: &mut Context<Self>) -> AnyElement {
        let n = facts.idle.len();
        let label = if n == 0 {
            "No idle session".to_string()
        } else {
            format!("{n:02} Idle · park all")
        };
        let ids = facts.idle.clone();
        bar_text(bar(art.rx(806.), art.by(868.), 230., GY), &label)
            .id("lc-park")
            .when(n > 0, |d| d.cursor_pointer().hover(|s| s.bg(rgb(lit(GY)))))
            .on_click(cx.listener(move |this, _, _, cx| {
                for id in &ids {
                    act(&this.model, Action::Park(id.clone()), cx);
                }
            }))
            .into_any_element()
    }

    /// The session switcher: one segment for every live session, as far as the column holds.
    fn bridge_switcher(
        &self,
        art: Art,
        facts: &Facts,
        at: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let room = (((art.by(780.) - 332.) / 32.).floor() as usize).max(1);
        let n = facts.switcher.len();
        let start = at
            .map_or(0, |i| i.saturating_sub(room / 2))
            .min(n.saturating_sub(room));
        let mut out = vec![rect(16., art.by(780.), 150., 6., PU).into_any_element()];
        for (i, (id, name, c)) in facts.switcher.iter().enumerate().skip(start).take(room) {
            let on = at == Some(i);
            let id = id.clone();
            let row = (i - start) as f32;
            out.push(
                seg(
                    SharedString::from(format!("lc-sw-{i}")),
                    (16., 332. + row * 32., 150., 28.),
                    if on { PA } else { *c },
                    S_SMALL,
                    &format!("{:02}", i + 1),
                    &short_name(name, 13),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.screen = Screen::Session(id.clone());
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        out
    }
}

impl Shell {
    /// The Bridge list screens replace Inbox, Tasks, Repos and Settings; the other screens
    /// keep their own content inside the frame.
    pub(super) fn bridge_content(
        &mut self,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::bridge_focus::{BridgeFocus, BridgeFocusEvent};
        use crate::bridge_lens::{BridgeLens, BridgeLensEvent};
        use crate::bridge_screens::*;
        fn handle(this: &mut Shell, event: &BridgeEvent, cx: &mut Context<Shell>) {
            match event {
                BridgeEvent::OpenSession(id) => this.open_session(id, cx),
                BridgeEvent::AllSettings => {
                    this.bridge_full_settings = true;
                    cx.notify();
                }
            }
        }
        let model = self.model.clone();
        let focus_id = match &self.screen {
            Screen::Focus(id) => Some(id.clone()),
            _ => None,
        };
        match self.screen {
            Screen::Focus(_) => {
                let id = focus_id.unwrap_or_default();
                if let Some((v, _)) = &self.bridge_screens.focus {
                    v.update(cx, |v, cx| v.show(&id, cx));
                } else {
                    let v = cx.new(|cx| BridgeFocus::new(model, id, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &BridgeFocusEvent, cx| match e {
                        BridgeFocusEvent::Open(id) => t.open_focus(id, cx),
                        BridgeFocusEvent::Detail(id) => t.open_session(id, cx),
                        BridgeFocusEvent::OpenFile { session, path } => {
                            t.open_session_file(session, path, cx)
                        }
                    });
                    self.bridge_screens.focus = Some((v, sub));
                }
                self.bridge_screens
                    .focus
                    .as_ref()
                    .map(|(v, _)| v.clone().into_any_element())
            }
            Screen::Lens => {
                if self.bridge_screens.lens.is_none() {
                    let v = cx.new(|cx| BridgeLens::new(model, window, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &BridgeLensEvent, cx| match e {
                        BridgeLensEvent::Back => {
                            let back = std::mem::replace(&mut t.lens_back, Screen::Repos);
                            t.go(back, cx);
                        }
                        BridgeLensEvent::OpenSession(id) => t.open_session(id, cx),
                    });
                    self.bridge_screens.lens = Some((v, sub));
                }
                let target = self.bridge_screens.lens_target.take();
                if let (Some((v, _)), Some((codebase, path, session))) =
                    (&self.bridge_screens.lens, target)
                {
                    v.update(cx, |v, cx| v.show(&codebase, &path, session, cx));
                }
                self.bridge_screens
                    .lens
                    .as_ref()
                    .map(|(v, _)| v.clone().into_any_element())
            }
            Screen::Inbox => {
                if self.bridge_screens.inbox.is_none() {
                    let v = cx.new(|cx| BridgeInbox::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &BridgeEvent, cx| handle(t, e, cx));
                    self.bridge_screens.inbox = Some((v, sub));
                }
                self.bridge_screens
                    .inbox
                    .as_ref()
                    .map(|(v, _)| v.clone().into_any_element())
            }
            Screen::Tasks => {
                if self.bridge_screens.tasks.is_none() {
                    let v = cx.new(|cx| BridgeTasks::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &BridgeEvent, cx| handle(t, e, cx));
                    self.bridge_screens.tasks = Some((v, sub));
                }
                self.bridge_screens
                    .tasks
                    .as_ref()
                    .map(|(v, _)| v.clone().into_any_element())
            }
            Screen::Repos => {
                if self.bridge_screens.repos.is_none() {
                    let v = cx.new(|cx| BridgeRepos::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &BridgeEvent, cx| handle(t, e, cx));
                    self.bridge_screens.repos = Some((v, sub));
                }
                self.bridge_screens
                    .repos
                    .as_ref()
                    .map(|(v, _)| v.clone().into_any_element())
            }
            Screen::Settings if !self.bridge_full_settings => {
                if self.bridge_screens.settings.is_none() {
                    let v = cx.new(|cx| BridgeSettings::new(model, cx));
                    let sub = cx.subscribe(&v, |t, _, e: &BridgeEvent, cx| handle(t, e, cx));
                    self.bridge_screens.settings = Some((v, sub));
                }
                self.bridge_screens
                    .settings
                    .as_ref()
                    .map(|(v, _)| v.clone().into_any_element())
            }
            _ => None,
        }
        .unwrap_or(content)
    }
}
