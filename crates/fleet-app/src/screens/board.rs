//! The active-context board screen (BOARD §8).
//!
//! *One board per context, one column per status, one key per edit.* The screen owns nothing
//! authoritative: it draws [`crate::state::BoardState`], and every key that changes a card
//! sends a request and waits for the daemon's answer — there is no optimistic write anywhere
//! here, because a board that shows a move the daemon refused is a board you cannot trust.
//!
//! The keyboard model is the Hub's: `h` / `l` walk the columns, `j` / `k` walk the cards,
//! `Enter` opens the detail dialog, and every property has one letter that opens its picker.
//! `/` is the exception: while the filter input owns the keyboard the screen publishes the
//! `Filter` key context instead of `Hub > Board`, so the bare letters type instead of firing
//! (`crate::state::AppState::context_chain`).

use std::time::Instant;

use crate::{
    actions::filter as filter_actions,
    bridge::Bridge,
    dialogs::{self, ConfirmRequest, Dialogs, card_picker::PickerKind, typed_char},
    state::{AppState, HubPane, HubTab, Overlay, Screen, StickyError},
    views::board_screen::{self, BoardClick, BoardProps},
};
use fleet_core::{
    board::Card,
    ids::{BoardId, CardId, RepoId, StatusId, WorktreeId},
};
use fleet_proto::{
    error::ErrorKind,
    job::{JobKind, JobStatus},
    request::RequestBody,
    response::ResponseBody,
};
use fleet_ui_kit::Icon;
use gpui::{AnyElement, App, Entity, FocusHandle, ScrollHandle, Subscription, Window, prelude::*};

mod actions;
mod lifecycle;
mod navigation;
#[cfg(test)]
mod tests;

pub(crate) use actions::{
    NO_REPO_IN_CONTEXT, create_worktree, delete_card, filter, go_board, has_repo_in_context,
    move_next_column, move_prev_column, new_card, no_remote_reason, open_card, open_dialog,
    open_remote, open_worktree, pick_assignee, pick_estimate, pick_labels, pick_priority,
    pick_status, readonly_message, refuses, reload, remote_url, settings, sync,
};
pub(crate) use lifecycle::{
    Refusal, open_session, request_worktree_reporting, send_card_reporting,
};
use lifecycle::{ensure_current, fail, request_worktree, send_card, syncing};
use navigation::{board_id, edit_filter, leave_filter_input, on_click, step_focus};
pub(crate) use navigation::{
    focus_card, jump_rows, move_rows, next_card, next_column, prev_card, prev_column, selected_card,
};

/// Hub tab for the active context's board.
pub(crate) struct BoardScreen {
    /// Horizontal scroller of the columns; `h` / `l` reveal the focused one.
    board_scroll: ScrollHandle,
    /// One vertical scroller per column; `j` / `k` reveal the focused card.
    column_scrolls: Vec<ScrollHandle>,
    /// Which board the column scrollers belong to; they are positional, not portable.
    scrolls_board: Option<BoardId>,
    revealed_focus: Option<(BoardId, usize, usize, Option<CardId>)>,
    /// Keeps the load observation alive; dropping it stops the board refreshing itself.
    observation: Option<Subscription>,
}

impl BoardScreen {
    /// Builds the screen using the frozen screen constructor.
    #[must_use]
    pub(crate) fn new(_cx: &mut App) -> Self {
        Self {
            board_scroll: ScrollHandle::new(),
            column_scrolls: Vec::new(),
            scrolls_board: None,
            revealed_focus: None,
            observation: None,
        }
    }

    /// Installs the load observation once; every later call is a no-op.
    ///
    /// *Render prepares nothing.* An `EnsureBoard` sent from a paint puts a daemon round trip
    /// inside the frame, so the load hangs off [`AppState`] instead: every notify that leaves
    /// the board tab showing with a stale slot claims one load, which is exactly the set of
    /// moments the repaint used to catch.
    pub(crate) fn bind(&mut self, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
        if self.observation.is_some() {
            return;
        }
        let observed = bridge.clone();
        self.observation = Some(cx.observe(state, move |state, cx| {
            lifecycle::synchronize(&state, &observed, cx);
        }));
        let (deferred_state, deferred_bridge) = (state.clone(), bridge.clone());
        cx.defer(move |cx| lifecycle::synchronize(&deferred_state, &deferred_bridge, cx));
    }

