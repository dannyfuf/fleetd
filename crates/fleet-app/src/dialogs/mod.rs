//! The window's dialog drafts, the card each dialog renders, and the entity the Shell mounts.

mod assign_repo;
mod clone_repo;
mod confirm;
mod context;
mod create_worktree;
mod edit_hooks;
pub mod filter;
mod help;
mod host;
mod input;
mod palette;
mod quit;
mod rename_terminal;
mod settings;

use fleet_core::ids::RepoId;
use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Div, Entity, FocusHandle, Pixels, Window, div, px};

use crate::{bridge::Bridge, state::AppState};

/// Clamped cursor stepping, named so it does not collide with the per-dialog `move_cursor`
/// helpers that step a whole draft.
pub(crate) use crate::state::move_cursor as step;
pub use confirm::ConfirmRequest;
pub use host::{ActiveDialog, request_confirm, request_edit_hooks};
pub(crate) use host::{
    DialogHost, SessionTransport, notify, open_agent_session, open_session, open_worktree,
    read_host, retain_task, with_host,
};
pub(crate) use input::{clear_all, field, type_into, typed_char};

/// §3.8 card width for a single-field prompt: new/edit context, rename, assign repo.
const NARROW_W: Pixels = px(460.0);
/// §3.8 card width for a yes/no prompt with facts above it.
const PROMPT_W: Pixels = px(520.0);
/// §3.8 card width for the settings rail plus pane.
const WIDE_W: Pixels = px(720.0);
/// §3.8 card width for the three-column keymap.
const HELP_W: Pixels = px(880.0);

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
    /// Edit a repository's prepare and post-create hooks (`e`).
    EditHooks,
    /// §3.8.6 Settings (`,`).
    Settings,
    /// Rename the Workspace's active terminal (`ctrl-s ,`).
    RenameTerminal,
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
            Self::EditHooks => "Hooks",
            Self::Settings => "Settings",
            Self::RenameTerminal => "Rename",
            Self::Help => "Help",
            Self::Quit => "Quit",
            Self::QuitDaemon => "QuitDaemon",
        }
    }

    /// The card width §3.8 fixes for this dialog. Confirms size themselves from their facts.
    #[must_use]
    pub(crate) fn width(&self, cx: &App) -> Pixels {
        match self {
            Self::CreateWorktree | Self::CloneRepo | Self::QuitDaemon | Self::EditHooks => {
                cx.theme().metrics.dialog_w
            }
            Self::Confirm => cx.theme().metrics.confirm_compact_w,
            Self::NewContext | Self::EditContext | Self::RenameTerminal | Self::AssignRepo => {
                NARROW_W
            }
            Self::Settings => WIDE_W,
            Self::Help => HELP_W,
            Self::Quit => PROMPT_W,
        }
    }

    /// Renders the open dialog's card.
    ///
    /// [`ActiveDialog`] has already applied the `Dialog > <name>` key context above this
    /// element, handles `Esc` (`dialog::Cancel`) by closing the dialog, and hands down the
    /// `host` whose drafts seeding prepared, so rendering only reads. The returned root
    /// element always tracks `focus`, which is what puts the `on_action` listeners below on
    /// the key-dispatch path.
    fn render(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        host: &Entity<DialogHost>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        match self {
            Self::CreateWorktree => create_worktree::render(state, bridge, focus, host, window, cx),
            Self::CloneRepo => clone_repo::render(state, bridge, focus, host, window, cx),
            Self::Confirm => confirm::render(state, bridge, focus, host, window, cx),
            Self::NewContext | Self::EditContext => {
                context::render(self, state, bridge, focus, host, window, cx)
            }
            Self::AssignRepo => assign_repo::render(state, bridge, focus, host, window, cx),
            Self::EditHooks => edit_hooks::render(state, bridge, focus, host, window, cx),
            Self::Settings => settings::render(state, bridge, focus, host, window, cx),
            Self::RenameTerminal => rename_terminal::render(state, bridge, focus, host, window, cx),
            Self::Help => help::render(state, focus, window, cx),
            Self::Quit => quit::render_quit(state, focus, window, cx),
            Self::QuitDaemon => quit::render_quit_daemon(state, focus, window, cx),
        }
    }
}

/// Initializes a draft at an overlay transition.
pub(crate) fn seed(dialog: &Dialogs, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if with_host(state, cx, |host| host.open.as_ref() == Some(dialog)) {
        return;
    }
    with_host(state, cx, |host| host.open = Some(dialog.clone()));
    match dialog {
        Dialogs::CreateWorktree => create_worktree::seed(state, bridge, cx),
        Dialogs::CloneRepo => clone_repo::seed(state, bridge, cx),
        Dialogs::Confirm => confirm::seed(state, bridge, cx),
        Dialogs::NewContext => context::seed(state, cx, false),
        Dialogs::EditContext => context::seed(state, cx, true),
        Dialogs::AssignRepo => assign_repo::seed(state, cx),
        Dialogs::EditHooks => edit_hooks::seed(state, cx),
        Dialogs::Settings => settings::seed(state, bridge, cx),
        Dialogs::RenameTerminal => rename_terminal::seed(state, cx),
        Dialogs::Help => help::prepare(),
        Dialogs::Quit | Dialogs::QuitDaemon => {}
    }
}

/// The root element every dialog returns: focus-tracking and full-window, so the card's own
/// scrim covers the screen behind it.
pub(crate) fn root(focus: &FocusHandle) -> Div {
    div().track_focus(focus).size_full()
}

/// The repository the Hub's cursor is on, if any.
///
/// §3.8.1: "the rail selection is the repo; from `All` it is the repo of the highlighted
/// worktree".
#[must_use]
pub(crate) fn focused_repo(state: &AppState) -> Option<RepoId> {
    crate::presentation::selected_repo_id(state)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn both_context_dialogs_share_one_key_context() {
        assert_eq!(
            Dialogs::NewContext.context_name(),
            Dialogs::EditContext.context_name(),
            "KEYMAP gives both context dialogs one row"
        );
    }

    #[test]
    fn focused_repo_matches_hub() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.displayed_hub.worktrees = vec![crate::presentation::DisplayedWorktree {
            id: "acme/api#newest".parse().expect("worktree id"),
            repo: "acme/api".parse().expect("repo id"),
        }];
        state.cursors.worktrees = 0;

        assert_eq!(
            focused_repo(&state).as_ref().map(RepoId::as_str),
            Some("acme/api")
        );

        state.hub_pane = crate::state::HubPane::Repos;
        state.displayed_hub.repos = vec![crate::presentation::DisplayedRepo {
            kind: crate::presentation::DisplayedRepoKind::All,
            repo: None,
            job: None,
        }];
        state.cursors.repos = 0;
        state.scope = crate::state::RepoScope::All;

        assert_eq!(
            focused_repo(&state).as_ref().map(RepoId::as_str),
            Some("acme/api"),
            "All resolves the highlighted worktree's repository"
        );
    }
}
