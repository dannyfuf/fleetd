//! Typed GitHub CLI operations matching swarm inventory section 7.

use std::sync::Arc;

use async_trait::async_trait;
use fleet_core::{
    github::{
        InspectionPrState, InspectionPullRequest, PrChecks, PrReviewDecision, PrTab, PullRequest,
        RemoteRepo,
    },
    ids::RepoId,
};
use serde::Deserialize;

use crate::{
    DaemonError, DaemonResult,
    adapters::shell::{Shell, ShellCommand},
};

const PR_FIELDS: &str = "number,title,url,author,headRefName,baseRefName,isDraft,isCrossRepository,headRepository,headRepositoryOwner,reviewDecision,statusCheckRollup,additions,deletions,labels,updatedAt";

/// Typed GitHub metadata operations used by discovery and pull-request services.
#[async_trait]
pub trait Github: Send + Sync {
    /// Returns the authenticated viewer login.
    async fn viewer_login(&self) -> DaemonResult<String>;
    /// Lists up to 1,000 repositories for an owner.
    async fn list_repositories(&self, owner: &str) -> DaemonResult<Vec<RemoteRepo>>;
    /// Returns the open pull request for a same-repository branch, when present.
    async fn open_pull_request(
        &self,
        repo: &RepoId,
        branch: &str,
    ) -> DaemonResult<Option<PullRequest>>;
    /// Returns the newest open, merged, or closed inspection PR for a branch.
    async fn latest_inspection_pull_request(
        &self,
        repo: &RepoId,
        branch: &str,
    ) -> DaemonResult<Option<InspectionPullRequest>>;
    /// Lists the viewer's authored or review-requested open pull requests.
    async fn list_pull_requests(&self, repo: &RepoId, tab: PrTab)
    -> DaemonResult<Vec<PullRequest>>;
    /// Verifies GitHub CLI authentication using `gh auth status`.
    async fn auth_status(&self) -> DaemonResult<()>;
}

/// GitHub CLI adapter implemented through an injected [`Shell`].
#[derive(Clone)]
pub struct GhCli {
    shell: Arc<dyn Shell>,
    cwd: Option<std::path::PathBuf>,
}

impl GhCli {
    /// Creates a GitHub CLI adapter.
    #[must_use]
    pub fn new(shell: Arc<dyn Shell>) -> Self {
        Self { shell, cwd: None }
    }

    /// Sets an optional working directory for every `gh` invocation.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<std::path::PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    async fn run(&self, args: impl IntoIterator<Item = impl Into<String>>) -> DaemonResult<String> {
        let mut command = ShellCommand::new("gh").args(args);
        if let Some(cwd) = &self.cwd {
            command = command.cwd(cwd);
        }
        let result = self.shell.run(command).await?;
        if !result.success() {
            return Err(DaemonError::Github(format!(
                "gh exited {}: {}",
                result.status,
                result.stderr.trim()
            )));
        }
        Ok(result.stdout)
    }
}

#[async_trait]
impl Github for GhCli {
    async fn viewer_login(&self) -> DaemonResult<String> {
        self.run(["api", "user", "--jq", ".login"])
            .await
            .map(|output| output.trim().to_owned())
    }

    async fn list_repositories(&self, owner: &str) -> DaemonResult<Vec<RemoteRepo>> {
        let output = self
            .run([
                "repo",
                "list",
                owner,
                "--limit",
                "1000",
                "--json",
                "name,owner,nameWithOwner,description,sshUrl,isPrivate,updatedAt,defaultBranchRef",
            ])
            .await?;
        let rows: Vec<GhRepo> = serde_json::from_str(&output)
            .map_err(|error| DaemonError::Github(format!("invalid repository JSON: {error}")))?;
        Ok(rows.into_iter().map(RemoteRepo::from).collect())
    }

    async fn open_pull_request(
        &self,
        repo: &RepoId,
        branch: &str,
    ) -> DaemonResult<Option<PullRequest>> {
        let output = self
            .run([
                "pr",
                "list",
                "--repo",
                repo.as_str(),
                "--state",
                "open",
                "--head",
                branch,
                "--limit",
                "1",
                "--json",
                PR_FIELDS,
            ])
            .await?;
        parse_pull_requests(repo, &output).map(|mut pulls| pulls.pop())
    }