    /// Renders the board into the Hub's body.
    pub(crate) fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.bind(state, bridge, cx);
        let columns = state
            .read(cx)
            .board()
            .map_or(0, |view| view.board.statuses.len());
        let shown = state.read(cx).board().map(|view| view.board.id.clone());
        if self.scrolls_board != shown {
            // Handles are reused by position: another board's board would inherit its offsets.
            self.column_scrolls.clear();
            self.scrolls_board = shown;
        }
        self.column_scrolls.resize_with(columns, ScrollHandle::new);

        let click_state = state.clone();
        let click_bridge = bridge.clone();
        let now = crate::presentation::now_unix();
        let app = state.read(cx);
        // The chip states which system the board mirrors, so it reads the registry's label and
        // not `jira` — the key a config file uses. Resolved before the borrow of `app.board`
        // below, because the lookup needs the whole state.
        let backend_label = app
            .board()
            .map(|view| app.backend_label(&view.board.backend.kind));
        let props = BoardProps {
            view: app.board.view.as_ref(),
            loading: app.board.loading,
            error: app.board.error.as_deref(),
            filter: &app.board.filter,
            filter_editing: app.board.filter_editing,
            focus: (app.board.focus.column, app.board.focus.row),
            syncing: syncing(app),
            backend_label: backend_label.as_deref(),
            now,
        };
        // The focused card, not only its coordinates: a refresh that inserts a card above it
        // moves the same selection to a place the scroller has not revealed yet.
        let selection = props.view.map(|view| {
            (
                view.board.id.clone(),
                props.focus.0,
                props.focus.1,
                selected_card(app).map(|card| card.id.clone()),
            )
        });
        if self.revealed_focus != selection {
            self.reveal_focus(props.focus);
            self.revealed_focus = selection;
        }
        let body = board_screen::render(
            &props,
            &self.board_scroll,
            &self.column_scrolls,
            move |click, cx| on_click(click, &click_state, &click_bridge, cx),
            cx,
        );

        crate::dialogs::root(focus)
            .on_key_down({
                let state = state.clone();
                move |event, _window, cx| {
                    let Some(text) = typed_char(event) else {
                        cx.propagate();
                        return;
                    };
                    let typed = state.update(cx, |app, cx| {
                        if !app.board.filter_editing {
                            return false;
                        }
                        app.board.filter.push_str(text);
                        app.board.focus.row = 0;
                        app.clamp_board_focus();
                        cx.notify();
                        true
                    });
                    if typed {
                        cx.stop_propagation();
                    } else {
                        // The board itself owns bare letters as commands; only the filter input
                        // may swallow them as text.
                        cx.propagate();
                    }
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &filter_actions::Backspace, _window, cx| {
                    edit_filter(&state, cx, |query| {
                        query.pop();
                    });
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &filter_actions::DeleteWord, _window, cx| {
                    edit_filter(&state, cx, |query| {
                        *query = crate::dialogs::filter::delete_word(query);
                    });
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &filter_actions::Clear, _window, cx| {
                    edit_filter(&state, cx, String::clear);
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &filter_actions::CursorDown, _window, cx| step_focus(&state, 0, 1, cx)
            })
            .on_action({
                let state = state.clone();
                move |_: &filter_actions::CursorUp, _window, cx| step_focus(&state, 0, -1, cx)
            })
            .on_action({
                let state = state.clone();
                let bridge = bridge.clone();
                move |_: &filter_actions::Accept, _window, cx| {
                    leave_filter_input(&state, cx);
                    open_card(&state, &bridge, cx);
                }
            })
            .on_action({
                let state = state.clone();
                move |_: &filter_actions::Escape, _window, cx| {
                    if state.update(cx, |app, cx| {
                        let handled = app.board_filter_escape();
                        if handled {
                            cx.notify();
                        }
                        handled
                    }) {
                        cx.stop_propagation();
                    } else {
                        // Nothing to leave or clear: `Esc` belongs to whoever is behind the
                        // board, and it still never quits (§A13).
                        cx.propagate();
                    }
                }
            })
            .child(body)
            .into_any_element()
    }

    /// Keeps the focused column and card inside their scrollers.
    fn reveal_focus(&self, (column, row): (usize, usize)) {
        self.board_scroll.scroll_to_item(column);
        if let Some(scroll) = self.column_scrolls.get(column) {
            scroll.scroll_to_item(row);
        }
    }
}
