//! Prepared worktree creation, recovery, publication, and deletion.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use fleet_core::{
    config::Config,
    ids::{HostId, JobId, RepoId, SessionId, WorktreeId},
    model::{Degraded, Repo, RepoHooks, Worktree},
    paths::{CreatingMarker, creating_marker_path, hot_marker_path, uuid_attempt_path},
    validate::{validate_branch, validate_slug},
};
use fleet_proto::{job::JobKind, response::WorktreeDeleteResult};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{files::Files, git::Git},
    jobs::{JobCtx, JobManager},
    stores::{config::ConfigStore, state::StateStore},
};

use super::pool::{CancellableCopy, Pool};

const TRASH_MARKER_FILE: &str = "fleet-trash.json";

/// Worktree service owning prepared-copy claim, publication, and deletion orchestration.
#[derive(Clone)]
pub struct Worktrees {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    files: Arc<dyn Files>,
    git: Arc<dyn Git>,
    pool: Pool,
    trash_jobs: Arc<Mutex<HashMap<String, JobId>>>,
    startup_ready: Arc<AtomicBool>,
    startup_notify: Arc<tokio::sync::Notify>,
}

impl Worktrees {
    /// Creates the worktree service and schedules startup intent recovery.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        files: Arc<dyn Files>,
        git: Arc<dyn Git>,
    ) -> Self {
        let service = Self {
            pool: Pool::without_background(
                Arc::clone(&config),
                Arc::clone(&state),
                Arc::clone(&jobs),
                Arc::clone(&git),
                Arc::clone(&files),
            ),
            config,
            state,
            jobs,
            files,
            git,
            trash_jobs: Arc::new(Mutex::new(HashMap::new())),
            startup_ready: Arc::new(AtomicBool::new(false)),
            startup_notify: Arc::new(tokio::sync::Notify::new()),
        };
        service.schedule_startup_recovery();
        service
    }

    /// Claims or copies a prepared slot and atomically publishes a worktree.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        repo: RepoId,
        slug: String,
        branch: Option<String>,
        base: Option<String>,
        host: Option<HostId>,
        hooks: RepoHooks,
    ) -> DaemonResult<(bool, Worktree)> {
        self.await_startup_recovery().await;
        validate_slug(&slug).map_err(|error| DaemonError::Validation(error.to_string()))?;
        if host.is_some() {
            return Err(remote_unsupported());
        }
        let requested_branch = branch.clone();
        let branch = branch.unwrap_or_else(|| slug.clone());
        validate_branch(&branch).map_err(|error| DaemonError::Validation(error.to_string()))?;

        if let Some(existing) = self
            .existing_create_result(&repo, &slug, requested_branch.as_deref())
            .await?
        {
            return Ok((false, existing));
        }

        let service = self.clone();
        let target = format!("{}#{}:{}", repo, slug, Uuid::new_v4());
        let title = format!("Create {repo}#{slug}");
        let (sender, receiver) = oneshot::channel();
        self.jobs.submit(
            JobKind::CreateWorktree,
            target,
            title,
            true,
            true,
            move |context| async move {
                let result = service
                    .create_local(repo, slug, branch, base, hooks, &context)
                    .await;
                let copied = result
                    .as_ref()
                    .map(Clone::clone)
                    .map_err(clone_daemon_error);
                let _ignored = sender.send(copied);
                result.map(drop)
            },
        );
        receiver.await.map_err(|_| DaemonError::Cancelled)?
    }

    /// Creates a worktree from `refs/swarm/pulls/<n>/head`.
    pub async fn create_from_pr(
        &self,
        repo: RepoId,
        number: u64,
    ) -> DaemonResult<(bool, Worktree)> {
        self.await_startup_recovery().await;
        let slug = format!("pr-{number}");
        if let Some(existing) = self.existing_create_result(&repo, &slug, None).await? {
            return Ok((false, existing));
        }
        let service = self.clone();
        let target = format!("{}#{}:{}", repo, slug, Uuid::new_v4());
        let (sender, receiver) = oneshot::channel();
        self.jobs.submit(
            JobKind::CreateWorktree,
            target,
            format!("Create {repo} pull request #{number}"),
            true,
            true,
            move |context| async move {
                let result = service.create_pr_local(repo, slug, number, &context).await;
                let copied = result
                    .as_ref()
                    .map(Clone::clone)
                    .map_err(clone_daemon_error);
                let _ignored = sender.send(copied);
                result.map(drop)
            },
        );
        receiver.await.map_err(|_| DaemonError::Cancelled)?
    }

    /// Deletes worktrees independently using transactional trash rollback.
    pub async fn delete(&self, ids: Vec<WorktreeId>) -> DaemonResult<Vec<WorktreeDeleteResult>> {
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            let result = self.delete_one_job(id.clone()).await;
            results.push(WorktreeDeleteResult {
                worktree_id: id,
                ok: result.is_ok(),
                reason: result.err().map(|error| error.to_string()),
            });
        }
        Ok(results)
    }

    /// Hard-kills the session associated with a worktree.
    pub async fn kill(&self, id: WorktreeId) -> DaemonResult<()> {
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .iter()
            .find(|worktree| worktree.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        // Session ownership is intentionally outside this service. Killing an absent local
        // daemon session is idempotent; the shared facade must wire Sessions for a live kill.
        Ok(())
    }

    /// Updates `lastOpenedAt` transactionally without changing worktree identity.
    pub async fn touch_opened(&self, id: WorktreeId) -> DaemonResult<()> {
        let now = Utc::now().to_rfc3339();
        self.state
            .transaction(move |state| {
                let worktree = state
                    .worktrees
                    .iter_mut()
                    .find(|worktree| worktree.id == id)
                    .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
                worktree.last_opened_at = Some(now);
                Ok(())
            })
            .await
    }

    /// Resolves the absolute path of a local worktree and rejects remote mirrors.
    pub async fn path(&self, id: WorktreeId) -> DaemonResult<String> {
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .into_iter()
            .find(|worktree| worktree.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        let path = PathBuf::from(&worktree.path);
        if !path.is_absolute() {
            return Err(DaemonError::Validation(format!(
                "worktree {id} has a non-absolute path"
            )));
        }
        Ok(path.to_string_lossy().into_owned())
    }

    /// Restores one validated worktree entry while its trash directory still exists.
    pub async fn restore_trash(&self, entry: String) -> DaemonResult<()> {
        validate_trash_entry(&entry)?;
        let config = self.config.load().await?;
        let trash = self.trash_dir().join(&entry);
        if !self.files.exists(&trash) {
            return Err(DaemonError::NotFound(format!("trash entry {entry}")));
        }
        let marker_path = trash_marker_path(&trash);
        let marker: TrashMarker = serde_json::from_str(&self.files.read_text(&marker_path)?)?;
        validate_restore_marker(&marker, &config)?;
        let destination = PathBuf::from(&marker.original_path);
        if self.files.exists(&destination) {
            return Err(DaemonError::Conflict(format!(
                "restore destination already exists: {}",
                destination.display()
            )));
        }

        let moved = Arc::new(Mutex::new(false));
        let moved_in_transaction = Arc::clone(&moved);
        let files = Arc::clone(&self.files);
        let trash_in_transaction = trash.clone();
        let destination_in_transaction = destination.clone();
        let worktree = marker.worktree.clone();
        let transaction = self
            .state
            .transaction(move |state| {
                if state.worktrees.iter().any(|item| item.id == worktree.id) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree {} is already registered",
                        worktree.id
                    )));
                }
                files.rename(&trash_in_transaction, &destination_in_transaction)?;
                *moved_in_transaction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                state.worktrees.push(worktree);
                Ok(())
            })
            .await;
        if let Err(error) = transaction {
            if *moved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                && self.files.exists(&destination)
            {
                let _ignored = self.files.rename(&destination, &trash);
            }
            return Err(error);
        }

        self.files.remove_file(&trash_marker_path(&destination))?;
        if let Some(job) = self
            .trash_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&entry)
        {
            let _ignored = self.jobs.cancel(&job);
        }
        Ok(())
    }

    /// Reconciles publish intents and removes abandoned private attempts.
    pub async fn recover_startup(&self) -> DaemonResult<()> {
        let config = self.config.load().await?;
        let state = self.state.load().await?;
        for repo in state.repos {
            let root = repo_worktrees_dir(&config, &repo.id);
            if !self.files.exists(&root) {
                continue;
            }
            for child in self.files.list(&root)? {
                let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if name.starts_with(".hot") || name.starts_with(".discard-") {
                    continue;
                }
                if let Some((slug, _attempt)) = name.rsplit_once(".creating-") {
                    self.recover_attempt(&config, &repo, slug, &child).await?;
                } else {
                    self.recover_published(&config, &repo, name, &child).await?;
                }
            }
        }
        Ok(())
    }

    async fn create_local(
        &self,
        repo_id: RepoId,
        slug: String,
        branch: String,
        base: Option<String>,
        hooks: RepoHooks,
        context: &JobCtx,
    ) -> DaemonResult<(bool, Worktree)> {
        let (mut repo, config) = self.repo_and_config(&repo_id).await?;
        repo.hooks = hooks.clone();
        let base_ref = base.unwrap_or_else(|| format!("origin/{}", repo.default_branch));
        let id = worktree_id(&repo_id, &slug)?;
        let root = repo_worktrees_dir(&config, &repo_id);
        self.files.create_dir_all(&root)?;
        self.reclaim_publish_intent(&id, &root.join(&slug)).await?;
        self.assert_create_conflicts(&id, &repo, &slug, &branch, &root)
            .await?;

        let attempt = uuid_attempt_path(&root, &slug, Uuid::new_v4());
        let mut attempt_guard = AttemptGuard::new(Arc::clone(&self.files), attempt.clone());
        context.progress("claiming prepared copy")?;
        let claimed = self
            .claim_or_fallback(&repo, &config, &root, &attempt, context)
            .await?;
        let result = async {
            let fresh = if claimed {
                self.pool
                    .is_fresh_copy(&attempt, &repo, &config)
                    .await
                    .unwrap_or(false)
            } else {
                false
            };
            if !fresh
                || base_ref != format!("origin/{}", repo.default_branch)
                || branch != repo.default_branch
            {
                self.fetch_attempt_refs(&attempt, &repo, &base_ref, Some(&branch))
                    .await?;
            }
            check_cancelled(context)?;
            if self.git.remote_branch_exists(&attempt, &branch).await? {
                self.git.checkout_branch(&attempt, &branch).await?;
            } else {
                if !self.git.revision_exists(&attempt, &base_ref).await? {
                    return Err(DaemonError::Git(format!(
                        "base ref {base_ref} does not exist"
                    )));
                }
                self.git
                    .checkout_new_branch(&attempt, &branch, &base_ref)
                    .await?;
            }
            if !fresh {
                self.run_prepare_hooks(&attempt, &hooks, context).await?;
            }
            self.files.remove_file(&hot_marker_path(&attempt))?;
            let worktree = self
                .publish(&repo, &slug, &branch, &base_ref, &hooks, &attempt)
                .await?;
            self.schedule_post_create(worktree.clone(), hooks.post_create.clone());
            Ok((true, worktree))
        }
        .await;

        if result.is_err() && self.files.exists(&attempt) {
            let cleanup = if matches!(result, Err(DaemonError::Conflict(_))) {
                self.trash_attempt(&attempt)
            } else {
                self.files.remove_detached(&attempt)
            };
            if let Err(error) = cleanup {
                tracing::warn!(%error, path = %attempt.display(), "failed to clean create attempt");
            }
        }
        attempt_guard.disarm();
        self.pool.refill(repo_id);
        result
    }

    async fn create_pr_local(
        &self,
        repo_id: RepoId,
        slug: String,
        number: u64,
        context: &JobCtx,
    ) -> DaemonResult<(bool, Worktree)> {
        let (repo, config) = self.repo_and_config(&repo_id).await?;
        let id = worktree_id(&repo_id, &slug)?;
        let root = repo_worktrees_dir(&config, &repo_id);
        self.files.create_dir_all(&root)?;
        self.reclaim_publish_intent(&id, &root.join(&slug)).await?;
        let branch = format!("pr/{number}");
        self.assert_create_conflicts(&id, &repo, &slug, &branch, &root)
            .await?;
        let attempt = uuid_attempt_path(&root, &slug, Uuid::new_v4());
        let mut attempt_guard = AttemptGuard::new(Arc::clone(&self.files), attempt.clone());
        let claimed = self
            .claim_or_fallback(&repo, &config, &root, &attempt, context)
            .await?;
        let result = async {
            let fresh = if claimed {
                self.pool
                    .is_fresh_copy(&attempt, &repo, &config)
                    .await
                    .unwrap_or(false)
            } else {
                false
            };
            context.progress(format!("fetching pull request #{number}"))?;
            self.git.fetch_pull_request(&attempt, number).await?;
            let pull_ref = format!("refs/swarm/pulls/{number}/head");
            self.git
                .checkout_force_branch(&attempt, &branch, &pull_ref)
                .await?;
            self.files.remove_file(&hot_marker_path(&attempt))?;
            if !fresh {
                self.run_prepare_hooks(&attempt, &repo.hooks, context)
                    .await?;
            }
            let base_ref = format!("pull/{number}/head");
            let worktree = self
                .publish(&repo, &slug, &branch, &base_ref, &repo.hooks, &attempt)
                .await?;
            self.schedule_post_create(worktree.clone(), repo.hooks.post_create.clone());
            Ok((true, worktree))
        }
        .await;
        if result.is_err() && self.files.exists(&attempt) {
            let _ignored = self.files.remove_detached(&attempt);
        }
        attempt_guard.disarm();
        self.pool.refill(repo_id);
        result
    }

    async fn claim_or_fallback(
        &self,
        repo: &Repo,
        config: &Config,
        root: &Path,
        attempt: &Path,
        context: &JobCtx,
    ) -> DaemonResult<bool> {
        let lock = self.jobs.repo_lock(&repo.id);
        let _guard = lock.lock().await;
        check_cancelled(context)?;
        if self
            .pool
            .claim_into_locked(&repo.id, attempt, config, root)?
        {
            context.progress("prepared-copy-claimed")?;
            return Ok(true);
        }
        context.progress("no prepared copy; copying pristine base")?;
        let mut copy = CancellableCopy::start(
            Arc::clone(&self.files),
            PathBuf::from(&repo.path),
            attempt.to_path_buf(),
            None,
        );
        copy.wait().await?;
        copy.disarm();
        Ok(false)
    }

    async fn fetch_attempt_refs(
        &self,
        attempt: &Path,
        repo: &Repo,
        base_ref: &str,
        requested_branch: Option<&str>,
    ) -> DaemonResult<()> {
        let default = branch_refspec(&repo.default_branch);
        let mut mandatory = vec![default];
        if let Some(base_branch) = base_ref.strip_prefix("origin/")
            && base_branch != repo.default_branch
        {
            mandatory.push(branch_refspec(base_branch));
        }
        let mut combined = mandatory.clone();
        if let Some(branch) = requested_branch
            && branch != repo.default_branch
            && base_ref != format!("origin/{branch}")
        {
            combined.push(branch_refspec(branch));
        }
        if let Err(combined_error) = self.git.fetch_refs(attempt, "origin", &combined).await {
            self.git
                .fetch_refs(attempt, "origin", &mandatory)
                .await
                .map_err(|_| combined_error)?;
        }
        if !self
            .git
            .remote_branch_exists(attempt, &repo.default_branch)
            .await?
        {
            return Err(DaemonError::Git(format!(
                "default branch origin/{} is missing",
                repo.default_branch
            )));
        }
        if base_ref != format!("origin/{}", repo.default_branch)
            && !self.git.revision_exists(attempt, base_ref).await?
        {
            return Err(DaemonError::Git(format!(
                "selected base ref {base_ref} is missing"
            )));
        }
        Ok(())
    }

    async fn run_prepare_hooks(
        &self,
        cwd: &Path,
        hooks: &RepoHooks,
        context: &JobCtx,
    ) -> DaemonResult<()> {
        for command in &hooks.prepare {
            check_cancelled(context)?;
            context.progress(format!("prepare: {command}"))?;
            match run_hook(cwd, command, Some(context)).await {
                Ok(output) if output.status.success() => {
                    record_output(context, &output)?;
                }
                Ok(output) => {
                    record_output(context, &output)?;
                    context.progress(format!(
                        "warning: prepare hook exited {}",
                        output.status.code().unwrap_or(-1)
                    ))?;
                }
                Err(DaemonError::Cancelled) => return Err(DaemonError::Cancelled),
                Err(error) => context.progress(format!("warning: prepare hook failed: {error}"))?,
            }
        }
        Ok(())
    }

    async fn publish(
        &self,
        repo: &Repo,
        slug: &str,
        branch: &str,
        base_ref: &str,
        hooks: &RepoHooks,
        attempt: &Path,
    ) -> DaemonResult<Worktree> {
        let id = worktree_id(&repo.id, slug)?;
        let session = SessionId::local(&repo.name, slug)
            .map_err(|error| DaemonError::Validation(error.to_string()))?
            .to_string();
        let canonical = attempt
            .parent()
            .ok_or_else(|| DaemonError::Validation("attempt has no parent".to_owned()))?
            .join(slug);
        let created_at = Utc::now().to_rfc3339();
        let worktree = Worktree {
            id: id.clone(),
            repo_id: repo.id.clone(),
            slug: slug.to_owned(),
            branch: branch.to_owned(),
            base_ref: base_ref.to_owned(),
            path: canonical.to_string_lossy().into_owned(),
            session,
            host: None,
            created_at: created_at.clone(),
            last_opened_at: None,
            degraded: None,
        };
        let intent = CreatingMarker {
            id: id.to_string(),
            repo_id: repo.id.clone(),
            branch: branch.to_owned(),
            base_ref: base_ref.to_owned(),
            created_at,
        };
        let mut intent_text = serde_json::to_string_pretty(&intent)?;
        intent_text.push('\n');

        let published = Arc::new(Mutex::new(false));
        let published_in_transaction = Arc::clone(&published);
        let files = Arc::clone(&self.files);
        let attempt_in_transaction = attempt.to_path_buf();
        let canonical_in_transaction = canonical.clone();
        let marker_in_transaction = creating_marker_path(attempt);
        let id_in_transaction = id.clone();
        let hooks = hooks.clone();
        let worktree_in_transaction = worktree.clone();
        let repo_id = repo.id.clone();
        let transaction = self
            .state
            .transaction(move |state| {
                if state
                    .worktrees
                    .iter()
                    .any(|item| item.id == id_in_transaction)
                {
                    return Err(DaemonError::Conflict(format!(
                        "worktree {id_in_transaction} already exists"
                    )));
                }
                if state.worktrees.iter().any(|item| {
                    item.session == worktree_in_transaction.session
                        || item.path == worktree_in_transaction.path
                }) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree path or session conflicts for {id_in_transaction}"
                    )));
                }
                if files.exists(&canonical_in_transaction) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree destination already exists: {}",
                        canonical_in_transaction.display()
                    )));
                }
                files.atomic_write_text(&marker_in_transaction, &intent_text)?;
                files.rename(&attempt_in_transaction, &canonical_in_transaction)?;
                *published_in_transaction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                let registered_repo = state
                    .repos
                    .iter_mut()
                    .find(|item| item.id == repo_id)
                    .ok_or_else(|| DaemonError::NotFound(format!("repository {repo_id}")))?;
                registered_repo.hooks = hooks;
                state.worktrees.push(worktree_in_transaction.clone());
                Ok(worktree_in_transaction)
            })
            .await;
        match transaction {
            Ok(worktree) => {
                self.files.remove_file(&creating_marker_path(&canonical))?;
                Ok(worktree)
            }
            Err(error) => {
                if *published
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    && self.files.exists(&canonical)
                    && let Ok(trashed) = self.files.trash(&canonical)
                {
                    let _ignored = self.files.remove_detached(&trashed);
                }
                Err(error)
            }
        }
    }

    async fn existing_create_result(
        &self,
        repo: &RepoId,
        slug: &str,
        explicit_branch: Option<&str>,
    ) -> DaemonResult<Option<Worktree>> {
        let id = worktree_id(repo, slug)?;
        let state = self.state.load().await?;
        let Some(existing) = state.worktrees.into_iter().find(|item| item.id == id) else {
            return Ok(None);
        };
        if let Some(branch) = explicit_branch
            && existing.branch != branch
        {
            return Err(DaemonError::Conflict(format!(
                "worktree {id} exists with branch {}, not {branch}",
                existing.branch
            )));
        }
        Ok(Some(existing))
    }

    async fn assert_create_conflicts(
        &self,
        id: &WorktreeId,
        repo: &Repo,
        slug: &str,
        branch: &str,
        root: &Path,
    ) -> DaemonResult<()> {
        let canonical = root.join(slug);
        let session = SessionId::local(&repo.name, slug)
            .map_err(|error| DaemonError::Validation(error.to_string()))?
            .to_string();
        let state = self.state.load().await?;
        if let Some(existing) = state.worktrees.iter().find(|item| item.id == *id) {
            if existing.branch == branch {
                return Err(DaemonError::Conflict(format!(
                    "worktree {id} already exists"
                )));
            }
            return Err(DaemonError::Conflict(format!(
                "worktree {id} exists with branch {}",
                existing.branch
            )));
        }
        if state
            .worktrees
            .iter()
            .any(|item| item.session == session || item.path == canonical.to_string_lossy())
        {
            return Err(DaemonError::Conflict(format!(
                "worktree path or session conflicts for {id}"
            )));
        }
        if self.files.exists(&canonical) {
            return Err(DaemonError::Conflict(format!(
                "worktree destination already exists: {}",
                canonical.display()
            )));
        }
        Ok(())
    }

    async fn repo_and_config(&self, id: &RepoId) -> DaemonResult<(Repo, Config)> {
        let config = self.config.load().await?;
        let state = self.state.load().await?;
        let repo = state
            .repos
            .into_iter()
            .find(|repo| &repo.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("repository {id}")))?;
        Ok((repo, config))
    }

    async fn reclaim_publish_intent(
        &self,
        expected_id: &WorktreeId,
        canonical: &Path,
    ) -> DaemonResult<()> {
        if !self.files.exists(canonical) {
            return Ok(());
        }
        let marker_path = creating_marker_path(canonical);
        let marker_text = self.files.read_text(&marker_path).map_err(|_| {
            DaemonError::Conflict(format!(
                "worktree destination exists without publish intent: {}",
                canonical.display()
            ))
        })?;
        let marker: CreatingMarker = serde_json::from_str(&marker_text).map_err(|_| {
            DaemonError::Conflict(format!(
                "worktree destination has invalid publish intent: {}",
                canonical.display()
            ))
        })?;
        if marker.id != expected_id.to_string()
            || marker.worktree_id(expected_id.slug()).ok().as_ref() != Some(expected_id)
        {
            return Err(DaemonError::Conflict(format!(
                "mismatched publish intent at {}",
                canonical.display()
            )));
        }
        let trashed = self.files.trash(canonical)?;
        self.files.remove_detached(&trashed)
    }

    async fn delete_one_job(&self, id: WorktreeId) -> DaemonResult<()> {
        let service = self.clone();
        let (sender, receiver) = oneshot::channel();
        self.jobs.submit(
            JobKind::DeleteWorktree,
            format!("{}:{}", id, Uuid::new_v4()),
            format!("Delete {id}"),
            false,
            false,
            move |context| async move {
                let result = service.delete_one(id, &context).await;
                let copied = match &result {
                    Ok(()) => Ok(()),
                    Err(error) => Err(clone_daemon_error(error)),
                };
                let _ignored = sender.send(copied);
                result
            },
        );
        receiver.await.map_err(|_| DaemonError::Cancelled)?
    }

    async fn delete_one(&self, id: WorktreeId, context: &JobCtx) -> DaemonResult<()> {
        let config = self.config.load().await?;
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        let canonical = repo_worktrees_dir(&config, &worktree.repo_id).join(&worktree.slug);
        if Path::new(&worktree.path) != canonical {
            return Err(DaemonError::Validation(format!(
                "worktree {id} path is not canonical"
            )));
        }
        if !self.files.exists(&canonical) {
            return Err(DaemonError::NotFound(format!(
                "worktree path {}",
                canonical.display()
            )));
        }
        context.progress("moving worktree to trash")?;
        let marker = TrashMarker {
            original_path: worktree.path.clone(),
            worktree: worktree.clone(),
        };
        let mut marker_text = serde_json::to_string_pretty(&marker)?;
        marker_text.push('\n');
        let moved = Arc::new(Mutex::new(None::<PathBuf>));
        let moved_in_transaction = Arc::clone(&moved);
        let files = Arc::clone(&self.files);
        let canonical_in_transaction = canonical.clone();
        let id_in_transaction = id.clone();
        let transaction = self
            .state
            .transaction(move |state| {
                if !state
                    .worktrees
                    .iter()
                    .any(|item| item.id == id_in_transaction)
                {
                    return Err(DaemonError::NotFound(format!(
                        "worktree {id_in_transaction}"
                    )));
                }
                files.atomic_write_text(
                    &trash_marker_path(&canonical_in_transaction),
                    &marker_text,
                )?;
                let trash = files.trash(&canonical_in_transaction)?;
                *moved_in_transaction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(trash);
                state.worktrees.retain(|item| item.id != id_in_transaction);
                Ok(())
            })
            .await;
        if let Err(error) = transaction {
            if let Some(trash) = moved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                && self.files.exists(&trash)
            {
                let _ignored = self.files.rename(&trash, &canonical);
                let _ignored = self.files.remove_file(&trash_marker_path(&canonical));
            }
            return Err(error);
        }
        let trash = moved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| DaemonError::Filesystem {
                path: canonical,
                source: std::io::Error::other("trash rename did not return a destination"),
            })?;
        self.schedule_trash_cleanup(trash, config.trash.retention_ms);
        Ok(())
    }

    fn schedule_trash_cleanup(&self, trash: PathBuf, retention_ms: u64) {
        let Some(entry) = trash
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            return;
        };
        let files = Arc::clone(&self.files);
        let jobs = Arc::clone(&self.trash_jobs);
        let entry_for_job = entry.clone();
        let id = self.jobs.submit(
            JobKind::Custom("trash_cleanup".to_owned()),
            entry.clone(),
            format!("Remove trash entry {entry}"),
            true,
            false,
            move |_context| async move {
                tokio::time::sleep(Duration::from_millis(retention_ms)).await;
                if files.exists(&trash) {
                    files.remove_detached(&trash)?;
                }
                jobs.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&entry_for_job);
                Ok(())
            },
        );
        self.trash_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(entry, id);
    }

    fn schedule_post_create(&self, worktree: Worktree, hooks: Vec<String>) {
        if hooks.is_empty() {
            return;
        }
        let service = self.clone();
        let target = worktree.id.to_string();
        self.jobs.submit(
            JobKind::PostCreateHooks,
            target,
            format!("Run hooks for {}", worktree.id),
            false,
            true,
            move |context| async move {
                let mut first_failure = None;
                for (index, command) in hooks.iter().enumerate() {
                    context.progress(format!("post-create: {command}"))?;
                    match run_hook(Path::new(&worktree.path), command, None).await {
                        Ok(output) => {
                            record_output(&context, &output)?;
                            if !output.status.success() && first_failure.is_none() {
                                first_failure = Some((
                                    format!("hook {}: {command}", index + 1),
                                    output.status.code(),
                                ));
                            }
                        }
                        Err(error) => {
                            context
                                .progress(format!("warning: post-create hook failed: {error}"))?;
                            if first_failure.is_none() {
                                first_failure =
                                    Some((format!("hook {}: {command}", index + 1), None));
                            }
                        }
                    }
                }
                if let Some((step, exit_code)) = first_failure {
                    let id = worktree.id.clone();
                    let degraded = Degraded {
                        kind: "post_create_hooks".to_owned(),
                        step,
                        exit_code,
                        at: Utc::now().to_rfc3339(),
                        log_path: service
                            .jobs
                            .log_path(&context.id)
                            .to_string_lossy()
                            .into_owned(),
                    };
                    service
                        .state
                        .transaction(move |state| {
                            let item = state
                                .worktrees
                                .iter_mut()
                                .find(|item| item.id == id)
                                .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
                            item.degraded = Some(degraded);
                            Ok(())
                        })
                        .await?;
                    return Err(DaemonError::Shell(
                        "one or more post-create hooks failed".to_owned(),
                    ));
                }
                Ok(())
            },
        );
    }

    fn schedule_startup_recovery(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.startup_ready.store(true, Ordering::Release);
            return;
        }
        let service = self.clone();
        self.jobs.submit(
            JobKind::Custom("worktree_recovery".to_owned()),
            "startup",
            "Recover interrupted worktree publication",
            false,
            false,
            move |context| async move {
                let result = match context.progress("scanning worktree publish intents") {
                    Ok(()) => service.recover_startup().await,
                    Err(error) => Err(error),
                };
                service.startup_ready.store(true, Ordering::Release);
                service.startup_notify.notify_waiters();
                result
            },
        );
    }

    async fn await_startup_recovery(&self) {
        while !self.startup_ready.load(Ordering::Acquire) {
            let notified = self.startup_notify.notified();
            if self.startup_ready.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }

    async fn recover_attempt(
        &self,
        config: &Config,
        repo: &Repo,
        slug: &str,
        attempt: &Path,
    ) -> DaemonResult<()> {
        let marker = self.read_creating_marker(attempt);
        let Ok(marker) = marker else {
            return self.trash_attempt(attempt);
        };
        if !self
            .valid_recovery_marker(repo, slug, attempt, &marker)
            .await
        {
            return self.trash_attempt(attempt);
        }
        let canonical = repo_worktrees_dir(config, &repo.id).join(slug);
        if self.files.exists(&canonical) {
            return self.trash_attempt(attempt);
        }
        self.files.rename(attempt, &canonical)?;
        if let Err(error) = self
            .register_recovered(repo, slug, &canonical, marker)
            .await
        {
            let trashed = self.files.trash(&canonical)?;
            self.files.remove_detached(&trashed)?;
            return Err(error);
        }
        Ok(())
    }

    async fn recover_published(
        &self,
        _config: &Config,
        repo: &Repo,
        slug: &str,
        path: &Path,
    ) -> DaemonResult<()> {
        let marker = match self.read_creating_marker(path) {
            Ok(marker) => marker,
            Err(_) => return Ok(()),
        };
        let state = self.state.load().await?;
        if state
            .worktrees
            .iter()
            .any(|item| item.path == path.to_string_lossy())
        {
            self.files.remove_file(&creating_marker_path(path))?;
            return Ok(());
        }
        let expected_id = worktree_id(&repo.id, slug)?;
        if state.worktrees.iter().any(|item| item.id == expected_id) {
            return self.trash_attempt(path);
        }
        if !self.valid_recovery_marker(repo, slug, path, &marker).await {
            return self.trash_attempt(path);
        }
        self.register_recovered(repo, slug, path, marker).await
    }

    async fn valid_recovery_marker(
        &self,
        repo: &Repo,
        slug: &str,
        path: &Path,
        marker: &CreatingMarker,
    ) -> bool {
        let expected_id = worktree_id(&repo.id, slug).ok();
        if marker.repo_id != repo.id
            || expected_id
                .as_ref()
                .is_none_or(|id| marker.id != id.to_string())
            || marker.base_ref.is_empty()
            || marker.branch.is_empty()
        {
            return false;
        }
        if marker.worktree_id(slug).is_err() {
            return false;
        }
        self.git
            .current_branch(path)
            .await
            .is_ok_and(|branch| branch == marker.branch)
    }

    async fn register_recovered(
        &self,
        repo: &Repo,
        slug: &str,
        path: &Path,
        marker: CreatingMarker,
    ) -> DaemonResult<()> {
        let worktree = Worktree {
            id: marker
                .worktree_id(slug)
                .map_err(|error| DaemonError::Validation(error.to_string()))?,
            repo_id: repo.id.clone(),
            slug: slug.to_owned(),
            branch: marker.branch,
            base_ref: marker.base_ref,
            path: path.to_string_lossy().into_owned(),
            session: SessionId::local(&repo.name, slug)
                .map_err(|error| DaemonError::Validation(error.to_string()))?
                .to_string(),
            host: None,
            created_at: marker.created_at,
            last_opened_at: None,
            degraded: None,
        };
        let id = worktree.id.clone();
        self.state
            .transaction(move |state| {
                if state.worktrees.iter().any(|item| {
                    item.id == id || item.path == worktree.path || item.session == worktree.session
                }) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree {id} conflicts with registered state"
                    )));
                }
                state.worktrees.push(worktree);
                Ok(())
            })
            .await?;
        self.files.remove_file(&creating_marker_path(path))
    }

    fn read_creating_marker(&self, path: &Path) -> DaemonResult<CreatingMarker> {
        Ok(serde_json::from_str(
            &self.files.read_text(&creating_marker_path(path))?,
        )?)
    }

    fn trash_attempt(&self, path: &Path) -> DaemonResult<()> {
        if !self.files.exists(path) {
            return Ok(());
        }
        let trashed = self.files.trash(path)?;
        self.files.remove_detached(&trashed)
    }

    fn trash_dir(&self) -> PathBuf {
        self.state
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("trash")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrashMarker {
    original_path: String,
    worktree: Worktree,
}

struct AttemptGuard {
    files: Arc<dyn Files>,
    path: PathBuf,
    armed: bool,
}

impl AttemptGuard {
    fn new(files: Arc<dyn Files>, path: PathBuf) -> Self {
        Self {
            files,
            path,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.armed || !self.files.exists(&self.path) {
            return;
        }
        let Some(parent) = self.path.parent() else {
            return;
        };
        let discarded = parent.join(format!(".discard-{}", Uuid::new_v4()));
        if self.files.rename(&self.path, &discarded).is_ok() {
            let _ignored = self.files.remove_detached(&discarded);
        }
    }
}

fn trash_marker_path(root: &Path) -> PathBuf {
    root.join(".git").join(TRASH_MARKER_FILE)
}

fn validate_restore_marker(marker: &TrashMarker, config: &Config) -> DaemonResult<()> {
    if marker.worktree.host.is_some() {
        return Err(remote_unsupported());
    }
    let expected = repo_worktrees_dir(config, &marker.worktree.repo_id).join(&marker.worktree.slug);
    if Path::new(&marker.original_path) != expected || marker.worktree.path != marker.original_path
    {
        return Err(DaemonError::Validation(
            "trash entry does not contain a canonical worktree path".to_owned(),
        ));
    }
    Ok(())
}

fn validate_trash_entry(entry: &str) -> DaemonResult<()> {
    if entry.is_empty()
        || matches!(entry, "." | "..")
        || Path::new(entry).file_name().and_then(|name| name.to_str()) != Some(entry)
    {
        return Err(DaemonError::Validation(
            "trash entry must be one directory name".to_owned(),
        ));
    }
    Ok(())
}

fn repo_worktrees_dir(config: &Config, repo: &RepoId) -> PathBuf {
    Path::new(&config.worktrees_dir)
        .join(repo.owner())
        .join(repo.name())
}

fn worktree_id(repo: &RepoId, slug: &str) -> DaemonResult<WorktreeId> {
    WorktreeId::try_from(format!("{repo}#{slug}"))
        .map_err(|error| DaemonError::Validation(error.to_string()))
}

fn branch_refspec(branch: &str) -> String {
    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
}

fn remote_unsupported() -> DaemonError {
    DaemonError::Unsupported("remote hosts are not supported yet".to_owned())
}

fn check_cancelled(context: &JobCtx) -> DaemonResult<()> {
    if context.cancel.is_cancelled() {
        Err(DaemonError::Cancelled)
    } else {
        Ok(())
    }
}

async fn run_hook(
    cwd: &Path,
    command: &str,
    context: Option<&JobCtx>,
) -> DaemonResult<std::process::Output> {
    let mut process = tokio::process::Command::new("sh");
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = process.output();
    if let Some(context) = context {
        tokio::select! {
            () = context.cancel.cancelled() => Err(DaemonError::Cancelled),
            output = output => output.map_err(|error| DaemonError::Shell(format!("sh -c `{command}`: {error}"))),
        }
    } else {
        output
            .await
            .map_err(|error| DaemonError::Shell(format!("sh -c `{command}`: {error}")))
    }
}

fn record_output(context: &JobCtx, output: &std::process::Output) -> DaemonResult<()> {
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        context.progress(line)?;
    }
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        context.progress(line)?;
    }
    Ok(())
}

