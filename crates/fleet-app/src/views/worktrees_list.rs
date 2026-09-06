//! The Hub's worktrees list (UX-SPEC §3.3).
//!
//! One row answers *which branch is alive, which needs attention, which can I throw away*, in
//! the §2.9 column ladder and with the §2.5 glyph vocabulary. Everything §3.3 lists as omitted
//! (`WorktreeId`, `path`, `ahead`/`behind`, `published`, PR title …) is reachable through `i`,
//! `I` or the delete confirm instead.
//!
//! Two invariants are encoded in the model half rather than left to the renderer:
//!
//! * **Cursor stability (§3.3).** [`sort_rows`] is called only on an explicit user action; a
//!   background event changes a row's glyph *in place* because the sort key never moves.
//! * **Absence of knowledge (§1.3).** A row with no inspection keeps its session glyph and
//!   simply carries no derived mark; it never renders a `none`-looking blank.

use std::collections::HashMap;

use fleet_core::{
    github::InspectionPrState,
    ids::{HostId, RepoId, WorktreeId},
    model::Worktree,
    sessions::{AgentActivity, Session, SessionKind, SessionState, WorktreeStatus},
};
use fleet_proto::{
    job::{JobKind, JobRecord, JobStatus},
    snapshot::HostStatus,
};
use fleet_ui_kit::{
    AgeLabel, ColumnAlign, ColumnLadder, DegradedChip, EmptyState, Freshness, Icon, IconSize,
    KeepAliveChips, KeepAliveLabel, ListView, Pane, PaneBorder, PaneHeader, PrBadge, PrBadgeState,
    ResolvedColumn, Row, RowColumn, StatusGlyph, StatusKind, Text, Tone, Truncate, truncate,
};
use gpui::{
    AnyElement, App, IntoElement, SharedString, UniformListScrollHandle, Window, div, prelude::*,
};

use crate::screens::hub::{Inspected, age_secs};

/// Character budget of the `owner/name` column (§2.9 column 3).
const REPO_BUDGET: usize = 14;

/// One row of the worktrees list, fully resolved from the snapshot and the inspection cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    /// The worktree this row stands for.
    pub id: WorktreeId,
    /// Its repository, for the `owner/name` column and for `x` prune.
    pub repo: RepoId,
    /// The §2.5 glyph of column 1.
    pub glyph: StatusKind,
    /// `Worktree.branch` — the only string the user thinks in.
    pub branch: SharedString,
    /// Whether the inspection says the worktree is dirty (`✎`).
    pub dirty: bool,
    /// The remote host, absent for the 95 % local case.
    pub host: Option<SharedString>,
    /// Whether that host's last probe failed.
    pub host_unreachable: bool,
    /// `owner/name`, shown in `All` scope or in a wide pane.
    pub repo_label: SharedString,
    /// Keep-alive labels of the running terminals (§4 sleep policy).
    pub keep_alive: Vec<SharedString>,
    /// Post-create hooks failed; outranks the keep-alive chips in the same slot.
    pub degraded: bool,
    /// A job phase, which replaces the keep-alive slot *and* the age column.
    pub phase: Option<SharedString>,
    /// The row is being deleted: dimmed to 40 %, non-selectable.
    pub deleting: bool,
    /// The inspection errored; the age column gains an amber `triangle-alert`.
    pub inspect_error: bool,
    /// `#n` plus the badge state, when a PR matches the branch.
    pub pr: Option<(u64, PrBadgeState)>,
    /// Whether the derived marks are older than 10 minutes (§2.6).
    pub stale_marks: bool,
    /// Age of `lastOpenedAt ?? createdAt`, in seconds.
    pub age: Option<i64>,
}

/// The icon a keep-alive label carries: an agent, a port, or the generic bolt (§3.3).
#[must_use]
pub fn keep_alive_icon(label: &str) -> Icon {
    let lowered = label.to_lowercase();
    if lowered.starts_with(':') || lowered.contains("port") {
        Icon::Server
    } else if lowered.contains("claude")
        || lowered.contains("opencode")
        || lowered.contains("agent")
    {
        Icon::Bot
    } else {
        Icon::Zap
    }
}

