//! The Neon skin of the war room (`NeonGrid.dc.html`): a 3D city with one district
//! per codebase and one tower per part. Every live session is a light. A session that needs
//! you sends up a beam with a flat tag at its top. The HUD panels float over the city: Waits
//! for you, the decision with its answers, Its last steps, the district board and Budget. The
//! app frame (`shell/neon.rs`) draws the header and the tabs.

mod city;
mod hud;
mod layout;
mod panels;
mod pins;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;

use aivory_protocol::{Pane, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, Window, canvas, div, rgb};

use self::city::{BG, BuildingPin, Hits, Light, Look, MIST, ORBITRON, Scene, top};
use self::hud::wait_code;
use self::layout::{BUILDING, layout};
use crate::theme::*;
use crate::ui;
use crate::warroom::WarRoomScreen;
use crate::warroom::decisions::decisions;
use crate::warroom::skins::common::{TextCache, kind_word, part_of};

/// The trail of a light that moves, in frames.
const TRAIL: usize = 26;
/// A click on a light counts within this distance, in pixels.
const PICK: f32 = 24.;
/// The length of the glitch after a change, in seconds.
const GLITCH: f32 = 0.35;

/// One district of the district board.
struct Dock {
    name: String,
    color: u32,
    /// `2 run · 1 wait · 1 idle`.
    state: String,
    asks: usize,
    /// The session that a click selects (the longest wait, else a busy one).
    pick: Option<String>,
}

/// The position of one light between frames.
struct Flight {
    pos: [f32; 3],
    trail: VecDeque<[f32; 3]>,
}

/// The state of the Neon that lives between frames.
#[derive(Default)]
pub(in crate::warroom) struct NeonState {
    /// The selected session (an agent light, a pin or a waits row).
    sel: Option<String>,
    started: Option<Instant>,
    last: Option<Instant>,
    flights: HashMap<String, Flight>,
    /// The values of the animated bars, by key; they ease to their target.
    ease: HashMap<String, f32>,
    /// The selection and the wait queue of the last frame; a change starts a glitch.
    sig: u64,
    glitch_at: Option<Instant>,
    /// The shaped text that lives between frames.
    cache: Rc<RefCell<TextCache>>,
    /// The click targets, written by the canvas.
    hits: Rc<RefCell<Hits>>,
}

impl NeonState {
    /// Ease the bar `key` toward `target` (0 to 1) and give its value for this frame.
    fn eased(&mut self, key: &str, target: f32, dt: f32) -> f32 {
        let v = self.ease.entry(key.to_string()).or_insert(0.);
        *v += (target - *v) * (1. - (-dt * 4.).exp());
        *v
    }
}

impl WarRoomScreen {
    /// The whole war room area as the Neon.
    pub(in crate::warroom) fn render_neon(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let st = &mut self.neon;
        let t = (now - *st.started.get_or_insert(now)).as_secs_f32();
        let dt = st.last.map_or(0.04, |l| (now - l).as_secs_f32()).min(0.1);
        st.last = Some(now);

        // The city: a district per codebase with live sessions, a building per part.
        let live: Vec<&Pane> = war.sessions.iter().filter(|s| s.state.is_live()).collect();
        let mut input: Vec<(String, Vec<String>)> = Vec::new();
        for cb in &war.codebases {
            if live.iter().any(|s| s.codebase == cb.id) {
                input.push((cb.id.clone(), cb.part_names().map(String::from).collect()));
            }
        }
        for s in &live {
            if !input.iter().any(|(id, _)| *id == s.codebase) {
                input.push((s.codebase.clone(), Vec::new()));
            }
        }
        for s in &live {
            let part = part_of(s);
            if let Some((_, parts)) = input.iter_mut().find(|(id, _)| *id == s.codebase)
                && !parts.contains(&part)
            {
                parts.push(part);
            }
        }
        // A stable order (by codebase id and part name): a tower moves only when a session
        // starts or ends, never because the model lists its sessions in another order.
        input.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, parts) in &mut input {
            parts.sort();
            parts.dedup();
        }
        let city = layout(&input);
        let count = city.buildings.len();
        let (mut hot, mut fault) = (vec![false; count], vec![false; count]);
        let mut crowd = vec![0usize; count];
        let mut load = vec![(0usize, 0usize); city.districts.len()];
        let home = |s: &Pane| city.building(&s.codebase, &part_of(s));
        for s in &live {
            let Some(b) = home(s) else { continue };
            let d = city.buildings[b].district;
            load[d].1 += 1;
            match s.state {
                SessionState::Error => fault[b] = true,
                SessionState::Idle => {}
                _ => hot[b] = true,
            }
            if s.state != SessionState::Idle {
                crowd[b] += 1;
                load[d].0 += 1;
            }
        }

        // The selection: a live session, else the longest wait.
        let waits = decisions(war);
        let selected = st
            .sel
            .clone()
            .filter(|id| live.iter().any(|s| s.id == *id))
            .or_else(|| waits.first().map(|p| p.id.clone()));

        // A change of the selection or of the queue starts a short glitch.
        let mut h = DefaultHasher::new();
        selected.hash(&mut h);
        for p in &waits {
            p.id.hash(&mut h);
        }
        let sig = h.finish();
        if sig != st.sig {
            if st.sig != 0 {
                st.glitch_at = Some(now);
            }
            st.sig = sig;
        }
        let glitch = st
            .glitch_at
            .map_or(0., |at| (1. - (now - at).as_secs_f32() / GLITCH).max(0.));

