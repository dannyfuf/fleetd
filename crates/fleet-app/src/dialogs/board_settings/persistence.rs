use super::*;

/// Loads the open board into the draft, including its backend's own settings rows.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let app = state.read(cx);
    let draft = app
        .board()
        .map(|view| {
            let kind = view.board.backend.kind.clone();
            let settings = view.board.backend.settings.clone();
            let columns: Vec<ColumnDraft> =
                view.board.statuses.iter().map(ColumnDraft::load).collect();
            let mut draft = BoardSettingsState {
                board_id: Some(view.board.id.clone()),
                name: view.board.name.clone(),
                prefix: view.board.prefix.clone(),
                default_repo_id: view.board.default_repo_id.clone(),
                start_on_worktree: view.board.settings.start_on_worktree,
                push_new_cards: view.board.settings.push_new_cards,
                conflict_policy: view.board.settings.conflict_policy,
                max_live_runs: view.board.settings.max_live_runs,
                board: Some(std::rc::Rc::new(view.board.clone())),
                original_columns: columns.clone(),
                columns,
                cards_by_column: std::rc::Rc::new(cards_by_column(view)),
                // §5.4: a context board has no checkout to run in, and a linked board's columns
                // answer to its backend. Both keep every other row.
                automation_locked: view.board.worktree_id.is_none()
                    || view.board.backend.kind != BackendRef::LOCAL,
                backend_kind: kind.clone(),
                original_kind: kind.clone(),
                original_settings: settings,
                ..BoardSettingsState::default()
            };
            draft.rows = backend_rows(&schema_for(app, &kind), &draft.original_settings);
            draft.prepare();
            draft
        })
        .unwrap_or_default();
    with_host(state, cx, |host| {
        let generation = host.board_settings.generation.wrapping_add(1);
        host.board_settings = BoardSettingsState {
            generation,
            ..draft
        };
        // A column's automation pill opens the dialog on that column, drilled in.
        if let Some(column) = host.board_settings_column.take()
            && column < host.board_settings.columns.len()
        {
            host.board_settings.section = BoardSection::Columns;
            host.board_settings.opened_column = Some(column);
            host.board_settings.row = 0;
            host.board_settings.prepare();
        }
    });
    materialize_input(state, None, None, cx);
}

/// The live cards each column holds, in board order.
///
/// Archived cards are left out: they are not on the canvas, the daemon does not refuse a
/// column for them, and moving them would write an activity entry about a card nobody sees.
#[must_use]
fn cards_by_column(view: &fleet_core::board::BoardView) -> Vec<(StatusId, Vec<CardId>)> {
    view.board
        .statuses
        .iter()
        .map(|status| {
            let cards = view
                .cards
                .iter()
                .filter(|card| !card.archived && card.status_id == status.id)
                .map(|card| card.id.clone())
                .collect();
            (status.id.clone(), cards)
        })
        .collect()
}

/// `^s`: send one `UpdateBoard` with everything the dialog changed.
pub(super) fn save(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let draft = read_host(state, cx, |host, _| host.board_settings.clone());
    // A save already in flight is answered by the reply it is waiting for; a board that changed
    // under the dialog is not answered by anything, and returning silently makes `Enter` a dead
    // key with nothing on the error line to read. `card_picker::apply` says so for the same case.
    if draft.saving {
        return;
    }
    if !state
        .read(cx)
        .board()
        .is_some_and(|view| Some(&view.board.id) == draft.board_id.as_ref())
    {
        with_host(state, cx, |host| {
            host.board_settings.error = Some("That board is no longer loaded".into());
        });
        notify(state, cx);
        return;
    }
    if let Some(message) = draft.validate() {
        with_host(state, cx, |host| host.board_settings.error = Some(message));
        notify(state, cx);
        return;
    }
    let Some(board_id) = draft.board_id.clone() else {
        return;
    };
    // The one setting this dialog does not show — the branch template — is carried through
    // unchanged, because `BoardPatch::settings` replaces the whole struct.
    let settings = state
        .read(cx)
        .board()
        .map_or_else(BoardSettings::default, |view| view.board.settings.clone());
    // Every column, every time: `BoardPatch.statuses` replaces the vector outright, so a patch
    // carrying only the ones this dialog touched would delete the rest. `fleet board columns`
    // sends the whole vector for the same reason.
    let statuses = draft
        .columns
        .iter()
        .map(ColumnDraft::status)
        .collect::<Result<Vec<_>, _>>();
    let statuses = match statuses {
        Ok(statuses) => statuses,
        Err(message) => {
            with_host(state, cx, |host| host.board_settings.error = Some(message));
            notify(state, cx);
            return;
        }
    };
    let patch = BoardPatch {
        name: Some(draft.name.trim().to_owned()),
        prefix: Some(draft.prefix.trim().to_owned()),
        default_repo_id: Some(draft.default_repo_id.clone()),
        statuses: Some(statuses),
        settings: Some(BoardSettings {
            start_on_worktree: draft.start_on_worktree,
            push_new_cards: draft.push_new_cards,
            conflict_policy: draft.conflict_policy,
            max_live_runs: draft.max_live_runs,
            ..settings
        }),
        // A backend the dialog did not change is left out of the patch entirely: `update`
        // resets the sync cursor for any backend it is handed, and the board's own settings
        // rows must not cost a re-pull.
        backend: Some(BackendRef {
            kind: draft.backend_kind.clone(),
            settings: draft.settings_json(),
        })
        .filter(|backend| {
            state
                .read(cx)
                .board()
                .is_none_or(|view| *backend != view.board.backend)
        }),
        ..BoardPatch::default()
    };

    with_host(state, cx, |host| {
        host.board_settings.saving = true;
        host.board_settings.error = None;
    });
    let generation = draft.generation;
    let reply = bridge.request(RequestBody::UpdateBoard {
        board_id: board_id.clone(),
        patch,
    });
    notify(state, cx);
    complete_request(state, cx, async move |state, cx| {
        let answer = match reply.recv().await {
            Ok(Ok(ResponseBody::Board(view))) => Ok(view),
            // The daemon's own words, verbatim: refusing a kind change on a linked board, or a
            // setting the backend cannot parse, is a sentence only the backend can write.
            Ok(Err(error)) => Err(error.message),
            Ok(Ok(_)) => Err("Unexpected settings response".into()),
            Err(_) => Err("Daemon disconnected before replying".into()),
        };
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            if state.read(cx).overlay != Some(crate::state::Overlay::Dialog(Dialogs::BoardSettings))
                || !state
                    .read(cx)
                    .board()
                    .is_some_and(|view| view.board.id == board_id)
            {
                return;
            }
            let completed = with_host(&state, cx, |host| {
                host.board_settings
                    .finish_save(generation, answer.as_ref().err().cloned())
            });
            if completed && let Ok(view) = answer {
                state.update(cx, |app, cx| {
                    app.apply_board_view(view);
                    app.close_overlay();
                    cx.notify();
                });
            }
            notify(&state, cx);
        });
    });
}

