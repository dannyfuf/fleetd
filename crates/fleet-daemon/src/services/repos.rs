//! Repository registration, discovery, caching, and clone orchestration.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use fleet_core::{
    cache::RepoCache,
    ids::{ContextId, JobId, RepoId},
    model::{CloneJob, CloneStatus, Repo, RepoHooks},
    paths::FleetHome,
};
use fleet_proto::{
    job::{JobKind, JobRecord, JobStatus},
    response::BaseRefs,
};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{files::Files, git::Git, github::Github, process::Process},
    jobs::{JobCtx, JobManager},
    stores::{config::ConfigStore, state::StateStore},
};

/// Repository domain service with durable state, configuration, and job scheduling handles.
#[derive(Clone)]
pub struct Repos {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    git: Arc<dyn Git>,
    github: Arc<dyn Github>,
    files: Arc<dyn Files>,
    process: Option<Arc<dyn Process>>,
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
    ) -> Self {
        Self {
            config,
            state,
            jobs,
            git,
            github,
            files,
            process: None,
        }
    }

    /// Adds process liveness for daemon-startup clone reconciliation.
    #[must_use]
    pub fn with_process(mut self, process: Arc<dyn Process>) -> Self {
        self.process = Some(process);
        self
    }

    /// Resumes reconciliation for detached clones recorded before a daemon restart.
    pub async fn reconcile_startup(&self) -> DaemonResult<()> {
        let clones = self
            .state
            .load()
            .await?
            .clones
            .into_iter()
            .filter(|clone| clone.status != CloneStatus::Failed)
            .collect::<Vec<_>>();
        for clone in clones {
            let Some(pid) = clone.pid else {
                let repo = clone.id.clone();
                self.state
                    .transaction(move |state| {
                        if let Some(clone) = state.clones.iter_mut().find(|item| item.id == repo) {
                            clone.status = CloneStatus::Failed;
                            clone.error =
                                Some("clone was interrupted before its process started".to_owned());
                        }
                        Ok(())
                    })
                    .await?;
                continue;
            };
            let state = Arc::clone(&self.state);
            let git = Arc::clone(&self.git);
            let files = Arc::clone(&self.files);
            let process = self.process.clone();
            self.jobs.submit(
                JobKind::Clone,
                format!("{}:startup-reconcile", clone.id),
                format!("Reconcile clone {}", clone.id),
                true,
                false,
                move |context| async move {
                    while process
                        .as_ref()
                        .map_or_else(|| process_is_alive(pid), |process| process.is_alive(pid))
                    {
                        tokio::select! {
                            () = context.cancel.cancelled() => return Err(DaemonError::Cancelled),
                            () = tokio::time::sleep(Duration::from_millis(100)) => {}
                        }
                    }
                    reconcile_clone(context, clone, state, git, files).await
                },
            );
        }
        Ok(())
    }

    /// Starts detached `git clone --progress` and reconciliation (inventory sections 2, 6, and 7).
    pub async fn clone_repo(
        &self,
        owner: String,
        name: String,
        url: String,
        context: ContextId,
        default_branch: Option<String>,
    ) -> DaemonResult<JobRecord> {
        let id = RepoId::try_from(format!("{owner}/{name}"))
            .map_err(|error| DaemonError::Validation(error.to_string()))?;
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
        let job_id = self.jobs.submit(
            JobKind::Clone,
            clone.id.to_string(),
            format!("Clone {}", clone.id),
            true,
            true,
            move |context| async move { clone_operation(context, clone, state, git, files).await },
        );
        job_record(&self.jobs, &job_id).ok_or_else(|| {
            DaemonError::NotFound(format!("clone job for repository {operation_id}"))
        })
    }

    /// Cascades repository deletion through registered children and recoverable trash.
    pub async fn delete(&self, repo: RepoId) -> DaemonResult<()> {
        let snapshot = self.state.load().await?;
        if snapshot
            .worktrees
            .iter()
            .any(|worktree| worktree.repo_id == repo && worktree.host.is_some())
        {
            return Err(DaemonError::Unsupported(
                "remote hosts are not supported yet".to_owned(),
            ));
        }
        let registered = snapshot.repos.iter().find(|item| item.id == repo).cloned();
        let clone = snapshot.clones.iter().find(|item| item.id == repo).cloned();
        if registered.is_none() && clone.is_none() {
            return Err(DaemonError::NotFound(format!("repository {repo}")));
        }

        for job in self.jobs.list() {
            if (job.target == repo.as_str()
                || job.target.starts_with(&format!("{repo}:"))
                || job.target.starts_with(&format!("{repo}#")))
                && matches!(job.status, JobStatus::Queued | JobStatus::Running)
            {
                let _ignored = self.jobs.cancel(&job.id);
            }
        }

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
                if state.worktrees.iter().any(|worktree| {
                    worktree.repo_id == repo_in_transaction && worktree.host.is_some()
                }) {
                    return Err(DaemonError::Unsupported(
                        "remote hosts are not supported yet".to_owned(),
                    ));
                }
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

    /// Moves a repository to an existing context in one state transaction (inventory sections 1 and 6).
    pub async fn move_to_context(&self, repo: RepoId, context: ContextId) -> DaemonResult<Repo> {
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
        let (sender, receiver) = oneshot::channel::<Result<RepoCache, String>>();
        let sender = Arc::new(Mutex::new(Some(sender)));
        self.jobs.submit(
            JobKind::RepoDiscovery,
            target,
            format!("Discover repositories for {owner}"),
            true,
            true,
            move |context| async move {
                let result: DaemonResult<RepoCache> = async {
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
                .await;
                match result {
                    Ok(cache) => {
                        if let Some(sender) = sender
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                        {
                            let _ignored = sender.send(Ok(cache));
                        }
                        Ok(())
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if let Some(sender) = sender
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                        {
                            let _ignored = sender.send(Err(message));
                        }
                        Err(error)
                    }
                }
            },
        );
        match receiver.await {
            Ok(Ok(cache)) => Ok(cache),
            Ok(Err(_error)) if cached.is_some() => {
                cached.ok_or_else(|| DaemonError::NotFound("repository cache".to_owned()))
            }
            Ok(Err(error)) => Err(DaemonError::Github(error)),
            Err(_) => Err(DaemonError::Cancelled),
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
            let (sender, receiver) = oneshot::channel::<Result<(), String>>();
            let sender = Arc::new(Mutex::new(Some(sender)));
            self.jobs.submit(
                JobKind::RepoFetch,
                format!("{repo}:{}", Uuid::new_v4()),
                format!("Fetch {repo}"),
                true,
                true,
                move |context| async move {
                    context.progress("fetching origin")?;
                    let result = git.fetch(&path_for_job, true).await;
                    match result {
                        Ok(()) => {
                            if let Some(sender) = sender
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .take()
                            {
                                let _ignored = sender.send(Ok(()));
                            }
                            Ok(())
                        }
                        Err(error) => {
                            let message = error.to_string();
                            if let Some(sender) = sender
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .take()
                            {
                                let _ignored = sender.send(Err(message));
                            }
                            Err(error)
                        }
                    }
                },
            );
            match receiver.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(DaemonError::Git(error)),
                Err(_) => return Err(DaemonError::Cancelled),
            }
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

    /// Starts a non-destructive import from the default swarm home.
    pub async fn import_from_swarm(&self) -> DaemonResult<JobRecord> {
        Err(DaemonError::Unimplemented("repos::import_from_swarm"))
    }
}

async fn clone_operation(
    context: JobCtx,
    clone: CloneJob,
    state: Arc<StateStore>,
    git: Arc<dyn Git>,
    files: Arc<dyn Files>,
) -> DaemonResult<()> {
    context.progress("starting detached clone")?;
    let staging = PathBuf::from(&clone.staging_path);
    let log = PathBuf::from(&clone.log_path);
    if let Some(parent) = staging.parent()
        && let Err(error) = files.create_dir_all(parent)
    {
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
    let process = match git.clone_repo(&clone.url, &staging, &log).await {
        Ok(process) => process,
        Err(error) => return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await,
    };
    let clone_id = clone.id.clone();
    state
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
        .await?;
    context.progress(format!("clone running as pid {}", process.pid))?;

    while process_is_alive(process.pid) {
        tokio::select! {
            () = context.cancel.cancelled() => return Err(DaemonError::Cancelled),
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
    if let Err(error) = git.revision(&staging, "HEAD").await {
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
    context.progress("reconciling clone")?;
    let default_branch =
        resolve_default_branch(git.as_ref(), &staging, &clone.default_branch).await;
    if files.exists(&final_path) {
        return fail_clone(
            &state,
            &clone.id,
            &staging,
            files.as_ref(),
            DaemonError::Conflict(format!(
                "repository path already exists: {}",
                final_path.display()
            )),
        )
        .await;
    }
    if let Err(error) = files.rename(&staging, &final_path) {
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
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
    let clone_id = clone.id.clone();
    let result = state
        .transaction(move |state| {
            if !state.clones.iter().any(|record| record.id == clone_id) {
                return Err(DaemonError::Cancelled);
            }
            if state.repos.iter().any(|record| record.id == clone_id) {
                return Err(DaemonError::Conflict(format!("repository {clone_id}")));
            }
            state.clones.retain(|record| record.id != clone_id);
            state.repos.push(repo);
            Ok(())
        })
        .await;
    if let Err(error) = result {
        let _ignored = files.rename(&final_path, &staging);
        return fail_clone(&state, &clone.id, &staging, files.as_ref(), error).await;
    }
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
    if files.exists(staging) {
        let _ignored = files.remove_detached(staging);
    }
    Err(error)
}

async fn resolve_default_branch(git: &dyn Git, path: &Path, hint: &str) -> String {
    if let Ok(reference) = git.origin_head(path).await
        && let Some(branch) = reference.strip_prefix("refs/remotes/origin/")
        && !branch.is_empty()
    {
        return branch.to_owned();
    }
    if !hint.is_empty() && git.remote_branch_exists(path, hint).await.unwrap_or(false) {
        return hint.to_owned();
    }
    if git.repair_origin_head(path).await.is_ok()
        && let Ok(reference) = git.origin_head(path).await
        && let Some(branch) = reference.strip_prefix("refs/remotes/origin/")
        && !branch.is_empty()
    {
        return branch.to_owned();
    }
    if let Ok(branches) = git.remote_branches(path).await {
        for candidate in ["origin/main", "origin/master"] {
            if branches.iter().any(|branch| branch == candidate) {
                return candidate.trim_start_matches("origin/").to_owned();
            }
        }
        if let Some(branch) = branches
            .iter()
            .find_map(|branch| branch.strip_prefix("origin/"))
            .filter(|branch| *branch != "HEAD" && !branch.is_empty())
        {
            return branch.to_owned();
        }
    }
    if hint.is_empty() {
        "main".to_owned()
    } else {
        hint.to_owned()
    }
}

fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal zero performs an existence/permission check and does not modify the process.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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

fn cache_is_fresh(fetched_at: &str, ttl_seconds: i64) -> bool {
    if ttl_seconds < 0 {
        return false;
    }
    DateTime::parse_from_rfc3339(fetched_at)
        .ok()
        .map(|fetched| Utc::now().signed_duration_since(fetched.with_timezone(&Utc)))
        .is_some_and(|age| age.num_milliseconds() >= 0 && age.num_seconds() < ttl_seconds)
}

fn read_cache<T: serde::de::DeserializeOwned>(files: &dyn Files, path: &Path) -> Option<T> {
    files
        .read_text(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn write_cache<T: serde::Serialize>(files: &dyn Files, path: &Path, cache: &T) -> DaemonResult<()> {
    let mut text = serde_json::to_string(cache)?;
    text.push('\n');
    files.atomic_write_text(path, &text)
}

fn fleet_home(state: &StateStore) -> DaemonResult<FleetHome> {
    state
        .path()
        .parent()
        .map(|path| FleetHome::new(path.to_path_buf()))
        .ok_or_else(|| DaemonError::Validation("state path has no parent".to_owned()))
}

fn job_record(jobs: &JobManager, id: &JobId) -> Option<JobRecord> {
    jobs.list().into_iter().find(|record| &record.id == id)
}
