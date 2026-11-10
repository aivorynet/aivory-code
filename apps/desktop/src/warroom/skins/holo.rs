//! The Holo look (boards `HoloBloom` and `HoloSession`): see-through glass panels over slow far
//! rings and a bloom of strands. Strand bundle = one session; strand length = tokens of the
//! session (relative to the largest); color = state (cyan and violet work, orange waits for
//! you, slate is idle); an arc on the ring = one codebase (orange when a session of it waits);
//! a light packet runs along the strands of working sessions only. The war room screen is in
//! `war`, the session view in `session`. The page (glow, grid, rings, frame) and the bar come
//! from the shell (`shell/holo.rs`), so this look paints no page of its own.

mod synapse;
pub(super) use synapse::{Cache as SynapseCache, Orbit};
mod war;

use std::time::Instant;

use aivory_protocol::WarRoom;
use gpui::{AnyElement, Context, Window};

use super::common::asks;
use crate::warroom::WarRoomScreen;

impl WarRoomScreen {
    /// The Holo war room area: the overview, or the session view of the open session.
    pub(super) fn holo_render(
        &mut self,
        war: &WarRoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let st = &mut self.holo;
        let t = (now - *st.started.get_or_insert(now)).as_secs_f32();
        crate::ui::animate(window, cx);
        self.holo_overview(war, t, window, cx)
    }

    /// Keys of the Holo look: 1 to 9 answer the decision on screen.
    pub(in crate::warroom) fn holo_key(&mut self, key: &str, _alt: bool, cx: &mut Context<Self>) {
        let Some(war) = self.war.clone() else {
            return;
        };
        let Some(n) = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) else {
            return;
        };
        let queue = asks(&war);
        let pane = self
            .holo
            .pick
            .clone()
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
