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

use std::{collections::HashMap, rc::Rc, sync::Arc};

use fleet_core::agents::AgentKind;
use fleet_core::board::{
    Action as ColumnAction, ActionKind, Board, BoardView, Card, CardRun, Priority, Status,
    StatusCategory, column_cards as ops_column_cards, is_satisfied,
};
use fleet_core::ids::{CardId, StatusId, WorktreeId};
use fleet_ui_kit::{
    ActiveTheme, Badge, BlockedTone, Button, ButtonSize, ButtonStyle, Callout, CardTile, Chip,
    ContextMenu, EmptyState, FilterField, HarnessTargetExt, Icon, IconButton, KanbanBoard,
    KanbanColumn, Menu, MenuAnchor, MenuItem, PageHeader, Pane, PaneBorder, PaneHeader,
    PopoverMenu, PrBadgeState, PriorityLevel, RunMark, SkeletonRows, SpinnerWithLabel, Text, Theme,
    Tone,
};
use gpui::{
    Action, AnyElement, App, Context, ElementId, Entity, Hsla, ListState, MouseButton,
    ScrollHandle, SharedString, Window, div, prelude::*,
};

use crate::{action_catalogue, actions::board as board_actions};

mod model;
#[cfg(test)]
mod tests;

pub(crate) use model::category_accent;
#[cfg(test)]
use model::priority_level;
pub use model::{
    BoardMarks, BoardModel, CardMenu, CardRow, ColumnRows, HeaderFacts, LinkedBranch,
    ReadonlyFields, TileMark, build, counts, visible_cards,
};

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
    /// The board screen's live filter editor.
    pub filter_input: Entity<fleet_ui_kit::TextInput>,
    /// The focused column and card.
    pub focus: (usize, usize),
    /// Whether a `board.sync` job is running for this board.
    pub syncing: bool,
    /// Whether this surface carries the run keys (`A`, `X`, `>`): a worktree's board.
    pub runs: bool,
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
    /// Open *New card* with this column's status chosen: the column's `+` and `Add card`.
    AddCard(usize),
    /// Open board settings on this column: its automation pill.
    ColumnSettings(usize),
    /// Clear the filter query: the filter field's ✕.
    ClearFilter,
}

/// What the header's filter field reads while it holds no query.
const FILTER_PLACEHOLDER: &str = "Filter cards";

/// The screen's click handler: the screen owns turning a click's intent into state.
type OnClick = std::rc::Rc<dyn Fn(BoardClick, &mut App)>;

/// The catalogue's short label for an action: what a button or a menu item reads.
fn label(action: &dyn Action) -> &'static str {
    action_catalogue::info(action.name()).map_or("", |info| info.short_label)
}

