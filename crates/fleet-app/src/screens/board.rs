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
use gpui::{AnyElement, App, Entity, FocusHandle, ScrollHandle, Window, prelude::*};

/// Hub tab for the active context's board.
pub struct BoardScreen {
    /// Horizontal scroller of the columns; `h` / `l` reveal the focused one.
    board_scroll: ScrollHandle,
    /// One vertical scroller per column; `j` / `k` reveal the focused card.
    column_scrolls: Vec<ScrollHandle>,
    /// Which board the column scrollers belong to; they are positional, not portable.
    scrolls_board: Option<BoardId>,
    revealed_focus: Option<(BoardId, usize, usize, Option<CardId>)>,
}

impl BoardScreen {
    /// Builds the screen using the frozen screen constructor.
    #[must_use]
    pub fn new(_cx: &mut App) -> Self {
        Self {
            board_scroll: ScrollHandle::new(),
            column_scrolls: Vec::new(),
            scrolls_board: None,
            revealed_focus: None,
        }
    }

    /// Renders the board and starts any pending authoritative load.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        ensure_current(state, bridge, cx);
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

// ---------------------------------------------------------------------------- loading

/// Ensures the active context's board through the ordinary asynchronous reply channel.
pub fn ensure_current(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    ensure_backends(state, bridge, cx);
    let Some((context_id, generation)) = state.update(cx, |state, _| state.begin_board_load())
    else {
        return;
    };
    let reply = bridge.request(RequestBody::EnsureBoard {
        context_id: context_id.clone(),
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let result = match reply.recv().await {
            Ok(Ok(ResponseBody::Board(view))) => Ok(view),
            Ok(Ok(_)) => Err("EnsureBoard returned an unexpected response".to_owned()),
            Ok(Err(error)) => Err(error.message),
            Err(error) => Err(format!("Board request channel closed: {error}")),
        };
        state.update(cx, |state, cx| {
            state.finish_board_load(&context_id, generation, result);
            cx.notify();
        });
    })
    .detach();
}

/// Fetches the daemon's backend registry once per connection.
///
/// Nothing in `fleet-app` knows a backend by name: the header's chip, the settings dialog's
/// kind cycler and every settings row it draws are built from these descriptors. A failure is
/// silent on purpose — the header falls back to the raw kind and the settings dialog to the
/// board's own kind, which is exactly what an older daemon would leave it with.
fn ensure_backends(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if !state.update(cx, |app, _| app.begin_backends_load()) {
        return;
    }
    let reply = bridge.request(RequestBody::ListBoardBackends {});
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::BoardBackends(backends))) = reply.recv().await else {
            // The ask is made once per connection and the flag was set before it went out, so
            // a failure has to release it or this connection never asks again.
            state.update(cx, |app, _| app.backends_load_failed());
            return;
        };
        state.update(cx, |app, cx| {
            app.apply_backends(backends);
            cx.notify();
        });
    })
    .detach();
}

/// Whether a `board.sync` job is queued or running for the shown board.
fn syncing(state: &AppState) -> bool {
    let Some(board) = state.board().map(|view| view.board.id.clone()) else {
        return false;
    };
    state.snapshot.as_ref().is_some_and(|snapshot| {
        snapshot.jobs.iter().any(|job| {
            matches!(&job.kind, JobKind::Custom(kind) if kind == "board.sync")
                && job.target == board.as_str()
                && matches!(job.status, JobStatus::Queued | JobStatus::Running)
        })
    })
}

// ---------------------------------------------------------------------------- selection

/// The selected unarchived card, in the contract's column order and behind the filter.
#[must_use]
pub fn selected_card(state: &AppState) -> Option<&Card> {
    let view = state.board()?;
    let status = view.board.statuses.get(state.board.focus.column)?;
    board_screen::visible_cards(view, &status.id, &state.board.filter)
        .get(state.board.focus.row)
        .copied()
}

/// The board being shown, when one is loaded.
#[must_use]
fn board_id(state: &AppState) -> Option<BoardId> {
    state.board().map(|view| view.board.id.clone())
}

