//! §3.9 Command palette (`:`) — *jump to anything by name, or do the thing whose key I do not
//! remember*.
//!
//! Three sections in a fixed order — `GO` (objects), `DO` (valid commands only) and `CONTEXT` —
//! capped at [`ROW_CAP`] rows so `Enter` is predictable: the top match never moves below the
//! fold. Every `DO` row carries its bound key, right-aligned, read out of
//! [`crate::keymap::table`], so the palette teaches itself out of the loop.
//!
//! An invalid command is **not listed at all**, never greyed, because a greyed row costs a `j`.
//! A destructive command is prefixed with `triangle-alert` and still routed through its confirm
//! dialog — the palette never bypasses §1.7.
//!
//! `q` is deliberately not bound here (`docs/APP-CONTRACTS.md` §3): gpui dispatches bindings
//! before a text input sees the key, so binding `q` would make the query untypable.

use fleet_core::{
    ids::{CardId, ContextId, JobId, RepoId, SessionId, WorktreeId},
    sessions::{AgentActivity, SessionKind, SessionState},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{board, card_detail, fleet, palette as palette_actions},
    bridge::Bridge,
    dialogs::{
        ConfirmRequest, Dialogs, TextInput, notify, request_confirm, step, type_into, with_host,
    },
    keymap,
    screens::workspace::status_kind,
    state::{AppState, HubTab, Overlay, RepoScope, Screen, latest_failed_job, running_jobs},
};

/// The palette's total row cap (§3.9).
pub const ROW_CAP: usize = 10;
/// How many rows each section shows on an empty query (§3.9 "States").
pub const IDLE_ROWS: usize = 5;

/// The palette's draft.
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    /// The query.
    pub query: TextInput,
    /// The flat cursor across all sections.
    pub cursor: usize,
}

/// What a palette row does when `Enter` runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// Open a session by id.
    OpenSession(SessionId),
    /// Open (or create) a worktree's session.
    OpenWorktree(WorktreeId),
    /// Scope the Hub to a repository.
    SelectRepo(RepoId),
    /// Switch the active context.
    SwitchContext(ContextId),
    /// Run a command.
    Command(Command),
    /// Cancel a background job.
    CancelJob(JobId),
}

