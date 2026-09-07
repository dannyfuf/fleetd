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