    async fn latest_inspection_pull_request(
        &self,
        repo: &RepoId,
        branch: &str,
    ) -> DaemonResult<Option<InspectionPullRequest>> {
        let output = self
            .run([
                "pr",
                "list",
                "--repo",
                repo.as_str(),
                "--head",
                branch,
                "--state",
                "all",
                "--limit",
                "100",
                "--json",
                "number,state,url,baseRefName,headRefOid,updatedAt",
            ])
            .await?;
        let mut rows: Vec<GhInspectionPull> = serde_json::from_str(&output)
            .map_err(|error| DaemonError::Github(format!("invalid inspection PR JSON: {error}")))?;
        rows.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
        rows.pop().map(TryInto::try_into).transpose()
    }

    async fn list_pull_requests(
        &self,
        repo: &RepoId,
        tab: PrTab,
    ) -> DaemonResult<Vec<PullRequest>> {
        let mut args = vec![
            "pr".to_owned(),
            "list".to_owned(),
            "--repo".to_owned(),
            repo.to_string(),
            "--state".to_owned(),
            "open".to_owned(),
            "--limit".to_owned(),
            "100".to_owned(),
            "--json".to_owned(),
            PR_FIELDS.to_owned(),
        ];
        match tab {
            PrTab::Mine => args.extend(["--author".to_owned(), "@me".to_owned()]),
            PrTab::Review => args.extend([
                "--search".to_owned(),
                "user-review-requested:@me".to_owned(),
            ]),
        }
        let output = self.run(args).await?;
        parse_pull_requests(repo, &output)
    }

