//! Dragging a card between and within the board's columns (UX-SPEC §Board, BOARD §8).
//!
//! The drag is gpui's: a tile carries a [`CardDrag`] from `on_drag`, every column listens with
//! `on_drag_move` and `on_drop`. What the frame draws of it — the faded place the card left,
//! the accent column, the [`fleet_ui_kit::DropSlot`] marking the insertion boundary — is the
//! [`DragState`] those listeners keep. A drop decides nothing here: it is a
//! [`super::BoardClick::Drop`], and the screen sends it down the path `[` / `]` take.

use std::{cell::RefCell, rc::Rc};

use fleet_core::ids::CardId;
use fleet_ui_kit::{ActiveTheme, COLUMN_WIDTH_CH, theme::ch};
use gpui::{App, Context, Pixels, SharedString, Window, div, prelude::*, px};

use super::CardRow;

/// Distance past a card midpoint required to reverse an already chosen insertion boundary.
/// This dead band keeps sub-pixel sampling and ±1 px pointer wobble from flipping the target.
const DROP_TARGET_HYSTERESIS: Pixels = px(4.0);

/// What a dragged tile carries: the column's prepared rows, and where the card was drawn when
/// the drag began.
///
/// The rows are the model's own `Rc`, so the value every visible tile hands gpui each frame
/// costs a reference count, not a copy of the card; the card is read out of it only once a
/// drag has actually started.
#[derive(Debug, Clone)]
pub(crate) struct CardDrag {
    /// The column's prepared rows, the dragged card among them.
    pub rows: Rc<[CardRow]>,
    /// The column the card was drawn in.
    pub column: usize,
    /// Its row there, counted over the tiles the column draws.
    pub row: usize,
}

impl CardDrag {
    /// The prepared card being dragged.
    #[must_use]
    pub(crate) fn card(&self) -> Option<&CardRow> {
        self.rows.get(self.row)
    }
}

/// Where a drag would land, and what the slot there says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DropTarget {
    /// The column under the pointer.
    pub column: usize,
    /// The place among that column's drawn tiles: `0` before the first, `n` after the last.
    pub slot: usize,
    /// The slot's sentence, or `None` where the drop would do nothing and no slot is drawn:
    /// the place the card already holds, or a board whose backend owns the status.
    pub label: Option<SharedString>,
}

/// The drag in flight, as the column listeners last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DragState {
    /// The card being dragged; its tile draws as the place it left.
    pub source: Option<CardId>,
    /// Where it would land.
    pub target: Option<DropTarget>,
}

/// The drag state the tile and column listeners share.
///
/// An `Rc<RefCell>` rather than a field of `AppState`: it changes on pointer motion, which
/// only this screen draws, and a notify on the app's one model repaints and re-observes the
/// whole shell. The listeners that write it `window.refresh()` when it changes, and the
/// screen reads it back only while gpui says a drag is active, so a drag released anywhere
/// else leaves nothing drawn behind.
pub(crate) type SharedDrag = Rc<RefCell<DragState>>;

/// The drag this frame draws: empty unless gpui has a drag in flight.
#[must_use]
pub(crate) fn frame(drag: &SharedDrag, cx: &App) -> DragState {
    if cx.has_active_drag() {
        drag.borrow().clone()
    } else {
        DragState::default()
    }
}

/// Points the drag at `slot` in `over`'s column, repainting only when that changes.
///
/// The sentence is written here, in the listener, and only for a new place: pointer motion
/// inside one slot costs a comparison, and render never formats it.
pub(crate) fn aim(
    shared: &SharedDrag,
    drag: &CardDrag,
    over: &ColumnDrop,
    slot: usize,
    window: &mut Window,
) {
    let mut state = shared.borrow_mut();
    if state
        .target
        .as_ref()
        .is_some_and(|at| at.column == over.column && at.slot == slot)
    {
        return;
    }
    state.target = Some(DropTarget {
        column: over.column,
        slot,
        label: slot_label(drag, over, slot),
    });
    window.refresh();
}

/// The slot a pointer at `y` over a tile spanning `top..bottom` at `row` points to: before the
/// tile in its upper half, after it in its lower half. Once either adjacent boundary is chosen,
/// the pointer must cross the midpoint by [`DROP_TARGET_HYSTERESIS`] to choose the other one.
///
/// Measured against the tile, not its list item. Over the gaps and the footer no tile answers,
/// so the slot stays where it is.
#[must_use]
pub(crate) fn slot_over(
    row: usize,
    top: Pixels,
    bottom: Pixels,
    y: Pixels,
    current: Option<usize>,
) -> usize {
    let midpoint = top + (bottom - top) / 2.0;
    if current == Some(row) && y <= midpoint + DROP_TARGET_HYSTERESIS {
        row
    } else if current == Some(row + 1) && y >= midpoint - DROP_TARGET_HYSTERESIS {
        row + 1
    } else if y < midpoint {
        row
    } else {
        row + 1
    }
}

