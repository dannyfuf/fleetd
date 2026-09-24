//! The Worktrees page (UX-SPEC §3.3): its header, its column heads, and one comfortable row per
//! worktree that answers the pointer as well as the keys.
//!
//! Every word here comes from the prepared [`WorktreeRow`]s ([`model`]); this file only lays
//! them out. Every row action is an action the Hub already handles, so a button, a menu item and
//! a key all run the same code, and each shows its key from the live keymap.

use std::rc::Rc;

use fleet_ui_kit::theme::ch;
use fleet_ui_kit::{
    ActiveTheme, AgeLabel, Button, ButtonSize, ButtonStyle, Chip, ColumnLadder, ContextMenu,
    EmptyState, FilterField, HarnessTargetExt, Icon, IconButton, IconSize, ListHeader, ListPointer,
    ListView, Menu, MenuAnchor, MenuItem, PageHeader, Pane, PaneBorder, PopoverMenu, PrBadge,
    ResolvedColumn, Row, RowColumn, StatusDot, Text, TextInput, Tone, Truncate, truncate,
};
use gpui::{
    Action, AnyElement, App, Entity, IntoElement, SharedString, UniformListScrollHandle, Window,
    div, prelude::*,
};

use crate::{
    action_catalogue,
    actions::{hub, repos, worktrees},
    views::{first_run::EmptySurface, harness},
};

mod model;
#[cfg(test)]
mod tests;

pub use model::{
    DetailPr, GitFacts, KnownGit, KnownPr, NameIcon, RowInputs, SessionWords, WorktreeDetail,
    WorktreeRow, build_rows, matches, sort_rows, summary,
};
pub(crate) use model::{job_targets_worktree, owns_row};

/// Character budget of the `owner/name` column (§2.9 column 2).
const REPO_BUDGET: usize = 12;
/// The page's title.
const TITLE: &str = "Worktrees";
/// The filter field's placeholder.
const FILTER_PLACEHOLDER: &str = "Filter";
/// What a row without a pull request says in that column.
const NO_PR: &str = "\u{2014}";
/// The dirty fact, in words, beside the name.
const DIRTY: &str = "uncommitted changes";
/// The chip a worktree whose post-create hooks failed carries.
const HOOKS_FAILED: &str = "Setup hook failed";
/// The button beside it.
const VIEW_LOG: &str = "View log";
/// The widest a row's name grows before it ellipsizes, in `ch`.
const NAME_MAX_CH: f32 = 28.0;
/// Of `worktrees::Open`'s keys, the one the page teaches (`Open ⏎`); `o` still works.
pub(crate) const OPEN_KEY: &str = "enter";
/// The accessible name of a row's `⋯` trigger.
const MORE_ACTIONS: &str = "More actions";

/// The catalogue's short label for an action: what a button or a menu item reads.
pub(crate) fn label(action: &dyn Action) -> &'static str {
    action_catalogue::info(action.name()).map_or("", |info| info.short_label)
}

/// A row-indexed handler: `(row index, window, cx)`.
pub type RowHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// What a row does when the pointer uses it. Each handler first puts the cursor on the row.
#[derive(Clone)]
pub struct RowHandlers {
    /// Move the cursor to the row (a click, or the press before any button on it).
    pub select: RowHandler,
    /// Open the row's worktree, like `⏎` (a double-click).
    pub open: RowHandler,
    /// Open the row's failed hooks job in Jobs (`View log`).
    pub view_log: RowHandler,
}

/// The filter, as the page header draws it.
pub enum FilterSlot {
    /// No filter, or a retained query (§3.10 stage two), shown in the idle field.
    Idle(Option<SharedString>),
    /// The live editor owns the keyboard, with `shown/total`.
    Editing {
        /// The Hub's one filter editor.
        input: Entity<TextInput>,
        /// Its query, for the no-match empty state.
        query: SharedString,
        /// Rows shown.
        shown: usize,
        /// Rows before the filter.
        total: usize,
    },
}

