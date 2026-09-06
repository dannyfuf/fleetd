//! The Hub's repos rail (UX-SPEC §3.2).
//!
//! The rail answers exactly two questions — *what does the worktrees list show* and *is any
//! repository unhealthy or still cloning* — so it carries one glyph, one name and one count per
//! row and nothing else. Everything §3.2 lists as intentionally omitted (`url`, `path`,
//! `defaultBranch`, hooks, pool state) lives in the detail panel.
//!
//! The model half ([`rail_rows`], [`aggregate_glyph`], [`filter_rows`]) is pure and tested; the
//! render half composes `fleet-ui-kit` components only.

use fleet_core::{
    ids::{ContextId, RepoId},
    model::{CloneJob, CloneStatus, Repo, Worktree},
};
use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use fleet_ui_kit::{
    ColumnAlign, ListView, Pane, PaneBorder, PaneHeader, Row, RowColumn, StatusGlyph, StatusKind,
    Text, Truncate, truncate,
};
use gpui::{AnyElement, IntoElement, SharedString, UniformListScrollHandle, px};

use crate::state::parse_percent;

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
    /// A repository whose deletion transaction is running: dimmed and non-selectable.
    Deleting,
}

impl RailKind {
    /// Whether the cursor may rest on this row.
    #[must_use]
    pub const fn selectable(self) -> bool {
        !matches!(self, Self::Deleting)
    }
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
}

