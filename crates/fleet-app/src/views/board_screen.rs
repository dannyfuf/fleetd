//! The kanban surface of the board tab (BOARD §8, UX-SPEC §board).
//!
//! *Which column is this card in, and what is the one key that moves it?* The screen is a
//! horizontal scroller of [`KanbanColumn`]s, one per `board.statuses` in contract order, each
//! holding the [`CardTile`]s that `ops::column_cards` puts in it.
//!
//! Everything that decides **what is drawn** is a pure function here — the filter predicate,
//! the visible slice of a column, the priority and category mappings, the header facts — so
//! the screen never derives the same thing twice and every rule is unit tested without gpui.

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

/// How many skeleton columns a cold load shows.
pub const SKELETON_COLUMNS: usize = 3;
/// How many skeleton rows each of them shows.
pub const SKELETON_ROWS: usize = 4;

// ---------------------------------------------------------------------------- pure model

/// Whether a card survives the board filter.
///
/// The match is a case-insensitive **substring** over the four things a card is looked up by —
/// title, key, labels and assignee — for the reason §3.10 gives: a filter that hides rows whose
/// letters you can see is worse than one that ignores the order you typed them in.
#[must_use]
pub fn card_matches(board: &Board, card: &Card, query: &str) -> bool {
    matches_needle(board, card, &query.trim().to_lowercase())
}

fn matches_needle(board: &Board, card: &Card, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let contains = |text: &str| text.to_lowercase().contains(needle);
    contains(&card.title)
        || card.remote.as_ref().is_some_and(|link| contains(&link.key))
        || contains(&card.local_key(board))
        || card.assignee.as_deref().is_some_and(contains)
        || card.labels.iter().any(|id| {
            board
                .labels
                .iter()
                .find(|label| &label.id == id)
                .is_some_and(|label| contains(&label.name))
                || contains(id.as_str())
        })
}

/// The cards of one column, in contract order, after the filter.
#[must_use]
pub fn visible_cards<'a>(view: &'a BoardView, status: &StatusId, query: &str) -> Vec<&'a Card> {
    let needle = query.trim().to_lowercase();
    ops_column_cards(&view.cards, status)
        .into_iter()
        .filter(|card| matches_needle(&view.board, card, &needle))
        .collect()
}

/// How many cards the whole board shows, and how many it holds (`8/12`).
///
/// Both halves count the same population: a card whose status no column names can never be
/// shown, so counting it in the total made a board with two orphans read `3/5` with no filter
/// set and no filter chip — indistinguishable from a filter hiding two cards. The orphans are
/// named by their own error row, which is where they can actually be acted on.
#[must_use]
pub fn counts(view: &BoardView, query: &str) -> (usize, usize) {
    let columns = grouped_cards(view, query);
    (columns.iter().map(Vec::len).sum(), placed(view))
}

/// The live cards this board has a column for.
fn placed(view: &BoardView) -> usize {
    view.cards
        .iter()
        .filter(|card| {
            !card.archived
                && view
                    .board
                    .statuses
                    .iter()
                    .any(|status| status.id == card.status_id)
        })
        .count()
}

fn grouped_cards<'a>(view: &'a BoardView, query: &str) -> Vec<Vec<&'a Card>> {
    let needle = query.trim().to_lowercase();
    let mut columns = vec![Vec::new(); view.board.statuses.len()];
    for card in view.cards.iter().filter(|card| !card.archived) {
        if let Some(column) = view
            .board
            .statuses
            .iter()
            .position(|status| status.id == card.status_id)
            && matches_needle(&view.board, card, &needle)
        {
            columns[column].push(card);
        }
    }
    for cards in &mut columns {
        cards.sort_by(|a, b| {
            (a.position, &a.created_at, a.number).cmp(&(b.position, &b.created_at, b.number))
        });
    }
    columns
}

/// The kit's priority level for a domain priority.
#[must_use]
pub const fn priority_level(priority: Priority) -> PriorityLevel {
    match priority {
        Priority::Urgent => PriorityLevel::Urgent,
        Priority::High => PriorityLevel::High,
        Priority::Medium => PriorityLevel::Medium,
        Priority::Low => PriorityLevel::Low,
        Priority::None => PriorityLevel::None,
    }
}

/// The accent a status column wears.
///
/// The kit takes a resolved color because the mapping from a workflow category to a token is
/// domain knowledge: `Started` is the amber "in flight" of §1.4, `Completed` the green, a
/// `Canceled` column the muted grey that says "this is not a failure, it is a dead end".
#[must_use]
pub fn category_accent(status: &Status, theme: &Theme) -> Hsla {
    if let Some(token) = status.color.as_deref()
        && let Some(color) = token_color(token, theme)
    {
        return color;
    }
    match status.category {
        StatusCategory::Backlog => theme.colors.text_muted,
        StatusCategory::Unstarted => theme.colors.text_secondary,
        StatusCategory::Started => theme.colors.warning,
        StatusCategory::Completed => theme.colors.success,
        StatusCategory::Canceled => theme.colors.border_strong,
    }
}