/// Everything the page needs to draw itself.
pub struct ListProps<Rows> {
    /// The rows, already sorted and filtered.
    pub rows: Rows,
    /// Cursor index into `rows`.
    pub cursor: usize,
    /// Whether the list owns the keyboard.
    pub focused: bool,
    /// The pane's width in `ch`, which resolves the §2.9 ladder.
    pub pane_ch: f32,
    /// The selected repository's name, or `None` in `All` scope (which forces the repo column).
    pub scope_repo: Option<SharedString>,
    /// The subtitle the model built.
    pub summary: SharedString,
    /// The filter field.
    pub filter: FilterSlot,
    /// `<age>` when the snapshot is frozen (§1.3).
    pub stale: Option<SharedString>,
    /// Whether the daemon has not sent a snapshot yet (§3.13 cold load).
    pub loading: bool,
    /// Whether the context holds a repository. Without one there is nothing to branch from, so
    /// the page leads with `Clone repo` and offers no `New worktree` (§3.13 step 2).
    pub has_repos: bool,
    /// Whether `u` has a delete to undo, so the menus offer it.
    pub undo_available: bool,
    /// The pointer contract.
    pub handlers: RowHandlers,
}

/// Renders the Worktrees page: header, column heads, rows and the §3.13 empty states.
///
/// `age_offset` ages the prepared rows by the seconds elapsed since the projection was built,
/// so a clock tick re-labels the age column without rebuilding the model.
#[must_use]
pub fn render(
    props: ListProps<impl AsRef<[WorktreeRow]> + 'static>,
    scroll: &UniformListScrollHandle,
    age_offset: i64,
    cx: &App,
) -> AnyElement {
    let ListProps {
        rows,
        cursor,
        focused,
        pane_ch,
        scope_repo,
        summary,
        filter,
        stale,
        loading,
        has_repos,
        undo_available,
        handlers,
    } = props;
    let theme = cx.theme();

    let query = match &filter {
        FilterSlot::Idle(query) => query.clone(),
        FilterSlot::Editing { query, .. } => Some(query.clone()).filter(|query| !query.is_empty()),
    };
    // While fleetd is gone the rows are true but frozen: they stay navigable, drawn at the stale
    // opacity under the header's one `Stale · <age>` chip (§3.12 C).
    let frozen = stale.is_some();
    let header = page_header(summary, filter, stale, has_repos);
    let columns = columns(pane_ch, scope_repo.is_none());
    let heads = columns.iter().fold(ListHeader::new(), |header, column| {
        header.column(column, column_head(column.key.as_ref()))
    });

    let empty = if loading {
        EmptyState::new("Loading\u{2026}").into_any_element()
    } else if let Some(query) = &query {
        EmptySurface::Filter.render(Some(query))
    } else if !has_repos {
        EmptySurface::WorktreesNoRepos.render(None)
    } else {
        match &scope_repo {
            None => EmptySurface::Worktrees.render(None),
            Some(repo) => EmptySurface::WorktreesRepo.render(Some(repo)),
        }
    };

    let pointer = ListPointer::new()
        .on_select({
            let select = handlers.select.clone();
            move |ix, window, cx| select(ix, window, cx)
        })
        .on_open({
            let open = handlers.open.clone();
            move |ix, window, cx| open(ix, window, cx)
        })
        // The right-click selects; the row's `ContextMenu` opens the menu itself.
        .on_menu(|_, _, _, _| {});
    let row_count = rows.as_ref().len();
    let list = ListView::new(
        "hub-worktrees",
        row_count,
        move |index, is_cursor, _window, cx| {
            let Some(row) = rows.as_ref().get(index) else {
                return div().into_any_element();
            };
            worktree_row(
                row,
                RowContext {
                    ix: index,
                    is_cursor,
                    focused,
                    age_offset,
                    undo_available,
                    columns: &columns,
                    pointer: &pointer,
                    handlers: &handlers,
                },
                cx,
            )
        },
    )
    .row_height(theme.metrics.row_h_comfortable)
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);

    let body = div()
        .flex()
        .flex_col()
        .size_full()
        .child(
            div()
                .flex_none()
                .px(theme.space.xl)
                .pt(theme.space.xl)
                .pb(theme.space.lg)
                .child(header),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .px(theme.space.md)
                .child(heads)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .pt(theme.space.xs)
                        .when(frozen, |el| el.opacity(theme.metrics.stale_opacity))
                        .child(list),
                ),
        );

    Pane::new()
        .border(PaneBorder::None)
        .focused(focused)
        .body(body)
        .into_any_element()
}

