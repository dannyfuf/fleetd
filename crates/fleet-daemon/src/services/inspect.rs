//! Worktree activity, process, and port inspection orchestration.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use fleet_core::{
    github::{InspectionPrState, InspectionPullRequest},
    ids::{RepoId, WorktreeId},
    inspection::{
        WARNING_AHEAD_BEHIND_UNAVAILABLE, WARNING_FETCH_FAILED, WARNING_GH_UNAVAILABLE,
        WARNING_NO_UPSTREAM, WARNING_PR_HEAD_COMPARISON_FAILED,
        WARNING_PUBLISHED_STATUS_UNAVAILABLE, WARNING_TARGET_COMPARISON_FAILED,
        WARNING_TARGET_REF_MISSING, WARNING_TARGET_REF_UNAVAILABLE,
        WARNING_UNIQUE_COMMIT_COUNT_UNAVAILABLE, WARNING_UPSTREAM_GONE, WorktreeInspection,
        derive_merged, derive_published,
    },
    model::{Repo, Worktree},
    sessions::{AgentActivity, SessionState, WorktreeStatus},
};
use fleet_proto::job::JobKind;
use futures_util::{StreamExt, stream::FuturesUnordered};

use crate::{
    DaemonError, DaemonResult,
    adapters::{git::Git, github::Github},
    jobs::{JobCtx, JobManager},
    services::sessions::Sessions,
    stores::{config::ConfigStore, state::StateStore},
};

const REMOTE_UNSUPPORTED: &str = "remote hosts are not supported yet";

/// Worktree inspection service coordinating Git, GitHub, process, and remote facts.
#[derive(Clone)]
pub struct Inspect {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    git: Arc<dyn Git>,
    github: Arc<dyn Github>,
    sessions: Option<Sessions>,
}