/// A board-supplied token **name** resolved against the theme; unknown names fall back.
fn token_color(token: &str, theme: &Theme) -> Option<Hsla> {
    Some(match token {
        "accent" => theme.colors.accent,
        "success" => theme.colors.success,
        "warning" => theme.colors.warning,
        "danger" => theme.colors.danger,
        "info" => theme.colors.info,
        "muted" => theme.colors.text_muted,
        "secondary" => theme.colors.text_secondary,
        _ => return None,
    })
}

/// The label chips of a card, as the kit wants them: name plus an optional token **name**.
#[must_use]
pub fn label_chips(board: &Board, card: &Card) -> Vec<(SharedString, Option<SharedString>)> {
    card.labels
        .iter()
        .filter_map(|id| board.labels.iter().find(|label| &label.id == id))
        .map(|label| {
            (
                SharedString::from(label.name.clone()),
                label.color.clone().map(SharedString::from),
            )
        })
        .collect()
}

/// The `show_on_card` custom property values of a card, in schema order.
#[must_use]
pub fn card_extras(board: &Board, card: &Card) -> Vec<SharedString> {
    board
        .properties
        .iter()
        .filter(|schema| schema.show_on_card)
        .filter_map(|schema| {
            card.properties
                .get(&schema.key)
                .map(|value| value.display())
                .filter(|value| !value.is_empty())
                .map(|value| SharedString::from(format!("{}: {value}", schema.name)))
        })
        .collect()
}

/// The facts the board header states, derived once and drawn once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderFacts {
    /// Board name.
    pub name: String,
    /// Identifier prefix, e.g. `FLT`.
    pub prefix: String,
    /// What the backend chip reads: the registry's human label for `board.backend.kind`.
    ///
    /// The chip states which system the board mirrors, and `jira` is the key a config file
    /// uses, not the name the product has. The raw kind stands in only until the registry
    /// answers, because a chip with nothing in it reads as a broken header.
    pub backend: String,
    /// Whether that backend is the local no-remote one.
    pub local: bool,
    /// Relative age of `sync.last_synced_at`, when the board was ever synced.
    pub synced: Option<String>,
    /// Cards with unpushed local edits.
    pub dirty: usize,
    /// Cards with an unresolved conflict.
    pub conflicts: usize,
    /// The board's last sync error, verbatim.
    pub error: Option<String>,
}

impl HeaderFacts {
    /// Reads the facts out of a loaded board.
    ///
    /// `backend_label` is the registry's name for this board's backend kind; `None` falls back
    /// to the kind itself, which is what the first frames of a connection have.
    #[must_use]
    pub fn of(view: &BoardView, backend_label: Option<&str>, now: i64) -> Self {
        Self {
            name: view.board.name.clone(),
            prefix: view.board.prefix.clone(),
            backend: backend_label
                .map_or_else(|| view.board.backend.kind.clone(), ToOwned::to_owned),
            local: view.board.backend.is_local(),
            synced: view
                .board
                .sync
                .last_synced_at
                .as_deref()
                .map(|at| crate::presentation::age_label(at, now)),
            // Archived cards are in no column and reachable by no key: counting them would
            // put a chip in the header for rows the board never shows.
            dirty: view
                .cards
                .iter()
                .filter(|card| !card.archived && card.dirty)
                .count(),
            conflicts: view
                .cards
                .iter()
                .filter(|card| !card.archived && card.conflict.is_some())
                .count(),
            error: view.board.sync.last_error.clone(),
        }
    }
}

// ---------------------------------------------------------------------------- rendering

/// Everything the screen needs to draw itself.
pub struct BoardProps<'a> {
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
pub enum BoardClick {
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
pub fn render(
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
            .children(tiles);
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

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::{
        board::{CardDraft, Label, create_card, new_board},
        ids::{ContextId, LabelId},
        model::Context,
    };

    fn context() -> Context {
        Context {
            id: ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
            name: "Fleet".into(),
            owners: vec![],
            created_at: "2026-09-06T12:00:00Z".into(),
        }
    }

    fn view() -> BoardView {
        let mut board = new_board(&context(), "2026-09-06T12:00:00Z");
        board.labels.push(Label {
            id: LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}")),
            name: "Bug".into(),
            color: Some("danger".into()),
        });
        let mut cards = Vec::new();
        for (index, title) in ["Fix login", "Ship the board", "Polish"].iter().enumerate() {
            let card = create_card(
                &mut board,
                &cards,
                format!("card-{index}")
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                CardDraft {
                    title: (*title).to_owned(),
                    assignee: (index == 1).then(|| "Danny".to_owned()),
                    labels: if index == 0 {
                        vec![LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}"))]
                    } else {
                        Vec::new()
                    },
                    ..CardDraft::default()
                },
                "2026-09-06T12:00:00Z",
            )
            .unwrap_or_else(|error| panic!("{error}"));
            cards.push(card);
        }
        BoardView { board, cards }
    }

