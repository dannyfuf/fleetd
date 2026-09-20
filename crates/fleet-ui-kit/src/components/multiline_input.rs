//! `MultilineInput` — the native-agent composer built on the shared [`super::TextInput`].
//!
//! The inner input owns editing, selection, undo, IME, pointer geometry and scrolling. This
//! wrapper keeps only the composer's prompt history, completion triggers, submit/escape events,
//! read-only state and focus-visible chrome (`NATIVE-AGENTS.md` §2, §8, §9).

mod history;
mod input;
mod triggers;

pub use history::PromptHistory;
pub use input::MultilineInput;
pub use triggers::Trigger;

/// How many submitted prompts a composer remembers. `NATIVE-AGENTS.md` §9's `↑` walks this.
pub const HISTORY_LIMIT: usize = 100;

/// The gpui key context pushed around a [`MultilineInput`].
///
/// The app binds submit, history and escape in its own agent contexts; it does not shadow them
/// with `NoAction`. The nested `FleetTextInput` consumes editing actions and propagates vertical
/// motion at visual-row boundaries so those owner bindings can run.
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
    /// The text changed. A picker re-filters from [`MultilineInput::active_trigger`] on this.
    Changed,
    /// Escape requested a cascade step.
    Escape,
}

#[cfg(test)]
mod tests;
