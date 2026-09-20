use super::*;
use fleet_core::{
    board::{BoardView, Card},
    config::NATIVE_BOARD,
};
use fleet_proto::response::BOARD_WORKTREE_CAPABILITY;

/// What the app says when the connected daemon serves no worktree boards.
///
/// Word for word the CLI's refusal (`fleet-client`'s dispatch check), because the remedy is the
/// same one: a user who has read the sentence once should not have to recognise a second
/// wording for the same daemon being too old.
pub const WORKTREE_BOARDS_UNSUPPORTED: &str =
    "this daemon does not support worktree boards; run `fleet daemon restart`";

/// Which board the single mirror is pointed at (BOARD §8).
///
/// The Hub and the Workspace are never visible at the same time, so one [`BoardState`] with a
/// scope serves both surfaces. Neither half is ever turned into a board id here: a context's
/// board and a worktree's board are each asked for by their own request, and the daemon's
/// answer is what says which board that is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardScope {
    /// The active context's own board — `EnsureBoard(context)`.
    Context(ContextId),
    /// One worktree's board — `EnsureWorktreeBoard(worktree)`.
    Worktree(WorktreeId),
}

/// Keyboard selection within the board's status columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoardFocus {
    /// Zero-based status column.
    pub column: usize,
    /// Zero-based card within the column.
    pub row: usize,
}

/// Optional secondary grouping beneath a status column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    /// Group by priority.
    Priority,
    /// Group by assignee.
    Assignee,
    /// Group by label.
    Labels,
}

/// The shown board's data and local presentation state (BOARD §8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardState {
    /// Which board this mirror is pointed at, once a surface has claimed it.
    ///
    /// `None` is the Hub's default — the active context's own board — which is what
    /// [`AppState::board_scope`] resolves it to and what the first load records here.
    pub scope: Option<BoardScope>,
    /// Last authoritative board response.
    pub view: Option<BoardView>,
    /// Whether an EnsureBoard request is in flight.
    pub loading: bool,
    /// Most recent load failure, retained until an explicit retry.
    pub error: Option<String>,
    /// Keyboard selection.
    pub focus: BoardFocus,
    /// Card substring filter.
    pub filter: String,
    /// Whether the filter input still owns the keyboard (§3.10's two-stage `Esc`).
    ///
    /// The board is the one list whose filter is not the Hub's [`FilterState`]: its rows are
    /// cards in columns, not worktrees, so `Overlay::Filter` — which moves the worktree cursor
    /// and opens a worktree on `Enter` — cannot serve it. The screen publishes the `Filter` key
    /// context while this is set, which is what makes the bare letters type instead of fire.
    pub filter_editing: bool,
    /// Optional secondary grouping.
    pub group_secondary: Option<GroupBy>,
    /// Bumped by every mutation of [`Self::view`].
    ///
    /// The board screen memoises its whole derived model behind this counter, so it has to move
    /// on every applied card and every applied view — `AppState::snapshot_revision` does not,
    /// because a card edit lands through [`AppState::apply_card`] and never through a snapshot.
    pub revision: u64,
}

impl BoardState {
    /// Whether cards are currently being filtered.
    #[must_use]
    fn is_filtered(&self) -> bool {
        !self.filter.trim().is_empty()
    }

    /// Records that [`Self::view`] now holds something else than it did.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl AppState {
    /// The board the mirror currently holds, whatever scope it is pointed at.
    #[must_use]
    pub fn board(&self) -> Option<&BoardView> {
        self.board.view.as_ref()
    }

    /// Which board the mirror is pointed at: the scope a surface set, or the Hub's default.
    #[must_use]
    pub(crate) fn board_scope(&self) -> Option<BoardScope> {
        self.board
            .scope
            .clone()
            .or_else(|| self.active_context().cloned().map(BoardScope::Context))
    }

    /// Whether `view` is the board the mirror is pointed at.
    ///
    /// A worktree's board is recognised by its worktree; a context's board is the one with that
    /// context and **no** worktree, which is the daemon's own rule (BOARD §0) and the reason a
    /// worktree board can never land in the Hub's slot although both name the same context.
    #[must_use]
    fn board_scope_admits(&self, view: &BoardView) -> bool {
        match self.board_scope() {
            Some(BoardScope::Context(context)) => {
                view.board.context_id == context && view.board.worktree_id.is_none()
            }
            Some(BoardScope::Worktree(worktree)) => {
                view.board.worktree_id.as_ref() == Some(&worktree)
            }
            None => false,
        }
    }

