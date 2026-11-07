//! War room skins (2026-10-29). Each skin is its own interface on the same live data.
//! Minimal keeps the header, the board and the right column of `mod.rs`; the other skins
//! paint the whole war room area themselves.

mod board;
mod bridge;
pub(crate) mod command;
mod common;
mod holo;
mod hud;
pub(crate) mod navigator;
mod neon;
pub(crate) mod operator;
mod tactical;

use std::rc::Rc;

use aivory_protocol::{Look, WarRoom};
use gpui::{AnyElement, Context, Window};
use serde_json::json;

pub(super) use board::{BoardState, WidthClass, side_width, width_class};
pub(super) use bridge::BridgeState;
pub(crate) use bridge::data as bridge_data;
pub use bridge::detail::{BridgeDetail, BridgeDetailEvent};
pub(super) use hud::HudState;
pub(super) use navigator::NavigatorState;
pub(super) use neon::NeonState;
pub(super) use operator::OperatorState;

use super::WarRoomScreen;

/// Every skin with its label, in menu order.
pub(super) const SKINS: [(Look, &str); 8] = [
    (Look::Minimal, "Minimal"),
    (Look::Bridge, "Bridge"),
    (Look::Operator, "Operator"),
    (Look::Neon, "Neon"),
    (Look::Navigator, "Navigator"),
    (Look::Holo, "Holo"),
    (Look::Tactical, "Tactical"),
    (Look::Command, "Command"),
];

impl WarRoomScreen {
    /// The look on screen: the app-wide look (ADR 0022).
    pub(super) fn skin(&self) -> Look {
        crate::theme::tk().look
    }

    /// Switch the skin and save it in the settings.
    pub(super) fn set_skin(&mut self, skin: Look, cx: &mut Context<Self>) {
        if self.skin() == skin {
            return;
        }
        crate::theme::set_tokens(crate::look::tokens(
            skin,
            aivory_protocol::Theme::Light,
            false,
        ));
        self.save_display("look", json!(skin), cx);
        cx.refresh_windows();
    }

    /// The whole war room area for a full-screen skin; `None` for Minimal.
    pub(super) fn render_skin(
        &mut self,
        war: &Rc<WarRoom>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.skin() {
            Look::Minimal => None,
            Look::Bridge => Some(self.render_bridge(war, window, cx)),
            Look::Operator => Some(self.render_operator(war, window, cx)),
            Look::Neon => Some(self.render_neon(war, window, cx)),
            Look::Navigator => Some(self.render_navigator(war, window, cx)),
            Look::Holo => Some(self.render_holo(war, window, cx)),
            Look::Tactical => Some(self.render_tactical(war, window, cx)),
            Look::Command => Some(self.render_command(war, window, cx)),
        }
    }
}
