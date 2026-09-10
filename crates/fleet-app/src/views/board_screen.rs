//! The kanban surface of the board tab (BOARD §8, UX-SPEC §board).
//!
//! *Which column is this card in, and what is the one key that moves it?* The screen is a
//! horizontal scroller of [`KanbanColumn`]s, one per `board.statuses` in contract order, each
//! holding the [`CardTile`]s that `ops::column_cards` puts in it.
//!
//! Everything that decides **what is drawn** — the filter predicate, the visible slice of a
//! column, the priority and category mappings, the header facts — is a pure function in
//! `model`, so the screen never derives the same thing twice and every rule is unit tested
//! without gpui.

use std::rc::Rc;

use fleet_core::board::{
    Board, BoardView, Card, Priority, Status, StatusCategory, column_cards as ops_column_cards,
};
use fleet_core::ids::StatusId;
use fleet_ui_kit::{
    ActiveTheme, Badge, CardTile, Chip, EmptyState, FilterBar, Icon, IconSize, KanbanBoard,
    KanbanColumn, Pane, PaneBorder, PaneHeader, PriorityLevel, SkeletonRows, SpinnerWithLabel,
    Text, Theme, Tone,
};
use gpui::{
    AnyElement, App, Hsla, ListState, MouseButton, ScrollHandle, SharedString, div, prelude::*,
};

mod model;
#[cfg(test)]
mod tests;

use model::category_accent;
pub(crate) use model::priority_level;
pub use model::{BoardModel, CardRow, ColumnRows, HeaderFacts, build, counts, visible_cards};

/// How many skeleton columns a cold load shows.
const SKELETON_COLUMNS: usize = 3;
/// How many skeleton rows each of them shows.
const SKELETON_ROWS: usize = 4;

/// Everything the screen needs to draw itself.
///
/// The board is a **prepared** model, not the raw view: the grouping, the filter, the sorts and
/// every tile string are derived once per board revision by
/// `crate::screens::board::projection`, and this body only composes them.
pub(crate) struct BoardProps<'a> {
    /// The prepared board, absent while the first `EnsureBoard` is in flight.
    pub model: Option<&'a BoardModel>,
    /// Whether a load is running.
    pub loading: bool,
    /// The last load failure, retained until an explicit reload.
    pub error: Option<&'a str>,
    /// The live filter query.
    pub filter: &'a str,
    /// Whether the filter input owns the keyboard.
    pub filter_editing: bool,
    /// The focused column and card.
    pub focus: (usize, usize),
    /// Whether a `board.sync` job is running for this board.
    pub syncing: bool,
}

/// What a mouse click on the board asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoardClick {
    /// Focus this column.
    Column(usize),
    /// Focus this card.
    Card(usize, usize),
    /// Focus and open this card.
    OpenCard(usize, usize),
}