/// The page header: `Worktrees`, the summary, the filter field, `Clone repo` and the primary
/// `New worktree` — or, while the context holds no repository, a primary `Clone repo` alone.
fn page_header(
    summary: SharedString,
    filter: FilterSlot,
    stale: Option<SharedString>,
    has_repos: bool,
) -> PageHeader {
    let field = match filter {
        FilterSlot::Idle(query) => {
            let field = FilterField::new("worktrees-filter", FILTER_PLACEHOLDER)
                .action(Box::new(hub::OpenFilter));
            let field = match query {
                Some(query) => field.query(query),
                None => field,
            };
            field.harness_target("worktrees.filter").into_any_element()
        }
        FilterSlot::Editing {
            input,
            shown,
            total,
            ..
        } => FilterField::new("worktrees-filter", FILTER_PLACEHOLDER)
            .editor(input)
            .counts(shown, total)
            .harness_target("filter.input")
            .into_any_element(),
    };
    let clone = Button::new("worktrees-clone", label(&repos::Clone)).action(Box::new(repos::Clone));
    let mut header = PageHeader::new(TITLE).subtitle(summary).action(field);
    header = if has_repos {
        header
            .action(clone.harness_target("worktrees.clone"))
            .action(
                Button::new("worktrees-new", label(&worktrees::Create))
                    .icon(Icon::Plus)
                    .style(ButtonStyle::Primary)
                    .action(Box::new(worktrees::Create))
                    .harness_target("worktrees.new"),
            )
    } else {
        // Step 2 of §3.13: a worktree needs a repository, so the one way forward is the clone.
        header.action(
            clone
                .icon(Icon::Plus)
                .style(ButtonStyle::Primary)
                .harness_target("worktrees.clone"),
        )
    };
    if let Some(age) = stale {
        header = header.stale(age);
    }
    header
}

/// The primary button of the empty page: the one way forward.
pub(crate) fn new_worktree_button() -> Button {
    Button::new("worktrees-empty-new", label(&worktrees::Create))
        .icon(Icon::Plus)
        .style(ButtonStyle::Primary)
        .action(Box::new(worktrees::Create))
}

/// The head of one column, in sentence case. The actions column has none.
fn column_head(key: &str) -> &'static str {
    match key {
        "branch" => "Name",
        "repo" => "Repository",
        "session" => "Session",
        "pr" => "Pull request",
        "age" => "Age",
        _ => "",
    }
}

/// Resolve columns with the repository cell forced on in All scope.
#[must_use]
pub fn columns(pane_ch: f32, scope_is_all: bool) -> Vec<ResolvedColumn> {
    ColumnLadder::worktrees_in_scope(scope_is_all).resolve(pane_ch)
}

/// Every row action, in the order the ⋯ menu and the right-click menu list them. An item is
/// left out when nothing can run it, so `Undo delete` appears only while there is a delete to
/// undo.
pub(crate) fn row_menu(menu: Menu, undo_available: bool) -> Menu {
    let item = |action: Box<dyn Action>| MenuItem::new(label(action.as_ref())).action(action);
    let menu = menu
        .item(item(Box::new(worktrees::Open)))
        .item(item(Box::new(worktrees::OpenKeepAwake)))
        .item(item(Box::new(worktrees::Sleep)))
        .item(item(Box::new(worktrees::Inspect)))
        .separator()
        .item(item(Box::new(worktrees::CopyPath)))
        .item(item(Box::new(worktrees::CopyBranch)))
        .separator()
        .item(item(Box::new(worktrees::Kill)).destructive(true))
        .item(item(Box::new(worktrees::Delete)).destructive(true));
    if undo_available {
        menu.item(item(Box::new(worktrees::UndoDelete)))
    } else {
        menu
    }
}

