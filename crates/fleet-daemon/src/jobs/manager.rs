//! Job scheduling, concurrency limits, cancellation, and event publication.

mod logs;

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{File, OpenOptions},
    future::Future,
    io::{BufWriter, Write},
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    time::Duration as StdDuration,
};

use chrono::{DateTime, Duration, Utc};
use fleet_core::{
    ids::{JobId, RepoId},
    paths::FleetHome,
};
use fleet_proto::job::{JobKind, JobRecord, JobStatus};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::clock::{Clock, SystemClock},
    jobs::job::JobCtx,
};

struct ManagedJob {
    record: JobRecord,
    cancel: CancellationToken,
    cancellable: bool,
    retry: Option<RetryOperation>,
    log: Option<BufWriter<File>>,
}

type JobFuture = Pin<Box<dyn Future<Output = DaemonResult<()>> + Send>>;
type RetryOperation = Arc<dyn Fn(JobCtx) -> JobFuture + Send + Sync>;
type InitialOperation = Box<dyn FnOnce(JobCtx) -> JobFuture + Send>;

struct SubmissionSpec {
    kind: JobKind,
    target: String,
    title: String,
    cancellable: bool,
    retryable: bool,
    retry: Option<RetryOperation>,
}

struct RetentionPolicy {
    keep_finished_for: Duration,
    max_finished: usize,
}

struct JobState {
    jobs: BTreeMap<JobId, ManagedJob>,
    order: VecDeque<JobId>,
    in_flight_targets: HashMap<(JobKind, String), JobId>,
}

impl JobState {
    fn job(&self, id: &JobId) -> DaemonResult<&ManagedJob> {
        self.jobs
            .get(id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))
    }

    fn job_mut(&mut self, id: &JobId) -> DaemonResult<&mut ManagedJob> {
        self.jobs
            .get_mut(id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))
    }
}

/// Locks a manager mutex, recovering the guard after a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct JobManagerInner {
    logs_dir: PathBuf,
    clock: Arc<dyn Clock>,
    state: Mutex<JobState>,
    updates: broadcast::Sender<JobRecord>,
    repo_locks: Mutex<HashMap<RepoId, Weak<AsyncMutex<()>>>>,
    deleting_repos: Mutex<HashSet<RepoId>>,
    pool: Arc<Semaphore>,
    github: Arc<Semaphore>,
    retention: Mutex<RetentionPolicy>,
}

/// Detached background-job registry, scheduler resources, and progress log owner.
#[derive(Clone)]
pub struct JobManager {
    inner: Arc<JobManagerInner>,
}