/// Renders the board pane: header, then columns of tiles.
///
/// `on_click` is called with the intent of a click; the screen owns turning that into state.
pub(crate) fn render(
    props: &BoardProps<'_>,
    board_scroll: &ScrollHandle,
    column_lists: &[ListState],
    on_click: impl Fn(BoardClick, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let Some(model) = props.model else {
        return Pane::new()
            .border(PaneBorder::None)
            .focused(true)
            .header(loading_header(props))
            .body(cold_body(props, cx))
            .into_any_element();
    };

    let header = header(props, model, cx);

    let body: AnyElement = if model.no_columns {
        EmptyState::new("This board has no columns.")
            .action(",  board settings")
            .into_any_element()
    } else if model.total == 0 {
        EmptyState::new("No cards yet.")
            .action("c  new card")
            .into_any_element()
    } else if model.shown == 0 && !props.filter.trim().is_empty() {
        EmptyState::new(format!("Nothing matches \"{}\".", props.filter.trim()))
            // Escape is two-stage: while the filter input owns the keyboard the first one
            // only leaves the input, so promising one key here would read as a dead key.
            .action(if props.filter_editing {
                "esc esc  clear"
            } else {
                "esc  clear"
            })
            .into_any_element()
    } else {
        columns(props, model, board_scroll, column_lists, on_click, cx)
    };

    Pane::new()
        .border(PaneBorder::None)
        .focused(!props.filter_editing)
        .header(header)
        .body(
            div()
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .children(
                    props
                        .error
                        .map(|message| error_row(message.to_owned(), Some("r  reload"), cx)),
                )
                .children(
                    model
                        .facts
                        .error
                        .clone()
                        .filter(|_| props.error.is_none())
                        .map(|message| error_row(format!("sync: {message}"), None, cx)),
                )
                .children(
                    model
                        .orphans
                        .clone()
                        .map(|message| error_row(message, None, cx)),
                )
                .child(div().flex_1().min_h_0().p(cx.theme().space.md).child(body)),
        )
        .into_any_element()
}

/// The header while nothing has been loaded yet.
fn loading_header(props: &BoardProps<'_>) -> AnyElement {
    PaneHeader::new("Board")
        .trailing(if props.loading {
            SpinnerWithLabel::new("board-loading", "loading").into_any_element()
        } else {
            div().into_any_element()
        })
        .into_any_element()
}

/// Skeleton columns for a cold load, the error card for a failed one.
fn cold_body(props: &BoardProps<'_>, cx: &App) -> AnyElement {
    if let Some(message) = props.error {
        return div()
            .flex()
            .flex_col()
            .size_full()
            .child(error_row(message.to_owned(), Some("r  reload"), cx))
            .child(EmptyState::new("The board could not be loaded.").action("r  reload"))
            .into_any_element();
    }
    let theme = cx.theme();
    div()
        .flex()
        .flex_row()
        .size_full()
        .min_h_0()
        .gap(theme.space.md)
        .p(theme.space.md)
        .children((0..SKELETON_COLUMNS).map(|index| {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(fleet_ui_kit::theme::ch(fleet_ui_kit::COLUMN_WIDTH_CH))
                .id(SharedString::from(format!("board-skeleton-{index}")))
                .child(SkeletonRows::new(SKELETON_ROWS))
        }))
        .into_any_element()
}

/// The board header: `BOARD · <name>` on the left, the board's own facts on the right.
fn header(props: &BoardProps<'_>, model: &BoardModel, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let facts = &model.facts;
    let (shown, total) = (model.shown, model.total);
    let trailing = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.space.sm)
        .child(Badge::new(facts.prefix.clone()))
        .children((!facts.local).then(|| {
            Chip::labeled(Icon::Cloud, facts.backend.clone()).tone(if facts.error.is_some() {
                Tone::Danger
            } else {
                Tone::Secondary
            })
        }))
        .children(match (&facts.synced, facts.local) {
            (Some(age), _) => Some(Text::label(format!("synced {age}")).faint()),
            // A remote board with no stamp has never synced, and omitting the label leaves it
            // looking exactly like one synced seconds ago. The CLI says so in the same place.
            (None, false) => Some(Text::label("never synced").faint()),
            (None, true) => None,
        })
        .children((facts.dirty > 0).then(|| {
            Chip::counter(Icon::CloudUpload, facts.dirty)
                .tone(Tone::Warning)
                .id("board-dirty")
        }))
        .children((facts.conflicts > 0).then(|| {
            Chip::counter(Icon::TriangleAlert, facts.conflicts)
                .tone(Tone::Danger)
                .id("board-conflicts")
        }))
        .children(
            props
                .syncing
                .then(|| SpinnerWithLabel::new("board-sync", "syncing")),
        )
        .children(
            (props.loading && !props.syncing)
                .then(|| SpinnerWithLabel::new("board-refresh", "refreshing")),
        );

    let mut header = PaneHeader::new("Board")
        .scope(facts.name.clone())
        .shown(shown)
        .total(total)
        .trailing(trailing);
    if props.filter_editing {
        header = header.query_slot(
            FilterBar::new(props.filter.to_owned(), shown, total)
                .focused(true)
                .query_slot(),
        );
    } else if !props.filter.is_empty() {
        header = header.filter_chip(props.filter.to_owned());
    }
    header.into_any_element()
}