        // The lights ease to their buildings. A light that moves leaves a trail.
        let ease = 1. - (-dt * 2.5).exp();
        let mut slot = vec![0usize; count];
        let mut lights = Vec::with_capacity(live.len());
        let mut sel_building = None;
        for s in &live {
            let Some(bi) = home(s) else { continue };
            let b = &city.buildings[bi];
            let k = slot[bi];
            slot[bi] += 1;
            let lift = top(b.h, hot[bi] || fault[bi]);
            let aim = [
                b.x + BUILDING / 2. + (k % 2) as f32 * 22. - if k > 0 { 11. } else { 0. },
                b.y + BUILDING / 2. - k as f32 * 16.,
                lift + 40. + k as f32 * 20.,
            ];
            let fl = st.flights.entry(s.id.clone()).or_insert_with(|| Flight {
                pos: aim,
                trail: VecDeque::new(),
            });
            let far = (0..3).map(|i| (aim[i] - fl.pos[i]).powi(2)).sum::<f32>() > 9.;
            if far {
                fl.trail.push_back(fl.pos);
                if fl.trail.len() > TRAIL {
                    fl.trail.pop_front();
                }
            } else {
                fl.trail.pop_front();
            }
            for (pos, to) in fl.pos.iter_mut().zip(aim) {
                *pos += (to - *pos) * ease;
            }
            let look = match s.state {
                SessionState::NeedsYou => Look::Beacon,
                SessionState::Error => Look::Error,
                SessionState::Idle => Look::Idle,
                _ => Look::Work,
            };
            let label = match (look, &s.ask) {
                (Look::Beacon | Look::Error, Some(a)) => Some(format!(
                    "{}  {}",
                    kind_word(a.kind).to_uppercase(),
                    wait_code(s.minutes(war.now))
                )),
                (Look::Error, None) => Some("ERROR".to_string()),
                _ => None,
            };
            let is_sel = selected.as_deref() == Some(s.id.as_str());
            if is_sel {
                sel_building = Some(bi);
            }
            lights.push(Light {
                id: s.id.clone(),
                pos: fl.pos,
                trail: fl.trail.iter().copied().collect(),
                look,
                base: lift,
                label,
                rank: waits
                    .iter()
                    .position(|p| p.id == s.id)
                    .unwrap_or(usize::MAX),
                selected: is_sel,
            });
        }
        st.flights.retain(|id, _| live.iter().any(|s| s.id == *id));

        // The pins of the lit buildings and of the selection.
        let mut pins = Vec::new();
        for (bi, b) in city.buildings.iter().enumerate() {
            let chosen = sel_building == Some(bi);
            if !(hot[bi] || fault[bi] || chosen) {
                continue;
            }
            let here: Vec<&&Pane> = live.iter().filter(|s| home(s) == Some(bi)).collect();
            let Some(first) = here
                .iter()
                .find(|s| s.state != SessionState::Idle)
                .or(here.first())
            else {
                continue;
            };
            let mut text = b.name.to_uppercase();
            if crowd[bi] > 1 {
                text = format!("{text} ×{}", crowd[bi]);
            }
            pins.push(BuildingPin {
                building: bi,
                text,
                id: first.id.clone(),
                selected: chosen,
            });
        }

        // The district board: per district who works, what happened last, the spend.
        let docks: Vec<Dock> = city
            .districts
            .iter()
            .enumerate()
            .map(|(di, d)| {
                let mine: Vec<&Pane> = live
                    .iter()
                    .copied()
                    .filter(|s| s.codebase == d.id)
                    .collect();
                let asking = waits.iter().filter(|p| p.codebase == d.id).count();
                let idle = mine
                    .iter()
                    .filter(|s| s.state == SessionState::Idle)
                    .count();
                let run = load[di].0.saturating_sub(asking);
                let mut parts = Vec::new();
                if run > 0 {
                    parts.push(format!("{run} run"));
                }
                if asking > 0 {
                    parts.push(format!("{asking} wait"));
                }
                if idle > 0 {
                    parts.push(format!("{idle} idle"));
                }
                let busy = mine.iter().find(|s| s.state != SessionState::Idle);
                let ask = waits.iter().find(|p| p.codebase == d.id);
                Dock {
                    name: d.id.to_uppercase(),
                    color: d.color,
                    state: if parts.is_empty() {
                        "—".to_string()
                    } else {
                        parts.join(" · ")
                    },
                    asks: asking,
                    pick: ask.copied().or(busy.copied()).map(|p| p.id.clone()),
                }
            })
            .collect();
        let target = selected
            .as_deref()
            .and_then(|id| live.iter().copied().find(|s| s.id == id));
        let scene = Scene {
            city,
            hot,
            fault,
            load,
            pins,
            lights,
            t,
            glitch,
            hits: st.hits.clone(),
            cache: st.cache.clone(),
        };
        crate::ui::animate(window, cx);
        let empty = live.is_empty();
        let panels = self.neon_panels(war, &waits, target, &docks, dt, cx);
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(BG))
            .cursor_crosshair()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                    let hits = this.neon.hits.borrow();
                    let pin = hits
                        .pins
                        .iter()
                        .rev()
                        .find(|(_, r)| {
                            x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
                        })
                        .map(|(id, _)| id.clone());
                    let best = pin.or_else(|| {
                        hits.lights
                            .iter()
                            .map(|(id, p)| (id, (p[0] - x).hypot(p[1] - y)))
                            .filter(|(_, d)| *d < PICK)
                            .min_by(|a, b| a.1.total_cmp(&b.1))
                            .map(|(id, _)| id.clone())
                    });
                    drop(hits);
                    if let Some(id) = best {
                        this.neon.sel = Some(id);
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| city::paint(&scene, bounds, window, cx),
                )
                .absolute()
                .inset_0(),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(ORBITRON)
                        .text_size(ui::ts(T_HEADING))
                        .text_color(rgb(MIST))
                        .child("NO LIVE AGENTS"),
                )
            })
            .children(panels)
            .into_any_element()
    }
}
