//! Editor for repository prepare and post-create hook commands (`docs/UX-SPEC.md` §3.8.10).
//!
//! Every row is a live [`TextInput`] owned by the [`DialogHost`]: the prepare commands first,
//! the post-create commands after them, and one blank row at the end of each list. Typing into
//! a trailing blank row appends the next one, so the list grows as it is filled and a save
//! simply drops whatever stayed empty.
//!
//! The dialog draws the two lists as two [`SettingsCard`]s of numbered [`SettingsRow`]s. Each
//! row's control is a full-width mono [`ValueBox`] holding that row's editor, embedded and
//! always live, so `Tab` walks every command and the row whose editor holds the keyboard is the
//! cursor row.

use fleet_core::{ids::RepoId, model::RepoHooks};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{
    ButtonSize, Dialog, Icon, IconButton, SettingsCard, SettingsRow, ValueBox, ValueBoxWidth,
    prelude::*, theme::ch,
};
use gpui::{AnyElement, App, AppContext, Entity, FocusHandle, SharedString, Window, div};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{DialogHost, Dialogs, footer, notify, read_host, root, with_host},
    state::AppState,
};

#[derive(Debug, Clone)]
pub struct EditHooksState {
    repo: Option<RepoId>,
    /// The repository id as the header subtitle shows it, formatted once at seed.
    subtitle: SharedString,
    /// How many of `DialogHost.hook_inputs` are prepare commands; the rest are post-create.
    pub(super) prepare_len: usize,
    pub(super) field: usize,
}

impl Default for EditHooksState {
    fn default() -> Self {
        Self {
            repo: None,
            subtitle: SharedString::default(),
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
            subtitle: repo
                .as_ref()
                .map(|repo| SharedString::from(repo.as_str().to_owned()))
                .unwrap_or_default(),
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

/// What the trailing blank row of each list says while it is empty.
const NEXT_COMMAND_PLACEHOLDER: &str = "Type the next command…";

/// One command row, seeded with the command it already holds.
///
/// The editor is embedded — the row's [`ValueBox`] is its chrome, so it draws no box and no
/// status line of its own — and shows the command in the data face.
fn new_row(command: String, cx: &mut App) -> Entity<TextInput> {
    cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_embedded(true, cx);
        input.set_hide_status_line(true, cx);
        input.set_mono(true, cx);
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
    cx.subscribe(input, move |input, event, cx| {
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        if matches!(event, TextInputEvent::Focused) {
            claim_row(&state, &input, cx);
            return;
        }
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        append_blank_rows(&state, cx);
    })
}

/// Mirrors the row that just took focus into the marker the shell reconciles against.
///
/// `dialogs::focused_input` names the editor from `field`, and a click focuses a row without
/// asking the dialog, so without this the next `AppState` notify would move the caret back.
/// Rows are inserted as the list grows, so the index is resolved at event time.
fn claim_row(state: &Entity<AppState>, input: &Entity<TextInput>, cx: &mut App) {
    let Some(changed) = with_host(state, cx, |host| {
        let index = host
            .hook_inputs
            .iter()
            .position(|row| row.entity_id() == input.entity_id())?;
        let changed = host.edit_hooks.field != index;
        host.edit_hooks.field = index;
        Some(changed)
    }) else {
        return;
    };
    if changed {
        notify(state, cx);
    }
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

/// Numbers every row from its position, so an inserted row renumbers the ones under it, and
/// gives the placeholder to the trailing blank row of each list only.
fn relabel(state: &Entity<AppState>, cx: &mut App) {
    let (prepare_len, inputs) = read_host(state, cx, |host, _| {
        (host.edit_hooks.prepare_len, host.hook_inputs.clone())
    });
    let len = inputs.len();
    for (index, input) in inputs.into_iter().enumerate() {
        let label = if index < prepare_len {
            format!("Prepare command {}", index + 1)
        } else {
            format!("Post-create command {}", index - prepare_len + 1)
        };
        let placeholder = if is_trailing_blank(prepare_len, len, index) {
            NEXT_COMMAND_PLACEHOLDER
        } else {
            ""
        };
        input.update(cx, |input, cx| {
            input.set_label(Some(label.into()), cx);
            input.set_placeholder(placeholder, cx);
        });
    }
}

/// Whether row `index` is the trailing blank of its list — the row the next command is typed
/// into. It is never removed, so its ✕ is drawn disabled.
fn is_trailing_blank(prepare_len: usize, len: usize, index: usize) -> bool {
    index + 1 == prepare_len || index + 1 >= len
}

/// The row whose editor holds the keyboard: the cursor row. `None` while focus is elsewhere.
fn cursor_row(inputs: &[Entity<TextInput>], window: &Window, cx: &App) -> Option<usize> {
    inputs
        .iter()
        .position(|input| input.read(cx).focus_handle().is_focused(window))
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
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (inputs, prepare_len, subtitle) = {
        let host = host.read(cx);
        (
            host.hook_inputs.clone(),
            host.edit_hooks.prepare_len.min(host.hook_inputs.len()),
            host.edit_hooks.subtitle.clone(),
        )
    };
    let cursor = cursor_row(&inputs, window, cx);
    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    let theme = cx.theme();
    let body = div()
        .flex()
        .flex_col()
        .gap(theme.space.md)
        .child(section(
            state,
            Section::Prepare,
            &inputs[..prepare_len],
            0,
            cursor,
        ))
        .child(section(
            state,
            Section::PostCreate,
            &inputs[prepare_len..],
            prepare_len,
            cursor,
        ))
        .child(
            div().px(theme.space.xs).child(
                Text::caption(
                    "Tab walks every command in order. A filled last row grows a blank one under it.",
                )
                .muted(),
            ),
        );
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
                .dismiss_action(Dialogs::EditHooks.dismiss_action())
                .icon(Icon::FilePen)
                .subtitle(subtitle)
                .width(Dialogs::EditHooks.width(cx))
                .body(body)
                .actions(vec![
                    footer::cancel(&Dialogs::EditHooks),
                    footer::primary("hooks-save", "Save", Box::new(dialog::Confirm)),
                ]),
        )
        .into_any_element()
}

/// Which of the two command lists a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Commands that prepare a fresh copy of the repository.
    Prepare,
    /// Commands that run after a worktree is created.
    PostCreate,
}

impl Section {
    const fn title(self) -> &'static str {
        match self {
            Self::Prepare => "Prepare",
            Self::PostCreate => "After a worktree is created",
        }
    }

    /// The sentence under the title: when the list runs, and what a failure marks.
    const fn subtitle(self) -> &'static str {
        match self {
            Self::Prepare => {
                "Runs on the prepared copy, once, before any worktree is made from it."
            }
            Self::PostCreate => {
                "Runs inside each new worktree, in the background. A failure marks the worktree, \
                 not the repository."
            }
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::Prepare => "hooks-prepare",
            Self::PostCreate => "hooks-post-create",
        }
    }
}

