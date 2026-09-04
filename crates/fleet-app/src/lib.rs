//! Native GPUI Fleet application state and presentation layer, including its daemon bridge, keyboard modes and actions, screens, dialogs, and terminal renderer.

pub mod actions;
pub mod bridge;
pub mod dialogs;
pub mod keymap;
pub mod screens;
pub mod shell;
pub mod state;
pub mod terminal_element;
pub mod views;

pub use shell::{Shell, run};