/// Worst-of aggregation for a repository's session glyph (§3.2).
///
/// Health failures and active jobs stay at the top. Among ordinary session states, unknown
/// remains worst; then a finished agent outranks a working agent because finished work needs the
/// user's attention, and both outrank attachment/sleep state.
#[must_use]
pub fn aggregate_glyph(kinds: impl IntoIterator<Item = StatusKind>) -> StatusKind {
    fn rank(kind: StatusKind) -> u8 {
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
    kinds
        .into_iter()
        .max_by_key(|kind| rank(*kind))
        .unwrap_or(StatusKind::NoSession)
}

/// The label a repository shows: its bare name, or `owner/name` when the name collides (§5).
#[must_use]
pub fn display_name(repo: &Repo, all: &[Repo]) -> String {
    let collides = all
        .iter()
        .any(|other| other.id != repo.id && other.name == repo.name);
    if collides {
        format!("{}/{}", repo.owner, repo.name)
    } else {
        repo.name.clone()
    }
}

/// Whether a running job is deleting this repository's rows.
fn is_deleting(repo: &RepoId, jobs: &[JobRecord]) -> bool {
    jobs.iter().any(|job| {
        matches!(job.status, JobStatus::Running | JobStatus::Queued)
            && matches!(job.kind, JobKind::DeleteWorktree)
            && job.target.starts_with(repo.as_str())
    })
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
    glyphs: &dyn Fn(&RepoId) -> Vec<StatusKind>,
    jobs: &[JobRecord],
) -> Vec<RailRow> {
    let in_context = |candidate: &ContextId| context.is_none_or(|active| candidate == active);
    let scoped: Vec<&Repo> = repos
        .iter()
        .filter(|repo| in_context(&repo.context_id))
        .collect();

    let mut rows: Vec<RailRow> = scoped
        .iter()
        .map(|repo| {
            let count = worktrees
                .iter()
                .filter(|worktree| worktree.repo_id == repo.id)
                .count();
            let deleting = is_deleting(&repo.id, jobs);
            RailRow {
                kind: if deleting {
                    RailKind::Deleting
                } else {
                    RailKind::Repo
                },
                repo: Some(repo.id.clone()),
                name: SharedString::from(display_name(repo, repos)),
                glyph: if deleting {
                    StatusKind::JobRunning
                } else {
                    aggregate_glyph(glyphs(&repo.id))
                },
                count: Some(count),
                percent: None,
            }
        })
        .collect();

    rows.extend(
        clones
            .iter()
            .filter(|clone| in_context(&clone.context_id))
            .map(|clone| {
                let failed = clone.status == CloneStatus::Failed;
                RailRow {
                    kind: if failed {
                        RailKind::CloneFailed
                    } else {
                        RailKind::Cloning
                    },
                    repo: Some(clone.id.clone()),
                    name: SharedString::from(clone.name.clone()),
                    glyph: if failed {
                        StatusKind::CloneFailed
                    } else {
                        StatusKind::Cloning
                    },
                    count: None,
                    percent: clone_percent(clone, jobs),
                }
            }),
    );
    rows.sort_by(|left, right| left.name.cmp(&right.name));

    let total = worktrees
        .iter()
        .filter(|worktree| scoped.iter().any(|repo| repo.id == worktree.repo_id))
        .count();
    let mut all = vec![RailRow {
        kind: RailKind::All,
        repo: None,
        name: SharedString::new_static("All"),
        glyph: aggregate_glyph(rows.iter().map(|row| row.glyph)),
        count: Some(total),
        percent: None,
    }];
    all.extend(rows);
    all
}

/// The percent shown in a cloning row's count slot, when the daemon reported one.
fn clone_percent(clone: &CloneJob, jobs: &[JobRecord]) -> Option<u8> {
    jobs.iter()
        .filter(|job| matches!(job.kind, JobKind::Clone) && job.target == clone.id.as_str())
        .find_map(|job| job.progress.as_deref().and_then(parse_percent))
}

/// Applies the filter query (§3.10), keeping the pinned `All` row so the scope is never lost.
#[must_use]
pub fn filter_rows(rows: &[RailRow], query: &str) -> Vec<RailRow> {
    if query.is_empty() {
        return rows.to_vec();
    }
    let needle = query.to_lowercase();
    rows.iter()
        .filter(|row| row.kind == RailKind::All || row.name.to_lowercase().contains(&needle))
        .cloned()
        .collect()
}

/// Everything the rail needs to draw itself.
pub struct RailProps {
    /// A live filter editor that replaces the ordinary pane header.
    pub header_override: Option<AnyElement>,
    /// The rows, already filtered.
    pub rows: Vec<RailRow>,
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
pub fn render(props: RailProps, scroll: &UniformListScrollHandle) -> AnyElement {
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

    let repo_rows = rows.len().saturating_sub(1);
    let mut header = PaneHeader::new("Repos").total(repo_rows);
    if let Some(query) = filter.clone() {
        header = header.filter_chip(query);
    }
    if let Some(age) = stale {
        header = header.stale(age);
    }

    let empty = match filter.clone() {
        Some(query) => crate::views::first_run::empty_state("filter", Some(&query)),
        None => crate::views::first_run::empty_state("repos", Some(&context_name)),
    };

    // Only the `All` row survived the filter: the rail has nothing to offer.
    let body_rows = rows.clone();
    let list = ListView::new(
        "hub-repos-rail",
        if repo_rows == 0 { 0 } else { rows.len() },
        move |index, is_cursor, _window, _cx| {
            let Some(row) = body_rows.get(index) else {
                return gpui::div().into_any_element();
            };
            rail_row(row, is_cursor, focused, collapsed)
        },
    )
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);

    let mut pane = Pane::fixed(px(if collapsed { COLLAPSED_WIDTH } else { 240.0 }))
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
    let glyph = StatusGlyph::new(row.glyph).id(SharedString::from(format!(
        "rail-glyph-{}",
        row.name.as_ref()
    )));
    let mut element = Row::new()
        .leading(glyph)
        .selected(is_cursor)
        .cursor(is_cursor && focused)
        .dimmed(row.kind == RailKind::Deleting)
        .disabled(!row.kind.selectable());
    if collapsed {
        return element.into_any_element();
    }

    element = element.column(RowColumn::flex(
        Text::ui(truncate(row.name.as_ref(), NAME_BUDGET, Truncate::Middle))
            .tone(row_tone(row.kind)),
    ));
    let trailing = match (row.kind, row.percent, row.count) {
        (RailKind::CloneFailed, _, _) => Some(Text::ui("failed").faint()),
        (RailKind::Deleting, _, _) => Some(Text::ui("deleting").faint()),
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

/// A failed clone row states its failure in the name's contrast, never in a hue of its own.
fn rail_tone_is_muted(kind: RailKind) -> bool {
    matches!(kind, RailKind::Deleting | RailKind::Cloning)
}

fn row_tone(kind: RailKind) -> fleet_ui_kit::Tone {
    if rail_tone_is_muted(kind) {
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
        assert_eq!(display_name(&repos[0], &repos), "buk/payroll");
        assert_eq!(display_name(&repos[2], &repos), "www");
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
            &|_| Vec::new(),
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
            &|_| Vec::new(),
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
        let rows = rail_rows(Some(&context), &[], &clones, &[], &|_| Vec::new(), &[]);
        assert_eq!(rows[1].kind, RailKind::CloneFailed);
        assert_eq!(rows[1].glyph, StatusKind::CloneFailed);
        assert!(rows[1].kind.selectable());
    }

    #[test]
    fn filtering_keeps_the_all_row() {
        let repos = vec![repo("buk", "payroll", "buk"), repo("buk", "www", "buk")];
        let context = repos[0].context_id.clone();
        let rows = rail_rows(Some(&context), &repos, &[], &[], &|_| Vec::new(), &[]);
        let filtered = filter_rows(&rows, "pay");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].kind, RailKind::All);
        assert_eq!(filtered[1].name.as_ref(), "payroll");
        assert_eq!(filter_rows(&rows, "nothing").len(), 1);
    }

    #[test]
    fn a_deleting_row_is_not_selectable() {
        assert!(!RailKind::Deleting.selectable());
        assert!(RailKind::CloneFailed.selectable());
    }
}
