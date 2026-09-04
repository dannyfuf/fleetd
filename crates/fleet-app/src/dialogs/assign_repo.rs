//! §3.8.5 Assign repo to context (`m`).
//!
//! **[D-12]**: this dialog has no text field, so `j` / `k` move the selection alongside
//! `↓` / `↑` and `ctrl-n` / `ctrl-p`. `keymap.rs` binds `j` and `k` to the shared
//! `dialog::CursorDown` / `dialog::CursorUp` inside `Dialog > Assign` for exactly that reason,
//! so one listener serves all six keys.

use fleet_core::ids::{ContextId, RepoId};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{notify, root, step, with_host},
    state::AppState,
};

/// One row of the context list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    /// The context's id, which is what the move request carries.
    pub id: ContextId,
    /// Its display name.
    pub name: String,
    /// Its owners, joined — the reason a repo belongs to a context (§3.8.5).
    pub owners: String,
    /// Whether the repository already lives here.
    pub current: bool,
}

/// The Assign dialog's draft.
#[derive(Debug, Clone, Default)]
pub struct AssignState {
    /// The repository being moved.
    pub repo: Option<RepoId>,
    /// Every context, in rail order.
    pub rows: Vec<ContextRow>,
    /// Which row carries the cursor.
    pub cursor: usize,
}

impl AssignState {
    /// The context `Enter` would move the repository to.
    #[must_use]
    pub fn selected(&self) -> Option<&ContextRow> {
        self.rows.get(self.cursor)
    }
}

// ---------------------------------------------------------------------------- seeding

/// Fills the list and puts the cursor on the repository's current context.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let mut draft = AssignState::default();
    {
        let app = state.read(cx);
        draft.repo = crate::dialogs::focused_repo(app);
        if let Some(snapshot) = app.snapshot.as_ref() {
            let current = draft.repo.as_ref().and_then(|repo| {
                snapshot
                    .repos
                    .iter()
                    .find(|entry| &entry.id == repo)
                    .map(|entry| entry.context_id.clone())
            });
            draft.rows = snapshot
                .contexts
                .iter()
                .map(|context| ContextRow {
                    id: context.id.clone(),
                    name: context.name.clone(),
                    owners: context.owners.join(", "),
                    current: Some(&context.id) == current.as_ref(),
                })
                .collect();
            draft.cursor = draft
                .rows
                .iter()
                .position(|row| row.current)
                .unwrap_or_default();
        }
    }
    with_host(cx, |host| host.assign = draft);
}

// ---------------------------------------------------------------------------- rendering

/// Renders the dialog (§3.8.5).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let draft = with_host(cx, |host| host.assign.clone());

    let list = FuzzyList::new(draft.rows.iter().map(|row| {
        let mut item = FuzzyItem::new(row.name.clone());
        if !row.owners.is_empty() {
            item = item.secondary(row.owners.clone());
        }
        if row.current {
            item = item.trailing("current");
        }
        item
    }))
    .cursor(draft.cursor)
    .cap(draft.rows.len().max(1))
    // [D-12]: no text field above it, so the list owns `j` / `k`.
    .under_text_field(false)
    .empty(Text::ui("No contexts yet.").muted());

    let body = div().flex().flex_col().gap(gap).child(list).child(
        Text::ui(
            "Moves the repo record only \u{2014} nothing on disk changes, sessions keep running.",
        )
        .muted(),
    );

    let card = Dialog::new("Move repo")
        .icon(Icon::ArrowRightLeft)
        .width(super::Dialogs::AssignRepo.width())
        .subtitle(
            draft
                .repo
                .as_ref()
                .map_or_else(String::new, |repo| format!("\u{00b7} {}", repo.as_str())),
        )
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key("j/k", "move")
                .key("\u{2303}n/\u{2303}p", "move")
                .key("esc", "cancel"),
        )
        .primary("\u{23ce} Move");

    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();

    root(focus)
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| move_cursor(&state, -1, cx)
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            submit(&confirm_state, &confirm_bridge, cx);
        })
        .child(card)
        .into_any_element()
}

fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(cx, |host| {
        let len = host.assign.rows.len();
        host.assign.cursor = step(host.assign.cursor, delta, len);
    });
    notify(state, cx);
}

/// `Enter`: move the record. Moving to the current context is a no-op, so it just closes.
fn submit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let move_to = with_host(cx, |host| {
        let row = host.assign.selected()?;
        if row.current {
            return None;
        }
        Some((host.assign.repo.clone()?, row.id.clone()))
    });
    if let Some((repo, context)) = move_to {
        bridge.send(RequestBody::MoveRepoToContext { repo, context });
    }
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, current: bool) -> ContextRow {
        ContextRow {
            id: ContextId::try_from(id).unwrap_or_else(|error| panic!("{error}")),
            name: id.to_owned(),
            owners: "acme".to_owned(),
            current,
        }
    }

    #[test]
    fn the_cursor_starts_on_the_repos_current_context() {
        let draft = AssignState {
            repo: RepoId::try_from("buk/payroll").ok(),
            rows: vec![row("personal", false), row("buk", true)],
            cursor: 1,
        };
        assert_eq!(draft.selected().map(|row| row.current), Some(true));
    }

    #[test]
    fn the_selection_clamps_at_both_ends() {
        let mut draft = AssignState {
            rows: vec![row("a", false), row("b", false)],
            ..AssignState::default()
        };
        draft.cursor = step(draft.cursor, -1, draft.rows.len());
        assert_eq!(draft.cursor, 0);
        draft.cursor = step(draft.cursor, 5, draft.rows.len());
        assert_eq!(draft.cursor, 1);
    }
}