/// The badge a worktree's inspected pull request renders as.
///
/// A **closed, unmerged** PR gets no badge at all: §2.5 [D-3] reserves the blank cell for "this
/// column does not apply to this row", and `PrBadgeState` deliberately has no closed rendering.
#[must_use]
pub fn inspection_badge(state: InspectionPrState) -> Option<PrBadgeState> {
    match state {
        InspectionPrState::Open => Some(PrBadgeState::Review),
        InspectionPrState::Merged => Some(PrBadgeState::Merged),
        InspectionPrState::Closed => None,
    }
}

/// The phase word a running job on this row shows instead of percentages (§3.3).
#[must_use]
pub fn job_phase(job: &JobRecord) -> SharedString {
    if let Some(progress) = job.progress.as_deref().filter(|line| !line.is_empty()) {
        return SharedString::from(progress.to_owned());
    }
    SharedString::new_static(match job.kind {
        JobKind::CreateWorktree => "copying files\u{2026}",
        JobKind::DeleteWorktree => "deleting",
        JobKind::PostCreateHooks => "running hooks\u{2026}",
        JobKind::Prune => "pruning\u{2026}",
        JobKind::Inspect => "checking\u{2026}",
        _ => "working\u{2026}",
    })
}

/// The running job that owns a worktree row, if any.
fn job_for<'a>(id: &WorktreeId, jobs: &'a [JobRecord]) -> Option<&'a JobRecord> {
    jobs.iter().find(|job| {
        matches!(job.status, JobStatus::Running | JobStatus::Queued)
            && job.target == id.as_str()
            && matches!(
                job.kind,
                JobKind::CreateWorktree
                    | JobKind::DeleteWorktree
                    | JobKind::PostCreateHooks
                    | JobKind::Prune
            )
    })
}

/// Whether a queued or running row-level job currently owns this worktree.
#[must_use]
pub fn has_running_job(id: &WorktreeId, jobs: &[JobRecord]) -> bool {
    job_for(id, jobs).is_some()
}

/// Whether the worktree's session was put to sleep rather than merely detached (§2.5).
fn is_slept(id: &WorktreeId, sessions: &[Session]) -> bool {
    sessions.iter().any(|session| {
        matches!(&session.kind, SessionKind::Worktree(worktree) if worktree == id)
            && session.slept_at.is_some()
    })
}

/// Whether a host's last probe failed (`cloud-off`, forces the session to `unknown`).
fn host_unreachable(host: Option<&HostId>, hosts: &[HostStatus]) -> bool {
    host.is_some_and(|host| {
        hosts
            .iter()
            .any(|status| &status.id == host && !status.reachable)
    })
}

/// Maps session and recognized-agent activity to the shared primary status glyph.
///
/// Recognized activity deliberately outranks attachment and sleep state on every surface.
#[must_use]
pub fn session_glyph(
    session: SessionState,
    slept: bool,
    agent_activity: AgentActivity,
) -> StatusKind {
    match agent_activity {
        AgentActivity::Working => return StatusKind::AgentWorking,
        AgentActivity::Idle => return StatusKind::AgentFinished,
        AgentActivity::Unknown => {}
    }
    match session {
        SessionState::Attached => StatusKind::Attached,
        SessionState::Detached if slept => StatusKind::Sleeping,
        SessionState::Detached => StatusKind::DetachedAwake,
        SessionState::Unknown => StatusKind::Unknown,
        SessionState::None => StatusKind::NoSession,
    }
}

/// The §2.5 glyph of one row, in the priority the spec fixes.
#[must_use]
pub fn row_glyph(
    session: SessionState,
    slept: bool,
    agent_activity: AgentActivity,
    degraded: bool,
    unreachable: bool,
    job: bool,
) -> StatusKind {
    if unreachable {
        return StatusKind::HostUnreachable;
    }
    if degraded {
        return StatusKind::Degraded;
    }
    if job {
        return StatusKind::JobRunning;
    }
    session_glyph(session, slept, agent_activity)
}