/// Puts the focus on `card`, wherever the daemon's answer put it.
///
/// Every mutation lands here: a card that moved column, gained a label or was just created is
/// still the card the user is working on, and a selection that stays behind is a selection that
/// silently points at somebody else's card.
pub(crate) fn focus_card(app: &mut AppState, card: &CardId) {
    let found = app.board.view.as_ref().and_then(|view| {
        view.board
            .statuses
            .iter()
            .enumerate()
            .find_map(|(column, status)| {
                board_screen::visible_cards(view, &status.id, &app.board.filter)
                    .iter()
                    .position(|candidate| &candidate.id == card)
                    .map(|row| (column, row))
            })
    });
    if let Some((column, row)) = found {
        app.board.focus.column = column;
        app.board.focus.row = row;
    }
    app.clamp_board_focus();
}

/// Moves the focus by whole columns and rows, clamped at both ends (§5.11: never wraps).
fn step_focus(state: &Entity<AppState>, columns: isize, rows: isize, cx: &mut App) {
    let next = {
        let app = state.read(cx);
        let Some(view) = app.board() else {
            return;
        };
        let mut column = app.board.focus.column;
        let mut row = app.board.focus.row;
        if columns != 0 {
            column = crate::state::move_cursor(column, columns, view.board.statuses.len());
        }
        if rows != 0 {
            let len = view.board.statuses.get(column).map_or(0, |status| {
                board_screen::visible_cards(view, &status.id, &app.board.filter).len()
            });
            row = crate::state::move_cursor(row, rows, len);
        }
        (column, row)
    };
    state.update(cx, |app, cx| {
        app.board.focus.column = next.0;
        app.board.focus.row = next.1;
        app.clamp_board_focus();
        cx.notify();
    });
}

/// A mouse click on a tile or a column header.
fn on_click(click: BoardClick, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let (column, row, open) = match click {
        BoardClick::Column(column) => (column, None, false),
        BoardClick::Card(column, row) => (column, Some(row), false),
        BoardClick::OpenCard(column, row) => (column, Some(row), true),
    };
    state.update(cx, |app, cx| {
        app.board.focus.column = column;
        if let Some(row) = row {
            app.board.focus.row = row;
        }
        app.clamp_board_focus();
        cx.notify();
    });
    if open {
        open_card(state, bridge, cx);
    }
}

// ---------------------------------------------------------------------------- filter

/// Runs `edit` against the filter query while the input owns the keyboard.
fn edit_filter(state: &Entity<AppState>, cx: &mut App, edit: impl FnOnce(&mut String)) {
    let edited = state.update(cx, |app, cx| {
        if !app.board.filter_editing {
            return false;
        }
        edit(&mut app.board.filter);
        app.board.focus.row = 0;
        app.clamp_board_focus();
        cx.notify();
        true
    });
    if edited {
        cx.stop_propagation();
    }
}

/// Leaves the input but keeps the filter, which is stage one of the §3.10 `Esc`.
fn leave_filter_input(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.board.filter_editing = false;
        cx.notify();
    });
}

// ---------------------------------------------------------------------------- requests

/// Where a refused board request writes its message.
///
/// A dialog paints a scrim over the whole window, so a refusal written to the status bar's
/// sticky slot while one is open is a refusal nobody can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The status bar's sticky slot, for keys pressed on the board itself.
    Sticky,
    /// The error line of the card detail, while it is still showing this card.
    CardDetail(CardId),
}

impl Refusal {
    /// The sink a surface returning to the card detail must use.
    pub(crate) fn for_detail(detail: bool, card: &CardId) -> Self {
        if detail {
            Self::CardDetail(card.clone())
        } else {
            Self::Sticky
        }
    }

    pub(crate) fn report(self, state: &Entity<AppState>, message: String, cx: &mut App) {
        let showing = |card: &CardId, cx: &mut App| {
            matches!(
                state.read(cx).overlay,
                Some(Overlay::Dialog(Dialogs::CardDetail))
            ) && dialogs::with_host(state, cx, |host| {
                host.card_detail.card_id.as_ref() == Some(card)
            })
        };
        match self {
            Self::Sticky => fail(state, message, cx),
            // The detail can be closed and reopened on another card before a slow refusal
            // lands: card A's message on card B's dialog names a card the user did not touch.
            Self::CardDetail(card) if showing(&card, cx) => {
                dialogs::with_host(state, cx, |host| host.card_detail.error = Some(message));
                state.update(cx, |_, cx| cx.notify());
            }
            // The reply can arrive after the detail closed, and the next open reseeds the
            // slot: written there, the refusal would never be shown anywhere at all.
            Self::CardDetail(_) => fail(state, message, cx),
        }
    }
}

