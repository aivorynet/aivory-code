//! The Navigator skin of the war room: a 3D file-system landscape in the style of early-90s
//! workstation software. The fleet is the root pedestal; each codebase with live sessions is
//! a pedestal in the tree behind it; each part is a block on its pedestal (height = activity,
//! color = state). Agents are small cubes that glide along the ground lines to the block they
//! work on. A click selects and flies the camera there; drag orbits a little; the wheel zooms.
//! Beveled grey windows hold the toolbar (path, counters, dispatch, skin menu), the Needs you
//! list, the selected item card and the overview inset.

mod camera;
pub(crate) mod panels;
mod scene;
mod tree;
mod windows;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use aivory_protocol::{Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollWheelEvent, SharedString, Window, canvas, div, px, rgb,
};

use self::camera::{FLY, Pose};
use self::panels::{OUTLINE, TEXT, TITLE, bevel, button, sunken};
use self::scene::{Aim, Cube, Hits, Look, Picked, Scene};
use self::tree::{Tree, layout, length, point_at, splice};
use self::windows::{Panels, WinState};
use crate::theme::*;
use crate::ui;
use crate::warroom::skins::SKINS;
use crate::warroom::skins::common::{Pulse, TextCache, asks, live, next_ask, part_of};
use crate::warroom::{Overlay, WarRoomScreen};

/// The glide speed of an agent cube, in world units per second.
const SPEED: f32 = 280.;
/// The fade-in of the spotlight, in seconds.
const SPOT_IN: f32 = 0.35;
/// The zoom at the start and after a flight: a little nearer than the whole tree.
const START_ZOOM: f32 = -0.2;
/// A press that moves less than this is a click, not a drag, in pixels.
const CLICK: f32 = 4.;

/// What the user picked: a pedestal, a block, and maybe one session in it.
#[derive(Debug, Clone, PartialEq)]
struct Pick {
    codebase: String,
    part: Option<String>,
    session: Option<String>,
}

/// What the camera follows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum View {
    #[default]
    Home,
    Pick,
}

/// A drag of the mouse on the landscape.
struct Drag {
    last: [f32; 2],
    moved: f32,
    /// The right button turns and tilts; the left button pans.
    orbit: bool,
}

/// An agent cube on its way: the tree route to its block, the walk it is on, and how far.
struct Glide {
    route: Vec<[f32; 3]>,
    path: Vec<[f32; 3]>,
    s: f32,
}

/// The state of the Navigator skin that lives between frames.
#[derive(Default)]
pub(in crate::warroom) struct NavigatorState {
    pick: Option<Pick>,
    view: View,
    /// The pose that a flight starts from, and when.
    fly: Option<(Pose, Instant)>,
    /// The drag orbit (turn, tilt) and the log of the wheel zoom.
    orbit: [f32; 2],
    zoom: f32,
    /// The walk of the camera over the ground (world units, the shown value and the goal), and
    /// the goal of the turn. Both ease toward the goal.
    walk: [f32; 2],
    walk_to: [f32; 2],
    turn_to: f32,
    drag: Option<Drag>,
    started: Option<Instant>,
    last: Option<Instant>,
    /// The shown block heights, by codebase and part.
    heights: HashMap<(String, String), f32>,
    glides: HashMap<String, Glide>,
    primed: bool,
    pulse: Pulse,
    menu: bool,
    /// The spotlit block and when the spotlight came on.
    spot: Option<((String, String), Instant)>,
    /// The windows around the 3D view.
    panels: Panels,
    scene: Option<Rc<Scene>>,
    hits: Rc<RefCell<Hits>>,
    cache: Rc<RefCell<TextCache>>,
}

/// The look of a session.
fn look_of(s: &Pane) -> Look {
    match s.state {
        SessionState::NeedsYou => Look::Wait,
        SessionState::Error => Look::Error,
        SessionState::Working => Look::Work,
        _ => Look::Idle,
    }
}

