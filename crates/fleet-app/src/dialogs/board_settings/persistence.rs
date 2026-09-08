use super::*;

/// Loads the open board into the draft, including its backend's own settings rows.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let app = state.read(cx);
    let draft = app
        .board()
        .map(|view| {
            let kind = view.board.backend.kind.clone();
            let settings = view.board.backend.settings.clone();
            let mut draft = BoardSettingsState {
                board_id: Some(view.board.id.clone()),
                name: view.board.name.clone(),
                prefix: view.board.prefix.clone(),
                default_repo_id: view.board.default_repo_id.clone(),
                start_on_worktree: view.board.settings.start_on_worktree,
                push_new_cards: view.board.settings.push_new_cards,
                conflict_policy: view.board.settings.conflict_policy,
                backend_kind: kind.clone(),
                original_kind: kind.clone(),
                original_settings: settings,
                ..BoardSettingsState::default()
            };
            draft.rows = backend_rows(&schema_for(app, &kind), &draft.original_settings);
            draft
        })
        .unwrap_or_default();
    with_host(state, cx, |host| {
        let generation = host.board_settings.generation.wrapping_add(1);
        host.board_settings = BoardSettingsState {
            generation,
            ..draft
        };
    });
}

/// `Enter`: send one `UpdateBoard` with everything the dialog changed.
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
    let patch = BoardPatch {
        name: Some(draft.name.trim().to_owned()),
        prefix: Some(draft.prefix.trim().to_owned()),
        default_repo_id: Some(draft.default_repo_id.clone()),
        settings: Some(BoardSettings {
            start_on_worktree: draft.start_on_worktree,
            push_new_cards: draft.push_new_cards,
            conflict_policy: draft.conflict_policy,
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
