//! Worktree activity, process, and port inspection orchestration.

use std::{collections::HashMap, path::Path, sync::Arc};

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
use futures_util::{StreamExt, stream};

use crate::{
    DaemonError, DaemonResult,
    adapters::{git::Git, github::Github},
    error::REMOTE_UNSUPPORTED,
    jobs::{JobCtx, JobManager, JobPolicy},
    services::{
        awaited::{JobDelivery, copy_error},
        sessions::Sessions,
    },
    stores::state::StateStore,
};

const INSPECTION_CONCURRENCY: usize = 8;
const FETCH_CONCURRENCY: usize = 4;

/// Worktree inspection service coordinating Git, GitHub, process, and remote facts.
#[derive(Clone)]
pub struct Inspect {
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    git: Arc<dyn Git>,
    github: Arc<dyn Github>,
    sessions: Sessions,
}

impl Inspect {
    /// Creates the inspection service.
    #[must_use]
    pub fn new(
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        github: Arc<dyn Github>,
        sessions: Sessions,
    ) -> Self {
        Self {
            state,
            jobs,
            git,
            github,
            sessions,
        }
    }

    /// Inspects selected worktrees, preserving fail-closed warning semantics.
    pub async fn worktrees(
        &self,
        ids: Vec<WorktreeId>,
        repo: Option<RepoId>,
        fetch: bool,
    ) -> DaemonResult<Vec<WorktreeInspection>> {
        let repos = if let Some(repo) = &repo {
            vec![repo.clone()]
        } else {
            ids.iter()
                .map(|id| {
                    RepoId::try_from(id.repo())
                        .map_err(|error| DaemonError::Validation(error.to_string()))
                })
                .collect::<DaemonResult<Vec<_>>>()?
        };
        let service = self.clone();
        let (delivery, awaited) = JobDelivery::job_gets_copy(copy_error);
        let target = format!("inspect-{}", uuid::Uuid::new_v4());
        if repos.is_empty() {
            self.jobs.submit_for_all_repos(
                JobKind::Inspect,
                target,
                "Inspect worktrees",
                JobPolicy::new(true, true),
                move |context| async move {
                    delivery.finish(service.inspect_inner(ids, repo, fetch, &context).await)
                },
            )?;
        } else {
            self.jobs.submit_for_repos(
                repos,
                JobKind::Inspect,
                target,
                "Inspect worktrees",
                JobPolicy::new(true, true),
                move |context| async move {
                    delivery.finish(service.inspect_inner(ids, repo, fetch, &context).await)
                },
            )?;
        }
        awaited.wait().await
    }

