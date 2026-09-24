//! The pull requests screen (UX-SPEC §3.5): its page header, the Mine / Waiting-for-my-review
//! tabs, and the list of prepared rows.
//!
//! Rows are prepared in the update path ([`build_rows`], memoised by the Hub's projection) with
//! every word they draw; a frame only copies those words into elements. Every button dispatches
//! the action its key runs and shows that key from the live keymap; a press anywhere on a row
//! first puts the cursor on it, so the action acts on the row the pointer is on.

use std::collections::{HashMap, HashSet};

use std::rc::Rc;

use fleet_core::{
    github::{PrTab, PullRequest},
    ids::{HostId, RepoId, WorktreeId},
    model::Worktree,
};
use fleet_proto::response::PrSlice;
use fleet_proto::snapshot::LinkState;
use fleet_ui_kit::{
    ActiveTheme, AgeLabel, Button, ButtonSize, ButtonStyle, Callout, Chip, ColumnLadder,
    EmptyState, FilterField, HarnessTargetExt, Icon, IconButton, IconSize, ListHeader, ListPointer,
    ListView, Menu, MenuAnchor, MenuItem, PageHeader, Pane, PaneBorder, PopoverMenu, PrBadge,
    ResolvedColumn, Row, RowColumn, SegmentedTab, SegmentedTabs, SkeletonRows, Spinner, StatusKind,
    Text, Tone, Truncate, format_age, truncate,
};
use gpui::{
    Action, AnyElement, App, Context, IntoElement, SharedString, UniformListScrollHandle, Window,
    div, prelude::*,
};

use crate::{
    action_catalogue,
    actions::{hub as hub_actions, prs as pr_actions},
    presentation::{SnapshotIndex, age_secs, contains_folded},
    views::{
        detail::resolved_worktree_status,
        first_run::EmptySurface,
        harness,
        worktrees_list::{FilterSlot, OPEN_KEY},
    },
};

mod cells;
#[cfg(test)]
mod tests;

pub use cells::{Checks, ChecksKind, ReviewState};

/// The `All` scope cap of §3.5 [D-7]: never a refusal, always a final "narrow me" row.
const ALL_SCOPE_CAP: usize = 100;
/// Character budget of the `headRefName` column (§2.9 column 5).
const HEAD_BUDGET: usize = 12;
/// Character budget of the `owner/name` column (§2.9 column 6).
const REPO_BUDGET: usize = 10;
/// How many skeleton rows a cold load shows (§3.5).
const SKELETON_ROWS: usize = 6;
/// The page's title.
const TITLE: &str = "Pull requests";
/// The filter field's placeholder.
const FILTER_PLACEHOLDER: &str = "Filter";
/// The two tabs' labels (§3.5).
const MINE: &str = "Mine";
const REVIEW: &str = "Waiting for my review";
/// The tag after the title of a PR that already has a local worktree: opening it is instant.
const HAS_WORKTREE: &str = "has worktree";
/// The tag while `Enter` / `c` is creating that worktree.
const CREATING: &str = "creating worktree";
/// The Review tab's empty Reviews board (UX-SPEC §3.5, §3.13).
pub const REVIEWS_EMPTY: &str = "No reviews yet.";
/// The hint beside it while the board has no schedule; `Enter` does it.
pub const REVIEWS_ADD_SCHEDULE: &str = "\u{23CE} add the GitHub review schedule";
/// The error callout's button. It runs `r`, whose catalogue label is the longer "Refresh pull
/// requests"; next to a failure the verb a person looks for is "Retry".
const RETRY: &str = "Retry";

/// One PR row, resolved against the local worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRow {
    /// The repository the PR belongs to.
    pub repo: RepoId,
    /// `#{number}`.
    pub number: u64,
    /// The single-line title.
    pub title: SharedString,
    /// The author login, `ghost` when GitHub reported none.
    pub author: SharedString,
    /// `headRefName`.
    pub head: SharedString,
    /// Whether the head lives in another repository (`git-fork` prefix).
    pub cross_repository: bool,
    /// Where the review stands: the state chip.
    pub review: ReviewState,
    /// The checks column.
    pub checks: Checks,
    /// Whether `Enter` / `c` is creating this PR's worktree right now.
    pub creating: bool,
    /// Age of `updatedAt`, in seconds.
    pub age: Option<i64>,
    /// The §2.5 glyph of column 1: the local worktree's session, or a 30 % dot.
    pub presence: StatusKind,
    /// The local worktree, when one matches — this is what `Enter` opens instead of creating.
    pub local: Option<WorktreeId>,
    /// Machine owning the matching worktree, when it is remote.
    pub host: Option<HostId>,
    /// Current daemon-link state for that machine.
    pub host_link: Option<LinkState>,
    /// The PR's canonical URL, for `y` and `b`.
    pub url: SharedString,
}

