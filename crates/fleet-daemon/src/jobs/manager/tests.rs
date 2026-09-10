use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration as StdDuration,
};

use chrono::{TimeZone, Utc};
use fleet_proto::job::JobKind;

use crate::testing::fakes::FixedClock;

use super::*;

async fn wait_finished(manager: &JobManager, id: &JobId) -> JobRecord {
    tokio::time::timeout(StdDuration::from_secs(4), manager.wait(id))
        .await
        .expect("job did not finish")
        .expect("retained job")
}

#[tokio::test]
async fn repeated_progress_keeps_every_log_line_without_duplicate_updates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    let mut updates = manager.subscribe();
    let id = manager.submit(
        JobKind::Inspect,
        "all",
        "Inspect",
        false,
        false,
        |context| async move {
            context.progress("same")?;
            context.progress("same")?;
            context.progress("changed")?;
            Ok(())
        },
    );
    manager.wait(&id).await.expect("job finishes");
    let records = std::iter::from_fn(|| updates.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        records.len(),
        5,
        "queued, running, two progress changes, finished"
    );
    assert_eq!(
        manager.tail(&id, 10).await.expect("tail"),
        ["same", "same", "changed", "success"]
    );
}

#[tokio::test]
async fn tail_flushes_active_buffers_and_expiry_removes_only_finished_logs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let manager = JobManager::with_clock(temp.path(), clock.clone());
    let id = manager.submit(
        JobKind::Inspect,
        "all",
        "Inspect",
        true,
        false,
        |context| async move {
            context.progress("visible before completion")?;
            context.cancel.cancelled().await;
            Ok(())
        },
    );
    tokio::time::timeout(StdDuration::from_secs(1), async {
        while manager.record(&id).expect("record").progress.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("progress");
    assert_eq!(
        manager.tail(&id, 1).await.expect("active tail"),
        ["visible before completion"]
    );
    manager.set_retention(StdDuration::ZERO);
    clock.set(clock.now() + Duration::seconds(1));
    assert_eq!(manager.list().len(), 1);
    assert!(manager.log_path(&id).exists());
    manager.cancel(&id).expect("cancel");
    manager.wait(&id).await.expect("finish");
    clock.set(clock.now() + Duration::seconds(1));
    assert!(manager.list().is_empty());
    assert!(!manager.log_path(&id).exists());
}

#[tokio::test]
async fn successful_jobs_release_retained_retry_captures() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    let owner = Arc::new(());
    let weak = Arc::downgrade(&owner);
    let id = manager.submit(
        JobKind::Inspect,
        "all",
        "Inspect",
        false,
        true,
        move |_| async move {
            drop(owner);
            Ok(())
        },
    );
    manager.wait(&id).await.expect("finish");
    assert!(weak.upgrade().is_none());
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
        Some(vec!["success".to_owned()])
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
        |context| async move {
            context.cancel.cancelled().await;
            Err(DaemonError::Cancelled)
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
async fn cancel_waits_for_rollback() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    manager.set_cancellation_grace(StdDuration::ZERO);
    let (rollback_started, rollback_started_rx) = tokio::sync::oneshot::channel();
    let (rollback_finished, rollback_finished_rx) = tokio::sync::oneshot::channel();
    let (operation_ready, operation_ready_rx) = tokio::sync::oneshot::channel();
    let rollback_started = Arc::new(Mutex::new(Some(rollback_started)));
    let rollback_finished_rx = Arc::new(Mutex::new(Some(rollback_finished_rx)));
    let operation_ready = Arc::new(Mutex::new(Some(operation_ready)));

    struct RollbackOnDrop {
        context: JobCtx,
        started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
        finished: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    }

    impl Drop for RollbackOnDrop {
        fn drop(&mut self) {
            let started = Arc::clone(&self.started);
            let finished = Arc::clone(&self.finished);
            self.context.track_cleanup(async move {
                if let Some(sender) = lock(&started).take() {
                    let _ignored = sender.send(());
                }
                let receiver = lock(&finished).take().expect("single rollback attempt");
                let _ignored = receiver.await;
            });
        }
    }

    let id = manager.submit(
        JobKind::Custom("transaction".to_owned()),
        "repo",
        "Transactional mutation",
        true,
        false,
        move |context| {
            let rollback_started = Arc::clone(&rollback_started);
            let rollback_finished_rx = Arc::clone(&rollback_finished_rx);
            let operation_ready = Arc::clone(&operation_ready);
            async move {
                let _rollback = RollbackOnDrop {
                    context,
                    started: rollback_started,
                    finished: rollback_finished_rx,
                };
                operation_ready
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .expect("operation starts once")
                    .send(())
                    .expect("test observes operation start");
                std::future::pending::<DaemonResult<()>>().await
            }
        },
    );
    operation_ready_rx.await.expect("operation starts");

    manager.cancel(&id).expect("cancel starts");
    rollback_started_rx.await.expect("rollback starts");
    assert_eq!(
        manager.record(&id).expect("record").status,
        JobStatus::Cancelling
    );
    rollback_finished.send(()).expect("release rollback");
    assert_eq!(
        wait_finished(&manager, &id).await.status,
        JobStatus::Cancelled
    );
}

#[tokio::test]
async fn stalled_rollback_is_bounded_and_visible() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    manager.set_cancellation_grace(StdDuration::ZERO);
    manager.set_cleanup_grace(StdDuration::ZERO);
    let (cleanup_registered, cleanup_registered_rx) = tokio::sync::oneshot::channel();
    let cleanup_registered = Arc::new(Mutex::new(Some(cleanup_registered)));
    let id = manager.submit(
        JobKind::Custom("transaction".to_owned()),
        "repo-with-stalled-rollback",
        "Transactional mutation",
        true,
        false,
        move |context| {
            let cleanup_registered = Arc::clone(&cleanup_registered);
            async move {
                context.track_cleanup(std::future::pending());
                cleanup_registered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .expect("cleanup registers once")
                    .send(())
                    .expect("test observes cleanup registration");
                std::future::pending::<DaemonResult<()>>().await
            }
        },
    );
    cleanup_registered_rx.await.expect("cleanup registers");

    manager.cancel(&id).expect("cancel starts");
    let record = wait_finished(&manager, &id).await;

    assert_eq!(record.status, JobStatus::Cancelled);
    assert_eq!(
        record.progress.as_deref(),
        Some("warning: cancellation cleanup exceeded its grace period")
    );
}

