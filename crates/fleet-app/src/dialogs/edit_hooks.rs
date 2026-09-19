//! Editor for repository prepare and post-create hook commands.
//!
//! Every row is a live [`TextInput`] owned by the [`DialogHost`]: the prepare commands first,
//! the post-create commands after them, and one blank row at the end of each list. Typing into
//! a trailing blank row appends the next one, so the list grows as it is filled and a save
//! simply drops whatever stayed empty.

use fleet_core::{ids::RepoId, model::RepoHooks};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Dialog, Icon, KeyHintRow, prelude::*};
use gpui::{AnyElement, App, AppContext, Entity, FocusHandle, Window, div};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{DialogHost, notify, read_host, root, with_host},
    state::AppState,
};

#[derive(Debug, Clone)]
pub struct EditHooksState {
    repo: Option<RepoId>,
    /// How many of `DialogHost.hook_inputs` are prepare commands; the rest are post-create.
    pub(super) prepare_len: usize,
    pub(super) field: usize,
}

impl Default for EditHooksState {
    fn default() -> Self {
        Self {
            repo: None,
            prepare_len: 1,
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
    let (repo, prepare, post_create) = source.map_or_else(
        || (None, Vec::new(), Vec::new()),
        |source| {
            (
                Some(source.id),
                source.hooks.prepare,
                source.hooks.post_create,
            )
        },
    );
    // Each list always ends in a blank row, which is where the next command is typed.
    let prepare_len = prepare.len() + 1;
    let inputs: Vec<Entity<TextInput>> = prepare
        .into_iter()
        .chain(std::iter::once(String::new()))
        .chain(post_create)
        .chain(std::iter::once(String::new()))
        .map(|command| new_row(command, cx))
        .collect();
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, _| {
        host.edit_hooks = EditHooksState {
            repo,
            prepare_len,
            field: 0,
        };
        host.hook_inputs = inputs.clone();
    });
    let subscriptions = inputs
        .iter()
        .map(|input| watch_row(state, input, cx))
        .collect();
    host.update(cx, |host, _| host.hook_input_subscriptions = subscriptions);
    relabel(state, cx);
}

/// One command row, seeded with the command it already holds.
fn new_row(command: String, cx: &mut App) -> Entity<TextInput> {
    cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_text(command, cx);
        input
    })
}

/// Grows the list when the row that was typed into is the trailing blank of its section.
fn watch_row(
    state: &Entity<AppState>,
    input: &Entity<TextInput>,
    cx: &mut App,
) -> gpui::Subscription {
    let weak_state = state.downgrade();
    cx.subscribe(input, move |_, event, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        append_blank_rows(&state, cx);
    })
}

/// Restores the "one trailing blank row per section" rule after an edit.
fn append_blank_rows(state: &Entity<AppState>, cx: &mut App) {
    let (prepare_len, texts) = read_host(state, cx, |host, cx| {
        (
            host.edit_hooks.prepare_len,
            host.hook_inputs
                .iter()
                .map(|input| input.read(cx).text().to_owned())
                .collect::<Vec<_>>(),
        )
    });
    let prepare_full = prepare_len > 0 && !texts[prepare_len - 1].trim().is_empty();
    let post_full = texts.last().is_some_and(|last| !last.trim().is_empty());
    if !prepare_full && !post_full {
        return;
    }
    if prepare_full {
        let row = new_row(String::new(), cx);
        let subscription = watch_row(state, &row, cx);
        with_host(state, cx, |host| {
            host.hook_inputs.insert(prepare_len, row);
            host.hook_input_subscriptions.push(subscription);
            host.edit_hooks.prepare_len += 1;
            if host.edit_hooks.field >= prepare_len {
                host.edit_hooks.field += 1;
            }
        });
    }
    if post_full {
        let row = new_row(String::new(), cx);
        let subscription = watch_row(state, &row, cx);
        with_host(state, cx, |host| {
            host.hook_inputs.push(row);
            host.hook_input_subscriptions.push(subscription);
        });
    }
    relabel(state, cx);
    notify(state, cx);
}

