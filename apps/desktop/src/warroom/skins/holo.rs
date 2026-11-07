//! The Holo look (boards `HoloBloom` and `HoloSession`): see-through glass panels over slow far
//! rings and a bloom of strands. Strand bundle = one session; strand length = tokens of the
//! session (relative to the largest); color = state (cyan and violet work, orange waits for
//! you, slate is idle); an arc on the ring = one codebase (orange when a session of it waits);
//! a light packet runs along the strands of working sessions only. The war room screen is in
//! `war`, the session view in `session`. The page (glow, grid, rings, frame) and the bar come
//! from the shell (`shell/holo.rs`), so this look paints no page of its own.

mod session;
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
        if st.open.as_ref().is_some_and(|id| war.session(id).is_none()) {
            st.open = None;
        }
        let open = st.open.clone();
        crate::ui::animate(window, cx);
        match open {
            Some(id) => self.holo_session(war, &id, window, cx),
            None => self.holo_overview(war, t, window, cx),
        }
    }

    /// Back to the overview (the bar calls it for "War room").
    pub fn holo_close(&mut self, cx: &mut Context<Self>) {
        if self.holo.open.take().is_some() {
            cx.notify();
        }
    }

    /// Keys of the Holo look. Overview: 1 to 9 answer the decision on screen. Session view:
    /// Alt+J and Alt+K show the next and the previous session of the switcher.
    pub(in crate::warroom) fn holo_key(&mut self, key: &str, alt: bool, cx: &mut Context<Self>) {
        let Some(war) = self.war.clone() else {
            return;
        };
        if let Some(open) = self.holo.open.clone() {
            if !alt {
                return;
            }
            let ids = session::switcher_ids(&war);
            let Some(at) = ids.iter().position(|id| *id == open) else {
                return;
            };
            let to = match key {
                "j" => (at + 1) % ids.len(),
                "k" => (at + ids.len() - 1) % ids.len(),
                _ => return,
            };
            self.holo.open = Some(ids[to].clone());
            cx.notify();
            return;
        }
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
