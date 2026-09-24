use super::*;

/// `Enter`: a save inside the title, otherwise the picker or worktree the selected row points at.
pub(super) fn enter(
    state: &Entity<AppState>,
    bridge: &Bridge,
    targets: &[detail::PropertyRow],
    cx: &mut App,
) {
    match read_host(state, cx, |host, _| host.card_detail.edit) {
        Some(CardEdit::Title) => {
            commit_edit(state, bridge, cx);
            return;
        }
        Some(CardEdit::Description | CardEdit::Comment) => return,
        None => {}
    }
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    if let Enter::Unfold(folded) = enter_target(card(state.read(cx), &draft), &draft) {
        with_host(state, cx, |host| {
            host.card_detail.expanded_reports.extend(folded);
        });
        notify(state, cx);
        cx.stop_propagation();
        return;
    }
    let row = read_host(state, cx, |host, _| host.card_detail.property_row);
    open_row(state, bridge, targets, row, cx);
}

/// Selects property row `row` and does what it is for: the picker that edits it, the worktree
/// or issue it links, or the sentence a locked row answers with.
///
/// `⏎` on the selected row, a click on any row and the board's field keys (`s`, `p`, `o`, …)
/// all end here, so a row answers the pointer exactly as it answers the keyboard.
pub(super) fn open_row(
    state: &Entity<AppState>,
    bridge: &Bridge,
    targets: &[detail::PropertyRow],
    row: usize,
    cx: &mut App,
) {
    cx.stop_propagation();
    let Some(selected) = targets.get(row) else {
        return;
    };
    with_host(state, cx, |host| host.card_detail.property_row = row);
    notify(state, cx);
    match &selected.target {
        PropertyTarget::Pick(kind) => {
            // The board applies this gate before opening the same picker (§3.12 C): without it
            // the row opens a dialog whose Enter can only fail, instead of flashing the banner.
            if board::refuses(state, cx) {
                return;
            }
            // A field the backend owns is refused here rather than by the daemon two dialogs
            // later, on the sheet's own error line — the place every other refusal here lands.
            if let Some(message) = board::readonly_message(state.read(cx), kind) {
                with_host(state, cx, |host| host.card_detail.error = Some(message));
                notify(state, cx);
                return;
            }
            let kind = kind.clone();
            crate::dialogs::with_host(state, cx, |host| {
                host.card_picker.kind = kind;
                host.card_picker.then_worktree = false;
            });
            board::open_dialog(state, Dialogs::CardPicker, cx);
        }
        PropertyTarget::Worktree => {
            let draft = read_host(state, cx, |host, _| host.card_detail.clone());
            let Some((id, card_id)) = card(state.read(cx), &draft)
                .and_then(|card| Some((card.worktree_id.clone()?, card.id.clone())))
            else {
                return;
            };
            board::open_session(id, board::Refusal::CardDetail(card_id), state, bridge, cx);
        }
        PropertyTarget::Remote => open_remote(state, bridge, cx),
        PropertyTarget::PullRequest => board::open_pull_request(state, bridge, cx),
        PropertyTarget::ReadOnly => {
            // A locked row is a backend-declared property the remote owns; it answers with the
            // same sentence the standard read-only rows do, because two rows that carry the
            // same lock glyph must not answer `Enter` differently. Every other read-only row
            // (URL, Synced, Parent) states a fact with nothing to say about editing.
            if selected.locked {
                let backend = state.read(cx).board().map_or_else(String::new, |view| {
                    state.read(cx).backend_label(&view.board.backend.kind)
                });
                let message = format!("{} is read-only on {backend} boards", selected.label);
                with_host(state, cx, |host| host.card_detail.error = Some(message));
                notify(state, cx);
            }
        }
    }
}

/// `w`: create the card's worktree, asking for a repository first when nothing knows one.
pub(super) fn start_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    cx.stop_propagation();
    if board::refuses(state, cx) {
        return;
    }
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let Some(card) = card(state.read(cx), &draft).cloned() else {
        return;
    };
    let default_repo = state
        .read(cx)
        .board()
        .and_then(|view| view.board.default_repo_id.clone());
    if card.repo_id.is_none() && default_repo.is_none() {
        // The same guard the board surface applies to the same key: with no repository in the
        // context the picker's only row is the "No repository" clear row, whose Enter answers
        // "Pick a repository" forever — a dialog with no way to succeed.
        if !board::has_repo_in_context(state.read(cx)) {
            with_host(state, cx, |host| {
                host.card_detail.error = Some(board::NO_REPO_IN_CONTEXT.to_owned());
            });
            notify(state, cx);
            return;
        }
        with_host(state, cx, |host| {
            host.card_picker.kind = crate::dialogs::card_picker::PickerKind::Repo;
            host.card_picker.then_worktree = true;
        });
        board::open_dialog(state, Dialogs::CardPicker, cx);
        return;
    }
    let refusal = board::Refusal::CardDetail(card.id.clone());
    board::request_worktree_reporting(card.id, None, state, bridge, refusal, cx);
    cx.stop_propagation();
}

