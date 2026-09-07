//! Prepared-copy pool scheduling, refresh, and claiming.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use fleet_core::{
    config::Config,
    ids::{JobId, RepoId},
    model::{Repo, RepoHooks},
    paths::{HotMarker, hot_marker_path, slot_path},
};
use fleet_proto::job::JobKind;
use sha2::{Digest, Sha256};
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{files::Files, git::Git, shell::Shell},
    jobs::{JobCtx, JobManager, JobPolicy},
    stores::{config::ConfigStore, state::StateStore},
};

use super::{branch_refspec, check_cancelled, discard_path, hooks, repo_worktrees_dir};

/// Prepared-copy pool service serialized by per-repository job-manager locks.
#[derive(Clone)]
pub struct Pool {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    git: Arc<dyn Git>,
    files: Arc<dyn Files>,
    shell: Arc<dyn Shell>,
    in_flight: Arc<Mutex<HashMap<RepoId, PrepareFlights>>>,
}

#[derive(Default)]
struct PrepareFlights {
    ordinary: Option<JobId>,
    forced: Option<JobId>,
}

impl Pool {
    /// Creates a pool; periodic maintenance belongs to `Services`.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        files: Arc<dyn Files>,
        shell: Arc<dyn Shell>,
    ) -> Self {
        Self {
            config,
            state,
            jobs,
            git,
            files,
            shell,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Ensures configured slots exist. `force` refreshes every existing slot first.
    pub async fn prepare(&self, repo: RepoId, force: bool) -> DaemonResult<()> {
        self.jobs.ensure_repo_available(&repo)?;
        let id = self.submit_prepare(repo, force)?;
        match self.jobs.wait(&id).await?.status {
            fleet_proto::job::JobStatus::Succeeded => Ok(()),
            fleet_proto::job::JobStatus::Cancelled => Err(DaemonError::Cancelled),
            fleet_proto::job::JobStatus::Failed { error } => Err(DaemonError::Shell(error)),
            fleet_proto::job::JobStatus::Queued
            | fleet_proto::job::JobStatus::Running
            | fleet_proto::job::JobStatus::Cancelling => Err(DaemonError::Cancelled),
        }
    }

    /// Claims the lowest ready slot into a private path and queues its replacement.
    pub async fn claim(&self, repo: RepoId) -> DaemonResult<Option<PathBuf>> {
        let config = self.config.load().await?;
        let root = repo_worktrees_dir(&config, &repo);
        self.files.create_dir_all(&root)?;
        let destination = root.join(format!(".claimed-{}", Uuid::new_v4()));
        let claimed = self.claim_into(&repo, &destination).await?;
        if claimed {
            let _ignored = self.submit_prepare(repo, false);
            Ok(Some(destination))
        } else {
            Ok(None)
        }
    }

    /// Moves the lowest numbered ready slot to `destination` while holding the repo mutex.
    pub(crate) async fn claim_into(&self, repo: &RepoId, destination: &Path) -> DaemonResult<bool> {
        let config = self.config.load().await?;
        let root = repo_worktrees_dir(&config, repo);
        let lock = self.jobs.repo_lock(repo);
        let _guard = lock.lock().await;
        self.claim_into_locked(destination, &config, &root)
    }

    pub(crate) fn claim_into_locked(
        &self,
        destination: &Path,
        config: &Config,
        root: &Path,
    ) -> DaemonResult<bool> {
        for slot in 0..usize_from_u64(config.hot_pool_size) {
            let source = slot_path(root, slot);
            if !self.files.exists(&source) {
                continue;
            }
            match self.files.rename(&source, destination) {
                Ok(()) => {
                    return Ok(true);
                }
                Err(DaemonError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }

        Ok(false)
    }

    /// Queues one immediate replacement after a successful prepared-copy claim.
    pub(crate) fn refill(&self, repo: RepoId) {
        if self.jobs.ensure_repo_available(&repo).is_ok() {
            let _ignored = self.submit_prepare(repo, false);
        }
    }

    pub(crate) async fn is_fresh_copy(
        &self,
        copy: &Path,
        repo: &Repo,
        config: &Config,
    ) -> DaemonResult<bool> {
        let fingerprint = prepare_fingerprint(&repo.hooks)?;
        self.slot_is_fresh(copy, repo, config, &fingerprint).await
    }

    fn submit_prepare(&self, repo: RepoId, force: bool) -> DaemonResult<JobId> {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let flights = in_flight.entry(repo.clone()).or_default();
        flights.ordinary = active_job(&self.jobs, flights.ordinary.take());
        flights.forced = active_job(&self.jobs, flights.forced.take());
        if force {
            if let Some(id) = &flights.forced {
                return Ok(id.clone());
            }
        } else if let Some(id) = flights.forced.as_ref().or(flights.ordinary.as_ref()) {
            return Ok(id.clone());
        }
        let service = self.clone();
        let target = repo.to_string();
        let repo_for_job = repo.clone();
        let title = if force {
            format!("Refresh prepared copies for {repo}")
        } else {
            format!("Prepare copies for {repo}")
        };
        let id = self.jobs.submit_for_repo(
            repo.clone(),
            if force {
                JobKind::PoolRefresh
            } else {
                JobKind::PoolBuild
            },
            target,
            title,
            JobPolicy::new(true, true),
            move |context| async move { service.prepare_job(&repo_for_job, force, &context).await },
        )?;
        if force {
            flights.forced = Some(id.clone());
        } else {
            flights.ordinary = Some(id.clone());
        }
        Ok(id)
    }

    async fn prepare_job(
        &self,
        repo_id: &RepoId,
        force: bool,
        context: &JobCtx,
    ) -> DaemonResult<()> {
        self.jobs.ensure_repo_available(repo_id)?;
        let _permit = self
            .jobs
            .pool_semaphore()
            .acquire_owned()
            .await
            .map_err(|_| DaemonError::Cancelled)?;
        let lock = self.jobs.repo_lock(repo_id);
        let mut coordination = Some(lock.lock_owned().await);
        self.jobs.ensure_repo_available(repo_id)?;
        check_cancelled(context)?;

        let config = self.config.load().await?;
        let state = self.state.load().await?;
        let repo = state
            .repos
            .iter()
            .find(|repo| &repo.id == repo_id)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(format!("repository {repo_id}")))?;
        let root = repo_worktrees_dir(&config, repo_id);
        self.files.create_dir_all(&root)?;
        self.cleanup_artifacts(&root, config.hot_pool_size)?;

        if config.hot_pool_size == 0 {
            return Ok(());
        }

        let fingerprint = prepare_fingerprint(&repo.hooks)?;
        let existing = self.existing_slots(&root, config.hot_pool_size);
        let all_fresh = !existing.is_empty()
            && self
                .all_fresh(&existing, &repo, &config, &fingerprint)
                .await?;

        if force || !all_fresh {
            context.progress(format!("refreshing pristine base for {repo_id}"))?;
            self.refresh_base(&repo, context).await?;
        } else {
            context.progress("prepared copies are fresh; skipping fetch")?;
        }

        for slot in existing {
            check_cancelled(context)?;
            let stale = !force
                && !self
                    .slot_is_fresh(&slot_path(&root, slot), &repo, &config, &fingerprint)
                    .await?;
            if force || stale {
                self.refresh_slot(&repo, &root, slot, &fingerprint, context, &mut coordination)
                    .await?;
            }
        }

        for slot in 0..usize_from_u64(config.hot_pool_size) {
            check_cancelled(context)?;
            if !self.files.exists(&slot_path(&root, slot)) {
                self.build_slot(&repo, &root, slot, &fingerprint, context, &mut coordination)
                    .await?;
            }
        }
        self.cleanup_artifacts(&root, config.hot_pool_size)?;

        Ok(())
    }

    async fn refresh_base(&self, repo: &Repo, context: &JobCtx) -> DaemonResult<()> {
        let path = Path::new(&repo.path);
        self.git.fetch(path, true).await?;
        check_cancelled(context)?;
        if !self
            .git
            .remote_branch_exists(path, &repo.default_branch)
            .await?
        {
            return Err(DaemonError::Git(format!(
                "default branch origin/{} is missing for {}",
                repo.default_branch, repo.id
            )));
        }
        self.reset_to_default_branch(path, &repo.default_branch)
            .await
    }

    /// Returns a checkout to a clean `origin/<default>`: switch branches when it drifted,
    /// hard-reset when it is behind or dirty, and clean whatever the reset left behind.
    async fn reset_to_default_branch(&self, path: &Path, default_branch: &str) -> DaemonResult<()> {
        let current = self.git.current_branch(path).await?;
        let head = self.git.revision(path, "HEAD").await?;
        let remote = self
            .git
            .revision(path, &format!("origin/{default_branch}"))
            .await?;
        let dirty = !self.git.status_porcelain(path).await?.is_empty();
        if current != default_branch {
            self.git.checkout_reset(path, default_branch).await?;
        } else if head != remote || dirty {
            self.git.hard_reset(path, default_branch).await?;
        }
        if dirty {
            self.git.clean(path).await?;
        }
        Ok(())
    }

    async fn refresh_slot(
        &self,
        repo: &Repo,
        root: &Path,
        slot: usize,
        fingerprint: &str,
        context: &JobCtx,
        coordination: &mut Option<OwnedMutexGuard<()>>,
    ) -> DaemonResult<()> {
        let path = slot_path(root, slot);
        let old = self.read_hot_marker(&path).ok();
        if old
            .as_ref()
            .is_some_and(|marker| marker.prepare_fingerprint != fingerprint)
        {
            context.progress(format!("rebuilding slot {slot} after prepare hook change"))?;
            discard_path(self.files.as_ref(), &path)?;
            return self
                .build_slot(repo, root, slot, fingerprint, context, coordination)
                .await;
        }

        context.progress(format!("refreshing prepared slot {slot}"))?;
        self.files.remove_file(&hot_marker_path(&path))?;
        self.refresh_copy(&path, repo, None).await?;
        hooks::run_prepare(self.shell.as_ref(), &path, &repo.hooks, context).await?;
        self.write_hot_marker(&path, repo, fingerprint).await?;
        Ok(())
    }

    async fn build_slot(
        &self,
        repo: &Repo,
        root: &Path,
        slot: usize,
        fingerprint: &str,
        context: &JobCtx,
        coordination: &mut Option<OwnedMutexGuard<()>>,
    ) -> DaemonResult<()> {
        let (staging, pid_path) = copy_attempt_paths(root, slot);
        let final_path = slot_path(root, slot);
        self.files
            .atomic_write_text(&pid_path, &format!("{}\n", std::process::id()))?;
        context.progress(format!("copying prepared slot {slot}"))?;
        let mut copy = CancellableCopy::start_coordinated(
            Arc::clone(&self.files),
            PathBuf::from(&repo.path),
            staging.clone(),
            Some(pid_path.clone()),
            coordination.take().ok_or_else(|| {
                DaemonError::Conflict("prepared-copy coordination is missing".to_owned())
            })?,
            Some(context.clone()),
        );
        let copy_result = copy.wait().await;
        *coordination = copy.take_coordination();
        copy_result?;

        let result = async {
            check_cancelled(context)?;
            hooks::run_prepare(self.shell.as_ref(), &staging, &repo.hooks, context).await?;
            self.write_hot_marker(&staging, repo, fingerprint).await?;
            if self.files.exists(&final_path) {
                return Err(DaemonError::Conflict(format!(
                    "prepared slot already exists: {}",
                    final_path.display()
                )));
            }
            self.files.rename(&staging, &final_path)
        }
        .await;
        let _ignored = self.files.remove_file(&pid_path);
        if result.is_err() && self.files.exists(&staging) {
            let _ignored = discard_path(self.files.as_ref(), &staging);
        }
        copy.disarm();
        result
    }

    async fn refresh_copy(
        &self,
        path: &Path,
        repo: &Repo,
        requested_branch: Option<&str>,
    ) -> DaemonResult<()> {
        let default_ref = branch_refspec(&repo.default_branch);
        let mut refs = vec![default_ref.clone()];
        if let Some(branch) = requested_branch
            && branch != repo.default_branch
        {
            refs.push(branch_refspec(branch));
        }
        if let Err(combined_error) = self.git.fetch_refs(path, "origin", &refs).await {
            if requested_branch.is_some() {
                self.git
                    .fetch_refs(path, "origin", std::slice::from_ref(&default_ref))
                    .await
                    .map_err(|_| combined_error)?;
            } else {
                self.git
                    .fetch(path, true)
                    .await
                    .map_err(|_| combined_error)?;
            }
        }
        if !self
            .git
            .remote_branch_exists(path, &repo.default_branch)
            .await?
        {
            return Err(DaemonError::Git(format!(
                "default branch origin/{} is missing",
                repo.default_branch
            )));
        }
        self.reset_to_default_branch(path, &repo.default_branch)
            .await
    }

    async fn write_hot_marker(
        &self,
        copy: &Path,
        repo: &Repo,
        fingerprint: &str,
    ) -> DaemonResult<()> {
        let sha = self
            .git
            .revision(copy, &format!("origin/{}", repo.default_branch))
            .await?;
        if !(40..=64).contains(&sha.len()) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(DaemonError::Git(format!(
                "invalid origin SHA for {}: {sha}",
                repo.id
            )));
        }
        let marker = HotMarker {
            fetched_at: Utc::now().to_rfc3339(),
            default_branch: repo.default_branch.clone(),
            sha,
            prepare_fingerprint: fingerprint.to_owned(),
        };
        let mut text = serde_json::to_string_pretty(&marker)?;
        text.push('\n');
        self.files.atomic_write_text(&hot_marker_path(copy), &text)
    }

    async fn all_fresh(
        &self,
        slots: &[usize],
        repo: &Repo,
        config: &Config,
        fingerprint: &str,
    ) -> DaemonResult<bool> {
        for slot in slots {
            let root = repo_worktrees_dir(config, &repo.id);
            if !self
                .slot_is_fresh(&slot_path(root, *slot), repo, config, fingerprint)
                .await?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn slot_is_fresh(
        &self,
        slot: &Path,
        repo: &Repo,
        config: &Config,
        fingerprint: &str,
    ) -> DaemonResult<bool> {
        let marker = match self.read_hot_marker(slot) {
            Ok(marker) => marker,
            Err(_) => return Ok(false),
        };
        let fetched = match DateTime::parse_from_rfc3339(&marker.fetched_at) {
            Ok(value) => value.with_timezone(&Utc),
            Err(_) => return Ok(false),
        };
        let age = Utc::now().signed_duration_since(fetched).num_milliseconds();
        if age < 0
            || u64::try_from(age).unwrap_or(u64::MAX) >= config.hot_freshness_ms
            || marker.prepare_fingerprint != fingerprint
            || marker.default_branch != repo.default_branch
            || !self
                .git
                .remote_branch_exists(slot, &repo.default_branch)
                .await?
        {
            return Ok(false);
        }
        let sha = self
            .git
            .revision(slot, &format!("origin/{}", repo.default_branch))
            .await?;
        Ok(sha == marker.sha)
    }

    fn read_hot_marker(&self, slot: &Path) -> DaemonResult<HotMarker> {
        let path = hot_marker_path(slot);
        let marker: HotMarker = serde_json::from_str(&self.files.read_text(&path)?)?;
        if !(40..=64).contains(&marker.sha.len())
            || !marker.sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            || marker.prepare_fingerprint.len() != 64
            || !marker
                .prepare_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DaemonError::Validation(format!(
                "invalid prepared-copy marker at {}",
                path.display()
            )));
        }
        Ok(marker)
    }

    fn existing_slots(&self, root: &Path, size: u64) -> Vec<usize> {
        (0..usize_from_u64(size))
            .filter(|slot| self.files.exists(&slot_path(root, *slot)))
            .collect()
    }

    fn cleanup_artifacts(&self, root: &Path, size: u64) -> DaemonResult<()> {
        let configured = usize_from_u64(size);
        for entry in self.files.list(root)? {
            let Some(name) = entry.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some((slot, kind)) = parse_pool_entry(name) else {
                continue;
            };
            if slot >= configured || kind != PoolEntryKind::Slot {
                if kind == PoolEntryKind::Pid {
                    self.files.remove_file(&entry)?;
                } else if self.files.exists(&entry) {
                    discard_path(self.files.as_ref(), &entry)?;
                }
            }
        }
        Ok(())
    }
}

fn prepare_fingerprint(hooks: &RepoHooks) -> DaemonResult<String> {
    let json = serde_json::to_string(&hooks.prepare)?;
    Ok(format!("{:x}", Sha256::digest(json.as_bytes())))
}

fn usize_from_u64(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn active_job(jobs: &JobManager, id: Option<JobId>) -> Option<JobId> {
    id.filter(|id| {
        jobs.record(id).is_some_and(|record| {
            matches!(
                record.status,
                fleet_proto::job::JobStatus::Queued
                    | fleet_proto::job::JobStatus::Running
                    | fleet_proto::job::JobStatus::Cancelling
            )
        })
    })
}

fn copy_attempt_paths(root: &Path, slot: usize) -> (PathBuf, PathBuf) {
    let slot = slot_path(root, slot);
    let name = slot
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(".hot");
    let staging = root.join(format!("{name}.staging-{}", Uuid::new_v4()));
    let pid = root.join(format!(
        "{}.pid",
        staging
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(".hot.staging")
    ));
    (staging, pid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolEntryKind {
    Slot,
    Staging,
    Pid,
}

fn parse_pool_entry(name: &str) -> Option<(usize, PoolEntryKind)> {
    let (slot_name, kind) = if let Some(value) = name
        .strip_suffix(".pid")
        .and_then(|value| value.split_once(".staging-").map(|(slot, _attempt)| slot))
    {
        (value, PoolEntryKind::Pid)
    } else if let Some((value, _attempt)) = name.split_once(".staging-") {
        (value, PoolEntryKind::Staging)
    } else if let Some(value) = name.strip_suffix(".staging.pid") {
        (value, PoolEntryKind::Pid)
    } else if let Some(value) = name.strip_suffix(".staging") {
        (value, PoolEntryKind::Staging)
    } else {
        (name, PoolEntryKind::Slot)
    };
    if slot_name == ".hot" {
        Some((0, kind))
    } else {
        slot_name
            .strip_prefix(".hot.")?
            .parse::<usize>()
            .ok()
            .map(|slot| (slot, kind))
    }
}

pub(crate) struct CancellableCopy {
    task: Option<tokio::task::JoinHandle<DaemonResult<()>>>,
    files: Arc<dyn Files>,
    destination: PathBuf,
    pid_path: Option<PathBuf>,
    coordination: Option<OwnedMutexGuard<()>>,
    context: Option<JobCtx>,
    cleanup_on_drop: bool,
}

impl CancellableCopy {
    pub(crate) fn start_coordinated(
        files: Arc<dyn Files>,
        source: PathBuf,
        destination: PathBuf,
        pid_path: Option<PathBuf>,
        coordination: OwnedMutexGuard<()>,
        context: Option<JobCtx>,
    ) -> Self {
        let mut copy = Self::start_inner(files, source, destination, pid_path, Some(coordination));
        copy.context = context;
        copy
    }

    fn start_inner(
        files: Arc<dyn Files>,
        source: PathBuf,
        destination: PathBuf,
        pid_path: Option<PathBuf>,
        coordination: Option<OwnedMutexGuard<()>>,
    ) -> Self {
        let task_files = Arc::clone(&files);
        let task_destination = destination.clone();
        let task =
            tokio::task::spawn_blocking(move || task_files.clone_dir(&source, &task_destination));
        Self {
            task: Some(task),
            files,
            destination,
            pid_path,
            coordination,
            context: None,
            cleanup_on_drop: true,
        }
    }

    pub(crate) async fn wait(&mut self) -> DaemonResult<()> {
        let result = self
            .task
            .as_mut()
            .ok_or_else(|| DaemonError::Join("copy worker is missing".to_owned()))?
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?;
        self.task.take();
        result
    }

    pub(crate) fn disarm(&mut self) {
        self.cleanup_on_drop = false;
    }

    fn take_coordination(&mut self) -> Option<OwnedMutexGuard<()>> {
        self.coordination.take()
    }
}

impl Drop for CancellableCopy {
    fn drop(&mut self) {
        if !self.cleanup_on_drop {
            return;
        }
        let Some(task) = self.task.take() else {
            cleanup_copy_artifact(&self.files, &self.destination, self.pid_path.as_deref());
            return;
        };
        let files = Arc::clone(&self.files);
        let destination = self.destination.clone();
        let pid_path = self.pid_path.clone();
        let coordination = self.coordination.take();
        let cleanup = async move {
            let _result = task.await;
            cleanup_copy_artifact(&files, &destination, pid_path.as_deref());
            drop(coordination);
        };
        if let Some(context) = self.context.take() {
            context.track_cleanup(cleanup);
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(cleanup);
        }
    }
}

fn cleanup_copy_artifact(files: &Arc<dyn Files>, destination: &Path, pid_path: Option<&Path>) {
    if let Some(pid_path) = pid_path {
        let _ignored = files.remove_file(pid_path);
    }
    let _ignored = discard_path(files.as_ref(), destination);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crate::adapters::files::RealFiles;

    use super::*;

    struct BlockingFiles {
        inner: RealFiles,
        started: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl Files for BlockingFiles {
        fn read_text(&self, path: &Path) -> DaemonResult<String> {
            self.inner.read_text(path)
        }

        fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
            self.inner.create_dir_all(path)
        }

        fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
            if let Some(started) = self
                .started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                let _ignored = started.send(());
                self.release
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv()
                    .map_err(|error| DaemonError::Join(error.to_string()))?;
            }
            self.inner.clone_dir(source, destination)
        }

        fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
            self.inner.atomic_write_text(path, text)
        }

        fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
            self.inner.rename(source, destination)
        }

        fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
            self.inner.trash(path)
        }

        fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
            self.inner.remove_detached(path)
        }

        fn remove_file(&self, path: &Path) -> DaemonResult<()> {
            self.inner.remove_file(path)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
            self.inner.list(path)
        }

        fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
            self.inner.guard_strict_descendant(path)
        }

        fn set_removable_roots(&self, roots: Vec<PathBuf>) {
            self.inner.set_removable_roots(roots);
        }
    }

    #[test]
    fn fingerprints_match_sha256_of_json_stringified_hook_array() {
        let hooks = RepoHooks {
            prepare: vec!["one".to_owned(), "two".to_owned()],
            post_create: Vec::new(),
        };
        assert_eq!(
            prepare_fingerprint(&hooks).ok().as_deref(),
            Some("33688af4ac8c7316a90560422a92d5cd66fff0de701f56a962e61b10734ed79c")
        );
    }

    #[test]
    fn parses_slot_artifact_names() {
        assert_eq!(parse_pool_entry(".hot"), Some((0, PoolEntryKind::Slot)));
        assert_eq!(
            parse_pool_entry(".hot.2.staging"),
            Some((2, PoolEntryKind::Staging))
        );
        assert_eq!(
            parse_pool_entry(".hot.staging.pid"),
            Some((0, PoolEntryKind::Pid))
        );
        assert_eq!(parse_pool_entry("feature"), None);
    }

    #[tokio::test]
    async fn cancelled_copy_cannot_delete_later_attempt() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temp.path().join("worktrees/acme/api");
        let source = temp.path().join("repos/acme/api");
        std::fs::create_dir_all(&source).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(source.join("README.md"), "fleet\n")
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let files: Arc<dyn Files> = Arc::new(BlockingFiles {
            inner: RealFiles::new(temp.path().join("trash"), [temp.path().to_path_buf()]),
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(release_rx),
        });
        let coordination = Arc::new(tokio::sync::Mutex::new(()));
        let guard = Arc::clone(&coordination).lock_owned().await;
        let (first_attempt, first_pid) = copy_attempt_paths(&root, 0);
        let (later_attempt, _later_pid) = copy_attempt_paths(&root, 0);
        assert_ne!(first_attempt, later_attempt);
        let copy = CancellableCopy::start_coordinated(
            Arc::clone(&files),
            source,
            first_attempt.clone(),
            Some(first_pid),
            guard,
            None,
        );
        tokio::task::spawn_blocking(move || started_rx.recv())
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));

        drop(copy);
        assert!(coordination.try_lock().is_err());
        release_tx
            .send(())
            .unwrap_or_else(|error| panic!("{error}"));
        let _guard = coordination.lock().await;
        std::fs::create_dir_all(&later_attempt).unwrap_or_else(|error| panic!("{error}"));

        assert!(!first_attempt.exists());
        assert!(later_attempt.exists());
    }
}
