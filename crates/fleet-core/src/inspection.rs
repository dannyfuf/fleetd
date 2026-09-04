//! Worktree and process inspection result types.

use serde::{Deserialize, Serialize};

use crate::{
    github::{InspectionPrState, InspectionPullRequest},
    ids::{RepoId, WorktreeId},
    sessions::SessionState,
};

/// Warning emitted when a requested fetch fails.
pub const WARNING_FETCH_FAILED: &str = "fetch failed";
/// Warning emitted when GitHub inspection is unavailable.
pub const WARNING_GH_UNAVAILABLE: &str = "gh unavailable";
/// Warning emitted when the branch has no upstream.
pub const WARNING_NO_UPSTREAM: &str = "no upstream";
/// Warning emitted when the configured upstream is gone.
pub const WARNING_UPSTREAM_GONE: &str = "upstream gone";
/// Warning emitted when divergence cannot be calculated.
pub const WARNING_AHEAD_BEHIND_UNAVAILABLE: &str = "ahead/behind unavailable";
/// Warning emitted when publication status cannot be calculated.
pub const WARNING_PUBLISHED_STATUS_UNAVAILABLE: &str = "published status unavailable";
/// Warning emitted when the comparison target cannot be resolved.
pub const WARNING_TARGET_REF_UNAVAILABLE: &str = "target ref unavailable";
/// Warning emitted when the comparison target does not exist.
pub const WARNING_TARGET_REF_MISSING: &str = "target ref missing";
/// Warning emitted when unique commits cannot be counted.
pub const WARNING_UNIQUE_COMMIT_COUNT_UNAVAILABLE: &str = "unique commit count unavailable";
/// Warning emitted when target ancestry cannot be checked.
pub const WARNING_TARGET_COMPARISON_FAILED: &str = "target comparison failed";
/// Warning emitted when pull-request head ancestry cannot be checked.
pub const WARNING_PR_HEAD_COMPARISON_FAILED: &str = "pull request head comparison failed";

/// Complete pure facts produced by worktree inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInspection {
    /// Inspected worktree identifier.
    pub worktree_id: WorktreeId,
    /// Parent repository identifier.
    pub repo_id: RepoId,
    /// Host label, including `local`.
    pub host: String,
    /// Authoritative path on the host.
    pub path: String,
    /// Checked-out branch.
    pub branch: String,
    /// Original base ref.
    pub base_ref: String,
    /// Local HEAD commit, when available.
    pub head: Option<String>,
    /// Branch used for merge comparison.
    pub target_branch: String,
    /// Configured upstream, when present.
    pub upstream: Option<String>,
    /// Commits ahead of the upstream.
    pub ahead: Option<u64>,
    /// Commits behind the upstream.
    pub behind: Option<u64>,
    /// Whether the configured upstream is gone.
    pub upstream_gone: bool,
    /// Whether tracked or untracked changes exist.
    pub dirty: bool,
    /// Whether HEAD is already an ancestor of the target branch.
    pub merged_into_target: bool,
    /// Commits unique relative to the target.
    pub unique_commits: Option<u64>,
    /// Whether an origin branch exists or a matching upstream is gone.
    pub published: bool,
    /// Whether the conservative merged predicate holds.
    pub merged: bool,
    /// Most recent pull request for the branch.
    pub pr: Option<InspectionPullRequest>,
    /// Current session attachment state.
    pub session: SessionState,
    /// Keep-alive labels reported by running terminals.
    pub running: Vec<String>,
    /// ISO-8601 inspection time.
    pub inspected_at: String,
    /// Non-fatal inspection warnings.
    pub warnings: Vec<String>,
    /// Fundamental inspection error.
    pub error: Option<String>,
}

/// Derives publication from a remote branch or a matching gone-upstream fact.
#[must_use]
pub const fn derive_published(remote_branch_exists: bool, matching_upstream_gone: bool) -> bool {
    remote_branch_exists || matching_upstream_gone
}

/// Derives the conservative merged result from PR and target ancestry facts.
#[must_use]
pub fn derive_merged(
    local_head: Option<&str>,
    pull_request: Option<&InspectionPullRequest>,
    pr_head_contains_local_head: bool,
    merged_into_target: bool,
    published: bool,
) -> bool {
    let merged_pr_contains_head = match (local_head, pull_request) {
        (Some(head), Some(pr)) if pr.state == InspectionPrState::Merged => {
            pr.head_ref_oid == head || pr_head_contains_local_head
        }
        _ => false,
    };
    merged_pr_contains_head || (merged_into_target && published)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_accepts_remote_or_gone_upstream() {
        assert!(derive_published(true, false));
        assert!(derive_published(false, true));
        assert!(!derive_published(false, false));
    }

    #[test]
    fn merged_fails_closed_for_post_merge_commits() {
        let pr = InspectionPullRequest {
            number: 1,
            state: InspectionPrState::Merged,
            url: "https://github.com/a/b/pull/1".to_owned(),
            base_ref_name: "main".to_owned(),
            head_ref_oid: "merged-head".to_owned(),
        };
        assert!(derive_merged(Some("local"), Some(&pr), true, false, false));
        assert!(!derive_merged(Some("newer"), Some(&pr), false, false, true));
        assert!(derive_merged(Some("newer"), Some(&pr), false, true, true));
    }
}