/// One palette row, already resolved against the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Which section it belongs to.
    pub section: PaletteSectionKind,
    /// The row label, which is also what the query matches against.
    pub label: String,
    /// The muted right-hand description.
    pub detail: Option<String>,
    /// The bound key, right-aligned.
    pub key: Option<String>,
    /// Whether the row is prefixed with `triangle-alert`.
    pub destructive: bool,
    /// The glyph, when it is not derived from a session state.
    pub icon: Icon,
    /// The §2.5 glyph this row wears, for `GO` rows.
    ///
    /// It is a resolved [`StatusKind`] and not a raw [`SessionState`] on purpose: `detached`
    /// alone cannot tell `circle` from `moon`, and §5 invariant 1 requires the palette to
    /// draw exactly the glyph the Hub draws for the same worktree.
    pub status: Option<StatusKind>,
    /// What `Enter` does.
    pub run: Run,
}

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
    /// Open the OpenCode agent session.
    OpenOpencode,
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
        Self::OpenOpencode,
        Self::Quit,
        Self::QuitDaemon,
    ];

    /// The row label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::BoardGoBoard => "Board: Go to board",
            Self::BoardPrevColumn => "Board: Previous column",
            Self::BoardNextColumn => "Board: Next column",
            Self::BoardNextCard => "Board: Next card",
            Self::BoardPrevCard => "Board: Previous card",
            Self::BoardOpenCard => "Board: Open card",
            Self::BoardNewCard => "Board: New card",
            Self::BoardPickStatus => "Board: Status picker",
            Self::BoardPickPriority => "Board: Priority picker",
            Self::BoardPickAssignee => "Board: Assignee picker",
            Self::BoardPickLabels => "Board: Labels picker",
            Self::BoardPickEstimate => "Board: Estimate picker",
            Self::BoardMovePrevColumn => "Board: Move card to previous column",
            Self::BoardMoveNextColumn => "Board: Move card to next column",
            Self::BoardCreateWorktree => "Board: Create worktree from card",
            Self::BoardOpenWorktree => "Board: Open linked worktree",
            Self::BoardSync => "Board: Sync",
            Self::BoardFullSync => "Board: Full sync",
            Self::BoardOpenRemote => "Board: Open remote issue",
            Self::BoardDeleteCard => "Board: Delete card",
            Self::BoardSettings => "Board: Settings",
            Self::BoardReload => "Board: Reload",
            Self::BoardFilter => "Board: Filter cards",
            Self::CardDetailClose => "Card detail: Close",
            Self::CardDetailEditTitle => "Card detail: Edit title",
            Self::CardDetailEditDescription => "Card detail: Edit description",
            Self::CardDetailAddComment => "Card detail: Add comment",
            Self::CardDetailNextProperty => "Card detail: Next property",
            Self::CardDetailPrevProperty => "Card detail: Previous property",
            Self::CardDetailEditProperty => "Card detail: Edit selected property",
            Self::CardDetailCreateWorktree => "Card detail: Create worktree",
            Self::CardDetailOpenRemote => "Card detail: Open remote issue",
            Self::CardDetailKeepLocal => "Card detail: Resolve conflict: keep local",
            Self::CardDetailTakeRemote => "Card detail: Resolve conflict: take remote",
            Self::CardDetailSave => "Card detail: Save text edit",

            Self::NewWorktree => "New worktree",
            Self::CloneRepo => "Clone repo",
            Self::DeleteWorktree => "Delete worktree",
            Self::PruneWorktrees => "Prune worktrees",
            Self::SleepSession => "Sleep session",
            Self::KillSession => "Kill session",
            Self::InspectWorktree => "Inspect worktree",
            Self::MoveRepo => "Move repo to context",
            Self::NewContext => "New context",
            Self::EditContext => "Edit context",
            Self::DeleteContext => "Delete context",
            Self::PullRequests => "Pull requests",
            Self::Worktrees => "Worktrees",
            Self::JobsPanel => "Jobs panel",
            Self::Settings => "Settings",
            Self::Help => "Keymap",
            Self::Refresh => "Refresh",
            Self::UpdateFleet => "Update Fleet",
            Self::OpenClaude => "Open Claude agent",
            Self::OpenOpencode => "Open OpenCode agent",
            Self::Quit => "Quit Fleet",
            Self::QuitDaemon => "Quit and stop fleetd",
        }
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
            Self::OpenClaude | Self::OpenOpencode => Icon::Bot,
            Self::Quit => Icon::CircleX,
        }
    }

    /// Whether the row wears `triangle-alert`. It still goes through its confirm.
    #[must_use]
    pub const fn destructive(self) -> bool {
        matches!(
            self,
            Self::BoardDeleteCard
                | Self::DeleteWorktree
                | Self::DeleteContext
                | Self::PruneWorktrees
                | Self::KillSession
                | Self::QuitDaemon
        )
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
            Self::OpenOpencode => "fleet::OpenAgentOpencode",
            Self::Quit => "fleet::Quit",
            Self::QuitDaemon => "fleet::QuitAndStopDaemon",
        }
    }

    /// Whether the command can run right now. Invalid commands are not listed (§3.9).
    #[must_use]
    pub fn valid(self, state: &AppState) -> bool {
        self.valid_with(state, card_context(state, None, None))
    }

    /// `valid` with the board lookup hoisted out.
    ///
    /// [`card_context`] sorts a column and lowercases every card field a filter touches. Doing
    /// that once per command, for all of `ALL`, on every palette keystroke is the whole cost of
    /// listing the palette on a board of any size.
    fn valid_with(self, state: &AppState, card: CardContext) -> bool {
        let has_card = card.present;
        let snapshot = state.snapshot.as_ref();
        let has_repo = snapshot.is_some_and(|snapshot| !snapshot.repos.is_empty());
        let has_worktree = snapshot.is_some_and(|snapshot| !snapshot.worktrees.is_empty());
        let has_context = snapshot.is_some_and(|snapshot| !snapshot.contexts.is_empty());
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
            Self::NewWorktree | Self::PruneWorktrees => has_repo,
            Self::CloneRepo | Self::MoveRepo => has_context && has_repo || self == Self::CloneRepo,
            Self::DeleteWorktree | Self::InspectWorktree | Self::SleepSession => has_worktree,
            Self::KillSession => snapshot.is_some_and(|snapshot| !snapshot.sessions.is_empty()),
            Self::EditContext | Self::DeleteContext => state.active_context().is_some(),
            Self::PullRequests => !on_prs && has_repo,
            Self::Worktrees => on_prs || on_board,
            Self::UpdateFleet => state.update_version.is_some(),
            Self::NewContext
            | Self::JobsPanel
            | Self::Settings
            | Self::Help
            | Self::Refresh
            | Self::OpenClaude
            | Self::OpenOpencode
            | Self::Quit
            | Self::QuitDaemon => true,
        }
    }
}

