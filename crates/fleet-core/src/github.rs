//! GitHub pull-request and repository metadata types.

use serde::{Deserialize, Serialize};

use crate::{ids::RepoId, model::Worktree};

/// Pull-request list tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrTab {
    /// Pull requests authored by the viewer.
    Mine,
    /// Pull requests awaiting the viewer's review.
    Review,
}

impl std::fmt::Display for PrTab {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Mine => "mine",
            Self::Review => "review",
        })
    }
}

/// Aggregate pull-request check status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrChecks {
    /// Every reported check passed.
    Pass,
    /// At least one check failed.
    Fail,
    /// At least one check is still pending.
    Pending,
    /// No checks were reported.
    None,
}

/// Aggregate GitHub review decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrReviewDecision {
    /// Required reviews approved the pull request.
    Approved,
    /// A reviewer requested changes.
    ChangesRequested,
    /// A review is required.
    ReviewRequired,
    /// GitHub reported no decision.
    None,
}

/// Display state derived from draft, checks, and reviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    /// Draft pull request.
    Draft,
    /// Continuous integration failed.
    CiFail,
    /// Changes were requested.
    Changes,
    /// Continuous integration is pending.
    CiPending,
    /// Pull request is approved.
    Approved,
    /// Pull request awaits review.
    Review,
}

/// A pull request returned by the GitHub adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    /// Repository containing the pull request.
    pub repo_id: RepoId,
    /// Repository-local pull request number.
    pub number: u64,
    /// Pull request title.
    pub title: String,
    /// Canonical GitHub HTTPS URL.
    pub url: String,
    /// Author login, with null authors normalized to `ghost`.
    pub author: String,
    /// Source branch name.
    pub head_ref_name: String,
    /// Target branch name.
    pub base_ref_name: String,
    /// Whether the pull request is a draft.
    pub is_draft: bool,
    /// Whether the source repository differs from the target repository.
    pub is_cross_repository: bool,
    /// Source repository for cross-repository pull requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_repo: Option<RepoId>,
    /// Aggregate review decision.
    pub review_decision: PrReviewDecision,
    /// Aggregate check status.
    pub checks: PrChecks,
    /// Number of completed successful checks when check details were available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_passed: Option<u32>,
    /// Total number of checks when check details were available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_total: Option<u32>,
    /// Added line count.
    pub additions: u64,
    /// Deleted line count.
    pub deletions: u64,
    /// Label names.
    pub labels: Vec<String>,
    /// ISO-8601 update time.
    pub updated_at: String,
}

impl PullRequest {
    /// Returns whether the URL exactly matches this pull request's GitHub identity.
    #[must_use]
    pub fn has_valid_url(&self) -> bool {
        self.url
            == format!(
                "https://github.com/{}/{}/pull/{}",
                self.repo_id.owner(),
                self.repo_id.name(),
                self.number
            )
    }
}

/// GitHub pull-request facts used while inspecting a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectionPullRequest {
    /// Pull request number.
    pub number: u64,
    /// GitHub lifecycle state.
    pub state: InspectionPrState,
    /// Canonical GitHub URL.
    pub url: String,
    /// Target branch name.
    pub base_ref_name: String,
    /// Pull request head commit.
    pub head_ref_oid: String,
}

/// GitHub lifecycle states used by inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InspectionPrState {
    /// Open pull request.
    #[serde(rename = "OPEN")]
    Open,
    /// Merged pull request.
    #[serde(rename = "MERGED")]
    Merged,
    /// Closed but unmerged pull request.
    #[serde(rename = "CLOSED")]
    Closed,
}

/// A repository returned by GitHub discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepo {
    /// GitHub owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Canonical `owner/name` display name.
    pub full_name: String,
    /// Description, normalized to an empty string when absent.
    pub description: String,
    /// SSH clone URL.
    pub ssh_url: String,
    /// Whether GitHub marks the repository private.
    pub is_private: bool,
    /// ISO-8601 update time.
    pub updated_at: String,
    /// Default branch, normalized to `main` when absent.
    pub default_branch: String,
}

/// Derives a pull request's display state using swarm's strict priority order.
#[must_use]
pub fn derive_pr_state(
    is_draft: bool,
    checks: PrChecks,
    review_decision: PrReviewDecision,
) -> PrState {
    if is_draft {
        PrState::Draft
    } else if checks == PrChecks::Fail {
        PrState::CiFail
    } else if review_decision == PrReviewDecision::ChangesRequested {
        PrState::Changes
    } else if checks == PrChecks::Pending {
        PrState::CiPending
    } else if review_decision == PrReviewDecision::Approved {
        PrState::Approved
    } else {
        PrState::Review
    }
}

