//! The Hub sidebar (UX-SPEC §3.2): the repositories the worktree list is scoped to, a collapse
//! control at its foot and an edge the pointer can drag.
//!
//! Rows answer the pointer the way their keys do: a click on a repository selects it like `⏎`,
//! and its `⋯` and its right-click menu list `n` `e` `m` `d` (and `x` on a failed clone).

use std::{collections::HashMap, rc::Rc};

use fleet_core::{
    ids::{ContextId, JobId, RepoId},
    model::{CloneJob, CloneStatus, Repo, Worktree},
};
use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use fleet_ui_kit::{
    ButtonSize, Chip, ContextMenu, HarnessTargetExt, Icon, IconButton, IconSize, ListPointer,
    ListView, Menu, MenuAnchor, MenuItem, NavItem, PopoverMenu, Sidebar, SidebarSection, StatusDot,
    StatusGlyph, StatusKind, Text, Tone,
};
use gpui::{
    Action, AnyElement, App, IntoElement, Pixels, SharedString, UniformListScrollHandle, Window,
    div, prelude::*,
};

use crate::{
    actions::{hub, repos},
    presentation::parse_percent,
    views::{first_run::EmptySurface, harness, worktrees_list::label},
};

/// The repositories section's title.
const REPOS_TITLE: &str = "Repositories";
/// What the pinned scope row reads.
const ALL_LABEL: &str = "All repositories";
/// The accessible name of a row's `⋯` trigger.
const MORE_ACTIONS: &str = "More actions";
/// How many rows tall the Repositories section is while it has no repository to list.
const EMPTY_ROWS: f32 = 3.0;

/// What a rail row stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailKind {
    /// The pinned `All` pseudo-repo of row 0.
    All,
    /// A cloned repository.
    Repo,
    /// A clone still in flight; the count slot shows its percent when one is parseable.
    Cloning,
    /// A clone that failed and has not been dismissed (`x`).
    CloneFailed,
}

/// One row of the rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailRow {
    /// What the row stands for.
    pub kind: RailKind,
    /// The repository, absent only on the `All` row.
    pub repo: Option<RepoId>,
    /// `All`, `Repo.name`, or `owner/name` when two repos share a name (§5).
    pub name: SharedString,
    /// The aggregate session glyph, or the clone's own glyph.
    pub glyph: StatusKind,
    /// Worktree count for the count slot; absent while a clone is running or failing.
    pub count: Option<usize>,
    /// Clone progress percent, when the daemon's progress line carries one.
    pub percent: Option<u8>,
    /// The concrete failed clone attempt that `Enter` must reveal in Jobs.
    pub job: Option<JobId>,
    /// `1 issue` / `3 issues`: worktrees whose hooks failed or whose host is unreachable.
    pub issues: Option<SharedString>,
    /// What a clone row says in its trailing slot: `cloning 40%`, `cloning…`, `failed`.
    pub status: Option<SharedString>,
}

/// How loudly a glyph asks for attention (§3.2).
///
/// Health failures and active jobs stay at the top. Among ordinary session states, unknown
/// remains worst; then a finished agent outranks a working agent because finished work needs the
/// user's attention, and both outrank attachment/sleep state.
const fn rank(kind: StatusKind) -> u8 {
    match kind {
        StatusKind::CloneFailed => 11,
        StatusKind::HostUnreachable => 10,
        StatusKind::Degraded => 9,
        StatusKind::JobRunning | StatusKind::Cloning => 8,
        StatusKind::Unknown => 7,
        StatusKind::AgentFinished => 6,
        StatusKind::AgentWorking => 5,
        StatusKind::Attached => 4,
        StatusKind::DetachedAwake => 3,
        StatusKind::Sleeping => 2,
        StatusKind::NoSession => 1,
    }
}

/// Worst-of aggregation for a repository's session glyph (§3.2).
#[must_use]
fn aggregate_glyph(kinds: impl IntoIterator<Item = StatusKind>) -> StatusKind {
    kinds
        .into_iter()
        .max_by_key(|kind| rank(*kind))
        .unwrap_or(StatusKind::NoSession)
}

const fn job_is_active(status: &JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
    )
}

fn clone_target(target: &str) -> &str {
    target
        .rsplit_once(':')
        .filter(|(repo, suffix)| !suffix.is_empty() && repo.parse::<RepoId>().is_ok())
        .map_or(target, |(repo, _)| repo)
}

