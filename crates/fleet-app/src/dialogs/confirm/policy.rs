use super::*;

/// What the open Confirm dialog asks about.
///
/// The hub, the workspace and the palette all route their destructive keys through this: call
/// [`crate::dialogs::request_confirm`] and then open [`crate::dialogs::Dialogs::Confirm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmRequest {
    /// `d` on a worktree row.
    DeleteWorktree {
        /// The worktree to delete.
        id: WorktreeId,
    },
    /// `d` on a repo row: cascades to every worktree and session.
    DeleteRepo {
        /// The repository to delete.
        repo: RepoId,
        /// How many worktrees go with it.
        worktrees: usize,
    },
    /// `D` in the Hub, or `ctrl-d` in the Edit-context dialog.
    DeleteContext {
        /// The context to delete.
        context: ContextId,
        /// Its display name.
        name: String,
        /// How many repositories go with it.
        repos: usize,
        /// How many worktrees go with it.
        worktrees: usize,
        /// How many sessions go with it.
        sessions: usize,
    },
    /// `x` on a worktree row: a multi-target confirm with its own body.
    Prune {
        /// The repository to prune.
        repo: RepoId,
    },
    /// `K` on a worktree row.
    KillSession {
        /// The session to kill.
        session: SessionId,
        /// How many terminals die with it.
        terminals: usize,
        /// The keep-alive labels running in it.
        running: Vec<String>,
        /// Whether an editor in it has unsaved changes.
        unsaved: bool,
    },
    /// `d` on a board card (BOARD §8).
    DeleteCard {
        /// The card to delete.
        card: CardId,
        /// Its display key, e.g. `FLT-12`.
        key: String,
        /// Its title, which is what the consequence sentence names.
        title: String,
    },
    /// `ctrl-s x` in the Workspace.
    CloseTerminal {
        /// The terminal to close.
        terminal: TerminalId,
        /// Its one-based tab index.
        index: usize,
        /// Its name.
        name: String,
        /// The command running in it, when one is.
        running: Option<String>,
    },
}

impl ConfirmRequest {
    /// The title §3.8.3 gives this action.
    ///
    /// The delete-worktree confirm has two, because it has two shapes: the compact form puts
    /// the full `WorktreeId` in the title, the expanded form puts it on row 1 of the body and
    /// the title falls back to `Delete worktree` — printing the id in both places says the
    /// same long string twice and pushes the facts down.
    #[must_use]
    pub fn title(&self, compact: bool) -> String {
        match self {
            Self::DeleteWorktree { id } if compact => format!("Delete {}?", id.as_str()),
            Self::DeleteWorktree { .. } => "Delete worktree".to_owned(),
            Self::DeleteRepo { repo, .. } => format!("Delete repository {}?", repo.as_str()),
            Self::DeleteContext { name, .. } => format!("Delete context \"{name}\"?"),
            Self::DeleteCard { key, .. } => format!("Delete {key}?"),
            Self::Prune { repo } => format!("Prune {}", repo.as_str()),
            Self::KillSession { session, .. } => format!("Kill session {}?", session.as_str()),
            Self::CloseTerminal { index, name, .. } => {
                format!("Close terminal {index} \"{name}\"?")
            }
        }
    }

    /// The consequence sentence. Users confirm the sentence, not the title (§3.8.3).
    #[must_use]
    pub fn consequence(&self, facts: &Facts) -> String {
        match self {
            Self::DeleteWorktree { .. } => {
                if facts.risky {
                    "Deleting kills the session and moves the copy to trash; commits that exist \
                     only here are lost."
                        .to_owned()
                } else {
                    "Moves the copy to trash, then removes it in the background.".to_owned()
                }
            }
            Self::DeleteRepo { worktrees, .. } => format!(
                "Also deletes {worktrees} worktrees and their sessions. The base clone and every \
                 copy go to trash."
            ),
            Self::DeleteContext {
                repos,
                worktrees,
                sessions,
                ..
            } => format!(
                "Also deletes {repos} repositories, {worktrees} worktrees and every session in \
                 them ({sessions} running)."
            ),
            Self::DeleteCard { title, .. } => format!(
                "Removes \"{title}\" with its comments and activity. A worktree created from it \
                 is kept."
            ),
            Self::Prune { .. } => {
                "Deletes the ones listed below. The skipped ones are kept, with the reason shown."
                    .to_owned()
            }
            Self::KillSession {
                terminals,
                running,
                unsaved,
                ..
            } => {
                let mut sentence = format!("Kills {terminals} terminals at once.");
                if *unsaved {
                    sentence.push_str(" An editor has unsaved changes.");
                }
                if !running.is_empty() {
                    sentence.push_str(&format!(" {} are killed too.", running.join(", ")));
                }
                sentence.push_str(" Nothing is saved.");
                sentence
            }
            Self::CloseTerminal { running, .. } => match running {
                Some(command) => format!("{command} is running in it and will be killed."),
                None => "The terminal and its shell are closed.".to_owned(),
            },
        }
    }