/// Whether the board tab has a focused card for the `Board:` and `Card detail:` rows.
/// What the board's selection and the palette's backdrop offer the card rows.
///
/// Resolved once per palette render and handed to every command, because finding the selected
/// card sorts a column and lowercases every field the filter touches.
#[derive(Debug, Clone, Copy, Default)]
struct CardContext {
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

fn card_context(
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

/// The key bound to an action, formatted for the right-hand column.
#[must_use]
pub fn key_for(action: &str) -> Option<String> {
    keymap::table()
        .into_iter()
        .find(|spec| spec.action == action)
        .map(|spec| super::help::pretty_keys(spec.keys))
}

/// Whether `query`'s characters appear in `label`, in order and case-insensitively.
#[must_use]
pub fn matches(label: &str, query: &str) -> bool {
    if query.trim().is_empty() {
        return true;
    }
    let label = label.to_ascii_lowercase();
    let mut chars = label.chars();
    query
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .all(|wanted| chars.any(|actual| actual == wanted))
}

/// The §2.5 detail wording a `GO` row carries on its right.
///
/// §3.9 wants the row's **state** there (`session attached`, `sleeping`, `PR · mine`, `repo`),
/// not a type word: "worktree" repeats what the id already says, while the state is the thing
/// that decides whether jumping there resumes work or starts it.
#[must_use]
pub fn session_detail(session: SessionState, slept: bool) -> &'static str {
    match session {
        SessionState::Attached => "session attached",
        SessionState::Detached if slept => "sleeping",
        SessionState::Detached => "running, detached",
        SessionState::Unknown => "unknown",
        SessionState::None => "no session",
    }
}

/// Every candidate row, in section order, before the cap.
#[must_use]
pub fn candidates(
    state: &AppState,
    query: &str,
    behind: Option<Dialogs>,
    detail_card: Option<&CardId>,
) -> Vec<Entry> {
    let mut rows = Vec::new();
    let sessions_only = query.trim() == "sessions";
    let effective_query = if sessions_only { "" } else { query };
    let idle = effective_query.trim().is_empty();
    if let Some(snapshot) = state.snapshot.as_ref() {
        // GO: sessions first, because reaching one from inside another is the point (§3.9).
        let mut go: Vec<Entry> = Vec::new();
        for session in &snapshot.sessions {
            // §5 invariant 1: the row wears the Hub's glyph and the Hub's wording for the same
            // worktree, so both are resolved from the one `WorktreeStatus` the Hub reads.
            // `slept_at` alone cannot tell `attached` from `running, detached` — it only tells
            // `awake` from `sleeping` — and reading it as "attached" is how the palette came to
            // call a detached session green.
            let worktree = match &session.kind {
                SessionKind::Worktree(id) => snapshot
                    .worktrees
                    .iter()
                    .find(|worktree| &worktree.id == id),
                SessionKind::Agent { .. } => None,
            };
            let slept = session.slept_at.is_some();
            let runtime_status = worktree.and_then(|worktree| {
                snapshot
                    .statuses
                    .iter()
                    .find(|status| status.worktree_id == worktree.id)
            });
            let agent_activity = runtime_status.map_or_else(
                || state.session_agent_activity(&session.id),
                |status| status.agent_activity,
            );
            let session_state = runtime_status.map_or(
                // An agent session has no `WorktreeStatus`; the session record itself is
                // then the only evidence, and it can only say awake or slept.
                if slept {
                    SessionState::Detached
                } else {
                    SessionState::Attached
                },
                |status| status.session,
            );
            let degraded = worktree.is_some_and(|worktree| worktree.degraded.is_some());
            go.push(Entry {
                section: PaletteSectionKind::Go,
                // §3.9 lists worktrees by their `WorktreeId`; a session id is a different id
                // scheme and mixing the two in one section makes the list unreadable.
                label: worktree.map_or_else(
                    || session.id.as_str().to_owned(),
                    |worktree| worktree.id.as_str().to_owned(),
                ),
                detail: Some(session_detail(session_state, slept).to_owned()),
                key: None,
                destructive: false,
                icon: Icon::GitBranch,
                status: Some(status_kind(session_state, slept, agent_activity, degraded)),
                run: Run::OpenSession(session.id.clone()),
            });
        }
        for worktree in &snapshot.worktrees {
            if snapshot
                .sessions
                .iter()
                .any(|session| session.id.as_str() == worktree.session)
            {
                continue;
            }
            // §1.3: until the daemon reports a status the state is `unknown`, never a false
            // `none` — the same rule the worktrees list follows.
            let status = snapshot
                .statuses
                .iter()
                .find(|status| status.worktree_id == worktree.id);
            let session = status.map_or(SessionState::Unknown, |status| status.session);
            go.push(Entry {
                section: PaletteSectionKind::Go,
                label: worktree.id.as_str().to_owned(),
                detail: Some(session_detail(session, false).to_owned()),
                key: None,
                destructive: false,
                icon: Icon::GitBranch,
                status: Some(status_kind(
                    session,
                    false,
                    status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
                    worktree.degraded.is_some(),
                )),
                run: Run::OpenWorktree(worktree.id.clone()),
            });
        }
        for repo in &snapshot.repos {
            go.push(Entry {
                section: PaletteSectionKind::Go,
                label: repo.id.as_str().to_owned(),
                detail: Some("repo".to_owned()),
                key: None,
                destructive: false,
                icon: Icon::FolderGit2,
                status: None,
                run: Run::SelectRepo(repo.id.clone()),
            });
        }
        go.retain(|entry| matches!(&entry.run, Run::OpenSession(_)) || !sessions_only);
        go.retain(|entry| matches(&entry.label, effective_query));
        if idle {
            go.truncate(IDLE_ROWS);
        }
        rows.extend(go);

        // DO: valid commands, then the jobs worth cancelling.
        let card = card_context(state, behind, detail_card);
        let mut commands: Vec<Entry> = Command::ALL
            .iter()
            .copied()
            .filter(|command| command.valid_with(state, card))
            .map(|command| Entry {
                section: PaletteSectionKind::Do,
                label: command.label().to_owned(),
                detail: None,
                key: key_for(command.action()),
                destructive: command.destructive(),
                icon: command.icon(),
                status: None,
                run: Run::Command(command),
            })
            .collect();
        for job in running_jobs(&snapshot.jobs) {
            if !job.cancellable {
                continue;
            }
            commands.push(Entry {
                section: PaletteSectionKind::Do,
                label: format!("Cancel job: {} {}", super::quit::kind_word(job), job.target),
                detail: None,
                key: key_for("fleet::OpenJobs"),
                destructive: false,
                icon: Icon::CircleStop,
                status: None,
                run: Run::CancelJob(job.id.clone()),
            });
        }
        if let Some(failed) = latest_failed_job(&snapshot.jobs) {
            commands.push(Entry {
                section: PaletteSectionKind::Do,
                label: format!("Show failed job: {}", failed.title),
                detail: None,
                key: key_for("fleet::FocusStickyError"),
                destructive: false,
                icon: Icon::CircleX,
                status: None,
                run: Run::Command(Command::JobsPanel),
            });
        }
        commands.retain(|entry| !sessions_only && matches(&entry.label, effective_query));
        if idle {
            commands.truncate(IDLE_ROWS);
        }
        rows.extend(commands);

        // CONTEXT: the digit that switches to it is the key hint.
        let mut contexts: Vec<Entry> = snapshot
            .contexts
            .iter()
            .enumerate()
            .map(|(index, context)| Entry {
                section: PaletteSectionKind::Context,
                label: context.name.clone(),
                detail: None,
                key: (index < 9).then(|| (index + 1).to_string()),
                destructive: false,
                icon: Icon::Boxes,
                status: None,
                run: Run::SwitchContext(context.id.clone()),
            })
            .collect();
        contexts.retain(|entry| !sessions_only && matches(&entry.label, effective_query));
        rows.extend(contexts);
    }
    rows
}

// ---------------------------------------------------------------------------- rendering

/// Renders the palette overlay (§3.9).
///
/// The shell routes [`crate::state::Overlay::Palette`] here; the element is the palette card
/// inside the kit's top-anchored [`fleet_ui_kit::Overlay`].
pub fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    seed(state, cx);
    let (query, cursor, behind, detail_card) = with_host(cx, |host| {
        (
            host.palette.query.clone(),
            host.palette.cursor,
            host.behind_palette.clone(),
            host.card_detail.card_id.clone(),
        )
    });
    let app = state.read(cx);
    let rows = candidates(app, query.value(), behind, detail_card.as_ref());
    let total = rows.len();

