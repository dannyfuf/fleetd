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

/// Returns whether a worktree represents a pull request by base ref or same-repo branch.
#[must_use]
pub fn worktree_matches_pr(worktree: &Worktree, pull_request: &PullRequest) -> bool {
    worktree.repo_id == pull_request.repo_id
        && (worktree.base_ref == format!("pull/{}/head", pull_request.number)
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