    /// The header glyph §3.8 assigns to this confirm.
    #[must_use]
    pub const fn icon(&self, compact: bool) -> Icon {
        if !compact {
            return Icon::TriangleAlert;
        }
        match self {
            Self::DeleteWorktree { .. }
            | Self::DeleteRepo { .. }
            | Self::DeleteContext { .. }
            | Self::DeleteCard { .. } => Icon::Trash,
            Self::Prune { .. } => Icon::Scissors,
            Self::KillSession { .. } => Icon::Power,
            Self::CloseTerminal { .. } => Icon::X,
        }
    }

    /// The verb on the primary action.
    #[must_use]
    pub fn action_label(&self, prune_count: usize) -> String {
        match self {
            Self::DeleteWorktree { .. }
            | Self::DeleteRepo { .. }
            | Self::DeleteContext { .. }
            | Self::DeleteCard { .. } => "Delete".to_owned(),
            Self::Prune { .. } => format!("Prune {prune_count}"),
            Self::KillSession { .. } => "Kill".to_owned(),
            Self::CloseTerminal { .. } => "Close".to_owned(),
        }
    }

    /// Whether §3.8.3 requires `Y` regardless of what the facts say.
    ///
    /// Repo and context delete cascade far beyond the row under the cursor, so [D-10] escalates
    /// them unconditionally.
    #[must_use]
    pub const fn always_strong(&self) -> bool {
        matches!(self, Self::DeleteRepo { .. } | Self::DeleteContext { .. })
    }

    /// Whether `I` re-checks anything here.
    #[must_use]
    pub const fn rechecks(&self) -> bool {
        matches!(self, Self::DeleteWorktree { .. } | Self::Prune { .. })
    }

    /// The full id of the target, shown on its own line in the expanded form.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::DeleteWorktree { id } => id.as_str().to_owned(),
            Self::DeleteRepo { repo, .. } | Self::Prune { repo } => repo.as_str().to_owned(),
            Self::DeleteContext { context, .. } => context.as_str().to_owned(),
            Self::DeleteCard { key, .. } => key.clone(),
            Self::KillSession { session, .. } => session.as_str().to_owned(),
            Self::CloseTerminal { name, .. } => name.clone(),
        }
    }
}

/// Whether the open confirm needs `Y` rather than `y`.
pub(super) fn strong_required(state: &Entity<AppState>, cx: &mut App) -> bool {
    with_host(state, cx, |host| {
        confirmation_policy(&host.confirm, now_unix()).key == ConfirmKey::Upper
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConfirmationPolicy {
    pub key: ConfirmKey,
    pub authorized: bool,
}

pub(super) fn confirmation_policy(draft: &ConfirmState, now: i64) -> ConfirmationPolicy {
    let Some(request) = draft.request.as_ref() else {
        return ConfirmationPolicy {
            key: ConfirmKey::Upper,
            authorized: false,
        };
    };
    let key = if request.always_strong() {
        ConfirmKey::Upper
    } else {
        match request {
            ConfirmRequest::DeleteWorktree { .. } => {
                let facts = worktree_facts(draft.inspection.as_ref(), draft.loading, now);
                if draft.error.is_some() {
                    ConfirmKey::Upper
                } else {
                    facts.list.confirm_key()
                }
            }
            ConfirmRequest::Prune { .. } => {
                if draft.loading || draft.prune.is_none() || draft.error.is_some() {
                    ConfirmKey::Upper
                } else {
                    ConfirmKey::Lower
                }
            }
            ConfirmRequest::KillSession {
                running, unsaved, ..
            } if *unsaved || !running.is_empty() => ConfirmKey::Upper,
            _ => ConfirmKey::Lower,
        }
    };
    let authorized = !draft.loading
        && (!matches!(request, ConfirmRequest::Prune { .. })
            || (draft.prune.is_some() && draft.error.is_none()));
    ConfirmationPolicy { key, authorized }
}

pub(super) fn admits(policy: ConfirmationPolicy, pressed: ConfirmKey) -> bool {
    policy.authorized && (pressed == ConfirmKey::Upper || policy.key == ConfirmKey::Lower)
}
