use super::*;

/// What `Enter` on a PR with no local worktree is about to create (§3.5 [D-6]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct WillCreate {
    /// The proposed worktree, `repo/slug`.
    worktree: String,
    /// The local branch: `headRefName`, or `pr/<n>` across repositories.
    branch: String,
    /// The base ref persisted on the worktree.
    base: String,
    /// `dannyfuf/payroll → pr/412`, only for a cross-repository PR.
    fork: Option<String>,
}

/// Derives the §3.5 `WILL CREATE` block for one pull request.
fn will_create(pr: &PullRequest) -> WillCreate {
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
            FactValue::known(truncate(&pr.url, path_budget(cx), Truncate::Middle)),
        );

    let tail = match local {
        Some(worktree) => {
            let glyph = row_glyph(
                status.map_or(SessionState::None, |status| status.session),
                false,
                status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
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
                    FactRow::new("path", path_value(&worktree.path, home, cx))
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
    variant(vec![
        head(
            Icon::GitPullRequest,
            SharedString::from(format!("#{}  {}", pr.number, pr.title)),
            SharedString::from(format!("{} · {}", pr.repo_id, pr.author)),
            cx,
        ),
        block(
            vec![PrBadge::state_only(pr_badge_state(state)).into_any_element()],
            cx,
        ),
        block(vec![table.into_any_element()], cx),
        tail,
    ])
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

#[cfg(test)]
mod tests {
    use fleet_core::ids::RepoId;

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
    }
}
