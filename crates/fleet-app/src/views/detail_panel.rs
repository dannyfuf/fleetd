//! The Hub's detail panel (UX-SPEC §3.4).
//!
//! Everything deliberately kept out of a row, on demand, in one 340 px column — with the age
//! of every job-derived fact. The panel is opened by `i`, mirrors the cursor row, and is
//! **never in the focus cycle** (KEYMAP A1): it takes no key of its own, so nothing here
//! listens for actions.
//!
//! Three renderings of §1.3 are enforced by construction rather than by convention:
//!
//! * a nullable inspection fact renders `—` through [`FactValue::from_option`], never `0`;
//! * every `warnings[]` string is rendered **verbatim**, never paraphrased;
//! * a running inspection dims the previous values to 60 % and never blanks them.

use fleet_core::{
    github::{PrChecks, PrReviewDecision, PullRequest, derive_pr_state, local_branch_for_pr},
    ids::WorktreeId,
    inspection::WorktreeInspection,
    model::{CloneJob, CloneStatus, Repo, Worktree},
    sessions::{SessionState, WorktreeStatus},
};
use fleet_proto::snapshot::PoolStatus;
use fleet_ui_kit::{
    ActiveTheme, FactRow, FactValue, FreshnessStamp, Icon, IconSize, KeyValueList, Pane,
    PaneBorder, PrBadge, SectionHeader, Sheet, StatusGlyph, StatusKind, Text, Tone, Truncate,
    format_age, truncate,
};
use gpui::{AnyElement, App, IntoElement, SharedString, div, prelude::*, px};

use crate::{
    screens::hub::{Inspected, age_secs},
    views::{prs_screen::pr_badge_state, worktrees_list::row_glyph},
};

/// Width of the panel when the window is too narrow to inset it (§3.4).
pub const OVERLAY_WIDTH: f32 = 320.0;
/// The panel's two 12 px gutters (`theme.space.md`), together.
const GUTTERS: f32 = 24.0;
/// The label column of a fact row (`theme.metrics.fact_label_w`).
const LABEL_W: f32 = 104.0;
/// The gap between the label and the value column (`theme.space.sm`).
const LABEL_GAP: f32 = 8.0;

/// How many characters of the mono face fit in `width` pixels.
const fn mono_budget(width: f32) -> usize {
    // `as` truncates towards zero, which is exactly the number of whole columns that fit.
    (width / fleet_ui_kit::theme::CH) as usize
}

/// Character budget of a path shown in a fact row's **value column**.
///
/// Derived from the column, never guessed, and sized for the narrower of the panel's two
/// surfaces so it holds in both: a budget wider than the column produces a middle ellipsis
/// *and* a wrapped second line on the same path, which is a lie about where the path was cut.
const PATH_BUDGET: usize = mono_budget(OVERLAY_WIDTH - GUTTERS - LABEL_W - LABEL_GAP);
/// Character budget of a full-width mono line in the panel body (a hook command).
const LINE_BUDGET: usize = mono_budget(OVERLAY_WIDTH - GUTTERS);

/// Collapses `$HOME` to `~` so the path fits and reads like the shell prints it.
#[must_use]
pub fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) => format!("~{rest}"),
        None => path.to_owned(),
    }
}

/// Wraps a panel body in its surface: an inset [`Pane`] normally, a docked [`Sheet`] when the
/// window is below 1120 px and the list must not lose width (§3.4).
#[must_use]
pub fn panel(body: AnyElement, as_overlay: bool) -> AnyElement {
    if as_overlay {
        Sheet::new(true)
            .width(px(OVERLAY_WIDTH))
            .body(body)
            .into_any_element()
    } else {
        Pane::fixed(px(340.0))
            .border(PaneBorder::Left)
            .raised(true)
            .body(body)
            .into_any_element()
    }
}

/// The panel's two-line head: `⑂ branch` over a `fg.muted` provenance line.
fn head(icon: Icon, title: SharedString, subtitle: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .px(theme.space.md)
        .pt(theme.space.md)
        .gap(theme.space.xxs)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(icon.el().size(IconSize::Large))
                .child(Text::title(title).ellipsize()),
        )
        .child(Text::ui(subtitle).muted().ellipsize())
        .into_any_element()
}

