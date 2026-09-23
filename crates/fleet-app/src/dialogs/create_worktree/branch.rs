//! The live branch editor of §3.8.1.
//!
//! The editor is the only place the branch text lives while the dialog is open;
//! `CreateState.branch` is the mirror its `Changed` event keeps, and everything the card
//! derives from it — the validation message and the worktree-id preview — is published back
//! onto the editor from here. None of it is computed inside `render`.

use super::*;

/// Creates the branch editor and wires its mirror. The dialog's seeding owns the draft.
pub(super) fn seed_input(state: &Entity<AppState>, cx: &mut App) {
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_label(Some("Branch".into()), cx);
        input.set_placeholder("feat/rut-validator", cx);
        input.set_mono(true, cx);
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, _| host.create_branch = Some(input.clone()));
    let weak_state = state.downgrade();
    let subscription = cx.subscribe(&input, move |input, event, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        let typed = input.read(cx).text().to_owned();
        with_host(&state, cx, |host| {
            host.create.branch = typed;
            // A refused create is about the branch that was sent, and the base list is
            // filtered by what is typed, so both start again from this keystroke.
            host.create.error = None;
            host.create.base_cursor = 0;
        });
        refresh_status(&state, cx);
        notify(&state, cx);
    });
    host.update(cx, |host, _| {
        host.create_branch_subscription = Some(subscription)
    });
    refresh_status(state, cx);
}

/// Publishes the first of validity, collision and id preview that applies (§3.8.1).
///
/// The collision is read from the snapshot, so this also runs when a snapshot lands under the
/// open dialog: a worktree created elsewhere must turn `Create` into `Open` here too.
pub(crate) fn refresh_status(state: &Entity<AppState>, cx: &mut App) {
    let Some(input) = read_host(state, cx, |host, _| host.create_branch.clone()) else {
        return;
    };
    let (invalid, preview) = {
        let duplicate = read_host(state, cx, |host, cx| {
            host.create
                .preview_id()
                .filter(|id| existing_worktree(state.read(cx), id).is_some())
        });
        read_host(state, cx, |host, _| {
            let draft = &host.create;
            let invalid = draft.branch_error();
            let preview = match (&invalid, duplicate, draft.preview_id()) {
                (Some(_), _, _) => None,
                (None, Some(id), _) => {
                    Some(format!("{id} already exists \u{2014} Open it \u{23ce}"))
                }
                (None, None, Some(id)) => Some(format!("Creates {id}")),
                (None, None, None) => None,
            };
            (invalid, preview)
        })
    };
    // `Changed` is emitted from inside the editor's own update, so the status it derives is
    // published on the next update turn rather than re-entering the entity that emitted it.
    cx.defer(move |cx| {
        input.update(cx, |input, cx| {
            input.set_invalid(invalid.map(Into::into), cx);
            input.set_preview(preview.map(Into::into), cx);
        });
    });
}