/// Renders the board pane: header, then columns of tiles.
///
/// `on_click` is called with the intent of a click; the screen owns turning that into state.
pub(crate) fn render(
    props: &BoardProps<'_>,
    board_scroll: &ScrollHandle,
    column_lists: &[ListState],
    on_click: impl Fn(BoardClick, &mut App) + 'static,
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
    let on_click: OnClick = std::rc::Rc::new(on_click);
    let theme = cx.theme();

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
        columns(props, model, board_scroll, column_lists, &on_click, cx)
    };

    let local = model.facts.local;
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .child(header(props, model, &on_click, cx))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(theme.space.sm)
                .px(theme.space.xl)
                .children(props.error.map(|message| {
                    Callout::new(Tone::Danger, Icon::CircleX, message.to_owned()).actions(
                        action_button("board-error-reload", Box::new(board_actions::Reload)),
                    )
                }))
                .children(
                    model
                        .facts
                        .error
                        .clone()
                        .filter(|_| props.error.is_none())
                        .map(|message| {
                            Callout::new(
                                Tone::Danger,
                                Icon::CloudOff,
                                format!("Sync failed: {message}"),
                            )
                            .actions(action_button(
                                "board-sync-error-settings",
                                Box::new(board_actions::Settings),
                            ))
                        }),
                )
                .children(model.orphans.clone().map(|message| {
                    Callout::new(Tone::Warning, Icon::TriangleAlert, message).actions(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space.xs)
                            .child(action_button(
                                "board-orphans-settings",
                                Box::new(board_actions::Settings),
                            ))
                            .children((!local).then(|| {
                                action_button("board-orphans-sync", Box::new(board_actions::Sync))
                            })),
                    )
                })),
        )
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// A compact secondary button running `action`, labelled from the catalogue.
fn action_button(id: &'static str, action: Box<dyn Action>) -> Button {
    Button::new(id, label(action.as_ref()))
        .size(ButtonSize::Compact)
        .action(action)
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
    let theme = cx.theme();
    if let Some(message) = props.error {
        return div()
            .flex()
            .flex_col()
            .size_full()
            .gap(theme.space.md)
            .p(theme.space.xl)
            .child(
                Callout::new(Tone::Danger, Icon::CircleX, message.to_owned()).actions(
                    action_button("board-error-reload", Box::new(board_actions::Reload)),
                ),
            )
            .child(EmptyState::new("The board could not be loaded.").action("r  reload"))
            .into_any_element();
    }
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
                .id(("board-skeleton", index))
                .child(SkeletonRows::new(SKELETON_ROWS))
        }))
        .into_any_element()
}

