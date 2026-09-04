//! §3.8.4 New / Edit context.
//!
//! Two fields and one derived id. `owners` carries the app's **only** teaching line, because
//! it is the one field whose purpose is not guessable from its name.
//!
//! **[D-11]**: `ctrl-d` here deletes the context, routed through the expanded `Y` confirm of
//! §3.8.3 rather than deleting anything itself.

use fleet_core::{ids::ContextId, slug::normalize_context_id};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{context_dialog, dialog},
    bridge::Bridge,
    dialogs::{
        ConfirmRequest, Dialogs, TextInput, notify, request_confirm, root, type_into, with_host,
    },
    state::{AppState, Overlay},
};

/// Which field owns the keyboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Field {
    /// The name input.
    #[default]
    Name,
    /// The comma-separated owners input.
    Owners,
}

/// The New / Edit context draft.
#[derive(Debug, Clone, Default)]
pub struct ContextState {
    /// The context being edited, or `None` when creating one.
    pub editing: Option<ContextId>,
    /// Whether the id is read-only because repositories already live in this context.
    pub id_locked: bool,
    /// The display name.
    pub name: TextInput,
    /// The comma-separated owner list.
    pub owners: TextInput,
    /// Which field owns the keyboard.
    pub field: Field,
    /// Every existing context id, for the duplicate check.
    pub existing: Vec<String>,
    /// How many repositories, worktrees and sessions a delete would cascade to.
    pub cascade: (usize, usize, usize),
}

impl ContextState {
    /// The `ContextId` the typed name produces (§1 slugify rules).
    #[must_use]
    pub fn preview_id(&self) -> String {
        normalize_context_id(self.name.value())
    }

