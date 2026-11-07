//! The contract between `aivoryd` and its clients (the GPUI desktop app and the Flutter phone
//! app).
//!
//! - Data types for every screen of the accepted design (`docs/design/screens/README.md`).
//!   JSON uses camelCase field names; optional fields and empty lists are left out.
//! - [`rpc`]: the JSON-RPC 2.0 surface (method names, parameters, results, the war-room feed).
//! - [`layout`]: pure layout helpers that all clients share (Sankey, grid columns, DAG).
//! - [`demo`]: the sample data of the design, served by `aivoryd serve --demo`.
//! - [`schema`]: the JSON Schema of the whole surface (`proto/schema/`).
//!
//! Times are [`UnixMs`]. Clients show "waits 6 min" as `now - since`, with `now` from the
//! snapshot plus the time since it arrived.

pub mod actions;
pub mod code;
pub mod common;
pub mod coord;
pub mod demo;
pub mod inbox;
pub mod layout;
pub mod repos;
pub mod resources;
pub mod rpc;
pub mod schema;
pub mod session;
pub mod settings;
pub mod tasks;
pub mod theme;
pub mod warroom;

pub use actions::*;
pub use common::*;
pub use inbox::*;
pub use repos::*;
pub use resources::*;
pub use session::*;
pub use settings::*;
pub use tasks::*;
pub use warroom::*;

/// Version of this contract. A client refuses a daemon with a different major version.
pub const PROTOCOL_VERSION: &str = "0.1.0";