fn clone_daemon_error(error: &DaemonError) -> DaemonError {
    match error {
        DaemonError::NotFound(value) => DaemonError::NotFound(value.clone()),
        DaemonError::Conflict(value) => DaemonError::Conflict(value.clone()),
        DaemonError::Validation(value) => DaemonError::Validation(value.clone()),
        DaemonError::Filesystem { path, source } => {
            DaemonError::fs(path, std::io::Error::new(source.kind(), source.to_string()))
        }
        DaemonError::Json(value) => DaemonError::Validation(value.to_string()),
        DaemonError::Shell(value) => DaemonError::Shell(value.clone()),
        DaemonError::Git(value) => DaemonError::Git(value.clone()),
        DaemonError::Github(value) => DaemonError::Github(value.clone()),
        DaemonError::Process(value) => DaemonError::Process(value.clone()),
        DaemonError::Timeout(value) => DaemonError::Timeout(value.clone()),
        DaemonError::Cancelled => DaemonError::Cancelled,
        DaemonError::Protocol(value) => DaemonError::Protocol(value.clone()),
        DaemonError::Unimplemented(value) => DaemonError::Unimplemented(value),
        DaemonError::Unsupported(value) => DaemonError::Unsupported(value.clone()),
        DaemonError::Join(value) => DaemonError::Join(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_as_trash_entry_names() {
        assert!(validate_trash_entry("123-feature").is_ok());
        assert!(validate_trash_entry("../123-feature").is_err());
        assert!(validate_trash_entry(".").is_err());
    }
}