    /// The owners, split and trimmed the way the daemon stores them.
    #[must_use]
    pub fn owner_list(&self) -> Vec<String> {
        self.owners
            .value()
            .split(',')
            .map(str::trim)
            .filter(|owner| !owner.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The duplicate-id message, when the preview collides with another context.
    #[must_use]
    pub fn duplicate(&self) -> Option<String> {
        let id = self.preview_id();
        if id.is_empty() {
            return None;
        }
        if self
            .editing
            .as_ref()
            .is_some_and(|open| open.as_str() == id)
        {
            return None;
        }
        self.existing
            .iter()
            .any(|existing| existing == &id)
            .then(|| format!("A context with id \"{id}\" already exists."))
    }

    /// Whether `Enter` may create or save.
    #[must_use]
    pub fn can_submit(&self) -> bool {
        !self.preview_id().is_empty()
            && self.duplicate().is_none()
            && ContextId::try_from(self.preview_id()).is_ok()
    }
}

// ---------------------------------------------------------------------------- seeding

/// Fills the draft: empty for `N`, the active context's values for `E`.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App, editing: bool) {
    let mut draft = ContextState::default();
    {
        let app = state.read(cx);
        if let Some(snapshot) = app.snapshot.as_ref() {
            draft.existing = snapshot
                .contexts
                .iter()
                .map(|entry| entry.id.as_str().to_owned())
                .collect();
            if editing
                && let Some(context) = snapshot
                    .contexts
                    .iter()
                    .find(|entry| Some(&entry.id) == app.active_context())
            {
                draft.editing = Some(context.id.clone());
                draft.name = TextInput::new(context.name.clone());
                draft.owners = TextInput::new(context.owners.join(", "));
                let repos: Vec<_> = snapshot
                    .repos
                    .iter()
                    .filter(|repo| repo.context_id == context.id)
                    .collect();
                let worktrees = snapshot
                    .worktrees
                    .iter()
                    .filter(|worktree| repos.iter().any(|repo| repo.id == worktree.repo_id))
                    .count();
                draft.id_locked = !repos.is_empty();
                draft.cascade = (repos.len(), worktrees, snapshot.sessions.len());
            }
        }
    }
    with_host(cx, |host| host.context = draft);
}

// ---------------------------------------------------------------------------- rendering

/// Renders the dialog (§3.8.4).
pub(crate) fn render(
    dialog_kind: &Dialogs,
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let draft = with_host(cx, |host| host.context.clone());
    let editing = draft.editing.clone();
    let duplicate = draft.duplicate();

    let mut name_field = TextField::new(draft.name.value().to_owned())
        .label("Name")
        .placeholder("Buk HR")
        .caret(draft.name.caret())
        .focused(draft.field == Field::Name);
    name_field = match (&duplicate, draft.id_locked) {
        (Some(message), _) => name_field.invalid(message.clone()),
        // §3.8.4: once repos exist the id is read-only outright, and saying so beats a
        // disabled-looking input.
        (None, true) => name_field.preview(format!(
            "\u{2192} {} (id is fixed once repos exist)",
            draft.preview_id()
        )),
        (None, false) => name_field.preview(format!("\u{2192} {}", draft.preview_id())),
    };

    let owners_field = TextField::new(draft.owners.value().to_owned())
        .label("Owners")
        .placeholder("bukhr, dannyfuf")
        .caret(draft.owners.caret())
        .focused(draft.field == Field::Owners)
        .preview("GitHub orgs/users used to scope PRs");

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(name_field)
        .child(owners_field);

    let mut hints = KeyHintRow::new()
        .key("\u{21e5}", "field")
        .key("esc", "cancel");
    if editing.is_some() {
        hints = hints.key("\u{2303}d", "delete context");
    }
    let mut card = Dialog::new(if editing.is_some() {
        "Edit context"
    } else {
        "New context"
    })
    .icon(Icon::Boxes)
    .width(dialog_kind.width())
    .body(body)
    .hint_row(hints)
    .primary(if editing.is_some() {
        "\u{23ce} Save"
    } else {
        "\u{23ce} Create"
    });
    if let Some(open) = editing.as_ref() {
        card = card.subtitle(format!("\u{00b7} {}", open.as_str()));
    }
    if draft.owner_list().is_empty() {
        card = card.error("Without owners, GitHub repo search and PR \"mine\" are empty.");
    }

    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    let delete_state = state.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let changed = with_host(cx, |host| match host.context.field {
                    Field::Name => type_into(&mut host.context.name, event),
                    Field::Owners => type_into(&mut host.context.owners, event),
                });
                if changed {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| toggle_field(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| toggle_field(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                if edit(cx, TextInput::backspace) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                if edit(cx, TextInput::delete_word) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                if edit(cx, TextInput::clear) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                move_caret(cx, TextInput::home);
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                move_caret(cx, TextInput::end);
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                move_caret(cx, TextInput::left);
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                move_caret(cx, TextInput::right);
                notify(&state, cx);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            submit(&confirm_state, &confirm_bridge, cx);
        })
        .on_action(move |_: &context_dialog::Delete, _window, cx| {
            open_delete_confirm(&delete_state, cx);
        })
        .child(card)
        .into_any_element()
}

fn toggle_field(state: &Entity<AppState>, cx: &mut App) {
    with_host(cx, |host| {
        host.context.field = match host.context.field {
            Field::Name => Field::Owners,
            Field::Owners => Field::Name,
        };
    });
    notify(state, cx);
}

/// Applies a mutating edit to whichever field has focus.
fn edit(cx: &mut App, apply: impl FnOnce(&mut TextInput) -> bool) -> bool {
    with_host(cx, |host| match host.context.field {
        Field::Name => apply(&mut host.context.name),
        Field::Owners => apply(&mut host.context.owners),
    })
}

/// Applies a caret move to whichever field has focus.
fn move_caret(cx: &mut App, apply: impl FnOnce(&mut TextInput)) {
    with_host(cx, |host| match host.context.field {
        Field::Name => apply(&mut host.context.name),
        Field::Owners => apply(&mut host.context.owners),
    });
}

/// `Enter`: create or save. A duplicate id makes it inert (§3.8.4).
fn submit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some((editing, name, owners)) = with_host(cx, |host| {
        if !host.context.can_submit() {
            return None;
        }
        Some((
            host.context.editing.clone(),
            host.context.name.value().to_owned(),
            host.context.owner_list(),
        ))
    }) else {
        return;
    };
    match editing {
        Some(id) => bridge.send(RequestBody::UpdateContext {
            id,
            name: Some(name),
            owners: Some(owners),
        }),
        None => bridge.send(RequestBody::CreateContext { name, owners }),
    }
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}

/// `ctrl-d`: hand the delete to the expanded `Y` confirm instead of doing it here.
fn open_delete_confirm(state: &Entity<AppState>, cx: &mut App) {
    let Some((context, name, cascade)) = with_host(cx, |host| {
        Some((
            host.context.editing.clone()?,
            host.context.name.value().to_owned(),
            host.context.cascade,
        ))
    }) else {
        return;
    };
    request_confirm(
        cx,
        ConfirmRequest::DeleteContext {
            context,
            name,
            repos: cascade.0,
            worktrees: cascade.1,
            sessions: cascade.2,
        },
    );
    state.update(cx, |app, cx| {
        app.open_overlay(Overlay::Dialog(Dialogs::Confirm));
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_preview_is_the_slugified_name() {
        let mut draft = ContextState {
            name: TextInput::new("Buk HR"),
            ..ContextState::default()
        };
        assert_eq!(draft.preview_id(), "buk-hr");
        draft.name = TextInput::new("  ");
        assert_eq!(draft.preview_id(), "");
        assert!(!draft.can_submit());
    }

    #[test]
    fn owners_are_split_on_commas_and_trimmed() {
        let draft = ContextState {
            owners: TextInput::new(" bukhr ,dannyfuf, "),
            ..ContextState::default()
        };
        assert_eq!(draft.owner_list(), vec!["bukhr", "dannyfuf"]);
    }

    #[test]
    fn a_duplicate_id_blocks_enter_but_editing_its_own_id_does_not() {
        let mut draft = ContextState {
            name: TextInput::new("Buk"),
            existing: vec!["buk".to_owned()],
            ..ContextState::default()
        };
        assert_eq!(
            draft.duplicate().as_deref(),
            Some("A context with id \"buk\" already exists.")
        );
        assert!(!draft.can_submit());
        draft.editing = ContextId::try_from("buk").ok();
        assert_eq!(draft.duplicate(), None);
        assert!(draft.can_submit());
    }

    #[test]
    fn empty_owners_are_allowed() {
        let draft = ContextState {
            name: TextInput::new("Personal"),
            ..ContextState::default()
        };
        assert!(draft.owner_list().is_empty());
        assert!(draft.can_submit());
    }
}
