//! Native GPUI Fleet application state and presentation layer, including its daemon bridge, keyboard modes and actions, screens, dialogs, and terminal renderer.

pub(crate) mod actions;
pub mod bridge;
pub mod dialogs;
pub(crate) mod drive;
pub(crate) mod keymap;
pub(crate) mod notify_sound;
pub mod presentation;
pub mod screens;
pub mod shell;
pub mod state;
pub(crate) mod terminal;
pub mod views;
/// Read-only subagent output mirrors and catch-up cursors.
pub mod watches;

pub use shell::{Shell, run};