/// Sends a card mutation and applies the daemon's answer; failures take the sticky slot.
pub(crate) fn send_card(
    state: &Entity<AppState>,
    bridge: &Bridge,
    body: RequestBody,
    cx: &mut App,
) {
    send_card_reporting(state, bridge, body, Refusal::Sticky, cx);
}

/// Sends a card mutation and writes a refusal to `refusal`.
pub(crate) fn send_card_reporting(
    state: &Entity<AppState>,
    bridge: &Bridge,
    body: RequestBody,
    refusal: Refusal,
    cx: &mut App,
) {
    let reply = bridge.request(body);
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = match reply.recv().await {
            Ok(answer) => answer,
            Err(error) => {
                let message = format!("Board request channel closed: {error}");
                cx.update(|cx| refusal.report(&state, message, cx));
                return;
            }
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::Card(card)) => state.update(cx, |app, cx| {
                let id = card.id.clone();
                app.apply_card(card);
                focus_card(app, &id);
                cx.notify();
            }),
            Ok(ResponseBody::Ack) => state.update(cx, |app, cx| {
                app.board_stale = true;
                cx.notify();
            }),
            Ok(_) => {}
            Err(error) => refusal.report(&state, error.message, cx),
        });
    })
    .detach();
}

/// Records a refusal in the sticky slot (§1.8: an error is never a toast).
pub(crate) fn fail(state: &Entity<AppState>, message: String, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.sticky_error = Some(StickyError {
            text: message,
            job: None,
            retryable: false,
        });
        cx.notify();
    });
}

/// Opens a worktree's session and routes the app to it.
///
/// `refusal` is where a failure is written: the card detail dialog paints a scrim over the
/// status bar, so a refusal sent to the sticky slot from there is one nobody can read.
pub(crate) fn open_session(
    worktree: WorktreeId,
    refusal: Refusal,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    let reply = bridge.request(RequestBody::EnsureSession {
        worktree: Some(worktree),
        agent: None,
        sleep_previous: true,
    });
    let state = state.clone();
    // Starting a session takes as long as it takes, and the user keeps typing meanwhile.
    let requested_from = state.read(cx).overlay.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::Session(session)) => state.update(cx, |app, cx| {
                // Anything opened since the request is the user's current work: routing to the
                // session now would close it and throw its draft away.
                if app.overlay != requested_from {
                    return;
                }
                app.touch_session(session.id.clone());
                app.screen = Screen::Workspace {
                    session: session.id.clone(),
                };
                app.close_overlay();
                cx.notify();
            }),
            Ok(_) => {}
            Err(error) => refusal.report(&state, error.message, cx),
        });
    })
    .detach();
}

/// Creates the worktree a card asks for, then links and opens nothing: §8 keeps `w` and `o`
/// separate so a create never steals the screen while its hooks are still running.
pub(crate) fn request_worktree(
    card: CardId,
    repo: Option<RepoId>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    request_worktree_reporting(card, repo, state, bridge, Refusal::Sticky, cx);
}

/// Creates a card's worktree and writes a refusal to `refusal`.
pub(crate) fn request_worktree_reporting(
    card: CardId,
    repo: Option<RepoId>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    refusal: Refusal,
    cx: &mut App,
) {
    let reply = bridge.request(RequestBody::CreateWorktreeFromCard {
        card_id: card,
        repo_id: repo,
        base: None,
        host: None,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::CardWorktree { card, worktree, .. }) => state.update(cx, |app, cx| {
                let id = card.id.clone();
                app.apply_card(card);
                focus_card(app, &id);
                app.toast_short(
                    format!("\u{2713} {}", worktree.id.as_str()),
                    Icon::GitBranchPlus,
                    Instant::now(),
                );
                cx.notify();
            }),
            Ok(_) => {}
            Err(error) => refusal.report(&state, error.message, cx),
        });
    })
    .detach();
}