/// The worst-of glyph per repository, folded as the projection walks the worktrees so no
/// per-repo vector is ever built.
#[derive(Default)]
pub struct RepoGlyphs<'a> {
    worst: HashMap<&'a RepoId, StatusKind>,
    issues: HashMap<&'a RepoId, usize>,
}

impl<'a> RepoGlyphs<'a> {
    /// Folds one worktree's glyph into its repository's aggregate.
    pub fn add(&mut self, repo: &'a RepoId, glyph: StatusKind) {
        let slot = self.worst.entry(repo).or_insert(StatusKind::NoSession);
        if rank(glyph) > rank(*slot) {
            *slot = glyph;
        }
        if matches!(glyph, StatusKind::Degraded | StatusKind::HostUnreachable) {
            *self.issues.entry(repo).or_default() += 1;
        }
    }

    /// How many of a repository's worktrees are degraded or on an unreachable host.
    fn issues(&self, repo: &RepoId) -> usize {
        self.issues.get(repo).copied().unwrap_or_default()
    }

    /// A repository with no worktrees aggregates to the quietest glyph.
    fn get(&self, repo: &RepoId) -> StatusKind {
        self.worst
            .get(repo)
            .copied()
            .unwrap_or(StatusKind::NoSession)
    }
}

/// Builds every rail row for one context, `All` first and the rest sorted by label.
///
/// A clone in flight sorts among the cloned repositories rather than at the end: "a repo being
/// born must be visible where it will live" (§3.2).
#[must_use]
pub fn rail_rows(
    context: Option<&ContextId>,
    repos: &[Repo],
    clones: &[CloneJob],
    worktrees: &[Worktree],
    glyphs: &RepoGlyphs<'_>,
    jobs: &[JobRecord],
) -> Vec<RailRow> {
    let in_context = |candidate: &ContextId| context.is_none_or(|active| candidate == active);
    let scoped: Vec<&Repo> = repos
        .iter()
        .filter(|repo| in_context(&repo.context_id))
        .collect();

    let mut counts = HashMap::new();
    for worktree in worktrees {
        *counts.entry(&worktree.repo_id).or_insert(0usize) += 1;
    }
    let mut names = HashMap::new();
    for repo in repos {
        let entry = names.entry(repo.name.as_str()).or_insert((&repo.id, false));
        entry.1 |= entry.0 != &repo.id;
    }
    for clone in clones {
        let entry = names
            .entry(clone.name.as_str())
            .or_insert((&clone.id, false));
        entry.1 |= entry.0 != &clone.id;
    }
    let mut clone_attempts: HashMap<&str, &JobRecord> = HashMap::new();
    for job in jobs.iter().filter(|job| matches!(job.kind, JobKind::Clone)) {
        let slot = clone_attempts
            .entry(clone_target(&job.target))
            .or_insert(job);
        if (job.started_at.as_str(), job.id.as_str()) > (slot.started_at.as_str(), slot.id.as_str())
        {
            *slot = job;
        }
    }
    let mut rows: Vec<RailRow> = scoped
        .iter()
        .map(|repo| {
            let count = counts.get(&repo.id).copied().unwrap_or_default();
            RailRow {
                kind: RailKind::Repo,
                repo: Some(repo.id.clone()),
                name: if names
                    .get(repo.name.as_str())
                    .is_some_and(|(_, collides)| *collides)
                {
                    SharedString::new(repo.id.as_str())
                } else {
                    SharedString::new(repo.name.as_str())
                },
                glyph: glyphs.get(&repo.id),
                count: Some(count),
                percent: None,
                job: None,
                issues: issue_label(glyphs.issues(&repo.id)),
                status: None,
            }
        })
        .collect();

    rows.extend(
        clones
            .iter()
            .filter(|clone| in_context(&clone.context_id))
            .map(|clone| {
                let failed = clone.status == CloneStatus::Failed;
                let attempt = clone_attempts.get(clone.id.as_str()).copied();
                let percent = attempt
                    .filter(|job| job_is_active(&job.status))
                    .and_then(|job| job.progress.as_deref())
                    .and_then(parse_percent);
                RailRow {
                    kind: if failed {
                        RailKind::CloneFailed
                    } else {
                        RailKind::Cloning
                    },
                    repo: Some(clone.id.clone()),
                    name: if names
                        .get(clone.name.as_str())
                        .is_some_and(|(_, collides)| *collides)
                    {
                        SharedString::new(clone.id.as_str())
                    } else {
                        SharedString::from(clone.name.clone())
                    },
                    glyph: if failed {
                        StatusKind::CloneFailed
                    } else {
                        StatusKind::Cloning
                    },
                    count: None,
                    percent,
                    job: failed.then(|| attempt.map(|job| job.id.clone())).flatten(),
                    issues: None,
                    status: Some(clone_status(failed, percent)),
                }
            }),
    );
    rows.sort_by(|left, right| left.name.cmp(&right.name));

    let total = scoped
        .iter()
        .map(|repo| counts.get(&repo.id).copied().unwrap_or_default())
        .sum();
    let mut all = vec![RailRow {
        kind: RailKind::All,
        repo: None,
        name: SharedString::new_static("All"),
        glyph: aggregate_glyph(rows.iter().map(|row| row.glyph)),
        count: Some(total),
        percent: None,
        job: None,
        issues: None,
        status: None,
    }];
    all.extend(rows);
    all
}

