//! The Hub's pull-requests screen (UX-SPEC §3.5).
//!
//! *What am I waiting on, what is waiting on me, and can I start on it in one key?* The screen
//! replaces the worktrees list in place (`p`), keeps the repos rail, and adds one thing the
//! worktrees list cannot have: the **local-presence glyph**, which decides whether `Enter`
//! opens an existing worktree or creates one.
//!
//! The fetch is cached in the daemon (`github.prTtlSeconds: 90`), so every rendering here is a
//! function of a [`PrSlice`] plus its age; stale rows stay listed and never disappear behind a
//! spinner (§3.5 *Loading with cache*).

use fleet_core::{
    github::{PrState, PrTab, PullRequest, derive_pr_state, worktree_matches_pr},
    ids::{RepoId, WorktreeId},
    model::Worktree,
    sessions::{AgentActivity, Session, SessionKind, SessionState, WorktreeStatus},
};
use fleet_proto::response::PrSlice;
use fleet_ui_kit::{
    ActiveTheme, ColumnLadder, Icon, IconSize, ListView, Pane, PaneBorder, PrBadge, PrBadgeState,
    ResolvedColumn, Row, RowColumn, SegmentedTab, SegmentedTabs, SkeletonRows, StatusGlyph,
    StatusKind, Text, Tone, Truncate, format_age, truncate,
};
use gpui::{AnyElement, App, IntoElement, SharedString, UniformListScrollHandle, div, prelude::*};

use crate::{screens::hub::age_secs, views::worktrees_list::session_glyph};

/// The `All` scope cap of §3.5 [D-7]: never a refusal, always a final "narrow me" row.
pub const ALL_SCOPE_CAP: usize = 100;
/// Character budget of the `headRefName` column (§2.9 column 5).
const HEAD_BUDGET: usize = 12;
/// Character budget of the `owner/name` column (§2.9 column 6).
const REPO_BUDGET: usize = 10;
/// How many skeleton rows a cold load shows (§3.5).
const SKELETON_ROWS: usize = 6;

/// Maps the domain's `PrState` onto the kit's badge, which knows nothing about GitHub.
#[must_use]
pub fn pr_badge_state(state: PrState) -> PrBadgeState {
    match state {
        PrState::Draft => PrBadgeState::Draft,
        PrState::CiFail => PrBadgeState::CiFail,
        PrState::Changes => PrBadgeState::Changes,
        PrState::CiPending => PrBadgeState::CiPending,
        PrState::Approved => PrBadgeState::Approved,
        PrState::Review => PrBadgeState::Review,
    }
}

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
    /// The PR's canonical URL, for `y` and `b`.
    pub url: SharedString,
}

/// The local-presence glyph and worktree of one PR (§3.5, match rule §1).
#[must_use]
pub fn presence(
    pr: &PullRequest,
    worktrees: &[Worktree],
    statuses: &[WorktreeStatus],
    sessions: &[Session],
    creating: bool,
) -> (StatusKind, Option<WorktreeId>) {
    if creating {
        return (StatusKind::JobRunning, None);
    }
    let Some(worktree) = worktrees
        .iter()
        .find(|worktree| worktree_matches_pr(worktree, pr))
    else {
        return (StatusKind::NoSession, None);
    };
    // A worktree exists but its status has not landed yet: `unknown`, never `none` (§1.3).
    let status = statuses
        .iter()
        .find(|status| status.worktree_id == worktree.id);
    let session = status.map_or(SessionState::Unknown, |status| status.session);
    let slept = sessions.iter().any(|session| {
        matches!(&session.kind, SessionKind::Worktree(id) if id == &worktree.id)
            && session.slept_at.is_some()
    });
    let kind = session_glyph(
        session,
        slept,
        status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
    );
    (kind, Some(worktree.id.clone()))
}

/// Everything the model needs to turn a slice into rows.
pub struct PrInputs<'a> {
    /// The daemon's slice for the active tab.
    pub slice: Option<&'a PrSlice>,
    /// Every local worktree in scope.
    pub worktrees: &'a [Worktree],
    /// Their runtime statuses.
    pub statuses: &'a [WorktreeStatus],
    /// Daemon sessions, for the awake / slept distinction.
    pub sessions: &'a [Session],
    /// PRs whose worktree is being created right now.
    pub creating: &'a [(RepoId, u64)],
    /// The current epoch second.
    pub now: i64,
}