/// Everything the model needs to turn a slice into rows.
pub struct PrInputs<'a> {
    /// The daemon's slice for the active tab.
    pub slice: Option<&'a PrSlice>,
    /// Every local worktree in scope, in snapshot order.
    pub worktrees: &'a [Worktree],
    /// PRs whose worktree is being created right now.
    pub creating: &'a [(RepoId, u64)],
    /// The current epoch second.
    pub now: i64,
}

/// Builds the rows of one tab, sorted by `updatedAt` desc and capped at [`ALL_SCOPE_CAP`].
#[must_use]
pub fn build_rows(inputs: &PrInputs<'_>, index: &SnapshotIndex<'_>) -> Vec<PrRow> {
    let Some(slice) = inputs.slice else {
        return Vec::new();
    };
    // Preserve the first matching worktree in snapshot order when both the head branch and the
    // `pull/<n>/head` base ref name a local worktree.
    let mut branches = HashMap::new();
    let mut bases = HashMap::new();
    for (position, worktree) in inputs.worktrees.iter().enumerate() {
        branches
            .entry((&worktree.repo_id, worktree.branch.as_str()))
            .or_insert(position);
        bases
            .entry((&worktree.repo_id, worktree.base_ref.as_str()))
            .or_insert(position);
    }
    let creating: HashSet<_> = inputs
        .creating
        .iter()
        .map(|(repo, number)| (repo, *number))
        .collect();
    let mut prs: Vec<&PullRequest> = slice.prs.iter().collect();
    prs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    prs.into_iter()
        .take(ALL_SCOPE_CAP)
        .map(|pr| {
            let branch = (!pr.is_cross_repository)
                .then(|| branches.get(&(&pr.repo_id, pr.head_ref_name.as_str())))
                .flatten();
            let pull_ref = format!("pull/{}/head", pr.number);
            let base = bases.get(&(&pr.repo_id, pull_ref.as_str()));
            let local = branch
                .into_iter()
                .chain(base)
                .min()
                .and_then(|position| inputs.worktrees.get(*position));
            let is_creating = creating.contains(&(&pr.repo_id, pr.number));
            let (presence_glyph, local, host, host_link) = if is_creating {
                (StatusKind::JobRunning, None, None, None)
            } else if let Some(worktree) = local {
                let status = index.status(&worktree.id);
                let slept = index
                    .sessions_for_worktree(&worktree.id)
                    .iter()
                    .any(|session| session.slept_at.is_some());
                let unreachable = worktree.host.as_ref().is_some_and(|host| {
                    index
                        .host(host)
                        .is_none_or(|status| !status.reachable || status.link == LinkState::Down)
                });
                let host_link = worktree
                    .host
                    .as_ref()
                    .and_then(|host| index.host(host))
                    .map(|status| status.link);
                let job_running = index
                    .jobs_for_target(worktree.id.as_str())
                    .iter()
                    .any(|job| crate::views::worktrees_list::owns_row(job));
                (
                    resolved_worktree_status(
                        status,
                        slept,
                        worktree.degraded.is_some(),
                        unreachable,
                        job_running,
                    ),
                    Some(worktree.id.clone()),
                    worktree.host.clone(),
                    host_link,
                )
            } else {
                (StatusKind::NoSession, None, None, None)
            };
            PrRow {
                repo: pr.repo_id.clone(),
                number: pr.number,
                title: SharedString::from(pr.title.clone()),
                author: SharedString::from(pr.author.clone()),
                head: SharedString::from(pr.head_ref_name.clone()),
                cross_repository: pr.is_cross_repository,
                review: ReviewState::of(pr.is_draft, pr.review_decision),
                checks: Checks::of(pr.checks, pr.checks_passed, pr.checks_total),
                creating: is_creating,
                age: age_secs(&pr.updated_at, inputs.now),
                presence: presence_glyph,
                local,
                host,
                host_link,
                url: SharedString::from(pr.url.clone()),
            }
        })
        .collect()
}

