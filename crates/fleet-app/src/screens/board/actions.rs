use super::*;
use chrono::{DateTime, Utc};
use fleet_core::{agents::Delegation, board::CardRun};
use fleet_ui_kit::format_duration;

/// Says what a key needs before it can do anything.
///
/// §8 gives every board key an action. A key that neither acts nor says anything reads as a
/// broken app, and an empty board — or a filter that hides every card — is where that shows.
pub(super) const NO_CARD: &str = "No card selected";
/// What `x` needs: a card the backend has linked and published an address for.
pub(super) const NO_REMOTE: &str = "No remote issue on this card";
/// What `x` answers on a linked card whose board never learned an address for it.
///
/// The card *has* a remote issue; only `JiraSettings::browse_url` had nothing to build a URL
/// out of. Answering "No remote issue on this card" there names the wrong fact and leaves the
/// reader with nowhere to go — the fix is a board setting, so the sentence names it.
pub(super) const NO_REMOTE_URL: &str =
    "This board has no address for its backend \u{2014} set its site in board settings";
/// What `d` answers on a card the backend owns: the sync would bring it straight back.
const NO_DELETE_MIRRORED: &str = "Mirrored card \u{2014} delete it in the backend";
/// What `w` answers when the context holds no repository the picker could offer.
pub(crate) const NO_REPO_IN_CONTEXT: &str = "No repository in this context";
/// What `o` answers on a card linked to the worktree whose Workspace the board is drawn in.
///
/// The Hub's board is always somewhere else, so `o` there is always a move. In the board pane
/// it can be a card pointing at the session already on screen, and `EnsureSession` on it would
/// sleep and re-open the very worktree the user is standing in.
pub(crate) const ALREADY_IN_WORKTREE: &str = "Already in this worktree";

/// Opens a board dialog with a fresh draft.
pub(crate) fn open_dialog(state: &Entity<AppState>, dialog: Dialogs, cx: &mut App) {
    let from_detail = state.read(cx).overlay == Some(Overlay::Dialog(Dialogs::CardDetail));
    let selected = selected_card(state.read(cx)).map(|card| card.id.clone());
    dialogs::with_host(state, cx, |host| {
        if dialog == Dialogs::CardPicker {
            host.card_picker.card_id =
                picker_target(selected, from_detail, host.card_detail.card_id.as_ref());
            host.card_picker.then_detail = from_detail;
        }
        host.open = None;
    });
    state.update(cx, |state, cx| {
        state.open_overlay(Overlay::Dialog(dialog));
        cx.notify();
    });
}

/// The card a picker edits: the card detail's own, whenever it is the surface that opened it.
///
/// A refresh that lands while the detail is up can move another card under the board's cursor,
/// and the picker has to keep editing the card the dialog is showing rather than follow it.
#[must_use]
pub(super) fn picker_target(
    selected: Option<CardId>,
    from_detail: bool,
    detail: Option<&CardId>,
) -> Option<CardId> {
    if from_detail {
        detail.cloned()
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
pub(crate) fn readonly_message(state: &AppState, kind: &PickerKind) -> Option<String> {
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

fn needs(state: &Entity<AppState>, message: &'static str, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.toast_short(message, Icon::Boxes, Instant::now());
        cx.notify();
    });
}

/// The same refusal, in a sentence that names the card it is about.
///
/// A key whose sentence is built per press rather than fixed: `{KEY} has no live run` is the
/// daemon's own wording, so the key and `fleet board` cannot say two different things about
/// one card (contracts §5.5).
pub(super) fn needs_named(state: &Entity<AppState>, message: String, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.toast_short(message, Icon::Boxes, Instant::now());
        cx.notify();
    });
}

/// Says that the backend, not Fleet, owns this field.
fn toast_readonly(state: &Entity<AppState>, message: String, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.toast_short(message, Icon::Lock, Instant::now());
        cx.notify();
    });
}

/// `g b` — go to the board tab and load the active context's board.
pub(crate) fn go_board(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
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

/// `Enter` — open the card detail dialog.
pub(crate) fn open_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    if selected_card(state.read(cx)).is_none() {
        needs(state, NO_CARD, cx);
        return;
    }
    open_dialog(state, Dialogs::CardDetail, cx);
}

