//! The active context's board mirror and the local presentation state around it (BOARD §8).

use fleet_core::{
    board::{BackendDescriptor, BoardView, Card},
    ids::ContextId,
};

use super::{
    AppState, FilterEscape, HubTab, NO_ACTIVE_CONTEXT, Overlay, Screen, clamp_cursor, filter_escape,
};
use crate::dialogs::Dialogs;

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

/// The active context's board data and local presentation state (BOARD §8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardState {
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
}

impl BoardState {
    /// Whether cards are currently being filtered.
    #[must_use]
    pub fn is_filtered(&self) -> bool {
        !self.filter.trim().is_empty()
    }
}

impl AppState {
    /// The loaded board for the active context.
    #[must_use]
    pub fn board(&self) -> Option<&BoardView> {
        self.board.view.as_ref()
    }

    /// Applies an authoritative board view; responses for another context are ignored.
    pub fn apply_board_view(&mut self, view: BoardView) {
        if self.active_context() != Some(&view.board.context_id) {
            return;
        }
        self.board_stale = false;
        self.board.view = Some(view);
        self.board.loading = false;
        self.board.error = None;
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
        self.clamp_board_focus();
    }

    /// Clears board data and invalidates requests from the previous context or connection.
    pub fn clear_board(&mut self) {
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
        self.board = BoardState::default();
        self.board_stale = true;
        self.board_generation = self.board_generation.wrapping_add(1);
        // A reconnect can land on a different fleetd with a different registry, and the
        // descriptors are what the header and the settings dialog are drawn from. The list
        // itself is kept until a newer one arrives so the header's label does not flicker.
        self.board_backends_asked = false;
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
    pub fn apply_backends(&mut self, backends: Vec<BackendDescriptor>) {
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
    pub fn backend_descriptor(&self, kind: &str) -> Option<&BackendDescriptor> {
        self.board_backends
            .iter()
            .find(|descriptor| descriptor.kind == kind)
    }

    /// The human name of a backend kind, falling back to the raw key.
    ///
    /// The fallback matters on the first frames of a connection and against an older daemon:
    /// a header that draws nothing at all where the backend goes reads as a broken board.
    #[must_use]
    pub fn backend_label(&self, kind: &str) -> String {
        self.backend_descriptor(kind)
            .map_or_else(|| kind.to_owned(), |descriptor| descriptor.label.clone())
    }

    /// The standard card fields this board's backend cannot write back.
    ///
    /// Empty on a local board, exactly as `fleet_core::board::ops` reads it: a local board
    /// declares no backend, so nothing it holds is read-only.
    #[must_use]
    pub fn readonly_fields(&self) -> &[String] {
        self.board()
            .filter(|view| !view.board.backend.is_local())
            .map_or(&[][..], |view| &view.board.sync.readonly_fields)
    }

    /// Whether the daemon would refuse a local edit to `field` on the shown board.
    #[must_use]
    pub fn is_readonly_field(&self, field: &str) -> bool {
        self.readonly_fields()
            .iter()
            .any(|readonly| readonly == field)
    }

    /// Claims one load; an error waits for reload instead of retrying every render.
    pub(crate) fn begin_board_load(&mut self) -> Option<(ContextId, u64)> {
        if !matches!(self.screen, Screen::Hub { tab: HubTab::Board }) {
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
        let Some(context) = self.active_context().cloned() else {
            // Every board is `EnsureBoard(active_context)`, so with no active context there is
            // nothing to ask for and skeleton columns would promise a load that never goes
            // out. `apply_snapshot` clears the board the moment one is activated, which drops
            // this message and makes the load stale again.
            if self.board.view.is_none() {
                self.board.error = Some(NO_ACTIVE_CONTEXT.to_owned());
            }
            return None;
        };
        if self.board.loading || !self.board_stale {
            return None;
        }
        self.board.loading = true;
        self.board.error = None;
        self.board_stale = false;
        Some((context, self.board_generation))
    }

    /// Completes a load only if its context and generation still own the board slot.
    pub(crate) fn finish_board_load(
        &mut self,
        context: &ContextId,
        generation: u64,
        result: Result<BoardView, String>,
    ) {
        if generation != self.board_generation || self.active_context() != Some(context) {
            return;
        }
        self.board.loading = false;
        match result {
            Ok(view) if &view.board.context_id == context => {
                let stale = self.board_stale;
                self.apply_board_view(view);
                self.board_stale = stale;
            }
            Ok(_) => self.board.error = Some("EnsureBoard returned a different context".into()),
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
    #[must_use]
    pub fn board_filter_owns_keys(&self) -> bool {
        self.overlay.is_none()
            && self.agent_popup.is_none()
            && matches!(self.screen, Screen::Hub { tab: HubTab::Board })
            && self.board.filter_editing
    }

    /// `Esc` on the board: leave the filter input, then clear the filter (§3.10, [D-15]).
    ///
    /// Returns whether it consumed the key; when it did not, `Esc` belongs to whoever owns the
    /// surface behind the board.
    pub fn board_filter_escape(&mut self) -> bool {
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