/// How many rows the cap hid, for the `+n more — select a repo to narrow` button.
#[must_use]
pub fn hidden_rows(slice: Option<&PrSlice>) -> usize {
    slice.map_or(0, |slice| slice.total.saturating_sub(ALL_SCOPE_CAP))
}

/// Whether a row survives the filter query: number, title, author or branch.
#[must_use]
pub fn matches(row: &PrRow, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let needle = query.to_lowercase();
    row.number.to_string().contains(&needle)
        || contains_folded(&row.title, &needle)
        || contains_folded(&row.author, &needle)
        || contains_folded(&row.head, &needle)
}

/// Everything the screen needs to draw itself.
pub struct PrProps<Rows = Vec<PrRow>> {
    /// The page header's filter field: idle (with any retained query) or the live editor.
    pub filter_slot: FilterSlot,
    /// The rows of the active tab, already filtered.
    pub rows: Rows,
    /// Cursor index into `rows`.
    pub cursor: usize,
    /// Whether the list owns the keyboard.
    pub focused: bool,
    /// Which tab is active.
    pub tab: PrTab,
    /// Mine's count, or `None` while it is loading (`…`).
    pub mine_count: Option<usize>,
    /// The review tab's count, or `None` while it is loading.
    pub review_count: Option<usize>,
    /// Age of the active slice's `fetchedAt`, in seconds.
    pub fetched_age: Option<i64>,
    /// Whether a replacement fetch is running.
    pub loading: bool,
    /// Whether nothing has ever been fetched, which is what shows skeleton rows.
    pub cold: bool,
    /// The slice's most recent error, kept sticky at the top of the tab.
    pub error: Option<SharedString>,
    /// How many rows the §3.5 [D-7] cap hid.
    pub hidden: usize,
    /// The pane's width in `ch`, which resolves the §2.9 ladder.
    pub pane_ch: f32,
    /// Whether the scope spans more than one repository.
    pub multi_repo: bool,
    /// The scope's name, for the empty states.
    pub scope: SharedString,
    /// What the subtitle says the list covers: the repository, or the context in `All`.
    pub covers: SharedString,
    /// The live filter query, when one is set.
    pub filter: Option<SharedString>,
}

/// A row-indexed pointer handler.
type RowHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// A tab handler.
type TabHandler = Rc<dyn Fn(PrTab, &mut Window, &mut App)>;

/// What the screen's pointer does, built once by the Hub and cloned into each frame.
#[derive(Clone)]
pub struct PrHandlers {
    /// A click on a tab: the same switch `Tab` / `h` / `l` make.
    pub select_tab: TabHandler,
    /// A press on row `ix`: put the cursor on it.
    pub select_row: RowHandler,
    /// A double-click on row `ix`: what `⏎` does on it.
    pub open_row: RowHandler,
}

impl PrHandlers {
    /// Handlers that do nothing, for a render test or a gallery.
    #[must_use]
    pub fn inert() -> Self {
        Self {
            select_tab: Rc::new(|_, _, _| {}),
            select_row: Rc::new(|_, _, _| {}),
            open_row: Rc::new(|_, _, _| {}),
        }
    }

    /// The rows' click / double-click / right-click contract (UX-SPEC §5.1): a press selects, a
    /// double-click opens as `⏎` does, and a right click selects before the row's `ContextMenu`
    /// opens its menu.
    fn pointer(&self) -> ListPointer {
        let select = self.select_row.clone();
        let open = self.open_row.clone();
        ListPointer::new()
            .on_select(move |ix, window, cx| select(ix, window, cx))
            .on_open(move |ix, window, cx| open(ix, window, cx))
            // The row's `ContextMenu` shows the menu; this handler exists so the right click is
            // routed through `on_select` first and the menu acts on the row under the pointer.
            .on_menu(|_, _, _, _| {})
    }
}

