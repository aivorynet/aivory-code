//! AIVory Code remote link: iroh endpoint and device keys for the phone (ADR 0015).
//!
//! The phone reaches the daemon over iroh (QUIC with hole punching and a relay). One
//! connection carries one bidirectional stream with the ALPN [`ALPN`]. Each frame on the
//! stream is one JSON-RPC message with a 4-byte big-endian length prefix ([`frame`]).
//!
//! The daemon side uses [`endpoint`], [`frame`], [`keys`], [`hello`], and [`pairlink`]. The
//! phone side uses [`client::LinkClient`], which has no generic types, takes and returns JSON
//! strings, and so is simple to wrap in an FFI layer.

pub mod client;
pub mod endpoint;
pub mod error;
pub mod frame;
pub mod hello;
pub mod keys;
pub mod pairlink;
pub mod push;

/// The re-exported iroh crate, for the daemon side.
pub use iroh;

pub use client::{ConnectOptions, LinkClient, MessageSink, PathInfo, PathKind};
pub use error::LinkError;
pub use hello::{HELLO_METHOD, Hello, HelloReply, UNAUTHORIZED};
pub use pairlink::PairLink;

/// The application protocol of the link.
pub const ALPN: &[u8] = b"aivory/rpc/1";
