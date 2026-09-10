//! Job scheduling, concurrency limits, cancellation, and event publication.

mod logs;

use std::{
    any::Any,
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{File, OpenOptions},
    future::Future,
    io::{BufWriter, Write},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
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
use futures_util::FutureExt;
use tokio::sync::{Mutex as AsyncMutex, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::clock::{Clock, SystemClock},
    jobs::job::{CleanupTracker, JobCtx},
};

struct ManagedJob {
    record: JobRecord,
    cancel: CancellationToken,
    cancellable: bool,
    retry: Option<RetryOperation>,
    log: JobLog,
    repos: Vec<RepoId>,
    all_repos: bool,
}

/// A job's buffered log writer, held outside the registry lock so its IO never blocks the
/// registry: writers take this mutex, everything else takes `JobManagerInner::state`.
type JobLog = Arc<Mutex<Option<BufWriter<File>>>>;

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
    repos: Vec<RepoId>,
    all_repos: bool,
}

struct RetentionPolicy {
    keep_finished_for: Duration,
    max_finished: usize,
}

/// Cancellation and retry behavior attached to a typed job submission.
#[derive(Debug, Clone, Copy)]
pub struct JobPolicy {
    cancellable: bool,
    retryable: bool,
}

impl JobPolicy {
    /// Creates a submission policy.
    #[must_use]
    pub const fn new(cancellable: bool, retryable: bool) -> Self {
        Self {
            cancellable,
            retryable,
        }
    }
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
    cancellation_grace: Mutex<StdDuration>,
    cleanup_grace: Mutex<StdDuration>,
    quiesce_budget: Mutex<StdDuration>,
}

/// Longest a cancelled job keeps running after its cancellation token fires.
const CANCELLATION_GRACE: StdDuration = StdDuration::from_secs(3);

/// Longest a cancelled job's registered cleanup is awaited once the operation itself is gone.
const CLEANUP_GRACE: StdDuration = StdDuration::from_secs(3);

