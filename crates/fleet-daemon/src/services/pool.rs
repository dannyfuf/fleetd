//! Prepared-copy pool scheduling, refresh, and claiming.

mod sha256;

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use fleet_core::{
    config::Config,
    ids::RepoId,
    model::{Repo, RepoHooks},
    paths::{HotMarker, hot_marker_path, slot_path, slot_pid_path, slot_staging_path},
};
use fleet_proto::{job::JobKind, snapshot::PoolStatus};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        files::Files,
        git::Git,
        shell::{Shell, ShellCommand, ShellResult},
    },
    jobs::{JobCtx, JobManager},
    stores::{config::ConfigStore, state::StateStore},
};

static STATUS_CACHE: OnceLock<Mutex<HashMap<RepoId, PoolStatus>>> = OnceLock::new();

/// Prepared-copy pool service serialized by per-repository job-manager locks.
#[derive(Clone)]
pub struct Pool {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    git: Arc<dyn Git>,
    files: Arc<dyn Files>,
    shell: Option<Arc<dyn Shell>>,
}

impl Pool {
    /// Creates the prepared-copy pool service and starts startup/periodic maintenance.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        files: Arc<dyn Files>,
    ) -> Self {
        let service = Self::without_background(config, state, jobs, git, files);
        service.start_background_maintenance();
        service
    }

    pub(crate) fn without_background(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            config,
            state,
            jobs,
            git,
            files,
            shell: None,
        }
    }

    /// Uses the daemon's shared command adapter for prepare hooks.
    #[must_use]
    pub fn with_shell(mut self, shell: Arc<dyn Shell>) -> Self {
        self.shell = Some(shell);
        self
    }

    /// Ensures configured slots exist. `force` refreshes every existing slot first.
    pub async fn prepare(&self, repo: RepoId, force: bool) -> DaemonResult<()> {
        let (sender, receiver) = oneshot::channel();
        self.submit_prepare(repo, force, Some(sender));
        receiver.await.map_err(|_| DaemonError::Cancelled)?
    }

    /// Claims the lowest ready slot into a private path and queues its replacement.
    pub async fn claim(&self, repo: RepoId) -> DaemonResult<Option<PathBuf>> {
        let config = self.config.load().await?;
        let root = repo_worktrees_dir(&config, &repo);
        self.files.create_dir_all(&root)?;
        let destination = root.join(format!(".claimed-{}", Uuid::new_v4()));
        let claimed = self.claim_into(&repo, &destination).await?;
        if claimed {
            self.submit_prepare(repo, false, None);
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
        self.claim_into_locked(repo, destination, &config, &root)
    }

    pub(crate) fn claim_into_locked(
        &self,
        repo: &RepoId,
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
                    self.update_status(repo, config, root);
                    return Ok(true);
                }
                Err(DaemonError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        self.update_status(repo, config, root);
        Ok(false)
    }

    /// Queues one immediate replacement after a successful prepared-copy claim.
    pub(crate) fn refill(&self, repo: RepoId) {
        self.submit_prepare(repo, false, None);
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

    /// Summarizes each repository's actual default-layout slots plus observed custom-layout slots.
    #[must_use]
    pub fn snapshot_statuses(repos: &[Repo], configured_size: u64) -> Vec<PoolStatus> {
        let configured = u32::try_from(configured_size).unwrap_or(u32::MAX);
        let cache = STATUS_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        repos
            .iter()
            .map(|repo| {
                if let Some(status) = cache.get(&repo.id) {
                    let mut status = status.clone();
                    status.size = configured;
                    return status;
                }
                scan_default_layout(repo, configured_size).unwrap_or(PoolStatus {
                    repo: repo.id.clone(),
                    ready: 0,
                    size: configured,
                    refreshed_at: None,
                })
            })
            .collect()
    }

    fn submit_prepare(
        &self,
        repo: RepoId,
        force: bool,
        completion: Option<oneshot::Sender<DaemonResult<()>>>,
    ) {
        let completion = completion.map(|sender| Arc::new(Mutex::new(Some(sender))));
        let service = self.clone();
        let target = format!("{}:{}", repo, Uuid::new_v4());
        let title = if force {
            format!("Refresh prepared copies for {repo}")
        } else {
            format!("Prepare copies for {repo}")
        };
        self.jobs.submit(
            if force {
                JobKind::PoolRefresh
            } else {
                JobKind::PoolBuild
            },
            target,
            title,
            true,
            true,
            move |context| async move {
                let result = service.prepare_job(&repo, force, &context).await;
                if let Some(completion) = &completion
                    && let Some(sender) = completion
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                {
                    let copied = match &result {
                        Ok(()) => Ok(()),
                        Err(error) => Err(clone_daemon_error(error)),
                    };
                    let _ignored = sender.send(copied);
                }
                result
            },
        );
    }

    async fn prepare_job(
        &self,
        repo_id: &RepoId,
        force: bool,
        context: &JobCtx,
    ) -> DaemonResult<()> {
        let _permit = self
            .jobs
            .pool_semaphore()
            .acquire_owned()
            .await
            .map_err(|_| DaemonError::Cancelled)?;
        let lock = self.jobs.repo_lock(repo_id);
        let _guard = lock.lock().await;
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
            self.update_status(repo_id, &config, &root);
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
                self.refresh_slot(&repo, &config, &root, slot, &fingerprint, context)
                    .await?;
            }
        }

        for slot in 0..usize_from_u64(config.hot_pool_size) {
            check_cancelled(context)?;
            if !self.files.exists(&slot_path(&root, slot)) {
                self.build_slot(&repo, &root, slot, &fingerprint, context)
                    .await?;
            }
        }
        self.cleanup_artifacts(&root, config.hot_pool_size)?;
        self.update_status(repo_id, &config, &root);
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
        let current = self.git.current_branch(path).await?;
        let head = self.git.revision(path, "HEAD").await?;
        let remote = self
            .git
            .revision(path, &format!("origin/{}", repo.default_branch))
            .await?;
        let dirty = !self.git.status_porcelain(path).await?.is_empty();
        if current != repo.default_branch {
            self.git.checkout_reset(path, &repo.default_branch).await?;
        } else if head != remote || dirty {
            self.git.hard_reset(path, &repo.default_branch).await?;
        }
        if dirty {
            self.git.clean(path).await?;
        }
        Ok(())
    }

    async fn refresh_slot(
        &self,
        repo: &Repo,
        config: &Config,
        root: &Path,
        slot: usize,
        fingerprint: &str,
        context: &JobCtx,
    ) -> DaemonResult<()> {
        let path = slot_path(root, slot);
        let old = self.read_hot_marker(&path).ok();
        if old
            .as_ref()
            .is_some_and(|marker| marker.prepare_fingerprint != fingerprint)
        {
            context.progress(format!("rebuilding slot {slot} after prepare hook change"))?;
            self.discard(&path)?;
            return self
                .build_slot(repo, root, slot, fingerprint, context)
                .await;
        }

        context.progress(format!("refreshing prepared slot {slot}"))?;
        self.files.remove_file(&hot_marker_path(&path))?;
        self.refresh_copy(&path, repo, None).await?;
        self.run_prepare_hooks(&path, &repo.hooks, context).await?;
        self.write_hot_marker(&path, repo, fingerprint).await?;
        if config.hot_pool_size <= slot as u64 {
            self.discard(&path)?;
        }
        Ok(())
    }

    async fn build_slot(
        &self,
        repo: &Repo,
        root: &Path,
        slot: usize,
        fingerprint: &str,
        context: &JobCtx,
    ) -> DaemonResult<()> {
        let staging = slot_staging_path(root, slot);
        let pid_path = slot_pid_path(root, slot);
        let final_path = slot_path(root, slot);
        if self.files.exists(&staging) {
            self.discard(&staging)?;
        }
        self.files
            .atomic_write_text(&pid_path, &format!("{}\n", std::process::id()))?;
        context.progress(format!("copying prepared slot {slot}"))?;
        let mut copy = CancellableCopy::start(
            Arc::clone(&self.files),
            PathBuf::from(&repo.path),
            staging.clone(),
            Some(pid_path.clone()),
        );
        copy.wait().await?;

        let result = async {
            check_cancelled(context)?;
            self.run_prepare_hooks(&staging, &repo.hooks, context)
                .await?;
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
            let _ignored = self.discard(&staging);
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
            self.git
                .fetch_refs(path, "origin", std::slice::from_ref(&default_ref))
                .await
                .map_err(|_| combined_error)?;
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
        let current = self.git.current_branch(path).await?;
        let head = self.git.revision(path, "HEAD").await?;
        let target = format!("origin/{}", repo.default_branch);
        let remote = self.git.revision(path, &target).await?;
        let dirty = !self.git.status_porcelain(path).await?.is_empty();
        if current != repo.default_branch {
            self.git.checkout_reset(path, &repo.default_branch).await?;
        } else if head != remote || dirty {
            self.git.hard_reset(path, &repo.default_branch).await?;
        }
        if dirty {
            self.git.clean(path).await?;
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
            match run_shell_hook(self.shell.as_deref(), cwd, command, context).await {
                Ok(output) if output.success() => {
                    record_hook_output(context, &output)?;
                }
                Ok(output) => {
                    record_hook_output(context, &output)?;
                    context.progress(format!(
                        "warning: prepare hook exited {}: {command}",
                        output.status
                    ))?;
                }
                Err(DaemonError::Cancelled) => return Err(DaemonError::Cancelled),
                Err(error) => {
                    context.progress(format!("warning: prepare hook failed: {error}"))?;
                }
            }
        }
        Ok(())
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
                    self.discard(&entry)?;
                }
            }
        }
        Ok(())
    }

    fn discard(&self, path: &Path) -> DaemonResult<()> {
        if !self.files.exists(path) {
            return Ok(());
        }
        let parent = path.parent().ok_or_else(|| {
            DaemonError::Validation(format!("path has no parent: {}", path.display()))
        })?;
        let discarded = parent.join(format!(".discard-{}", Uuid::new_v4()));
        self.files.rename(path, &discarded)?;
        self.files.remove_detached(&discarded)
    }

    fn update_status(&self, repo: &RepoId, config: &Config, root: &Path) {
        let mut refreshed_at = None;
        let mut ready = 0_u32;
        for slot in 0..usize_from_u64(config.hot_pool_size) {
            let path = slot_path(root, slot);
            if !self.files.exists(&path) {
                continue;
            }
            ready = ready.saturating_add(1);
            if let Ok(marker) = self.read_hot_marker(&path)
                && refreshed_at
                    .as_ref()
                    .is_none_or(|current: &String| marker.fetched_at > *current)
            {
                refreshed_at = Some(marker.fetched_at);
            }
        }
        STATUS_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                repo.clone(),
                PoolStatus {
                    repo: repo.clone(),
                    ready,
                    size: u32::try_from(config.hot_pool_size).unwrap_or(u32::MAX),
                    refreshed_at,
                },
            );
    }

    fn start_background_maintenance(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let startup = self.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            if let Ok(state) = startup.state.load().await {
                for repo in state.repos {
                    startup.submit_prepare(repo.id, false, None);
                }
            }
        });

        let periodic = self.clone();
        tokio::spawn(async move {
            loop {
                let interval = periodic
                    .config
                    .load()
                    .await
                    .map(|config| config.hot_refresh_interval_ms)
                    .unwrap_or(60_000);
                let delay = if interval == 0 { 60_000 } else { interval };
                tokio::time::sleep(Duration::from_millis(delay)).await;
                let Ok(config) = periodic.config.load().await else {
                    continue;
                };
                if config.hot_pool_size == 0 || config.hot_refresh_interval_ms == 0 {
                    continue;
                }
                if let Ok(state) = periodic.state.load().await {
                    for repo in state.repos {
                        if let Err(error) = periodic.prepare(repo.id, false).await {
                            tracing::warn!(%error, "periodic prepared-copy refresh failed");
                        }
                    }
                }
            }
        });
    }
}

