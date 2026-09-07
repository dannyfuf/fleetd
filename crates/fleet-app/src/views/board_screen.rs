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

use fleet_core::board::{
    Board, BoardView, Card, Priority, Status, StatusCategory, column_cards as ops_column_cards,
};
use fleet_core::ids::StatusId;
use fleet_ui_kit::{
    ActiveTheme, Badge, CardTile, Chip, EmptyState, FilterBar, Icon, IconSize, KanbanBoard,
    KanbanColumn, Pane, PaneBorder, PaneHeader, PriorityLevel, SkeletonRows, SpinnerWithLabel,
    Text, Theme, Tone,
};
use gpui::{AnyElement, App, Hsla, MouseButton, ScrollHandle, SharedString, div, prelude::*};

mod model;
#[cfg(test)]
mod tests;

pub(crate) use model::priority_level;
use model::{HeaderFacts, card_extras, category_accent, grouped_cards, label_chips, placed};
pub use model::{counts, visible_cards};

/// How many skeleton columns a cold load shows.
const SKELETON_COLUMNS: usize = 3;
/// How many skeleton rows each of them shows.
const SKELETON_ROWS: usize = 4;

/// Everything the screen needs to draw itself.
pub(crate) struct BoardProps<'a> {
    /// The loaded board, absent while the first `EnsureBoard` is in flight.
    pub view: Option<&'a BoardView>,
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
    /// The registry's label for the board's backend kind, when it is known.
    pub backend_label: Option<&'a str>,
    /// The current epoch second, for the synced stamp.
    pub now: i64,
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
    column_scrolls: &[ScrollHandle],
    on_click: impl Fn(BoardClick, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let Some(view) = props.view else {
        return Pane::new()
            .border(PaneBorder::None)
            .focused(true)
            .header(loading_header(props))
            .body(cold_body(props, cx))
            .into_any_element();
    };

    let grouped = grouped_cards(view, props.filter);
    let shown = grouped.iter().map(Vec::len).sum();
    let total = placed(view);
    let orphans: Vec<_> = view
        .cards
        .iter()
        .filter(|card| {
            !card.archived
                && !view
                    .board
                    .statuses
                    .iter()
                    .any(|status| status.id == card.status_id)
        })
        .collect();
    let facts = HeaderFacts::of(view, props.backend_label, props.now);
    let header = header(props, &facts, shown, total, cx);

    let body: AnyElement = if view.board.statuses.is_empty() {
        EmptyState::new("This board has no columns.")
            .action(",  board settings")
            .into_any_element()
    } else if total == 0 {
        EmptyState::new("No cards yet.")
            .action("c  new card")
            .into_any_element()
    } else if shown == 0 && !props.filter.trim().is_empty() {
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
        columns(
            props,
            view,
            &grouped,
            board_scroll,
            column_scrolls,
            on_click,
            cx,
        )
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
                    facts
                        .error
                        .clone()
                        .filter(|_| props.error.is_none())
                        .map(|message| error_row(format!("sync: {message}"), None, cx)),
                )
                .children((!orphans.is_empty()).then(|| {
                    error_row(
                        format!(
                            // Reloading returns the same view: only a status this board still
                            // has, or a sync that restores the missing one, can place them.
                            "{} card(s) reference statuses this board no longer has \u{2014} \
                             ,  board settings or S  sync: {}",
                            orphans.len(),
                            orphans
                                .iter()
                                .map(|card| format!(
                                    "{} {}",
                                    card.display_key(&view.board),
                                    card.title
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        None,
                        cx,
                    )
                }))
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
fn header(
    props: &BoardProps<'_>,
    facts: &HeaderFacts,
    shown: usize,
    total: usize,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
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
fn columns(
    props: &BoardProps<'_>,
    view: &BoardView,
    grouped: &[Vec<&Card>],
    board_scroll: &ScrollHandle,
    column_scrolls: &[ScrollHandle],
    on_click: impl Fn(BoardClick, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let (focus_column, focus_row) = props.focus;
    let columns: Vec<AnyElement> = view
        .board
        .statuses
        .iter()
        .enumerate()
        .map(|(index, status)| {
            let cards = &grouped[index];
            let focused = index == focus_column;
            let tiles: Vec<AnyElement> = cards
                .iter()
                .enumerate()
                .map(|(row, card)| {
                    let selected = focused && row == focus_row;
                    let click = on_click.clone();
                    CardTile::new(
                        SharedString::from(format!("board-card-{}", card.id.as_str())),
                        card.display_key(&view.board),
                        card.title.clone(),
                    )
                    .priority(priority_level(card.priority))
                    .labels(label_chips(&view.board, card))
                    .assignee(card.assignee.clone().map(SharedString::from))
                    .estimate(card.estimate)
                    .due(card.due_date.clone().map(SharedString::from))
                    .worktree(card.worktree_id.is_some())
                    .dirty(card.dirty)
                    .conflict(card.conflict.is_some())
                    .selected(selected)
                    .focused(selected)
                    .extras(card_extras(&view.board, card))
                    .on_click(move |event, _window, cx| {
                        if event.click_count >= 2 {
                            click(BoardClick::OpenCard(index, row), cx);
                        } else {
                            click(BoardClick::Card(index, row), cx);
                        }
                    })
                    .into_any_element()
                })
                .collect();

            let mut column = KanbanColumn::new(
                SharedString::from(format!("board-column-{}", status.id.as_str())),
                status.name.clone(),
            )
            .count(cards.len())
            .accent(Some(category_accent(status, &theme)))
            .focused(focused)
            .empty_hint(if props.filter.is_empty() {
                "No cards here."
            } else {
                "No match here."
            })
            .tiles(tiles);
            if let Some(scroll) = column_scrolls.get(index) {
                column = column.scroll_handle(scroll.clone());
            }

            let click = on_click.clone();
            div()
                .id(SharedString::from(format!("board-column-hit-{index}")))
                .flex()
                .flex_none()
                .h_full()
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    click(BoardClick::Column(index), cx);
                })
                .child(column)
                .into_any_element()
        })
        .collect();

    KanbanBoard::new("board-columns")
        .scroll_handle(board_scroll.clone())
        .columns(columns)
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
