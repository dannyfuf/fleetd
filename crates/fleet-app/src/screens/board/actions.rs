use super::*;

/// Says what a key needs before it can do anything.
///
/// §8 gives every board key an action. A key that neither acts nor says anything reads as a
/// broken app, and an empty board — or a filter that hides every card — is where that shows.
const NO_CARD: &str = "No card selected";
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
    open_dialog(state, Dialogs::CardCreate, cx);
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
pub(crate) fn move_prev_column(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    move_card(state, bridge, -1, cx);
}

/// `]` — move the card to the next column.
pub(crate) fn move_next_column(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    move_card(state, bridge, 1, cx);
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
