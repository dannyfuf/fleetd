//! Repository registration, discovery, caching, and clone orchestration.

use super::{
    awaited::{JobDelivery, git_error, github_error},
    cache::{cache_is_fresh, fleet_home, read_cache, write_cache},
};

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::Utc;
use fleet_core::{
    cache::RepoCache,
    ids::{ContextId, RepoId},
    model::{CloneJob, CloneStatus, Repo, RepoHooks},
    paths::clone_publish_marker_path,
};
use fleet_proto::{
    job::{JobKind, JobRecord},
    response::BaseRefs,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        files::Files,
        git::Git,
        github::Github,
        process::{Process, pid_is_alive},
    },
    jobs::{JobCtx, JobManager, JobPolicy},
    stores::{config::ConfigStore, state::StateStore},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClonePublishIntent {
    repo: Repo,
}

/// Repository domain service with durable state, configuration, and job scheduling handles.
#[derive(Clone)]
pub struct Repos {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    git: Arc<dyn Git>,
    github: Arc<dyn Github>,
    files: Arc<dyn Files>,
    process: Arc<dyn Process>,
    context_lifecycle: Arc<tokio::sync::Mutex<()>>,
}

impl Repos {
    /// Creates the repository service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        github: Arc<dyn Github>,
        files: Arc<dyn Files>,
        process: Arc<dyn Process>,
    ) -> Self {
        Self {
            config,
            state,
            jobs,
            git,
            github,
            files,
            process,
            context_lifecycle: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(super) fn context_lifecycle(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.context_lifecycle)
    }

    /// Resumes reconciliation for detached clones recorded before a daemon restart.
    pub async fn reconcile_startup(&self) -> DaemonResult<()> {
        let clones = self
            .state
            .load()
            .await?
            .clones
            .into_iter()
            .filter(|clone| {
                clone.status != CloneStatus::Failed
                    || self
                        .files
                        .exists(&clone_publish_marker_path(Path::new(&clone.path)))
            })
            .collect::<Vec<_>>();
        for clone in clones {
            let pid_file = clone_pid_path(&clone);
            let publication_pending = self
                .files
                .exists(&clone_publish_marker_path(Path::new(&clone.path)));
            let recovered_pid = clone
                .pid
                .or_else(|| read_clone_pid(self.files.as_ref(), &pid_file));
            if publication_pending {
                let _ignored = self.files.remove_file(&pid_file);
            }
            if !publication_pending && recovered_pid.is_none() {
                mark_clone_interrupted_before_launch(&self.state, &clone.id).await?;
                continue;
            }
            if clone.pid.is_none()
                && let Some(pid) = recovered_pid
            {
                let clone_id = clone.id.clone();
                self.state
                    .transaction(move |state| {
                        let record = state
                            .clones
                            .iter_mut()
                            .find(|record| record.id == clone_id)
                            .ok_or(DaemonError::Cancelled)?;
                        record.pid = Some(pid);
                        record.status = CloneStatus::Cloning;
                        Ok(())
                    })
                    .await?;
                let _ignored = self.files.remove_file(&pid_file);
            }
            let pid_to_wait = if publication_pending {
                None
            } else {
                recovered_pid
            };
            let state = Arc::clone(&self.state);
            let git = Arc::clone(&self.git);
            let files = Arc::clone(&self.files);
            let process = Arc::clone(&self.process);
            self.jobs.submit_for_repo(
                clone.id.clone(),
                JobKind::Clone,
                format!("{}:startup-reconcile", clone.id),
                format!("Reconcile clone {}", clone.id),
                JobPolicy::new(true, false),
                move |context| async move {
                    if let Some(pid) = pid_to_wait {
                        while process.is_alive(pid) {
                            tokio::select! {
                                () = context.cancel.cancelled() => {
                                    terminate_process_group(pid).await;
                                    return fail_clone(
                                        &state,
                                        &clone.id,
                                        Path::new(&clone.staging_path),
                                        files.as_ref(),
                                        DaemonError::Cancelled,
                                    ).await;
                                }
                                () = tokio::time::sleep(Duration::from_millis(100)) => {}
                            }
                        }
                    }
                    reconcile_clone(context, clone, state, git, files).await
                },
            )?;
        }
        Ok(())
    }

    /// Starts detached `git clone --progress` and reconciliation.
    pub async fn clone_repo(
        &self,
        owner: String,
        name: String,
        url: String,
        context: ContextId,
        default_branch: Option<String>,
    ) -> DaemonResult<JobRecord> {
        let _context_lifecycle = self.context_lifecycle.lock().await;
        let id = RepoId::try_from(format!("{owner}/{name}"))
            .map_err(|error| DaemonError::Validation(error.to_string()))?;
        self.jobs.ensure_repo_available(&id)?;
        if url.trim().is_empty() {
            return Err(DaemonError::Validation(
                "repository URL must not be empty".to_owned(),
            ));
        }
        let config = self.config.load().await?;
        let final_path = PathBuf::from(&config.repos_dir).join(&owner).join(&name);
        let attempt = Uuid::new_v4();
        let staging_path = PathBuf::from(&config.repos_dir)
            .join(&owner)
            .join(format!("{name}.staging-{}-{attempt}", std::process::id()));
        let home = fleet_home(&self.state)?;
        let default_branch = default_branch
            .filter(|branch| !branch.trim().is_empty())
            .or_else(|| {
                read_cache::<RepoCache>(self.files.as_ref(), &home.github_owner_cache_path(&owner))
                    .and_then(|cache| {
                        cache
                            .repos
                            .into_iter()
                            .find(|repo| repo.full_name == id.as_str())
                            .map(|repo| repo.default_branch)
                    })
            })
            .unwrap_or_else(|| "main".to_owned());
        let log_path = home
            .logs_dir()
            .join(format!("clone-{owner}-{name}-{attempt}.log"));
        let clone = CloneJob {
            id: id.clone(),
            owner,
            name,
            url,
            context_id: context.clone(),
            default_branch,
            path: final_path.to_string_lossy().into_owned(),
            staging_path: staging_path.to_string_lossy().into_owned(),
            log_path: log_path.to_string_lossy().into_owned(),
            pid: None,
            started_at: Utc::now().to_rfc3339(),
            status: CloneStatus::Starting,
            error: None,
        };
        let clone_for_state = clone.clone();
        let id_for_state = id.clone();
        let final_path_for_state = final_path.clone();
        let staging_for_state = staging_path.clone();
        let files_for_state = Arc::clone(&self.files);
        self.state
            .transaction(move |state| {
                if !state.contexts.iter().any(|item| item.id == context) {
                    return Err(DaemonError::NotFound(format!("context {context}")));
                }
                if state.repos.iter().any(|repo| repo.id == id_for_state)
                    || state.clones.iter().any(|job| job.id == id_for_state)
                {
                    return Err(DaemonError::Conflict(format!("repository {id_for_state}")));
                }
                if files_for_state.exists(&final_path_for_state)
                    || files_for_state.exists(&staging_for_state)
                {
                    return Err(DaemonError::Conflict(format!(
                        "repository path already exists for {id_for_state}"
                    )));
                }
                state.clones.push(clone_for_state);
                Ok(())
            })
            .await?;

        let state = Arc::clone(&self.state);
        let git = Arc::clone(&self.git);
        let files = Arc::clone(&self.files);
        let operation_id = clone.id.clone();
        let job_id = self.jobs.submit_for_repo(
            id.clone(),
            JobKind::Clone,
            clone.id.to_string(),
            format!("Clone {}", clone.id),
            JobPolicy::new(true, true),
            move |context| async move { clone_operation(context, clone, state, git, files).await },
        )?;
        self.jobs.record(&job_id).ok_or_else(|| {
            DaemonError::NotFound(format!("clone job for repository {operation_id}"))
        })
    }

    /// Deletes one repository's registry entries and trashes its directories. Callers
    /// hold the deletion guard and have already removed the repository's worktrees; the
    /// whole cascade lives in `Services::delete_repo_cascade`.
    pub(crate) async fn delete_guarded(&self, repo: RepoId) -> DaemonResult<()> {
        let snapshot = self.state.load().await?;
        let registered = snapshot.repos.iter().find(|item| item.id == repo).cloned();
        let clone = snapshot.clones.iter().find(|item| item.id == repo).cloned();
        if registered.is_none() && clone.is_none() {
            return Err(DaemonError::NotFound(format!("repository {repo}")));
        }

        self.jobs.quiesce_repo(&repo).await?;
        let lock = self.jobs.repo_lock(&repo);
        let _repo_guard = lock.lock().await;

        let config = self.config.load().await?;
        let mut paths = Vec::new();
        if let Some(item) = &registered {
            paths.push(PathBuf::from(&item.path));
            paths.push(
                PathBuf::from(&config.worktrees_dir)
                    .join(item.id.owner())
                    .join(item.id.name()),
            );
        }
        if let Some(item) = &clone {
            paths.push(PathBuf::from(&item.staging_path));
        }
        paths.sort();
        paths.dedup();

        let files = Arc::clone(&self.files);
        let moved = Arc::new(Mutex::new(Vec::<(PathBuf, PathBuf)>::new()));
        let moved_in_transaction = Arc::clone(&moved);
        let repo_in_transaction = repo.clone();
        let result = self
            .state
            .transaction(move |state| {
                for path in &paths {
                    if files.exists(path) {
                        let destination = files.trash(path)?;
                        moved_in_transaction
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push((path.clone(), destination));
                    }
                }
                state
                    .worktrees
                    .retain(|worktree| worktree.repo_id != repo_in_transaction);
                state.repos.retain(|item| item.id != repo_in_transaction);
                state.clones.retain(|item| item.id != repo_in_transaction);
                Ok(())
            })
            .await;

        let moved = moved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Err(error) = result {
            for (source, destination) in moved.iter().rev() {
                if self.files.exists(destination) {
                    let _ignored = self.files.rename(destination, source);
                }
            }
            return Err(error);
        }
        for (_source, destination) in moved {
            self.files.remove_detached(&destination)?;
        }
        Ok(())
    }

    /// Moves a repository to an existing context in one state transaction.
    pub async fn move_to_context(&self, repo: RepoId, context: ContextId) -> DaemonResult<Repo> {
        let _context_lifecycle = self.context_lifecycle.lock().await;
        self.state
            .transaction(move |state| {
                if !state.contexts.iter().any(|item| item.id == context) {
                    return Err(DaemonError::NotFound(format!("context {context}")));
                }
                let item = state
                    .repos
                    .iter_mut()
                    .find(|item| item.id == repo)
                    .ok_or_else(|| DaemonError::NotFound(format!("repository {repo}")))?;
                item.context_id = context;
                Ok(item.clone())
            })
            .await
    }

    /// Searches cached/discovered owner repositories using case-insensitive token matching.
    pub async fn search_remote(&self, owner: String, query: String) -> DaemonResult<RepoCache> {
        let mut cache = self.list_remote(owner, false).await?;
        let tokens = query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        cache.repos.retain(|repo| {
            let haystack =
                format!("{} {} {}", repo.full_name, repo.name, repo.description).to_lowercase();
            tokens.iter().all(|token| haystack.contains(token))
        });
        cache.repos.truncate(8);
        Ok(cache)
    }

    /// Loads or refreshes the owner repository cache through exact `gh` commands.
    pub async fn list_remote(&self, owner: String, force: bool) -> DaemonResult<RepoCache> {
        validate_owner(&owner)?;
        let config = self.config.load().await?;
        let cache_path = fleet_home(&self.state)?.github_owner_cache_path(&owner);
        let cached = read_cache::<RepoCache>(self.files.as_ref(), &cache_path);
        if !force
            && cached.as_ref().is_some_and(|cache| {
                cache_is_fresh(&cache.fetched_at, config.github.cache_ttl_seconds)
            })
        {
            return cached.ok_or_else(|| DaemonError::NotFound("repository cache".to_owned()));
        }

        let github = Arc::clone(&self.github);
        let files = Arc::clone(&self.files);
        let semaphore = self.jobs.github_semaphore();
        let owner_for_job = owner.clone();
        let target = format!("{owner}:{}", Uuid::new_v4());
        let (delivery, awaited) = JobDelivery::caller_gets_copy(github_error);
        self.jobs.submit(
            JobKind::RepoDiscovery,
            target,
            format!("Discover repositories for {owner}"),
            true,
            true,
            move |context| async move {
                delivery.finish(
                    async {
                        let _permit = semaphore
                            .acquire_owned()
                            .await
                            .map_err(|error| DaemonError::Join(error.to_string()))?;
                        context.progress(format!("listing repositories for {owner_for_job}"))?;
                        let repos = github.list_repositories(&owner_for_job).await?;
                        let cache = RepoCache {
                            fetched_at: Utc::now().to_rfc3339(),
                            repos,
                        };
                        write_cache(files.as_ref(), &cache_path, &cache)?;
                        Ok(cache)
                    }
                    .await,
                )
            },
        );
        match awaited.wait().await {
            Ok(cache) => Ok(cache),
            // Every delivered failure arrives in the GitHub category; a bare cancellation
            // means the job was dropped before it delivered anything.
            Err(DaemonError::Cancelled) => Err(DaemonError::Cancelled),
            Err(error) => cached.ok_or(error),
        }
    }

    /// Lists remote-tracking base refs, optionally fetching first.
    pub async fn list_base_refs(&self, repo: RepoId, force: bool) -> DaemonResult<BaseRefs> {
        let state = self.state.load().await?;
        let item = state
            .repos
            .iter()
            .find(|item| item.id == repo)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(format!("repository {repo}")))?;
        let path = PathBuf::from(item.path);
        if force {
            let git = Arc::clone(&self.git);
            let path_for_job = path.clone();
            let (delivery, awaited) = JobDelivery::caller_gets_copy(git_error);
            self.jobs.submit_for_repo(
                repo.clone(),
                JobKind::RepoFetch,
                format!("{repo}:{}", Uuid::new_v4()),
                format!("Fetch {repo}"),
                JobPolicy::new(true, true),
                move |context| async move {
                    delivery.finish(
                        async {
                            context.progress("fetching origin")?;
                            git.fetch(&path_for_job, true).await
                        }
                        .await,
                    )
                },
            )?;
            awaited.wait().await?;
        }
        let mut refs = self
            .git
            .remote_branches(&path)
            .await?
            .into_iter()
            .filter(|reference| reference.starts_with("origin/") && reference != "origin/HEAD")
            .collect::<Vec<_>>();
        refs.sort();
        refs.dedup();
        Ok(BaseRefs {
            refs,
            fetching: false,
            fetched_at: Utc::now().to_rfc3339(),
        })
    }

    /// Replaces one repository's prepare and post-create hooks.
    pub async fn set_hooks(&self, repo: RepoId, hooks: RepoHooks) -> DaemonResult<Repo> {
        self.state
            .transaction(move |state| {
                let item = state
                    .repos
                    .iter_mut()
                    .find(|item| item.id == repo)
                    .ok_or_else(|| DaemonError::NotFound(format!("repository {repo}")))?;
                item.hooks = hooks;
                Ok(item.clone())
            })
            .await
    }

    /// Dismisses a retained failed clone record.
    pub async fn dismiss_clone(&self, repo: RepoId) -> DaemonResult<()> {
        self.state
            .transaction(move |state| {
                let clone = state
                    .clones
                    .iter()
                    .find(|clone| clone.id == repo)
                    .ok_or_else(|| DaemonError::NotFound(format!("clone {repo}")))?;
                if clone.status != CloneStatus::Failed {
                    return Err(DaemonError::Conflict(format!(
                        "clone {repo} has not failed"
                    )));
                }
                state.clones.retain(|clone| clone.id != repo);
                Ok(())
            })
            .await
    }
}

