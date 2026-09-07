//! §3.8.5 Assign repo to context (`m`).

use fleet_core::ids::{ContextId, RepoId};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{DialogHost, notify, root, step, with_host},
    state::AppState,
};

/// One row of the context list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    /// The context's id, which is what the move request carries.
    pub(crate) id: ContextId,
    /// Its display name.
    pub(crate) name: String,
    /// Its owners, joined — the reason a repo belongs to a context (§3.8.5).
    pub(crate) owners: String,
    /// Whether the repository already lives here.
    pub(crate) current: bool,
}

/// The Assign dialog's draft.
#[derive(Debug, Clone, Default)]
pub struct AssignState {
    /// The repository being moved.
    pub(crate) repo: Option<RepoId>,
    /// Every context, in rail order.
    pub(crate) rows: std::rc::Rc<[ContextRow]>,
    /// Which row carries the cursor.
    pub(crate) cursor: usize,
    pub(crate) scroll: gpui::UniformListScrollHandle,
}

impl AssignState {
    fn select_current(&mut self) {
        self.cursor = self
            .rows
            .iter()
            .position(|row| row.current)
            .unwrap_or_default();
        self.scroll
            .scroll_to_item(self.cursor, gpui::ScrollStrategy::Nearest);
    }

    /// The context `Enter` would move the repository to.
    #[must_use]
    pub fn selected(&self) -> Option<&ContextRow> {
        self.rows.get(self.cursor)
    }
}

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
            draft.select_current();
        }
    }
    with_host(state, cx, |host| host.assign = draft);
}

/// Renders the dialog (§3.8.5).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let draft = host.read(cx).assign.clone();

    let rows = draft.rows.clone();
    let list = ListView::new(
        "assign-contexts",
        rows.len(),
        move |index, selected, _, _| {
            let row = &rows[index];
            Row::new()
                .cursor(selected)
                .selected(selected)
                .column(RowColumn::flex(Text::ui(row.name.clone()).ellipsize()))
                .when(!row.owners.is_empty(), |item| {
                    item.column(RowColumn::flex(
                        Text::ui(row.owners.clone()).muted().ellipsize(),
                    ))
                })
                .when(row.current, |item| {
                    item.column(
                        RowColumn::auto(Text::ui("current").faint()).align(ColumnAlign::Right),
                    )
                })
                .into_any_element()
        },
    )
    .cursor(draft.cursor)
    .track_scroll(&draft.scroll)
    .empty(Text::ui("No contexts yet.").muted());
    let list = div()
        .h(cx.theme().metrics.row_h * draft.rows.len().clamp(1, 10) as f32)
        .child(list);

    let body = div().flex().flex_col().gap(gap).child(list).child(
        Text::ui(
            "Moves the repo record only \u{2014} nothing on disk changes, sessions keep running.",
        )
        .muted(),
    );

    let card = Dialog::new("Move repo")
        .icon(Icon::ArrowRightLeft)
        .width(super::Dialogs::AssignRepo.width(cx))
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
    with_host(state, cx, |host| {
        let len = host.assign.rows.len();
        host.assign.cursor = step(host.assign.cursor, delta, len);
        host.assign
            .scroll
            .scroll_to_item(host.assign.cursor, gpui::ScrollStrategy::Nearest);
    });
    notify(state, cx);
}

/// `Enter`: move the record. Moving to the current context is a no-op, so it just closes.
fn submit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let move_to = with_host(state, cx, |host| {
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
        let mut draft = AssignState {
            repo: RepoId::try_from("buk/payroll").ok(),
            rows: vec![row("personal", false), row("buk", true)].into(),
            ..AssignState::default()
        };
        draft.select_current();
        assert_eq!(draft.selected().map(|row| row.current), Some(true));
    }
}