/// A vertical stack with the panel's 12 px padding.
fn block(children: Vec<AnyElement>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .px(theme.space.md)
        .pt(theme.space.md)
        .gap(theme.space.xs)
        .children(children)
        .into_any_element()
}

/// Everything the worktree variant needs.
pub struct WorktreeProps<'a> {
    /// The worktree under the cursor.
    pub worktree: &'a Worktree,
    /// Its runtime status, for the SESSION block.
    pub status: Option<&'a WorktreeStatus>,
    /// Whether its session is slept rather than merely detached.
    pub slept: bool,
    /// Whether its host's last probe failed.
    pub host_unreachable: bool,
    /// The inspection cache entry, which may be missing, loading or errored.
    pub inspected: Option<&'a Inspected>,
    /// `$FLEET_HOME`'s parent, for tilde collapsing.
    pub home: &'a str,
    /// The current epoch second.
    pub now: i64,
}

/// §3.4's worktree panel: head, `path`, SESSION, SAFETY, times, freshness footer.
#[must_use]
pub fn worktree(props: WorktreeProps<'_>, cx: &App) -> AnyElement {
    let WorktreeProps {
        worktree,
        status,
        slept,
        host_unreachable,
        inspected,
        home,
        now,
    } = props;

    let host = worktree
        .host
        .as_ref()
        .map_or_else(|| "local".to_owned(), |host| format!("@{host}"));
    let subtitle = format!("{} · {} · {host}", worktree.repo_id, worktree.base_ref);

    let session_state = status.map_or(SessionState::None, |status| status.session);
    let glyph = row_glyph(
        session_state,
        slept,
        worktree.degraded.is_some(),
        host_unreachable,
        false,
    );

    let mut children = vec![
        head(
            Icon::GitBranch,
            SharedString::from(worktree.branch.clone()),
            SharedString::from(subtitle),
            cx,
        ),
        block(
            vec![
                FactRow::new(
                    "path",
                    FactValue::known(truncate(
                        &tilde(&worktree.path, home),
                        PATH_BUDGET,
                        Truncate::Middle,
                    )),
                )
                .mono(true)
                .into_any_element(),
            ],
            cx,
        ),
        session_block(glyph, status, cx),
    ];
    children.push(safety_block(inspected, now, cx));
    children.push(times_block(worktree, now, cx));
    if let Some(footer) = freshness_footer(inspected, now) {
        children.push(footer);
    }

    div()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .children(children)
        .into_any_element()
}

/// `SESSION  ◉ attached` plus one row per terminal (§3.4) — the only place window names exist.
fn session_block(glyph: StatusKind, status: Option<&WorktreeStatus>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let header = SectionHeader::new("Session").trailing(
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .child(
                StatusGlyph::new(glyph)
                    .size(IconSize::Medium)
                    .id("detail-session-glyph"),
            )
            .child(Text::ui(glyph.detail_word()).muted()),
    );
    let windows: Vec<AnyElement> = status
        .map(|status| {
            status
                .windows
                .iter()
                .map(|window| {
                    let label = if window.keep_alive.is_empty() {
                        FactValue::Null
                    } else {
                        FactValue::known(window.keep_alive.join(", "))
                    };
                    FactRow::new(format!("{} {}", window.index + 1, window.name), label)
                        .into_any_element()
                })
                .collect()
        })
        .unwrap_or_default();

    let mut children = vec![header.into_any_element()];
    children.extend(windows);
    block(children, cx)
}

