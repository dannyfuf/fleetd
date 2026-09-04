//! Modal dialogs for Fleet creation, configuration, confirmation, and help flows.
//!
//! # Extension point
//!
//! [`Dialogs`] is the **open dialog**, held by [`crate::state::Overlay::Dialog`]. The shell
//! owns opening and closing it, gives it the `Dialog > <name>` key context returned by
//! [`Dialogs::context_name`], and calls [`Dialogs::render`] inside the frame's overlay layer.
//!
//! The dialog agent implements the bodies. The two signatures below are frozen — see
//! `docs/APP-CONTRACTS.md` — but variants may grow payloads (`Confirm(ConfirmDialogState)` and
//! so on) as long as the names and the context words stay put, because `docs/KEYMAP.md` binds
//! keys against those words.

use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{bridge::Bridge, state::AppState};

/// Which dialog is open (§3.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialogs {
    /// §3.8.1 Create worktree (`n` in the worktrees pane).
    CreateWorktree,
    /// §3.8.2 Clone repo (`n` in the repos pane).
    CloneRepo,
    /// §3.8.3 Confirm — delete / prune / kill / close terminal.
    Confirm,
    /// §3.8.4 New context (`N`).
    NewContext,
    /// §3.8.4 Edit context (`E`).
    EditContext,
    /// §3.8.5 Assign repo to context (`m`).
    AssignRepo,
    /// §3.8.6 Settings (`,`).
    Settings,
    /// §3.8.7 Help (`?`).
    Help,
    /// §3.8.8 Quit (`ctrl-q`) with work still running.
    Quit,
    /// §3.8.9 Quit and stop the daemon (`ctrl-shift-q`).
    QuitDaemon,
}

impl Dialogs {
    /// The second half of this dialog's key context, i.e. `Dialog > <name>`.
    ///
    /// `NewContext` and `EditContext` share `Context` because `docs/KEYMAP.md` gives them one
    /// row (`ctrl-d` deletes from either).
    #[must_use]
    pub const fn context_name(&self) -> &'static str {
        match self {
            Self::CreateWorktree => "Create",
            Self::CloneRepo => "Clone",
            Self::Confirm => "Confirm",
            Self::NewContext | Self::EditContext => "Context",
            Self::AssignRepo => "Assign",
            Self::Settings => "Settings",
            Self::Help => "Help",
            Self::Quit => "Quit",
            Self::QuitDaemon => "QuitDaemon",
        }
    }

    /// The dialog's title, used by the placeholder body and by the palette.
    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self {
            Self::CreateWorktree => "New worktree",
            Self::CloneRepo => "Clone repository",
            Self::Confirm => "Confirm",
            Self::NewContext => "New context",
            Self::EditContext => "Edit context",
            Self::AssignRepo => "Move repo to context",
            Self::Settings => "Settings",
            Self::Help => "Keymap",
            Self::Quit => "Quit Fleet?",
            Self::QuitDaemon => "Stop fleetd and quit?",
        }
    }

    /// Renders the dialog into the frame's overlay layer.
    ///
    /// The shell has already applied the `Dialog > <name>` key context above this element and
    /// handles `Esc` (`dialog::Cancel`) by closing the dialog; an implementation that needs
    /// `Esc` for something else — §3.8.2's "Esc cancels only the search request" — stops the
    /// propagation of that action itself. The returned root element **must**
    /// `.track_focus(focus)`.
    pub fn render(
        &self,
        _state: &Entity<AppState>,
        _bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        // Placeholder: the shared §3.8 frame with an empty body. The dialogs agent replaces
        // this match arm by arm without touching the shell.
        div()
            .track_focus(focus)
            .size_full()
            .child(
                Dialog::new(self.title())
                    .body(div().child(Text::ui("Not implemented yet.").muted()))
                    .hint_row(KeyHintRow::new().key("esc", "cancel")),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialog_has_a_key_context_word() {
        for dialog in [
            Dialogs::CreateWorktree,
            Dialogs::CloneRepo,
            Dialogs::Confirm,
            Dialogs::NewContext,
            Dialogs::EditContext,
            Dialogs::AssignRepo,
            Dialogs::Settings,
            Dialogs::Help,
            Dialogs::Quit,
            Dialogs::QuitDaemon,
        ] {
            assert!(!dialog.context_name().is_empty());
            assert!(!dialog.title().is_empty());
        }
        assert_eq!(
            Dialogs::NewContext.context_name(),
            Dialogs::EditContext.context_name(),
            "KEYMAP gives both context dialogs one row"
        );
    }
}
