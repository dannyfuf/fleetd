//! Repository and clone rows for the Hub rail.

use std::collections::HashMap;

use fleet_core::{
    ids::{ContextId, JobId, RepoId},
    model::{CloneJob, CloneStatus, Repo, Worktree},
};
use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use fleet_ui_kit::{
    ActiveTheme, ColumnAlign, ListView, Pane, PaneBorder, PaneHeader, Row, RowColumn, StatusGlyph,
    StatusKind, Text, Truncate, truncate,
};
use gpui::{AnyElement, App, IntoElement, SharedString, UniformListScrollHandle, px};

use crate::{presentation::parse_percent, views::first_run::EmptySurface};

/// Width of the collapsed icon rail (`H`, KEYMAP A22).
pub const COLLAPSED_WIDTH: f32 = 44.0;
/// Character budget of the rail's name column at the full 240 px width.
const NAME_BUDGET: usize = 18;

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
}

impl<'a> RepoGlyphs<'a> {
    /// Folds one worktree's glyph into its repository's aggregate.
    pub fn add(&mut self, repo: &'a RepoId, glyph: StatusKind) {
        let slot = self.worst.entry(repo).or_insert(StatusKind::NoSession);
        if rank(glyph) > rank(*slot) {
            *slot = glyph;
        }
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
                    percent: attempt
                        .filter(|job| job_is_active(&job.status))
                        .and_then(|job| job.progress.as_deref())
                        .and_then(parse_percent),
                    job: failed.then(|| attempt.map(|job| job.id.clone())).flatten(),
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
    }];
    all.extend(rows);
    all
}

/// The filter query (§3.10) never hides the pinned `All` row, so the scope is never lost.
#[must_use]
pub fn matches(row: &RailRow, query: &str) -> bool {
    if query.is_empty() || row.kind == RailKind::All {
        return true;
    }
    row.name.to_lowercase().contains(&query.to_lowercase())
}

/// Everything the rail needs to draw itself.
pub struct RailProps<Rows = Vec<RailRow>> {
    /// A live filter editor that replaces the ordinary pane header.
    pub header_override: Option<AnyElement>,
    /// The rows, already filtered.
    pub rows: Rows,
    /// The cursor index into `rows`.
    pub cursor: usize,
    /// Whether the rail owns the keyboard (blue cursor bar and focus ring).
    pub focused: bool,
    /// `H`: draw the 44 px icon rail instead of the 240 px rail.
    pub collapsed: bool,
    /// The active context's display name, for the empty state.
    pub context_name: SharedString,
    /// The live filter query, when one is set.
    pub filter: Option<SharedString>,
    /// `stale · <age>` for the header when the snapshot is frozen (§1.3).
    pub stale: Option<SharedString>,
}

/// Renders the rail: header, rows, and the §3.13 empty states.
#[must_use]
pub fn render(
    props: RailProps<impl AsRef<[RailRow]> + 'static>,
    scroll: &UniformListScrollHandle,
    cx: &App,
) -> AnyElement {
    let RailProps {
        rows,
        header_override,
        cursor,
        focused,
        collapsed,
        context_name,
        filter,
        stale,
    } = props;

    let repo_rows = rows.as_ref().len().saturating_sub(1);
    let mut header = PaneHeader::new("Repos").total(repo_rows);
    if let Some(query) = filter.clone() {
        header = header.filter_chip(query);
    }
    if let Some(age) = stale {
        header = header.stale(age);
    }

    let empty = match filter.clone() {
        Some(query) => EmptySurface::Filter.render(Some(&query)),
        None => EmptySurface::Repos.render(Some(&context_name)),
    };

    // Only the `All` row survived the filter: the rail has nothing to offer.
    let row_count = rows.as_ref().len();
    let body_rows = rows;
    let list = ListView::new(
        "hub-repos-rail",
        if repo_rows == 0 { 0 } else { row_count },
        move |index, is_cursor, _window, _cx| {
            let Some(row) = body_rows.as_ref().get(index) else {
                return gpui::div().into_any_element();
            };
            rail_row(row, is_cursor, focused, collapsed)
        },
    )
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);

    let width = if collapsed {
        px(COLLAPSED_WIDTH)
    } else {
        cx.theme().metrics.rail_w
    };
    let mut pane = Pane::fixed(width)
        .border(PaneBorder::Right)
        .focused(focused)
        .body(list);
    if !collapsed {
        pane = pane.header(header_override.unwrap_or_else(|| header.into_any_element()));
    }
    pane.into_any_element()
}