/// The SAFETY block: exactly the facts the delete confirm will quote, plus their age.
fn safety_block(inspected: Option<&Inspected>, now: i64, cx: &App) -> AnyElement {
    let Some(inspected) = inspected else {
        return block(
            vec![
                SectionHeader::new("Safety").into_any_element(),
                Text::ui("not checked · I to check")
                    .faint()
                    .into_any_element(),
            ],
            cx,
        );
    };
    if let Some(error) = inspected.error.as_deref() {
        return block(
            vec![
                SectionHeader::new("Safety").into_any_element(),
                Text::ui(format!("error: {error}"))
                    .tone(Tone::Danger)
                    .into_any_element(),
                Text::hint("I  retry").into_any_element(),
            ],
            cx,
        );
    }
    let Some(data) = inspected.data.as_ref() else {
        return block(
            vec![
                SectionHeader::new("Safety")
                    .trailing(Text::ui("\u{27F3} checking\u{2026}").muted())
                    .into_any_element(),
            ],
            cx,
        );
    };

    let age = age_secs(&data.inspected_at, now).unwrap_or_default();
    let mut list = KeyValueList::titled("Safety")
        .trailing(FreshnessStamp::new("checked", age))
        .row("dirty", dirty_value(data))
        .row(
            "ahead / behind",
            match (data.ahead, data.behind) {
                (Some(ahead), Some(behind)) => {
                    FactValue::known(format!("\u{21E1}{ahead} \u{21E3}{behind}"))
                }
                _ => FactValue::Null,
            },
        )
        .row(
            "unique commits",
            FactValue::from_option(data.unique_commits.map(|count| count.to_string())),
        )
        .row("published", FactValue::known(yes_no(data.published)))
        .row("merged", FactValue::known(yes_no(data.merged)));
    if let Some(pr) = data.pr.as_ref() {
        list = list.row(
            "PR",
            FactValue::known(format!("#{} {}", pr.number, pr_state_word(pr.state))),
        );
    }

    let mut children = vec![list.into_any_element()];
    children.extend(
        data.warnings
            .iter()
            .map(|warning| FactRow::warning(warning.clone()).into_any_element()),
    );

    let body = block(children, cx);
    if inspected.loading {
        // §3.4: a running inspection dims the previous values, it never blanks them.
        return div().opacity(0.6).child(body).into_any_element();
    }
    body
}

/// `dirty  12 files` — a file count when the status collection succeeded.
fn dirty_value(data: &WorktreeInspection) -> FactValue {
    match (data.dirty, data.dirty_files) {
        (true, Some(count)) => FactValue::known(format!("{count} files")),
        (true, None) => FactValue::known("yes"),
        (false, _) => FactValue::known("clean"),
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn pr_state_word(state: fleet_core::github::InspectionPrState) -> &'static str {
    match state {
        fleet_core::github::InspectionPrState::Open => "open",
        fleet_core::github::InspectionPrState::Merged => "merged",
        fleet_core::github::InspectionPrState::Closed => "closed",
    }
}

/// `opened 2h ago · created 5d ago` — low priority by definition, so it sits last.
fn times_block(worktree: &Worktree, now: i64, cx: &App) -> AnyElement {
    let opened = worktree
        .last_opened_at
        .as_deref()
        .and_then(|iso| age_secs(iso, now))
        .map(|age| format!("opened {} ago", format_age(age)));
    let created =
        age_secs(&worktree.created_at, now).map(|age| format!("created {} ago", format_age(age)));
    let line = [opened, created]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    block(vec![Text::ui(line).muted().into_any_element()], cx)
}

/// `inspected <age> · I refresh`, shown only past 60 s (§2.6) so fresh facts stay quiet.
fn freshness_footer(inspected: Option<&Inspected>, now: i64) -> Option<AnyElement> {
    let data = inspected?.data.as_ref()?;
    let age = age_secs(&data.inspected_at, now)?;
    if age <= 60 {
        return None;
    }
    Some(
        div()
            .px(px(12.0))
            .pt(px(8.0))
            .child(FreshnessStamp::new("inspected", age).action("I", "refresh"))
            .into_any_element(),
    )
}

/// Everything the repository variant needs.
pub struct RepoProps<'a> {
    /// The repository under the cursor.
    pub repo: &'a Repo,
    /// How many worktrees it owns.
    pub worktrees: usize,
    /// How many of them have an attached session.
    pub live: usize,
    /// Its prepared-copy pool, when the daemon reports one.
    pub pool: Option<&'a PoolStatus>,
    /// `$HOME`, for tilde collapsing.
    pub home: &'a str,
    /// The current epoch second.
    pub now: i64,
}