// ---------------------------------------------------------------------------- commands

/// Opens a board dialog with a fresh draft.
pub fn open_dialog(state: &Entity<AppState>, dialog: Dialogs, cx: &mut App) {
    let from_detail = state.read(cx).overlay == Some(Overlay::Dialog(Dialogs::CardDetail));
    let selected = selected_card(state.read(cx)).map(|card| card.id.clone());
    dialogs::with_host(state, cx, |host| {
        if dialog == Dialogs::CardPicker {
            host.card_picker.card_id = picker_target(selected, from_detail, &host.card_detail);
            host.card_picker.then_detail = from_detail;
        }
        host.open = None;
    });
    state.update(cx, |state, cx| {
        state.open_overlay(Overlay::Dialog(dialog));
        cx.notify();
    });
}

fn picker_target(
    selected: Option<CardId>,
    from_detail: bool,
    detail: &crate::dialogs::card_detail::CardDetailState,
) -> Option<CardId> {
    if from_detail {
        detail.card_id.clone()
    } else {
        selected
    }
}

/// The sentence a field the backend cannot write earns, or `None` when it is writable.
///
/// The app knows no backend by name: the field comes from `board.sync.readonly_fields`, which
/// `sync::adopt_schema` copied out of the backend's own `BackendSchema`, and the system's name
/// comes from the registry's descriptor. Both are the daemon's words, not the app's.
#[must_use]
pub fn readonly_message(state: &AppState, kind: &PickerKind) -> Option<String> {
    let field = kind.card_field()?;
    if !state.is_readonly_field(field) {
        return None;
    }
    let backend = state.board().map_or_else(String::new, |view| {
        state.backend_label(&view.board.backend.kind)
    });
    // The picker's own word, not the wire key: "Due date is read-only" is the sentence a user
    // can act on; "due_date is read-only" is one they have to translate first.
    Some(format!("{} is read-only on {backend} boards", kind.label()))
}

/// Opens the picker for `kind` against the focused card.
fn open_picker(state: &Entity<AppState>, kind: PickerKind, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    if selected_card(state.read(cx)).is_none() {
        needs(state, NO_CARD, cx);
        return;
    }
    // A picker that opens on a field the daemon will refuse is a dialog whose `Enter` can only
    // fail. The refusal is a fact about the backend, not a failed keystroke, so it is a toast
    // (§1.8 keeps the sticky slot for failures).
    if let Some(message) = readonly_message(state.read(cx), &kind) {
        toast_readonly(state, message, cx);
        return;
    }
    dialogs::with_host(state, cx, |host| {
        host.card_picker.kind = kind;
        host.card_picker.then_worktree = false;
    });
    open_dialog(state, Dialogs::CardPicker, cx);
}

/// Refuses a mutating key while the daemon is gone, flashing the banner instead (§3.12 C).
pub(crate) fn refuses(state: &Entity<AppState>, cx: &mut App) -> bool {
    let refuses = state.read(cx).refuses_mutations();
    if refuses {
        state.update(cx, |app, cx| {
            app.toast_short("fleetd is not reachable", Icon::Unplug, Instant::now());
            cx.notify();
        });
    }
    refuses
}

/// Moves the card focus by whole rows; the Hub's shared cursor keys land here on this tab.
pub(crate) fn move_rows(state: &Entity<AppState>, rows: isize, cx: &mut App) {
    step_focus(state, 0, rows, cx);
}

/// Jumps the card focus to the first or last card of the focused column.
pub(crate) fn jump_rows(state: &Entity<AppState>, bottom: bool, cx: &mut App) {
    let len = {
        let app = state.read(cx);
        app.board().map_or(0, |view| {
            view.board
                .statuses
                .get(app.board.focus.column)
                .map_or(0, |status| {
                    board_screen::visible_cards(view, &status.id, &app.board.filter).len()
                })
        })
    };
    state.update(cx, |app, cx| {
        app.board.focus.row = if bottom { len.saturating_sub(1) } else { 0 };
        app.clamp_board_focus();
        cx.notify();
    });
}