/// Longest `quiesce_repo` waits for a repository's active jobs, taken as one budget for all of
/// them rather than one per job.
///
/// `quiesce_repo` cancels every cancellable job before it waits, so their graces run down
/// concurrently and the wait is bounded by the slowest job, not by their sum. The budget is
/// therefore sized above `CANCELLATION_GRACE + CLEANUP_GRACE` — the worst case for a job that
/// answers neither — so a job that is quiescing correctly is never reported as blocking the
/// repository, and kept under the client's ten-second request timeout so a deletion blocked by a
/// non-cancellable job answers with a conflict instead of a transport timeout.
const QUIESCE_BUDGET: StdDuration = CANCELLATION_GRACE
    .saturating_add(CLEANUP_GRACE)
    .saturating_add(StdDuration::from_secs(1));

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
                cancellation_grace: Mutex::new(CANCELLATION_GRACE),
                cleanup_grace: Mutex::new(CLEANUP_GRACE),
                quiesce_budget: Mutex::new(QUIESCE_BUDGET),
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
                repos: Vec::new(),
                all_repos: false,
            },
            Box::new(move |context| Box::pin(operation(context))),
        )
    }

    /// Atomically admits and submits work scoped to one repository.
    pub fn submit_for_repo<F, Fut>(
        &self,
        repo: RepoId,
        kind: JobKind,
        target: impl Into<String>,
        title: impl Into<String>,
        policy: JobPolicy,
        operation: F,
    ) -> DaemonResult<JobId>
    where
        F: FnOnce(JobCtx) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = DaemonResult<()>> + Send + 'static,
    {
        self.submit_for_repos(vec![repo], kind, target, title, policy, operation)
    }

    /// Atomically admits and submits work spanning a known repository set.
    pub fn submit_for_repos<F, Fut>(
        &self,
        mut repos: Vec<RepoId>,
        kind: JobKind,
        target: impl Into<String>,
        title: impl Into<String>,
        policy: JobPolicy,
        operation: F,
    ) -> DaemonResult<JobId>
    where
        F: FnOnce(JobCtx) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = DaemonResult<()>> + Send + 'static,
    {
        repos.sort();
        repos.dedup();
        let retry = policy.retryable.then(|| {
            let template = Arc::new(Mutex::new(operation.clone()));
            let retry_operation: RetryOperation = Arc::new(move |context| {
                let operation = lock(&template).clone();
                Box::pin(operation(context))
            });
            retry_operation
        });
        self.submit_boxed_admitted(
            SubmissionSpec {
                kind,
                target: target.into(),
                title: title.into(),
                cancellable: policy.cancellable,
                retryable: policy.retryable,
                retry,
                repos,
                all_repos: false,
            },
            Box::new(move |context| Box::pin(operation(context))),
        )
    }

    /// Atomically admits work that may touch any currently registered repository.
    pub fn submit_for_all_repos<F, Fut>(
        &self,
        kind: JobKind,
        target: impl Into<String>,
        title: impl Into<String>,
        policy: JobPolicy,
        operation: F,
    ) -> DaemonResult<JobId>
    where
        F: FnOnce(JobCtx) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = DaemonResult<()>> + Send + 'static,
    {
        let retry = policy.retryable.then(|| {
            let template = Arc::new(Mutex::new(operation.clone()));
            let retry_operation: RetryOperation = Arc::new(move |context| {
                let operation = lock(&template).clone();
                Box::pin(operation(context))
            });
            retry_operation
        });
        self.submit_boxed_admitted(
            SubmissionSpec {
                kind,
                target: target.into(),
                title: title.into(),
                cancellable: policy.cancellable,
                retryable: policy.retryable,
                retry,
                repos: Vec::new(),
                all_repos: true,
            },
            Box::new(move |context| Box::pin(operation(context))),
        )
    }

    fn submit_boxed_admitted(
        &self,
        spec: SubmissionSpec,
        operation: InitialOperation,
    ) -> DaemonResult<JobId> {
        let deleting = lock(&self.inner.deleting_repos);
        if spec.all_repos
            && let Some(repo) = deleting.iter().next()
        {
            return Err(DaemonError::Conflict(format!(
                "repository {repo} is being deleted"
            )));
        }
        if let Some(repo) = spec.repos.iter().find(|repo| deleting.contains(*repo)) {
            return Err(DaemonError::Conflict(format!(
                "repository {repo} is being deleted"
            )));
        }
        let id = self.submit_boxed(spec, operation);
        drop(deleting);
        Ok(id)
    }

    fn submit_boxed(&self, spec: SubmissionSpec, operation: InitialOperation) -> JobId {
        let SubmissionSpec {
            kind,
            target,
            title,
            cancellable,
            retryable,
            retry,
            repos,
            all_repos,
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
                    log: JobLog::default(),
                    repos,
                    all_repos,
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
            let result = AssertUnwindSafe(async {
                manager.set_running(&task_id);
                let context = JobCtx {
                    id: task_id.clone(),
                    cancel: cancel.clone(),
                    manager: manager.clone(),
                    cleanup: CleanupTracker::default(),
                };
                let cleanup = context.cleanup.clone();
                let mut future = operation(context);
                if cancellable {
                    let (result, cancelled) = tokio::select! {
                        () = cancel.cancelled() => {
                            let cancellation_grace = *lock(&manager.inner.cancellation_grace);
                            let result = tokio::time::timeout(
                                cancellation_grace,
                                &mut future,
                            )
                            .await
                            .unwrap_or(Err(DaemonError::Cancelled));
                            (result, true)
                        }
                        result = &mut future => (result, false),
                    };
                    drop(future);
                    if cancelled {
                        let cleanup_grace = *lock(&manager.inner.cleanup_grace);
                        if tokio::time::timeout(cleanup_grace, cleanup.wait())
                            .await
                            .is_err()
                        {
                            tracing::warn!(
                                job = %task_id,
                                ?cleanup_grace,
                                "cancellation cleanup exceeded its grace period"
                            );
                            if let Err(error) = manager.record_progress(
                                &task_id,
                                "warning: cancellation cleanup exceeded its grace period"
                                    .to_owned(),
                            ) {
                                tracing::warn!(
                                    %error,
                                    job = %task_id,
                                    "failed to record cancellation cleanup timeout"
                                );
                            }
                        }
                    }
                    result
                } else {
                    future.await
                }
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|payload| {
                Err(DaemonError::Join(format!(
                    "job panicked: {}",
                    panic_message(payload.as_ref())
                )))
            });
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
                    repos: job.repos.clone(),
                    all_repos: job.all_repos,
                },
                operation,
            )
        };
        let new_id =
            self.submit_boxed_admitted(spec, Box::new(move |context| operation(context)))?;
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

    #[cfg(test)]
    pub(crate) fn set_cancellation_grace(&self, cancellation_grace: StdDuration) {
        *lock(&self.inner.cancellation_grace) = cancellation_grace;
    }

    #[cfg(test)]
    pub(crate) fn set_cleanup_grace(&self, cleanup_grace: StdDuration) {
        *lock(&self.inner.cleanup_grace) = cleanup_grace;
    }

    #[cfg(test)]
    pub(crate) fn set_quiesce_budget(&self, quiesce_budget: StdDuration) {
        *lock(&self.inner.quiesce_budget) = quiesce_budget;
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
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
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
        self.prune(self.inner.clock.now());
        let active = {
            let state = lock(&self.inner.state);
            state
                .jobs
                .values()
                .filter(|job| {
                    (job.all_repos
                        || job.repos.contains(repo)
                        || (job.repos.is_empty() && job_targets_repo(&job.record.target, repo)))
                        && matches!(
                            job.record.status,
                            JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
                        )
                })
                .map(|job| job.record.clone())
                .collect::<Vec<_>>()
        };
        for job in &active {
            if job.cancellable {
                let _ignored = self.cancel(&job.id);
            }
        }
        let deadline = tokio::time::Instant::now() + *lock(&self.inner.quiesce_budget);
        for job in active {
            match tokio::time::timeout_at(deadline, self.wait(&job.id)).await {
                Ok(record) => {
                    let _record = record?;
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        job = %job.id,
                        target = %job.target,
                        %repo,
                        "job did not quiesce inside the repository's quiesce budget"
                    );
                    // Reporting instead of proceeding keeps the caller from deleting a tree a
                    // non-cancellable job is still working in; its guard drops the tombstone.
                    return Err(DaemonError::Conflict(format!(
                        "{} for {} is still running",
                        job.title, job.target
                    )));
                }
            }
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
        let (changed, log) = {
            let mut state = lock(&self.inner.state);
            let (key, changed, log) = {
                let Some(job) = state.jobs.get_mut(id) else {
                    return;
                };
                job.record.status = status;
                if matches!(job.record.status, JobStatus::Succeeded) {
                    job.retry = None;
                }
                job.record.finished_at = Some(finished.to_rfc3339());
                job.record.cancellable = false;
                (
                    (job.record.kind.clone(), job.record.target.clone()),
                    job.record.clone(),
                    Arc::clone(&job.log),
                )
            };
            if state.in_flight_targets.get(&key) == Some(id) {
                state.in_flight_targets.remove(&key);
            }
            (changed, log)
        };
        // The final flush happens without the registry lock, and before the terminal record is
        // published, so a subscriber that reads the log still sees every line.
        if let Some(mut log) = lock(&log).take()
            && let Err(error) = log.flush()
        {
            tracing::warn!(%error, job = %id, "failed to flush job log");
        }
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

fn panic_message(payload: &(dyn Any + Send)) -> Cow<'_, str> {
    if let Some(message) = payload.downcast_ref::<&str>() {
        Cow::Borrowed(message)
    } else if let Some(message) = payload.downcast_ref::<String>() {
        Cow::Borrowed(message)
    } else {
        Cow::Borrowed("unknown panic payload")
    }
}

#[cfg(test)]
mod tests;
