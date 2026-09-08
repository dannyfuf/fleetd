//! `MultilineInput` — the native-agent composer: a wrapping, IME-aware, multi-line editor.
//!
//! It is [`super::TextInput`]'s multi-line sibling, and it is split the same way, so the half
//! that decides what the text *is* can be tested without a window:
//!
//! 1. [`MultilineBuffer`] — the editing model: an owned string, a byte cursor, a selection
//!    anchor and the composer's edit set (insert, newline, backspace / delete, word-wise
//!    deletion, line and word motion, shift-selection, select-all). It is pure — no gpui
//!    element, no theme — and `\n` is content rather than something to strip.
//! 2. [`PromptHistory`] — the last [`HISTORY_LIMIT`] submitted prompts plus the draft `↑`
//!    was opened from, so walking off the end of the list restores what the user was typing.
//! 3. [`MultilineInput`] — the gpui entity: focus, keys, the platform clipboard, an
//!    [`gpui::ElementInputHandler`] so dead keys and IME composition work like a native field,
//!    and a custom element that shapes wrapped lines, paints the selection wash and the 2 px
//!    caret, and grows the box one line at a time up to the canvas' eight.
//!
//! The composer never acts on a thread. It reports intent through [`MultilineInputEvent`] and
//! the owner decides what a submit, a completion trigger or an escape means
//! (`NATIVE-AGENTS.md` §2, §8, §9).

mod buffer;
mod element;
mod input;

pub use buffer::{MultilineBuffer, PromptHistory};
pub use input::MultilineInput;

/// How many submitted prompts a composer remembers. `NATIVE-AGENTS.md` §9's `↑` walks this.
pub const HISTORY_LIMIT: usize = 100;

/// The gpui key context a [`MultilineInput`] pushes.
///
/// The composer owns bare `⏎`, `⇧⏎`, `esc`, the arrows and the whole edit set, so an app that
/// binds those keys — the agent thread binds `⏎` to send and `y` / `n` / `1`–`4` to decision
/// cards — must shadow them in this context with `gpui::NoAction`, or the action fires instead
/// of the composer seeing the key.
pub const MULTILINE_INPUT_KEY_CONTEXT: &str = "FleetMultilineInput";

/// Events emitted by the native-agent composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultilineInputEvent {
    /// Enter submitted the current non-empty text.
    Submit(String),
    /// `@` or `/` requested an attachment or command completion surface.
    Trigger(char),
    /// Escape requested queue cancellation or focus exit.
    Escape,
}

#[cfg(test)]
mod tests;
