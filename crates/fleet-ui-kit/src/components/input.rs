//! Pure text-editing state shared by single-line and multi-line inputs.
//!
//! Rendering, focus, clipboard access, and platform event routing belong to the live input
//! component. This module owns the byte-offset editing rules and deterministic undo history.

mod buffer;
mod history;

pub use buffer::{InputBuffer, InputMode};
pub use history::{HISTORY_CAP, TYPING_GROUP_WINDOW};

#[cfg(test)]
mod tests;