fn prepare_fingerprint(hooks: &RepoHooks) -> DaemonResult<String> {
    let json = serde_json::to_string(&hooks.prepare)?;
    Ok(sha256::digest_hex(json.as_bytes()))
}

fn repo_worktrees_dir(config: &Config, repo: &RepoId) -> PathBuf {
    Path::new(&config.worktrees_dir)
        .join(repo.owner())
        .join(repo.name())
}

fn branch_refspec(branch: &str) -> String {
    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
}

fn usize_from_u64(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn check_cancelled(context: &JobCtx) -> DaemonResult<()> {
    if context.cancel.is_cancelled() {
        Err(DaemonError::Cancelled)
    } else {
        Ok(())
    }
}

async fn run_shell_hook(
    shell: Option<&dyn Shell>,
    cwd: &Path,
    command: &str,
    context: &JobCtx,
) -> DaemonResult<ShellResult> {
    if let Some(shell) = shell {
        check_cancelled(context)?;
        let output = shell
            .run(ShellCommand::new("sh").args(["-c", command]).cwd(cwd))
            .await?;
        check_cancelled(context)?;
        return Ok(output);
    }
    let mut process = tokio::process::Command::new("sh");
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = process.output();
    tokio::select! {
        () = context.cancel.cancelled() => Err(DaemonError::Cancelled),
        output = output => output
            .map(|output| ShellResult {
                status: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
            .map_err(|error| DaemonError::Shell(format!("sh -c `{command}`: {error}"))),
    }
}

fn record_hook_output(context: &JobCtx, output: &ShellResult) -> DaemonResult<()> {
    for line in output.stdout.lines() {
        context.progress(line)?;
    }
    for line in output.stderr.lines() {
        context.progress(line)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolEntryKind {
    Slot,
    Staging,
    Pid,
}

fn parse_pool_entry(name: &str) -> Option<(usize, PoolEntryKind)> {
    let (slot_name, kind) = if let Some(value) = name.strip_suffix(".staging.pid") {
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

fn scan_default_layout(repo: &Repo, configured_size: u64) -> Option<PoolStatus> {
    let repo_path = Path::new(&repo.path);
    let home = repo_path.parent()?.parent()?.parent()?;
    let root = home
        .join("worktrees")
        .join(repo.id.owner())
        .join(repo.id.name());
    let mut ready = 0_u32;
    let mut refreshed = BTreeSet::new();
    for slot in 0..usize_from_u64(configured_size) {
        let path = slot_path(&root, slot);
        if path.is_dir() {
            ready = ready.saturating_add(1);
            if let Ok(text) = std::fs::read_to_string(hot_marker_path(&path))
                && let Ok(marker) = serde_json::from_str::<HotMarker>(&text)
            {
                refreshed.insert(marker.fetched_at);
            }
        }
    }
    Some(PoolStatus {
        repo: repo.id.clone(),
        ready,
        size: u32::try_from(configured_size).unwrap_or(u32::MAX),
        refreshed_at: refreshed.into_iter().next_back(),
    })
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

pub(crate) struct CancellableCopy {
    task: Option<tokio::task::JoinHandle<DaemonResult<()>>>,
    files: Arc<dyn Files>,
    destination: PathBuf,
    pid_path: Option<PathBuf>,
    cleanup_on_drop: bool,
}

impl CancellableCopy {
    pub(crate) fn start(
        files: Arc<dyn Files>,
        source: PathBuf,
        destination: PathBuf,
        pid_path: Option<PathBuf>,
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
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _result = task.await;
                cleanup_copy_artifact(&files, &destination, pid_path.as_deref());
            });
        }
    }
}

fn cleanup_copy_artifact(files: &Arc<dyn Files>, destination: &Path, pid_path: Option<&Path>) {
    if let Some(pid_path) = pid_path {
        let _ignored = files.remove_file(pid_path);
    }
    if !files.exists(destination) {
        return;
    }
    let Some(parent) = destination.parent() else {
        return;
    };
    let discarded = parent.join(format!(".discard-{}", Uuid::new_v4()));
    if files.rename(destination, &discarded).is_ok() {
        let _ignored = files.remove_detached(&discarded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