/// The catalogue's short label for an action: what a button or a menu item reads.
fn label(action: &dyn Action) -> &'static str {
    action_catalogue::info(action.name()).map_or("", |info| info.short_label)
}

/// The verbs one pull request offers, as the row's ⋯, its right-click menu and the detail
/// panel's ⋯ all list them. A verb that cannot work on this PR is left out, not greyed:
/// `c` only creates, so it is absent once a worktree exists, and `I` only checks one.
pub fn menu_builder(
    local: bool,
    creating: bool,
) -> impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + Clone + 'static {
    move |menu, _, _| {
        let entry = |action: Box<dyn Action>| MenuItem::new(label(action.as_ref())).action(action);
        let mut menu = menu
            .item(entry(Box::new(pr_actions::Open)))
            .item(entry(Box::new(pr_actions::OpenKeepAwake)));
        if !local && !creating {
            menu = menu.item(entry(Box::new(pr_actions::CreateWithoutOpening)));
        }
        if local {
            menu = menu.item(entry(Box::new(pr_actions::Inspect)));
        }
        menu.separator()
            .item(entry(Box::new(hub_actions::OpenInBrowser)))
            .item(entry(Box::new(pr_actions::CopyUrl)))
    }
}

/// Renders the page header, the tabs, the error callout and the list.
///
/// `age_offset` ages the prepared rows by the seconds elapsed since the projection was built,
/// so a clock tick re-labels the age column without rebuilding the model.
#[must_use]
pub fn render(
    props: PrProps<impl AsRef<[PrRow]> + 'static>,
    handlers: &PrHandlers,
    scroll: &UniformListScrollHandle,
    age_offset: i64,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let PrProps {
        rows,
        filter_slot,
        cursor,
        focused,
        tab,
        mine_count,
        review_count,
        fetched_age,
        loading,
        cold,
        error,
        hidden,
        pane_ch,
        multi_repo,
        scope,
        covers,
        filter,
    } = props;

    let tabs = tabs(tab, mine_count, review_count, handlers);

    let header = page_header(covers, fetch_stamp_text(loading, fetched_age), filter_slot);

    // The page's own side padding is not the list's width: the ladder resolves against what is
    // left between the list's two gutters.
    let list_ch = pane_ch - f32::from(theme.space.md * 2.0) / fleet_ui_kit::theme::CH;
    let columns =
        ColumnLadder::pull_requests_for(tab == PrTab::Review, multi_repo).resolve(list_ch);
    let row_count = rows.as_ref().len();
    let empty = empty_state(filter, tab, &scope, cx);
    let list_header = (row_count > 0 && !cold).then(|| column_heads(&columns));
    let pointer = handlers.pointer();
    let body_rows = rows;
    let row_columns = columns;
    let list = ListView::new("hub-prs", row_count, move |index, is_cursor, _w, cx| {
        let Some(row) = body_rows.as_ref().get(index) else {
            return div().into_any_element();
        };
        pr_row(
            row,
            RowContext {
                ix: index,
                is_cursor,
                focused,
                columns: &row_columns,
                age_offset,
                pointer: &pointer,
            },
            cx,
        )
    })
    .row_height(theme.metrics.row_h_comfortable)
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);

    let page = div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .px(theme.space.xl)
                .pt(theme.space.xl)
                .child(div().pb(theme.space.md).child(header))
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .border_b(theme.metrics.hairline)
                        .border_color(theme.colors.border)
                        .child(tabs),
                )
                .children(error.map(|message| error_callout(message, cx))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .px(theme.space.md)
                .pt(theme.space.sm)
                .children(list_header)
                .child(if cold {
                    SkeletonRows::new(SKELETON_ROWS)
                        .row_height(theme.metrics.row_h_comfortable)
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .min_h_0()
                        .pt(theme.space.xs)
                        .child(list)
                        .into_any_element()
                })
                .children((hidden > 0).then(|| more_button(hidden, cx))),
        );

    Pane::new()
        .border(PaneBorder::None)
        .focused(focused)
        .body(page)
        .into_any_element()
}