/// `c` — new card.
pub(crate) fn new_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    if state.read(cx).board().is_none() {
        needs(state, "No board loaded yet", cx);
        return;
    }
    // `c` lands the card in the board's default column; only a column's `+` names one.
    dialogs::with_host(state, cx, |host| host.card_create_in = None);
    open_dialog(state, Dialogs::CardCreate, cx);
}

/// A column's `+` or `Add card` — New card with that column's status already chosen.
///
/// The same guards `c` has; the column is focused first, so the card the dialog makes lands
/// where the pointer asked and the cursor is already there to meet it.
pub(super) fn new_card_in(state: &Entity<AppState>, column: usize, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let Some(status) = state
        .read(cx)
        .board()
        .and_then(|view| view.board.statuses.get(column))
        .map(|status| status.id.clone())
    else {
        needs(state, "No board loaded yet", cx);
        return;
    };
    state.update(cx, |app, cx| {
        app.board.focus.column = column;
        app.clamp_board_focus();
        cx.notify();
    });
    dialogs::with_host(state, cx, |host| host.card_create_in = Some(status));
    open_dialog(state, Dialogs::CardCreate, cx);
}

/// A column's automation pill — board settings, opened drilled into that column.
///
/// Through the same guards `,` has; the column is only handed to the seed once the dialog is
/// actually opening, so a refused `,` leaves nothing behind for the next one.
pub(super) fn column_settings(
    state: &Entity<AppState>,
    bridge: &Bridge,
    column: usize,
    cx: &mut App,
) {
    state.update(cx, |app, cx| {
        app.board.focus.column = column;
        app.clamp_board_focus();
        cx.notify();
    });
    if refuses(state, cx) {
        return;
    }
    if state.read(cx).board().is_none() {
        needs(state, "No board loaded yet", cx);
        return;
    }
    dialogs::with_host(state, cx, |host| host.board_settings_column = Some(column));
    settings(state, bridge, cx);
}

/// `s` — status picker.
pub(crate) fn pick_status(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Status, cx);
}

/// `p` — priority picker.
pub(crate) fn pick_priority(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Priority, cx);
}

/// `a` — assignee picker.
pub(crate) fn pick_assignee(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Assignee, cx);
}

/// `t` — labels picker.
pub(crate) fn pick_labels(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Labels, cx);
}

/// `e` — estimate picker.
pub(crate) fn pick_estimate(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Estimate, cx);
}

/// `b` — the cards this one waits for (contracts §5.5).
pub(crate) fn pick_blocked_by(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::BlockedBy, cx);
}

/// `m` — which agent runs this card, `Model` and `Effort` being the rows after it.
pub(crate) fn pick_agent(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    open_picker(state, PickerKind::Provider, cx);
}

/// The status one column away from the focused one, when there is one.
#[must_use]
pub(super) fn adjacent_status(state: &AppState, delta: isize) -> Option<(StatusId, usize)> {
    let view = state.board()?;
    let index = state.board.focus.column.checked_add_signed(delta)?;
    view.board
        .statuses
        .get(index)
        .map(|status| (status.id.clone(), index))
}

/// Whether this card has a run that a move or an `X` has to deal with.
///
/// Read exactly as the tile's mark is (`AppState::refresh_card_marks`): the mirror first, the
/// card's own rows second. A child the daemon has started is named by a `DelegationChanged`
/// that lands in milliseconds, while the run row waits for the `BoardChanged` reload — so a
/// key that consulted the card alone would refuse, or move silently, on a card the face is
/// already drawing as `working`. The two surfaces have to say the same thing about one card.
#[must_use]
pub(super) fn has_live_or_pending_run(state: &AppState, card: &Card) -> bool {
    card.pending_run.is_some()
        || card.runs.last().is_some_and(CardRun::is_live)
        || live_card_child(state, card).is_some()
}