impl Inspect {
    /// Creates the inspection service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        github: Arc<dyn Github>,
    ) -> Self {
        Self {
            config,
            state,
            jobs,
            git,
            github,
            sessions: None,
        }
    }

    /// Adds live daemon session observations to inspection results.
    #[must_use]
    pub fn with_sessions(mut self, sessions: Sessions) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// Inspects selected worktrees, preserving fail-closed warning semantics.
    pub async fn worktrees(
        &self,
        ids: Vec<WorktreeId>,
        repo: Option<RepoId>,
        fetch: bool,
    ) -> DaemonResult<Vec<WorktreeInspection>> {
        let service = self.clone();
        let target = format!("inspect-{}", uuid::Uuid::new_v4());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));
        self.jobs.submit(
            JobKind::Inspect,
            target,
            "Inspect worktrees",
            true,
            true,
            move |context| async move {
                let result = service.inspect_inner(ids, repo, fetch, &context).await;
                let outcome = result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| DaemonError::Git(error.to_string()));
                if let Some(sender) = sender
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    let _ignored = sender.send(result);
                }
                outcome
            },
        );
        receiver.await.map_err(|_| DaemonError::Cancelled)?
    }

    pub(crate) async fn inspect_inner(
        &self,
        ids: Vec<WorktreeId>,
        repo_filter: Option<RepoId>,
        fetch: bool,
        context: &JobCtx,
    ) -> DaemonResult<Vec<WorktreeInspection>> {
        let state = self.state.load().await?;
        // Loading config here validates it before any external work and preserves the
        // documented inspection dependency on both stores.
        let _config = self.config.load().await?;
        let by_id = state
            .worktrees
            .iter()
            .map(|worktree| (worktree.id.clone(), worktree.clone()))
            .collect::<HashMap<_, _>>();
        let selected = if ids.is_empty() {
            state.worktrees.clone()
        } else {
            ids.into_iter()
                .map(|id| {
                    by_id
                        .get(&id)
                        .cloned()
                        .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))
                })
                .collect::<DaemonResult<Vec<_>>>()?
        }
        .into_iter()
        .filter(|worktree| {
            repo_filter
                .as_ref()
                .is_none_or(|repo| &worktree.repo_id == repo)
        })
        .collect::<Vec<_>>();

        let repos = state
            .repos
            .into_iter()
            .map(|repo| (repo.id.clone(), repo))
            .collect::<HashMap<_, _>>();
        let fetch_failures = if fetch {
            context.progress("fetching repository remotes")?;
            self.fetch_repositories(&selected, &repos, context).await?
        } else {
            HashMap::new()
        };
        if context.cancel.is_cancelled() {
            return Err(DaemonError::Cancelled);
        }

        let statuses = match &self.sessions {
            Some(sessions) => sessions.refresh_statuses(repo_filter.clone()).await?,
            None => status_snapshot(&selected),
        };
        let status_by_id = statuses
            .into_iter()
            .map(|status| (status.worktree_id.clone(), status))
            .collect::<HashMap<_, _>>();
        context.progress(format!("inspecting {} worktrees", selected.len()))?;

        let mut pending = FuturesUnordered::new();
        for (index, worktree) in selected.into_iter().enumerate() {
            let repo = repos.get(&worktree.repo_id).cloned();
            let status = status_by_id
                .get(&worktree.id)
                .cloned()
                .unwrap_or_else(|| unknown_status(&worktree));
            let fetch_failed = fetch_failures.contains_key(&worktree.repo_id);
            let service = self.clone();
            let cancel = context.cancel.clone();
            pending.push(async move {
                if cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }
                Ok((
                    index,
                    service
                        .inspect_one(worktree, repo, status, fetch_failed)
                        .await,
                ))
            });
        }

        let mut inspections = Vec::new();
        while let Some(inspection) = pending.next().await {
            inspections.push(inspection?);
        }
        inspections.sort_by_key(|(index, _inspection)| *index);
        let inspections = inspections
            .into_iter()
            .map(|(_index, inspection)| inspection)
            .collect::<Vec<_>>();
        context.progress(format!("inspected {} worktrees", inspections.len()))?;
        Ok(inspections)
    }

    async fn fetch_repositories(
        &self,
        worktrees: &[Worktree],
        repos: &HashMap<RepoId, Repo>,
        context: &JobCtx,
    ) -> DaemonResult<HashMap<RepoId, String>> {
        let mut pending = FuturesUnordered::new();
        let selected_repos = worktrees
            .iter()
            .filter(|worktree| worktree.host.is_none())
            .map(|worktree| worktree.repo_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for repo_id in &selected_repos {
            let Some(repo) = repos.get(repo_id) else {
                continue;
            };
            let git = Arc::clone(&self.git);
            let cancel = context.cancel.clone();
            let id = repo.id.clone();
            let path = repo.path.clone();
            pending.push(async move {
                if cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }
                let result = git.fetch(Path::new(&path), true).await;
                Ok((id, result.err().map(|error| error.to_string())))
            });
        }
        let mut failures = HashMap::new();
        while let Some(result) = pending.next().await {
            let (repo, failure) = result?;
            if let Some(failure) = failure {
                failures.insert(repo, failure);
            }
        }
        Ok(failures)
    }

    async fn inspect_one(
        &self,
        worktree: Worktree,
        repo: Option<Repo>,
        status: WorktreeStatus,
        fetch_failed: bool,
    ) -> WorktreeInspection {
        let inspected_at = chrono::Utc::now().to_rfc3339();
        let Some(repo) = repo else {
            return failed_inspection(
                &worktree,
                status,
                inspected_at,
                "repository is missing from state".to_owned(),
            );
        };
        if worktree.host.is_some() {
            return failed_inspection(
                &worktree,
                status,
                inspected_at,
                REMOTE_UNSUPPORTED.to_owned(),
            );
        }

        let path = Path::new(&worktree.path);
        let remote_path = Path::new(&repo.path);
        let mut warnings = Vec::new();
        if fetch_failed {
            warnings.push(WARNING_FETCH_FAILED.to_owned());
        }

        let head = match self.git.revision(path, "HEAD").await {
            Ok(head) if !head.is_empty() => head,
            Ok(_) => {
                return failed_inspection(
                    &worktree,
                    status,
                    inspected_at,
                    "HEAD resolved to an empty revision".to_owned(),
                );
            }
            Err(error) => {
                return failed_inspection(&worktree, status, inspected_at, error.to_string());
            }
        };
        let porcelain = match self.git.status_porcelain(path).await {
            Ok(porcelain) => porcelain,
            Err(error) => {
                return failed_inspection(&worktree, status, inspected_at, error.to_string());
            }
        };
        let dirty_files = porcelain.lines().filter(|line| !line.is_empty()).count() as u64;

        let pull_request = match self.latest_pull_request(&repo.id, &worktree.branch).await {
            Ok(pull_request) => pull_request,
            Err(()) => {
                warnings.push(WARNING_GH_UNAVAILABLE.to_owned());
                None
            }
        };
        let target_branch = pull_request.as_ref().map_or_else(
            || repo.default_branch.clone(),
            |pull| pull.base_ref_name.clone(),
        );

        let (upstream, upstream_gone) = match self.git.upstream(path, &worktree.branch).await {
            Ok(raw) => parse_upstream(&raw),
            Err(_) => (None, false),
        };
        if upstream.is_none() {
            warnings.push(WARNING_NO_UPSTREAM.to_owned());
        }
        if upstream_gone {
            warnings.push(WARNING_UPSTREAM_GONE.to_owned());
        }
        let (ahead, behind) = if let Some(upstream) = upstream.as_deref() {
            match self.git.divergence(path, upstream).await {
                Ok((behind, ahead)) => (Some(ahead), Some(behind)),
                Err(_) => {
                    warnings.push(WARNING_AHEAD_BEHIND_UNAVAILABLE.to_owned());
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        let remote_branch_exists = match self
            .git
            .remote_branch_exists(remote_path, &worktree.branch)
            .await
        {
            Ok(exists) => exists,
            Err(_) => {
                warnings.push(WARNING_PUBLISHED_STATUS_UNAVAILABLE.to_owned());
                false
            }
        };
        let expected_upstream = format!("origin/{}", worktree.branch);
        let matching_upstream_gone =
            upstream_gone && upstream.as_deref() == Some(expected_upstream.as_str());
        let published = derive_published(remote_branch_exists, matching_upstream_gone);

        let target = format!("origin/{target_branch}");
        let (unique_commits, merged_into_target) =
            match self.git.revision_exists(remote_path, &target).await {
                Ok(true) => {
                    let unique = match self
                        .git
                        .unique_commits_from(remote_path, &target, &head)
                        .await
                    {
                        Ok(count) => Some(count),
                        Err(_) => {
                            warnings.push(WARNING_UNIQUE_COMMIT_COUNT_UNAVAILABLE.to_owned());
                            None
                        }
                    };
                    let merged = match self.git.is_ancestor(remote_path, &head, &target).await {
                        Ok(merged) => merged,
                        Err(_) => {
                            warnings.push(WARNING_TARGET_COMPARISON_FAILED.to_owned());
                            false
                        }
                    };
                    (unique, merged)
                }
                Ok(false) => {
                    warnings.push(WARNING_TARGET_REF_MISSING.to_owned());
                    (None, false)
                }
                Err(_) => {
                    warnings.push(WARNING_TARGET_REF_UNAVAILABLE.to_owned());
                    (None, false)
                }
            };

        let pr_head_contains_local_head = self
            .pr_head_contains_local_head(path, &head, pull_request.as_ref(), &mut warnings)
            .await;
        let merged = derive_merged(
            Some(&head),
            pull_request.as_ref(),
            pr_head_contains_local_head,
            merged_into_target,
            published,
        );

        WorktreeInspection {
            worktree_id: worktree.id,
            repo_id: worktree.repo_id,
            host: "local".to_owned(),
            path: worktree.path,
            branch: worktree.branch,
            base_ref: worktree.base_ref,
            head: Some(head),
            target_branch,
            upstream,
            ahead,
            behind,
            upstream_gone,
            dirty: dirty_files > 0,
            dirty_files: Some(dirty_files),
            merged_into_target,
            unique_commits,
            published,
            merged,
            pr: pull_request,
            session: status.session,
            running: status.running,
            inspected_at,
            warnings,
            error: None,
        }
    }

    async fn latest_pull_request(
        &self,
        repo: &RepoId,
        branch: &str,
    ) -> Result<Option<InspectionPullRequest>, ()> {
        let semaphore = self.jobs.github_semaphore();
        let Ok(_permit) = semaphore.acquire_owned().await else {
            return Err(());
        };
        self.github
            .latest_inspection_pull_request(repo, branch)
            .await
            .map_err(|_| ())
    }

    async fn pr_head_contains_local_head(
        &self,
        path: &Path,
        head: &str,
        pull_request: Option<&InspectionPullRequest>,
        warnings: &mut Vec<String>,
    ) -> bool {
        let Some(pull_request) = pull_request else {
            return false;
        };
        if pull_request.state != InspectionPrState::Merged || pull_request.head_ref_oid == head {
            return false;
        }
        match self
            .git
            .is_ancestor(path, head, &pull_request.head_ref_oid)
            .await
        {
            Ok(contains) => contains,
            Err(_) => {
                warnings.push(WARNING_PR_HEAD_COMPARISON_FAILED.to_owned());
                false
            }
        }
    }

    /// Refreshes runtime worktree statuses for one repository or all repositories.
    ///
    pub async fn refresh_statuses(
        &self,
        repo: Option<RepoId>,
    ) -> DaemonResult<Vec<WorktreeStatus>> {
        if let Some(sessions) = &self.sessions {
            return sessions.refresh_statuses(repo).await;
        }
        let state = self.state.load().await?;
        let worktrees = state
            .worktrees
            .into_iter()
            .filter(|worktree| repo.as_ref().is_none_or(|repo| &worktree.repo_id == repo))
            .collect::<Vec<_>>();
        Ok(status_snapshot(&worktrees))
    }

    /// Produces fail-closed status placeholders until the status poller has observed each worktree.
    #[must_use]
    pub fn unknown_statuses(worktrees: &[Worktree]) -> Vec<WorktreeStatus> {
        worktrees.iter().map(unknown_status).collect()
    }
}

fn parse_upstream(raw: &str) -> (Option<String>, bool) {
    let (name, tracking) = raw.split_once('\0').unwrap_or((raw, ""));
    let name = name.trim();
    (
        (!name.is_empty()).then(|| name.to_owned()),
        tracking.contains("gone"),
    )
}

fn status_snapshot(worktrees: &[Worktree]) -> Vec<WorktreeStatus> {
    worktrees
        .iter()
        .map(|worktree| WorktreeStatus {
            worktree_id: worktree.id.clone(),
            session: if worktree.host.is_some() {
                SessionState::Unknown
            } else {
                SessionState::None
            },
            windows: Vec::new(),
            running: Vec::new(),
            agent_activity: AgentActivity::Unknown,
            agent_activity_changed_at: None,
        })
        .collect()
}

fn unknown_status(worktree: &Worktree) -> WorktreeStatus {
    WorktreeStatus {
        worktree_id: worktree.id.clone(),
        session: SessionState::Unknown,
        windows: Vec::new(),
        running: Vec::new(),
        agent_activity: AgentActivity::Unknown,
        agent_activity_changed_at: None,
    }
}

fn failed_inspection(
    worktree: &Worktree,
    status: WorktreeStatus,
    inspected_at: String,
    error: String,
) -> WorktreeInspection {
    WorktreeInspection {
        worktree_id: worktree.id.clone(),
        repo_id: worktree.repo_id.clone(),
        host: worktree
            .host
            .as_ref()
            .map_or_else(|| "local".to_owned(), ToString::to_string),
        path: worktree.path.clone(),
        branch: worktree.branch.clone(),
        base_ref: worktree.base_ref.clone(),
        head: None,
        target_branch: String::new(),
        upstream: None,
        ahead: None,
        behind: None,
        upstream_gone: false,
        dirty: false,
        dirty_files: None,
        merged_into_target: false,
        unique_commits: None,
        published: false,
        merged: false,
        pr: None,
        session: status.session,
        running: status.running,
        inspected_at,
        warnings: Vec::new(),
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        ids::{RepoId, WorktreeId},
        model::Worktree,
        sessions::SessionState,
    };

    use super::{Inspect, parse_upstream};

    #[test]
    fn unobserved_worktrees_fail_closed() {
        let worktree = Worktree {
            id: WorktreeId::try_from("owner/repo#feature")
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: RepoId::try_from("owner/repo").unwrap_or_else(|error| panic!("{error}")),
            slug: "feature".to_owned(),
            branch: "feature".to_owned(),
            base_ref: "main".to_owned(),
            path: "/tmp/feature".to_owned(),
            session: "fleet-owner-repo-feature".to_owned(),
            host: None,
            created_at: "2026-09-04T00:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        };

        let statuses = Inspect::unknown_statuses(&[worktree]);

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].session, SessionState::Unknown);
        assert!(statuses[0].windows.is_empty());
        assert!(statuses[0].running.is_empty());
    }

    #[test]
    fn parses_upstream_and_gone_marker() {
        assert_eq!(
            parse_upstream("origin/feature\0[ahead 2, gone]"),
            (Some("origin/feature".to_owned()), true)
        );
        assert_eq!(parse_upstream("\0"), (None, false));
    }
}