/// The columns and their tiles.
///
/// A column hands [`KanbanColumn::rows`] a closure over its prepared rows rather than a vector
/// of finished tiles, so a 300-card column builds the dozen elements its viewport can show and
/// not 300 (`gpui-performance` rule 4). Every string the closure reaches for was allocated once,
/// when the model was prepared.
fn columns(
    props: &BoardProps<'_>,
    model: &BoardModel,
    board_scroll: &ScrollHandle,
    column_lists: &[ListState],
    on_click: impl Fn(BoardClick, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let (focus_column, focus_row) = props.focus;
    let empty_hint = if props.filter.is_empty() {
        "No cards here."
    } else {
        "No match here."
    };
    let columns: Vec<AnyElement> = model
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let focused = index == focus_column;
            let mut kanban =
                KanbanColumn::new(column.element_id.clone(), column.status.name.clone())
                    .count(column.rows.len())
                    .accent(Some(category_accent(&column.status, &theme)))
                    .focused(focused)
                    .empty_hint(empty_hint);
            if let Some(list) = column_lists.get(index) {
                let rows = Rc::clone(&column.rows);
                let click = on_click.clone();
                kanban = kanban.rows(list.clone(), rows.len(), move |row, _window, _cx| {
                    let Some(card) = rows.get(row) else {
                        return div().into_any_element();
                    };
                    tile(card, focused && row == focus_row, index, row, click.clone())
                });
            }

            let click = on_click.clone();
            div()
                .id(column.hit_id.clone())
                .flex()
                .flex_none()
                .h_full()
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    click(BoardClick::Column(index), cx);
                })
                .child(kanban)
                .into_any_element()
        })
        .collect();

    KanbanBoard::new("board-columns")
        .scroll_handle(board_scroll.clone())
        .columns(columns)
        .into_any_element()
}

/// One prepared card as its tile.
fn tile(
    card: &CardRow,
    selected: bool,
    column: usize,
    row: usize,
    on_click: impl Fn(BoardClick, &mut App) + 'static,
) -> AnyElement {
    CardTile::new(
        card.element_id.clone(),
        card.key.clone(),
        card.title.clone(),
    )
    .priority(card.priority)
    .labels(card.labels.clone())
    .assignee(card.assignee.clone())
    .estimate(card.estimate)
    .due(card.due.clone())
    .worktree(card.worktree)
    .dirty(card.dirty)
    .conflict(card.conflict)
    .selected(selected)
    .focused(selected)
    .extras(card.extras.clone())
    .on_click(move |event, _window, cx| {
        if event.click_count >= 2 {
            on_click(BoardClick::OpenCard(column, row), cx);
        } else {
            on_click(BoardClick::Card(column, row), cx);
        }
    })
    .into_any_element()
}

/// The sticky error row: it never hides the columns underneath it.
///
/// `hint` is the key that answers *this* row. Only the load failure is answered by `r`, and
/// spelling it on every row put it beside an orphan row whose own text says `,  board settings
/// or S  sync` and notes that reloading returns the same view.
fn error_row(message: impl Into<SharedString>, hint: Option<&'static str>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.space.sm)
        .w_full()
        .h(theme.metrics.row_h)
        .px(theme.space.lg)
        .child(
            Icon::CircleX
                .el()
                .size(IconSize::Small)
                .color(Tone::Danger.color(theme)),
        )
        .child(Text::ui(message).tone(Tone::Danger).ellipsize())
        .children(hint.map(Text::hint))
        .into_any_element()
}