/// One rail row: `[glyph][name][count]`, or just the glyph when collapsed.
fn rail_row(row: &RailRow, is_cursor: bool, focused: bool, collapsed: bool) -> AnyElement {
    let glyph = StatusGlyph::new(row.glyph).id(rail_row_id(row));
    let mut element = Row::new()
        .leading(glyph)
        .selected(is_cursor)
        .cursor(is_cursor && focused);
    if collapsed {
        return element.into_any_element();
    }

    element = element.column(RowColumn::flex(
        Text::ui(truncate(row.name.as_ref(), NAME_BUDGET, Truncate::Middle))
            .tone(row_tone(row.kind)),
    ));
    let trailing = match (row.kind, row.percent, row.count) {
        (RailKind::CloneFailed, _, _) => Some(Text::ui("failed").faint()),
        (RailKind::Cloning, Some(percent), _) => Some(Text::ui(format!("{percent}%")).muted()),
        (_, _, Some(count)) => Some(Text::ui(count.to_string()).muted()),
        _ => None,
    };
    if let Some(trailing) = trailing {
        element = element.column(
            RowColumn::fixed(fleet_ui_kit::theme::ch(7.0), trailing).align(ColumnAlign::Right),
        );
    }
    element.into_any_element()
}

fn rail_row_id(row: &RailRow) -> SharedString {
    row.repo.as_ref().map_or_else(
        || SharedString::new_static("rail-glyph-all"),
        |repo| SharedString::from(format!("rail-glyph-{}", repo.as_str())),
    )
}