/// The live card-called child this card has out, when the delegation mirror holds one.
///
/// The join is the mirror's own caller — `Card { board, card }` — not the card's newest run
/// id: a card-called `DelegationChanged` names its board and its card, so the mirror can
/// answer for a run the card's `runs` has not caught up with yet (`BOARD.md` §11.8). The one
/// thing that outranks it is the card's own outcome: once its row says a run ended, a mirror
/// row that has not caught up says nothing, exactly as the mark fold treats it.
#[must_use]
pub(super) fn live_card_child<'a>(state: &'a AppState, card: &Card) -> Option<&'a Delegation> {
    let delegation = state.agents.live_card_run(&card.board_id, &card.id)?;
    card.runs
        .iter()
        .all(|run| run.id != delegation.id || run.outcome.is_none())
        .then_some(delegation)
}

/// How long a child has been going, in the words every agent surface states one.
#[must_use]
fn child_elapsed(delegation: &Delegation, now: DateTime<Utc>) -> String {
    let millis = delegation.elapsed(now).num_milliseconds().max(0);
    format_duration(u64::try_from(millis).unwrap_or_default()).to_string()
}

/// The confirm a move (`[` / `]` or a drop) must raise before it moves this card, or `None` for
/// a plain move.
///
/// One question, asked of the mirror (§5.5): does this card have a live card-called child out?
/// Whether the card's own `runs` has recorded it yet is not part of it — that row arrives a
/// board round trip later, and gating on it is what let a `[` pressed the instant the tile
/// said `needs you` send a plain move and collect the daemon's `Conflict` instead. A card the
/// mirror holds no live child for moves with no question; if the daemon disagrees, its
/// `Conflict` is what says so, through `lifecycle::fail`.
#[must_use]
pub(super) fn move_cancels_run(
    state: &AppState,
    target: usize,
    now: DateTime<Utc>,
) -> Option<ConfirmRequest> {
    let view = state.board()?;
    let card = selected_card(state)?;
    let delegation = live_card_child(state, card)?;
    Some(ConfirmRequest::MoveCancelsRun {
        card: card.id.clone(),
        key: card.display_key(&view.board),
        target: view.board.statuses.get(target)?.name.clone(),
        elapsed: child_elapsed(delegation, now),
    })
}

/// Where a move puts the focused card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Destination {
    /// `[` / `]`: the end of the column this many columns away.
    Step(isize),
    /// A dropped card: this column, at this place among the cards it already holds — the
    /// daemon's own `index` (`ops::move_card`), counted without the moving card.
    At { column: usize, index: usize },
}

/// The one move path: `[` / `]` and a dropped card both land here, so the refusals, the
/// read-only toast and the confirm over a live run are the same sentences whichever the user
/// reached for.
fn move_card(state: &Entity<AppState>, bridge: &Bridge, to: Destination, cx: &mut App) {
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
    let (resolved, index) = match to {
        Destination::Step(delta) => (adjacent_status(state.read(cx), delta), None),
        Destination::At { column, index } => (
            state
                .read(cx)
                .board()
                .and_then(|view| view.board.statuses.get(column))
                .map(|status| (status.id.clone(), column)),
            Some(index),
        ),
    };
    let Some((status_id, column)) = resolved else {
        return;
    };
    // A child that is out there working is not stopped behind the user's back: the move and
    // the cancellation are one sentence, answered once (§5.5).
    if let Some(request) = move_cancels_run(state.read(cx), column, Utc::now()) {
        ConfirmRequest::stage_move_target(
            state,
            MoveTarget {
                status: status_id,
                index,
            },
            cx,
        );
        dialogs::request_confirm(cx, request);
        state.update(cx, |app, cx| {
            app.open_overlay(Overlay::Dialog(Dialogs::Confirm));
            cx.notify();
        });
        return;
    }
    send_card(
        state,
        bridge,
        RequestBody::MoveCard {
            card_id: card,
            status_id,
            index,
            cancel_run: false,
        },
        cx,
    );
}

/// A card dropped on `column` at `slot`, a place among the tiles that column draws.
///
/// The card is selected first, exactly as a press on its tile does, and then it takes the
/// path `[` / `]` take. A drop back where the card already stands asks for nothing.
pub(super) fn drop_card(
    state: &Entity<AppState>,
    bridge: &Bridge,
    card: &CardId,
    column: usize,
    slot: usize,
    cx: &mut App,
) {
    state.update(cx, |app, cx| {
        app.select_card(card);
        cx.notify();
    });
    let destination = {
        let app = state.read(cx);
        let Some(view) = app.board() else {
            return;
        };
        drop_destination(view, &app.board.filter, card, column, slot)
    };
    if let Some(destination) = destination {
        move_card(state, bridge, destination, cx);
    }
}

