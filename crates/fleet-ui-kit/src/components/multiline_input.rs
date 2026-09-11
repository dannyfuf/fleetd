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

pub use buffer::{MultilineBuffer, PromptHistory, Trigger};
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
///
/// The composer never acts on a thread: it reports, and the owner decides what a submit, a
/// completion trigger, a change or an escape means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultilineInputEvent {
    /// Enter submitted the current non-empty text.
    Submit(String),
    /// `@`, `$` or `/` opened a completion surface. The character stays in the buffer.
    Trigger(Trigger),
    /// The text changed. A picker re-filters from [`MultilineInput::active_trigger`] on this,
    /// which is what keeps all three trigger characters typable.
    Changed,
    /// Escape requested a cascade step — an IME preedit and a selection cancel first.
    Escape,
}

#[cfg(test)]
mod tests;