/// The Mine / Waiting-for-my-review tabs; a click is the switch `Tab` makes.
fn tabs(
    tab: PrTab,
    mine_count: Option<usize>,
    review_count: Option<usize>,
    handlers: &PrHandlers,
) -> SegmentedTabs {
    let select_tab = handlers.select_tab.clone();
    SegmentedTabs::new([
        tab_of(MINE, mine_count),
        tab_of(REVIEW, review_count).attention(true),
    ])
    .active(usize::from(tab == PrTab::Review))
    .harness_tabs("prs.tab")
    .on_select(move |index, window, cx| {
        let tab = if index == 0 {
            PrTab::Mine
        } else {
            PrTab::Review
        };
        select_tab(tab, window, cx);
    })
}

/// What the Review tab draws around the Reviews board: the page header and both tabs.
pub struct ReviewBoardProps {
    /// Mine's count, or `None` while it is loading (`…`).
    pub mine_count: Option<usize>,
    /// Reviews waiting on the user, folded from the board (§2.2).
    pub review_count: usize,
    /// What the subtitle says the Mine list covers.
    pub covers: SharedString,
    /// Age of Mine's `fetchedAt`, in seconds.
    pub fetched_age: Option<i64>,
    /// Whether a Mine fetch is running.
    pub loading: bool,
}

/// The Review tab on a daemon with `board.reviews`: the page header and the tabs over `pane`,
/// the Reviews board (or its empty state), which takes the list's and the detail's place.
///
/// The header keeps its subtitle and drops the filter field and Refresh: on the board `/` is
/// the board's own filter and `r` reloads the board, both from the pane's header.
#[must_use]
pub fn render_review_board(
    props: ReviewBoardProps,
    handlers: &PrHandlers,
    pane: AnyElement,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let ReviewBoardProps {
        mine_count,
        review_count,
        covers,
        fetched_age,
        loading,
    } = props;
    let header = PageHeader::new(TITLE).subtitle(format!(
        "Open on GitHub for {covers} \u{00b7} {}",
        fetch_stamp_text(loading, fetched_age)
    ));
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .px(theme.space.xl)
                .pt(theme.space.xl)
                .child(div().pb(theme.space.md).child(header))
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .border_b(theme.metrics.hairline)
                        .border_color(theme.colors.border)
                        .child(tabs(
                            PrTab::Review,
                            mine_count,
                            Some(review_count),
                            handlers,
                        )),
                ),
        )
        .child(div().flex().flex_col().flex_1().min_h_0().child(pane))
        .into_any_element()
}

/// `No reviews yet.`, with the schedule offer while the board has none to fill it.
///
/// The offer is a control, not a key line (ADR 0023): a click runs `on_add`, which does what
/// its `⏎` does.
#[must_use]
pub fn review_board_empty(
    offer_schedule: bool,
    on_add: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let empty = EmptyState::new(REVIEWS_EMPTY);
    if offer_schedule {
        empty
            .button(
                Button::new("reviews-empty-add-schedule", REVIEWS_ADD_SCHEDULE).on_click(on_add),
            )
            .into_any_element()
    } else {
        empty.into_any_element()
    }
}

fn tab_of(label: &'static str, count: Option<usize>) -> SegmentedTab {
    count.map_or_else(
        || SegmentedTab::bare(label).loading(true),
        |count| SegmentedTab::new(label, count),
    )
}

fn fetch_stamp_text(loading: bool, fetched_age: Option<i64>) -> SharedString {
    match (loading, fetched_age) {
        (true, Some(age)) => SharedString::from(format!(
            "\u{27F3} refreshing · fetched {} ago",
            format_age(age)
        )),
        (true, None) => SharedString::new_static("\u{27F3} refreshing\u{2026}"),
        (false, Some(age)) => SharedString::from(format!("fetched {} ago", format_age(age))),
        (false, None) => SharedString::new_static("never fetched"),
    }
}