/// One command list: a card of numbered rows, each a full-width box holding the row's editor
/// and a remove ✕.
///
/// The last row of each list is always blank — it is where the next command is typed — so its
/// ✕ is drawn disabled. A click on a row hands the keyboard to its editor.
fn section(
    state: &Entity<AppState>,
    section: Section,
    rows: &[Entity<TextInput>],
    first: usize,
    cursor: Option<usize>,
) -> AnyElement {
    let last = rows.len().saturating_sub(1);
    SettingsCard::new(section.id())
        .title(section.title())
        .subtitle(section.subtitle())
        .rows(rows.iter().enumerate().map(|(offset, input)| {
            let index = first + offset;
            let remove_state = state.clone();
            let focus_state = state.clone();
            let number = SharedString::from((offset + 1).to_string());
            SettingsRow::new(("hooks-row", index))
                .leading(
                    div()
                        .w(ch(2.0))
                        .flex()
                        .justify_end()
                        .child(Text::caption(number).muted()),
                )
                .control(
                    ValueBox::new(("hooks-box", index), "")
                        .width(ValueBoxWidth::Fill)
                        .mono(true)
                        .editor(input.clone())
                        .harness_target_indexed("dialog.field", index),
                )
                .trailing(
                    IconButton::new((section.id(), index), Icon::X, "Remove command")
                        .size(ButtonSize::Compact)
                        .disabled(offset == last)
                        .on_click(move |_, _, cx| remove_row(&remove_state, index, cx))
                        .harness_target_indexed("hooks.remove", index),
                )
                .cursor(cursor == Some(index))
                .on_click(move |_, window, cx| focus_row(&focus_state, index, window, cx))
                .into_any_element()
        }))
        .into_any_element()
}

