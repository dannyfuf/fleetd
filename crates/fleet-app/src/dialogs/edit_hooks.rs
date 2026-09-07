//! Editor for repository prepare and post-create hook commands.

use fleet_core::{ids::RepoId, model::RepoHooks};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Dialog, Icon, KeyHintRow, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{DialogHost, field, notify, root, typed_char, with_host},
    state::AppState,
};

#[derive(Debug, Clone, Default)]
pub struct EditHooksState {
    repo: Option<RepoId>,
    prepare: TextFieldState,
    post_create: TextFieldState,
    field: usize,
}

pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let repo = with_host(state, cx, |host| host.pending_hooks_repo.take());
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
    with_host(state, cx, |host| {
        host.edit_hooks = source.map_or_else(EditHooksState::default, |source| EditHooksState {
            repo: Some(source.id),
            prepare: TextFieldState::from_text(source.hooks.prepare.join("; ")),
            post_create: TextFieldState::from_text(source.hooks.post_create.join("; ")),
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

fn edit_input(state: &mut EditHooksState) -> &mut TextFieldState {
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
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let draft = &host.read(cx).edit_hooks;
    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    super::input::actions(
        root(focus),
        state,
        |host| edit_input(&mut host.edit_hooks),
        notify,
    )
    .on_key_down({
        let state = state.clone();
        move |event, _window, cx| {
            if let Some(text) = typed_char(event) {
                with_host(&state, cx, |host| {
                    edit_input(&mut host.edit_hooks).insert(text)
                });
                notify(&state, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::NextField, _window, cx| {
            with_host(&state, cx, |host| {
                host.edit_hooks.field = (host.edit_hooks.field + 1) % 2
            });
            notify(&state, cx);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::PrevField, _window, cx| {
            with_host(&state, cx, |host| {
                host.edit_hooks.field = (host.edit_hooks.field + 1) % 2
            });
            notify(&state, cx);
        }
    })
    .on_action(move |_: &dialog::Confirm, _window, cx| {
        let draft = with_host(&confirm_state, cx, |host| host.edit_hooks.clone());
        let Some(repo) = draft.repo else { return };
        confirm_bridge.send(RequestBody::SetRepoHooks {
            repo,
            hooks: RepoHooks {
                prepare: commands(draft.prepare.text()),
                post_create: commands(draft.post_create.text()),
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
                        field(&draft.prepare)
                            .label("Prepare commands (; separated)")
                            .focused(draft.field == 0),
                    )
                    .child(
                        field(&draft.post_create)
                            .label("Post-create commands (; separated)")
                            .focused(draft.field == 1),
                    ),
            )
            .hint_row(KeyHintRow::new().key("tab", "next").key("esc", "cancel"))
            .primary("enter  save"),
    )
    .into_any_element()
}