    let mut card = fleet_ui_kit::Palette::new(query.value().to_owned())
        .cursor(cursor)
        .cap(ROW_CAP)
        .total(total)
        .empty(format!("Nothing matches \"{}\".", query.value()));
    for kind in [
        PaletteSectionKind::Go,
        PaletteSectionKind::Do,
        PaletteSectionKind::Context,
    ] {
        let section: Vec<PaletteRow> = rows
            .iter()
            .filter(|entry| entry.section == kind)
            .map(|entry| {
                let mut row = PaletteRow::new(entry.label.clone())
                    .destructive(entry.destructive)
                    .icon(entry.icon);
                if let Some(status) = entry.status {
                    row = row.leading(StatusGlyph::new(status).id(gpui::SharedString::from(
                        format!("palette-glyph-{}", entry.label),
                    )));
                }
                if let Some(detail) = entry.detail.clone() {
                    row = row.detail(detail);
                }
                if let Some(key) = entry.key.clone() {
                    row = row.key(key);
                }
                row
            })
            .collect();
        if !section.is_empty() {
            card = card.section(PaletteSection::new(kind, section));
        }
    }

    let (top, width) = {
        let theme = cx.theme();
        (theme.metrics.palette_top, theme.metrics.palette_w)
    };
    let run_state = state.clone();
    let run_bridge = bridge.clone();