    /// Applies an authoritative board view; responses for another scope are ignored.
    pub fn apply_board_view(&mut self, view: BoardView) {
        if !self.board_scope_admits(&view) {
            return;
        }
        self.board_stale = false;
        self.board.view = Some(view);
        self.board.loading = false;
        self.board.error = None;
        self.board.touch();
        self.clamp_board_focus();
    }

    /// Upserts a card only into its currently loaded board.
    pub fn apply_card(&mut self, card: Card) {
        let Some(view) = self.board.view.as_mut() else {
            return;
        };
        if view.board.id != card.board_id {
            return;
        }
        if !view
            .board
            .statuses
            .iter()
            .any(|status| status.id == card.status_id)
        {
            self.board_stale = true;
            return;
        }
        if let Some(current) = view.cards.iter_mut().find(|current| current.id == card.id) {
            *current = card;
        } else {
            view.cards.push(card);
        }
        view.cards.sort_by(|a, b| {
            (&a.status_id, a.position, &a.created_at, a.number).cmp(&(
                &b.status_id,
                b.position,
                &b.created_at,
                b.number,
            ))
        });
        self.board.touch();
        self.clamp_board_focus();
    }

    /// Clears board data and invalidates requests from the previous context or connection.
    pub(super) fn clear_board(&mut self) {
        self.invalidate_board();
        // A reconnect can land on a different fleetd with a different registry, and the
        // descriptors are what the header and the settings dialog are drawn from. The list
        // itself is kept until a newer one arrives so the header's label does not flicker.
        self.board_backends_asked = false;
    }

    /// Drops the mirror and strands every response the slot it held still owed.
    ///
    /// The generation counter is the one invalidation mechanism the board has: bumping it is
    /// what makes a reply already in flight — from the previous context, the previous
    /// connection or the previous scope — land on a slot that is no longer its own.
    fn invalidate_board(&mut self) {
        if matches!(
            self.overlay,
            Some(Overlay::Dialog(
                Dialogs::CardDetail
                    | Dialogs::CardCreate
                    | Dialogs::CardPicker
                    | Dialogs::BoardSettings
            ))
        ) {
            self.close_overlay();
        }
        let revision = self.board.revision.wrapping_add(1);
        self.board = BoardState::default();
        self.board.revision = revision;
        self.board_stale = true;
        self.board_generation = self.board_generation.wrapping_add(1);
    }

    /// Points the mirror at another board; returns whether that changed which board is shown.
    ///
    /// Comparing against the *resolved* scope is what keeps the Hub still: pointing a mirror
    /// that has never been pointed anywhere at the active context is where it already is, so
    /// it keeps the view it loaded instead of paying for a round trip to redraw the same board.
    fn point_board_at(&mut self, scope: Option<BoardScope>) -> bool {
        let moved = self.board_scope() != scope;
        if moved {
            self.invalidate_board();
        }
        self.board.scope = scope;
        moved
    }

    /// Points the board at the active context's board, which is the one the Hub tab shows.
    ///
    /// Returns whether the mirror moved, so an observation that runs on every notify only
    /// notifies when something actually changed.
    pub(crate) fn enter_context_board_scope(&mut self) -> bool {
        let scope = self.active_context().cloned().map(BoardScope::Context);
        self.point_board_at(scope)
    }

    /// Points the board at one worktree's board, or refuses when this daemon has none.
    ///
    /// The refusal is a toast and not the sticky slot (§2.7): it is a fact about the daemon on
    /// the other end, not a failure of the keystroke, and the sentence carries its own remedy.
    /// `fleet-client` rechecks the capability at dispatch, so a connection that changes under
    /// this answer fails the request with the same sentence rather than hanging.
    ///
    /// A refusal reached with the board pane already on screen — a `fleet://board` tab the user
    /// put in `windows[]`, a daemon downgraded under a live one — also drops the mirror and
    /// writes the same sentence to [`BoardState::error`]. The pane draws whatever the mirror
    /// holds, and what the mirror holds is never this tab's board: without that it would draw
    /// skeleton columns for a load that can never go out, or the Hub's context board under a
    /// worktree's heading. `ctrl-s b` from a tab that is not the board's only toasts.
    pub(crate) fn enter_worktree_board_scope(
        &mut self,
        worktree: WorktreeId,
        now: Instant,
    ) -> bool {
        if !self.daemon_capabilities.contains(BOARD_WORKTREE_CAPABILITY) {
            self.toast(
                Toast::new(WORKTREE_BOARDS_UNSUPPORTED).icon(Icon::Info),
                now,
                dwell_for(ToastDuration::Normal),
            );
            if self.board_pane_is_active() {
                self.invalidate_board();
                self.board.error = Some(WORKTREE_BOARDS_UNSUPPORTED.to_owned());
            }
            return false;
        }
        self.point_board_at(Some(BoardScope::Worktree(worktree)));
        true
    }

