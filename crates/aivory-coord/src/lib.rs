//! AIVory Code local coordination: routing keys, semantic deltas, claims, conflict radar, mailbox.
//!
//! This crate is the local scope of `docs/design/04-coordination-sync.md`: all sessions of one
//! daemon. It has no hub, no encryption, and no merge queue. The API is synchronous. State
//! lives in the `coord_*` tables of a `rusqlite::Connection` that the caller gives to
//! [`Coord`].
//!
//! # Pieces
//!
//! - [`keys`]: routing keys (`path:`, `sym:`, `test:`, `mod:`) as plain strings, and the
//!   [`keys::KeyMapper`] hook for a keyed hash in team scope.
//! - [`compute_delta`], [`delta_from_snapshots`]: the semantic [`TurnDelta`] of a turn from
//!   the stubs of `aivory-syntax` before and after.
//! - [`Coord::claim`], [`Coord::heartbeat`], [`Coord::land`], [`Coord::release`]: claims with a
//!   10-minute lease. A claim never blocks. [`Policy::gate_editing_overlap`] turns an overlap
//!   of editing claims into a [`Gate`] that the daemon handles.
//! - [`Coord::record_delta`]: the call at the end of a turn. It stores the delta, makes the
//!   editing claims, fires the subscriptions, and runs the radar.
//! - [`Coord::radar`]: tiers 1 to 3, trial merge of the latest snapshots through `aivory-vcs`,
//!   risk score ([`RadarHit`]). Tier 3 reads the references through a [`RefSource`]
//!   (`aivory_index::Index` implements it).
//! - [`Coord::send`], [`Coord::take_for_turn`], [`Coord::subscribe`]: the durable mailbox.
//!   Delivery only at a turn boundary, as quoted data (ADR 0019).
//!
//! # Wiring in the daemon
//!
//! 1. Open the connection and call `Coord::new(conn)`.
//! 2. At the end of a turn, after `aivory_vcs::finish_turn` made the snapshot:
//!    `let d = delta_from_snapshots(&vcs, repo, session, turn)?` and
//!    `coord.record_delta(&d, Some(&RadarEnv { vcs: &vcs, repo, refs: &index }))?`.
//!    Show `Outcome::hits` in the radar view. Act on `Outcome::gates`.
//! 3. While a session lives, call `coord.heartbeat(session)` at least every 5 minutes.
//! 4. Before the next input of a session, call `coord.take_for_turn(session)` and put the text
//!    in front of the input (Claude through hook context, Codex and Gemini as a prefix).
//! 5. When a change lands, call `coord.land(session)`. When a session ends, call
//!    `coord.release(session)`. Call `coord.sweep()` now and then.
//!
//! The first snapshot of a session must have the base commit as its parent
//! (`SnapshotRequest::parent`). Without a common history the trial merge of two sessions
//! cannot run, and the radar scores by tier only.
//!
//! Times are Unix milliseconds. Events carry a hybrid logical clock value ([`Hlc`]). Tests
//! use [`ManualClock`] through [`Coord::with_clock`].

mod clock;
mod coord;
mod delta;
mod error;
pub mod keys;
mod mailbox;
mod radar;

pub use clock::{Clock, Hlc, HlcClock, ManualClock, SystemClock};
pub use coord::{
    Claim, ClaimState, Coord, Footprint, Gate, LEASE_MS, MergeStats, Outcome, Policy,
    SCHEMA_VERSION,
};
pub use delta::{
    Change, ChangedSymbol, FileDelta, FileVersions, LineRange, TestDelta, TestState, TurnDelta,
    compute_delta, delta_from_snapshots, is_test, line_counts,
};
pub use error::{Error, Result};
pub use mailbox::{
    MAX_PINGS_PER_HOUR, MERGE_WINDOW_MS, MailItem, Ping, PingKind, SendOutcome, Subscription,
};
pub use radar::{NoRefs, RadarEnv, RadarHit, RefSource, TIER_DEPENDENCY, TIER_PATH, TIER_SYMBOL};
