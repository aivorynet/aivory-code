//! The console body of a session transcript: markdown, tool call rows, diffs, prompts. The
//! war room panes, the deep dive transcript and the inbox tail draw their lines from here.

pub mod ansi;
pub mod blocks;
pub mod display;
pub mod draw;
pub mod list;
pub mod markdown;
pub mod sample;
pub mod todo;

pub use blocks::Pending;
pub use draw::{Frame, Hooks, Style};
pub use list::BlockList;