    /// Whether a `ListBoardBackends` request should go out now, marking it as issued.
    pub(crate) fn begin_backends_load(&mut self) -> bool {
        if self.board_backends_asked || self.refuses_mutations() {
            return false;
        }
        self.board_backends_asked = true;
        true
    }

    /// Adopts the daemon's backend registry.
    pub(crate) fn apply_backends(&mut self, backends: Vec<BackendDescriptor>) {
        self.board_backends = backends;
    }

    /// Releases the once-per-connection guard so a failed ask can be made again.
    ///
    /// The flag is set before the request, so without this one refused `ListBoardBackends`
    /// leaves the header showing the raw kind and the settings dialog with no backend rows for
    /// the rest of the connection — a permanent consequence of a momentary failure.
    pub(crate) fn backends_load_failed(&mut self) {
        self.board_backends_asked = false;
    }

    /// The descriptor of one backend kind, when the daemon registers it.
    #[must_use]
    pub(crate) fn backend_descriptor(&self, kind: &str) -> Option<&BackendDescriptor> {
        self.board_backends
            .iter()
            .find(|descriptor| descriptor.kind == kind)
    }

    /// The human name of a backend kind, falling back to the raw key.
    ///
    /// The fallback matters on the first frames of a connection and against an older daemon:
    /// a header that draws nothing at all where the backend goes reads as a broken board.
    #[must_use]
    pub(crate) fn backend_label(&self, kind: &str) -> String {
        self.backend_descriptor(kind)
            .map_or_else(|| kind.to_owned(), |descriptor| descriptor.label.clone())
    }

    /// The standard card fields this board's backend cannot write back.
    ///
    /// Empty on a local board, exactly as `fleet_core::board::ops` reads it: a local board
    /// declares no backend, so nothing it holds is read-only.
    #[must_use]
    fn readonly_fields(&self) -> &[String] {
        self.board()
            .filter(|view| !view.board.backend.is_local())
            .map_or(&[][..], |view| &view.board.sync.readonly_fields)
    }

    /// Whether the daemon would refuse a local edit to `field` on the shown board.
    #[must_use]
    pub(crate) fn is_readonly_field(&self, field: &str) -> bool {
        self.readonly_fields()
            .iter()
            .any(|readonly| readonly == field)
    }

    /// Whether a surface that draws the board is showing.
    ///
    /// The Hub's tab is one. A worktree scope is the other: only the Workspace's board pane
    /// sets one, and it points the mirror back at the context when it goes away, so the scope
    /// itself says whether that pane is there to draw the answer.
    #[must_use]
    fn board_is_shown(&self) -> bool {
        matches!(self.screen, Screen::Hub { tab: HubTab::Board })
            || matches!(self.board.scope, Some(BoardScope::Worktree(_)))
    }

    /// Whether the Workspace's `fleet://board` tab is the surface drawing the board.
    ///
    /// The reserved command decides it, not the tab's name, exactly as the Workspace's own
    /// model does: the daemon owns the tab list, and an agent tab shadows the terminal strip
    /// entirely, so neither of them may answer for a board pane that is not on screen.
    #[must_use]
    pub(crate) fn board_pane_is_active(&self) -> bool {
        matches!(self.screen, Screen::Workspace { .. })
            && self.active_agent_thread().is_none()
            && self
                .active_terminal_record()
                .is_some_and(|terminal| terminal.is_native() && terminal.command == NATIVE_BOARD)
    }

    /// The invalidation counter every board request in flight is stamped with.
    ///
    /// The Workspace's board pane records it beside the worktree it claimed the scope for, so
    /// a [`AppState::clear_board`] — a reconnect, a context switch — makes the claim stale and
    /// the pane points the mirror again, while a refusal the daemon has already explained is
    /// asked for exactly once.
    #[must_use]
    pub(crate) const fn board_generation(&self) -> u64 {
        self.board_generation
    }