/// §3.4's repo variant.
#[must_use]
pub fn repo(props: RepoProps<'_>, cx: &App) -> AnyElement {
    let RepoProps {
        repo,
        worktrees,
        live,
        pool,
        home,
        now,
    } = props;
    let pool_line = pool.map(|pool| {
        let refreshed = pool
            .refreshed_at
            .as_deref()
            .and_then(|iso| age_secs(iso, now))
            .map(|age| format!(" · refreshed {} ago", format_age(age)))
            .unwrap_or_default();
        format!("{}/{} ready{refreshed}", pool.ready, pool.size)
    });

    let mut list = KeyValueList::new()
        .row("owner", FactValue::known(repo.owner.clone()))
        .row("default", FactValue::known(repo.default_branch.clone()))
        .mono_row(
            "path",
            FactValue::known(truncate(
                &tilde(&repo.path, home),
                PATH_BUDGET,
                Truncate::Middle,
            )),
        )
        .row("worktrees", FactValue::known(worktrees.to_string()))
        .row("live", FactValue::known(live.to_string()))
        .row(
            "prepare",
            FactValue::from_option(
                (!repo.hooks.prepare.is_empty())
                    .then(|| format!("{} commands", repo.hooks.prepare.len())),
            ),
        )
        .row(
            "post-create",
            FactValue::from_option(
                (!repo.hooks.post_create.is_empty())
                    .then(|| format!("{} commands", repo.hooks.post_create.len())),
            ),
        )
        .mono_row("url", FactValue::known(repo.url.clone()));
    if let Some(pool_line) = pool_line {
        list = list.row("prepared", FactValue::known(pool_line));
    }

    let hooks: Vec<AnyElement> = repo
        .hooks
        .prepare
        .iter()
        .chain(repo.hooks.post_create.iter())
        .map(|command| {
            Text::data_small(truncate(command, LINE_BUDGET, Truncate::Tail))
                .muted()
                .into_any_element()
        })
        .collect();

    let mut children = vec![
        head(
            Icon::FolderGit2,
            SharedString::from(repo.name.clone()),
            SharedString::from(repo.id.to_string()),
            cx,
        ),
        block(vec![list.into_any_element()], cx),
    ];
    if !hooks.is_empty() {
        let mut hook_block = vec![SectionHeader::new("Hooks").into_any_element()];
        hook_block.extend(hooks);
        children.push(block(hook_block, cx));
    }
    div()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .children(children)
        .into_any_element()
}

/// §3.4's clone-job variant: status, staging path, log path, error in red.
#[must_use]
pub fn clone_job(clone: &CloneJob, home: &str, cx: &App) -> AnyElement {
    let status = match clone.status {
        CloneStatus::Starting => "starting",
        CloneStatus::Cloning => "cloning",
        CloneStatus::Failed => "failed",
    };
    let mut children = vec![
        head(
            Icon::CloudDownload,
            SharedString::from(clone.name.clone()),
            SharedString::from(clone.id.to_string()),
            cx,
        ),
        block(
            vec![
                KeyValueList::new()
                    .row("status", FactValue::known(status))
                    .mono_row(
                        "staging",
                        FactValue::known(truncate(
                            &tilde(&clone.staging_path, home),
                            PATH_BUDGET,
                            Truncate::Middle,
                        )),
                    )
                    .mono_row(
                        "log",
                        FactValue::known(truncate(
                            &tilde(&clone.log_path, home),
                            PATH_BUDGET,
                            Truncate::Middle,
                        )),
                    )
                    .into_any_element(),
            ],
            cx,
        ),
    ];
    if let Some(error) = clone.error.as_deref() {
        children.push(block(
            vec![
                Text::ui(error.to_owned())
                    .tone(Tone::Danger)
                    .into_any_element(),
                Text::hint("\u{23CE}  open in the jobs panel").into_any_element(),
            ],
            cx,
        ));
    }
    div()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .children(children)
        .into_any_element()
}

/// What `Enter` on a PR with no local worktree is about to create (§3.5 [D-6]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WillCreate {
    /// The proposed worktree, `repo/slug`.
    pub worktree: String,
    /// The local branch: `headRefName`, or `pr/<n>` across repositories.
    pub branch: String,
    /// The base ref persisted on the worktree.
    pub base: String,
    /// `dannyfuf/payroll → pr/412`, only for a cross-repository PR.
    pub fork: Option<String>,
}

/// Derives the §3.5 `WILL CREATE` block for one pull request.
#[must_use]
pub fn will_create(pr: &PullRequest) -> WillCreate {
    let branch = local_branch_for_pr(pr);
    WillCreate {
        worktree: format!("{}/{}", pr.repo_id.name(), branch.replace('/', "-")),
        branch: branch.clone(),
        base: format!("pull/{}/head", pr.number),
        fork: pr
            .head_repo
            .as_ref()
            .filter(|_| pr.is_cross_repository)
            .map(|head| format!("{head} \u{2192} {branch}")),
    }
}

