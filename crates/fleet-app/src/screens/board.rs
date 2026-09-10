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

use std::{cell::RefCell, rc::Rc, time::Instant};

use crate::{
    actions::filter as filter_actions,
    bridge::Bridge,
    dialogs::{self, ConfirmRequest, Dialogs, card_picker::PickerKind, typed_char},
    state::{AppState, HubPane, HubTab, Overlay, Screen, StickyError},
    views::board_screen::{self, BoardClick, BoardModel, BoardProps},
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
use fleet_ui_kit::{Icon, KanbanColumn};
use gpui::{
    AnyElement, App, Entity, FocusHandle, ListState, ScrollHandle, Subscription, Window, prelude::*,
};

mod actions;
mod lifecycle;
mod navigation;
mod projection;
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
    /// One virtualized list per column; `j` / `k` reveal the focused card through it.
    column_lists: Vec<ListState>,
    /// The model the lists were last spliced against, so a frame that changed nothing splices
    /// nothing.
    listed: Option<Rc<BoardModel>>,
    /// Which board the column lists belong to; they are positional, not portable.
    scrolls_board: Option<BoardId>,
    revealed_focus: Option<(BoardId, usize, usize, Option<CardId>)>,
    /// The derived model, kept behind its revision key so a frame that changed nothing pays
    /// nothing (`gpui-performance` rule 3).
    projection: RefCell<projection::ProjectionCache>,
    /// Keeps the load observation alive; dropping it stops the board refreshing itself.
    observation: Option<Subscription>,
}

impl BoardScreen {
    /// Builds the screen using the frozen screen constructor.
    #[must_use]
    pub(crate) fn new(_cx: &mut App) -> Self {
        Self {
            board_scroll: ScrollHandle::new(),
            column_lists: Vec::new(),
            listed: None,
            scrolls_board: None,
            revealed_focus: None,
            projection: RefCell::default(),
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
        let now = crate::presentation::now_unix();
        let model = projection::prepare(state.read(cx), &self.projection, now);
        let shown = state.read(cx).board().map(|view| view.board.id.clone());
        if self.scrolls_board != shown {
            // Lists are reused by position: another board's columns would inherit their offsets
            // and, worse, their measured tile heights.
            self.column_lists.clear();
            self.listed = None;
            self.scrolls_board = shown;
        }
        self.sync_lists(&model);

        let click_state = state.clone();
        let click_bridge = bridge.clone();
        let app = state.read(cx);
        let props = BoardProps {
            model: app.board().is_some().then_some(model.as_ref()),
            loading: app.board.loading,
            error: app.board.error.as_deref(),
            filter: &app.board.filter,
            filter_editing: app.board.filter_editing,
            focus: (app.board.focus.column, app.board.focus.row),
            syncing: syncing(app),
        };
        // The focused card, not only its coordinates: a refresh that inserts a card above it
        // moves the same selection to a place the scroller has not revealed yet.
        let selection = app.board().map(|view| {
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
            &self.column_lists,
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

    /// Gives every column a list that knows the rows it now holds.
    ///
    /// A [`ListState`] carries the measured height of each tile, so it is told what changed
    /// rather than rebuilt: only the columns whose rows actually moved are spliced, and every
    /// other column keeps its measurements (`gpui-performance` rule 5). The comparison is on the
    /// rows and not only on their count, because a tile's height follows its title and its meta
    /// row — a card edited in place is a new height at the same index.
    fn sync_lists(&mut self, model: &Rc<BoardModel>) {
        if self
            .listed
            .as_ref()
            .is_some_and(|listed| Rc::ptr_eq(listed, model))
        {
            return;
        }
        self.column_lists
            .resize_with(model.columns.len(), KanbanColumn::list_state);
        for (index, column) in model.columns.iter().enumerate() {
            let previous = self
                .listed
                .as_ref()
                .and_then(|listed| listed.columns.get(index));
            if previous.is_some_and(|old| old.rows == column.rows) {
                continue;
            }
            if let Some(list) = self.column_lists.get(index) {
                list.splice(
                    0..previous.map_or(0, |old| old.rows.len()),
                    column.rows.len(),
                );
            }
        }
        self.listed = Some(Rc::clone(model));
    }

    /// Keeps the focused column and card inside their scrollers.
    fn reveal_focus(&self, (column, row): (usize, usize)) {
        self.board_scroll.scroll_to_item(column);
        if let Some(list) = self.column_lists.get(column) {
            list.scroll_to_reveal_item(row);
        }
    }
}