/// A failed clone row states its failure in the name's contrast, never in a hue of its own.
fn row_tone(kind: RailKind) -> fleet_ui_kit::Tone {
    if kind == RailKind::Cloning {
        fleet_ui_kit::Tone::Secondary
    } else {
        fleet_ui_kit::Tone::Default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(owner: &str, name: &str, context: &str) -> Repo {
        Repo {
            id: RepoId::try_from(format!("{owner}/{name}"))
                .unwrap_or_else(|error| panic!("{error}")),
            owner: owner.to_owned(),
            name: name.to_owned(),
            url: format!("git@github.com:{owner}/{name}.git"),
            context_id: ContextId::try_from(context).unwrap_or_else(|error| panic!("{error}")),
            default_branch: "main".to_owned(),
            path: format!("/home/u/.fleet/repos/{owner}/{name}"),
            cloned_at: "2026-09-01T10:00:00Z".to_owned(),
            hooks: fleet_core::model::RepoHooks::default(),
        }
    }

    fn worktree(repo_id: &str, slug: &str) -> Worktree {
        Worktree {
            id: fleet_core::ids::WorktreeId::try_from(format!("{repo_id}#{slug}"))
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: RepoId::try_from(repo_id).unwrap_or_else(|error| panic!("{error}")),
            slug: slug.to_owned(),
            branch: slug.to_owned(),
            base_ref: "origin/main".to_owned(),
            path: format!("/home/u/.fleet/worktrees/{slug}"),
            session: format!("s/{slug}"),
            host: None,
            created_at: "2026-09-02T10:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        }
    }

    fn clone_job(owner: &str, name: &str, context: &str, status: CloneStatus) -> CloneJob {
        CloneJob {
            id: RepoId::try_from(format!("{owner}/{name}"))
                .unwrap_or_else(|error| panic!("{error}")),
            owner: owner.to_owned(),
            name: name.to_owned(),
            url: format!("git@github.com:{owner}/{name}.git"),
            context_id: ContextId::try_from(context).unwrap_or_else(|error| panic!("{error}")),
            default_branch: "main".to_owned(),
            path: "/tmp/repo".to_owned(),
            staging_path: "/tmp/staging".to_owned(),
            log_path: "/tmp/clone.log".to_owned(),
            pid: None,
            started_at: "2026-09-04T10:00:00Z".to_owned(),
            status,
            error: None,
        }
    }

    fn job(
        id: &str,
        kind: JobKind,
        target: &str,
        status: JobStatus,
        started_at: &str,
        progress: Option<&str>,
    ) -> JobRecord {
        JobRecord {
            id: JobId::try_from(id).unwrap_or_else(|error| panic!("{error}")),
            kind,
            target: target.to_owned(),
            title: id.to_owned(),
            status,
            progress: progress.map(str::to_owned),
            log_path: "/tmp/job.log".to_owned(),
            started_at: started_at.to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: true,
        }
    }

    #[test]
    fn unknown_outranks_every_other_session_state() {
        assert_eq!(
            aggregate_glyph([
                StatusKind::NoSession,
                StatusKind::Attached,
                StatusKind::Unknown
            ]),
            StatusKind::Unknown
        );
        assert_eq!(
            aggregate_glyph([StatusKind::NoSession, StatusKind::Attached]),
            StatusKind::Attached
        );
        assert_eq!(
            aggregate_glyph([StatusKind::Sleeping, StatusKind::NoSession]),
            StatusKind::Sleeping
        );
        assert_eq!(aggregate_glyph([]), StatusKind::NoSession);
    }

    #[test]
    fn an_unreachable_host_aggregates_like_unknown() {
        assert_eq!(
            aggregate_glyph([StatusKind::Attached, StatusKind::HostUnreachable]),
            StatusKind::HostUnreachable
        );
    }

    #[test]
    fn finished_agents_aggregate_above_working_and_plain_sessions() {
        assert_eq!(
            aggregate_glyph([
                StatusKind::Attached,
                StatusKind::AgentWorking,
                StatusKind::AgentFinished,
            ]),
            StatusKind::AgentFinished
        );
        assert_eq!(
            aggregate_glyph([StatusKind::AgentFinished, StatusKind::Unknown]),
            StatusKind::Unknown
        );
    }

    #[test]
    fn names_disambiguate_only_on_collision() {
        let repos = vec![
            repo("buk", "payroll", "buk"),
            repo("acme", "payroll", "buk"),
            repo("buk", "www", "buk"),
        ];
        let rows = rail_rows(None, &repos, &[], &[], &RepoGlyphs::default(), &[]);
        assert_eq!(rows[2].name.as_ref(), "buk/payroll");
        assert_eq!(rows[3].name.as_ref(), "www");
    }

    #[test]
    fn all_is_pinned_first_and_counts_every_worktree_in_the_context() {
        let repos = vec![repo("buk", "www", "buk"), repo("buk", "payroll", "buk")];
        let worktrees = vec![
            worktree("buk/payroll", "a"),
            worktree("buk/payroll", "b"),
            worktree("buk/www", "c"),
        ];
        let rows = rail_rows(
            Some(&repos[0].context_id),
            &repos,
            &[],
            &worktrees,
            &RepoGlyphs::default(),
            &[],
        );
        assert_eq!(rows[0].kind, RailKind::All);
        assert_eq!(rows[0].count, Some(3));
        assert_eq!(rows[1].name.as_ref(), "payroll");
        assert_eq!(rows[1].count, Some(2));
        assert_eq!(rows[2].name.as_ref(), "www");
    }

    #[test]
    fn clones_sort_where_the_repo_will_live() {
        let repos = vec![repo("buk", "aaa", "buk"), repo("buk", "zzz", "buk")];
        let clones = vec![clone_job("buk", "mmm", "buk", CloneStatus::Cloning)];
        let rows = rail_rows(
            Some(&repos[0].context_id),
            &repos,
            &clones,
            &[],
            &RepoGlyphs::default(),
            &[],
        );
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_ref()).collect();
        assert_eq!(names, vec!["All", "aaa", "mmm", "zzz"]);
        assert_eq!(rows[2].kind, RailKind::Cloning);
        assert_eq!(rows[2].glyph, StatusKind::Cloning);
    }

    #[test]
    fn a_failed_clone_keeps_its_row_until_dismissed() {
        let clones = vec![clone_job("buk", "old-api", "buk", CloneStatus::Failed)];
        let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
        let rows = rail_rows(
            Some(&context),
            &[],
            &clones,
            &[],
            &RepoGlyphs::default(),
            &[],
        );
        assert_eq!(rows[1].kind, RailKind::CloneFailed);
        assert_eq!(rows[1].glyph, StatusKind::CloneFailed);
    }

    #[test]
    fn failed_clone_opens_its_own_job() {
        let clones = vec![clone_job("buk", "old-api", "buk", CloneStatus::Failed)];
        let failed = job(
            "job-clone-failed",
            JobKind::Clone,
            "buk/old-api:startup-reconcile",
            JobStatus::Failed {
                error: "network".to_owned(),
            },
            "2026-09-04T10:00:00Z",
            None,
        );
        let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
        let rows = rail_rows(
            Some(&context),
            &[],
            &clones,
            &[],
            &RepoGlyphs::default(),
            std::slice::from_ref(&failed),
        );
        assert_eq!(rows[1].job.as_ref(), Some(&failed.id));
    }

    #[test]
    fn child_delete_does_not_mark_repo_deleting() {
        let repos = vec![repo("buk", "payroll", "buk")];
        let child_delete = job(
            "job-delete-child",
            JobKind::DeleteWorktree,
            "buk/payroll#feature:attempt-1",
            JobStatus::Running,
            "2026-09-04T10:00:00Z",
            None,
        );
        let rows = rail_rows(
            Some(&repos[0].context_id),
            &repos,
            &[],
            &[],
            &RepoGlyphs::default(),
            &[child_delete],
        );
        assert_eq!(rows[1].kind, RailKind::Repo);
    }

    #[test]
    fn clone_progress_uses_current_attempt() {
        let clones = vec![clone_job("buk", "api", "buk", CloneStatus::Cloning)];
        let jobs = vec![
            job(
                "job-old",
                JobKind::Clone,
                "buk/api",
                JobStatus::Failed {
                    error: "network".to_owned(),
                },
                "2026-09-04T10:00:00Z",
                Some("25%"),
            ),
            job(
                "job-current",
                JobKind::Clone,
                "buk/api:startup-reconcile",
                JobStatus::Running,
                "2026-09-04T10:01:00Z",
                Some("70%"),
            ),
        ];
        let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
        let rows = rail_rows(
            Some(&context),
            &[],
            &clones,
            &[],
            &RepoGlyphs::default(),
            &jobs,
        );
        assert_eq!(rows[1].percent, Some(70));
    }

    #[test]
    fn same_name_clone_rows_have_distinct_identity() {
        let clones = vec![
            clone_job("buk", "api", "buk", CloneStatus::Cloning),
            clone_job("acme", "api", "buk", CloneStatus::Cloning),
        ];
        let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
        let rows = rail_rows(
            Some(&context),
            &[],
            &clones,
            &[],
            &RepoGlyphs::default(),
            &[],
        );
        assert_eq!(rows[1].name.as_ref(), "acme/api");
        assert_eq!(rows[2].name.as_ref(), "buk/api");
        assert_ne!(rail_row_id(&rows[1]), rail_row_id(&rows[2]));
    }

    #[test]
    fn filtering_keeps_the_all_row() {
        let repos = vec![repo("buk", "payroll", "buk"), repo("buk", "www", "buk")];
        let context = repos[0].context_id.clone();
        let rows = rail_rows(
            Some(&context),
            &repos,
            &[],
            &[],
            &RepoGlyphs::default(),
            &[],
        );
        let filtered: Vec<_> = rows.iter().filter(|row| matches(row, "pay")).collect();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].kind, RailKind::All);
        assert_eq!(filtered[1].name.as_ref(), "payroll");
        assert_eq!(rows.iter().filter(|row| matches(row, "nothing")).count(), 1);
    }
}