/// Numbers every row from its position, so an inserted row renumbers the ones under it.
fn relabel(state: &Entity<AppState>, cx: &mut App) {
    let (prepare_len, inputs) = read_host(state, cx, |host, _| {
        (host.edit_hooks.prepare_len, host.hook_inputs.clone())
    });
    for (index, input) in inputs.into_iter().enumerate() {
        let label = if index < prepare_len {
            format!("Prepare command {}", index + 1)
        } else {
            format!("Post-create command {}", index - prepare_len + 1)
        };
        input.update(cx, |input, cx| input.set_label(Some(label.into()), cx));
    }
}

/// The commands one section would save: every non-empty row, in order.
fn commands(texts: &[String]) -> Vec<String> {
    texts
        .iter()
        .map(|command| command.trim_end_matches('\n'))
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let inputs = host.read(cx).hook_inputs.clone();
    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    root(focus)
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, window, cx| move_field(&state, 1, window, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, window, cx| move_field(&state, -1, window, cx)
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            let (repo, prepare_len, texts) = read_host(&confirm_state, cx, |host, cx| {
                (
                    host.edit_hooks.repo.clone(),
                    host.edit_hooks.prepare_len,
                    host.hook_inputs
                        .iter()
                        .map(|input| input.read(cx).text().to_owned())
                        .collect::<Vec<_>>(),
                )
            });
            let Some(repo) = repo else { return };
            let (prepare, post_create) = texts.split_at(prepare_len.min(texts.len()));
            confirm_bridge.send(RequestBody::SetRepoHooks {
                repo,
                hooks: RepoHooks {
                    prepare: commands(prepare),
                    post_create: commands(post_create),
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
                    div().flex().flex_col().gap(cx.theme().space.md).children(
                        inputs.into_iter().enumerate().map(|(index, input)| {
                            input.harness_target_indexed("dialog.field", index)
                        }),
                    ),
                )
                .hint_row(KeyHintRow::new().key("tab", "next").key("esc", "cancel"))
                .primary("enter  save"),
        )
        .into_any_element()
}

/// `Tab` / `S-Tab`: hand the keyboard to the next row, wrapping at both ends.
fn move_field(state: &Entity<AppState>, delta: isize, window: &mut Window, cx: &mut App) {
    let input = with_host(state, cx, |host| {
        let count = host.hook_inputs.len();
        host.edit_hooks.field = crate::dialogs::step(host.edit_hooks.field, delta, count);
        host.hook_inputs.get(host.edit_hooks.field).cloned()
    });
    if let Some(input) = input {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    notify(state, cx);
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
        let rows: Vec<String> = original
            .iter()
            .cloned()
            .chain(std::iter::once(String::new()))
            .collect();
        assert_eq!(commands(&rows), original);
    }

    #[gpui::test]
    fn typing_into_the_trailing_blank_row_appends_the_next_one(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new("/tmp/hooks", std::time::Instant::now()));
        cx.update(|cx| seed(&state, cx));
        let rows = cx.update(|cx| read_host(&state, cx, |host, _| host.hook_inputs.clone()));
        assert_eq!(
            rows.len(),
            2,
            "one blank prepare row and one blank post row"
        );

        cx.update(|cx| {
            rows[0].update(cx, |input, cx| input.set_text("bundle install", cx));
        });
        let (prepare_len, rows) = cx.update(|cx| {
            read_host(&state, cx, |host, _| {
                (host.edit_hooks.prepare_len, host.hook_inputs.clone())
            })
        });
        assert_eq!(prepare_len, 2, "the filled row grew a blank under it");
        assert_eq!(rows.len(), 3);
        cx.update(|cx| {
            assert_eq!(rows[1].read(cx).text(), "");
            assert_eq!(rows[2].read(cx).text(), "");
        });
    }
}