/// `Pull requests` over `Open on GitHub for <scope> · fetched 12s ago`, then the filter field
/// and Refresh.
fn page_header(covers: SharedString, stamp: SharedString, filter: FilterSlot) -> PageHeader {
    let field = match filter {
        FilterSlot::Idle(query) => {
            let field = FilterField::new("prs-filter", FILTER_PLACEHOLDER)
                .action(Box::new(hub_actions::OpenFilter));
            let field = match query {
                Some(query) => field.query(query),
                None => field,
            };
            field.harness_target("prs.filter").into_any_element()
        }
        FilterSlot::Editing {
            input,
            shown,
            total,
            ..
        } => FilterField::new("prs-filter", FILTER_PLACEHOLDER)
            .editor(input)
            .counts(shown, total)
            .harness_target("filter.input")
            .into_any_element(),
    };
    PageHeader::new(TITLE)
        .subtitle(format!("Open on GitHub for {covers} \u{00b7} {stamp}"))
        .action(field)
        .action(
            Button::new("prs-refresh", label(&pr_actions::Refresh))
                .icon(Icon::RefreshCw)
                .action(Box::new(pr_actions::Refresh))
                .harness_target("prs.refresh"),
        )
}

/// The column heads, from the same ladder resolution as the rows.
fn column_heads(columns: &[ResolvedColumn]) -> ListHeader {
    columns.iter().fold(ListHeader::new(), |header, column| {
        let label = match column.key.as_ref() {
            "number" => "#",
            "title" => "Title",
            "author" => "Author",
            "head" => "Branch",
            "repo" => "Repository",
            "state" => "Status",
            "checks" => "Checks",
            "age" => "Age",
            _ => "",
        };
        header.column(column, label)
    })
}

/// The sticky error of §3.5, as a callout with the Retry its `r` runs. It never hides the stale
/// rows underneath it.
fn error_callout(message: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.space.sm)
        .pt(theme.space.md)
        .child(
            div().flex_1().min_w_0().child(
                Callout::new(Tone::Danger, Icon::CircleX, "Could not load pull requests")
                    .detail(message),
            ),
        )
        .child(
            Button::new("prs-retry", RETRY)
                .action(Box::new(pr_actions::Refresh))
                .harness_target("prs.retry"),
        )
        .into_any_element()
}

/// An empty tab: the fact, and the button that does what the old `r  refresh` hint said.
fn empty_state(
    filter: Option<SharedString>,
    tab: PrTab,
    scope: &SharedString,
    cx: &App,
) -> AnyElement {
    if let Some(query) = filter {
        return EmptySurface::Filter.render(Some(&query));
    }
    let surface = match tab {
        PrTab::Mine => EmptySurface::PrsMine,
        PrTab::Review => EmptySurface::PrsReview,
    };
    let (fact, _) = surface.copy(Some(scope));
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .size_full()
        .items_center()
        .justify_center()
        .gap(theme.space.md)
        .child(Text::ui(fact).muted())
        .child(
            Button::new("prs-empty-refresh", label(&pr_actions::Refresh))
                .icon(Icon::RefreshCw)
                .action(Box::new(pr_actions::Refresh)),
        )
        .into_any_element()
}

/// `+n more — select a repo to narrow`: the §3.5 [D-7] cap row, as the button that takes the
/// keyboard to the repositories.
fn more_button(hidden: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .py(theme.space.sm)
        .child(
            Button::new(
                "prs-more",
                format!("+{hidden} more \u{2014} select a repo to narrow"),
            )
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .action(Box::new(hub_actions::GoRepos))
            .harness_target("prs.more"),
        )
        .into_any_element()
}

/// What one row needs besides its PR.
struct RowContext<'a> {
    ix: usize,
    is_cursor: bool,
    focused: bool,
    columns: &'a [ResolvedColumn],
    age_offset: i64,
    pointer: &'a ListPointer,
}