/// `\u{23ce}` with a delete armed: move the column's cards out, then drop it from the draft.
///
/// The moves go out one at a time, in column order, and the column only leaves the draft once
/// every one of them has been accepted. A refusal stops the sequence where it is: the column
/// stays, the daemon's sentence goes on the error line, and the cards already moved stay moved
/// — each of them was recorded as its own `Moved` activity entry, so nothing is silently
/// half-done (contracts §5.4).
pub(super) fn delete_with_cards(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some((doomed, target, cards, board_id, generation)) = read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        let doomed = draft.pending_delete?;
        let SettingRow::Column(target) = draft.focused() else {
            return None;
        };
        Some((
            doomed,
            target,
            draft.cards_in(doomed).to_vec(),
            draft.board_id.clone()?,
            draft.generation,
        ))
    }) else {
        return;
    };
    if target == doomed {
        with_host(state, cx, |host| {
            host.board_settings.error =
                Some("choose a different column for the cards to move to".to_owned());
        });
        notify(state, cx);
        return;
    }
    let Some(status_id) = read_host(state, cx, |host, _| {
        Some(host.board_settings.columns.get(target)?.status.id.clone())
    }) else {
        return;
    };
    with_host(state, cx, |host| {
        host.board_settings.saving = true;
        host.board_settings.error = None;
        host.board_settings.notice = None;
    });
    notify(state, cx);

    let bridge = bridge.clone();
    complete_request(state, cx, async move |state, cx| {
        let mut moved = 0_usize;
        let mut failure = None;
        for card_id in &cards {
            let reply = bridge.request(RequestBody::MoveCard {
                card_id: card_id.clone(),
                status_id: status_id.clone(),
                index: None,
                // A card with a live run refuses here in the daemon's own words rather than
                // being cancelled behind a delete: cancelling a run is `X`, and a column being
                // tidied away is no reason to end one.
                cancel_run: false,
            });
            match reply.recv().await {
                Ok(Ok(_)) => moved += 1,
                Ok(Err(error)) => {
                    failure = Some(error.message);
                    break;
                }
                Err(_) => {
                    failure = Some("Daemon disconnected before replying".into());
                    break;
                }
            }
        }
        let all_moved = moved == cards.len();
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            if state.read(cx).overlay != Some(crate::state::Overlay::Dialog(Dialogs::BoardSettings))
                || !state
                    .read(cx)
                    .board()
                    .is_some_and(|view| view.board.id == board_id)
            {
                return;
            }
            with_host(&state, cx, |host| {
                let draft = &mut host.board_settings;
                if draft.generation != generation || !draft.saving {
                    return;
                }
                draft.saving = false;
                draft.pending_delete = None;
                draft.error = failure;
                if all_moved && doomed < draft.columns.len() {
                    let id = draft.columns.remove(doomed).status.id;
                    std::rc::Rc::make_mut(&mut draft.cards_by_column)
                        .retain(|(status, _)| *status != id);
                    draft.row = draft.row.min(draft.columns.len().saturating_sub(1));
                }
                draft.prepare();
            });
            notify(&state, cx);
        });
    });
}