/// `1 issue`, `3 issues`, or nothing at zero (§1.2).
fn issue_label(issues: usize) -> Option<SharedString> {
    match issues {
        0 => None,
        1 => Some(SharedString::new_static("1 issue")),
        many => Some(SharedString::from(format!("{many} issues"))),
    }
}

/// A clone row's words: `failed`, `cloning 40%`, or `cloning…` until a percent is parseable.
fn clone_status(failed: bool, percent: Option<u8>) -> SharedString {
    match (failed, percent) {
        (true, _) => SharedString::new_static("failed"),
        (false, Some(percent)) => SharedString::from(format!("cloning {percent}%")),
        (false, None) => SharedString::new_static("cloning\u{2026}"),
    }
}

/// The filter query (§3.10) never hides the pinned `All` row, so the scope is never lost.
#[must_use]
pub fn matches(row: &RailRow, query: &str) -> bool {
    if query.is_empty() || row.kind == RailKind::All {
        return true;
    }
    row.name.to_lowercase().contains(&query.to_lowercase())
}

/// `(new width, window, cx)`: what a drag of the sidebar's edge reports.
pub type ResizeHandler = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;

/// What the sidebar's rows and edge do when the pointer uses them. Built by the Hub, which owns
/// the cursor, the scope and the stored width.
#[derive(Clone)]
pub struct SidebarHandlers {
    /// A repository row: a click selects (like `⏎`), a double-click opens, a right-click selects
    /// before its menu opens.
    pub repos: ListPointer,
    /// A drag of the edge, with the new width.
    pub resize: ResizeHandler,
}

/// Everything the sidebar needs to draw itself.
pub struct RailProps<Rows = Rc<[RailRow]>> {
    /// A live filter editor that replaces the repositories title.
    pub header_override: Option<AnyElement>,
    /// The repository rows, already filtered.
    pub rows: Rows,
    /// The cursor index into `rows`.
    pub cursor: usize,
    /// Whether the sidebar owns the keyboard (the cursor bar and the focus ring).
    pub focused: bool,
    /// `H`: draw the icon column instead of the full sidebar.
    pub collapsed: bool,
    /// The width the edge was dragged to, if it was.
    pub width: Option<Pixels>,
    /// The active context's display name, for the empty state.
    pub context_name: SharedString,
    /// The live filter query, when one is set.
    pub filter: Option<SharedString>,
    /// What the pointer does.
    pub handlers: SidebarHandlers,
}