    /// Claims one load; an error waits for reload instead of retrying every render.
    pub(crate) fn begin_board_load(&mut self) -> Option<(BoardScope, u64)> {
        if !self.board_is_shown() {
            return None;
        }
        if self.refuses_mutations() {
            // "Cold" means a load is in flight. With the daemon gone none ever will be, so the
            // board says why instead of drawing skeleton columns forever.
            if self.board.view.is_none() {
                self.board.error = Some("fleetd is not reachable".to_owned());
            }
            return None;
        }
        let Some(scope) = self.board_scope() else {
            // With no scope set the board is the active context's, so with no active context
            // there is nothing to ask for and skeleton columns would promise a load that never
            // goes out. `apply_snapshot` clears the board the moment one is activated, which
            // drops this message and makes the load stale again.
            if self.board.view.is_none() {
                self.board.error = Some(NO_ACTIVE_CONTEXT.to_owned());
            }
            return None;
        };
        if self.board.loading || !self.board_stale {
            return None;
        }
        // The Hub's default becomes explicit here: from the first load on, the mirror says
        // which board it holds rather than leaving it to be re-derived on every read.
        self.board.scope = Some(scope.clone());
        self.board.loading = true;
        self.board.error = None;
        self.board_stale = false;
        Some((scope, self.board_generation))
    }

    /// Completes a load only if its scope and generation still own the board slot.
    pub(crate) fn finish_board_load(
        &mut self,
        scope: &BoardScope,
        generation: u64,
        result: Result<BoardView, String>,
    ) {
        if generation != self.board_generation || self.board_scope().as_ref() != Some(scope) {
            return;
        }
        self.board.loading = false;
        match result {
            Ok(view) if self.board_scope_admits(&view) => {
                let stale = self.board_stale;
                self.apply_board_view(view);
                self.board_stale = stale;
            }
            Ok(_) => self.board.error = Some("the daemon answered with another board".into()),
            Err(error) => self.board.error = Some(error),
        }
    }

    /// Keeps the board selection inside the columns and rows that are actually drawn.
    ///
    /// The filter is part of that: a selection that indexes a hidden card is a selection the
    /// user cannot see, and every key that acts on "the focused card" would act on the wrong
    /// one.
    pub(crate) fn clamp_board_focus(&mut self) {
        let Some(view) = self.board.view.as_ref() else {
            return;
        };
        self.board.focus.column = clamp_cursor(self.board.focus.column, view.board.statuses.len());
        let len = view
            .board
            .statuses
            .get(self.board.focus.column)
            .map_or(0, |status| {
                crate::views::board_screen::visible_cards(view, &status.id, &self.board.filter)
                    .len()
            });
        self.board.focus.row = clamp_cursor(self.board.focus.row, len);
    }

    /// Whether the board's filter input, rather than the board itself, owns the keyboard.
    ///
    /// Both surfaces that draw the board answer here: the Hub's tab and the Workspace's board
    /// pane share one editor, one `BoardState.filter` and one two-stage `Esc`, so the pane
    /// publishes `Filter > BoardFilter` over `Workspace > Native > Board` for the same reason
    /// the Hub publishes it over `Hub > Board` — the bare letters have to type.
    #[must_use]
    pub(crate) fn board_filter_owns_keys(&self) -> bool {
        self.overlay.is_none()
            && self.agent_popup.is_none()
            && (matches!(self.screen, Screen::Hub { tab: HubTab::Board })
                || self.board_pane_is_active())
            && self.board.filter_editing
    }

    /// `Esc` on the board: leave the filter input, then clear the filter (§3.10, [D-15]).
    ///
    /// Returns whether it consumed the key; when it did not, `Esc` belongs to whoever owns the
    /// surface behind the board.
    pub(crate) fn board_filter_escape(&mut self) -> bool {
        match filter_escape(self.board.filter_editing) {
            FilterEscape::LeaveInput => {
                self.board.filter_editing = false;
                true
            }
            FilterEscape::ClearFilter if self.board.is_filtered() => {
                self.board.filter.clear();
                self.clamp_board_focus();
                true
            }
            FilterEscape::ClearFilter => false,
        }
    }
}

#[cfg(test)]
mod tests;