    #[test]
    fn the_header_counts_only_the_cards_the_board_shows() {
        let mut view = view();
        view.cards[0].dirty = true;
        view.cards[1].dirty = true;
        view.cards[1].archived = true;
        view.cards[2].archived = true;
        view.cards[2].conflict = Some(fleet_core::board::Conflict {
            detected_at: "2026-09-06T12:00:00Z".into(),
            remote: fleet_core::board::RemoteCard::default(),
            fields: vec!["title".into()],
        });
        let facts = HeaderFacts::of(&view, None, 0);
        // An archived card is in no column and reachable by no key: a chip for one is a chip
        // pointing at nothing.
        assert_eq!((facts.dirty, facts.conflicts), (1, 0));
    }

    #[test]
    fn an_emptied_property_is_no_chip_at_all() {
        let mut view = view();
        view.board
            .properties
            .push(fleet_core::board::PropertySchema {
                key: "teams".into(),
                name: "Teams".into(),
                kind: fleet_core::board::PropertyKind::MultiSelect,
                options: vec![],
                editable: true,
                source: fleet_core::board::PropertySource::Local,
                show_on_card: true,
            });
        view.cards[0].properties.insert(
            "teams".into(),
            fleet_core::board::PropertyValue::MultiSelect(vec![]),
        );
        assert!(card_extras(&view.board, &view.cards[0]).is_empty());
        view.cards[0].properties.insert(
            "teams".into(),
            fleet_core::board::PropertyValue::MultiSelect(vec!["core".into()]),
        );
        assert_eq!(
            card_extras(&view.board, &view.cards[0]),
            vec![SharedString::from("Teams: core")]
        );
    }

    #[test]
    fn the_filter_matches_title_key_label_and_assignee() {
        let view = view();
        let board = &view.board;
        assert!(card_matches(board, &view.cards[0], ""));
        assert!(card_matches(board, &view.cards[0], "LOGIN"));
        assert!(card_matches(board, &view.cards[0], "bug"));
        assert!(card_matches(board, &view.cards[1], "danny"));
        let key = view.cards[0].display_key(board);
        assert!(card_matches(board, &view.cards[0], &key.to_lowercase()));
        assert!(!card_matches(board, &view.cards[2], "zzz"));
    }

    #[test]
    fn a_column_shows_only_its_unarchived_matching_cards() {
        let mut view = view();
        let status = view.board.statuses[1].id.clone();
        for card in &mut view.cards {
            card.status_id = status.clone();
        }
        assert_eq!(visible_cards(&view, &status, "").len(), 3);
        assert_eq!(visible_cards(&view, &status, "polish").len(), 1);
        view.cards[0].archived = true;
        assert_eq!(visible_cards(&view, &status, "").len(), 2);
        assert_eq!(counts(&view, ""), (2, 2));
    }

    #[test]
    fn counts_report_the_whole_board_not_one_column() {
        let view = view();
        assert_eq!(counts(&view, ""), (3, 3));
        assert_eq!(counts(&view, "polish"), (1, 3));
    }

    #[test]
    fn priorities_map_onto_the_kits_levels() {
        assert_eq!(priority_level(Priority::Urgent), PriorityLevel::Urgent);
        assert_eq!(priority_level(Priority::None), PriorityLevel::None);
    }

    #[test]
    fn header_facts_count_dirty_and_conflicted_cards() {
        let mut view = view();
        view.cards[0].dirty = true;
        view.board.sync.last_synced_at = Some("2026-09-06T11:00:00Z".into());
        let facts = HeaderFacts::of(&view, Some("Local"), 1_788_523_200);
        assert_eq!(facts.dirty, 1);
        assert_eq!(facts.conflicts, 0);
        assert!(facts.local);
        assert!(facts.synced.is_some());
    }

    #[test]
    fn label_chips_carry_the_token_name_not_a_color() {
        let view = view();
        let chips = label_chips(&view.board, &view.cards[0]);
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].0.as_ref(), "Bug");
        assert_eq!(chips[0].1.as_deref(), Some("danger"));
    }
}