async fn clone_operation(
    context: JobCtx,
    clone: CloneJob,
    state: Arc<StateStore>,
    git: Arc<dyn Git>,
    files: Arc<dyn Files>,
) -> DaemonResult<()> {
    let clone = begin_clone_attempt(&state, clone).await?;
    context.progress("starting detached clone")?;
    let staging = PathBuf::from(&clone.staging_path);
    let log = PathBuf::from(&clone.log_path);
    let pid_file = clone_pid_path(&clone);
    if let Some(parent) = staging.parent()
        && let Err(error) = files.create_dir_all(parent)
    {
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
    let process = match git.clone_repo(&clone.url, &staging, &log, &pid_file).await {
        Ok(process) => process,
        Err(error) => return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await,
    };
    let clone_id = clone.id.clone();
    if let Err(error) = state
        .transaction(move |state| {
            let record = state
                .clones
                .iter_mut()
                .find(|record| record.id == clone_id)
                .ok_or(DaemonError::Cancelled)?;
            record.pid = Some(process.pid);
            record.status = CloneStatus::Cloning;
            Ok(())
        })
        .await
    {
        terminate_process_group(process.pid).await;
        let _ignored = files.remove_file(&pid_file);
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
    let _ignored = files.remove_file(&pid_file);
    context.progress(format!("clone running as pid {}", process.pid))?;

    while pid_is_alive(process.pid) {
        tokio::select! {
            () = context.cancel.cancelled() => {
                terminate_process_group(process.pid).await;
                return fail_clone(
                    &state,
                    &clone.id,
                    &staging,
                    files.as_ref(),
                    DaemonError::Cancelled,
                ).await;
            }
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    reconcile_clone(context, clone, state, git, files).await
}

async fn reconcile_clone(
    context: JobCtx,
    clone: CloneJob,
    state: Arc<StateStore>,
    git: Arc<dyn Git>,
    files: Arc<dyn Files>,
) -> DaemonResult<()> {
    let staging = PathBuf::from(&clone.staging_path);
    let final_path = PathBuf::from(&clone.path);
    if files.exists(&final_path) {
        return recover_published_clone(context, clone, state, git, files).await;
    }
    if let Err(error) = git.revision(&staging, "HEAD").await {
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
    context.progress("reconciling clone")?;
    let default_branch =
        resolve_default_branch(git.as_ref(), &staging, &clone.default_branch).await;
    let repo = Repo {
        id: clone.id.clone(),
        owner: clone.owner.clone(),
        name: clone.name.clone(),
        url: clone.url.clone(),
        context_id: clone.context_id.clone(),
        default_branch,
        path: clone.path.clone(),
        cloned_at: Utc::now().to_rfc3339(),
        hooks: RepoHooks::default(),
    };
    let intent = ClonePublishIntent { repo: repo.clone() };
    let mut intent_text = serde_json::to_string_pretty(&intent)?;
    intent_text.push('\n');
    let clone_id = clone.id.clone();
    let published = Arc::new(Mutex::new(false));
    let published_in_transaction = Arc::clone(&published);
    let files_in_transaction = Arc::clone(&files);
    let staging_in_transaction = staging.clone();
    let final_in_transaction = final_path.clone();
    let marker_in_transaction = clone_publish_marker_path(&staging);
    let result = state
        .transaction(move |state| {
            if !state.clones.iter().any(|record| record.id == clone_id) {
                return Err(DaemonError::Cancelled);
            }
            if state.repos.iter().any(|record| record.id == clone_id) {
                return Err(DaemonError::Conflict(format!("repository {clone_id}")));
            }
            if files_in_transaction.exists(&final_in_transaction) {
                return Err(DaemonError::Conflict(format!(
                    "repository path already exists: {}",
                    final_in_transaction.display()
                )));
            }
            files_in_transaction.atomic_write_text(&marker_in_transaction, &intent_text)?;
            files_in_transaction.rename(&staging_in_transaction, &final_in_transaction)?;
            *published_in_transaction
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            state.clones.retain(|record| record.id != clone_id);
            state.repos.push(repo);
            Ok(())
        })
        .await;
    if let Err(error) = result {
        if *published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            let error = DaemonError::Join(format!(
                "clone publication state save failed: {error}; published repository preserved at {}",
                final_path.display()
            ));
            return record_clone_failure(&state, &clone.id, error).await;
        }
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
    files.remove_file(&clone_publish_marker_path(&final_path))?;
    context.progress("clone registered")?;
    Ok(())
}

async fn begin_clone_attempt(state: &StateStore, clone: CloneJob) -> DaemonResult<CloneJob> {
    let fresh_staging = clone_staging_attempt(&clone)?;
    let clone_id = clone.id.clone();
    state
        .transaction(move |state| {
            let record = state
                .clones
                .iter_mut()
                .find(|record| record.id == clone_id)
                .ok_or(DaemonError::Cancelled)?;
            if record.status == CloneStatus::Failed {
                record.staging_path = fresh_staging.to_string_lossy().into_owned();
                record.pid = None;
                record.status = CloneStatus::Starting;
                record.error = None;
            }
            Ok(record.clone())
        })
        .await
}

async fn recover_published_clone(
    context: JobCtx,
    clone: CloneJob,
    state: Arc<StateStore>,
    git: Arc<dyn Git>,
    files: Arc<dyn Files>,
) -> DaemonResult<()> {
    let final_path = PathBuf::from(&clone.path);
    let marker = clone_publish_marker_path(&final_path);
    let intent = files
        .read_text(&marker)
        .ok()
        .and_then(|text| serde_json::from_str::<ClonePublishIntent>(&text).ok())
        .filter(|intent| valid_clone_publish_intent(intent, &clone, &final_path));
    let Some(intent) = intent else {
        return record_clone_failure(
            &state,
            &clone.id,
            DaemonError::Conflict(format!(
                "repository destination exists without a valid clone publish intent: {}; completed clone preserved at {}",
                final_path.display(),
                clone.staging_path
            )),
        )
        .await;
    };
    if let Err(error) = git.revision(&final_path, "HEAD").await {
        return record_clone_failure(&state, &clone.id, error).await;
    }
    context.progress("recovering published clone")?;
    let clone_id = clone.id.clone();
    state
        .transaction(move |state| {
            if !state.clones.iter().any(|record| record.id == clone_id) {
                return Err(DaemonError::Cancelled);
            }
            if state.repos.iter().any(|record| record.id == clone_id) {
                return Err(DaemonError::Conflict(format!("repository {clone_id}")));
            }
            state.clones.retain(|record| record.id != clone_id);
            state.repos.push(intent.repo);
            Ok(())
        })
        .await?;
    files.remove_file(&marker)?;
    context.progress("clone registered")?;
    Ok(())
}

async fn fail_clone(
    state: &StateStore,
    repo: &RepoId,
    staging: &Path,
    files: &dyn Files,
    error: DaemonError,
) -> DaemonResult<()> {
    let error = if files.exists(staging) {
        match files.remove_detached(staging) {
            Ok(()) => error,
            Err(cleanup) => DaemonError::Join(format!(
                "clone failed: {error}; cleanup failed for {}: {cleanup}",
                staging.display()
            )),
        }
    } else {
        error
    };
    record_clone_failure(state, repo, error).await
}

async fn record_clone_failure(
    state: &StateStore,
    repo: &RepoId,
    error: DaemonError,
) -> DaemonResult<()> {
    let persisted_message = error.to_string();
    let repo = repo.clone();
    state
        .transaction(move |state| {
            if let Some(clone) = state.clones.iter_mut().find(|clone| clone.id == repo) {
                clone.status = CloneStatus::Failed;
                clone.error = Some(persisted_message);
                clone.pid = None;
            }
            Ok(())
        })
        .await?;
    Err(error)
}

async fn mark_clone_interrupted_before_launch(
    state: &StateStore,
    repo: &RepoId,
) -> DaemonResult<()> {
    let repo = repo.clone();
    state
        .transaction(move |state| {
            if let Some(clone) = state.clones.iter_mut().find(|item| item.id == repo) {
                clone.status = CloneStatus::Failed;
                clone.error = Some("clone was interrupted before its process started".to_owned());
            }
            Ok(())
        })
        .await
}

fn clone_staging_attempt(clone: &CloneJob) -> DaemonResult<PathBuf> {
    let parent = Path::new(&clone.staging_path)
        .parent()
        .ok_or_else(|| DaemonError::Validation("clone staging path has no parent".to_owned()))?;
    Ok(parent.join(format!(
        "{}.staging-{}-{}",
        clone.name,
        std::process::id(),
        Uuid::new_v4()
    )))
}

fn clone_pid_path(clone: &CloneJob) -> PathBuf {
    PathBuf::from(format!("{}.pid", clone.log_path))
}

fn read_clone_pid(files: &dyn Files, path: &Path) -> Option<u32> {
    files.read_text(path).ok()?.trim().parse().ok()
}

fn valid_clone_publish_intent(
    intent: &ClonePublishIntent,
    clone: &CloneJob,
    final_path: &Path,
) -> bool {
    let repo = &intent.repo;
    repo.id == clone.id
        && repo.owner == clone.owner
        && repo.name == clone.name
        && repo.url == clone.url
        && repo.context_id == clone.context_id
        && Path::new(&repo.path) == final_path
        && !repo.default_branch.is_empty()
}

async fn resolve_default_branch(git: &dyn Git, path: &Path, hint: &str) -> String {
    if let Ok(reference) = git.origin_head(path).await
        && let Some(branch) = reference.strip_prefix("refs/remotes/origin/")
        && !branch.is_empty()
    {
        return branch.to_owned();
    }
    if git.repair_origin_head(path).await.is_ok()
        && let Ok(reference) = git.origin_head(path).await
        && let Some(branch) = reference.strip_prefix("refs/remotes/origin/")
        && !branch.is_empty()
    {
        return branch.to_owned();
    }
    if !hint.is_empty() && git.remote_branch_exists(path, hint).await.unwrap_or(false) {
        return hint.to_owned();
    }
    if let Ok(branches) = git.remote_branches(path).await {
        for candidate in ["origin/main", "origin/master"] {
            if branches.iter().any(|branch| branch == candidate) {
                return candidate.trim_start_matches("origin/").to_owned();
            }
        }
        if let Some(branch) = branches
            .iter()
            .filter_map(|branch| branch.strip_prefix("origin/"))
            .find(|branch| *branch != "HEAD" && !branch.is_empty())
        {
            return branch.to_owned();
        }
        if branches.is_empty() && !hint.is_empty() {
            return hint.to_owned();
        }
    }
    if let Ok(branch) = git.symbolic_head(path).await
        && !branch.is_empty()
    {
        return branch;
    }
    "main".to_owned()
}

async fn terminate_process_group(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    signal_process_group(pid, libc::SIGTERM);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while pid_is_alive(pid.cast_unsigned()) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if pid_is_alive(pid.cast_unsigned()) {
        signal_process_group(pid, libc::SIGKILL);
    }
}

fn signal_process_group(pid: i32, signal: i32) {
    // SAFETY: the PID comes from a child Fleet launched in its own process group.
    let group_result = unsafe { libc::kill(-pid, signal) };
    if group_result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        // Older persisted clone attempts may predate process-group launch.
        // SAFETY: the PID is the exact persisted child PID.
        let _result = unsafe { libc::kill(pid, signal) };
    }
}

fn validate_owner(owner: &str) -> DaemonResult<()> {
    if owner.is_empty()
        || owner == "."
        || owner == ".."
        || owner.contains(['/', '\\'])
        || owner.chars().any(char::is_whitespace)
    {
        Err(DaemonError::Validation(format!(
            "invalid GitHub owner `{owner}`"
        )))
    } else {
        Ok(())
    }
}