/// The board header: the name and what the board is doing on the left, its actions on the right.
///
/// `Fleet board FLT` over `8 cards · 1 of 2 runs working · 1 needs you`; then the sync button
/// (`Jira · synced 2m  S`) and its ⋯, the filter, `Board settings  ,` and its ⋯, and the one
/// primary action, `New card  c`. Every control dispatches the action its key runs.
fn header(props: &BoardProps<'_>, model: &BoardModel, on_click: &OnClick, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let facts = &model.facts;
    let needs_you = facts.needs_you_label.clone().map(|text| {
        let target = model.needs_you_at;
        let click = on_click.clone();
        div()
            .id("board-needs-you")
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .when(target.is_some(), |el| el.cursor_pointer())
            .when_some(target, move |el, (column, row)| {
                el.on_click(move |_, _, cx| click(BoardClick::Card(column, row), cx))
            })
            .child(fleet_ui_kit::StatusDot::small(Tone::Warning))
            .child(Text::caption(text).tone(Tone::Warning))
    });
    let dot = || Text::caption("\u{b7}").faint();
    let mut page = PageHeader::new(facts.name.clone())
        .badge(Badge::new(facts.prefix.clone()))
        .subtitle(model.summary.clone());
    if let Some(needs_you) = needs_you {
        page = page.fact(dot()).fact(needs_you);
    }
    if facts.dirty > 0 {
        page = page.fact(
            Chip::counter(Icon::CloudUpload, facts.dirty)
                .tone(Tone::Warning)
                .id("board-dirty"),
        );
    }
    if facts.conflicts > 0 {
        page = page.fact(
            Chip::counter(Icon::TriangleAlert, facts.conflicts)
                .tone(Tone::Danger)
                .id("board-conflicts"),
        );
    }
    if props.loading && !props.syncing {
        page = page.fact(SpinnerWithLabel::new("board-refresh", "refreshing"));
    }

    // The backend's words: `Jira · synced 2m`. A local board mirrors nothing, so it has no sync
    // button — only the ⋯ that still holds Reload.
    let sync_text = match (&facts.synced, facts.local) {
        (_, true) => None,
        (Some(age), false) => Some(format!("{} \u{b7} synced {age}", facts.backend)),
        // A remote board with no stamp has never synced, and omitting the words leaves it
        // looking exactly like one synced seconds ago. The CLI says so in the same place.
        (None, false) => Some(format!("{} \u{b7} never synced", facts.backend)),
    };
    let sync = sync_text.map(|text| {
        let button = Button::new("board-sync", text)
            .icon(if facts.error.is_some() {
                Icon::CloudOff
            } else {
                Icon::Cloud
            })
            .style(ButtonStyle::Ghost)
            .action(Box::new(board_actions::Sync))
            .harness_target("board.sync");
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .children(
                props
                    .syncing
                    .then(|| SpinnerWithLabel::new("board-sync-spinner", "syncing")),
            )
            .child(button)
    });
    let local = facts.local;
    let more_sync = PopoverMenu::new("board-sync-more")
        .anchor(MenuAnchor::BottomRight)
        .trigger_with(move |open, _, _| {
            IconButton::new(
                "board-sync-more-trigger",
                Icon::Ellipsis,
                if local {
                    "More board actions"
                } else {
                    "More sync actions"
                },
            )
            .selected(open)
        })
        .menu(move |menu, _, _| {
            let menu = if local {
                menu
            } else {
                menu.item(menu_item(Box::new(board_actions::FullSync)))
            };
            menu.item(menu_item(Box::new(board_actions::Reload)))
        });
    // One box, two faces: the retained query while idle, the live editor with `shown/total`
    // while `/` has the keyboard. The ✕ clears the query in either face.
    let clear = on_click.clone();
    let filter = FilterField::new("board-filter", FILTER_PLACEHOLDER)
        .action(Box::new(board_actions::Filter))
        .on_clear(move |_, cx| clear(BoardClick::ClearFilter, cx))
        .map(|field| {
            if props.filter_editing {
                field
                    .editor(props.filter_input.clone())
                    .counts(model.shown, model.total)
            } else {
                field.query(props.filter.to_owned())
            }
        })
        .harness_target("board.filter");
    let settings = Button::new("board-settings", label(&board_actions::Settings))
        .action(Box::new(board_actions::Settings))
        .harness_target("board.settings");
    let more_settings = PopoverMenu::new("board-settings-more")
        .anchor(MenuAnchor::BottomRight)
        .trigger_with(|open, _, _| {
            IconButton::new(
                "board-settings-more-trigger",
                Icon::Ellipsis,
                "More board settings",
            )
            .selected(open)
        })
        .menu(|menu, _, _| menu.item(menu_item(Box::new(board_actions::Columns))));
    let new_card = Button::new("board-new-card", label(&board_actions::NewCard))
        .icon(Icon::Plus)
        .style(ButtonStyle::Primary)
        .action(Box::new(board_actions::NewCard))
        .harness_target("board.new");

    div()
        .flex_none()
        .px(theme.space.xl)
        .pt(theme.space.lg)
        .pb(theme.space.md)
        .child(
            page.map(|page| match sync {
                Some(sync) => page.action(sync),
                None => page,
            })
            .action(more_sync)
            .action(filter)
            .action(settings)
            .action(more_settings)
            .action(new_card),
        )
        .into_any_element()
}

/// One menu entry running `action`, labelled from the catalogue, its key from the live keymap.
fn menu_item(action: Box<dyn Action>) -> MenuItem {
    let destructive = action_catalogue::info(action.name()).is_some_and(|info| info.destructive);
    MenuItem::new(label(action.as_ref()))
        .destructive(destructive)
        .action(action)
}