/// What a column says about a drop, prepared once per column per frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnDrop {
    /// The column's index.
    pub column: usize,
    /// Its status name: `Todo`.
    pub name: SharedString,
    /// Who picks the card up, when entering the column starts a run.
    pub pickup: Option<SharedString>,
    /// Whether the board's backend owns the status: no drop moves anything, so no slot shows.
    pub readonly: bool,
}

/// The sentence a slot says for `drag` over `slot` in `at`, or `None` when no slot is drawn.
#[must_use]
pub(crate) fn slot_label(drag: &CardDrag, at: &ColumnDrop, slot: usize) -> Option<SharedString> {
    if at.readonly {
        return None;
    }
    let key = &drag.card()?.key;
    if drag.column == at.column {
        // Before or after itself is where the card already stands.
        return (slot != drag.row && slot != drag.row + 1)
            .then(|| SharedString::from(format!("Drop to put {key} here")));
    }
    Some(SharedString::from(match &at.pickup {
        Some(pickup) => format!("Drop to start {key} \u{b7} {pickup}"),
        None => format!("Drop to move {key} to {}", at.name),
    }))
}

/// The tile under the pointer while a card is dragged.
pub(crate) struct DragPreview {
    /// The tile's id and its card; `None` when the drag named a row its column no longer has.
    tile: Option<(SharedString, CardRow)>,
}

impl DragPreview {
    /// A preview of `row`'s tile.
    pub(crate) fn new(row: Option<CardRow>) -> Self {
        Self {
            tile: row.map(|row| {
                (
                    SharedString::from(format!("{}-preview", row.element_id)),
                    row,
                )
            }),
        }
    }
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // As wide as the tile it was picked up from: the column less its padding and hairlines.
        let width = ch(COLUMN_WIDTH_CH) - (theme.space.sm + theme.metrics.hairline) * 2.0;
        div().w(width).children(
            self.tile
                .as_ref()
                .map(|(id, row)| super::card_face(id.clone(), row).lifted(true)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag() -> CardDrag {
        let row = CardRow {
            id: CardId::try_from("card-3").unwrap_or_else(|error| panic!("{error}")),
            element_id: "board-card-card-3".into(),
            key: "FLT-3".into(),
            title: "Seed the board preset".into(),
            priority: fleet_ui_kit::PriorityLevel::None,
            labels: Vec::new(),
            assignee: None,
            estimate: None,
            due: None,
            worktree: false,
            dirty: false,
            conflict: false,
            extras: Vec::new(),
            run: None,
            run_label: None,
            blocked: None,
            blocked_label: None,
            link: None,
            reference: None,
            menu: super::super::CardMenu::default(),
        };
        CardDrag {
            rows: Rc::from(vec![row]),
            column: 1,
            row: 0,
        }
    }

    fn column(column: usize, pickup: Option<&'static str>) -> ColumnDrop {
        ColumnDrop {
            column,
            name: "In progress".into(),
            pickup: pickup.map(SharedString::from),
            readonly: false,
        }
    }

    /// The mockup's sentence: a column that starts a run says who will pick the card up.
    #[test]
    fn a_column_that_starts_a_run_says_who_picks_the_card_up() {
        assert_eq!(
            slot_label(&drag(), &column(2, Some("codex will pick it up")), 0).as_deref(),
            Some("Drop to start FLT-3 \u{b7} codex will pick it up")
        );
        assert_eq!(
            slot_label(&drag(), &column(2, None), 0).as_deref(),
            Some("Drop to move FLT-3 to In progress")
        );
    }

    /// Before or after itself is no move, and a read-only status is no move anywhere.
    #[test]
    fn no_slot_is_drawn_where_the_drop_would_move_nothing() {
        assert_eq!(slot_label(&drag(), &column(1, None), 0), None);
        assert_eq!(slot_label(&drag(), &column(1, None), 1), None);
        assert_eq!(
            slot_label(&drag(), &column(1, None), 2).as_deref(),
            Some("Drop to put FLT-3 here")
        );
        let readonly = ColumnDrop {
            readonly: true,
            ..column(2, None)
        };
        assert_eq!(slot_label(&drag(), &readonly, 0), None);
    }

    #[test]
    fn a_chosen_boundary_has_a_dead_band_around_the_card_midpoint() {
        let (top, bottom, midpoint) = (px(20.0), px(80.0), px(50.0));
        assert_eq!(slot_over(3, top, bottom, midpoint, None), 4);
        assert_eq!(slot_over(3, top, bottom, midpoint - px(1.0), Some(4)), 4);
        assert_eq!(slot_over(3, top, bottom, midpoint + px(1.0), Some(4)), 4);
        assert_eq!(slot_over(3, top, bottom, midpoint - px(1.0), Some(3)), 3);
        assert_eq!(slot_over(3, top, bottom, midpoint + px(1.0), Some(3)), 3);
        assert_eq!(
            slot_over(
                3,
                top,
                bottom,
                midpoint - DROP_TARGET_HYSTERESIS - px(1.0),
                Some(4),
            ),
            3
        );
        assert_eq!(
            slot_over(
                3,
                top,
                bottom,
                midpoint + DROP_TARGET_HYSTERESIS + px(1.0),
                Some(3),
            ),
            4
        );
    }
}
