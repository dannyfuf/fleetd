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

#[derive(Debug, Clone)]
pub struct EditHooksState {
    repo: Option<RepoId>,
    prepare: Vec<TextFieldState>,
    post_create: Vec<TextFieldState>,
    field: usize,
}

impl Default for EditHooksState {
    fn default() -> Self {
        Self {
            repo: None,
            prepare: vec![TextFieldState::default()],
            post_create: vec![TextFieldState::default()],
            field: 0,
        }
    }
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
            prepare: command_fields(source.hooks.prepare),
            post_create: command_fields(source.hooks.post_create),
            field: 0,
        });
    });
}

fn command_fields(commands: Vec<String>) -> Vec<TextFieldState> {
    commands
        .into_iter()
        .map(TextFieldState::from_text)
        .chain(std::iter::once(TextFieldState::default()))
        .collect()
}

fn commands(fields: &[TextFieldState]) -> Vec<String> {
    fields
        .iter()
        .map(TextFieldState::text)
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .collect()
}

fn edit_input(state: &mut EditHooksState) -> &mut TextFieldState {
    let prepare_len = state.prepare.len();
    if state.field < prepare_len {
        &mut state.prepare[state.field]
    } else {
        let index = state
            .field
            .saturating_sub(prepare_len)
            .min(state.post_create.len().saturating_sub(1));
        &mut state.post_create[index]
    }
}

fn append_blank_after_edit(state: &mut EditHooksState) {
    if state.prepare.last().is_some_and(|field| !field.is_empty()) {
        state.prepare.push(TextFieldState::default());
    }
    if state
        .post_create
        .last()
        .is_some_and(|field| !field.is_empty())
    {
        state.post_create.push(TextFieldState::default());
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
                    edit_input(&mut host.edit_hooks).insert(text);
                    append_blank_after_edit(&mut host.edit_hooks);
                });
                notify(&state, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::NextField, _window, cx| {
            with_host(&state, cx, |host| {
                let count = host.edit_hooks.prepare.len() + host.edit_hooks.post_create.len();
                host.edit_hooks.field = (host.edit_hooks.field + 1) % count.max(1);
            });
            notify(&state, cx);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::PrevField, _window, cx| {
            with_host(&state, cx, |host| {
                let count = host.edit_hooks.prepare.len() + host.edit_hooks.post_create.len();
                host.edit_hooks.field = host
                    .edit_hooks
                    .field
                    .checked_sub(1)
                    .unwrap_or_else(|| count.saturating_sub(1));
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
                prepare: commands(&draft.prepare),
                post_create: commands(&draft.post_create),
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
                    .children(draft.prepare.iter().enumerate().map(|(index, command)| {
                        field(command)
                            .label(format!("Prepare command {}", index + 1))
                            .focused(draft.field == index)
                    }))
                    .children(
                        draft
                            .post_create
                            .iter()
                            .enumerate()
                            .map(|(index, command)| {
                                field(command)
                                    .label(format!("Post-create command {}", index + 1))
                                    .focused(draft.field == draft.prepare.len() + index)
                            }),
                    ),
            )
            .hint_row(KeyHintRow::new().key("tab", "next").key("esc", "cancel"))
            .primary("enter  save"),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_semicolons_round_trip_losslessly() {
        let original = vec![
            "printf '%s; still one command' value".to_owned(),
            "if test -f Gemfile; then bundle install; fi".to_owned(),
        ];
        let fields = command_fields(original.clone());
        assert_eq!(commands(&fields), original);
    }
}