    async fn auth_status(&self) -> DaemonResult<()> {
        self.run(["auth", "status"]).await.map(drop)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepo {
    name: String,
    owner: GhLogin,
    name_with_owner: String,
    description: Option<String>,
    ssh_url: String,
    is_private: bool,
    updated_at: String,
    default_branch_ref: Option<GhBranch>,
}

impl From<GhRepo> for RemoteRepo {
    fn from(repo: GhRepo) -> Self {
        Self {
            owner: repo.owner.login,
            name: repo.name,
            full_name: repo.name_with_owner,
            description: repo.description.unwrap_or_default(),
            ssh_url: repo.ssh_url,
            is_private: repo.is_private,
            updated_at: repo.updated_at,
            default_branch: repo
                .default_branch_ref
                .map_or_else(|| "main".to_owned(), |branch| branch.name),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhLogin {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhBranch {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPull {
    number: u64,
    title: String,
    url: String,
    author: Option<GhLogin>,
    head_ref_name: String,
    base_ref_name: String,
    is_draft: bool,
    is_cross_repository: bool,
    head_repository: Option<GhHeadRepo>,
    head_repository_owner: Option<GhLogin>,
    review_decision: Option<String>,
    #[serde(default)]
    status_check_rollup: Vec<GhCheck>,
    additions: u64,
    deletions: u64,
    #[serde(default)]
    labels: Vec<GhLabel>,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhHeadRepo {
    name: String,
    name_with_owner: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhCheck {
    conclusion: Option<String>,
    status: Option<String>,
}

fn parse_pull_requests(repo: &RepoId, output: &str) -> DaemonResult<Vec<PullRequest>> {
    let rows: Vec<GhPull> = serde_json::from_str(output)
        .map_err(|error| DaemonError::Github(format!("invalid pull-request JSON: {error}")))?;
    rows.into_iter()
        .map(|pull| convert_pull(repo.clone(), pull))
        .collect()
}

fn convert_pull(repo_id: RepoId, pull: GhPull) -> DaemonResult<PullRequest> {
    let head_repo = if pull.is_cross_repository {
        let full_name = pull
            .head_repository
            .as_ref()
            .and_then(|repo| repo.name_with_owner.clone())
            .or_else(|| {
                pull.head_repository_owner.as_ref().and_then(|owner| {
                    pull.head_repository
                        .as_ref()
                        .map(|repo| format!("{}/{}", owner.login, repo.name))
                })
            });
        full_name
            .map(RepoId::try_from)
            .transpose()
            .map_err(|error| DaemonError::Github(error.to_string()))?
    } else {
        None
    };
    let (checks_passed, checks_total) = check_counts(&pull.status_check_rollup);
    Ok(PullRequest {
        repo_id,
        number: pull.number,
        title: pull.title,
        url: pull.url,
        author: pull
            .author
            .map_or_else(|| "ghost".to_owned(), |user| user.login),
        head_ref_name: pull.head_ref_name,
        base_ref_name: pull.base_ref_name,
        is_draft: pull.is_draft,
        is_cross_repository: pull.is_cross_repository,
        head_repo,
        review_decision: parse_review(pull.review_decision.as_deref()),
        checks: parse_checks(&pull.status_check_rollup),
        checks_passed,
        checks_total,
        additions: pull.additions,
        deletions: pull.deletions,
        labels: pull.labels.into_iter().map(|label| label.name).collect(),
        updated_at: pull.updated_at,
    })
}

fn check_counts(checks: &[GhCheck]) -> (Option<u32>, Option<u32>) {
    if checks.is_empty() {
        return (None, None);
    }
    let total = u32::try_from(checks.len()).unwrap_or(u32::MAX);
    let passed = checks
        .iter()
        .filter(|check| check.conclusion.as_deref() == Some("SUCCESS"))
        .count();
    (Some(u32::try_from(passed).unwrap_or(u32::MAX)), Some(total))
}

fn parse_review(value: Option<&str>) -> PrReviewDecision {
    match value {
        Some("APPROVED") => PrReviewDecision::Approved,
        Some("CHANGES_REQUESTED") => PrReviewDecision::ChangesRequested,
        Some("REVIEW_REQUIRED") => PrReviewDecision::ReviewRequired,
        _ => PrReviewDecision::None,
    }
}

fn parse_checks(checks: &[GhCheck]) -> PrChecks {
    if checks.is_empty() {
        return PrChecks::None;
    }
    if checks.iter().any(|check| {
        matches!(
            check.conclusion.as_deref(),
            Some("FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE")
        )
    }) {
        PrChecks::Fail
    } else if checks.iter().any(|check| {
        check.conclusion.as_deref().is_none_or(str::is_empty)
            || !matches!(check.status.as_deref(), Some("COMPLETED") | None)
    }) {
        PrChecks::Pending
    } else {
        PrChecks::Pass
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhInspectionPull {
    number: u64,
    state: String,
    url: String,
    base_ref_name: String,
    head_ref_oid: String,
    updated_at: String,
}

impl TryFrom<GhInspectionPull> for InspectionPullRequest {
    type Error = DaemonError;

    fn try_from(pull: GhInspectionPull) -> Result<Self, Self::Error> {
        let state = match pull.state.as_str() {
            "OPEN" => InspectionPrState::Open,
            "MERGED" => InspectionPrState::Merged,
            "CLOSED" => InspectionPrState::Closed,
            other => {
                return Err(DaemonError::Github(format!(
                    "unknown pull-request state `{other}`"
                )));
            }
        };
        Ok(Self {
            number: pull.number,
            state,
            url: pull.url,
            base_ref_name: pull.base_ref_name,
            head_ref_oid: pull.head_ref_oid,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        adapters::shell::ShellResult,
        testing::fakes::{FakeShell, FakeShellCall},
    };

    use super::*;

    #[tokio::test]
    async fn repository_discovery_uses_exact_gh_shape_and_normalizes_defaults() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.args.starts_with(&["repo".to_owned(), "list".to_owned()]),
            ShellResult {
                status: 0,
                stdout: r#"[{"name":"api","owner":{"login":"acme"},"nameWithOwner":"acme/api","description":null,"sshUrl":"git@github.com:acme/api.git","isPrivate":true,"updatedAt":"2026-01-01T00:00:00Z","defaultBranchRef":null}]"#.to_owned(),
                stderr: String::new(),
            },
        );
        let github = GhCli::new(shell.clone());
        let repositories = github
            .list_repositories("acme")
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(repositories[0].description, "");
        assert_eq!(repositories[0].default_branch, "main");
        let FakeShellCall::Run(command) = &shell.calls()[0] else {
            panic!("expected normal shell call");
        };
        assert_eq!(
            command.args,
            vec![
                "repo",
                "list",
                "acme",
                "--limit",
                "1000",
                "--json",
                "name,owner,nameWithOwner,description,sshUrl,isPrivate,updatedAt,defaultBranchRef"
            ]
        );
    }
}