/// One PR row at the comfortable density, built strictly from the resolved §2.9 columns, with
/// its hover actions and its right-click menu.
fn pr_row(row: &PrRow, context: RowContext<'_>, cx: &App) -> AnyElement {
    let RowContext {
        ix,
        is_cursor,
        focused,
        columns,
        age_offset,
        pointer,
    } = context;
    let theme = cx.theme();
    let mut element = Row::with_id(("pr-row", ix))
        .comfortable()
        .selected(is_cursor)
        .cursor(is_cursor && focused);

    for column in columns {
        let cell: AnyElement = match column.key.as_ref() {
            "number" => Text::data_small(format!("#{}", row.number))
                .muted()
                .into_any_element(),
            "title" => div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .min_w_0()
                .child(Text::ui_strong(row.title.clone()).ellipsize())
                .children(worktree_tag(row))
                .into_any_element(),
            "author" => Text::ui(row.author.clone())
                .muted()
                .ellipsize()
                .into_any_element(),
            "head" => div()
                .flex()
                .items_center()
                .gap(theme.space.xxs)
                .when(row.cross_repository, |el| {
                    el.child(
                        Icon::GitFork
                            .el()
                            .size(IconSize::Small)
                            .color(Tone::Secondary.color(theme)),
                    )
                })
                .child(
                    Text::data_small(truncate(row.head.as_ref(), HEAD_BUDGET, Truncate::Tail))
                        .muted(),
                )
                .into_any_element(),
            "repo" => Text::ui(truncate(row.repo.as_ref(), REPO_BUDGET, Truncate::Head))
                .muted()
                .into_any_element(),
            "state" => state_chip(row.review).into_any_element(),
            "checks" => checks_cell(&row.checks, ("pr-checks", ix), cx),
            "age" => row
                .age
                .map_or_else(AgeLabel::none, |age| {
                    AgeLabel::from_secs(age.saturating_add(age_offset).max(0))
                })
                .into_any_element(),
            "actions" => {
                element = element
                    .column(RowColumn::resolved(column, hover_actions(row, ix, cx)).hover_only());
                continue;
            }
            _ => continue,
        };
        element = element.column(RowColumn::resolved(column, cell));
    }

    let element = pointer.attach(ix, element);
    fleet_ui_kit::ContextMenu::new(
        ("pr-menu", ix),
        element.harness_target_indexed("prs.row", ix),
    )
    .menu(menu_builder(row.local.is_some(), row.creating))
    .into_any_element()
}

/// The review state as the kit's PR chip, the same pill the worktrees page draws.
pub fn state_chip(review: ReviewState) -> PrBadge {
    PrBadge::state_only(review.badge()).chip()
}

/// `has worktree`, or a spinning `creating worktree` while `Enter` / `c` makes one.
fn worktree_tag(row: &PrRow) -> Option<Chip> {
    if row.creating {
        return Some(
            Chip::labeled(Icon::LoaderCircle, CREATING)
                .spinning(true)
                .id(SharedString::from(format!(
                    "pr-creating-{}-{}",
                    row.repo, row.number
                )))
                .filled(true),
        );
    }
    row.local.is_some().then(|| {
        Chip::new()
            .text(HAS_WORKTREE)
            .tone(Tone::Secondary)
            .filled(true)
    })
}

/// The checks cell: a glyph (or a spinner) and the word, in the checks' tone.
fn checks_cell(checks: &Checks, id: (&'static str, usize), cx: &App) -> AnyElement {
    let theme = cx.theme();
    let tone = checks.tone();
    div()
        .flex()
        .items_center()
        .gap(theme.space.xs)
        .children(
            checks
                .icon()
                .map(|icon| icon.el().size(IconSize::Small).color(tone.color(theme))),
        )
        .when(checks.kind == ChecksKind::Running, |el| {
            el.child(Spinner::new(id).size(IconSize::Small).tone(tone))
        })
        .child(Text::ui(checks.label.clone()).tone(tone))
        .into_any_element()
}

/// `Open ⏎` and the ⋯ holding every other verb, shown while the row is hovered or selected.
fn hover_actions(row: &PrRow, ix: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(theme.space.xs)
        .child(
            Button::new(("pr-open", ix), label(&pr_actions::Open))
                .style(ButtonStyle::Secondary)
                .size(ButtonSize::Compact)
                .prefer_key(OPEN_KEY)
                .action(Box::new(pr_actions::Open))
                .harness_target(harness::name(|| format!("prs.row[{ix}].open"))),
        )
        .child(
            PopoverMenu::new(("pr-more", ix))
                .anchor(MenuAnchor::BottomRight)
                .trigger_with(move |open, _, _| {
                    IconButton::new(("pr-more-trigger", ix), Icon::Ellipsis, "More actions")
                        .size(ButtonSize::Compact)
                        .selected(open)
                })
                .menu(menu_builder(row.local.is_some(), row.creating))
                .harness_target(harness::name(|| format!("prs.row[{ix}].menu"))),
        )
        .into_any_element()
}