impl JobManager {
    /// Creates a manager using the system clock, pool limit 2, and GitHub limit 4.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self::with_clock(home, Arc::new(SystemClock))
    }

    /// Creates a manager with an injected deterministic wall clock.
    #[must_use]
    pub fn with_clock(home: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        let home = FleetHome::new(home.into());
        let (updates, _receiver) = broadcast::channel(256);
        Self {
            inner: Arc::new(JobManagerInner {
                logs_dir: home.jobs_log_dir(),
                clock,
                state: Mutex::new(JobState {
                    jobs: BTreeMap::new(),
                    order: VecDeque::new(),
                    in_flight_targets: HashMap::new(),
                }),
                updates,
                repo_locks: Mutex::new(HashMap::new()),
                deleting_repos: Mutex::new(HashSet::new()),
                pool: Arc::new(Semaphore::new(2)),
                github: Arc::new(Semaphore::new(4)),
                retention: Mutex::new(RetentionPolicy {
                    keep_finished_for: Duration::minutes(10),
                    max_finished: 200,
                }),
            }),
        }
    }

    /// Submits async work and immediately returns its stable identifier.
    pub fn submit<F, Fut>(
        &self,
        kind: JobKind,
        target: impl Into<String>,
        title: impl Into<String>,
        cancellable: bool,
        retryable: bool,
        operation: F,
    ) -> JobId
    where
        F: FnOnce(JobCtx) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = DaemonResult<()>> + Send + 'static,
    {
        let target = target.into();
        let title = title.into();
        let retry = retryable.then(|| {
            let template = Arc::new(Mutex::new(operation.clone()));
            let retry_operation: RetryOperation = Arc::new(move |context| {
                let operation = lock(&template).clone();
                Box::pin(operation(context))
            });
            retry_operation
        });
        self.submit_boxed(
            SubmissionSpec {
                kind,
                target,
                title,
                cancellable,
                retryable,
                retry,
            },
            Box::new(move |context| Box::pin(operation(context))),
        )
    }

    fn submit_boxed(&self, spec: SubmissionSpec, operation: InitialOperation) -> JobId {
        let SubmissionSpec {
            kind,
            target,
            title,
            cancellable,
            retryable,
            retry,
        } = spec;
        let target_key = (kind.clone(), target.clone());
        {
            let state = lock(&self.inner.state);
            if let Some(id) = state.in_flight_targets.get(&target_key) {
                return id.clone();
            }
        }
        // `job-<uuid>` is non-empty and whitespace-free, the whole job id grammar.
        let id = JobId::try_from(format!("job-{}", Uuid::new_v4()))
            .expect("job-<uuid> is a valid job id");
        let cancel = CancellationToken::new();
        let log_path = self.log_path(&id);
        let record = JobRecord {
            id: id.clone(),
            kind,
            target,
            title,
            status: JobStatus::Queued,
            progress: None,
            log_path: log_path.to_string_lossy().into_owned(),
            started_at: self.inner.clock.now().to_rfc3339(),
            finished_at: None,
            cancellable,
            retryable,
        };
        {
            let mut state = lock(&self.inner.state);
            if let Some(existing) = state.in_flight_targets.get(&target_key) {
                return existing.clone();
            }
            state.order.push_back(id.clone());
            state.in_flight_targets.insert(target_key, id.clone());
            state.jobs.insert(
                id.clone(),
                ManagedJob {
                    record: record.clone(),
                    cancel: cancel.clone(),
                    cancellable,
                    retry,
                    log: None,
                },
            );
        }
        if let Err(error) = self.append_log_line(&id, None) {
            tracing::warn!(%error, job = %id, "failed to create job log");
        }
        let _receivers = self.inner.updates.send(record);
        let manager = self.clone();
        let task_id = id.clone();
        tokio::spawn(async move {
            manager.set_running(&task_id);
            let context = JobCtx {
                id: task_id.clone(),
                cancel: cancel.clone(),
                manager: manager.clone(),
            };
            let mut future = operation(context);
            let result = if cancellable {
                tokio::select! {
                    () = cancel.cancelled() => {
                        tokio::time::timeout(StdDuration::from_secs(3), &mut future)
                            .await
                            .unwrap_or(Err(DaemonError::Cancelled))
                    }
                    result = &mut future => result,
                }
            } else {
                future.await
            };
            manager.finish(&task_id, result);
        });
        id
    }

    /// Lists active jobs followed by retained finished jobs in submission order.
    #[must_use]
    pub fn list(&self) -> Vec<JobRecord> {
        self.prune(self.inner.clock.now());
        let state = lock(&self.inner.state);
        state
            .order
            .iter()
            .filter_map(|id| state.jobs.get(id).map(|job| job.record.clone()))
            .collect()
    }

    /// Requests cancellation of a running cancellable job and returns its current record.
    pub fn cancel(&self, id: &JobId) -> DaemonResult<JobRecord> {
        let (record, cancel) = {
            let mut state = lock(&self.inner.state);
            let job = state.job_mut(id)?;
            if !matches!(job.record.status, JobStatus::Queued | JobStatus::Running) {
                return Err(DaemonError::Conflict(format!("job {id} is not active")));
            }
            if !job.record.cancellable {
                return Err(DaemonError::Conflict(format!(
                    "job {id} is not cancellable"
                )));
            }
            job.record.status = JobStatus::Cancelling;
            let cancel = job.cancel.clone();
            (job.record.clone(), cancel)
        };
        cancel.cancel();
        let _receivers = self.inner.updates.send(record.clone());
        Ok(record)
    }

    /// Restarts a retained job when its original operation can be reconstructed.
    pub fn retry(&self, id: &JobId) -> DaemonResult<JobRecord> {
        let (spec, operation) = {
            let state = lock(&self.inner.state);
            let job = state.job(id)?;
            if !matches!(
                job.record.status,
                JobStatus::Failed { .. } | JobStatus::Cancelled
            ) {
                return Err(DaemonError::Conflict(format!(
                    "job {id} has not failed or been cancelled"
                )));
            }
            if !job.record.retryable {
                return Err(DaemonError::Conflict(format!("job {id} is not retryable")));
            }
            let retry = job.retry.clone().ok_or_else(|| {
                DaemonError::Unsupported(format!("job {id} has no retained retry operation"))
            })?;
            let operation = Arc::clone(&retry);
            (
                SubmissionSpec {
                    kind: job.record.kind.clone(),
                    target: job.record.target.clone(),
                    title: job.record.title.clone(),
                    cancellable: job.cancellable,
                    retryable: true,
                    retry: Some(retry),
                },
                operation,
            )
        };
        let new_id = self.submit_boxed(spec, Box::new(move |context| operation(context)));
        self.record(&new_id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {new_id}")))
    }

    /// Removes acknowledged completed jobs and their retained logs.
    pub fn dismiss(&self, ids: &[JobId]) -> DaemonResult<()> {
        let log_paths = {
            let mut state = lock(&self.inner.state);
            let mut paths = Vec::new();
            for id in ids {
                let job = state.job(id)?;
                if matches!(
                    job.record.status,
                    JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
                ) {
                    return Err(DaemonError::Conflict(format!("job {id} is still active")));
                }
            }
            for id in ids {
                if let Some(job) = state.jobs.remove(id) {
                    paths.push(PathBuf::from(job.record.log_path));
                }
                state.order.retain(|entry| entry != id);
            }
            paths
        };
        for path in log_paths {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(DaemonError::fs(path, error)),
            }
        }
        Ok(())
    }

    /// Configures how long completed records stay visible to clients.
    pub fn set_retention(&self, keep_finished_for: StdDuration) {
        let keep_finished_for = Duration::from_std(keep_finished_for).unwrap_or(Duration::MAX);
        lock(&self.inner.retention).keep_finished_for = keep_finished_for;
        self.prune(self.inner.clock.now());
    }

    /// Returns one retained job record.
    #[must_use]
    pub fn record(&self, id: &JobId) -> Option<JobRecord> {
        lock(&self.inner.state)
            .jobs
            .get(id)
            .map(|job| job.record.clone())
    }

    /// Subscribes to every created or changed job record.
    pub fn subscribe(&self) -> broadcast::Receiver<JobRecord> {
        self.inner.updates.subscribe()
    }

    /// Waits for a retained job to reach a terminal state.
    pub async fn wait(&self, id: &JobId) -> DaemonResult<JobRecord> {
        let mut updates = self.subscribe();
        loop {
            let record = self
                .record(id)
                .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))?;
            if !matches!(
                record.status,
                JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
            ) {
                return Ok(record);
            }
            loop {
                match updates.recv().await {
                    Ok(record) if &record.id == id => break,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(DaemonError::Cancelled);
                    }
                }
            }
        }
    }

    /// Returns the shared in-process mutex for one repository.
    pub fn repo_lock(&self, repo: &RepoId) -> Arc<AsyncMutex<()>> {
        let mut locks = lock(&self.inner.repo_locks);
        if let Some(lock) = locks.get(repo).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(repo.clone(), Arc::downgrade(&lock));
        lock
    }

    /// Prevents new work for a repository until the returned guard is dropped.
    pub fn begin_repo_deletion(&self, repo: &RepoId) -> DaemonResult<RepoDeletionGuard> {
        let mut deleting = lock(&self.inner.deleting_repos);
        if !deleting.insert(repo.clone()) {
            return Err(DaemonError::Conflict(format!(
                "repository {repo} is already being deleted"
            )));
        }
        Ok(RepoDeletionGuard {
            manager: self.clone(),
            repo: repo.clone(),
        })
    }

    /// Rejects work submitted after repository deletion has started.
    pub fn ensure_repo_available(&self, repo: &RepoId) -> DaemonResult<()> {
        if lock(&self.inner.deleting_repos).contains(repo) {
            Err(DaemonError::Conflict(format!(
                "repository {repo} is being deleted"
            )))
        } else {
            Ok(())
        }
    }

    /// Cancels and awaits every active job scoped to a repository.
    pub async fn quiesce_repo(&self, repo: &RepoId) -> DaemonResult<()> {
        let active = self
            .list()
            .into_iter()
            .filter(|job| {
                job_targets_repo(&job.target, repo)
                    && matches!(
                        job.status,
                        JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
                    )
            })
            .collect::<Vec<_>>();
        for job in &active {
            if job.cancellable {
                let _ignored = self.cancel(&job.id);
            }
        }
        for job in active {
            let _record = self.wait(&job.id).await?;
        }
        Ok(())
    }

    /// Returns the global prepared-pool concurrency semaphore with two permits.
    #[must_use]
    pub fn pool_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.inner.pool)
    }

    /// Returns the global GitHub concurrency semaphore with four permits.
    #[must_use]
    pub fn github_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.inner.github)
    }

    fn set_running(&self, id: &JobId) {
        let mut state = lock(&self.inner.state);
        if let Some(job) = state.jobs.get_mut(id)
            && matches!(job.record.status, JobStatus::Queued)
        {
            job.record.status = JobStatus::Running;
            let _receivers = self.inner.updates.send(job.record.clone());
        }
    }

    fn finish(&self, id: &JobId, result: DaemonResult<()>) {
        let finished = self.inner.clock.now();
        let (status, outcome) = match result {
            Ok(()) => (JobStatus::Succeeded, Cow::Borrowed("success")),
            Err(DaemonError::Cancelled) => (JobStatus::Cancelled, Cow::Borrowed("cancelled")),
            Err(error) => {
                let error = error.to_string();
                let outcome = Cow::Owned(format!("failed: {error}"));
                (JobStatus::Failed { error }, outcome)
            }
        };
        if let Err(error) = self.append_log_line(id, Some(&outcome)) {
            tracing::warn!(%error, job = %id, "failed to append terminal job status");
        }
        let changed = {
            let mut state = lock(&self.inner.state);
            let (key, changed) = {
                let Some(job) = state.jobs.get_mut(id) else {
                    return;
                };
                if let Some(mut log) = job.log.take()
                    && let Err(error) = log.flush()
                {
                    tracing::warn!(%error, job = %id, "failed to flush job log");
                }
                job.record.status = status;
                if matches!(job.record.status, JobStatus::Succeeded) {
                    job.retry = None;
                }
                job.record.finished_at = Some(finished.to_rfc3339());
                job.record.cancellable = false;
                (
                    (job.record.kind.clone(), job.record.target.clone()),
                    job.record.clone(),
                )
            };
            if state.in_flight_targets.get(&key) == Some(id) {
                state.in_flight_targets.remove(&key);
            }
            changed
        };
        let _receivers = self.inner.updates.send(changed);
        self.prune(finished);
    }

    fn prune(&self, now: DateTime<Utc>) {
        let (keep_finished_for, max_finished) = {
            let retention = lock(&self.inner.retention);
            (retention.keep_finished_for, retention.max_finished)
        };
        let mut state = lock(&self.inner.state);
        let cutoff = now.checked_sub_signed(keep_finished_for);
        let mut finished = state
            .order
            .iter()
            .filter(|id| {
                state
                    .jobs
                    .get(*id)
                    .is_some_and(|job| matches!(job.record.status, JobStatus::Succeeded))
            })
            .cloned()
            .collect::<Vec<_>>();
        let expired = finished
            .iter()
            .filter(|id| {
                cutoff.is_some_and(|cutoff| finished_before(&state.jobs[id].record, cutoff))
            })
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let excess = finished.len().saturating_sub(max_finished);
        finished.truncate(excess);
        let remove = expired
            .into_iter()
            .chain(finished)
            .collect::<std::collections::BTreeSet<_>>();
        state.order.retain(|id| !remove.contains(id));
        let paths = remove
            .into_iter()
            .filter_map(|id| {
                state
                    .jobs
                    .remove(&id)
                    .map(|job| PathBuf::from(job.record.log_path))
            })
            .collect::<Vec<_>>();
        drop(state);
        for path in paths {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "failed to remove expired job log")
                }
            }
        }
    }
}

/// Exclusive repository-deletion tombstone.
pub struct RepoDeletionGuard {
    manager: JobManager,
    repo: RepoId,
}

impl Drop for RepoDeletionGuard {
    fn drop(&mut self) {
        lock(&self.manager.inner.deleting_repos).remove(&self.repo);
    }
}

fn job_targets_repo(target: &str, repo: &RepoId) -> bool {
    target
        .strip_prefix(repo.as_str())
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with([':', '#']))
}

fn finished_before(record: &JobRecord, cutoff: DateTime<Utc>) -> bool {
    record
        .finished_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|value| value.with_timezone(&Utc) < cutoff)
}

#[cfg(test)]
mod tests;
