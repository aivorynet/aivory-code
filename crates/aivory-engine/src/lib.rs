//! The engine of `aivoryd`, as a library: durable local state and the autopilot.
//!
//! - [`store`]: SQLite in WAL mode (ADR 0018). Numbered migrations and typed CRUD for tasks,
//!   answered inbox items, settings, pulse-chart samples and paired devices.
//! - [`activity`]: the 60-sample pulse-chart ring.
//! - [`pricing`]: model prices and the cost estimate of a session.
//! - [`usage`]: provider usage windows, estimated from local token events.
//! - [`autopilot`]: a pure planner that parks idle sessions and starts queued tasks.
//!
//! Nothing here starts a thread or a process. The daemon calls these functions and carries
//! out the [`autopilot::Action`]s.

pub mod activity;
pub mod autopilot;
pub mod pricing;
pub mod store;
pub mod usage;

#[cfg(test)]
mod testutil;

pub use activity::Sampler;
pub use autopilot::{Action, plan};
pub use pricing::{CostEstimate, PriceTable, session_cost};
pub use store::{AnsweredItem, Store, StoreError};
pub use usage::{TokenEvent, estimate, usage_windows};
