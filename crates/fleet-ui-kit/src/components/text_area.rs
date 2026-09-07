//! `TextArea` — the multi-line sibling of [`super::text_field`], for the board's card
//! description and its comments.
//!
//! The module has the same three layers as `text_field.rs`, and a caller picks exactly one:
//!
//! 1. [`TextArea`] — the presentational `RenderOnce` surface. The caller owns the string and
//!    the caret byte offset and passes both down every frame.
//! 2. [`TextAreaState`] — the editing model: an owned string, a byte cursor, a preferred
//!    column and a scroll row, plus the KEYMAP §3.8 edit set widened to two dimensions
//!    (`↑` / `↓` keeping the column, `Enter` inserting a newline, `Tab` inserting
//!    [`TAB_WIDTH`] spaces). It is pure — no gpui element, no theme — so it is unit-testable
//!    and reusable by any dialog that renders its own chrome.
//! 3. There is deliberately no live `TextInput`-style entity here yet: the card dialogs own
//!    their focus and route keys through [`TextAreaState::handle_keystroke`]. When one needs
//!    IME composition, it grows the same way `TextField` did — around
//!    [`TextAreaState::handle_edit_keystroke`], never around `handle_keystroke`, or every
//!    printable character is inserted twice.
//!
//! The surface keeps `TextField`'s visual language: the same box, the same border ladder
//! (danger beats focus beats rest) and the same 2 px accent caret bar. What it does not keep
//! is the 18 px status slot — a multi-line body has no derived preview, and a description
//! that fails validation fails on the field that names it.

mod line;
mod state;
mod surface;

pub use state::{TAB_WIDTH, TextAreaState};
pub use surface::{TEXT_AREA_ROWS, TextArea};

#[cfg(test)]
mod tests;