/// Where a card dropped on `column` at `slot` goes, or `None` for a drop that moves nothing.
///
/// `slot` counts the tiles the column *draws*, the dragged card among them when it is already
/// there; the daemon's `index` counts the whole column without it. The two differ under a
/// filter, so the drop lands before the drawn card it was dropped above — or after the last
/// drawn card — wherever the hidden ones sit.
#[must_use]
pub(super) fn drop_destination(
    view: &fleet_core::board::BoardView,
    filter: &str,
    card: &CardId,
    column: usize,
    slot: usize,
) -> Option<Destination> {
    let status = view.board.statuses.get(column)?;
    let shown = crate::views::board_screen::visible_cards(view, &status.id, filter);
    let from = shown.iter().position(|candidate| &candidate.id == card);
    if from.is_some_and(|from| slot == from || slot == from + 1) {
        return None;
    }
    let others: Vec<&Card> = shown
        .into_iter()
        .filter(|candidate| &candidate.id != card)
        .collect();
    let slot = match from {
        Some(from) if slot > from => slot - 1,
        _ => slot,
    }
    .min(others.len());
    let whole: Vec<&CardId> = fleet_core::board::column_cards(&view.cards, &status.id)
        .into_iter()
        .map(|candidate| &candidate.id)
        .filter(|candidate| *candidate != card)
        .collect();
    let rank = |target: &Card| whole.iter().position(|candidate| **candidate == target.id);
    let index = match (others.get(slot), others.last()) {
        (Some(before), _) => rank(before)?,
        (None, Some(last)) => rank(last)? + 1,
        (None, None) => whole.len(),
    };
    Some(Destination::At { column, index })
}

/// What `X` would stop on this card, or the sentence saying why it cannot.
///
/// The `Ok` triple is exactly what [`ConfirmRequest::stage_card_run_cancel`] takes: the card,
/// the key the dialog titles itself with, and the one risk line it states.
fn card_run_cancel_draft(
    state: &AppState,
    card: &CardId,
    now: DateTime<Utc>,
) -> Result<(CardId, String, String), String> {
    let Some(view) = state.board() else {
        return Err("No board loaded yet".to_owned());
    };
    let Some(card) = view.cards.iter().find(|item| &item.id == card) else {
        return Err(NO_CARD.to_owned());
    };
    let key = card.display_key(&view.board);
    if !has_live_or_pending_run(state, card) {
        // The daemon's own refusal, word for word (`boards::automation`), so the key and
        // `fleet board card cancel` cannot say two different things about one card.
        return Err(format!("{key} has no live run"));
    }
    let fact = if card.pending_run.is_some() {
        format!("{key} is waiting for a free slot; the run it is owed is dropped")
    } else if let Some(delegation) = live_card_child(state, card) {
        format!(
            "child {} stops after {} and reports no further work",
            delegation.child,
            child_elapsed(delegation, now)
        )
    } else {
        // The card is the authority on whether its run ended; the mirror simply never heard
        // of this child, which is what a reconnect looks like from here.
        format!("{key}'s run stops and reports no further work")
    };
    Ok((card.id.clone(), key, fact))
}

/// Raises §3.8.3's confirm over the run `X` would stop (contracts §5.5).
///
/// The split with [`super::runs::cancel_run`] is the one the phase draws: the key owns *which
/// card, and may it at all* — it has already refused an unreachable daemon, a board with no
/// cursor and a card with no live run in the daemon's own words — and this owns *what exactly
/// stops, and did the user say yes*. Staged and adopted rather than sent, because cancelling a
/// run is irreversible: nothing reaches the daemon before the user has read back which card it
/// is and what goes with it.
pub(super) fn ask_cancel_run(state: &Entity<AppState>, card: &CardId, cx: &mut App) {
    match card_run_cancel_draft(state.read(cx), card, Utc::now()) {
        Ok((card, key, fact)) => {
            ConfirmRequest::stage_card_run_cancel(state, card, key, fact, cx);
            state.update(cx, |app, cx| {
                app.open_overlay(Overlay::Dialog(Dialogs::Confirm));
                cx.notify();
            });
        }
        // Unreachable through the key, which resolved the same card a moment earlier; a board
        // that was swapped in between still says what it can no longer do.
        Err(message) => needs_named(state, message, cx),
    }
}