#[tokio::test]
async fn panicked_cleanup_does_not_wedge_cancellation() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    manager.set_cancellation_grace(StdDuration::ZERO);
    let (cleanup_started, cleanup_started_rx) = tokio::sync::oneshot::channel();
    let cleanup_started = Arc::new(Mutex::new(Some(cleanup_started)));
    let id = manager.submit(
        JobKind::Custom("transaction".to_owned()),
        "repo-with-panicking-cleanup",
        "Transactional mutation",
        true,
        false,
        move |context| {
            let cleanup_started = Arc::clone(&cleanup_started);
            async move {
                context.track_cleanup(async move {
                    if let Some(sender) = lock(&cleanup_started).take() {
                        let _ignored = sender.send(());
                    }
                    panic!("cleanup exploded");
                });
                std::future::pending::<DaemonResult<()>>().await
            }
        },
    );

    cleanup_started_rx.await.expect("cleanup starts");
    tokio::task::yield_now().await;
    manager.cancel(&id).expect("cancel starts");

    assert_eq!(
        wait_finished(&manager, &id).await.status,
        JobStatus::Cancelled
    );
}

#[tokio::test]
async fn panicked_job_finishes_and_releases_target() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    let first = manager.submit(
        JobKind::Inspect,
        "panic-target",
        "Panics",
        false,
        false,
        |_| async move { panic!("operation exploded") },
    );

    let failed = wait_finished(&manager, &first).await;
    assert!(matches!(
        failed.status,
        JobStatus::Failed { ref error } if error.contains("job panicked: operation exploded")
    ));

    let second = manager.submit(
        JobKind::Inspect,
        "panic-target",
        "Succeeds",
        false,
        false,
        |_| async move { Ok(()) },
    );
    assert_ne!(second, first);
    assert_eq!(
        wait_finished(&manager, &second).await.status,
        JobStatus::Succeeded
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lagged_waiter_observes_finished_job() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    let id = manager.submit(
        JobKind::Inspect,
        "lagged-target",
        "Target",
        false,
        false,
        |_| async move {
            std::future::pending::<()>().await;
            Ok(())
        },
    );
    tokio::task::yield_now().await;

    let waiting_manager = manager.clone();
    let waiting_id = id.clone();
    let waiter = tokio::spawn(async move { waiting_manager.wait(&waiting_id).await });
    tokio::task::yield_now().await;

    manager.finish(&id, Ok(()));
    let unrelated = manager.submit(
        JobKind::Custom("noise".to_owned()),
        "noise",
        "Noise",
        false,
        false,
        |_| async move {
            std::future::pending::<()>().await;
            Ok(())
        },
    );
    for index in 0..300 {
        manager
            .record_progress(&unrelated, format!("update {index}"))
            .expect("record noise");
    }

    let record = tokio::time::timeout(StdDuration::from_secs(1), waiter)
        .await
        .expect("lagged waiter completes")
        .expect("wait task")
        .expect("retained target");
    assert_eq!(record.status, JobStatus::Succeeded);
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
async fn deletion_rejects_typed_submissions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    let repo = RepoId::try_from("acme/api").expect("repo id");
    let _deletion = manager.begin_repo_deletion(&repo).expect("deletion guard");

    let error = manager
        .submit_for_repo(
            repo,
            JobKind::Inspect,
            "inspect-with-uuid-target",
            "Inspect",
            JobPolicy::new(true, false),
            |_| async { Ok(()) },
        )
        .expect_err("typed submission must be rejected");

    assert!(matches!(error, DaemonError::Conflict(message) if message.contains("being deleted")));
    assert!(manager.list().is_empty());
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

#[tokio::test]
async fn failed_jobs_do_not_expire_with_success_retention() {
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
        |_context| async { Err(DaemonError::Git("failed".to_owned())) },
    );
    assert!(matches!(
        wait_finished(&manager, &id).await.status,
        JobStatus::Failed { .. }
    ));
    clock.set(initial + Duration::seconds(2));

    assert!(manager.list().iter().any(|record| record.id == id));
}