/// Renders the sidebar: the repositories, the collapse control and the §3.13 empty states.
#[must_use]
pub fn render(
    props: RailProps<impl AsRef<[RailRow]> + 'static>,
    scroll: &UniformListScrollHandle,
    cx: &App,
) -> AnyElement {
    let RailProps {
        header_override,
        rows,
        cursor,
        focused,
        collapsed,
        width,
        context_name,
        filter,
        handlers,
    } = props;
    let row_h = fleet_ui_kit::ActiveTheme::theme(cx).metrics.row_h;

    // Only the `All` row survived the filter: the rail has nothing to offer. A collapsed rail has
    // no room for the sentence, so it shows nothing rather than a clipped one.
    let repo_rows = rows.as_ref().len().saturating_sub(1);
    let empty = if collapsed {
        div().into_any_element()
    } else {
        match filter.clone() {
            Some(query) => EmptySurface::Filter.render(Some(&query)),
            None => EmptySurface::Repos.render(Some(&context_name)),
        }
    };
    let row_count = if repo_rows == 0 {
        0
    } else {
        rows.as_ref().len()
    };
    let pointer = handlers.repos.clone();
    let list = ListView::new(
        "hub-repos-rail",
        row_count,
        move |ix, is_cursor, _window, _cx| {
            let Some(row) = rows.as_ref().get(ix) else {
                return div().into_any_element();
            };
            repo_item(
                row,
                &ItemContext {
                    ix,
                    is_cursor,
                    focused,
                    collapsed,
                    pointer: &pointer,
                },
            )
        },
    )
    .row_height(row_h)
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);

    let mut repos_section = SidebarSection::new(REPOS_TITLE)
        .action(repos_actions(filter))
        .body(list);
    // A virtualized list cannot measure itself: the section is its rows' height and scrolls when
    // the sidebar is shorter.
    if row_count > 0 {
        repos_section = repos_section.body_height(row_h * row_count as f32);
    } else if !collapsed {
        // The empty state is a sentence over a Clone repo button: three rows hold both, where one
        // would centre them over the section's own header.
        repos_section = repos_section.body_height(row_h * EMPTY_ROWS);
    }
    if let Some(header) = header_override {
        repos_section = repos_section.header(header);
    }
    let sidebar = Sidebar::new("hub-sidebar")
        .width(width)
        .collapsed(collapsed)
        .focused(focused)
        .section(repos_section);
    let resize = handlers.resize;
    // The sidebar's own rect stays the collapse oracle (TESTING-HARNESS §3): `sidebar_w` or the
    // dragged width expanded, `sidebar_collapsed_w` collapsed.
    sidebar
        .footer(collapse_button(collapsed))
        .on_resize(move |width, window, cx| resize(width, window, cx))
        .handle_target("repos.resize")
        .harness_target("repos.rail")
        .into_any_element()
}

/// The repositories title's controls: the retained filter, then `+` (Clone repo, `n`).
fn repos_actions(filter: Option<SharedString>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .children(filter.map(|query| Chip::labeled(Icon::Search, query)))
        .child(
            IconButton::new("repos-clone", Icon::Plus, label(&repos::Clone))
                .size(ButtonSize::Compact)
                .action(Box::new(repos::Clone))
                .harness_target("repos.clone"),
        )
        .into_any_element()
}

/// The foot of the sidebar: the pointer's `H`.
fn collapse_button(collapsed: bool) -> impl IntoElement {
    IconButton::new(
        "repos-collapse",
        if collapsed {
            Icon::PanelLeftOpen
        } else {
            Icon::PanelLeftClose
        },
        label(&hub::ToggleRepoRail),
    )
    .size(ButtonSize::Compact)
    .action(Box::new(hub::ToggleRepoRail))
    .harness_target("repos.collapse")
}

/// Everything one repository item needs besides its row.
struct ItemContext<'a> {
    ix: usize,
    is_cursor: bool,
    focused: bool,
    collapsed: bool,
    pointer: &'a ListPointer,
}

/// One repository item: `[dot][name][issue chip][count]`, a clone's spinner and progress, or the
/// pinned `All repositories`; every item but `All` carries its `⋯` and right-click menu.
fn repo_item(row: &RailRow, ctx: &ItemContext<'_>) -> AnyElement {
    let ix = ctx.ix;
    let label = if row.kind == RailKind::All {
        SharedString::new_static(ALL_LABEL)
    } else {
        row.name.clone()
    };
    let item = NavItem::new(("repo-item", ix), label)
        .leading(leading(row))
        .selected(ctx.is_cursor)
        .cursor(ctx.is_cursor && ctx.focused)
        .collapsed(ctx.collapsed)
        .pointer(ctx.pointer, ix)
        .when(row.kind == RailKind::Cloning, |item| {
            item.tone(Tone::Muted).progress(row.percent)
        });
    let item = with_trailing(item, row, ix);
    if row.kind == RailKind::All {
        return item
            .harness_target_indexed("repos.row", ix)
            .into_any_element();
    }
    let kind = row.kind;
    ContextMenu::new(
        ("repo-menu", ix),
        item.hover_action(more_menu(ix, kind))
            .harness_target_indexed("repos.row", ix),
    )
    .menu(move |menu, _, _| repo_menu(menu, kind))
    .into_any_element()
}

