//! The commands the palette runs: the catalogue-backed `Command` list and its validity rules.

use super::*;

/// Every command the palette can run.
///
/// The list is deliberately small: only the commands that are worth reaching without their key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Go to board.
    BoardGoBoard,
    /// Previous column.
    BoardPrevColumn,
    /// Next column.
    BoardNextColumn,
    /// Next card.
    BoardNextCard,
    /// Previous card.
    BoardPrevCard,
    /// Open card.
    BoardOpenCard,
    /// New card.
    BoardNewCard,
    /// Status picker.
    BoardPickStatus,
    /// Priority picker.
    BoardPickPriority,
    /// Assignee picker.
    BoardPickAssignee,
    /// Labels picker.
    BoardPickLabels,
    /// Estimate picker.
    BoardPickEstimate,
    /// Move card to previous column.
    BoardMovePrevColumn,
    /// Move card to next column.
    BoardMoveNextColumn,
    /// Create worktree from card.
    BoardCreateWorktree,
    /// Open linked worktree.
    BoardOpenWorktree,
    /// Sync.
    BoardSync,
    /// Full sync.
    BoardFullSync,
    /// Open remote issue.
    BoardOpenRemote,
    /// Delete card.
    BoardDeleteCard,
    /// Settings.
    BoardSettings,
    /// Reload.
    BoardReload,
    /// Filter cards.
    BoardFilter,
    /// Attach the focused card's run.
    BoardAttachRun,
    /// Cancel the focused card's run.
    BoardCancelRun,
    /// Run the column's action now.
    BoardRunNow,
    /// Pick the cards this one is blocked by.
    BoardPickBlockedBy,
    /// Pick the agent that runs this card.
    BoardPickAgent,
    /// Board settings, on its Columns section.
    BoardColumns,
    /// Close.
    CardDetailClose,
    /// Edit title.
    CardDetailEditTitle,
    /// Edit description.
    CardDetailEditDescription,
    /// Add comment.
    CardDetailAddComment,
    /// Next property.
    CardDetailNextProperty,
    /// Previous property.
    CardDetailPrevProperty,
    /// Edit selected property.
    CardDetailEditProperty,
    /// Create worktree.
    CardDetailCreateWorktree,
    /// Open remote issue.
    CardDetailOpenRemote,
    /// Resolve conflict: keep local.
    CardDetailKeepLocal,
    /// Resolve conflict: take remote.
    CardDetailTakeRemote,
    /// Save text edit.
    CardDetailSave,

    /// Open or select the active worktree session's board tab.
    WorkspaceOpenBoard,

    /// Open §3.8.1.
    NewWorktree,
    /// Open §3.8.2.
    CloneRepo,
    /// Delete the worktree under the cursor, through its confirm.
    DeleteWorktree,
    /// Prune the selected repository, through its confirm.
    PruneWorktrees,
    /// Sleep the session of the worktree under the cursor.
    SleepSession,
    /// Kill that session, through its confirm.
    KillSession,
    /// Re-inspect the worktree under the cursor.
    InspectWorktree,
    /// Open §3.8.5.
    MoveRepo,
    /// Open §3.8.4 for a new context.
    NewContext,
    /// Open §3.8.4 for the active context.
    EditContext,
    /// Delete the active context, through its confirm.
    DeleteContext,
    /// Go to the pull-requests screen.
    PullRequests,
    /// Go back to the worktrees list.
    Worktrees,
    /// Open the Jobs panel.
    JobsPanel,
    /// Open §3.8.6.
    Settings,
    /// Open §3.8.7.
    Help,
    /// Refresh statuses.
    Refresh,
    /// Update Fleet.
    UpdateFleet,
    /// Open the Claude agent session.
    OpenClaude,
    /// Open the Codex agent session.
    OpenCodex,
    /// Quit the app; the daemon keeps running.
    Quit,
    /// Quit and stop the daemon.
    QuitDaemon,
}