/// `esc` — discard an open edit, else close the dialog.
pub(crate) fn close(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) -> bool {
    let editing = with_host(state, cx, |host| {
        let editing = host.card_detail.is_editing();
        if editing {
            host.card_detail.cancel();
            host.card_detail_input = None;
            host.card_detail_input_subscription = None;
        }
        editing
    });
    if editing {
        notify(state, cx);
        return true;
    }
    state.update(cx, |state, cx| {
        state.close_overlay();
        cx.notify();
    });
    false
}

/// `i` — edit the title.
pub(crate) fn edit_title(
    state: &Entity<AppState>,
    _bridge: &Bridge,
    window: &mut Window,
    cx: &mut App,
) {
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let Some(title) = card(state.read(cx), &draft).map(|card| card.title.clone()) else {
        return;
    };
    begin(state, CardEdit::Title, title, window, cx);
}

/// `d` — edit the description.
pub(crate) fn edit_description(
    state: &Entity<AppState>,
    _bridge: &Bridge,
    window: &mut Window,
    cx: &mut App,
) {
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let Some(description) = card(state.read(cx), &draft).map(|card| card.description.clone())
    else {
        return;
    };
    begin(state, CardEdit::Description, description, window, cx);
}

/// `c` — write a comment.
pub(crate) fn add_comment(
    state: &Entity<AppState>,
    _bridge: &Bridge,
    window: &mut Window,
    cx: &mut App,
) {
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    if card(state.read(cx), &draft).is_none() {
        // The dialog is rendering `missing()`; an edit opened over it takes the keyboard and
        // the first `Esc` only cancels a buffer nothing is showing.
        return;
    }
    begin(state, CardEdit::Comment, String::new(), window, cx);
}

/// `j` — next property row.
pub(crate) fn next_property(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let len = property_count(state, cx);
    move_row(state, 1, len, cx);
}

/// `k` — previous property row.
pub(crate) fn prev_property(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let len = property_count(state, cx);
    move_row(state, -1, len, cx);
}

/// `Enter` — edit the selected property.
pub(crate) fn edit_property(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let targets = property_targets(state, cx);
    enter(state, bridge, &targets, cx);
}

/// `w` — create the card's worktree.
pub(crate) fn create_worktree(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    start_worktree(state, bridge, cx);
}

/// `x` — open this card's remote issue in the browser.
///
/// The sheet stays open: the browser is another window, and closing the card the user is
/// reading to show it somewhere else loses the place they were in.
pub(crate) fn open_remote(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let Some(card) = card(state.read(cx), &draft) else {
        return;
    };
    let Some(url) = board::remote_url(card) else {
        // The sentence goes on the sheet itself, where every
        // other refusal on this surface goes — and it is the same sentence the board screen's
        // `x` answers with: a linked card whose board has no address for its backend is a
        // different fact from a card with no remote issue at all, and only one of them names
        // the setting that fixes it.
        let refusal = board::no_remote_reason(card);
        with_host(state, cx, |host| {
            host.card_detail.error = Some(refusal.to_owned());
        });
        notify(state, cx);
        return;
    };
    cx.open_url(&url);
}

/// `K` — resolve the conflict by keeping the local card.
pub(crate) fn keep_local(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    resolve(state, bridge, ConflictResolution::KeepLocal, cx);
}

/// `R` — resolve the conflict by taking the remote card.
pub(crate) fn take_remote(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    resolve(state, bridge, ConflictResolution::TakeRemote, cx);
}

/// `ctrl-s` — save the open text edit.
pub(crate) fn save(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    commit_edit(state, bridge, cx);
}

/// How many property rows the shown card has.
pub(super) fn property_count(state: &Entity<AppState>, cx: &mut App) -> usize {
    property_targets(state, cx).len()
}

/// The property rows of the shown card, for the palette-driven entry points.
pub(super) fn property_targets(state: &Entity<AppState>, cx: &mut App) -> Vec<detail::PropertyRow> {
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let app = state.read(cx);
    let (Some(view), Some(card)) = (app.board(), card(app, &draft)) else {
        return Vec::new();
    };
    detail::property_rows(&view.board, &view.cards, card, now_unix())
}