/// Says what a key needs before it can do anything.
/// §8 gives every board key an action. A key that neither acts nor says anything reads as a
/// broken app, and an empty board — or a filter that hides every card — is where that shows.
const NO_CARD: &str = "No card selected";
/// What `x` needs: a card the backend has linked and published an address for.
pub(crate) const NO_REMOTE: &str = "No remote issue on this card";
/// What `x` answers on a linked card whose board never learned an address for it.
///
/// The card *has* a remote issue; only `JiraSettings::browse_url` had nothing to build a URL
/// out of. Answering "No remote issue on this card" there names the wrong fact and leaves the
/// reader with nowhere to go — the fix is a board setting, so the sentence names it.
pub(crate) const NO_REMOTE_URL: &str =
    "This board has no address for its backend \u{2014} set its site in board settings";
/// What `d` answers on a card the backend owns: the sync would bring it straight back.
pub(crate) const NO_DELETE_MIRRORED: &str = "Mirrored card \u{2014} delete it in the backend";

fn needs(state: &Entity<AppState>, message: &'static str, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.toast_short(message, Icon::Boxes, Instant::now());
        cx.notify();
    });
}

/// Says that the backend, not Fleet, owns this field.
pub(crate) fn toast_readonly(state: &Entity<AppState>, message: String, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.toast_short(message, Icon::Lock, Instant::now());
        cx.notify();
    });
}

/// `g b` — go to the board tab and load the active context's board.
pub fn go_board(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    state.update(cx, |state, cx| {
        state.screen = Screen::Hub { tab: HubTab::Board };
        state.hub_pane = HubPane::List;
        state.filter = crate::state::FilterState::default();
        state.board.filter.clear();
        state.board.filter_editing = false;
        state.breadcrumb_row = None;
        state.board_stale = true;
        cx.notify();
    });
    ensure_current(state, bridge, cx);
}

/// `h` / `←` — previous column.
pub fn prev_column(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, -1, 0, cx);
}

/// `l` / `→` — next column.
pub fn next_column(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, 1, 0, cx);
}

/// `j` / `↓` — next card.
pub fn next_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, 0, 1, cx);
}

/// `k` / `↑` — previous card.
pub fn prev_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, 0, -1, cx);
}

/// `Enter` — open the card detail dialog.
pub fn open_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    if selected_card(state.read(cx)).is_none() {
        needs(state, NO_CARD, cx);
        return;
    }
    open_dialog(state, Dialogs::CardDetail, cx);
}

/// `c` — new card.
pub fn new_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    if state.read(cx).board().is_none() {
        needs(state, "No board loaded yet", cx);
        return;
    }
    open_dialog(state, Dialogs::CardCreate, cx);
}

/// `s` — status picker.
pub fn pick_status(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Status, cx);
}

/// `p` — priority picker.
pub fn pick_priority(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Priority, cx);
}

/// `a` — assignee picker.
pub fn pick_assignee(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Assignee, cx);
}

/// `t` — labels picker.
pub fn pick_labels(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Labels, cx);
}

/// `e` — estimate picker.
pub fn pick_estimate(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Estimate, cx);
}

/// The status one column away from the focused one, when there is one.
fn adjacent_status(state: &AppState, delta: isize) -> Option<(StatusId, usize)> {
    let view = state.board()?;
    let index = state.board.focus.column.checked_add_signed(delta)?;
    view.board
        .statuses
        .get(index)
        .map(|status| (status.id.clone(), index))
}

/// `[` / `]` — move the focused card one column left or right.
fn move_card(state: &Entity<AppState>, bridge: &Bridge, delta: isize, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let Some(card) = selected_card(state.read(cx)).map(|card| card.id.clone()) else {
        needs(state, NO_CARD, cx);
        return;
    };
    // `ops::move_card` refuses the same thing; saying so here costs no round trip.
    if let Some(message) = readonly_message(state.read(cx), &PickerKind::Status) {
        toast_readonly(state, message, cx);
        return;
    }
    let Some((status_id, _)) = adjacent_status(state.read(cx), delta) else {
        return;
    };
    send_card(
        state,
        bridge,
        RequestBody::MoveCard {
            card_id: card,
            status_id,
            index: None,
        },
        cx,
    );
}

/// `[` — move the card to the previous column.
pub fn move_prev_column(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    move_card(state, bridge, -1, cx);
}