/// The height of a block from the tokens of its sessions; a flat tile when nobody works there.
fn block_height(look: Look, tokens: u64) -> f32 {
    if look == Look::Empty {
        return 8.;
    }
    (26. + 20. * (1. + tokens as f32 / 1500.).ln()).min(130.)
}

impl WarRoomScreen {
    /// The whole war room area as the Navigator.
    pub(in crate::warroom) fn render_navigator(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let st = &mut self.navigator;
        let t = (now - *st.started.get_or_insert(now)).as_secs_f32();
        let dt = st.last.map_or(0.03, |l| (now - l).as_secs_f32()).min(0.1);
        st.last = Some(now);
        st.pulse.observe(war, now);
        st.panels.observe(war, now);
        // The walk and the turn ease toward their goals.
        let ease = 1. - (-dt * 9.).exp();
        for i in 0..2 {
            st.walk[i] += (st.walk_to[i] - st.walk[i]) * ease;
            if (st.walk_to[i] - st.walk[i]).abs() < 0.05 {
                st.walk[i] = st.walk_to[i];
            }
        }
        st.orbit[0] += (st.turn_to - st.orbit[0]) * ease;
        if (st.turn_to - st.orbit[0]).abs() < 0.0005 {
            st.orbit[0] = st.turn_to;
        }
        if !st.primed {
            st.panels.screenshot_hooks(war);
            st.zoom = START_ZOOM;
            if let Some(z) = std::env::var("AIVORY_NAV_ZOOM")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
            {
                st.zoom = z;
            }
        }

        // The landscape: a pedestal per codebase with live sessions, a block per part.
        let live = live(war);
        let mut input: Vec<(String, Vec<String>)> = Vec::new();
        // A pedestal per codebase with live sessions, a tower per session (its id is the name).
        for cb in &war.codebases {
            if live.iter().any(|s| s.codebase == cb.id) {
                input.push((cb.id.clone(), Vec::new()));
            }
        }
        for s in &live {
            if !input.iter().any(|(id, _)| *id == s.codebase) {
                input.push((s.codebase.clone(), Vec::new()));
            }
            if let Some((_, towers)) = input.iter_mut().find(|(id, _)| *id == s.codebase) {
                towers.push(s.id.clone());
            }
        }
        let names: HashMap<String, String> =
            live.iter().map(|s| (s.id.clone(), part_of(s))).collect();
        let tree = layout(&input);
        let nb = tree.blocks.len();
        let mut looks = vec![Look::Empty; nb];
        let mut tokens = vec![0u64; nb];
        let mut here: Vec<Vec<&Pane>> = vec![Vec::new(); nb];
        for s in &live {
            if let Some(b) = tree.block(&s.codebase, &s.id) {
                looks[b] = looks[b].max(look_of(s));
                tokens[b] += s.tokens;
                here[b].push(s);
            }
        }
        for list in &mut here {
            list.sort_by(|a, b| look_of(b).cmp(&look_of(a)).then(a.since.cmp(&b.since)));
        }
        let mut counts = vec![0usize; tree.pedestals.len()];
        for s in &live {
            if let Some(p) = tree.pedestal(&s.codebase) {
                counts[p] += 1;
            }
        }
        let mut alert = vec![false; tree.pedestals.len()];
        for (b, look) in looks.iter().enumerate() {
            if *look == Look::Wait {
                alert[tree.blocks[b].pedestal] = true;
            }
        }

        // The block heights ease toward the activity.
        let k = 1. - (-dt * 4.).exp();
        let mut heights = Vec::with_capacity(nb);
        for (bi, b) in tree.blocks.iter().enumerate() {
            let key = (tree.pedestals[b.pedestal].id.clone(), b.name.clone());
            let goal = block_height(looks[bi], tokens[bi]);
            let primed = st.primed;
            let shown = st
                .heights
                .entry(key)
                .or_insert(if primed { 8. } else { goal });
            *shown += (goal - *shown) * k;
            if (goal - *shown).abs() < 0.2 {
                *shown = goal;
            }
            heights.push(*shown);
        }

        // The pick: the user's, else the longest wait (shown, but the camera stays).
        let queue = asks(war);
        if st
            .pick
            .as_ref()
            .is_some_and(|p| tree.pedestal(&p.codebase).is_none())
        {
            st.pick = None;
        }
        let pick = st.pick.clone().or_else(|| {
            queue.first().map(|p| Pick {
                codebase: p.codebase.clone(),
                part: Some(part_of(p)),
                session: Some(p.id.clone()),
            })
        });
        let sel_pedestal = pick.as_ref().and_then(|p| tree.pedestal(&p.codebase));
        let sel_block = pick
            .as_ref()
            .and_then(|p| tree.block(&p.codebase, p.session.as_deref()?));
        let spot_key = sel_block.map(|b| {
            (
                tree.pedestals[tree.blocks[b].pedestal].id.clone(),
                tree.blocks[b].name.clone(),
            )
        });
        if st.spot.as_ref().map(|(k, _)| k) != spot_key.as_ref() {
            st.spot = spot_key.map(|k| (k, now));
        }
        let spot = st.spot.as_ref().map_or(0., |(_, at)| {
            let k = ((now - *at).as_secs_f32() / SPOT_IN).min(1.);
            1. - (1. - k) * (1. - k)
        });

        // The agent cubes glide along the tree to their blocks.
        let mut cubes = Vec::with_capacity(live.len());
        for (bi, list) in here.iter().enumerate() {
            for (slot, s) in list.iter().enumerate() {
                let route = tree.route(bi, slot);
                let primed = st.primed;
                let g = st.glides.entry(s.id.clone()).or_insert_with(|| Glide {
                    s: if primed { 0. } else { length(&route) },
                    path: route.clone(),
                    route: route.clone(),
                });
                if g.route != route {
                    let end = length(&g.route);
                    let mut walk = splice(&g.route, end, &route);
                    if g.s < length(&g.path) {
                        walk[0] = point_at(&g.path, g.s);
                    }
                    g.path = walk;
                    g.route = route;
                    g.s = 0.;
                }
                let total = length(&g.path);
                let left = total - g.s;
                if left > 0. {
                    let pace = (left / 80.).clamp(0.25, 1.);
                    g.s = (g.s + SPEED * pace * dt).min(total);
                }
                let mut pos = point_at(&g.path, g.s);
                if g.s >= total && look_of(s) == Look::Work {
                    pos[2] += 1.5 + 1.5 * (t * 2.2 + slot as f32 * 1.7).sin();
                }
                cubes.push(Cube {
                    id: s.id.clone(),
                    pos,
                    look: look_of(s),
                });
            }
        }
        st.glides.retain(|id, _| live.iter().any(|s| s.id == *id));
        st.primed = true;

        // The camera: a flight of 0.8 s toward what it follows.
        let fly = st.fly.and_then(|(from, at)| {
            let k = (now - at).as_secs_f32() / FLY;
            (k < 1.).then_some((from, k))
        });
        if fly.is_none() {
            st.fly = None;
        }
        let aim = match st.view {
            View::Home => Aim::Home,
            View::Pick => match (sel_block, sel_pedestal) {
                (Some(b), _) if st.pick.is_some() => Aim::Block(b),
                (_, Some(p)) if st.pick.is_some() => Aim::Pedestal(p),
                _ => Aim::Home,
            },
        };
        let scene = Rc::new(Scene {
            tree,
            looks,
            heights,
            cubes,
            names,
            counts,
            alert,
            aim,
            fly,
            orbit: st.orbit,
            zoom: st.zoom,
            walk: st.walk,
            sel_block,
            sel_pedestal,
            spot,
            t,
            hits: st.hits.clone(),
            cache: st.cache.clone(),
        });
        st.scene = Some(scene.clone());
        crate::ui::animate(window, cx);

        // The card: the picked session, else the first one of the block.
        let card_session = pick.as_ref().and_then(|p| {
            p.session
                .as_ref()
                .and_then(|id| live.iter().find(|s| s.id == *id).copied())
                .or_else(|| live.iter().find(|s| s.codebase == p.codebase).copied())
        });

        let sel_session = card_session.or_else(|| live.first().copied());
        let t = scene.t;
        let fsn = self.nav_fsn(war, (pick.as_ref(), &queue, scene.clone(), t), cx);
        let locks = self.nav_locks(war, cx);
        let power = self.nav_power(war, cx);
        let selected = self.nav_selected(war, sel_session, t, cx);
        let osview = self.nav_osview(war, now, cx);
        let log = self.nav_log(war, t, cx);
        let dialog = self.nav_dialog(war, &queue, cx);

        let look_menu = self.nav_look_menu(cx);
        let mut wins = vec![
            ("fsn", fsn),
            ("locks", locks),
            ("power", power),
            ("osview", osview),
            ("log", log),
            ("sel", selected),
        ];
        // A maximized window goes on top.
        wins.sort_by_key(|(k, _)| self.navigator.panels.state_of(k) == WinState::Max);
        let win_menu = self.nav_win_menu(cx);
        div()
            .size_full()
            .relative()
            .bg(rgb(0x22384A))
            .children(wins.into_iter().map(|(_, w)| w))
            .children(look_menu)
            .children(win_menu)
            .children(dialog)
            .into_any_element()
    }

