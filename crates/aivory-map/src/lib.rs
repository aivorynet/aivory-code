//! The part map of a repository: `.aivory/map.toml`.
//!
//! A map names the parts of a codebase (for example `payment` or `ui`) and lists
//! the globs that belong to each part. The war room uses it to group sessions.
//!
//! ```toml
//! version = 1
//! updated = "2026-10-21T14:12:00Z"
//! author  = "claude opus-5.5"
//!
//! [[part]]
//! name  = "payment"
//! paths = ["crates/payment/**", "docs/payment/*.md"]
//! ```

mod edit;
mod error;
mod file;
mod init;
mod matcher;
mod stamp;

pub use edit::add_part;
pub use error::MapError;
pub use file::{MAP_DIR, MAP_FILE, build, codebase_id, load, map_path, parse, write};
pub use init::{INIT_AUTHOR, init};
pub use matcher::PartMatcher;
