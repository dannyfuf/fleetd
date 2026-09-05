//! Editor for repository prepare and post-create hook commands.

use fleet_core::{ids::RepoId, model::RepoHooks};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Dialog, Icon, KeyHintRow, TextField, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{TextInput, notify, root, typed_char, with_host},
    state::AppState,
};

#[derive(Debug, Clone, Default)]
pub struct EditHooksState {
    repo: Option<RepoId>,
    prepare: TextInput,
    post_create: TextInput,
    field: usize,
}

pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let repo = with_host(cx, |host| host.pending_hooks_repo.take());
    let source = repo.as_ref().and_then(|id| {
        state
            .read(cx)
            .snapshot
            .as_ref()?
            .repos
            .iter()
            .find(|repo| &repo.id == id)
            .cloned()
    });
    with_host(cx, |host| {
        host.edit_hooks = source.map_or_else(EditHooksState::default, |source| EditHooksState {
            repo: Some(source.id),
            prepare: TextInput::new(source.hooks.prepare.join("; ")),
            post_create: TextInput::new(source.hooks.post_create.join("; ")),
            field: 0,
        });
    });
}

fn commands(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .collect()
}

fn edit_input(state: &mut EditHooksState) -> &mut TextInput {
    if state.field == 0 {
        &mut state.prepare
    } else {
        &mut state.post_create
    }
}

pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let draft = with_host(cx, |host| host.edit_hooks.clone());
    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                if let Some(text) = typed_char(event) {
                    with_host(cx, |host| edit_input(&mut host.edit_hooks).insert(&text));
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| {
                with_host(cx, |host| {
                    host.edit_hooks.field = (host.edit_hooks.field + 1) % 2
                });
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| {
                with_host(cx, |host| {
                    host.edit_hooks.field = (host.edit_hooks.field + 1) % 2
                });
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                with_host(cx, |host| edit_input(&mut host.edit_hooks).backspace());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                with_host(cx, |host| edit_input(&mut host.edit_hooks).delete_word());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                with_host(cx, |host| edit_input(&mut host.edit_hooks).clear());
                notify(&state, cx);
            }
        })
        // §KEYMAP "Dialogs and text inputs": every text input answers `ctrl-a` / `ctrl-e` and
        // `←` / `→`. The bindings live on the shared `Dialog` context, so a dialog without
        // these listeners swallows the keys instead of moving the caret.
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                with_host(cx, |host| edit_input(&mut host.edit_hooks).home());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                with_host(cx, |host| edit_input(&mut host.edit_hooks).end());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                with_host(cx, |host| edit_input(&mut host.edit_hooks).left());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                with_host(cx, |host| edit_input(&mut host.edit_hooks).right());
                notify(&state, cx);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            let draft = with_host(cx, |host| host.edit_hooks.clone());
            let Some(repo) = draft.repo else { return };
            confirm_bridge.send(RequestBody::SetRepoHooks {
                repo,
                hooks: RepoHooks {
                    prepare: commands(draft.prepare.value()),
                    post_create: commands(draft.post_create.value()),
                },
            });
            confirm_state.update(cx, |app, cx| {
                app.close_overlay();
                cx.notify();
            });
        })
        .child(
            Dialog::new("Repository hooks")
                .icon(Icon::FilePen)
                .body(
                    div()
                        .flex()
                        .flex_col()
                        .gap(cx.theme().space.md)
                        .child(
                            TextField::new(draft.prepare.value().to_owned())
                                .label("Prepare commands (; separated)")
                                .caret(draft.prepare.caret())
                                .focused(draft.field == 0),
                        )
                        .child(
                            TextField::new(draft.post_create.value().to_owned())
                                .label("Post-create commands (; separated)")
                                .caret(draft.post_create.caret())
                                .focused(draft.field == 1),
                        ),
                )
                .hint_row(KeyHintRow::new().key("tab", "next").key("esc", "cancel"))
                .primary("enter  save"),
        )
        .into_any_element()
}