    /// Zoom by `d` (log scale; negative is nearer).
    fn nav_zoom(&mut self, d: f32) {
        let st = &mut self.navigator;
        st.zoom = (st.zoom + d).clamp(-1.4, 0.6);
    }

    /// Move the camera over the ground: `fwd` along the view direction, `side` to the right.
    /// Both are fractions of the camera distance. The goal moves; the view eases there.
    fn nav_walk(&mut self, fwd: f32, side: f32) {
        let st = &mut self.navigator;
        let (yaw, dist) = st
            .hits
            .borrow()
            .pose
            .map_or((0., 1200.), |p| (p.yaw, p.dist));
        let limit = st.scene.as_ref().map_or(1500., |s| {
            let ps = &s.tree.pedestals;
            let lo = ps
                .iter()
                .fold([f32::MAX; 2], |a, p| [a[0].min(p.x), a[1].min(p.y)]);
            let hi = ps.iter().fold([f32::MIN; 2], |a, p| {
                [a[0].max(p.x + p.w), a[1].max(p.y + p.d)]
            });
            ((hi[0] - lo[0]).max(hi[1] - lo[1]) + 600.).max(900.)
        });
        let (sn, cs) = (yaw.sin(), yaw.cos());
        let (f, r) = (fwd * dist, side * dist);
        st.walk_to[0] = (st.walk_to[0] + sn * f + cs * r).clamp(-limit, limit);
        st.walk_to[1] = (st.walk_to[1] + cs * f - sn * r).clamp(-limit, limit);
    }