/// `[` — move the card to the previous column.
pub(crate) fn move_prev_column(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    move_card(state, bridge, Destination::Step(-1), cx);
}

/// `]` — move the card to the next column.
pub(crate) fn move_next_column(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    move_card(state, bridge, Destination::Step(1), cx);
}

/// `w` — create a worktree from the focused card, asking for a repo only when nothing knows one.
pub(crate) fn create_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
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

/// Whether the board's context has a repository a card may be pointed at.
#[must_use]
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
pub(crate) fn open_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
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
    // Only the board pane can be standing in the worktree a card names; from the Hub `o` is
    // always a move to somewhere else.
    let app = state.read(cx);
    if app.board_pane_is_active() && app.active_worktree() == Some(&worktree) {
        needs(state, ALREADY_IN_WORKTREE, cx);
        return;
    }
    open_session(worktree, Refusal::Sticky, state, bridge, cx);
}

/// `S` — sync the board against its backend; `full` (`F`) ignores the incremental cursor.
pub(crate) fn sync(state: &Entity<AppState>, bridge: &Bridge, full: bool, cx: &mut App) {
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
pub(crate) fn open_remote(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
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
pub(crate) fn remote_url(card: &Card) -> Option<String> {
    card.remote
        .as_ref()?
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(ToOwned::to_owned)
}

/// `d` — delete the focused card through §3.8.3's Confirm dialog.
pub(crate) fn delete_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
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
pub(crate) fn settings(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
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
pub(crate) fn reload(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
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
pub(crate) fn filter(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.board.filter_editing = true;
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BoardFocus;
    use fleet_core::{
        agents::{
            AgentKind, Delegation, DelegationCaller, DelegationId, DelegationStatus, DeliveryState,
            ThreadId,
        },
        board::{ActionKind, BoardView, CardDraft, PendingRun, RunOutcome, create_card, new_board},
    };

    const NOW: &str = "2026-09-20T12:00:00Z";

    /// A one-card board with the cursor on the card, in the column it was created in.
    fn board() -> AppState {
        let context = fleet_core::model::Context {
            id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
            name: "Fleet".into(),
            owners: vec![],
            created_at: NOW.into(),
        };
        let mut board = new_board(&context, NOW);
        let card = create_card(
            &mut board,
            &[],
            "card-0".parse().unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: "Fix login".to_owned(),
                ..CardDraft::default()
            },
            NOW,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let column = board
            .statuses
            .iter()
            .position(|status| status.id == card.status_id)
            .unwrap_or_else(|| panic!("the card landed in no column"));
        let mut state = AppState::new("/tmp/fleet-board-run-keys", Instant::now());
        state.board.view = Some(BoardView {
            board,
            cards: vec![card],
            live_runs: Vec::new(),
        });
        state.board.focus = BoardFocus { column, row: 0 };
        state
    }

    /// The column one to the right of the focused one, by name.
    fn next_column_name(state: &AppState) -> String {
        state
            .board()
            .unwrap_or_else(|| panic!("no board"))
            .board
            .statuses[state.board.focus.column + 1]
            .name
            .clone()
    }

    /// Puts a live run on the board's only card and returns the child behind it.
    fn live_run(state: &mut AppState, caller: DelegationCaller) -> Delegation {
        let delegation = child(caller);
        record_run(state, &delegation, None);
        delegation
    }

    /// Records `delegation`'s run on the board's only card, ended as `outcome` says.
    ///
    /// The daemon writes this row and announces it as a `BoardChanged`, which the app answers
    /// with a whole board reload — so a test that wants the window *before* that reload simply
    /// does not call this.
    fn record_run(state: &mut AppState, delegation: &Delegation, outcome: Option<RunOutcome>) {
        let view = state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"));
        let card = &mut view.cards[0];
        card.runs.push(CardRun {
            id: delegation.id,
            thread_id: Some(delegation.child),
            status_id: card.status_id.clone(),
            action: ActionKind::Prompt,
            provider: AgentKind::Codex,
            model: None,
            effort: None,
            started_at: NOW.to_owned(),
            ended_at: outcome.map(|_| NOW.to_owned()),
            outcome,
            detail: None,
            report_comment_id: None,
            files_changed: 0,
            cost_usd: None,
            tokens: None,
        });
    }

    /// A card-called child the mirror can hold, with nothing on the card yet.
    fn child(caller: DelegationCaller) -> Delegation {
        let (id, child) = (DelegationId::new(), ThreadId::new());
        Delegation {
            id,
            caller,
            caller_turn: None,
            caller_item: None,
            child,
            provider: AgentKind::Codex,
            depth: 1,
            brief: "implement the card".to_owned(),
            expectation: "the tests pass".to_owned(),
            eager: false,
            status: DelegationStatus::Running,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: chrono::DateTime::UNIX_EPOCH,
            finished: None,
            headline: None,
            usage: None,
        }
    }

    /// The caller a column's automation writes on the child it starts.
    fn card_caller(state: &AppState) -> DelegationCaller {
        let view = state.board().unwrap_or_else(|| panic!("no board"));
        DelegationCaller::Card {
            board: view.board.id.clone(),
            card: view.cards[0].id.clone(),
        }
    }

    /// `]` asks once, with the column it would move to and the age of the child it would stop.
    ///
    /// Contracts §5.5: both halves have to hold. A card with no run moves silently, and so does
    /// a card whose run the mirror knows nothing live about — the daemon's `Conflict` is what
    /// speaks then, not a dialog the app invented.
    #[test]
    fn a_move_asks_only_when_a_live_child_is_behind_the_cards_run() {
        let now = chrono::DateTime::UNIX_EPOCH + chrono::Duration::minutes(2);
        let mut state = board();
        let target = state.board.focus.column + 1;
        assert!(
            move_cancels_run(&state, target, now).is_none(),
            "a card with no run moves with no confirm"
        );

        let caller = card_caller(&state);
        let delegation = live_run(&mut state, caller);
        assert!(
            move_cancels_run(&state, target, now).is_none(),
            "a run the delegation mirror has never heard of is not a child to stop"
        );

        state.agents.apply_delegation(delegation.clone());
        let column = next_column_name(&state);
        match move_cancels_run(&state, target, now) {
            Some(ConfirmRequest::MoveCancelsRun {
                key,
                target,
                elapsed,
                ..
            }) => {
                assert_eq!(key, "FLE-1");
                assert_eq!(target, column);
                assert_eq!(elapsed, "2m");
            }
            other => panic!("a live card-called child must raise the confirm, got {other:?}"),
        }
    }

    /// The mirror is what the confirm asks, not the card's own `runs` row.
    ///
    /// Regression (`scenarios/board/workflow-chain.scenario`): the tile is painted from the
    /// delegation mirror, so `needs you` reaches the face the moment the child opens its gate —
    /// milliseconds after the daemon started it, and a whole `EnsureWorktreeBoard` before the
    /// card carries the run. A `[` pressed in that window used to gate on `card.runs`, send a
    /// plain move and collect the daemon's `Conflict` in the sticky slot. Contracts §5.5: the
    /// confirm and the face read the same mirror, joined on the delegation's own caller.
    #[test]
    fn the_move_confirm_is_asked_when_the_mirror_holds_a_live_child_for_the_card() {
        let now = chrono::DateTime::UNIX_EPOCH + chrono::Duration::minutes(2);
        let mut state = board();
        let target = state.board.focus.column + 1;
        let mut delegation = child(card_caller(&state));
        delegation.status = DelegationStatus::Blocked;
        state.agents.apply_delegation(delegation);
        assert!(
            state.board().unwrap_or_else(|| panic!("no board")).cards[0]
                .runs
                .is_empty(),
            "the board reload that records the run has not landed yet"
        );

        let column = next_column_name(&state);
        match move_cancels_run(&state, target, now) {
            Some(ConfirmRequest::MoveCancelsRun {
                key,
                target,
                elapsed,
                ..
            }) => {
                assert_eq!(key, "FLE-1");
                assert_eq!(target, column);
                assert_eq!(elapsed, "2m");
            }
            other => panic!("a live child the mirror holds must raise the confirm, got {other:?}"),
        }
        // `X` reads the same fact, so the two surfaces cannot disagree about one card.
        let id = state.board().unwrap_or_else(|| panic!("no board")).cards[0]
            .id
            .clone();
        assert!(
            card_run_cancel_draft(&state, &id, now).is_ok(),
            "the card the confirm calls working has a run for `X` to stop"
        );
    }

    /// A run the card has already ended is nothing to ask about, mirror row or not.
    #[test]
    fn a_card_whose_run_has_ended_moves_with_no_confirm() {
        let now = chrono::DateTime::UNIX_EPOCH + chrono::Duration::minutes(2);
        let mut state = board();
        let target = state.board.focus.column + 1;
        let delegation = child(card_caller(&state));
        record_run(&mut state, &delegation, Some(RunOutcome::Succeeded));
        assert!(
            move_cancels_run(&state, target, now).is_none(),
            "a terminal run the mirror has forgotten moves with no question"
        );

        // The card outranks a mirror row that has not caught up with its own terminal event,
        // exactly as the mark fold treats it (`state/board.rs`).
        state.agents.apply_delegation(delegation);
        assert!(
            move_cancels_run(&state, target, now).is_none(),
            "the card is the authority on a run it has already ended"
        );
    }

    /// A child a *thread* called is not this card's run, however its id lines up.
    #[test]
    fn a_thread_called_child_is_not_a_card_run_to_confirm() {
        let now = chrono::DateTime::UNIX_EPOCH + chrono::Duration::minutes(2);
        let mut state = board();
        let target = state.board.focus.column + 1;
        let delegation = live_run(&mut state, DelegationCaller::Thread(ThreadId::new()));
        state.agents.apply_delegation(delegation.clone());
        assert!(move_cancels_run(&state, target, now).is_none());

        let mut ended = delegation;
        ended.caller = card_caller(&state);
        ended.status = DelegationStatus::Succeeded;
        state.agents.apply_delegation(ended);
        assert!(
            move_cancels_run(&state, target, now).is_none(),
            "a child that has already stopped is nothing to cancel"
        );
    }

    /// `X` refuses in the daemon's own sentence, and names what it would stop when it can.
    #[test]
    fn x_states_what_stops_or_refuses_in_the_daemons_own_words() {
        let now = chrono::DateTime::UNIX_EPOCH + chrono::Duration::minutes(2);
        let mut state = board();
        let id = state.board().unwrap_or_else(|| panic!("no board")).cards[0]
            .id
            .clone();
        assert_eq!(
            card_run_cancel_draft(&state, &id, now),
            Err("FLE-1 has no live run".to_owned())
        );

        let caller = card_caller(&state);
        let delegation = live_run(&mut state, caller);
        let child = delegation.child;
        state.agents.apply_delegation(delegation);
        let (card, key, fact) = card_run_cancel_draft(&state, &id, now)
            .unwrap_or_else(|message| panic!("a live run is cancellable, got {message}"));
        assert_eq!(card, id);
        assert_eq!(key, "FLE-1");
        assert_eq!(
            fact,
            format!("child {child} stops after 2m and reports no further work")
        );
    }

    /// An owed run is cancellable too: `X` drops the slot the card is queued for.
    #[test]
    fn a_card_waiting_for_a_slot_has_a_run_to_drop() {
        let now = chrono::DateTime::UNIX_EPOCH;
        let mut state = board();
        let view = state
            .board
            .view
            .as_mut()
            .unwrap_or_else(|| panic!("no board"));
        let status = view.cards[0].status_id.clone();
        view.cards[0].pending_run = Some(PendingRun {
            status_id: status,
            since: NOW.to_owned(),
        });
        let id = view.cards[0].id.clone();
        let (_, key, fact) = card_run_cancel_draft(&state, &id, now)
            .unwrap_or_else(|message| panic!("an owed run is cancellable, got {message}"));
        assert_eq!(key, "FLE-1");
        assert_eq!(
            fact,
            "FLE-1 is waiting for a free slot; the run it is owed is dropped"
        );
    }
}