/// Everything one row needs besides its data.
struct RowContext<'a> {
    ix: usize,
    is_cursor: bool,
    focused: bool,
    age_offset: i64,
    undo_available: bool,
    columns: &'a [ResolvedColumn],
    pointer: &'a ListPointer,
    handlers: &'a RowHandlers,
}

/// One comfortable row, built strictly from the resolved §2.9 columns, with its right-click
/// menu around it.
fn worktree_row(row: &WorktreeRow, ctx: RowContext<'_>, cx: &App) -> AnyElement {
    let ix = ctx.ix;
    let mut element = Row::with_id(("wt-row", ix))
        .comfortable()
        .selected(ctx.is_cursor)
        .cursor(ctx.is_cursor && ctx.focused)
        .dimmed(row.deleting)
        .disabled(row.deleting);

    for column in ctx.columns {
        let cell = match column.key.as_ref() {
            "branch" => RowColumn::resolved(column, name_cell(row, &ctx, cx)),
            "repo" => RowColumn::resolved(
                column,
                Text::ui(truncate(
                    row.repo_label.as_ref(),
                    REPO_BUDGET,
                    Truncate::Head,
                ))
                .muted(),
            ),
            "session" => RowColumn::resolved(column, session_cell(row, cx)),
            "pr" => RowColumn::resolved(
                column,
                row.pr.map_or_else(
                    || Text::ui(NO_PR).faint().into_any_element(),
                    |(number, state)| {
                        PrBadge::new(number, state)
                            .chip()
                            .stale(row.marks_stale(ctx.age_offset))
                            .into_any_element()
                    },
                ),
            ),
            "age" => RowColumn::resolved(column, age_cell(row, ctx.age_offset, cx)),
            "actions" => {
                RowColumn::resolved(column, hover_actions(ix, ctx.undo_available, cx)).hover_only()
            }
            _ => continue,
        };
        element = element.column(cell);
    }
    let undo_available = ctx.undo_available;
    ContextMenu::new(
        ("wt-menu", ix),
        ctx.pointer
            .attach(ix, element)
            .harness_target_indexed("worktrees.row", ix),
    )
    .menu(move |menu, _, _| row_menu(menu, undo_available))
    .into_any_element()
}

/// `⑂ spike  ↑2`, `⑂ hotfix  uncommitted changes`, `⚠ broken  [Setup hook failed] View log`.
fn name_cell(row: &WorktreeRow, ctx: &RowContext<'_>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let ix = ctx.ix;
    let dirty_opacity = if row.marks_stale(ctx.age_offset) {
        theme.metrics.stale_opacity
    } else {
        1.0
    };
    let icon = row
        .name_icon
        .icon
        .el()
        .size(IconSize::Medium)
        .tone(row.name_icon.tone)
        .spinning(row.name_icon.spins)
        .id(("wt-name-icon", ix));
    let view_log = row.degraded.then(|| {
        let handler = ctx.handlers.view_log.clone();
        Button::new(("wt-log", ix), VIEW_LOG)
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .on_click(move |_, window, cx| handler(ix, window, cx))
            .harness_target(harness::name(|| format!("worktrees.row[{ix}].log")))
    });
    div()
        .flex()
        .items_center()
        .min_w_0()
        .gap(theme.space.sm)
        .child(icon)
        // The name keeps its room first: the facts after it clip before the name gives way,
        // and a long name ellipsizes at its own budget.
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .max_w(ch(NAME_MAX_CH))
                .child(Text::ui_strong(row.branch.clone()).ellipsize()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .gap(theme.space.sm)
                .children(
                    row.ahead
                        .clone()
                        .map(|ahead| Text::data_small(ahead).muted().flex_none()),
                )
                .when(row.dirty, |el| {
                    el.child(
                        Text::caption(DIRTY)
                            .muted()
                            .opacity(dirty_opacity)
                            .flex_none(),
                    )
                })
                .children(row.host.clone().map(|host| {
                    let (icon, tone) = host_badge(
                        row.host_provider.as_deref(),
                        row.host_link,
                        row.host_unreachable,
                    );
                    Chip::labeled(icon, host).tone(tone)
                }))
                .when(row.degraded, |el| {
                    el.child(
                        Chip::new()
                            .text(HOOKS_FAILED)
                            .tone(Tone::Warning)
                            .filled(true),
                    )
                })
                .children(view_log),
        )
        .into_any_element()
}

