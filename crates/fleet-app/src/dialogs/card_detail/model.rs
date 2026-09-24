//! The property column's rows and their key chips, prepared in the update path.
//!
//! `render` only composes what is here (`docs/APP-CONTRACTS.md`, *render prepares nothing*):
//! the rows walk the board's statuses, labels and cards, and each chip is a key-table lookup,
//! so both are built once per change of what they read — the board mirror's revision, the card
//! on show, and the minute the `synced 2m` ages are stated at — and not once per frame.

use std::rc::Rc;

use fleet_ui_kit::Kbd;

use super::*;
use crate::{actions::board as board_actions, dialogs::card_picker::PickerKind, keymap};

/// The context the detail binds its keys in, the board's field keys included.
const KEY_CONTEXT: &str = "Dialog > CardDetail";

/// Everything the property rows are derived from, as cheap comparable values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PropertyKey {
    /// `BoardState::revision`, which moves on every applied view and every applied card.
    board: u64,
    /// The card on show.
    card: CardId,
    /// The minute the rows' ages are stated at.
    minute: i64,
}

/// The prepared property column of the card on show.
#[derive(Debug, Clone)]
pub(crate) struct PropertyModel {
    key: PropertyKey,
    /// One entry per row, top to bottom.
    pub(super) rows: Rc<[detail::PropertyRow]>,
    /// Each row's key chip, parallel to `rows`.
    pub(super) keys: Rc<[Option<Kbd>]>,
}

/// Rebuilds the property model when an input moved, and keeps the row cursor inside it.
///
/// Called from the dialog host's observation of the app state while the detail is open, and
/// by `seed`, so the sheet's first frame already has its rows.
pub(crate) fn refresh(state: &Entity<AppState>, cx: &mut App) {
    let now = now_unix();
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let app = state.read(cx);
    let key = app
        .board()
        .zip(card(app, &draft))
        .map(|(_, card)| PropertyKey {
            board: app.board.revision,
            card: card.id.clone(),
            minute: now / 60,
        });
    if draft.properties.as_ref().map(|model| &model.key) == key.as_ref() {
        return;
    }
    let model = key.and_then(|key| {
        let view = app.board()?;
        let card = card(app, &draft)?;
        let rows: Rc<[detail::PropertyRow]> =
            detail::property_rows(&view.board, &view.cards, card, now).into();
        let keys = rows.iter().map(row_key).collect();
        Some(Rc::new(PropertyModel { key, rows, keys }))
    });
    with_host(state, cx, |host| {
        let len = model.as_ref().map_or(0, |model| model.rows.len());
        host.card_detail.property_row = host.card_detail.property_row.min(len.saturating_sub(1));
        host.card_detail.properties = model;
    });
}

/// A row's chip: the key its action is bound to in the detail, from the key table itself.
fn row_key(row: &detail::PropertyRow) -> Option<Kbd> {
    let action = row_action(row)?;
    keymap::keystrokes_in(KEY_CONTEXT, action.as_ref()).map(|strokes| Kbd::new(&strokes))
}

/// The action whose key edits a row: the board's own field key where one exists, `⏎` otherwise.
///
/// Only the first row of a multi-row field names the key; the rows under it (`Blocked by`'s
/// second link) are still selected and opened by `⏎` or a click.
fn row_action(row: &detail::PropertyRow) -> Option<Box<dyn gpui::Action>> {
    use detail::PropertyTarget as T;
    Some(match &row.target {
        T::ReadOnly => return None,
        T::Worktree => Box::new(board_actions::OpenWorktree),
        T::Remote => Box::new(card_actions::OpenRemote),
        T::Pick(_) if row.label.is_empty() => Box::new(card_actions::EditProperty),
        T::Pick(PickerKind::Status) => Box::new(board_actions::PickStatus),
        T::Pick(PickerKind::Priority) => Box::new(board_actions::PickPriority),
        T::Pick(PickerKind::Assignee) => Box::new(board_actions::PickAssignee),
        T::Pick(PickerKind::Labels) => Box::new(board_actions::PickLabels),
        T::Pick(PickerKind::Estimate) => Box::new(board_actions::PickEstimate),
        T::Pick(PickerKind::BlockedBy) => Box::new(board_actions::PickBlockedBy),
        T::Pick(PickerKind::Provider) => Box::new(board_actions::PickAgent),
        T::Pick(_) => Box::new(card_actions::EditProperty),
    })
}
