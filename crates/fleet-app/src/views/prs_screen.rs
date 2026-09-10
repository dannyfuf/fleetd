//! Prepared pull-request rows and tab/list composition.

use std::collections::{HashMap, HashSet};

use fleet_core::{
    github::{PrTab, PullRequest, derive_pr_state},
    ids::{HostId, RepoId, WorktreeId},
    model::Worktree,
};
use fleet_proto::response::PrSlice;
use fleet_proto::snapshot::LinkState;
use fleet_ui_kit::{
    ActiveTheme, AgeLabel, ColumnLadder, Icon, IconSize, ListView, Pane, PaneBorder, PrBadge,
    PrBadgeState, ResolvedColumn, Row, RowColumn, SegmentedTab, SegmentedTabs, SkeletonRows,
    StatusGlyph, StatusKind, Text, Tone, Truncate, format_age, truncate,
};
use gpui::{AnyElement, App, IntoElement, SharedString, UniformListScrollHandle, div, prelude::*};

use crate::{
    presentation::{SnapshotIndex, age_secs, contains_folded, pr_badge_state},
    views::{detail::resolved_worktree_status, first_run::EmptySurface},
};

/// The `All` scope cap of §3.5 [D-7]: never a refusal, always a final "narrow me" row.
const ALL_SCOPE_CAP: usize = 100;
/// Character budget of the `headRefName` column (§2.9 column 5).
const HEAD_BUDGET: usize = 12;
/// Character budget of the `owner/name` column (§2.9 column 6).
const REPO_BUDGET: usize = 10;
/// How many skeleton rows a cold load shows (§3.5).
const SKELETON_ROWS: usize = 6;

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
    /// The collapsed state badge.
    pub state: PrBadgeState,
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
            let (presence_glyph, local, host, host_link) =
                if creating.contains(&(&pr.repo_id, pr.number)) {
                    (StatusKind::JobRunning, None, None, None)
                } else if let Some(worktree) = local {
                    let status = index.status(&worktree.id);
                    let slept = index
                        .sessions_for_worktree(&worktree.id)
                        .iter()
                        .any(|session| session.slept_at.is_some());
                    let unreachable = worktree.host.as_ref().is_some_and(|host| {
                        index.host(host).is_none_or(|status| {
                            !status.reachable || status.link == LinkState::Down
                        })
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
                state: pr_badge_state(derive_pr_state(pr.is_draft, pr.checks, pr.review_decision)),
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

/// How many rows the cap hid, for the faint `+n more — select a repo to narrow` row.
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
    /// A live filter editor that replaces the tab header.
    pub header_override: Option<AnyElement>,
    /// The rows of the active tab, already filtered.
    pub rows: Rows,
    /// Cursor index into `rows`.
    pub cursor: usize,
    /// Whether the list owns the keyboard.
    pub focused: bool,
    /// Which tab is active.
    pub tab: PrTab,
    /// `MINE`'s count, or `None` while it is loading (`…`).
    pub mine_count: Option<usize>,
    /// `REVIEW`'s count, or `None` while it is loading.
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
    /// The live filter query, when one is set.
    pub filter: Option<SharedString>,
}

/// Renders the tabs, the fetch stamp, the error row and the list.
///
/// `age_offset` ages the prepared rows by the seconds elapsed since the projection was built,
/// so a clock tick re-labels the age column without rebuilding the model.
#[must_use]
pub fn render(
    props: PrProps<impl AsRef<[PrRow]> + 'static>,
    scroll: &UniformListScrollHandle,
    age_offset: i64,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let PrProps {
        rows,
        header_override,
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
        filter,
    } = props;

    let tabs = SegmentedTabs::new([tab_of("Mine", mine_count), tab_of("Review", review_count)])
        .active(usize::from(tab == PrTab::Review));

    let stamp = Text::ui(fetch_stamp_text(loading, fetched_age));
    let stamp = if !loading && fetched_age.is_some() && error.is_some() {
        stamp.tone(Tone::Warning)
    } else {
        stamp.muted()
    };

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .w_full()
        .h(theme.metrics.pane_header_h)
        .px(theme.space.md)
        .child(tabs)
        .child(stamp);

    let empty = match (filter.clone(), tab) {
        (Some(query), _) => EmptySurface::Filter.render(Some(&query)),
        (None, PrTab::Mine) => EmptySurface::PrsMine.render(Some(&scope)),
        (None, PrTab::Review) => EmptySurface::PrsReview.render(Some(&scope)),
    };

    let columns =
        ColumnLadder::pull_requests_for(tab == PrTab::Review, multi_repo).resolve(pane_ch);
    let row_count = rows.as_ref().len();
    let body_rows = rows;
    let list = ListView::new("hub-prs", row_count, move |index, is_cursor, _w, cx| {
        let Some(row) = body_rows.as_ref().get(index) else {
            return div().into_any_element();
        };
        pr_row(row, is_cursor, focused, &columns, age_offset, cx)
    })
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);

    let body = div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .children(error.map(|message| error_row(message, cx)))
        .child(if cold {
            SkeletonRows::new(SKELETON_ROWS).into_any_element()
        } else {
            div().flex_1().min_h_0().child(list).into_any_element()
        })
        .children((hidden > 0).then(|| {
            div()
                .px(theme.space.md)
                .child(Text::ui(format!("+{hidden} more \u{2014} select a repo to narrow")).faint())
        }));

    let header = header_override.unwrap_or_else(|| header.into_any_element());
    Pane::new()
        .border(PaneBorder::None)
        .focused(focused)
        .header(header)
        .body(body)
        .into_any_element()
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
        (true, None) => SharedString::new_static("\u{27F3} refreshing"),
        (false, Some(age)) => SharedString::from(format!("fetched {} ago", format_age(age))),
        (false, None) => SharedString::new_static("never fetched"),
    }
}