/// Everything the model needs from the snapshot to build the rows.
pub struct RowInputs<'a> {
    /// The worktrees of the current scope.
    pub worktrees: Vec<&'a Worktree>,
    /// Runtime statuses, keyed by worktree.
    pub statuses: &'a [WorktreeStatus],
    /// Daemon sessions, for the awake / slept distinction.
    pub sessions: &'a [Session],
    /// Host reachability probes.
    pub hosts: &'a [HostStatus],
    /// Every job, for the running-phase rendering.
    pub jobs: &'a [JobRecord],
    /// The client's inspection cache.
    pub inspections: &'a HashMap<WorktreeId, Inspected>,
    /// The current epoch second, so the model stays pure.
    pub now: i64,
}

/// Builds one row per worktree, in the order the caller supplied them.
#[must_use]
pub fn build_rows(inputs: &RowInputs<'_>) -> Vec<WorktreeRow> {
    inputs
        .worktrees
        .iter()
        .map(|worktree| {
            let status = inputs
                .statuses
                .iter()
                .find(|status| status.worktree_id == worktree.id);
            let unreachable = host_unreachable(worktree.host.as_ref(), inputs.hosts);
            let job = job_for(&worktree.id, inputs.jobs);
            // §3.3 *Loading (cold)*: until the daemon reports a status the glyph is
            // `circle-help`, never the `none` dot (§1.3).
            let session = status.map_or(SessionState::Unknown, |status| status.session);
            let inspected = inputs.inspections.get(&worktree.id);
            let inspection = inspected.and_then(|slot| slot.data.as_ref());
            let inspected_age =
                inspection.and_then(|data| age_secs(&data.inspected_at, inputs.now));
            let stale_marks = inspected_age
                .map(Freshness::from_secs)
                .is_some_and(|freshness| freshness == Freshness::Stale);
            let errored = inspected.is_some_and(|slot| slot.error.is_some())
                || inspection.is_some_and(|data| data.error.is_some());

            WorktreeRow {
                id: worktree.id.clone(),
                repo: worktree.repo_id.clone(),
                glyph: row_glyph(
                    session,
                    is_slept(&worktree.id, inputs.sessions),
                    status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
                    worktree.degraded.is_some(),
                    unreachable,
                    job.is_some(),
                ),
                branch: SharedString::from(worktree.branch.clone()),
                // A mark derived from an errored inspection is not drawn at all (§2.6).
                dirty: !errored && inspection.is_some_and(|data| data.dirty),
                host: worktree
                    .host
                    .as_ref()
                    .map(|host| SharedString::from(host.to_string())),
                host_unreachable: unreachable,
                repo_label: SharedString::from(worktree.repo_id.to_string()),
                keep_alive: status
                    .map(|status| {
                        status
                            .running
                            .iter()
                            .map(|label| SharedString::from(label.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                degraded: worktree.degraded.is_some(),
                phase: job.map(job_phase),
                deleting: job.is_some_and(|job| matches!(job.kind, JobKind::DeleteWorktree)),
                inspect_error: errored,
                pr: if errored {
                    None
                } else {
                    inspection
                        .and_then(|data| data.pr.as_ref())
                        .and_then(|pr| inspection_badge(pr.state).map(|state| (pr.number, state)))
                },
                stale_marks,
                age: worktree
                    .last_opened_at
                    .as_deref()
                    .or(Some(worktree.created_at.as_str()))
                    .and_then(|iso| age_secs(iso, inputs.now)),
            }
        })
        .collect()
}

/// Sorts by `lastOpenedAt` desc, then `createdAt` desc (§3.3), so `Enter` alone is often the
/// whole task.
///
/// ISO-8601 UTC strings order lexicographically, which is why no parsing is needed here.
pub fn sort_rows(worktrees: &mut [&Worktree]) {
    worktrees.sort_by(|left, right| {
        right
            .last_opened_at
            .cmp(&left.last_opened_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.branch.cmp(&right.branch))
    });
}

/// Whether a row survives the filter query (§3.10): branch, repo, host or keep-alive label.
#[must_use]
pub fn matches(row: &WorktreeRow, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let needle = query.to_lowercase();
    row.branch.to_lowercase().contains(&needle)
        || row.repo_label.to_lowercase().contains(&needle)
        || row
            .host
            .as_ref()
            .is_some_and(|host| host.to_lowercase().contains(&needle))
        || row
            .keep_alive
            .iter()
            .any(|label| label.to_lowercase().contains(&needle))
}

/// Everything the list needs to draw itself.
pub struct ListProps {
    /// A live filter editor that replaces the ordinary pane header.
    pub header_override: Option<AnyElement>,
    /// The rows, already sorted and filtered.
    pub rows: Vec<WorktreeRow>,
    /// Cursor index into `rows`.
    pub cursor: usize,
    /// Whether the list owns the keyboard.
    pub focused: bool,
    /// The pane's width in `ch`, which resolves the §2.9 ladder.
    pub pane_ch: f32,
    /// `All` or the selected repository's name, for the header scope.
    pub scope: SharedString,
    /// Whether the scope is the `All` pseudo-repo, which forces the repo column on.
    pub scope_is_all: bool,
    /// How many rows exist before the filter, for `shown/total`.
    pub total: usize,
    /// The live filter query, when one is set.
    pub filter: Option<SharedString>,
    /// `stale · <age>` when the snapshot is frozen (§1.3).
    pub stale: Option<SharedString>,
    /// How many rows fit, for the `first–last/total` range of §2.10.
    pub visible_rows: usize,
    /// Whether the daemon has not sent a snapshot yet (§3.13 cold load).
    pub loading: bool,
}

/// Renders the worktrees pane: header, rows and the §3.13 empty states.
#[must_use]
pub fn render(props: ListProps, scroll: &UniformListScrollHandle) -> AnyElement {
    let ListProps {
        rows,
        header_override,
        cursor,
        focused,
        pane_ch,
        scope,
        scope_is_all,
        total,
        filter,
        stale,
        visible_rows,
        loading,
    } = props;

    let mut header = PaneHeader::new("Worktrees")
        .scope(scope.clone())
        .total(total);
    if filter.is_some() {
        header = header.shown(rows.len());
    }
    if let Some(query) = filter.clone() {
        header = header.filter_chip(query);
    }
    if let Some(age) = stale {
        header = header.stale(age);
    }
    if !rows.is_empty() {
        let first = cursor.saturating_sub(cursor % visible_rows.max(1)) + 1;
        let last = (first + visible_rows.max(1) - 1).min(rows.len());
        header = header.range(first, last);
    }

    let empty = if loading {
        EmptyState::new("Loading\u{2026}")
            .action("j / k  browse")
            .into_any_element()
    } else {
        match (filter.clone(), scope_is_all) {
            (Some(query), _) => crate::views::first_run::empty_state("filter", Some(&query)),
            (None, true) => crate::views::first_run::empty_state("worktrees", None),
            (None, false) => crate::views::first_run::empty_state("worktrees-repo", Some(&scope)),
        }
    };

    let columns = columns(pane_ch, scope_is_all);
    let body_rows = rows.clone();
    let list = ListView::new(
        "hub-worktrees",
        rows.len(),
        move |index, is_cursor, window, cx| {
            let Some(row) = body_rows.get(index) else {
                return div().into_any_element();
            };
            worktree_row(row, is_cursor, focused, pane_ch, &columns, window, cx)
        },
    )
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);

    let header = header_override.unwrap_or_else(|| header.into_any_element());
    Pane::new()
        .border(PaneBorder::None)
        .focused(focused)
        .header(header)
        .body(list)
        .into_any_element()
}

/// The §2.9 columns at this pane width.
///
/// The ladder resolves five of the six rules on its own; the sixth — *the repo prefix is shown
/// when the scope is `All` **or** the pane is at least 110 ch* — needs the scope, which the kit
/// deliberately knows nothing about, so it is applied here.
#[must_use]
pub fn columns(pane_ch: f32, scope_is_all: bool) -> Vec<ResolvedColumn> {
    let mut columns = ColumnLadder::worktrees().resolve(pane_ch);
    if scope_is_all && !columns.iter().any(|column| column.key.as_ref() == "repo") {
        let after_branch = columns
            .iter()
            .position(|column| column.key.as_ref() == "branch")
            .map_or(0, |index| index + 1);
        columns.insert(
            after_branch,
            ResolvedColumn {
                key: SharedString::new_static("repo"),
                width: Some(fleet_ui_kit::theme::ch(REPO_BUDGET as f32)),
                min_width: None,
                align: ColumnAlign::Left,
            },
        );
    }
    columns
}

/// One list row, built strictly from the resolved §2.9 columns.
fn worktree_row(
    row: &WorktreeRow,
    is_cursor: bool,
    focused: bool,
    pane_ch: f32,
    columns: &[ResolvedColumn],
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut element = Row::new()
        .leading(
            StatusGlyph::new(row.glyph)
                .id(SharedString::from(format!("wt-glyph-{}", row.id.as_str()))),
        )
        .selected(is_cursor)
        .cursor(is_cursor && focused)
        .dimmed(row.deleting)
        .disabled(row.deleting);

    for column in columns {
        let cell: Option<AnyElement> = match column.key.as_ref() {
            "glyph" => None,
            "branch" => Some(branch_cell(row, cx)),
            "repo" => Some(
                Text::data_small(truncate(
                    row.repo_label.as_ref(),
                    REPO_BUDGET,
                    Truncate::Head,
                ))
                .muted()
                .into_any_element(),
            ),
            "keepalive" => Some(keep_alive_cell(row, pane_ch)),
            "pr" => row.pr.map(|(number, state)| {
                PrBadge::new(number, state)
                    .stale(row.stale_marks)
                    .into_any_element()
            }),
            "age" => Some(age_cell(row, window, cx)),
            _ => None,
        };
        let Some(cell) = cell else {
            continue;
        };
        let mut wrapped = match column.width {
            Some(width) => RowColumn::fixed(width, cell),
            None => RowColumn::flex(cell),
        };
        wrapped = wrapped.align(column.align);
        element = element.column(wrapped);
    }
    element.into_any_element()
}

/// `feat/payroll-fix ✎ ☁devbox` — the dirty mark and the host chip ride with the branch.
fn branch_cell(row: &WorktreeRow, cx: &mut App) -> AnyElement {
    use fleet_ui_kit::ActiveTheme;
    let theme = cx.theme();
    let warning = Tone::Warning.color(theme);
    let secondary = Tone::Secondary.color(theme);
    let dirty_opacity = if row.stale_marks { 0.55 } else { 1.0 };
    div()
        .flex()
        .items_center()
        .min_w_0()
        .gap(theme.space.xs)
        .child(Text::data(row.branch.clone()).ellipsize())
        .when(row.dirty, |el| {
            el.child(
                Icon::FilePen
                    .el()
                    .size(IconSize::Small)
                    .color(warning)
                    .opacity(dirty_opacity),
            )
        })
        .children(row.host.clone().map(|host| {
            let (icon, color) = if row.host_unreachable {
                (Icon::CloudOff, warning)
            } else {
                (Icon::Cloud, secondary)
            };
            div()
                .flex()
                .items_center()
                .gap(theme.space.xxs)
                .child(icon.el().size(IconSize::Small).color(color))
                .child(Text::data_small(host).muted())
        }))
        .into_any_element()
}

/// Column 4: the job phase, else the degraded chip, else the keep-alive chips (§2.9).
fn keep_alive_cell(row: &WorktreeRow, pane_ch: f32) -> AnyElement {
    if let Some(phase) = row.phase.clone() {
        return Text::ui(phase).muted().into_any_element();
    }
    if row.degraded {
        return DegradedChip::hooks_failed()
            .hint("J", "log")
            .into_any_element();
    }
    if row.host_unreachable {
        return Text::ui("offline").tone(Tone::Warning).into_any_element();
    }
    KeepAliveChips::new(
        row.keep_alive
            .iter()
            .map(|label| KeepAliveLabel::with_icon(label.clone(), keep_alive_icon(label))),
    )
    .max_visible(3)
    .width_ch(KeepAliveChips::from_pane_ch(pane_ch))
    .into_any_element()
}

/// Column 6: the age, or nothing while a job phase owns the row; an errored inspection
/// prefixes an amber `triangle-alert` and the row stays operable (§3.3).
fn age_cell(row: &WorktreeRow, _window: &mut Window, cx: &mut App) -> AnyElement {
    use fleet_ui_kit::ActiveTheme;
    let theme = cx.theme();
    let warning = Tone::Warning.color(theme);
    let label = match (row.phase.is_some(), row.age) {
        (true, _) => AgeLabel::none(),
        (false, Some(seconds)) => AgeLabel::from_secs(seconds),
        (false, None) => AgeLabel::none(),
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
                    .color(warning),
            )
        })
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use fleet_core::inspection::WorktreeInspection;
    use fleet_core::{
        github::InspectionPullRequest,
        ids::WorktreeId,
        model::Degraded,
        sessions::{SessionState, WorktreeStatus},
    };

    use super::*;

    fn worktree(slug: &str, opened: Option<&str>, created: &str) -> Worktree {
        Worktree {
            id: WorktreeId::try_from(format!("buk/payroll#{slug}"))
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            slug: slug.to_owned(),
            branch: format!("feat/{slug}"),
            base_ref: "origin/main".to_owned(),
            path: format!("/home/u/.fleet/worktrees/{slug}"),
            session: format!("payroll/{slug}"),
            host: None,
            created_at: created.to_owned(),
            last_opened_at: opened.map(str::to_owned),
            degraded: None,
        }
    }

    fn inspection(id: &WorktreeId, dirty: bool, inspected_at: &str) -> WorktreeInspection {
        WorktreeInspection {
            worktree_id: id.clone(),
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            host: "local".to_owned(),
            path: "/tmp/wt".to_owned(),
            branch: "feat/x".to_owned(),
            base_ref: "origin/main".to_owned(),
            head: Some("abc".to_owned()),
            target_branch: "main".to_owned(),
            upstream: None,
            ahead: None,
            behind: None,
            upstream_gone: false,
            dirty,
            dirty_files: dirty.then_some(12),
            merged_into_target: false,
            unique_commits: None,
            published: false,
            merged: false,
            pr: Some(InspectionPullRequest {
                number: 412,
                state: InspectionPrState::Open,
                url: "https://github.com/buk/payroll/pull/412".to_owned(),
                base_ref_name: "main".to_owned(),
                head_ref_oid: "abc".to_owned(),
            }),
            session: SessionState::None,
            running: Vec::new(),
            inspected_at: inspected_at.to_owned(),
            warnings: Vec::new(),
            error: None,
        }
    }

    fn inputs<'a>(
        worktrees: Vec<&'a Worktree>,
        statuses: &'a [WorktreeStatus],
        inspections: &'a HashMap<WorktreeId, Inspected>,
        jobs: &'a [JobRecord],
    ) -> RowInputs<'a> {
        RowInputs {
            worktrees,
            statuses,
            sessions: &[],
            hosts: &[],
            jobs,
            inspections,
            // 2026-09-04T12:00:00Z
            now: 1_788_523_200,
        }
    }

    #[test]
    fn sorting_is_most_recently_opened_first() {
        let a = worktree("a", Some("2026-09-04T10:00:00Z"), "2026-09-01T10:00:00Z");
        let b = worktree("b", None, "2026-09-03T10:00:00Z");
        let c = worktree("c", Some("2026-09-04T11:00:00Z"), "2026-09-02T10:00:00Z");
        let mut rows = vec![&a, &b, &c];
        sort_rows(&mut rows);
        let slugs: Vec<&str> = rows.iter().map(|row| row.slug.as_str()).collect();
        assert_eq!(slugs, vec!["c", "a", "b"]);
    }

    #[test]
    fn glyph_priority_keeps_health_and_jobs_above_agent_activity() {
        assert_eq!(
            row_glyph(
                SessionState::Attached,
                false,
                AgentActivity::Working,
                true,
                true,
                true
            ),
            StatusKind::HostUnreachable
        );
        assert_eq!(
            row_glyph(
                SessionState::Attached,
                false,
                AgentActivity::Working,
                true,
                false,
                true
            ),
            StatusKind::Degraded
        );
        assert_eq!(
            row_glyph(
                SessionState::Attached,
                false,
                AgentActivity::Working,
                false,
                false,
                true
            ),
            StatusKind::JobRunning
        );
    }

    #[test]
    fn agent_activity_outranks_attachment_and_sleep_state() {
        assert_eq!(
            session_glyph(SessionState::Detached, true, AgentActivity::Working),
            StatusKind::AgentWorking
        );
        assert_eq!(
            session_glyph(SessionState::Attached, false, AgentActivity::Idle),
            StatusKind::AgentFinished
        );
        assert_eq!(
            session_glyph(SessionState::Detached, true, AgentActivity::Unknown),
            StatusKind::Sleeping
        );
        assert_eq!(
            session_glyph(SessionState::Detached, false, AgentActivity::Unknown),
            StatusKind::DetachedAwake
        );
        assert_eq!(
            session_glyph(SessionState::None, false, AgentActivity::Unknown),
            StatusKind::NoSession
        );
        assert_eq!(
            session_glyph(SessionState::Unknown, false, AgentActivity::Unknown),
            StatusKind::Unknown
        );
    }

    #[test]
    fn an_inspected_row_carries_its_dirty_mark_and_pr_badge() {
        let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
        let mut cache = HashMap::new();
        cache.insert(
            worktree.id.clone(),
            Inspected::ready(inspection(&worktree.id, true, "2026-09-04T11:59:30Z")),
        );
        let rows = build_rows(&inputs(vec![&worktree], &[], &cache, &[]));
        assert!(rows[0].dirty);
        assert_eq!(rows[0].pr, Some((412, PrBadgeState::Review)));
        assert!(!rows[0].stale_marks);
    }

    #[test]
    fn marks_go_stale_after_ten_minutes() {
        let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
        let mut cache = HashMap::new();
        cache.insert(
            worktree.id.clone(),
            Inspected::ready(inspection(&worktree.id, true, "2026-09-04T11:30:00Z")),
        );
        let rows = build_rows(&inputs(vec![&worktree], &[], &cache, &[]));
        assert!(rows[0].stale_marks);
    }

    #[test]
    fn an_errored_inspection_draws_no_derived_mark() {
        let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
        let mut cache = HashMap::new();
        cache.insert(worktree.id.clone(), Inspected::failed("gh unavailable"));
        let rows = build_rows(&inputs(vec![&worktree], &[], &cache, &[]));
        assert!(!rows[0].dirty);
        assert_eq!(rows[0].pr, None);
        assert!(rows[0].inspect_error);
    }

    #[test]
    fn a_running_job_replaces_the_phase_and_the_age() {
        let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
        let jobs = vec![JobRecord {
            id: "job-1".parse().unwrap_or_else(|error| panic!("{error}")),
            kind: JobKind::CreateWorktree,
            target: worktree.id.to_string(),
            title: "create".to_owned(),
            status: JobStatus::Running,
            progress: Some("copying files\u{2026}".to_owned()),
            log_path: "/tmp/j.log".to_owned(),
            started_at: "2026-09-04T11:59:00Z".to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: false,
        }];
        let cache = HashMap::new();
        let rows = build_rows(&inputs(vec![&worktree], &[], &cache, &jobs));
        assert_eq!(rows[0].glyph, StatusKind::JobRunning);
        assert_eq!(rows[0].phase.as_deref(), Some("copying files\u{2026}"));
        assert!(!rows[0].deleting);
    }

    #[test]
    fn a_degraded_worktree_shows_the_hook_failure() {
        let mut worktree = worktree("a", None, "2026-09-01T10:00:00Z");
        worktree.degraded = Some(Degraded {
            kind: "post_create_hooks".to_owned(),
            step: "1".to_owned(),
            exit_code: Some(1),
            at: "2026-09-04T11:00:00Z".to_owned(),
            log_path: "/tmp/hooks.log".to_owned(),
        });
        let cache = HashMap::new();
        let rows = build_rows(&inputs(vec![&worktree], &[], &cache, &[]));
        assert!(rows[0].degraded);
        assert_eq!(rows[0].glyph, StatusKind::Degraded);
    }

    #[test]
    fn the_filter_matches_branch_repo_and_keep_alive_labels() {
        let worktree = worktree("rut", None, "2026-09-01T10:00:00Z");
        let statuses = vec![WorktreeStatus {
            worktree_id: worktree.id.clone(),
            session: SessionState::Attached,
            windows: Vec::new(),
            running: vec!["claude".to_owned()],
            agent_activity: fleet_core::sessions::AgentActivity::Unknown,
            agent_activity_changed_at: None,
        }];
        let cache = HashMap::new();
        let rows = build_rows(&inputs(vec![&worktree], &statuses, &cache, &[]));
        assert!(matches(&rows[0], "rut"));
        assert!(matches(&rows[0], "payroll"));
        assert!(matches(&rows[0], "claude"));
        assert!(!matches(&rows[0], "nixos"));
        assert!(matches(&rows[0], ""));
    }

    #[test]
    fn a_closed_pull_request_renders_no_badge() {
        assert_eq!(inspection_badge(InspectionPrState::Closed), None);
        assert_eq!(
            inspection_badge(InspectionPrState::Merged),
            Some(PrBadgeState::Merged)
        );
    }

    #[test]
    fn a_worktree_with_no_status_yet_is_unknown_not_none() {
        let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
        let cache = HashMap::new();
        let rows = build_rows(&inputs(vec![&worktree], &[], &cache, &[]));
        assert_eq!(rows[0].glyph, StatusKind::Unknown);
    }

    #[test]
    fn the_repo_column_survives_a_narrow_pane_in_all_scope() {
        let keys = |pane_ch: f32, all: bool| -> Vec<String> {
            columns(pane_ch, all)
                .into_iter()
                .map(|column| column.key.to_string())
                .collect()
        };
        assert!(keys(138.0, false).contains(&"repo".to_owned()));
        assert!(!keys(93.0, false).contains(&"repo".to_owned()));
        assert!(keys(93.0, true).contains(&"repo".to_owned()));
        let all = keys(93.0, true);
        let branch = all.iter().position(|key| key == "branch");
        let repo = all.iter().position(|key| key == "repo");
        assert_eq!(
            repo,
            branch.map(|index| index + 1),
            "repo follows the branch"
        );
    }

    #[test]
    fn below_seventy_two_ch_only_four_columns_survive() {
        let keys: Vec<String> = columns(70.0, false)
            .into_iter()
            .map(|column| column.key.to_string())
            .collect();
        assert_eq!(keys, vec!["glyph", "branch", "pr", "age"]);
    }

    #[test]
    fn keep_alive_icons_distinguish_agents_from_ports() {
        assert_eq!(keep_alive_icon(":3000"), Icon::Server);
        assert_eq!(keep_alive_icon("claude"), Icon::Bot);
        assert_eq!(keep_alive_icon("build"), Icon::Zap);
    }
}