/// The trailing facts of a repository item, in the order the mockup reads them: the issue chip,
/// then the count or the clone's words.
fn with_trailing(item: NavItem, row: &RailRow, ix: usize) -> NavItem {
    let item = match &row.issues {
        Some(issues) => item.trailing(
            Chip::new()
                .text(issues.clone())
                .tone(Tone::Warning)
                .filled(true)
                .id(("repo-issues", ix)),
        ),
        None => item,
    };
    match (&row.status, row.count) {
        (Some(status), _) if row.kind == RailKind::CloneFailed => {
            item.trailing(Text::caption(status.clone()).faint())
        }
        (Some(status), _) => item.trailing(Text::caption(status.clone()).muted()),
        (None, Some(count)) => item.trailing(Text::caption(count.to_string()).muted()),
        (None, None) => item,
    }
}

/// The leading mark: the grid for `All`, a dot for a repository, the clone's own glyph.
fn leading(row: &RailRow) -> AnyElement {
    match row.kind {
        RailKind::All => Icon::LayoutGrid
            .el()
            .size(IconSize::Small)
            .tone(Tone::Secondary)
            .into_any_element(),
        RailKind::Repo => StatusDot::small(dot_tone(row.glyph)).into_any_element(),
        RailKind::Cloning | RailKind::CloneFailed => StatusGlyph::new(row.glyph)
            .id(rail_row_id(row))
            .into_any_element(),
    }
}

/// A repository's dot, from the worst-of glyph of its worktrees: amber when a finished agent
/// waits for you, green while one works, lighter while any session is alive, dim otherwise.
/// Failures are the issue chip's to say, not the dot's.
const fn dot_tone(glyph: StatusKind) -> Tone {
    match glyph {
        StatusKind::AgentFinished => Tone::Warning,
        StatusKind::AgentWorking => Tone::Success,
        StatusKind::Attached | StatusKind::DetachedAwake | StatusKind::JobRunning => {
            Tone::Secondary
        }
        _ => Tone::Muted,
    }
}

/// The `⋯` trigger drawn while the item is hovered: the same menu as its right-click.
fn more_menu(ix: usize, kind: RailKind) -> impl IntoElement {
    PopoverMenu::new(("repo-more", ix))
        .anchor(MenuAnchor::BottomRight)
        .trigger_with(move |open, _, _| {
            IconButton::new(("repo-more-trigger", ix), Icon::Ellipsis, MORE_ACTIONS)
                .size(ButtonSize::Compact)
                .selected(open)
        })
        .menu(move |menu, _, _| repo_menu(menu, kind))
        .harness_target(harness::name(|| format!("repos.row[{ix}].menu")))
}

/// Every action a repository row's keys run, each with its key from the live keymap. A clone in
/// flight only offers another clone; a failed one offers `x` first, which is only bound there.
pub(crate) fn repo_menu(menu: Menu, kind: RailKind) -> Menu {
    let item = |action: Box<dyn Action>| MenuItem::new(label(action.as_ref())).action(action);
    match kind {
        RailKind::Repo => menu
            .item(item(Box::new(repos::Clone)))
            .item(item(Box::new(repos::EditHooks)))
            .item(item(Box::new(repos::MoveToContext)))
            .separator()
            .item(item(Box::new(repos::Delete)).destructive(true)),
        RailKind::CloneFailed => menu
            .item(item(Box::new(repos::DismissClone)))
            .item(item(Box::new(repos::Clone))),
        RailKind::All | RailKind::Cloning => menu.item(item(Box::new(repos::Clone))),
    }
}

fn rail_row_id(row: &RailRow) -> SharedString {
    row.repo.as_ref().map_or_else(
        || SharedString::new_static("rail-glyph-all"),
        |repo| SharedString::from(format!("rail-glyph-{}", repo.as_str())),
    )
}

#[cfg(test)]
mod tests;