/// Returns the pull-request number a worktree checks out, or `None` for any other worktree.
///
/// This is the one definition of a "review worktree" the app relies on: a worktree whose
/// base ref is exactly `pull/<n>/head`, which the daemon writes in `create_pr_local` for every
/// worktree created from a pull request. Any other base ref, including a malformed number
/// such as `pull/+412/head` or `pull/0412/head`, returns `None`.
#[must_use]
pub fn pull_request_checkout(worktree: &Worktree) -> Option<u64> {
    parse_pull_head(&worktree.base_ref)
}

/// Returns the number in a canonical `pull/<n>/head` ref, rejecting signs and leading zeros.
fn parse_pull_head(base_ref: &str) -> Option<u64> {
    let digits = base_ref.strip_prefix("pull/")?.strip_suffix("/head")?;
    let canonical = digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if canonical { digits.parse().ok() } else { None }
}

/// Returns whether a worktree represents a pull request by base ref or same-repo branch.
#[must_use]
pub fn worktree_matches_pr(worktree: &Worktree, pull_request: &PullRequest) -> bool {
    worktree.repo_id == pull_request.repo_id
        && (pull_request_checkout(worktree) == Some(pull_request.number)
            || (!pull_request.is_cross_repository && worktree.branch == pull_request.head_ref_name))
}

/// Returns the local branch Fleet uses when creating a worktree from a pull request.
#[must_use]
pub fn local_branch_for_pr(pull_request: &PullRequest) -> String {
    if pull_request.is_cross_repository {
        format!("pr/{}", pull_request.number)
    } else {
        pull_request.head_ref_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WorktreeId;

    fn worktree(id: &str, branch: &str, base_ref: &str) -> Worktree {
        let id = WorktreeId::try_from(id).unwrap_or_else(|error| panic!("{error}"));
        Worktree {
            repo_id: id.repo().parse().unwrap_or_else(|error| panic!("{error}")),
            slug: id.slug().into(),
            id,
            branch: branch.into(),
            base_ref: base_ref.into(),
            path: "/tmp/worktree".into(),
            session: "payroll/worktree".into(),
            host: None,
            created_at: "2026-09-28T12:00:00Z".into(),
            last_opened_at: None,
            degraded: None,
        }
    }

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
            head_repo: cross.then(|| {
                RepoId::try_from("dannyfuf/payroll").unwrap_or_else(|error| panic!("{error}"))
            }),
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
    fn pull_head_base_ref_is_a_pull_request_checkout() {
        let review = worktree("buk/payroll#pr-412", "feat/rut", "pull/412/head");
        assert_eq!(pull_request_checkout(&review), Some(412));
        assert_eq!(parse_pull_head("pull/0/head"), Some(0));
    }

    #[test]
    fn other_base_refs_are_not_pull_request_checkouts() {
        for base_ref in [
            "origin/main",
            "pull/x/head",
            "pull/412/merge",
            "",
            "pull//head",
            "pull/+412/head",
            "pull/0412/head",
            "pull/-412/head",
            "pull/412/head/extra",
            "refs/pull/412/head",
            "pull/99999999999999999999/head",
        ] {
            assert_eq!(parse_pull_head(base_ref), None, "{base_ref:?}");
        }
        let own = worktree("buk/payroll#feat-rut", "feat/rut", "origin/main");
        assert_eq!(pull_request_checkout(&own), None);
    }

    #[test]
    fn worktree_matches_pr_by_pull_head_base_ref() {
        let review = worktree("buk/payroll#pr-412", "unrelated", "pull/412/head");
        let mut cross_repo = pull_request(true);
        assert!(worktree_matches_pr(&review, &cross_repo));
        cross_repo.number = 413;
        assert!(!worktree_matches_pr(&review, &cross_repo));
    }

    #[test]
    fn worktree_matches_pr_by_same_repo_branch_only() {
        let own = worktree("buk/payroll#feat-rut", "feat/rut", "origin/main");
        assert!(worktree_matches_pr(&own, &pull_request(false)));
        assert!(!worktree_matches_pr(&own, &pull_request(true)));
        let elsewhere = worktree("buk/billing#feat-rut", "feat/rut", "pull/412/head");
        assert!(!worktree_matches_pr(&elsewhere, &pull_request(false)));
    }

    #[test]
    fn pr_state_obeys_documented_priority() {
        assert_eq!(
            derive_pr_state(true, PrChecks::Fail, PrReviewDecision::ChangesRequested),
            PrState::Draft
        );
        assert_eq!(
            derive_pr_state(false, PrChecks::Fail, PrReviewDecision::ChangesRequested),
            PrState::CiFail
        );
        assert_eq!(
            derive_pr_state(false, PrChecks::Pending, PrReviewDecision::ChangesRequested),
            PrState::Changes
        );
        assert_eq!(
            derive_pr_state(false, PrChecks::Pending, PrReviewDecision::Approved),
            PrState::CiPending
        );
        assert_eq!(
            derive_pr_state(false, PrChecks::Pass, PrReviewDecision::Approved),
            PrState::Approved
        );
        assert_eq!(
            derive_pr_state(false, PrChecks::None, PrReviewDecision::None),
            PrState::Review
        );
    }
}