/// Everything the pull-request variant needs.
pub struct PrProps<'a> {
    /// The pull request under the cursor.
    pub pr: &'a PullRequest,
    /// The local worktree matching it, when one exists.
    pub local: Option<&'a Worktree>,
    /// That worktree's runtime status.
    pub status: Option<&'a WorktreeStatus>,
    /// `$HOME`, for tilde collapsing.
    pub home: &'a str,
    /// The current epoch second.
    pub now: i64,
}

/// §3.5's PR detail table, plus either the `WORKTREE` block or `WILL CREATE`.
#[must_use]
pub fn pull_request(props: PrProps<'_>, cx: &App) -> AnyElement {
    let PrProps {
        pr,
        local,
        status,
        home,
        now,
    } = props;
    let updated = age_secs(&pr.updated_at, now)
        .map(|age| format!("{} ago", format_age(age)))
        .unwrap_or_else(|| "\u{2014}".to_owned());
    let checks = match (pr.checks, pr.checks_passed, pr.checks_total) {
        (PrChecks::None, _, _) => FactValue::Null,
        (state, Some(passed), Some(total)) => {
            FactValue::known(format!("{} · {passed} of {total}", checks_word(state)))
        }
        (state, _, _) => FactValue::known(checks_word(state)),
    };

    let table = KeyValueList::new()
        .row("target", FactValue::known(pr.base_ref_name.clone()))
        .row(
            "diff",
            FactValue::known(format!("+{} \u{2212}{}", pr.additions, pr.deletions)),
        )
        .row("checks", checks)
        .row("review", FactValue::known(review_word(pr.review_decision)))
        .row(
            "labels",
            FactValue::from_option((!pr.labels.is_empty()).then(|| pr.labels.join(", "))),
        )
        .row("updated", FactValue::known(updated))
        .mono_row(
            "url",
            FactValue::known(truncate(&pr.url, PATH_BUDGET, Truncate::Middle)),
        );

    let tail = match local {
        Some(worktree) => {
            let glyph = row_glyph(
                status.map_or(SessionState::None, |status| status.session),
                false,
                worktree.degraded.is_some(),
                false,
                false,
            );
            let running = status
                .map(|status| status.running.join(", "))
                .filter(|labels| !labels.is_empty());
            block(
                vec![
                    SectionHeader::new("Worktree").into_any_element(),
                    FactRow::new(
                        "path",
                        FactValue::known(truncate(
                            &tilde(&worktree.path, home),
                            PATH_BUDGET,
                            Truncate::Middle,
                        )),
                    )
                    .mono(true)
                    .into_any_element(),
                    FactRow::new(
                        "session",
                        FactValue::known(match running {
                            Some(labels) => format!("{} · {labels}", glyph.detail_word()),
                            None => glyph.detail_word().to_owned(),
                        }),
                    )
                    .into_any_element(),
                ],
                cx,
            )
        }
        None => {
            let plan = will_create(pr);
            let mut rows = vec![
                SectionHeader::new("Will create").into_any_element(),
                FactRow::new("worktree", FactValue::known(plan.worktree)).into_any_element(),
                FactRow::new("branch", FactValue::known(plan.branch)).into_any_element(),
                FactRow::new("base", FactValue::known(plan.base)).into_any_element(),
            ];
            if let Some(fork) = plan.fork {
                rows.push(FactRow::new("fork", FactValue::known(fork)).into_any_element());
            }
            block(rows, cx)
        }
    };

    let state = derive_pr_state(pr.is_draft, pr.checks, pr.review_decision);
    div()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .child(head(
            Icon::GitPullRequest,
            SharedString::from(format!("#{}  {}", pr.number, pr.title)),
            SharedString::from(format!("{} · {}", pr.repo_id, pr.author)),
            cx,
        ))
        .child(block(
            vec![PrBadge::state_only(pr_badge_state(state)).into_any_element()],
            cx,
        ))
        .child(block(vec![table.into_any_element()], cx))
        .child(tail)
        .into_any_element()
}