impl Command {
    /// Every command, in the order the `DO` section lists them.
    pub const ALL: &'static [Self] = &[
        Self::BoardGoBoard,
        Self::BoardPrevColumn,
        Self::BoardNextColumn,
        Self::BoardNextCard,
        Self::BoardPrevCard,
        Self::BoardOpenCard,
        Self::BoardNewCard,
        Self::BoardPickStatus,
        Self::BoardPickPriority,
        Self::BoardPickAssignee,
        Self::BoardPickLabels,
        Self::BoardPickEstimate,
        Self::BoardMovePrevColumn,
        Self::BoardMoveNextColumn,
        Self::BoardCreateWorktree,
        Self::BoardOpenWorktree,
        Self::BoardSync,
        Self::BoardFullSync,
        Self::BoardOpenRemote,
        Self::BoardDeleteCard,
        Self::BoardSettings,
        Self::BoardReload,
        Self::BoardFilter,
        Self::BoardAttachRun,
        Self::BoardCancelRun,
        Self::BoardRunNow,
        Self::BoardPickBlockedBy,
        Self::BoardPickAgent,
        Self::BoardColumns,
        Self::CardDetailClose,
        Self::CardDetailEditTitle,
        Self::CardDetailEditDescription,
        Self::CardDetailAddComment,
        Self::CardDetailNextProperty,
        Self::CardDetailPrevProperty,
        Self::CardDetailEditProperty,
        Self::CardDetailCreateWorktree,
        Self::CardDetailOpenRemote,
        Self::CardDetailKeepLocal,
        Self::CardDetailTakeRemote,
        Self::CardDetailSave,
        Self::WorkspaceOpenBoard,
        Self::NewWorktree,
        Self::CloneRepo,
        Self::PruneWorktrees,
        Self::DeleteWorktree,
        Self::SleepSession,
        Self::KillSession,
        Self::InspectWorktree,
        Self::MoveRepo,
        Self::NewContext,
        Self::EditContext,
        Self::DeleteContext,
        Self::PullRequests,
        Self::Worktrees,
        Self::JobsPanel,
        Self::Settings,
        Self::Help,
        Self::Refresh,
        Self::UpdateFleet,
        Self::OpenClaude,
        Self::OpenCodex,
        Self::Quit,
        Self::QuitDaemon,
    ];

    /// What the catalogue says about this command's action: its label, place and risk.
    ///
    /// The palette keeps its own enum for the data only it needs (the glyph and the validity
    /// rule), and reads every word a person sees from `action_catalogue`, so the two lists
    /// cannot drift. A test holds every command to a catalogue entry flagged `palette`.
    #[must_use]
    pub fn info(self) -> &'static ActionInfo {
        action_catalogue::info(self.action())
            .expect("every palette command is catalogued (checked by the palette's tests)")
    }

    /// The glyph, the same one the action wears elsewhere.
    #[must_use]
    pub const fn icon(self) -> Icon {
        match self {
            Self::BoardGoBoard => Icon::Boxes,
            Self::BoardPrevColumn => Icon::ChevronLeft,
            Self::BoardNextColumn => Icon::ChevronRight,
            Self::BoardNextCard => Icon::CircleArrowDown,
            Self::BoardPrevCard => Icon::CircleArrowUp,
            Self::BoardOpenCard => Icon::Eye,
            Self::BoardNewCard => Icon::Plus,
            Self::BoardPickStatus => Icon::CircleDot,
            Self::BoardPickPriority => Icon::Flag,
            Self::BoardPickAssignee => Icon::Bot,
            Self::BoardPickLabels => Icon::FilePen,
            Self::BoardPickEstimate => Icon::Hourglass,
            Self::BoardMovePrevColumn => Icon::ChevronLeft,
            Self::BoardMoveNextColumn => Icon::ChevronRight,
            Self::BoardCreateWorktree => Icon::GitBranchPlus,
            Self::BoardOpenWorktree => Icon::GitBranch,
            Self::BoardSync => Icon::CloudDownload,
            Self::BoardFullSync => Icon::CloudDownload,
            Self::BoardOpenRemote => Icon::Globe,
            Self::BoardDeleteCard => Icon::Trash,
            Self::BoardSettings => Icon::Settings2,
            Self::BoardReload => Icon::RefreshCw,
            Self::BoardFilter => Icon::Search,
            Self::BoardAttachRun => Icon::Paperclip,
            Self::BoardCancelRun => Icon::CircleStop,
            Self::BoardRunNow => Icon::Zap,
            Self::BoardPickBlockedBy => Icon::CircleSlash,
            Self::BoardPickAgent => Icon::Bot,
            Self::BoardColumns => Icon::Boxes,
            Self::CardDetailClose => Icon::X,
            Self::CardDetailEditTitle => Icon::FilePen,
            Self::CardDetailEditDescription => Icon::FilePen,
            Self::CardDetailAddComment => Icon::Plus,
            Self::CardDetailNextProperty => Icon::CircleArrowDown,
            Self::CardDetailPrevProperty => Icon::CircleArrowUp,
            Self::CardDetailEditProperty => Icon::FilePen,
            Self::CardDetailCreateWorktree => Icon::GitBranchPlus,
            Self::CardDetailOpenRemote => Icon::Globe,
            Self::CardDetailKeepLocal => Icon::CloudUpload,
            Self::CardDetailTakeRemote => Icon::CloudDownload,
            Self::CardDetailSave => Icon::Check,

            Self::WorkspaceOpenBoard => Icon::Boxes,
            Self::NewWorktree => Icon::GitBranchPlus,
            Self::CloneRepo => Icon::CloudDownload,
            Self::DeleteWorktree | Self::DeleteContext => Icon::Trash,
            Self::PruneWorktrees => Icon::Scissors,
            Self::SleepSession => Icon::Moon,
            Self::KillSession | Self::QuitDaemon => Icon::Power,
            Self::InspectWorktree => Icon::Eye,
            Self::MoveRepo => Icon::ArrowRightLeft,
            Self::NewContext | Self::EditContext => Icon::Boxes,
            Self::PullRequests => Icon::GitPullRequest,
            Self::Worktrees => Icon::GitBranch,
            Self::JobsPanel => Icon::Clock,
            Self::Settings => Icon::Settings2,
            Self::Help => Icon::CircleQuestionMark,
            Self::Refresh => Icon::LoaderCircle,
            Self::UpdateFleet => Icon::CircleArrowUp,
            Self::OpenClaude | Self::OpenCodex => Icon::Bot,
            Self::Quit => Icon::CircleX,
        }
    }

    /// Whether the row wears `triangle-alert`, from the catalogue. It still goes through its
    /// confirm.
    #[must_use]
    pub fn destructive(self) -> bool {
        self.info().destructive
    }

    /// The action whose bound key this row shows.
    #[must_use]
    pub const fn action(self) -> &'static str {
        match self {
            Self::BoardGoBoard => "board::GoBoard",
            Self::BoardPrevColumn => "board::PrevColumn",
            Self::BoardNextColumn => "board::NextColumn",
            Self::BoardNextCard => "board::NextCard",
            Self::BoardPrevCard => "board::PrevCard",
            Self::BoardOpenCard => "board::OpenCard",
            Self::BoardNewCard => "board::NewCard",
            Self::BoardPickStatus => "board::PickStatus",
            Self::BoardPickPriority => "board::PickPriority",
            Self::BoardPickAssignee => "board::PickAssignee",
            Self::BoardPickLabels => "board::PickLabels",
            Self::BoardPickEstimate => "board::PickEstimate",
            Self::BoardMovePrevColumn => "board::MovePrevColumn",
            Self::BoardMoveNextColumn => "board::MoveNextColumn",
            Self::BoardCreateWorktree => "board::CreateWorktree",
            Self::BoardOpenWorktree => "board::OpenWorktree",
            Self::BoardSync => "board::Sync",
            Self::BoardFullSync => "board::FullSync",
            Self::BoardOpenRemote => "board::OpenRemote",
            Self::BoardDeleteCard => "board::DeleteCard",
            Self::BoardSettings => "board::Settings",
            Self::BoardReload => "board::Reload",
            Self::BoardFilter => "board::Filter",
            Self::BoardAttachRun => "board::AttachRun",
            Self::BoardCancelRun => "board::CancelRun",
            Self::BoardRunNow => "board::RunNow",
            Self::BoardPickBlockedBy => "board::PickBlockedBy",
            Self::BoardPickAgent => "board::PickAgent",
            Self::BoardColumns => "board::Columns",
            Self::CardDetailClose => "card_detail::Close",
            Self::CardDetailEditTitle => "card_detail::EditTitle",
            Self::CardDetailEditDescription => "card_detail::EditDescription",
            Self::CardDetailAddComment => "card_detail::AddComment",
            Self::CardDetailNextProperty => "card_detail::NextProperty",
            Self::CardDetailPrevProperty => "card_detail::PrevProperty",
            Self::CardDetailEditProperty => "card_detail::EditProperty",
            Self::CardDetailCreateWorktree => "card_detail::CreateWorktree",
            Self::CardDetailOpenRemote => "card_detail::OpenRemote",
            Self::CardDetailKeepLocal => "card_detail::KeepLocal",
            Self::CardDetailTakeRemote => "card_detail::TakeRemote",
            Self::CardDetailSave => "card_detail::Save",

            Self::WorkspaceOpenBoard => "prefix::OpenBoard",
            Self::NewWorktree => "worktrees::Create",
            Self::CloneRepo => "repos::Clone",
            Self::DeleteWorktree => "worktrees::Delete",
            Self::PruneWorktrees => "worktrees::Prune",
            Self::SleepSession => "worktrees::Sleep",
            Self::KillSession => "worktrees::Kill",
            Self::InspectWorktree => "worktrees::Inspect",
            Self::MoveRepo => "repos::MoveToContext",
            Self::NewContext => "hub::NewContext",
            Self::EditContext => "hub::EditContext",
            Self::DeleteContext => "hub::DeleteContext",
            Self::PullRequests => "hub::GoPrs",
            Self::Worktrees => "hub::GoWorktrees",
            Self::JobsPanel => "fleet::OpenJobs",
            Self::Settings => "fleet::OpenSettings",
            Self::Help => "fleet::OpenHelp",
            Self::Refresh => "fleet::Refresh",
            Self::UpdateFleet => "fleet::UpdateFleet",
            Self::OpenClaude => "fleet::OpenAgentClaude",
            Self::OpenCodex => "fleet::OpenAgentCodex",
            Self::Quit => "fleet::Quit",
            Self::QuitDaemon => "fleet::QuitAndStopDaemon",
        }
    }

    /// Whether the command can run right now. Invalid commands are not listed (§3.9).
    ///
    /// Listing goes through [`Self::valid_with`], which resolves the board's card once for the
    /// whole of `ALL`; this is the same question asked about one command in isolation.
    #[cfg(test)]
    #[must_use]
    pub fn valid(self, state: &AppState) -> bool {
        self.valid_with(state, card_context(state, None, None))
    }

    /// `valid` with the board lookup hoisted out.
    ///
    /// [`card_context`] sorts a column and lowercases every card field a filter touches. Doing
    /// that once per command, for all of `ALL`, on every palette keystroke is the whole cost of
    /// listing the palette on a board of any size.
    pub(super) fn valid_with(self, state: &AppState, card: CardContext) -> bool {
        let has_card = card.present;
        let snapshot = state.snapshot.as_ref();
        let has_repo = snapshot.is_some_and(|snapshot| !snapshot.repos.is_empty());
        let connected = state.daemon.is_connected();
        let on_prs = matches!(state.screen, Screen::Hub { tab: HubTab::Prs });
        let on_board = matches!(state.screen, Screen::Hub { tab: HubTab::Board });
        match self {
            Self::BoardGoBoard => state.active_context().is_some(),
            Self::BoardPrevColumn
            | Self::BoardNextColumn
            | Self::BoardNextCard
            | Self::BoardPrevCard
            | Self::BoardNewCard
            | Self::BoardSync
            | Self::BoardFullSync
            | Self::BoardSettings
            | Self::BoardReload
            | Self::BoardFilter => on_board,
            Self::BoardOpenCard | Self::BoardCreateWorktree => has_card,
            // `d` refuses every mirrored card — the sync would file the issue again as a new
            // card — so on a linked board this row could only ever fail (§3.9).
            Self::BoardDeleteCard => has_card && !card.mirrored,
            // A field the board's backend owns can only answer the read-only refusal, so the
            // row is not listed at all — the same rule `BoardOpenRemote` follows (§3.9).
            Self::BoardPickStatus | Self::BoardMovePrevColumn | Self::BoardMoveNextColumn => {
                has_card && !state.is_readonly_field("status_id")
            }
            Self::BoardPickPriority => has_card && !state.is_readonly_field("priority"),
            Self::BoardPickAssignee => has_card && !state.is_readonly_field("assignee"),
            Self::BoardPickLabels => has_card && !state.is_readonly_field("labels"),
            Self::BoardPickEstimate => has_card && !state.is_readonly_field("estimate"),
            // Runs exist only on a worktree board: over the Hub's context board the three rows
            // could only ever answer the daemon's capability refusal (contracts §5.5). The
            // board pane is the second surface the keys are bound on, and the palette lists a
            // row exactly where its key would fire.
            Self::BoardAttachRun | Self::BoardCancelRun | Self::BoardRunNow => {
                matches!(
                    state.board.scope,
                    Some(crate::state::BoardScope::Worktree(_))
                ) && ((state.board_pane_is_active()
                    && crate::screens::board::selected_card(state).is_some())
                    || (card.detail && has_card))
            }
            // Both edit a field every board has, so they follow the pickers above; `blocked_by`
            // is Fleet's own link and no backend owns it.
            Self::BoardPickBlockedBy | Self::BoardPickAgent => has_card,
            Self::BoardColumns => on_board,
            // A card the backend has not linked, or linked without publishing an address, has
            // no remote issue: the row would open a browser tab at nothing.
            Self::BoardOpenRemote | Self::CardDetailOpenRemote => has_card && card.remote,
            // Opening the worktree of a card that has none is a row with nothing behind it.
            Self::BoardOpenWorktree => has_card && card.worktree,
            // Closing and saving only mean anything on a detail that is already open behind
            // the palette: from the board they seed a fresh one, act on nothing, and leave a
            // dialog the user never asked for (§3.9 lists no row that cannot run).
            Self::CardDetailClose | Self::CardDetailSave => has_card && card.detail,
            Self::CardDetailEditTitle => has_card,
            Self::CardDetailEditDescription => has_card,
            Self::CardDetailAddComment => has_card,
            Self::CardDetailNextProperty => has_card,
            Self::CardDetailPrevProperty => has_card,
            Self::CardDetailEditProperty => has_card,
            Self::CardDetailCreateWorktree => has_card,
            // A resolution needs something to resolve; on a clean card both rows open the
            // detail and then return without doing anything.
            Self::CardDetailKeepLocal | Self::CardDetailTakeRemote => has_card && card.conflicted,
            // The key is a Workspace prefix row, and its handler lives on the Workspace root:
            // listed anywhere else the row would dispatch an action nothing is listening for.
            Self::WorkspaceOpenBoard => {
                connected
                    && state
                        .active_session()
                        .is_some_and(|session| matches!(session.kind, SessionKind::Worktree(_)))
            }
            Self::NewWorktree | Self::PruneWorktrees => {
                connected && crate::dialogs::focused_repo(state).is_some()
            }
            Self::CloneRepo => connected && state.active_context().is_some(),
            Self::MoveRepo => connected && crate::dialogs::focused_repo(state).is_some(),
            Self::DeleteWorktree | Self::InspectWorktree | Self::SleepSession => {
                connected && selected_worktree(state).is_some()
            }
            Self::KillSession => connected && target_session(state).is_some(),
            Self::EditContext | Self::DeleteContext => {
                connected && state.active_context().is_some()
            }
            Self::PullRequests => !on_prs && has_repo,
            Self::Worktrees => on_prs || on_board,
            Self::UpdateFleet => connected && state.update_version.is_some(),
            Self::NewContext
            | Self::Settings
            | Self::Refresh
            | Self::OpenClaude
            | Self::OpenCodex
            | Self::QuitDaemon => connected,
            Self::JobsPanel | Self::Help | Self::Quit => true,
        }
    }
}