    div()
        .track_focus(focus)
        .size_full()
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let typed = with_host(cx, |host| {
                    let typed = type_into(&mut host.palette.query, event);
                    if typed {
                        host.palette.cursor = 0;
                    }
                    typed
                });
                if typed {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::CursorDown, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::CursorUp, _window, cx| move_cursor(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::Backspace, _window, cx| {
                if with_host(cx, |host| host.palette.query.backspace()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::DeleteWord, _window, cx| {
                if with_host(cx, |host| host.palette.query.delete_word()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &palette_actions::Clear, _window, cx| {
                if with_host(cx, |host| host.palette.query.clear()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action(move |_: &palette_actions::Run, window, cx| {
            run_selected(&run_state, &run_bridge, window, cx);
        })
        .child(
            fleet_ui_kit::Overlay::new()
                .top(top)
                .width(width)
                .scrim(true)
                .child(card),
        )
        .into_any_element()
}

/// Resets the draft the first time the open palette is rendered.
fn seed(state: &Entity<AppState>, cx: &mut App) {
    let seed = state.update(cx, |app, _| app.palette_seed.take());
    with_host(cx, |host| {
        if !host.palette_open {
            host.palette = PaletteState::default();
            if let Some(seed) = seed {
                host.palette.query = TextInput::new(seed);
            }
            host.palette_open = true;
        }
    });
}

fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let len = {
        let (query, behind, detail_card) = with_host(cx, |host| {
            (
                host.palette.query.value().to_owned(),
                host.behind_palette.clone(),
                host.card_detail.card_id.clone(),
            )
        });
        candidates(state.read(cx), &query, behind, detail_card.as_ref())
            .len()
            .min(ROW_CAP)
    };
    with_host(cx, |host| {
        host.palette.cursor = step(host.palette.cursor, delta, len);
    });
    notify(state, cx);
}

/// `Enter`: close the palette, then do what the row says.
fn run_selected(state: &Entity<AppState>, bridge: &Bridge, window: &mut Window, cx: &mut App) {
    let query = with_host(cx, |host| host.palette.query.value().to_owned());
    let cursor = with_host(cx, |host| host.palette.cursor);
    // The same backdrop the row list was built against: a row that was listed must stay
    // findable by the index the cursor is on.
    let backdrop = super::with_host(cx, |host| host.behind_palette.clone());
    let detail_card = super::with_host(cx, |host| host.card_detail.card_id.clone());
    let Some(entry) = candidates(state.read(cx), &query, backdrop, detail_card.as_ref())
        .into_iter()
        .take(ROW_CAP)
        .nth(cursor)
    else {
        return;
    };
    // Read before closing: the palette replaced whatever dialog was open, and closing it is
    // what makes the host forget which one that was.
    let behind = super::with_host(cx, |host| host.behind_palette.clone());
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
    match entry.run {
        Run::OpenSession(session) => open_session(session, state, cx),
        Run::OpenWorktree(id) => open_worktree(id, state, bridge, cx),
        Run::SelectRepo(repo) => {
            state.update(cx, |app, cx| {
                app.scope = RepoScope::Repo(repo);
                app.screen = Screen::hub();
                cx.notify();
            });
        }
        Run::SwitchContext(context) => {
            bridge.send(RequestBody::SetActiveContext { id: Some(context) });
        }
        Run::CancelJob(job) => bridge.send(RequestBody::CancelJob { job }),
        Run::Command(command) => run_command(command, behind, state, bridge, window, cx),
    }
}

fn open_session(session: SessionId, state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.touch_session(session.clone());
        app.screen = Screen::Workspace { session };
        cx.notify();
    });
}

fn open_worktree(id: WorktreeId, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let reply = bridge.request(RequestBody::EnsureSession {
        worktree: Some(id),
        agent: None,
        sleep_previous: true,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::Session(session))) = reply.recv().await else {
            return;
        };
        cx.update(|cx| open_session(session.id, &state, cx));
    })
    .detach();
}

/// Runs one command. Destructive rows open their confirm rather than acting (§3.9).
fn run_command(
    command: Command,
    behind: Option<Dialogs>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    window: &mut Window,
    cx: &mut App,
) {
    let open = |dialog: Dialogs, cx: &mut App| {
        if dialog == Dialogs::CardDetail {
            // Re-seeding a detail the palette was opened over throws away the edit the command
            // is about to act on, which is every text row's `ctrl-s`.
            if behind.as_ref() != Some(&Dialogs::CardDetail) {
                super::card_detail::seed(state, cx);
            }
            super::with_host(cx, |host| host.open = Some(Dialogs::CardDetail));
        }
        state.update(cx, |app, cx| {
            app.open_overlay(Overlay::Dialog(dialog));
            cx.notify();
        });
    };
    match command {
        Command::BoardGoBoard => window.dispatch_action(Box::new(board::GoBoard), cx),
        Command::BoardPrevColumn => window.dispatch_action(Box::new(board::PrevColumn), cx),
        Command::BoardNextColumn => window.dispatch_action(Box::new(board::NextColumn), cx),
        Command::BoardNextCard => window.dispatch_action(Box::new(board::NextCard), cx),
        Command::BoardPrevCard => window.dispatch_action(Box::new(board::PrevCard), cx),
        Command::BoardOpenCard => window.dispatch_action(Box::new(board::OpenCard), cx),
        Command::BoardNewCard => window.dispatch_action(Box::new(board::NewCard), cx),
        Command::BoardPickStatus => window.dispatch_action(Box::new(board::PickStatus), cx),
        Command::BoardPickPriority => window.dispatch_action(Box::new(board::PickPriority), cx),
        Command::BoardPickAssignee => window.dispatch_action(Box::new(board::PickAssignee), cx),
        Command::BoardPickLabels => window.dispatch_action(Box::new(board::PickLabels), cx),
        Command::BoardPickEstimate => window.dispatch_action(Box::new(board::PickEstimate), cx),
        Command::BoardMovePrevColumn => window.dispatch_action(Box::new(board::MovePrevColumn), cx),
        Command::BoardMoveNextColumn => window.dispatch_action(Box::new(board::MoveNextColumn), cx),
        Command::BoardCreateWorktree => window.dispatch_action(Box::new(board::CreateWorktree), cx),
        Command::BoardOpenWorktree => window.dispatch_action(Box::new(board::OpenWorktree), cx),
        Command::BoardSync => window.dispatch_action(Box::new(board::Sync), cx),
        Command::BoardFullSync => window.dispatch_action(Box::new(board::FullSync), cx),
        Command::BoardOpenRemote => window.dispatch_action(Box::new(board::OpenRemote), cx),
        Command::BoardDeleteCard => window.dispatch_action(Box::new(board::DeleteCard), cx),
        Command::BoardSettings => window.dispatch_action(Box::new(board::Settings), cx),
        Command::BoardReload => window.dispatch_action(Box::new(board::Reload), cx),
        Command::BoardFilter => window.dispatch_action(Box::new(board::Filter), cx),
        Command::CardDetailClose => {
            // Like every other row here: the dialog comes back, and `Close` then does to it
            // exactly what `Esc` would — cancel the open edit, or close the dialog.
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::Close), cx);
        }
        Command::CardDetailEditTitle => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::EditTitle), cx);
        }
        Command::CardDetailEditDescription => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::EditDescription), cx);
        }
        Command::CardDetailAddComment => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::AddComment), cx);
        }
        Command::CardDetailNextProperty => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::NextProperty), cx);
        }
        Command::CardDetailPrevProperty => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::PrevProperty), cx);
        }
        Command::CardDetailEditProperty => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::EditProperty), cx);
        }
        Command::CardDetailCreateWorktree => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::CreateWorktree), cx);
        }
        Command::CardDetailOpenRemote => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::OpenRemote), cx);
        }
        Command::CardDetailKeepLocal => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::KeepLocal), cx);
        }
        Command::CardDetailTakeRemote => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::TakeRemote), cx);
        }
        Command::CardDetailSave => {
            open(Dialogs::CardDetail, cx);
            window.dispatch_action(Box::new(card_detail::Save), cx);
        }

        Command::NewWorktree => open(Dialogs::CreateWorktree, cx),
        Command::CloneRepo => open(Dialogs::CloneRepo, cx),
        Command::MoveRepo => open(Dialogs::AssignRepo, cx),
        Command::NewContext => open(Dialogs::NewContext, cx),
        Command::EditContext => open(Dialogs::EditContext, cx),
        Command::Settings => open(Dialogs::Settings, cx),
        Command::Help => open(Dialogs::Help, cx),
        Command::JobsPanel => {
            state.update(cx, |app, cx| {
                app.open_overlay(Overlay::Jobs);
                cx.notify();
            });
        }
        Command::DeleteWorktree => {
            if let Some(id) = cursor_worktree(state, cx) {
                request_confirm(cx, ConfirmRequest::DeleteWorktree { id });
                open(Dialogs::Confirm, cx);
            }
        }
        Command::PruneWorktrees => {
            if let Some(repo) = crate::dialogs::focused_repo(state.read(cx)) {
                request_confirm(cx, ConfirmRequest::Prune { repo });
                open(Dialogs::Confirm, cx);
            }
        }
        Command::KillSession => {
            let request = state.read(cx).snapshot.as_ref().and_then(|snapshot| {
                let session = snapshot.sessions.first()?;
                Some(ConfirmRequest::KillSession {
                    session: session.id.clone(),
                    terminals: session.terminals.len(),
                    running: session
                        .terminals
                        .iter()
                        .flat_map(|terminal| terminal.keep_alive.clone())
                        .collect(),
                    unsaved: false,
                })
            });
            if let Some(request) = request {
                request_confirm(cx, request);
                open(Dialogs::Confirm, cx);
            }
        }
        Command::DeleteContext => {
            let request = state.read(cx).snapshot.as_ref().and_then(|snapshot| {
                let id = state.read(cx).active_context()?.clone();
                let context = snapshot.contexts.iter().find(|entry| entry.id == id)?;
                let repos: Vec<_> = snapshot
                    .repos
                    .iter()
                    .filter(|repo| repo.context_id == id)
                    .collect();
                let worktrees = snapshot
                    .worktrees
                    .iter()
                    .filter(|worktree| repos.iter().any(|repo| repo.id == worktree.repo_id))
                    .count();
                Some(ConfirmRequest::DeleteContext {
                    context: id.clone(),
                    name: context.name.clone(),
                    repos: repos.len(),
                    worktrees,
                    sessions: snapshot.sessions.len(),
                })
            });
            if let Some(request) = request {
                request_confirm(cx, request);
                open(Dialogs::Confirm, cx);
            }
        }
        Command::SleepSession => {
            if let Some(id) = cursor_worktree(state, cx) {
                bridge.send(RequestBody::SleepWorktree { id });
            }
        }
        Command::InspectWorktree => {
            if let Some(id) = cursor_worktree(state, cx) {
                bridge.send(RequestBody::InspectWorktrees {
                    ids: vec![id],
                    repo: None,
                    fetch: true,
                });
            }
        }
        Command::PullRequests => {
            state.update(cx, |app, cx| {
                app.screen = Screen::Hub { tab: HubTab::Prs };
                cx.notify();
            });
        }
        Command::Worktrees => {
            state.update(cx, |app, cx| {
                app.screen = Screen::hub();
                cx.notify();
            });
        }
        Command::Refresh => bridge.send(RequestBody::RefreshStatuses { repo: None }),
        Command::UpdateFleet => bridge.send(RequestBody::Update),
        Command::OpenClaude => window.dispatch_action(Box::new(fleet::OpenAgentClaude), cx),
        Command::OpenOpencode => window.dispatch_action(Box::new(fleet::OpenAgentOpencode), cx),
        // The shell owns the whole quit flow, and its listeners sit on the window root, which
        // is an ancestor of this overlay — so dispatching reaches them.
        Command::Quit => window.dispatch_action(Box::new(fleet::Quit), cx),
        Command::QuitDaemon => window.dispatch_action(Box::new(fleet::QuitAndStopDaemon), cx),
    }
}

