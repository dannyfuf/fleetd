//! Job scheduling, concurrency limits, cancellation, logging, and event publication.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs::OpenOptions,
    future::Future,
    io::Write,
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

#[derive(Clone)]
struct ManagedJob {
    record: JobRecord,
    cancel: CancellationToken,
    retry: Option<RetrySpec>,
}

type JobFuture = Pin<Box<dyn Future<Output = DaemonResult<()>> + Send>>;
type RetryOperation = Arc<dyn Fn(JobCtx) -> JobFuture + Send + Sync>;
type InitialOperation = Box<dyn FnOnce(JobCtx) -> JobFuture + Send>;

#[derive(Clone)]
struct RetrySpec {
    kind: JobKind,
    target: String,
    title: String,
    cancellable: bool,
    operation: RetryOperation,
}

struct SubmissionSpec {
    kind: JobKind,
    target: String,
    title: String,
    cancellable: bool,
    retryable: bool,
    retry: Option<RetrySpec>,
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

struct JobManagerInner {
    logs_dir: PathBuf,
    clock: Arc<dyn Clock>,
    state: Mutex<JobState>,
    updates: broadcast::Sender<JobRecord>,
    repo_locks: Mutex<HashMap<RepoId, Weak<AsyncMutex<()>>>>,
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
                let operation = template
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                Box::pin(operation(context))
            });
            RetrySpec {
                kind: kind.clone(),
                target: target.clone(),
                title: title.clone(),
                cancellable,
                operation: retry_operation,
            }
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

    /// Submits work whose operation factory is retained for explicit failed-job retries.
    pub fn submit_retryable<F, Fut>(
        &self,
        kind: JobKind,
        target: impl Into<String>,
        title: impl Into<String>,
        cancellable: bool,
        operation: F,
    ) -> JobId
    where
        F: Fn(JobCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DaemonResult<()>> + Send + 'static,
    {
        let target = target.into();
        let title = title.into();
        let operation: RetryOperation = Arc::new(move |context| Box::pin(operation(context)));
        let retry = RetrySpec {
            kind: kind.clone(),
            target: target.clone(),
            title: title.clone(),
            cancellable,
            operation: Arc::clone(&operation),
        };
        self.submit_boxed(
            SubmissionSpec {
                kind,
                target,
                title,
                cancellable,
                retryable: true,
                retry: Some(retry),
            },
            Box::new(move |context| operation(context)),
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
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(id) = state.in_flight_targets.get(&target_key) {
                return id.clone();
            }
        }
        let id = loop {
            if let Ok(id) = JobId::try_from(format!("job-{}", Uuid::new_v4())) {
                break id;
            }
        };
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
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                    retry,
                },
            );
        }
        let _ignored = std::fs::create_dir_all(&self.inner.logs_dir).and_then(|()| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map(drop)
        });
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
            let result = if cancellable {
                tokio::select! {
                    () = cancel.cancelled() => Err(DaemonError::Cancelled),
                    result = operation(context) => result,
                }
            } else {
                operation(context).await
            };
            manager.finish(&task_id, result);
        });
        id
    }

    /// Lists active jobs followed by retained finished jobs in submission order.
    #[must_use]
    pub fn list(&self) -> Vec<JobRecord> {
        self.prune(self.inner.clock.now());
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .order
            .iter()
            .filter_map(|id| state.jobs.get(id).map(|job| job.record.clone()))
            .collect()
    }

    /// Requests cancellation of a running cancellable job and returns its current record.
    pub fn cancel(&self, id: &JobId) -> DaemonResult<JobRecord> {
        let (record, cancel) = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let job = state
                .jobs
                .get_mut(id)
                .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))?;
            if !matches!(job.record.status, JobStatus::Queued | JobStatus::Running) {
                return Err(DaemonError::Conflict(format!("job {id} is not active")));
            }
            if !job.record.cancellable {
                return Err(DaemonError::Conflict(format!(
                    "job {id} is not cancellable"
                )));
            }
            job.record.status = JobStatus::Cancelling;
            let cancel = Some(job.cancel.clone());
            (job.record.clone(), cancel)
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
            let _receivers = self.inner.updates.send(record.clone());
        }
        Ok(record)
    }

    /// Restarts a retained job when its original operation can be reconstructed.
    pub fn retry(&self, id: &JobId) -> DaemonResult<JobRecord> {
        let retry = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let job = state
                .jobs
                .get(id)
                .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))?;
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
            job.retry.clone().ok_or_else(|| {
                DaemonError::Unsupported(format!("job {id} has no retained retry operation"))
            })?
        };
        let operation = Arc::clone(&retry.operation);
        let new_id = self.submit_boxed(
            SubmissionSpec {
                kind: retry.kind.clone(),
                target: retry.target.clone(),
                title: retry.title.clone(),
                cancellable: retry.cancellable,
                retryable: true,
                retry: Some(retry),
            },
            Box::new(move |context| operation(context)),
        );
        self.record(&new_id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {new_id}")))
    }

    /// Removes acknowledged completed jobs and their retained logs.
    pub fn dismiss(&self, ids: &[JobId]) -> DaemonResult<()> {
        let log_paths = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut paths = Vec::new();
            for id in ids {
                let job = state
                    .jobs
                    .get(id)
                    .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))?;
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
        self.inner
            .retention
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keep_finished_for = keep_finished_for;
        self.prune(self.inner.clock.now());
    }

    /// Returns one retained job record.
    #[must_use]
    pub fn record(&self, id: &JobId) -> Option<JobRecord> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .jobs
            .get(id)
            .map(|job| job.record.clone())
    }

    /// Reads at most the last `lines` lines from a job's persistent log.
    pub async fn tail(&self, id: &JobId, lines: usize) -> DaemonResult<Vec<String>> {
        let path = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            PathBuf::from(
                &state
                    .jobs
                    .get(id)
                    .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))?
                    .record
                    .log_path,
            )
        };
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| DaemonError::fs(&path, error))?;
        let all = text.lines().map(str::to_owned).collect::<Vec<_>>();
        let start = all.len().saturating_sub(lines);
        Ok(all[start..].to_vec())
    }

    /// Subscribes to every created or changed job record.
    pub fn subscribe(&self) -> broadcast::Receiver<JobRecord> {
        self.inner.updates.subscribe()
    }

    /// Returns the shared in-process mutex for one repository.
    pub fn repo_lock(&self, repo: &RepoId) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .inner
            .repo_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(lock) = locks.get(repo).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(repo.clone(), Arc::downgrade(&lock));
        lock
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

    /// Returns the persistent log path for a job identifier.
    #[must_use]
    pub fn log_path(&self, id: &JobId) -> PathBuf {
        self.inner.logs_dir.join(format!("{id}.log"))
    }

    pub(crate) fn record_progress(&self, id: &JobId, line: String) -> DaemonResult<()> {
        let path = self.log_path(id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| DaemonError::fs(parent, error))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| DaemonError::fs(&path, error))?;
        writeln!(file, "{line}").map_err(|error| DaemonError::fs(&path, error))?;
        let changed = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let job = state
                .jobs
                .get_mut(id)
                .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))?;
            job.record.progress = Some(line);
            job.record.clone()
        };
        let _receivers = self.inner.updates.send(changed);
        Ok(())
    }

    fn set_running(&self, id: &JobId) {
        self.update_record(id, |record| {
            if matches!(record.status, JobStatus::Queued) {
                record.status = JobStatus::Running;
            }
        });
    }

    fn finish(&self, id: &JobId, result: DaemonResult<()>) {
        let finished = self.inner.clock.now();
        let changed = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (key, changed) = {
                let Some(job) = state.jobs.get_mut(id) else {
                    return;
                };
                job.record.status = match result {
                    Ok(()) => JobStatus::Succeeded,
                    Err(DaemonError::Cancelled) => JobStatus::Cancelled,
                    Err(error) => JobStatus::Failed {
                        error: error.to_string(),
                    },
                };
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

    fn update_record(&self, id: &JobId, update: impl FnOnce(&mut JobRecord)) {
        let changed = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(job) = state.jobs.get_mut(id) else {
                return;
            };
            update(&mut job.record);
            job.record.clone()
        };
        let _receivers = self.inner.updates.send(changed);
    }

    fn prune(&self, now: DateTime<Utc>) {
        let (keep_finished_for, max_finished) = {
            let retention = self
                .inner
                .retention
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (retention.keep_finished_for, retention.max_finished)
        };
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cutoff = now - keep_finished_for;
        let mut finished = state
            .order
            .iter()
            .filter(|id| {
                state.jobs.get(*id).is_some_and(|job| {
                    !matches!(
                        job.record.status,
                        JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
                    )
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let expired = finished
            .iter()
            .filter(|id| finished_before(&state.jobs[id].record, cutoff))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let excess = finished.len().saturating_sub(max_finished);
        finished.truncate(excess);
        let remove = expired
            .into_iter()
            .chain(finished)
            .collect::<std::collections::BTreeSet<_>>();
        state.order.retain(|id| !remove.contains(id));
        state.jobs.retain(|id, _job| !remove.contains(id));
    }
}

fn finished_before(record: &JobRecord, cutoff: DateTime<Utc>) -> bool {
    record
        .finished_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|value| value.with_timezone(&Utc) < cutoff)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration as StdDuration,
    };

    use chrono::{TimeZone, Utc};
    use fleet_proto::job::JobKind;

    use crate::testing::fakes::FixedClock;

    use super::*;

    async fn wait_finished(manager: &JobManager, id: &JobId) -> JobRecord {
        for _ in 0..100 {
            if let Some(record) = manager.list().into_iter().find(|record| &record.id == id)
                && !matches!(
                    record.status,
                    JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
                )
            {
                return record;
            }
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
        panic!("job did not finish");
    }

    #[tokio::test]
    async fn submit_records_progress_and_log_tail() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let manager = JobManager::new(temp.path());
        let id = manager.submit(
            JobKind::Inspect,
            "all",
            "Inspect",
            true,
            true,
            |context| async move {
                context.progress("first")?;
                context.progress("second")?;
                Ok(())
            },
        );
        let record = wait_finished(&manager, &id).await;
        assert_eq!(record.status, JobStatus::Succeeded);
        assert_eq!(record.progress.as_deref(), Some("second"));
        assert_eq!(
            manager.tail(&id, 1).await.ok(),
            Some(vec!["second".to_owned()])
        );
    }

    #[tokio::test]
    async fn cancellation_changes_running_job_to_cancelled() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let manager = JobManager::new(temp.path());
        let id = manager.submit(
            JobKind::Custom("wait".to_owned()),
            "x",
            "Wait",
            true,
            false,
            |_context| async move {
                std::future::pending::<()>().await;
                Ok(())
            },
        );
        tokio::task::yield_now().await;
        manager
            .cancel(&id)
            .unwrap_or_else(|error| panic!("{error}"));
        let record = wait_finished(&manager, &id).await;
        assert_eq!(record.status, JobStatus::Cancelled);
    }

    #[tokio::test]
    async fn suppresses_duplicate_active_kind_and_target() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let manager = JobManager::new(temp.path());
        let first = manager.submit(
            JobKind::RepoFetch,
            "acme/api",
            "Fetch",
            true,
            true,
            |_context| async move {
                std::future::pending::<()>().await;
                Ok(())
            },
        );
        let second = manager.submit(
            JobKind::RepoFetch,
            "acme/api",
            "Fetch again",
            true,
            true,
            |_context| async move { Ok(()) },
        );
        assert_eq!(second, first);
        assert_eq!(manager.list().len(), 1);
        manager
            .cancel(&first)
            .unwrap_or_else(|error| panic!("{error}"));
        let _record = wait_finished(&manager, &first).await;
    }

    #[tokio::test]
    async fn retry_starts_a_new_attempt_from_the_retained_factory() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let manager = JobManager::new(temp.path());
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempt_counter = Arc::clone(&attempts);
        let first = manager.submit(
            JobKind::RepoFetch,
            "acme/api",
            "Fetch",
            true,
            true,
            move |_context| {
                let attempts = Arc::clone(&attempt_counter);
                async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(DaemonError::Git("temporary failure".to_owned()))
                    } else {
                        Ok(())
                    }
                }
            },
        );
        assert!(matches!(
            wait_finished(&manager, &first).await.status,
            JobStatus::Failed { .. }
        ));

        let retried = manager
            .retry(&first)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_ne!(retried.id, first);
        assert_eq!(
            wait_finished(&manager, &retried.id).await.status,
            JobStatus::Succeeded
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn finished_jobs_expire_at_the_configured_retention() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let initial = Utc
            .with_ymd_and_hms(2026, 9, 4, 12, 0, 0)
            .single()
            .unwrap_or_else(|| panic!("valid timestamp"));
        let clock = Arc::new(FixedClock::new(initial));
        let manager = JobManager::with_clock(temp.path(), clock.clone());
        manager.set_retention(StdDuration::from_secs(1));
        let id = manager.submit(
            JobKind::Inspect,
            "all",
            "Inspect",
            true,
            false,
            |_context| async { Ok(()) },
        );
        let _record = wait_finished(&manager, &id).await;
        clock.set(initial + Duration::seconds(2));

        assert!(manager.list().is_empty());
    }
}