/// `]` — move the card to the next column.
pub fn move_next_column(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    move_card(state, bridge, 1, cx);
}

/// `w` — create a worktree from the focused card, asking for a repo only when nothing knows one.
pub fn create_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let Some(card) = selected_card(state.read(cx)).cloned() else {
        needs(state, NO_CARD, cx);
        return;
    };
    let default_repo = state
        .read(cx)
        .board()
        .and_then(|view| view.board.default_repo_id.clone());
    if card.repo_id.is_none() && default_repo.is_none() {
        // The picker only offers the board context's repositories; with none, its whole list is
        // the "No repository" clear row, whose Enter answers "Pick a repository" forever.
        if !has_repo_in_context(state.read(cx)) {
            needs(state, NO_REPO_IN_CONTEXT, cx);
            return;
        }
        dialogs::with_host(state, cx, |host| {
            host.card_picker.kind = PickerKind::Repo;
            host.card_picker.then_worktree = true;
        });
        open_dialog(state, Dialogs::CardPicker, cx);
        return;
    }
    request_worktree(card.id, None, state, bridge, cx);
}

/// What `w` answers when the context holds no repository the picker could offer.
pub(crate) const NO_REPO_IN_CONTEXT: &str = "No repository in this context";

/// Whether the board's context has a repository a card may be pointed at.
pub(crate) fn has_repo_in_context(state: &AppState) -> bool {
    let Some(context) = state.board().map(|view| view.board.context_id.clone()) else {
        return false;
    };
    state
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.repos.iter().any(|repo| repo.context_id == context))
}

/// `o` — open the session of the worktree this card created.
pub fn open_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let Some(card) = selected_card(state.read(cx)) else {
        needs(state, NO_CARD, cx);
        return;
    };
    let Some(worktree) = card.worktree_id.clone() else {
        needs(state, "No worktree yet \u{2014} w creates one", cx);
        return;
    };
    open_session(worktree, Refusal::Sticky, state, bridge, cx);
}

/// `S` — sync the board against its backend; `full` (`F`) ignores the incremental cursor.
pub fn sync(state: &Entity<AppState>, bridge: &Bridge, full: bool, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let Some(board) = board_id(state.read(cx)) else {
        needs(state, "No board loaded yet", cx);
        return;
    };
    let reply = bridge.request(RequestBody::SyncBoard {
        board_id: board,
        full,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::Job(job)) => state.update(cx, |app, cx| {
                app.toast_short(job.title.clone(), Icon::RefreshCw, Instant::now());
                cx.notify();
            }),
            Ok(_) => {}
            // A local board has no remote to pull from; that is a fact, not a failure, so it
            // takes a toast rather than the sticky error slot. A backend that refused for any
            // other reason did fail, and §1.8 keeps failures in the sticky slot.
            Err(error) if error.kind == ErrorKind::Unsupported => state.update(cx, |app, cx| {
                app.toast_short(error.message.clone(), Icon::CloudOff, Instant::now());
                cx.notify();
            }),
            Err(error) => fail(&state, error.message, cx),
        });
    })
    .detach();
}

/// `x` — open the focused card's remote issue in the browser.
///
/// The link is the one the backend itself put on the card (`RemoteLink.url`), so a backend
/// that publishes no browsable URL simply has no `x`, and the app never builds one.
pub fn open_remote(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let Some(card) = selected_card(state.read(cx)) else {
        needs(state, NO_CARD, cx);
        return;
    };
    let Some(url) = remote_url(card) else {
        needs(state, no_remote_reason(card), cx);
        return;
    };
    cx.open_url(&url);
}

/// Why `x` could not open this card, in the words the surface it was pressed on shows.
///
/// Both surfaces answer with this: a linked card whose board has no address for its backend is
/// a different fact from a card with no remote issue at all, and only one of them names the
/// setting that fixes it.
#[must_use]
pub(crate) fn no_remote_reason(card: &Card) -> &'static str {
    if card.remote.is_some() {
        NO_REMOTE_URL
    } else {
        NO_REMOTE
    }
}

/// The browsable address of a card's remote issue, when it has one.
#[must_use]
pub fn remote_url(card: &Card) -> Option<String> {
    card.remote
        .as_ref()?
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(ToOwned::to_owned)
}