/// Whether the board tab has a focused card for the `Board:` and `Card detail:` rows.
/// What the board's selection and the palette's backdrop offer the card rows.
///
/// Resolved once per palette render and handed to every command, because finding the selected
/// card sorts a column and lowercases every field the filter touches.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CardContext {
    /// A card is selected on the board tab.
    present: bool,
    /// That card carries an unresolved conflict.
    conflicted: bool,
    /// That card owns a worktree.
    worktree: bool,
    /// That card is linked to a remote issue with a browsable address.
    remote: bool,
    /// That card is linked to a remote issue at all, address or not.
    ///
    /// `remote` answers "can `x` open something"; this answers "does the backend own this
    /// card", which is what `d` refuses on — a linked card with no site setting is neither
    /// openable nor deletable, and one flag cannot say both.
    mirrored: bool,
    /// The card detail is the dialog the palette was opened over.
    detail: bool,
}

/// The places whose commands act on what the surface under the palette is showing, most
/// specific first.
///
/// A command bound to one of them works on the current selection — the selected worktree, the
/// open card — so a typed query ranks it above a namesake that works on the whole context or
/// anywhere: `del` on Worktrees is "Delete the worktree safely", not "Delete this context".
pub(super) fn here(state: &AppState, card: CardContext) -> &'static [Place] {
    if card.detail {
        return &[Place::CardDetail, Place::Board];
    }
    match &state.screen {
        Screen::Hub {
            tab: HubTab::Worktrees,
        } => &[Place::Worktrees],
        Screen::Hub { tab: HubTab::Prs } => &[Place::PullRequests],
        Screen::Hub { tab: HubTab::Board } => &[Place::Board, Place::CardDetail],
        Screen::Workspace { .. } if state.board_pane_is_active() => {
            &[Place::Board, Place::CardDetail]
        }
        Screen::Workspace { .. } if state.active_agent_thread().is_some() => &[Place::AgentThread],
        Screen::Workspace { .. } => &[Place::Terminal],
    }
}

pub(super) fn card_context(
    state: &AppState,
    behind: Option<Dialogs>,
    detail_card: Option<&CardId>,
) -> CardContext {
    let detail = behind == Some(Dialogs::CardDetail);
    let selected = matches!(state.screen, Screen::Hub { tab: HubTab::Board })
        .then(|| crate::screens::board::selected_card(state))
        .flatten();
    // The `Card detail:` rows act on the card the open dialog is holding, and that card
    // deliberately survives a refresh that moves the board's selection. Asking the board
    // instead hides "Open remote issue" for a card that has one, and offers "Keep local" for a
    // card with no conflict.
    let card = detail
        .then(|| {
            detail_card.and_then(|id| {
                state
                    .board()
                    .and_then(|view| view.cards.iter().find(|card| card.id == *id))
            })
        })
        .flatten()
        .or(selected);
    CardContext {
        present: card.is_some(),
        conflicted: card.is_some_and(|card| card.conflict.is_some()),
        worktree: card.is_some_and(|card| card.worktree_id.is_some()),
        remote: card.is_some_and(|card| crate::screens::board::remote_url(card).is_some()),
        mirrored: card.is_some_and(|card| card.remote.is_some()),
        detail,
    }
}