/// Every action a card's ⋯ and right-click menu holds, in the card's order, leaving out what
/// cannot work for it (the rules the keys refuse by, answered in the model).
fn card_menu(menu: Menu, facts: CardMenu, at: TileContext) -> Menu {
    let TileContext {
        readonly,
        runs,
        edges: (first, last),
        ..
    } = at;
    let item = |action: Box<dyn Action>| menu_item(action);
    let mut menu = menu;
    if !readonly.status {
        menu = menu.item(item(Box::new(board_actions::PickStatus)));
    }
    if !readonly.priority {
        menu = menu.item(item(Box::new(board_actions::PickPriority)));
    }
    if !readonly.assignee {
        menu = menu.item(item(Box::new(board_actions::PickAssignee)));
    }
    if !readonly.labels {
        menu = menu.item(item(Box::new(board_actions::PickLabels)));
    }
    if !readonly.estimate {
        menu = menu.item(item(Box::new(board_actions::PickEstimate)));
    }
    menu = menu
        .item(item(Box::new(board_actions::PickBlockedBy)))
        .item(item(Box::new(board_actions::PickAgent)))
        .separator()
        .item(item(Box::new(board_actions::CreateWorktree)));
    if facts.open_worktree {
        menu = menu.item(item(Box::new(board_actions::OpenWorktree)));
    }
    if facts.open_remote {
        menu = menu.item(item(Box::new(board_actions::OpenRemote)));
    }
    if !readonly.status && !first {
        menu = menu.item(item(Box::new(board_actions::MovePrevColumn)));
    }
    if !readonly.status && !last {
        menu = menu.item(item(Box::new(board_actions::MoveNextColumn)));
    }
    if runs && (facts.attach || facts.cancel || facts.run_now) {
        menu = menu.separator();
        if facts.attach {
            menu = menu.item(item(Box::new(board_actions::AttachRun)));
        }
        if facts.run_now {
            menu = menu.item(item(Box::new(board_actions::RunNow)));
        }
        if facts.cancel {
            menu = menu.item(item(Box::new(board_actions::CancelRun)));
        }
    }
    if facts.delete {
        menu = menu
            .separator()
            .item(item(Box::new(board_actions::DeleteCard)));
    }
    menu
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
    on_click: &OnClick,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let (focus_column, focus_row) = props.focus;
    let empty_hint = if props.filter.is_empty() {
        "No cards"
    } else {
        "No match here"
    };
    let runs = props.runs;
    let readonly = model.readonly;
    let last_column = model.columns.len().saturating_sub(1);
    let columns: Vec<AnyElement> = model
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let focused = index == focus_column;
            let add_click = on_click.clone();
            let add = IconButton::new(
                ("board-column-add", index),
                Icon::Plus,
                column.add_label.clone(),
            )
            .size(ButtonSize::Compact)
            .on_click(move |_, _, cx| add_click(BoardClick::AddCard(index), cx))
            .harness_target(crate::views::harness::name(|| {
                format!("board.column[{index}].add")
            }));
            let footer_click = on_click.clone();
            let footer = Button::new(("board-column-add-row", index), "Add card")
                .icon(Icon::Plus)
                .style(ButtonStyle::Ghost)
                .size(ButtonSize::Compact)
                .on_click(move |_, _, cx| footer_click(BoardClick::AddCard(index), cx));
            let mut kanban =
                KanbanColumn::new(column.element_id.clone(), column.status.name.clone())
                    .count(column.rows.len())
                    .accent(Some(category_accent(&column.status, &theme)))
                    .focused(focused)
                    .empty_hint(empty_hint)
                    .add_button(add)
                    .footer(footer);
            if let Some(automation) = column.automation.clone() {
                let click = on_click.clone();
                kanban = kanban
                    .automation(automation)
                    .on_automation_click(move |_, _, cx| {
                        click(BoardClick::ColumnSettings(index), cx);
                    });
            }
            if let Some(list) = column_lists.get(index) {
                let rows = Rc::clone(&column.rows);
                let click = on_click.clone();
                let edges = (index == 0, index == last_column);
                kanban = kanban.rows(list.clone(), rows.len(), move |row, _window, _cx| {
                    let Some(card) = rows.get(row) else {
                        return div().into_any_element();
                    };
                    let tile = TileContext {
                        column: index,
                        row,
                        selected: focused && row == focus_row,
                        runs,
                        readonly,
                        edges,
                    };
                    tile_with_menu(card, tile, &click)
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
                .harness_target_indexed("board.column", index)
                .into_any_element()
        })
        .collect();

    KanbanBoard::new("board-columns")
        .scroll_handle(board_scroll.clone())
        .columns(columns)
        .into_any_element()
}

/// Where one tile sits and what its menu may offer.
#[derive(Debug, Clone, Copy)]
struct TileContext {
    column: usize,
    row: usize,
    selected: bool,
    runs: bool,
    readonly: ReadonlyFields,
    /// Whether the column is the first and the last: `[` and `]` have nowhere to go there.
    edges: (bool, bool),
}

/// One prepared card as its tile, wrapped in its right-click menu, with the ⋯ that opens the
/// same menu (UX-SPEC §5.1: a right-click menu is never the only way to an action).
///
/// Both menus act on the selected card like its keys do: a press on the tile selects it before
/// either menu opens, and every entry dispatches the card action to the board.
fn tile_with_menu(card: &CardRow, at: TileContext, on_click: &OnClick) -> AnyElement {
    let (column, row) = (at.column, at.row);
    let facts = card.menu;
    let builder =
        move |menu: Menu, _: &mut Window, _: &mut Context<Menu>| card_menu(menu, facts, at);
    let card_id = Arc::new(ElementId::from(card.element_id.clone()));
    let child = |part: &'static str| {
        ElementId::NamedChild(Arc::clone(&card_id), SharedString::new_static(part))
    };
    let more = PopoverMenu::new(child("more"))
        .anchor(MenuAnchor::BottomRight)
        .trigger_with({
            let trigger = child("more-trigger");
            move |open, _, _| {
                IconButton::new(trigger, Icon::Ellipsis, "Card actions")
                    .size(ButtonSize::Compact)
                    .selected(open)
            }
        })
        .menu(builder)
        .harness_target(crate::views::harness::name(|| {
            format!("board.column[{column}].card[{row}].menu")
        }));
    let tile = tile(card, at, more, on_click).harness_target(crate::views::harness::name(|| {
        format!("board.column[{column}].card[{row}]")
    }));
    ContextMenu::new(child("context"), tile)
        .menu(builder)
        .into_any_element()
}

