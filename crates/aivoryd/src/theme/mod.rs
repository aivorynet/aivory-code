//! Theme packages in the daemon (ADR 0024): download, store, lock, service and command line.
//! The parser, the validator and the unpacker are in the crate `aivory-theme`.

pub mod bundle;
pub mod cli;
pub mod fetch;
pub mod lock;
pub mod rpc;
pub mod service;
pub mod store;

pub use fetch::{Fetcher, HttpFetcher, Source};
pub use service::ThemeService;
pub use store::Store;