/// The session column: a job's phase, else the session in words behind its dot.
fn session_cell(row: &WorktreeRow, cx: &App) -> AnyElement {
    if let Some(phase) = row.phase.clone() {
        return Text::ui(phase).muted().ellipsize().into_any_element();
    }
    let theme = cx.theme();
    let words = &row.session;
    div()
        .flex()
        .items_center()
        .min_w_0()
        .gap(theme.space.sm)
        .children(words.dot.map(StatusDot::small))
        .child(
            Text::ui(words.text.clone())
                .tone(if words.quiet {
                    Tone::Muted
                } else {
                    Tone::Secondary
                })
                .ellipsize(),
        )
        .into_any_element()
}

/// The row's hover actions: `Open ⏎` and the `⋯` menu, the same verbs its keys run.
fn hover_actions(ix: usize, undo_available: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(theme.space.xxs)
        .child(
            Button::new(("wt-open", ix), label(&worktrees::Open))
                .size(ButtonSize::Compact)
                .action(Box::new(worktrees::Open))
                .prefer_key(OPEN_KEY)
                .harness_target(harness::name(|| format!("worktrees.row[{ix}].open"))),
        )
        .child(
            PopoverMenu::new(("wt-more", ix))
                .anchor(MenuAnchor::BottomRight)
                .trigger_with(move |open, _, _| {
                    IconButton::new(("wt-more-trigger", ix), Icon::Ellipsis, MORE_ACTIONS)
                        .size(ButtonSize::Compact)
                        .selected(open)
                })
                .menu(move |menu, _, _| row_menu(menu, undo_available))
                .harness_target(harness::name(|| format!("worktrees.row[{ix}].menu"))),
        )
        .into_any_element()
}

/// The host chip's icon and tone: provider-aware, and link-aware on top of it.
///
/// `cloud-off` amber is the UX-SPEC §Host chip rule for an unreachable host, and a dropped
/// daemon link reads the same way to the user. A `command` machine is not a cloud, and a
/// legacy probe-only entry has no daemon link at all, so both get their own glyph.
#[must_use]
pub fn host_badge(
    provider: Option<&str>,
    link: Option<fleet_proto::snapshot::LinkState>,
    unreachable: bool,
) -> (Icon, Tone) {
    use fleet_proto::snapshot::LinkState;
    if unreachable || link == Some(LinkState::Down) {
        return (Icon::CloudOff, Tone::Warning);
    }
    if link == Some(LinkState::Legacy) || provider == Some("legacy") {
        return (Icon::Unplug, Tone::Warning);
    }
    let icon = if provider == Some("command") {
        Icon::Server
    } else {
        Icon::Cloud
    };
    let tone = if link == Some(LinkState::Connecting) {
        Tone::Muted
    } else {
        Tone::Secondary
    };
    (icon, tone)
}

/// The age column: the age, or nothing while a job phase owns the row; an errored inspection
/// prefixes an amber `triangle-alert` and the row stays operable (§3.3).
fn age_cell(row: &WorktreeRow, age_offset: i64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label = match (row.phase.is_some(), row.age) {
        (false, Some(seconds)) => AgeLabel::from_secs(seconds.saturating_add(age_offset).max(0)),
        _ => AgeLabel::none(),
    };
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(theme.space.xxs)
        .when(row.inspect_error, |el| {
            el.child(
                Icon::TriangleAlert
                    .el()
                    .size(IconSize::Small)
                    .tone(Tone::Warning),
            )
        })
        .child(label)
        .into_any_element()
}
