//! The window's dialog drafts, the card each dialog renders, and the entity the Shell mounts.

mod assign_repo;
mod board_settings;
mod card_create;
pub(crate) mod card_detail;
pub(crate) mod card_picker;
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
pub(crate) use settings::editor_command;

/// §3.8 card width for a single-field prompt: new/edit context, rename, assign repo.
const NARROW_W: Pixels = px(460.0);
/// §3.8 card width for a yes/no prompt with facts above it.
const PROMPT_W: Pixels = px(520.0);
/// §3.8 card width for the settings rail plus pane.
const WIDE_W: Pixels = px(720.0);
/// §3.8 card width for the three-column keymap.
const HELP_W: Pixels = px(880.0);

/// §3.8.2 card height for the clone-repo list: `560 × 420`.
const CLONE_H: Pixels = px(420.0);
/// §3.8.6 card height for the settings rail plus pane: `720 × 560`.
const SETTINGS_H: Pixels = px(560.0);
/// §3.8.7 card height for the three-column keymap: `880 × 620`.
const HELP_H: Pixels = px(620.0);

/// Which dialog is open (§3.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialogs {
    /// Board settings (BOARD §8).
    BoardSettings,
    /// Card property (BOARD §8).
    CardPicker,
    /// New card (BOARD §8).
    CardCreate,
    /// Card detail (BOARD §8).
    CardDetail,
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
            Self::BoardSettings => "BoardSettings",
            Self::CardPicker => "CardPicker",
            Self::CardCreate => "CardCreate",
            Self::CardDetail => "CardDetail",
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
            Self::CreateWorktree
            | Self::CloneRepo
            | Self::QuitDaemon
            | Self::EditHooks
            | Self::BoardSettings
            | Self::CardPicker
            | Self::CardCreate => cx.theme().metrics.dialog_w,
            Self::Confirm => cx.theme().metrics.confirm_compact_w,
            Self::NewContext | Self::EditContext | Self::RenameTerminal | Self::AssignRepo => {
                NARROW_W
            }
            Self::Settings => WIDE_W,
            Self::CardDetail | Self::Help => HELP_W,
            Self::Quit => PROMPT_W,
        }
    }

    /// The card height §3.8 fixes for this dialog, if it fixes one. The rest size to their
    /// content and stop at 90 % of the window, which is what [`Dialog::height`] omitted means.
    #[must_use]
    pub(crate) const fn height(&self) -> Option<Pixels> {
        match self {
            Self::CloneRepo => Some(CLONE_H),
            Self::Settings => Some(SETTINGS_H),
            Self::Help => Some(HELP_H),
            _ => None,
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
            Self::BoardSettings => board_settings::render(state, bridge, focus, host, window, cx),
            Self::CardPicker => card_picker::render(state, bridge, focus, host, window, cx),
            Self::CardCreate => card_create::render(state, bridge, focus, host, window, cx),
            Self::CardDetail => card_detail::render(state, bridge, focus, host, window, cx),
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
        Dialogs::BoardSettings => board_settings::seed(state, cx),
        Dialogs::CardPicker => card_picker::seed(state, cx),
        Dialogs::CardCreate => card_create::seed(state, cx),
        Dialogs::CardDetail => card_detail::seed(state, cx),
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
    fn every_dialog_has_a_key_context_word() {
        for dialog in [
            Dialogs::BoardSettings,
            Dialogs::CardPicker,
            Dialogs::CardCreate,
            Dialogs::CardDetail,
            Dialogs::CreateWorktree,
            Dialogs::CloneRepo,
            Dialogs::Confirm,
            Dialogs::NewContext,
            Dialogs::EditContext,
            Dialogs::AssignRepo,
            Dialogs::EditHooks,
            Dialogs::Settings,
            Dialogs::RenameTerminal,
            Dialogs::Help,
            Dialogs::Quit,
            Dialogs::QuitDaemon,
        ] {
            assert!(!dialog.context_name().is_empty());
        }
    }

    #[test]
    fn both_context_dialogs_share_one_key_context() {
        assert_eq!(
            Dialogs::NewContext.context_name(),
            Dialogs::EditContext.context_name(),
            "KEYMAP gives both context dialogs one row"
        );
    }

    /// Fixed dialog geometry is a named const with a doc comment naming its §3.8 clause, so
    /// the whole ladder moves together. `mod.rs` is where those consts live, so it is the one
    /// file this walk skips.
    #[test]
    fn dialog_views_take_their_geometry_from_named_consts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dialogs");
        let mut offenders = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
            for entry in entries {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Ok(relative) = path.strip_prefix(&root) else {
                    continue;
                };
                let name = relative.to_string_lossy().replace('\\', "/");
                if path.extension().is_none_or(|ext| ext != "rs") || name == "mod.rs" {
                    continue;
                }
                let body = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("reading {name}: {e}"));
                offenders.extend(
                    body.lines()
                        .enumerate()
                        .filter(|(_, line)| {
                            // `px(0.0)` is a zero origin, not geometry: `ListState` takes one.
                            !line.contains("px(0.0)")
                                && line
                                    .split("px(")
                                    .skip(1)
                                    .any(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                        })
                        .map(|(ix, line)| format!("{name}:{}: {}", ix + 1, line.trim())),
                );
            }
        }
        assert!(
            offenders.is_empty(),
            "dialog geometry belongs in a named const beside NARROW_W: {offenders:#?}"
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