fn checks_word(checks: PrChecks) -> &'static str {
    match checks {
        PrChecks::Pass => "pass",
        PrChecks::Fail => "fail",
        PrChecks::Pending => "pending",
        PrChecks::None => "none",
    }
}

fn review_word(decision: PrReviewDecision) -> &'static str {
    match decision {
        PrReviewDecision::Approved => "approved",
        PrReviewDecision::ChangesRequested => "changes requested",
        PrReviewDecision::ReviewRequired => "review required",
        PrReviewDecision::None => "none",
    }
}

/// The worktree a PR panel points at, by the §1 match rule.
#[must_use]
pub fn local_worktree_id(pr: &PullRequest, worktrees: &[Worktree]) -> Option<WorktreeId> {
    worktrees
        .iter()
        .find(|worktree| fleet_core::github::worktree_matches_pr(worktree, pr))
        .map(|worktree| worktree.id.clone())
}

#[cfg(test)]
mod tests {
    use fleet_core::{github::PrTab, ids::RepoId};

    use super::*;

    fn pull_request(cross: bool) -> PullRequest {
        PullRequest {
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            number: 412,
            title: "Fix RUT validation".to_owned(),
            url: "https://github.com/buk/payroll/pull/412".to_owned(),
            author: "dannyfuf".to_owned(),
            head_ref_name: "feat/rut".to_owned(),
            base_ref_name: "main".to_owned(),
            is_draft: false,
            is_cross_repository: cross,
            head_repo: cross
                .then(|| RepoId::try_from("dannyfuf/payroll").unwrap_or_else(|e| panic!("{e}"))),
            review_decision: PrReviewDecision::None,
            checks: PrChecks::Pass,
            checks_passed: Some(9),
            checks_total: Some(9),
            additions: 142,
            deletions: 18,
            labels: vec!["payroll".to_owned()],
            updated_at: "2026-09-04T10:00:00Z".to_owned(),
        }
    }

    #[test]
    fn the_path_budget_is_the_column_it_is_drawn_in() {
        // The derivation only holds while it mirrors the tokens it was derived from.
        let theme = fleet_ui_kit::Theme::dark();
        assert_eq!(f32::from(theme.space.md) * 2.0, GUTTERS);
        assert_eq!(f32::from(theme.metrics.fact_label_w), LABEL_W);
        assert_eq!(f32::from(theme.space.sm), LABEL_GAP);

        // A middle-ellipsised path must fit its column on one line, on both surfaces.
        let column = OVERLAY_WIDTH - GUTTERS - LABEL_W - LABEL_GAP;
        let path = tilde("/home/u/fleet-review-ux/widgets/feature-one", "/home/u");
        let drawn = truncate(&path, PATH_BUDGET, Truncate::Middle);
        assert!(
            drawn.chars().count() as f32 * fleet_ui_kit::theme::CH <= column,
            "`{drawn}` is {} chars, wider than the {column} px value column",
            drawn.chars().count()
        );
        assert!(
            drawn.contains('\u{2026}'),
            "a path longer than the column is still middle-ellipsised: {drawn}"
        );
    }

    #[test]
    fn tilde_collapses_only_the_home_prefix() {
        assert_eq!(tilde("/home/u/.fleet/x", "/home/u"), "~/.fleet/x");
        assert_eq!(tilde("/opt/x", "/home/u"), "/opt/x");
    }

    #[test]
    fn same_repo_pull_requests_reuse_the_head_branch() {
        let plan = will_create(&pull_request(false));
        assert_eq!(plan.branch, "feat/rut");
        assert_eq!(plan.base, "pull/412/head");
        assert_eq!(plan.worktree, "payroll/feat-rut");
        assert_eq!(plan.fork, None);
    }

    #[test]
    fn cross_repository_pull_requests_get_a_pr_branch_and_a_fork_line() {
        let plan = will_create(&pull_request(true));
        assert_eq!(plan.branch, "pr/412");
        assert_eq!(
            plan.fork.as_deref(),
            Some("dannyfuf/payroll \u{2192} pr/412")
        );
        let _ = PrTab::Mine;
    }

    #[test]
    fn null_facts_never_render_as_zero() {
        assert_eq!(
            FactValue::from_option(None::<String>),
            FactValue::Null,
            "a nullable inspection fact renders the dash, never 0"
        );
    }
}