/// `d` — delete the focused card through §3.8.3's Confirm dialog.
pub fn delete_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let target = {
        let app = state.read(cx);
        app.board().zip(selected_card(app)).map(|(view, card)| {
            (
                card.id.clone(),
                card.display_key(&view.board),
                card.title.clone(),
                card.remote.is_some(),
            )
        })
    };
    let Some((card, key, title, mirrored)) = target else {
        needs(state, NO_CARD, cx);
        return;
    };
    // The daemon refuses this one, and for a reason the dialog cannot state honestly: the
    // deletion would only drop the local comments and activity, and the next sync would file
    // the issue again as a new card.
    if mirrored {
        needs(state, NO_DELETE_MIRRORED, cx);
        return;
    }
    dialogs::request_confirm(cx, ConfirmRequest::DeleteCard { card, key, title });
    state.update(cx, |app, cx| {
        app.open_overlay(Overlay::Dialog(Dialogs::Confirm));
        cx.notify();
    });
}

/// `,` — board settings.
pub fn settings(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    if state.read(cx).board().is_none() {
        needs(state, "No board loaded yet", cx);
        return;
    }
    open_dialog(state, Dialogs::BoardSettings, cx);
}

/// `r` — reload the board from the daemon.
pub fn reload(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    // `begin_board_load` returns early with the daemon gone and only writes a message when no
    // board is loaded, so clearing the error first would erase the visible failure and leave
    // nothing at all behind: no reload, no banner, no toast.
    if refuses(state, cx) {
        return;
    }
    state.update(cx, |state, cx| {
        state.board_stale = true;
        state.board.error = None;
        cx.notify();
    });
    ensure_current(state, bridge, cx);
}