    pub(crate) async fn inspect_inner(
        &self,
        ids: Vec<WorktreeId>,
        repo_filter: Option<RepoId>,
        fetch: bool,
        context: &JobCtx,
    ) -> DaemonResult<Vec<WorktreeInspection>> {
        let state = self.state.load().await?;
        let mut prefabricated = Vec::new();
        let selected = if ids.is_empty() {
            state
                .worktrees
                .into_iter()
                .enumerate()
                .filter(|(_, worktree)| worktree.host.is_none())
                .collect::<Vec<_>>()
        } else {
            let by_id = state
                .worktrees
                .iter()
                .map(|worktree| (&worktree.id, worktree))
                .collect::<HashMap<_, _>>();
            let mut selected = Vec::new();
            for (index, id) in ids.iter().enumerate() {
                if repo_filter
                    .as_ref()
                    .is_some_and(|repo| id.repo() != repo.as_str())
                {
                    continue;
                }
                match by_id.get(id) {
                    Some(worktree) if worktree.host.is_none() => {
                        selected.push((index, (*worktree).clone()));
                    }
                    Some(worktree) => prefabricated.push((
                        index,
                        failed_inspection(
                            worktree,
                            Self::unknown_status(worktree),
                            chrono::Utc::now().to_rfc3339(),
                            REMOTE_UNSUPPORTED.to_owned(),
                        ),
                    )),
                    None => prefabricated.push((index, missing_inspection(id)?)),
                }
            }
            selected
        }
        .into_iter()
        .filter(|(_, worktree)| {
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
            self.fetch_repositories(
                &selected
                    .iter()
                    .map(|(_, worktree)| worktree.clone())
                    .collect::<Vec<_>>(),
                &repos,
                context,
            )
            .await?
        } else {
            HashMap::new()
        };
        if context.cancel.is_cancelled() {
            return Err(DaemonError::Cancelled);
        }

        let statuses = self.sessions.refresh_statuses(repo_filter.clone()).await?;
        let status_by_id = statuses
            .into_iter()
            .map(|status| (status.worktree_id.clone(), status))
            .collect::<HashMap<_, _>>();
        context.progress(format!("inspecting {} worktrees", selected.len()))?;

        let mut pending = stream::iter(selected.into_iter().map(|(index, worktree)| {
            let repo = repos.get(&worktree.repo_id).cloned();
            let status = status_by_id
                .get(&worktree.id)
                .cloned()
                .unwrap_or_else(|| Self::unknown_status(&worktree));
            let fetch_failed = fetch_failures.contains_key(&worktree.repo_id);
            let service = self.clone();
            let cancel = context.cancel.clone();
            async move {
                if cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }
                Ok((
                    index,
                    service
                        .inspect_one(worktree, repo, status, fetch_failed)
                        .await,
                ))
            }
        }))
        .buffer_unordered(INSPECTION_CONCURRENCY);

        let mut inspections = prefabricated;
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
        let selected_repos = worktrees
            .iter()
            .filter(|worktree| worktree.host.is_none())
            .map(|worktree| worktree.repo_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let selected = selected_repos
            .iter()
            .filter_map(|id| repos.get(id))
            .map(|repo| (repo.id.clone(), repo.path.clone()))
            .collect::<Vec<_>>();
        let mut pending = stream::iter(selected.into_iter().map(|(id, path)| {
            let git = Arc::clone(&self.git);
            let cancel = context.cancel.clone();
            async move {
                if cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }
                let result = git.fetch(Path::new(&path), true).await;
                Ok((id, result.err().map(|error| error.to_string())))
            }
        }))
        .buffer_unordered(FETCH_CONCURRENCY);
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
        let branch = match self.git.current_branch(path).await {
            Ok(branch) if !branch.is_empty() => branch,
            Ok(_) => {
                return failed_inspection(
                    &worktree,
                    status,
                    inspected_at,
                    "HEAD is detached".to_owned(),
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

        let pull_request = match self.latest_pull_request(&repo.id, &branch).await {
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

        let (upstream, upstream_gone) = match self.git.upstream(path, &branch).await {
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

        let remote_branch_exists = match self.git.remote_branch_exists(remote_path, &branch).await {
            Ok(exists) => exists,
            Err(_) => {
                warnings.push(WARNING_PUBLISHED_STATUS_UNAVAILABLE.to_owned());
                false
            }
        };
        let expected_upstream = format!("origin/{branch}");
        let matching_upstream_gone =
            upstream_gone && upstream.as_deref() == Some(expected_upstream.as_str());
        let published = derive_published(remote_branch_exists, matching_upstream_gone);

        let target = format!("origin/{target_branch}");
        let (unique_commits, merged_into_target) =
            match self.git.revision_exists(remote_path, &target).await {
                Ok(true) => {
                    match self
                        .shared_target_revision(path, remote_path, &target)
                        .await
                    {
                        Ok(target_head) => {
                            let unique = match self
                                .git
                                .unique_commits_from(path, &target_head, &head)
                                .await
                            {
                                Ok(count) => Some(count),
                                Err(_) => {
                                    warnings
                                        .push(WARNING_UNIQUE_COMMIT_COUNT_UNAVAILABLE.to_owned());
                                    None
                                }
                            };
                            let merged = match self.git.is_ancestor(path, &head, &target_head).await
                            {
                                Ok(merged) => merged,
                                Err(_) => {
                                    warnings.push(WARNING_TARGET_COMPARISON_FAILED.to_owned());
                                    false
                                }
                            };
                            (unique, merged)
                        }
                        Err(_) => {
                            warnings.push(WARNING_UNIQUE_COMMIT_COUNT_UNAVAILABLE.to_owned());
                            warnings.push(WARNING_TARGET_COMPARISON_FAILED.to_owned());
                            (None, false)
                        }
                    }
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
            branch,
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

    async fn shared_target_revision(
        &self,
        worktree: &Path,
        pristine: &Path,
        target: &str,
    ) -> DaemonResult<String> {
        let target_head = self.git.revision(pristine, target).await?;
        if target_head.is_empty() {
            return Err(DaemonError::Git(
                "target resolved to an empty revision".to_owned(),
            ));
        }
        let peeled = format!("{target_head}^{{commit}}");
        if self.git.revision(worktree, &peeled).await.is_err() {
            self.git
                .fetch_refs(
                    worktree,
                    &pristine.to_string_lossy(),
                    std::slice::from_ref(&target_head),
                )
                .await?;
            self.git.revision(worktree, &peeled).await?;
        }
        Ok(target_head)
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

    /// Produces a fail-closed status placeholder until the status poller has observed a worktree.
    #[must_use]
    pub(crate) fn unknown_status(worktree: &Worktree) -> WorktreeStatus {
        WorktreeStatus {
            worktree_id: worktree.id.clone(),
            session: SessionState::Unknown,
            windows: Vec::new(),
            running: Vec::new(),
            agent_activity: AgentActivity::Unknown,
            agent_activity_changed_at: None,
        }
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

fn missing_inspection(worktree_id: &WorktreeId) -> DaemonResult<WorktreeInspection> {
    let repo_id = RepoId::try_from(worktree_id.repo())
        .map_err(|error| DaemonError::Validation(error.to_string()))?;
    Ok(WorktreeInspection {
        repo_id,
        worktree_id: worktree_id.clone(),
        host: "local".to_owned(),
        path: String::new(),
        branch: String::new(),
        base_ref: String::new(),
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
        session: SessionState::Unknown,
        running: Vec::new(),
        inspected_at: chrono::Utc::now().to_rfc3339(),
        warnings: Vec::new(),
        error: Some(format!("worktree {worktree_id} was not found")),
    })
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

        let status = Inspect::unknown_status(&worktree);

        assert_eq!(status.session, SessionState::Unknown);
        assert!(status.windows.is_empty());
        assert!(status.running.is_empty());
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
