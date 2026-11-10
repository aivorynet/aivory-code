//! AIVory Code daemon: lists the native agent sessions on this machine and resumes one with the
//! user's own logged-in CLI. UIs talk to it over JSON-RPC 2.0 (newline-delimited JSON) on a
//! Windows named pipe or a Unix domain socket.

pub mod actions;
pub mod claude_usage;
pub mod client;
pub mod code;
pub mod codex_usage;
pub mod complete;
pub mod config;
pub mod coord;
pub mod demo;
pub mod endpoint;
pub mod fork;
pub mod gitdiff;
pub mod iroh_link;
pub mod ledger;
pub mod live;
pub mod outbox;
pub mod push;
pub mod remote;
pub mod repos;
pub mod resources;
pub mod rpc;
pub mod schedule;
pub mod server;
pub mod theme;

pub use config::Config;
pub use endpoint::Endpoint;
pub use outbox::Outbox;
pub use rpc::{Daemon, ResumeParams, RpcError};
pub use server::Server;
