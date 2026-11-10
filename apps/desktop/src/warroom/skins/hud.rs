//! What the Holo and Tactical looks share: the contacts (one per live session), the click
//! areas of a picture, the look state and the turn sizes. Holo draws its war room and session
//! view in `holo.rs`; Tactical draws its own in `tactical.rs`. Every control goes through `act`
//! (ADR 0022).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use aivory_protocol::{Pane, SessionState, WarRoom};
use gpui::{AnyElement, Context, Window};

use super::common::{Painter, TextCache, asks, clip, part_of};
use crate::warroom::WarRoomScreen;

/// A rectangle `[x, y, w, h]` in window pixels.
pub(super) type Rect = [f32; 4];

/// The picture of a look: paints on the canvas and records the click areas.
pub(super) type PaintFn = Box<dyn Fn(&mut Painter, Rect, &mut Vec<(Hit, Rect)>)>;

/// What a click on the picture does.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Hit {
    /// Show this session in the decision panel.
    Pick(String),
}

/// One live session as the looks draw it.
#[derive(Debug, Clone)]
pub(super) struct Contact {
    pub id: String,
    pub title: String,
    pub codebase: String,
    pub state: SessionState,
    pub tokens: u64,
    /// Minutes since the session last changed its state.
    pub minutes: f32,
    /// Tokens per minute, smoothed.
    pub rate: f32,
}

impl Contact {
    pub fn waits(&self) -> bool {
        matches!(self.state, SessionState::NeedsYou | SessionState::Error)
    }
}

/// The state of a look on the war room screen.
#[derive(Default)]
pub(in crate::warroom) struct HudState {
    pub(super) started: Option<Instant>,
    /// Per session: when, the tokens then, the smoothed tokens per minute.
    rates: HashMap<String, (Instant, u64, f32)>,
    pub(super) cache: Rc<RefCell<TextCache>>,
    /// The synapse layout of the Holo war room.
    pub(super) syn: super::holo::SynapseCache,
    /// The last state of each session, and the firings it caused (id, start time).
    /// Per session: the state, the tokens, and the time of its last firing.
    pub(super) seen: HashMap<String, (SessionState, u64, f32)>,
    pub(super) fires: Vec<(String, f32)>,
    /// The mouse control of the Holo synapse camera.
    pub(super) orbit: super::holo::Orbit,
    pub(super) hits: Rc<RefCell<Vec<(Hit, Rect)>>>,
    /// The session in the decision panel (the longest wait when `None`).
    pub(super) pick: Option<String>,
    /// The session in the Selected panel of the Holo war room (the busiest worker when `None`).
    pub(super) sel: Option<String>,
}

impl HudState {
    /// Update the token rates of the sessions.
    pub(super) fn observe(&mut self, war: &WarRoom, now: Instant) {
        for s in war.sessions.iter().filter(|s| s.state.is_live()) {
            let e = self
                .rates
                .entry(s.id.clone())
                .or_insert((now, s.tokens, 0.));
            let dt = (now - e.0).as_secs_f32();
            if dt >= 1. {
                let delta = s.tokens.saturating_sub(e.1) as f32;
                e.2 = e.2 * 0.7 + (delta / dt * 60.) * 0.3;
                e.0 = now;
                e.1 = s.tokens;
            }
        }
        self.rates
            .retain(|id, _| war.sessions.iter().any(|s| &s.id == id));
    }

    fn rate(&self, id: &str) -> f32 {
        self.rates.get(id).map_or(0., |e| e.2)
    }
}

fn rank(state: SessionState) -> u8 {
    match state {
        SessionState::NeedsYou | SessionState::Error => 0,
        SessionState::Working => 1,
        _ => 2,
    }
}

/// The live sessions grouped by codebase (waits first inside a group), and the codebases.
pub(super) fn contacts(st: &HudState, war: &WarRoom) -> (Vec<Contact>, Vec<String>) {
    let mut books: Vec<String> = Vec::new();
    for s in war.sessions.iter().filter(|s| s.state.is_live()) {
        if !books.contains(&s.codebase) {
            books.push(s.codebase.clone());
        }
    }
    let mut list = Vec::new();
    for b in &books {
        let mut group: Vec<&Pane> = war
            .sessions
            .iter()
            .filter(|s| s.state.is_live() && &s.codebase == b)
            .collect();
        group.sort_by_key(|s| (rank(s.state), s.since));
        for s in group {
            list.push(Contact {
                id: s.id.clone(),
                title: if s.title.is_empty() {
                    part_of(s)
                } else {
                    clip(&s.title, 30)
                },
                codebase: s.codebase.clone(),
                state: s.state,
                tokens: s.tokens,
                minutes: (war.now - s.since).max(0) as f32 / 60_000.,
                rate: st.rate(&s.id),
            });
        }
    }
    (list, books)
}

impl WarRoomScreen {
    pub(super) fn render_holo(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.holo_render(war, window, cx)
    }

    pub(super) fn render_tactical(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.tactical_render(war, window, cx)
    }

    /// Tactical: the keys 1 to 3 give the order of that number to the ask on screen.
    pub(in crate::warroom) fn hud_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(n) = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) else {
            return;
        };
        let Some(war) = self.war.clone() else {
            return;
        };
        let target = self.tactical.pick.clone();
        let queue = asks(&war);
        let pane = target
            .and_then(|id| queue.iter().find(|p| p.id == id).copied())
            .or_else(|| queue.first().copied());
        let Some((pane, ask)) = pane.and_then(|p| Some((p, p.ask.as_ref()?))) else {
            return;
        };
        if let Some(option) = ask.options.get(n - 1) {
            self.skin_answer(&pane.id, &ask.id, option, cx);
        }
    }
}