    /// Turn the camera by `d` radians (eased).
    fn nav_turn(&mut self, d: f32) {
        let st = &mut self.navigator;
        st.turn_to = (st.turn_to + d).clamp(-std::f32::consts::PI, std::f32::consts::PI);
    }

    /// Back to the default camera: no walk, no turn, the start zoom, the whole tree.
    fn nav_home(&mut self, cx: &mut Context<Self>) {
        self.nav_select(None, cx);
    }

    /// The keys move the camera: W/S and Up/Down fly forward and back, A/D slide, Left/Right
    /// turn, + and - zoom, Home returns to the default camera.
    pub(in crate::warroom) fn nav_key(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "+" | "=" => self.nav_zoom(-0.3),
            "-" | "_" => self.nav_zoom(0.3),
            "w" | "up" => self.nav_walk(0.14, 0.),
            "s" | "down" => self.nav_walk(-0.14, 0.),
            "a" => self.nav_walk(0., -0.14),
            "d" => self.nav_walk(0., 0.14),
            "left" => self.nav_turn(-0.12),
            "right" => self.nav_turn(0.12),
            "home" => {
                self.nav_home(cx);
                return;
            }
            _ => return,
        }
        cx.notify();
    }

    /// Fly the camera to what it should follow; the drag orbit and the zoom start over.
    fn nav_fly(&mut self, view: View) {
        let st = &mut self.navigator;
        if let Some(from) = st.hits.borrow().pose {
            st.fly = Some((from, Instant::now()));
        }
        st.orbit = [0.; 2];
        st.walk = [0.; 2];
        st.walk_to = [0.; 2];
        st.turn_to = 0.;
        st.zoom = START_ZOOM;
        st.view = view;
    }

    /// Pick something and fly there.
    fn nav_select(&mut self, pick: Option<Pick>, cx: &mut Context<Self>) {
        let view = if pick.is_some() {
            View::Pick
        } else {
            View::Home
        };
        self.navigator.pick = pick;
        self.nav_fly(view);
        cx.notify();
    }

    /// Pick a session and fly to its block.
    fn nav_select_session(&mut self, s: &Pane, cx: &mut Context<Self>) {
        self.nav_select(
            Some(Pick {
                codebase: s.codebase.clone(),
                part: Some(part_of(s)),
                session: Some(s.id.clone()),
            }),
            cx,
        );
    }

    /// A click on the landscape.
    fn nav_picked(&mut self, hit: Picked, cx: &mut Context<Self>) {
        let Some(scene) = self.navigator.scene.clone() else {
            return;
        };
        let tree: &Tree = &scene.tree;
        let pick = match hit {
            Picked::Cube(id) => {
                let Some(s) = self.war.as_ref().and_then(|w| w.session(&id)).cloned() else {
                    return;
                };
                self.nav_open_ask(&s);
                self.nav_select_session(&s, cx);
                return;
            }
            Picked::Block(b) => {
                let blk = &tree.blocks[b];
                if let Some(s) = self
                    .war
                    .as_ref()
                    .and_then(|w| w.session(&blk.name))
                    .cloned()
                {
                    self.nav_open_ask(&s);
                }
                Some(Pick {
                    codebase: tree.pedestals[blk.pedestal].id.clone(),
                    part: scene.names.get(&blk.name).cloned(),
                    session: Some(blk.name.clone()),
                })
            }
            Picked::Group(p) => {
                let codebase = tree.pedestals[p].id.clone();
                self.navigator.panels.group = Some(codebase.clone());
                let first = self.war.as_ref().and_then(|w| {
                    let mut list: Vec<&Pane> = live(w)
                        .into_iter()
                        .filter(|s| s.codebase == codebase)
                        .collect();
                    list.sort_by_key(|s| (s.ask.is_none(), s.since));
                    list.first().map(|s| (*s).clone())
                });
                match first {
                    Some(s) => self.nav_select_session(&s, cx),
                    None => cx.notify(),
                }
                return;
            }
            Picked::Pedestal(0) => None,
            Picked::Pedestal(p) => Some(Pick {
                codebase: tree.pedestals[p].id.clone(),
                part: None,
                session: None,
            }),
        };
        self.nav_select(pick, cx);
    }

    /// The look switch of the Toolchest: a list of the looks under the Look entry.
    fn nav_look_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.navigator.menu {
            return None;
        }
        let current = self.skin();
        let mut list = div().flex().flex_col().p(px(2.)).gap(px(2.));
        for (i, (s, name)) in SKINS.iter().enumerate() {
            let s = *s;
            let on = s == current;
            list = list.child(
                div()
                    .id(SharedString::from(format!("nav-skin-{i}")))
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .when(on, |d| d.bg(rgb(TITLE)).text_color(rgb(0xFFFFFF)))
                    .when(!on, |d| d.hover(|h| h.bg(rgb(panels::BTN_HOT))))
                    .child(format!("Look: {name}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigator.menu = false;
                        this.set_skin(s, cx);
                    })),
            );
        }
        Some(
            div()
                .absolute()
                .top(px(2.))
                .right(px(8.))
                .w(px(190.))
                .occlude()
                .border_1()
                .border_color(rgb(OUTLINE))
                .child(bevel(true, list.bg(rgb(panels::BTN)))),
        )
    }

    /// The fsn window: the 3D view of the fleet, or the camera wall of a group.
    fn nav_fsn(
        &self,
        war: &WarRoom,
        view: (Option<&Pick>, &[&Pane], Rc<Scene>, f32),
        cx: &mut Context<Self>,
    ) -> Div {
        let (pick, queue, scene, t) = view;
        let group = self.navigator.panels.group.clone();
        let path = windows::path_of(pick, group.as_deref());
        if let Some(cb) = &group {
            let n = live(war).iter().filter(|s| s.codebase == *cb).count();
            let wall = self.nav_wall(war, cb, t, cx);
            return self.win(
                cx,
                "fsn",
                format!("Live views — {path} · {n} sessions"),
                wall,
            );
        }
        let item =
            |id: &'static str, label: &'static str| div().id(id).cursor_pointer().child(label);
        let menu = div()
            .flex()
            .gap(px(18.))
            .px(px(10.))
            .py(px(3.))
            .text_size(ui::ts(T_SMALL))
            .border_b_1()
            .border_color(rgb(0x8A8A8A))
            .child(
                item("nav-m-file", "File").on_click(cx.listener(|this, _, _, cx| {
                    this.navigator.menu = !this.navigator.menu;
                    cx.notify();
                })),
            )
            .child(
                item("nav-m-view", "View").on_click(cx.listener(|this, _, _, cx| {
                    this.nav_select(None, cx);
                })),
            )
            .child(
                item("nav-m-session", "Session").on_click(cx.listener(|this, _, _, cx| {
                    this.toggle(Overlay::NewSession, cx);
                })),
            )
            // TODO(daemon): Locks and Help have no function yet; they only show the mockup.
            .child(div().child("Locks"))
            .child(div().child("Help"));
        let paint_scene = scene.clone();
        let empty = scene.cubes.is_empty() && scene.tree.blocks.is_empty();
        let viewport = div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0x000000))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    let p = [f32::from(ev.position.x), f32::from(ev.position.y)];
                    this.focus.focus(window, cx);
                    this.navigator.drag = Some(Drag {
                        last: p,
                        moved: 0.,
                        orbit: false,
                    });
                    if this.navigator.menu {
                        this.navigator.menu = false;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, _| {
                let st = &mut this.navigator;
                let Some(d) = st.drag.as_mut() else { return };
                if ev.pressed_button.is_none() {
                    st.drag = None;
                    return;
                }
                let p = [f32::from(ev.position.x), f32::from(ev.position.y)];
                let (dx, dy) = (p[0] - d.last[0], p[1] - d.last[1]);
                d.moved += dx.abs() + dy.abs();
                d.last = p;
                if d.moved < CLICK {
                    return;
                }
                if d.orbit {
                    st.turn_to = (st.turn_to + dx * 0.004)
                        .clamp(-std::f32::consts::PI, std::f32::consts::PI);
                    st.orbit[1] = (st.orbit[1] + dy * 0.003).clamp(-0.25, 0.5);
                } else {
                    // The left drag pans: the ground follows the mouse.
                    this.nav_walk(dy * 0.0016, -dx * 0.0016);
                    this.navigator.walk[0] = this.navigator.walk_to[0];
                    this.navigator.walk[1] = this.navigator.walk_to[1];
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    let p = [f32::from(ev.position.x), f32::from(ev.position.y)];
                    this.focus.focus(window, cx);
                    this.navigator.drag = Some(Drag {
                        last: p,
                        moved: 0.,
                        orbit: true,
                    });
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    let Some(d) = this.navigator.drag.take() else {
                        return;
                    };
                    if d.moved >= CLICK {
                        return;
                    }
                    let p = [f32::from(ev.position.x), f32::from(ev.position.y)];
                    let hit = this.navigator.hits.borrow().pick(p);
                    if let Some(hit) = hit {
                        this.nav_picked(hit, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.navigator.drag = None;
                }),
            )
            // The wheel flies the camera forward (away from you) and back along the view.
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let dy = f32::from(ev.delta.pixel_delta(px(40.)).y);
                this.nav_walk(dy * 0.004, 0.);
                cx.notify();
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| scene::paint(&paint_scene, bounds, window, cx),
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .right(px(10.))
                    .flex()
                    .gap(px(4.))
                    .child(
                        button("nav-fwd", "▲", false).on_click(cx.listener(|this, _, _, cx| {
                            this.nav_walk(0.2, 0.);
                            cx.notify();
                        })),
                    )
                    .child(button("nav-back", "▼", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.nav_walk(-0.2, 0.);
                            cx.notify();
                        },
                    )))
                    .child(button("nav-left", "◀", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.nav_turn(-0.25);
                            cx.notify();
                        },
                    )))
                    .child(button("nav-right", "▶", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.nav_turn(0.25);
                            cx.notify();
                        },
                    )))
                    .child(button("nav-zoom-in", "+", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.nav_zoom(-0.3);
                            cx.notify();
                        },
                    )))
                    .child(button("nav-zoom-out", "−", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.nav_zoom(0.3);
                            cx.notify();
                        },
                    ))),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(ui::ts(T_HEADING))
                        .text_color(rgb(0xC9D6F0))
                        .child("No live agents"),
                )
            });
        let up = pick.and_then(|p| {
            p.part.as_ref().map(|_| Pick {
                codebase: p.codebase.clone(),
                part: None,
                session: None,
            })
        });
        let next = next_ask(queue, pick.and_then(|p| p.session.as_deref()));
        let bar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(6.))
            .text_size(ui::ts(T_SMALL))
            .child("Location")
            .child(
                sunken(
                    div()
                        .px(px(6.))
                        .py(px(2.))
                        .font_family(tk().mono)
                        .text_color(rgb(TEXT))
                        .child(div().truncate().child(path.clone())),
                )
                .flex_1()
                .min_w_0(),
            )
            .child(
                button("nav-home", "Home", false).on_click(cx.listener(|this, _, _, cx| {
                    this.nav_home(cx);
                })),
            )
            .child(
                button("nav-up", "Up", false).on_click(cx.listener(move |this, _, _, cx| {
                    this.nav_select(up.clone(), cx);
                })),
            )
            .child(
                button("nav-next", "Fly to next ask", false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let target = next
                            .as_ref()
                            .and_then(|id| this.war.as_ref().and_then(|w| w.session(id)).cloned());
                        if let Some(s) = target {
                            this.nav_select_session(&s, cx);
                        }
                    },
                )),
            );
        let body = div().relative().flex().flex_col().size_full();
        let body = body
            .child(menu)
            .child(viewport)
            .child(windows::legend())
            .child(bar);
        self.win(
            cx,
            "fsn",
            format!("fsn — {path} · click a pedestal to fly there"),
            body,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_grow_with_activity_and_stay_in_range() {
        assert_eq!(block_height(Look::Empty, 1_000_000), 8.);
        let low = block_height(Look::Work, 0);
        let high = block_height(Look::Work, 200_000);
        assert!(low >= 26. && high > low && high <= 130.);
    }
}