/// The sticky error row of §3.5: it never hides the stale rows underneath it.
fn error_row(message: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .w_full()
        .h(theme.metrics.row_h)
        .px(theme.space.md)
        .child(
            Icon::CircleX
                .el()
                .size(IconSize::Small)
                .color(Tone::Danger.color(theme)),
        )
        .child(Text::ui(message).tone(Tone::Danger).ellipsize())
        .child(Text::hint("r  retry"))
        .into_any_element()
}

/// One PR row, built strictly from the resolved §2.9 columns.
fn pr_row(
    row: &PrRow,
    is_cursor: bool,
    focused: bool,
    columns: &[ResolvedColumn],
    age_offset: i64,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let mut element = Row::new()
        .leading(
            StatusGlyph::new(row.presence).id(SharedString::from(format!(
                "pr-glyph-{}-{}",
                row.repo, row.number
            ))),
        )
        .selected(is_cursor)
        .cursor(is_cursor && focused);

    for column in columns {
        let cell: Option<AnyElement> = match column.key.as_ref() {
            "presence" => None,
            "number" => Some(
                Text::data_small(format!("#{}", row.number))
                    .muted()
                    .into_any_element(),
            ),
            "title" => Some(Text::ui(row.title.clone()).ellipsize().into_any_element()),
            "author" => Some(
                Text::ui(row.author.clone())
                    .muted()
                    .ellipsize()
                    .into_any_element(),
            ),
            "head" => Some(
                div()
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
            ),
            "repo" => Some(
                Text::data_small(truncate(row.repo.as_ref(), REPO_BUDGET, Truncate::Head))
                    .muted()
                    .into_any_element(),
            ),
            "state" => Some(PrBadge::state_only(row.state).into_any_element()),
            "age" => Some(
                row.age
                    .map_or_else(AgeLabel::none, |age| {
                        AgeLabel::from_secs(age.saturating_add(age_offset).max(0))
                    })
                    .into_any_element(),
            ),
            _ => None,
        };
        let Some(cell) = cell else { continue };
        element = element.column(RowColumn::resolved(column, cell));
    }
    element.into_any_element()
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        github::{PrChecks, PrReviewDecision},
        ids::{HostId, WorktreeId},
        model::Degraded,
    };

    use super::*;

    use fleet_proto::snapshot::{HostStatus, Snapshot};

    fn pr(number: u64, updated: &str, cross: bool, head: &str) -> PullRequest {
        PullRequest {
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/buk/payroll/pull/{number}"),
            author: "dannyfuf".to_owned(),
            head_ref_name: head.to_owned(),
            base_ref_name: "main".to_owned(),
            is_draft: false,
            is_cross_repository: cross,
            head_repo: None,
            review_decision: PrReviewDecision::None,
            checks: PrChecks::Pass,
            checks_passed: None,
            checks_total: None,
            additions: 1,
            deletions: 1,
            labels: Vec::new(),
            updated_at: updated.to_owned(),
        }
    }

    fn worktree(branch: &str, base: &str) -> Worktree {
        Worktree {
            id: WorktreeId::try_from(format!("buk/payroll#{}", branch.replace('/', "-")))
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            slug: "local".to_owned(),
            branch: branch.to_owned(),
            base_ref: base.to_owned(),
            path: "/tmp/wt".to_owned(),
            session: "payroll/local".to_owned(),
            host: None,
            created_at: "2026-09-01T10:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        }
    }

    fn slice(prs: Vec<PullRequest>, total: usize) -> PrSlice {
        PrSlice {
            tab: PrTab::Mine,
            fetched_at: "2026-09-04T11:59:00Z".to_owned(),
            loading: false,
            error: None,
            total,
            prs,
        }
    }

    /// A snapshot carrying only what a PR row build reads through the index.
    fn snapshot(worktrees: Vec<Worktree>) -> Snapshot {
        Snapshot {
            boards: Vec::new(),
            generated_at: String::new(),
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees,
            active_context: None,
            sessions: Vec::new(),
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: String::new(),
                pid: 1,
                started_at: String::new(),
                home: String::new(),
            },
        }
    }

    fn rows(slice: &PrSlice, snapshot: &Snapshot, creating: &[(RepoId, u64)]) -> Vec<PrRow> {
        build_rows(
            &PrInputs {
                slice: Some(slice),
                worktrees: &snapshot.worktrees,
                creating,
                now: 1_788_523_200,
            },
            &SnapshotIndex::new(snapshot),
        )
    }

    #[test]
    fn rows_sort_by_update_time_and_cap_at_one_hundred() {
        let prs: Vec<PullRequest> = (1..=120)
            .map(|index| {
                pr(
                    index,
                    &format!("2026-09-0{}T10:00:00Z", (index % 9) + 1),
                    false,
                    "feat/x",
                )
            })
            .collect();
        let slice = slice(prs, 120);
        let rows = rows(&slice, &snapshot(Vec::new()), &[]);
        assert_eq!(rows.len(), ALL_SCOPE_CAP);
        assert_eq!(hidden_rows(Some(&slice)), 20);
        assert!(rows[0].age <= rows[ALL_SCOPE_CAP - 1].age);
    }

    #[test]
    fn a_pull_request_without_a_local_worktree_shows_the_dim_dot() {
        let slice = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
        let rows = rows(&slice, &snapshot(Vec::new()), &[]);
        assert_eq!(rows[0].presence, StatusKind::NoSession);
        assert_eq!(rows[0].local, None);
    }

    #[test]
    fn a_matching_worktree_lights_the_presence_glyph() {
        let slice = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
        let rows = rows(
            &slice,
            &snapshot(vec![worktree("feat/x", "origin/main")]),
            &[],
        );
        assert!(rows[0].local.is_some());
        assert_eq!(
            rows[0].presence,
            StatusKind::Unknown,
            "a worktree with no status yet is unknown, never `none`"
        );
    }

    #[test]
    fn a_pull_ref_worktree_matches_across_repositories() {
        let slice = slice(vec![pr(412, "2026-09-04T10:00:00Z", true, "feat/x")], 1);
        let rows = rows(
            &slice,
            &snapshot(vec![worktree("pr/412", "pull/412/head")]),
            &[],
        );
        assert!(rows[0].local.is_some());
    }

    #[test]
    fn creating_a_worktree_spins_the_presence_glyph_without_moving_the_row() {
        let repo = RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}"));
        let slice = slice(vec![pr(7, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
        let rows = rows(&slice, &snapshot(Vec::new()), &[(repo, 7)]);
        assert_eq!(rows[0].presence, StatusKind::JobRunning);
    }

    #[test]
    fn pr_presence_obeys_worktree_health_precedence() {
        let pull_requests = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
        let mut degraded = worktree("feat/x", "origin/main");
        degraded.degraded = Some(Degraded {
            kind: "post_create_hooks".to_owned(),
            step: "1".to_owned(),
            exit_code: Some(1),
            at: "2026-09-04T11:00:00Z".to_owned(),
            log_path: "/tmp/hooks.log".to_owned(),
        });
        let degraded_snapshot = snapshot(vec![degraded]);
        assert_eq!(
            rows(&pull_requests, &degraded_snapshot, &[])[0].presence,
            StatusKind::Degraded
        );

        let mut remote = worktree("feat/x", "origin/main");
        let host = HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}"));
        remote.host = Some(host.clone());
        let mut offline_snapshot = snapshot(vec![remote]);
        offline_snapshot.hosts.push(HostStatus {
            id: host,
            provider: "tailscale".to_owned(),
            version: None,
            link: fleet_proto::snapshot::LinkState::Down,
            address: None,
            agent_binaries: None,
            reachable: true,
            checked_at: "2026-09-04T11:59:00Z".to_owned(),
            error: Some("ssh timed out".to_owned()),
        });
        assert_eq!(
            rows(&pull_requests, &offline_snapshot, &[])[0].presence,
            StatusKind::HostUnreachable
        );
        let row = &rows(&pull_requests, &offline_snapshot, &[])[0];
        assert_eq!(row.host.as_ref().map(HostId::as_str), Some("devbox"));
        assert_eq!(row.host_link, Some(LinkState::Down));
    }

    #[test]
    fn refreshing_counts_are_consistently_marked() {
        let cached = tab_of("Mine", Some(7));
        assert_eq!(cached.count, Some(7));
        assert!(!cached.loading);
        assert_eq!(cached.count_text().as_deref(), Some("7"));
        assert_eq!(
            fetch_stamp_text(true, Some(120)).as_ref(),
            "\u{27F3} refreshing · fetched 2m ago"
        );

        let settled = tab_of("Review", Some(4));
        assert_eq!(settled.count_text().as_deref(), Some("4"));
        let cold = tab_of("Review", None);
        assert!(cold.loading);
        assert_eq!(cold.count_text().as_deref(), Some("\u{2026}"));
    }

    #[test]
    fn badge_states_follow_the_documented_priority() {
        assert_eq!(
            pr_badge_state(derive_pr_state(
                true,
                PrChecks::Fail,
                PrReviewDecision::Approved
            )),
            PrBadgeState::Draft
        );
        assert_eq!(
            pr_badge_state(derive_pr_state(
                false,
                PrChecks::Fail,
                PrReviewDecision::Approved
            )),
            PrBadgeState::CiFail
        );
        assert_eq!(
            pr_badge_state(derive_pr_state(
                false,
                PrChecks::Pass,
                PrReviewDecision::Approved
            )),
            PrBadgeState::Approved
        );
    }

    #[test]
    fn the_filter_matches_number_title_author_and_branch() {
        let slice = slice(vec![pr(412, "2026-09-04T10:00:00Z", false, "feat/rut")], 1);
        let rows = rows(&slice, &snapshot(Vec::new()), &[]);
        assert!(matches(&rows[0], "412"));
        assert!(matches(&rows[0], "pr 412"));
        assert!(matches(&rows[0], "danny"));
        assert!(matches(&rows[0], "rut"));
        assert!(!matches(&rows[0], "nixos"));
    }
}
