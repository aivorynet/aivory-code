//! The Bridge war room: the planet view (`planet.rs`). The
//! numbered navigation frame belongs to the shell (`shell/bridge.rs`).

pub(crate) mod data;
mod planet;
mod planetgen;

use planet::Planet;

/// The state of the Bridge war room between frames.
#[derive(Default)]
pub(in crate::warroom) struct BridgeState {
    planet: Planet,
}