/// `/` — put the keyboard in the filter input.
pub fn filter(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.board.filter_editing = true;
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BoardFocus;
    use fleet_core::board::{BoardView, CardDraft, create_card, new_board};

    fn view() -> BoardView {
        let context = fleet_core::model::Context {
            id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
            name: "Fleet".into(),
            owners: vec![],
            created_at: "2026-09-06T12:00:00Z".into(),
        };
        let mut board = new_board(&context, "2026-09-06T12:00:00Z");
        let mut cards = Vec::new();
        for (index, title) in ["Fix login", "Ship the board"].iter().enumerate() {
            let card = create_card(
                &mut board,
                &cards,
                format!("card-{index}")
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                CardDraft {
                    title: (*title).to_owned(),
                    ..CardDraft::default()
                },
                "2026-09-06T12:00:00Z",
            )
            .unwrap_or_else(|error| panic!("{error}"));
            cards.push(card);
        }
        BoardView { board, cards }
    }

    fn state() -> AppState {
        let mut state = AppState::new("/tmp/fleet-board-screen", Instant::now());
        let view = view();
        let column = view
            .board
            .statuses
            .iter()
            .position(|status| status.id == view.cards[0].status_id)
            .unwrap_or_else(|| panic!("no column"));
        state.board.view = Some(view);
        state.board.focus = BoardFocus { column, row: 0 };
        state
    }

    #[test]
    fn detail_picker_keeps_its_card_when_refresh_moves_it_away_from_selection() {
        let mut state = state();
        let detail_id = selected_card(&state).unwrap().id.clone();
        let draft = crate::dialogs::card_detail::CardDetailState {
            card_id: Some(detail_id.clone()),
            ..Default::default()
        };
        state.board.view.as_mut().unwrap().cards[0].status_id = "done".parse().unwrap();
        state.clamp_board_focus();
        let selected = selected_card(&state).map(|card| card.id.clone());
        assert_ne!(selected, Some(detail_id.clone()));
        assert_eq!(picker_target(selected, true, &draft), Some(detail_id));
    }

    #[test]
    fn the_selection_follows_the_filter_not_the_raw_column() {
        let mut state = state();
        assert_eq!(
            selected_card(&state).map(|card| card.title.clone()),
            Some("Fix login".to_owned())
        );
        state.board.filter = "board".to_owned();
        state.clamp_board_focus();
        assert_eq!(
            selected_card(&state).map(|card| card.title.clone()),
            Some("Ship the board".to_owned()),
            "the first visible card is the selected one"
        );
        state.board.filter = "zzz".to_owned();
        state.clamp_board_focus();
        assert!(selected_card(&state).is_none());
    }

    #[test]
    fn adjacent_columns_stop_at_both_ends() {
        let mut state = state();
        state.board.focus.column = 0;
        assert!(adjacent_status(&state, -1).is_none());
        assert!(adjacent_status(&state, 1).is_some());
        let last = state
            .board()
            .unwrap_or_else(|| panic!("no board"))
            .board
            .statuses
            .len()
            - 1;
        state.board.focus.column = last;
        assert!(adjacent_status(&state, 1).is_none());
    }

    #[test]
    fn a_board_without_a_sync_job_is_not_syncing() {
        let state = state();
        assert!(!syncing(&state));
    }

    #[test]
    fn a_read_only_field_is_named_with_the_backends_own_label() {
        let mut state = state();
        let board = &mut state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"))
            .board;
        board.backend.kind = "jira".into();
        board.sync.readonly_fields = vec!["priority".into(), "due_date".into()];
        state.apply_backends(vec![fleet_core::board::BackendDescriptor {
            kind: "jira".into(),
            label: "Jira (acli)".into(),
            capabilities: fleet_core::board::BackendCapabilities::default(),
            settings_schema: Vec::new(),
        }]);
        assert_eq!(
            readonly_message(&state, &PickerKind::Priority).as_deref(),
            Some("Priority is read-only on Jira (acli) boards"),
            "the sentence names the field the user pressed a key for, not the wire key"
        );
        assert_eq!(
            readonly_message(&state, &PickerKind::DueDate).as_deref(),
            Some("Due date is read-only on Jira (acli) boards")
        );
        assert_eq!(readonly_message(&state, &PickerKind::Status), None);
        // The repository is Fleet's own link; no backend has an opinion about it.
        assert_eq!(readonly_message(&state, &PickerKind::Repo), None);
    }

    #[test]
    fn a_local_board_refuses_nothing_however_stale_its_list_is() {
        let mut state = state();
        state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"))
            .board
            .sync
            .readonly_fields = vec!["priority".into()];
        assert_eq!(readonly_message(&state, &PickerKind::Priority), None);
    }

    /// Both `x` surfaces answer with the same sentence, and it is not the same sentence for the
    /// two cases: the card detail's used to say "no remote issue" about a linked card whose
    /// board simply has no address for its backend, naming the wrong fact and hiding the fix.
    #[test]
    fn the_refusal_x_answers_with_names_which_of_the_two_facts_it_is() {
        let mut card = state()
            .board
            .view
            .as_ref()
            .unwrap_or_else(|| panic!("no board"))
            .cards[0]
            .clone();
        assert_eq!(no_remote_reason(&card), NO_REMOTE);
        card.remote = Some(fleet_core::board::RemoteLink {
            parent_key: None,
            backend: "jira".into(),
            key: "SP-1".into(),
            url: None,
            version: None,
            remote_updated_at: None,
            synced_at: "now".into(),
        });
        assert_eq!(no_remote_reason(&card), NO_REMOTE_URL);
    }

    #[test]
    fn only_a_link_with_an_address_is_openable() {
        let mut card = state()
            .board
            .view
            .as_ref()
            .unwrap_or_else(|| panic!("no board"))
            .cards[0]
            .clone();
        assert_eq!(remote_url(&card), None);
        let mut link = fleet_core::board::RemoteLink {
            parent_key: None,
            backend: "jira".into(),
            key: "SP-1".into(),
            url: None,
            version: None,
            synced_at: "2026-09-06T12:00:00Z".into(),
            remote_updated_at: None,
        };
        card.remote = Some(link.clone());
        assert_eq!(remote_url(&card), None, "a key is not an address");
        link.url = Some("   ".into());
        card.remote = Some(link.clone());
        assert_eq!(remote_url(&card), None, "and neither is blank text");
        link.url = Some(" https://example.test/browse/SP-1 ".into());
        card.remote = Some(link);
        assert_eq!(
            remote_url(&card).as_deref(),
            Some("https://example.test/browse/SP-1")
        );
    }
}