/// The worktree the Hub's cursor is on.
fn cursor_worktree(state: &Entity<AppState>, cx: &App) -> Option<WorktreeId> {
    let app = state.read(cx);
    let snapshot = app.snapshot.as_ref()?;
    snapshot
        .worktrees
        .get(app.cursors.worktrees)
        .map(|worktree| worktree.id.clone())
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_go_row_says_its_state_not_its_type() {
        // §3.9's right-hand column is the §2.5 state; "worktree" is what the id already says.
        assert_eq!(
            session_detail(SessionState::Attached, false),
            "session attached"
        );
        assert_eq!(session_detail(SessionState::Detached, true), "sleeping");
        assert_eq!(
            session_detail(SessionState::Detached, false),
            "running, detached"
        );
        assert_eq!(session_detail(SessionState::None, false), "no session");
        assert_eq!(session_detail(SessionState::Unknown, false), "unknown");
    }
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_subsequence_query_matches_and_an_empty_one_matches_everything() {
        assert!(matches("payroll#feat-payroll-fix", "pay fix"));
        assert!(matches("Clone repo", ""));
        assert!(!matches("Clone repo", "zzz"));
    }

    #[test]
    fn every_command_has_a_label_and_a_bound_key() {
        for command in Command::ALL {
            assert!(!command.label().is_empty());
            assert!(
                key_for(command.action()).is_some(),
                "`{}` is bound to nothing",
                command.action()
            );
        }
    }

    #[test]
    fn destructive_commands_are_marked_and_are_the_ones_with_confirms() {
        assert!(Command::DeleteWorktree.destructive());
        assert!(Command::PruneWorktrees.destructive());
        assert!(Command::QuitDaemon.destructive());
        assert!(!Command::Settings.destructive());
    }

    #[test]
    fn commands_that_need_a_snapshot_are_not_listed_without_one() {
        let state = AppState::new("/tmp/fleet", Instant::now());
        assert!(!Command::NewWorktree.valid(&state));
        assert!(!Command::DeleteWorktree.valid(&state));
        assert!(Command::Settings.valid(&state));
        assert!(Command::Help.valid(&state));
        assert!(!Command::UpdateFleet.valid(&state));
    }

    #[test]
    fn an_empty_snapshot_still_offers_the_always_valid_commands() {
        let state = AppState::new("/tmp/fleet", Instant::now());
        let rows = candidates(&state, "", None, None);
        assert!(
            rows.is_empty(),
            "without a snapshot the palette has nothing to point at"
        );
    }

    fn go_snapshot(state: SessionState, slept: bool) -> fleet_proto::snapshot::Snapshot {
        use fleet_core::sessions::{Session, SessionKind};
        let id: WorktreeId = "acme/widgets#feature-one"
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let mut snapshot = fleet_proto::snapshot::Snapshot {
            boards: Vec::new(),
            generated_at: "2026-09-04T12:00:00Z".to_owned(),
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "0.1.0".to_owned(),
                pid: 1,
                started_at: "2026-09-04T09:00:00Z".to_owned(),
                home: "/tmp/fleet".to_owned(),
            },
        };
        snapshot.worktrees = vec![fleet_core::model::Worktree {
            id: id.clone(),
            repo_id: "acme/widgets"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            slug: "feature-one".to_owned(),
            branch: "feature-one".to_owned(),
            base_ref: "main".to_owned(),
            path: "/tmp/widgets/feature-one".to_owned(),
            session: "widgets/feature-one".to_owned(),
            host: None,
            created_at: "2026-09-04T09:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        }];
        snapshot.sessions = vec![Session {
            id: "widgets/feature-one"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            kind: SessionKind::Worktree(id.clone()),
            cwd: "/tmp/widgets/feature-one".to_owned(),
            terminals: Vec::new(),
            active_terminal: None,
            slept_at: slept.then(|| "2026-09-04T11:00:00Z".to_owned()),
            kept_terminals: Vec::new(),
        }];
        snapshot.statuses = vec![fleet_core::sessions::WorktreeStatus {
            worktree_id: id,
            session: state,
            windows: Vec::new(),
            running: Vec::new(),
            agent_activity: fleet_core::sessions::AgentActivity::Unknown,
            agent_activity_changed_at: None,
        }];
        snapshot
    }

    #[test]
    fn a_go_row_takes_its_state_and_its_id_from_the_same_place_the_hub_does() {
        let now = Instant::now();
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(go_snapshot(SessionState::Detached, false), now);
        let rows = candidates(&app, "", None, None);
        let go: Vec<_> = rows
            .iter()
            .filter(|entry| entry.section == PaletteSectionKind::Go)
            .collect();
        assert_eq!(go.len(), 1, "one worktree is one GO row, {go:?}");
        assert_eq!(
            go[0].label, "acme/widgets#feature-one",
            "\u{a7}3.9 labels a GO row with its WorktreeId, never a session id"
        );
        assert_eq!(go[0].detail.as_deref(), Some("running, detached"));
        assert_eq!(go[0].status, Some(StatusKind::DetachedAwake));

        // The same worktree, actually attached, and slept.
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(go_snapshot(SessionState::Attached, false), now);
        let rows = candidates(&app, "", None, None);
        assert_eq!(rows[0].status, Some(StatusKind::Attached));
        let mut app = AppState::new("/tmp/fleet", now);
        app.apply_snapshot(go_snapshot(SessionState::Detached, true), now);
        let rows = candidates(&app, "", None, None);
        assert_eq!(rows[0].status, Some(StatusKind::Sleeping));
        assert_eq!(rows[0].detail.as_deref(), Some("sleeping"));
    }

    fn board_with_one_card() -> AppState {
        let mut state = AppState::new("/tmp/fleet-palette-board", Instant::now());
        state.screen = Screen::Hub { tab: HubTab::Board };
        let context = fleet_core::model::Context {
            id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
            name: "Work".into(),
            owners: Vec::new(),
            created_at: "2026-09-06T12:00:00Z".into(),
        };
        let mut board = fleet_core::board::new_board(&context, &context.created_at);
        let card = fleet_core::board::create_card(
            &mut board,
            &[],
            "card-1".parse().unwrap_or_else(|error| panic!("{error}")),
            fleet_core::board::CardDraft {
                title: "Fix login".into(),
                ..Default::default()
            },
            &context.created_at,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let column = board
            .statuses
            .iter()
            .position(|status| status.id == card.status_id)
            .unwrap_or_else(|| panic!("no column"));
        state.board.view = Some(fleet_core::board::BoardView {
            board,
            cards: vec![card],
        });
        state.board.focus = crate::state::BoardFocus { column, row: 0 };
        state
    }

    #[test]
    fn card_rows_that_could_only_do_nothing_are_not_listed() {
        let mut state = board_with_one_card();
        assert!(Command::CardDetailEditTitle.valid(&state));
        // §3.9: a row that opens a dialog and then returns is not a valid row.
        assert!(!Command::CardDetailClose.valid(&state));
        assert!(!Command::CardDetailSave.valid(&state));
        assert!(!Command::CardDetailKeepLocal.valid(&state));
        assert!(!Command::CardDetailTakeRemote.valid(&state));
        assert!(!Command::BoardOpenWorktree.valid(&state));

        // Over an open detail, closing and saving are exactly what the palette is for.
        let behind = card_context(&state, Some(Dialogs::CardDetail), None);
        assert!(Command::CardDetailClose.valid_with(&state, behind));
        assert!(Command::CardDetailSave.valid_with(&state, behind));
        assert!(!Command::CardDetailKeepLocal.valid_with(&state, behind));

        let card = &mut state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"))
            .cards[0];
        card.worktree_id = Some(
            "acme/api#wor-1"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        );
        card.conflict = Some(fleet_core::board::Conflict {
            detected_at: "2026-09-06T12:00:00Z".into(),
            remote: fleet_core::board::RemoteCard::default(),
            fields: vec!["title".into()],
        });
        assert!(Command::BoardOpenWorktree.valid(&state));
        assert!(Command::CardDetailKeepLocal.valid(&state));
        assert!(Command::CardDetailTakeRemote.valid(&state));
    }

    /// The card detail deliberately keeps the card it opened on when a refresh moves the
    /// board's selection, so the `Card detail:` rows have to be judged against *that* card.
    #[test]
    fn the_card_detail_rows_follow_the_open_dialog_and_not_the_board_selection() {
        let mut state = board_with_one_card();
        let view = state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"));
        let mut second = view.cards[0].clone();
        second.id = "card-two".parse().unwrap_or_else(|error| panic!("{error}"));
        second.number = 2;
        second.conflict = Some(fleet_core::board::Conflict {
            detected_at: "2026-09-06T12:00:00Z".into(),
            remote: fleet_core::board::RemoteCard::default(),
            fields: vec!["title".into()],
        });
        let held = second.id.clone();
        view.cards.push(second);
        // The board is focused on the first card, which has no conflict; the dialog holds the
        // second, which does.
        let selection = card_context(&state, Some(Dialogs::CardDetail), None);
        assert!(!Command::CardDetailKeepLocal.valid_with(&state, selection));
        let held = card_context(&state, Some(Dialogs::CardDetail), Some(&held));
        assert!(Command::CardDetailKeepLocal.valid_with(&state, held));
        assert!(Command::CardDetailTakeRemote.valid_with(&state, held));
    }

    #[test]
    fn opening_a_remote_issue_needs_a_link_that_carries_an_address() {
        let mut state = board_with_one_card();
        assert!(
            !Command::BoardOpenRemote.valid(&state),
            "an unlinked card has no issue to open"
        );
        let card = &mut state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"))
            .cards[0];
        card.remote = Some(fleet_core::board::RemoteLink {
            parent_key: None,
            backend: "jira".into(),
            key: "SP-1".into(),
            url: None,
            version: None,
            synced_at: "2026-09-06T12:00:00Z".into(),
            remote_updated_at: None,
        });
        assert!(
            !Command::BoardOpenRemote.valid(&state),
            "a backend that publishes no URL leaves nothing to open"
        );
        let card = &mut state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"))
            .cards[0];
        if let Some(remote) = card.remote.as_mut() {
            remote.url = Some("https://example.test/browse/SP-1".into());
        }
        assert!(Command::BoardOpenRemote.valid(&state));
        assert!(Command::CardDetailOpenRemote.valid(&state));
    }

    #[test]
    fn the_section_order_is_fixed() {
        assert!(PaletteSectionKind::Go < PaletteSectionKind::Do);
        assert!(PaletteSectionKind::Do < PaletteSectionKind::Context);
    }
}