#[tokio::test]
async fn extreme_retention_does_not_overflow_date_arithmetic() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    manager.set_retention(StdDuration::MAX);
    let id = manager.submit(
        JobKind::Inspect,
        "all",
        "Inspect",
        false,
        false,
        |_| async { Ok(()) },
    );
    manager.wait(&id).await.expect("finished");
    assert_eq!(manager.list().len(), 1);
}

#[tokio::test]
async fn quiescing_reports_a_job_it_cannot_stop() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    manager.set_quiesce_timeout(StdDuration::from_millis(50));
    let repo = RepoId::try_from("acme/api").expect("repo id");
    let _id = manager
        .submit_for_repo(
            repo.clone(),
            JobKind::PostCreateHooks,
            "acme/api#feature",
            "Post-create hooks",
            JobPolicy::new(false, false),
            |_| async { std::future::pending::<DaemonResult<()>>().await },
        )
        .expect("submission");

    let error = tokio::time::timeout(StdDuration::from_secs(4), manager.quiesce_repo(&repo))
        .await
        .expect("quiesce must not wait for a job it cannot cancel")
        .expect_err("a non-cancellable job must be reported");

    assert!(
        matches!(&error, DaemonError::Conflict(message) if message.contains("acme/api#feature")),
        "{error}"
    );
    // The tombstone taken by the caller is released by its own guard, so the repository stays
    // usable once the conflict is reported.
    assert!(manager.ensure_repo_available(&repo).is_ok());
}

#[tokio::test]
async fn a_blocked_log_write_does_not_stall_the_job_registry() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manager = JobManager::new(temp.path());
    let id = manager.submit(JobKind::Inspect, "all", "Inspect", true, false, |_| async {
        std::future::pending::<DaemonResult<()>>().await
    });
    // A FIFO nobody reads parks the next log write inside `open(2)`, standing in for the slow
    // or networked filesystem this daemon's home may live on.
    let fifo = temp.path().join("blocking.log");
    let name = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(fifo.as_os_str()))
        .expect("fifo path");
    // SAFETY: the NUL-terminated path stays valid for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    {
        let mut state = lock(&manager.inner.state);
        let job = state.job_mut(&id).expect("submitted job");
        job.record.log_path = fifo.to_string_lossy().into_owned();
        *lock(&job.log) = None;
    }

    let writer = manager.clone();
    let logged = id.clone();
    let _blocked =
        std::thread::spawn(move || writer.record_progress(&logged, "blocked".to_owned()));
    tokio::time::sleep(StdDuration::from_millis(50)).await;

    let (sender, receiver) = std::sync::mpsc::channel();
    let probe = manager.clone();
    let _prober = std::thread::spawn(move || {
        let _ignored = sender.send(probe.list().len());
    });
    let listed = receiver
        .recv_timeout(StdDuration::from_secs(2))
        .expect("listing jobs must not block behind a job log write");
    assert_eq!(listed, 1);
}
