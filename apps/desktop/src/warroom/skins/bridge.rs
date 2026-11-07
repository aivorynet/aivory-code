//! The Bridge war room: the planet view (`planet.rs`) and the session view (`detail.rs`). The
//! numbered navigation frame belongs to the shell (`shell/bridge.rs`).

pub(crate) mod data;
pub(crate) mod detail;
mod planet;

use planet::Planet;

/// The state of the Bridge war room between frames.
#[derive(Default)]
pub(in crate::warroom) struct BridgeState {
    planet: Planet,
}