/// Hands the keyboard to row `index`: focuses its editor and moves the marker the shell
/// reconciles focus against, so the next notify keeps the caret there.
fn focus_row(state: &Entity<AppState>, index: usize, window: &mut Window, cx: &mut App) {
    let input = with_host(state, cx, |host| {
        let input = host.hook_inputs.get(index).cloned()?;
        host.edit_hooks.field = index;
        Some(input)
    });
    if let Some(input) = input {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    notify(state, cx);
}

/// Drops command row `index`. The trailing blank row of either list is never removed, so each
/// list keeps the row the next command is typed into.
fn remove_row(state: &Entity<AppState>, index: usize, cx: &mut App) {
    let removed = with_host(state, cx, |host| {
        let prepare_len = host.edit_hooks.prepare_len;
        let len = host.hook_inputs.len();
        if is_trailing_blank(prepare_len, len, index) {
            return false;
        }
        host.hook_inputs.remove(index);
        if index < prepare_len {
            host.edit_hooks.prepare_len -= 1;
        }
        let field = host.edit_hooks.field;
        if field > index {
            host.edit_hooks.field = field - 1;
        }
        true
    });
    if removed {
        relabel(state, cx);
        notify(state, cx);
    }
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

    #[gpui::test]
    fn a_removed_command_leaves_its_list_and_the_blank_rows_stay(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new("/tmp/hooks-remove", std::time::Instant::now()));
        cx.update(|cx| seed(&state, cx));
        let rows = cx.update(|cx| read_host(&state, cx, |host, _| host.hook_inputs.clone()));
        cx.update(|cx| {
            rows[0].update(cx, |input, cx| input.set_text("bundle install", cx));
            rows[1].update(cx, |input, cx| input.set_text("make test", cx));
        });
        let count = |cx: &mut gpui::TestAppContext| {
            cx.update(|cx| {
                read_host(&state, cx, |host, _| {
                    (host.edit_hooks.prepare_len, host.hook_inputs.len())
                })
            })
        };
        assert_eq!(count(cx), (2, 4), "each filled row grew a blank under it");

        // The trailing blank of either list is where the next command goes: never removed.
        cx.update(|cx| remove_row(&state, 1, cx));
        cx.update(|cx| remove_row(&state, 3, cx));
        assert_eq!(count(cx), (2, 4));

        cx.update(|cx| remove_row(&state, 0, cx));
        assert_eq!(count(cx), (1, 3), "the prepare command left its list");
        let texts = cx.update(|cx| {
            read_host(&state, cx, |host, cx| {
                host.hook_inputs
                    .iter()
                    .map(|input| input.read(cx).text().to_owned())
                    .collect::<Vec<_>>()
            })
        });
        assert_eq!(texts, vec!["", "make test", ""]);
    }

    #[gpui::test]
    fn the_cursor_row_follows_the_focused_input_and_the_trailing_blank_is_never_removed(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new("/tmp/hooks-cursor", std::time::Instant::now()));
        cx.update(|cx| seed(&state, cx));
        let rows = cx.update(|cx| read_host(&state, cx, |host, _| host.hook_inputs.clone()));
        cx.update(|cx| rows[0].update(cx, |input, cx| input.set_text("bundle install", cx)));
        let window = cx.add_empty_window();
        let cursor = |window: &mut gpui::VisualTestContext| {
            window.update(|window, cx| {
                read_host(&state, cx, |host, cx| {
                    (
                        host.edit_hooks.field,
                        cursor_row(&host.hook_inputs, window, cx),
                    )
                })
            })
        };
        assert_eq!(
            cursor(window),
            (0, None),
            "no editor holds the keyboard yet"
        );

        // `Tab` hands the keyboard to the next row, and the cursor goes with it.
        window.update(|window, cx| move_field(&state, 1, window, cx));
        assert_eq!(cursor(window), (1, Some(1)));

        // A row focused from outside the dialog's own keys (a click on its value) is the
        // cursor row too: the cursor follows the editor that has focus, not a stored index.
        let rows = window.update(|_, cx| read_host(&state, cx, |host, _| host.hook_inputs.clone()));
        assert_eq!(
            rows.len(),
            3,
            "the filled prepare row grew a blank under it"
        );
        window.update(|window, cx| rows[2].update(cx, |input, cx| input.focus(window, cx)));
        assert_eq!(cursor(window).1, Some(2));

        // The trailing blank of each list is where the next command is typed: its ✕ is
        // disabled and removing it does nothing, while a filled command can go.
        assert!(!is_trailing_blank(2, 3, 0));
        assert!(is_trailing_blank(2, 3, 1));
        assert!(is_trailing_blank(2, 3, 2));
        window.update(|_, cx| remove_row(&state, 1, cx));
        window.update(|_, cx| remove_row(&state, 2, cx));
        let texts = window.update(|_, cx| {
            read_host(&state, cx, |host, cx| {
                host.hook_inputs
                    .iter()
                    .map(|input| input.read(cx).text().to_owned())
                    .collect::<Vec<_>>()
            })
        });
        assert_eq!(texts, vec!["bundle install", "", ""]);
    }
}