/// One prepared card as its tile.
fn tile(card: &CardRow, at: TileContext, more: impl IntoElement, on_click: &OnClick) -> CardTile {
    let (column, row) = (at.column, at.row);
    let select = on_click.clone();
    let open = on_click.clone();
    let secondary = on_click.clone();
    // `Answer` is the needs-you card's own button: the run's thread, attached as a tab. It exists
    // where the run keys do — a worktree's board — and first selects the card it sits on.
    let answer = (at.runs && card.run == Some(RunMark::NeedsYou)).then(|| {
        let select = on_click.clone();
        Button::new(("board-card-answer", row), "Answer")
            .style(ButtonStyle::Secondary)
            .size(ButtonSize::Compact)
            .on_click(move |_, _, cx| select(BoardClick::Card(column, row), cx))
            .action(Box::new(board_actions::AttachRun))
    });
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
    .branch(card.link.as_ref().map(|link| link.branch.clone()))
    .pr(card.link.as_ref().and_then(|link| link.pr))
    .dirty(card.dirty)
    .conflict(card.conflict)
    // The tile decides nothing: which of the two the key line carries is the kit's own
    // precedence rule, and what each of them says was folded once, in the update path.
    .when_some(card.run, CardTile::run)
    .when_some(card.run_label.clone(), CardTile::run_label)
    .when_some(card.blocked, |tile, (count, tone)| {
        tile.blocked(count, tone)
    })
    .when_some(card.blocked_label.clone(), CardTile::blocked_label)
    .selected(at.selected)
    .focused(at.selected)
    .extras(card.extras.clone())
    .when_some(answer, CardTile::action)
    .menu(more)
    .on_click(move |_, _, cx| select(BoardClick::Card(column, row), cx))
    .on_double_click(move |_, _, cx| open(BoardClick::OpenCard(column, row), cx))
    .on_secondary_click(move |_, _, cx| secondary(BoardClick::Card(column, row), cx))
}