/// Builds the rows of one tab, sorted by `updatedAt` desc and capped at [`ALL_SCOPE_CAP`].
#[must_use]
pub fn build_rows(inputs: &PrInputs<'_>) -> Vec<PrRow> {
    let Some(slice) = inputs.slice else {
        return Vec::new();
    };
    let mut prs: Vec<&PullRequest> = slice.prs.iter().collect();
    prs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    prs.into_iter()
        .take(ALL_SCOPE_CAP)
        .map(|pr| {
            let creating = inputs
                .creating
                .iter()
                .any(|(repo, number)| repo == &pr.repo_id && *number == pr.number);
            let (presence_glyph, local) = presence(
                pr,
                inputs.worktrees,
                inputs.statuses,
                inputs.sessions,
                creating,
            );
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
        || row.title.to_lowercase().contains(&needle)
        || row.author.to_lowercase().contains(&needle)
        || row.head.to_lowercase().contains(&needle)
}

/// Everything the screen needs to draw itself.
pub struct PrProps {
    /// A live filter editor that replaces the tab header.
    pub header_override: Option<AnyElement>,
    /// The rows of the active tab, already filtered.
    pub rows: Vec<PrRow>,
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
#[must_use]
pub fn render(props: PrProps, scroll: &UniformListScrollHandle, cx: &App) -> AnyElement {
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

    let stamp = match (loading, fetched_age) {
        (true, Some(age)) => Text::ui(format!(
            "\u{27F3} refreshing · fetched {} ago",
            format_age(age)
        ))
        .muted(),
        (true, None) => Text::ui("\u{27F3} refreshing").muted(),
        (false, Some(age)) => {
            let text = Text::ui(format!("fetched {} ago", format_age(age)));
            if error.is_some() {
                text.tone(Tone::Warning)
            } else {
                text.muted()
            }
        }
        (false, None) => Text::ui("never fetched").muted(),
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
        (Some(query), _) => crate::views::first_run::empty_state("filter", Some(&query)),
        (None, PrTab::Mine) => crate::views::first_run::empty_state("prs-mine", Some(&scope)),
        (None, PrTab::Review) => crate::views::first_run::empty_state("prs-review", Some(&scope)),
    };

    let ladder = ColumnLadder::pull_requests();
    let columns: Vec<ResolvedColumn> = ladder
        .resolve(pane_ch)
        .into_iter()
        .filter(|column| column.key.as_ref() != "repo" || multi_repo)
        .collect();
    let body_rows = rows.clone();
    let list = ListView::new("hub-prs", rows.len(), move |index, is_cursor, _w, cx| {
        let Some(row) = body_rows.get(index) else {
            return div().into_any_element();
        };
        pr_row(row, is_cursor, focused, &columns, cx)
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
    match count {
        Some(count) => SegmentedTab::new(label, count),
        None => SegmentedTab::bare(label).loading(true),
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
                    .map_or_else(
                        fleet_ui_kit::AgeLabel::none,
                        fleet_ui_kit::AgeLabel::from_secs,
                    )
                    .into_any_element(),
            ),
            _ => None,
        };
        let Some(cell) = cell else { continue };
        let wrapped = match column.width {
            Some(width) => RowColumn::fixed(width, cell),
            None => RowColumn::flex(cell),
        };
        element = element.column(wrapped.align(column.align));
    }
    element.into_any_element()
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        github::{PrChecks, PrReviewDecision},
        ids::WorktreeId,
    };

    use super::*;

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
            id: WorktreeId::try_from(format!("buk/payroll#{branch}").replace('/', "-"))
                .or_else(|_| WorktreeId::try_from("buk/payroll#local"))
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

    fn inputs<'a>(slice: &'a PrSlice, worktrees: &'a [Worktree]) -> PrInputs<'a> {
        PrInputs {
            slice: Some(slice),
            worktrees,
            statuses: &[],
            sessions: &[],
            creating: &[],
            now: 1_788_523_200,
        }
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
        let rows = build_rows(&inputs(&slice, &[]));
        assert_eq!(rows.len(), ALL_SCOPE_CAP);
        assert_eq!(hidden_rows(Some(&slice)), 20);
        assert!(rows[0].age <= rows[ALL_SCOPE_CAP - 1].age);
    }

    #[test]
    fn a_pull_request_without_a_local_worktree_shows_the_dim_dot() {
        let slice = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
        let rows = build_rows(&inputs(&slice, &[]));
        assert_eq!(rows[0].presence, StatusKind::NoSession);
        assert_eq!(rows[0].local, None);
    }

    #[test]
    fn a_matching_worktree_lights_the_presence_glyph() {
        let slice = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
        let worktrees = vec![worktree("feat/x", "origin/main")];
        let rows = build_rows(&inputs(&slice, &worktrees));
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
        let worktrees = vec![worktree("pr/412", "pull/412/head")];
        let rows = build_rows(&inputs(&slice, &worktrees));
        assert!(rows[0].local.is_some());
    }

    #[test]
    fn creating_a_worktree_spins_the_presence_glyph_without_moving_the_row() {
        let repo = RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}"));
        let slice = slice(vec![pr(7, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
        let creating = vec![(repo, 7)];
        let inputs = PrInputs {
            slice: Some(&slice),
            worktrees: &[],
            statuses: &[],
            sessions: &[],
            creating: &creating,
            now: 1_788_523_200,
        };
        let rows = build_rows(&inputs);
        assert_eq!(rows[0].presence, StatusKind::JobRunning);
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
        let rows = build_rows(&inputs(&slice, &[]));
        assert!(matches(&rows[0], "412"));
        assert!(matches(&rows[0], "pr 412"));
        assert!(matches(&rows[0], "danny"));
        assert!(matches(&rows[0], "rut"));
        assert!(!matches(&rows[0], "nixos"));
    }
}
