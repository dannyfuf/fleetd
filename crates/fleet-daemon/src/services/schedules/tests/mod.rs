//! The firing loop and the CRUD verbs, driven on a paused tokio clock with a fixed wall clock
//! and a fake runner.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use fleet_core::{
    ids::{BoardId, ScheduleId},
    model::Context,
    paths::FleetHome,
    schedule::{Cadence, ScheduleDraft, ScheduleOutcome, SchedulePatch, ScheduleRun},
    state::default_state,
};
use fleet_proto::event::Event;
use fleet_proto::job::{JobKind, JobStatus};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::{
    LiveRun, RunResult, ScheduleRunner, Schedules,
    tick::{INTERRUPTED_SUMMARY, MAX_WAIT, SKIPPED_SUMMARY},
};
use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::BroadcastBus,
    services::Services,
    stores::{config::ConfigStore, schedules::ScheduleStore, state::StateStore},
    testing::fakes::FixedClock,
};

/// Records every run; when gated, each run waits for one permit before it finishes.
struct FakeRunner {
    calls: Mutex<Vec<(ScheduleId, String, tokio::time::Instant)>>,
    gate: Option<Semaphore>,
}

impl FakeRunner {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            gate: None,
        }
    }

    fn gated() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            gate: Some(Semaphore::new(0)),
        }
    }

    fn calls(&self) -> Vec<(ScheduleId, String, tokio::time::Instant)> {
        self.calls.lock().expect("fake runner calls lock").clone()
    }

    fn release(&self) {
        if let Some(gate) = &self.gate {
            gate.add_permits(1);
        }
    }
}

#[async_trait]
impl ScheduleRunner for FakeRunner {
    async fn run(
        &self,
        schedule: &fleet_core::schedule::Schedule,
        prompt: &str,
        cancel: CancellationToken,
    ) -> RunResult {
        self.calls.lock().expect("fake runner calls lock").push((
            schedule.id.clone(),
            prompt.to_owned(),
            tokio::time::Instant::now(),
        ));
        if let Some(gate) = &self.gate {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    return RunResult {
                        outcome: ScheduleOutcome::Failed,
                        summary: Some("canceled".to_owned()),
                        cost_usd: None,
                    };
                }
                permit = gate.acquire() => permit.expect("gate stays open").forget(),
            }
        }
        RunResult {
            outcome: ScheduleOutcome::Succeeded,
            summary: Some("1 created, 0 existing, 0 reopened".to_owned()),
            cost_usd: Some(0.25),
        }
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    _services: Services,
    home: FleetHome,
    events: BroadcastBus,
    board: BoardId,
    runner: Arc<FakeRunner>,
    schedules: Schedules,
    now: DateTime<Utc>,
}

async fn fixture(runner: FakeRunner) -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = FleetHome::new(temp.path());
    let files = Arc::new(RealFiles::new(
        home.trash_dir(),
        [home.boards_dir(), home.repos_dir(), home.worktrees_dir()],
    ));
    let state = Arc::new(StateStore::new(
        temp.path(),
        files.clone(),
        Arc::new(SystemClock),
    ));
    let mut initial = default_state();
    initial.contexts.push(Context {
        id: "work".parse().expect("context id"),
        name: "Work".into(),
        owners: vec![],
        created_at: "now".into(),
    });
    state.save(initial).await.expect("save state");
    let events = BroadcastBus::default();
    let services = Services::build(
        temp.path().to_path_buf(),
        Arc::new(ConfigStore::new(temp.path(), files.clone())),
        state,
        Arc::new(JobManager::new(temp.path())),
        Adapters::system(files.clone()),
        events.clone(),
    );
    let board = services
        .boards
        .ensure(&"work".parse().expect("context id"))
        .await
        .expect("board")
        .board
        .id;
    let now = Utc
        .with_ymd_and_hms(2026, 9, 24, 10, 0, 0)
        .single()
        .expect("fixed time");
    let clock: Arc<FixedClock> = Arc::new(FixedClock::new(now));
    let runner = Arc::new(runner);
    let schedules = Schedules::new(
        Arc::new(ScheduleStore::new(&home, files, clock.clone())),
        runner.clone(),
        Arc::clone(&services.jobs),
        clock,
        events.clone(),
        Arc::clone(&services.boards),
    );
    Fixture {
        _temp: temp,
        _services: services,
        home,
        events,
        board,
        runner,
        schedules,
        now,
    }
}

fn draft(board: &BoardId, cadence: Cadence) -> ScheduleDraft {
    ScheduleDraft {
        board_id: board.clone(),
        name: "GitHub reviews".to_owned(),
        prompt: "Find reviews for {board} since {last_run_at}.".to_owned(),
        cadence,
        agent: None,
        enabled: None,
        timeout_minutes: None,
    }
}

fn every(minutes: u32) -> Cadence {
    Cadence::Every { minutes }
}

fn once(at: DateTime<Utc>) -> Cadence {
    Cadence::Once {
        at: at.to_rfc3339(),
    }
}

/// Polls `check` on the paused clock until it holds; a missing effect fails the test.
async fn until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..1_000 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn runs(schedules: &Schedules, id: &ScheduleId) -> Vec<ScheduleRun> {
    schedules.runs(id).await.expect("runs")
}

async fn finished(schedules: &Schedules, id: &ScheduleId, count: usize) -> bool {
    let runs = runs(schedules, id).await;
    runs.len() == count && runs.iter().all(|run| run.outcome.is_some())
}

fn start_loop(schedules: &Schedules) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(schedules.clone().run_schedules(shutdown.clone()));
    (shutdown, handle)
}

async fn stop(shutdown: CancellationToken, handle: tokio::task::JoinHandle<()>) {
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("the loop stops within the join timeout")
        .expect("the loop does not panic");
}

#[tokio::test(start_paused = true)]
async fn a_due_schedule_fires_once_and_records_its_run() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(5)))
        .await
        .expect("create");
    assert_eq!(created.next_run_at.as_deref(), Some("2026-09-24T10:00:00Z"));
    let (shutdown, handle) = start_loop(&fixture.schedules);

    until("the run to finish", || {
        finished(&fixture.schedules, &created.id, 1)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(600)).await;

    let calls = fixture.runner.calls();
    assert_eq!(calls.len(), 1, "fired exactly once");
    assert!(
        calls[0]
            .1
            .starts_with(&format!("Find reviews for {} since never.", fixture.board))
    );
    let run = &runs(&fixture.schedules, &created.id).await[0];
    assert_eq!(run.outcome, Some(ScheduleOutcome::Succeeded));
    assert_eq!(
        run.summary.as_deref(),
        Some("1 created, 0 existing, 0 reopened")
    );
    assert_eq!(run.cost_usd, Some(0.25));
    assert_eq!(run.started_at, "2026-09-24T10:00:00Z");
    assert!(run.ended_at.is_some());
    assert!(run.job_id.is_some());
    let expected_log = fixture
        .home
        .schedule_log_path(&created.id, "2026-09-24T10:00:00Z");
    assert_eq!(
        run.log_path.as_deref(),
        Some(expected_log.to_string_lossy().as_ref())
    );
    let schedule = &fixture
        .schedules
        .list(Some(&fixture.board))
        .await
        .expect("list")[0];
    assert_eq!(
        schedule.next_run_at.as_deref(),
        Some("2026-09-24T10:05:00Z")
    );
    stop(shutdown, handle).await;
}

#[tokio::test(start_paused = true)]
async fn a_schedule_not_yet_due_does_not_fire() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(
            &fixture.board,
            once(fixture.now + chrono::Duration::hours(1)),
        ))
        .await
        .expect("create");
    let (shutdown, handle) = start_loop(&fixture.schedules);

    tokio::time::sleep(Duration::from_secs(600)).await;

    assert!(fixture.runner.calls().is_empty());
    assert!(runs(&fixture.schedules, &created.id).await.is_empty());
    stop(shutdown, handle).await;
}

#[tokio::test(start_paused = true)]
async fn a_fire_while_the_previous_run_is_live_is_skipped() {
    let fixture = fixture(FakeRunner::gated()).await;
    let created = fixture
        .schedules
        .create(draft(
            &fixture.board,
            once(fixture.now + chrono::Duration::hours(1)),
        ))
        .await
        .expect("create");

    let started = fixture.schedules.run_now(&created.id).await.expect("run");
    assert_eq!(started.runs.len(), 1);
    assert_eq!(started.runs[0].outcome, None);
    until("the runner to start", || async {
        fixture.runner.calls().len() == 1
    })
    .await;
    let skipped = fixture.schedules.run_now(&created.id).await.expect("run");

    assert_eq!(fixture.runner.calls().len(), 1, "no second run starts");
    let last = skipped.runs.last().expect("skipped run");
    assert_eq!(last.outcome, Some(ScheduleOutcome::Skipped));
    assert_eq!(last.summary.as_deref(), Some(SKIPPED_SUMMARY));
    assert_eq!(last.job_id, None);
    // The clock is fixed, so the skip came in the same second as the live run: its key is moved
    // on rather than shared, and the live run keeps its own job.
    assert_eq!(skipped.runs[0].started_at, "2026-09-24T10:00:00Z");
    assert_eq!(last.started_at, "2026-09-24T10:00:01Z");
    assert!(skipped.runs[0].job_id.is_some(), "{:?}", skipped.runs);

    fixture.runner.release();
    until("the live run to finish", || {
        finished(&fixture.schedules, &created.id, 2)
    })
    .await;
    let recorded = runs(&fixture.schedules, &created.id).await;
    assert_eq!(recorded[0].outcome, Some(ScheduleOutcome::Succeeded));
    assert!(recorded[0].job_id.is_some());
    assert_eq!(recorded[1].job_id, None);
}

#[tokio::test(start_paused = true)]
async fn dropping_a_fire_while_start_waits_releases_its_live_reservation() {
    let fixture = fixture(FakeRunner::gated()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    let gate = fixture.schedules.store.hold_gate().await;
    let schedules = fixture.schedules.clone();
    let id = created.id.clone();
    let fire = tokio::spawn(async move { schedules.fire(&id).await });
    for _ in 0..100 {
        if super::lock(&fixture.schedules.live).contains_key(&created.id) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(super::lock(&fixture.schedules.live).contains_key(&created.id));

    fire.abort();
    fire.await.expect_err("fire is dropped at the store await");
    assert!(!super::lock(&fixture.schedules.live).contains_key(&created.id));
    drop(gate);

    let answer = fixture
        .schedules
        .run_now(&created.id)
        .await
        .expect("the next fire starts");
    let run = answer.runs.last().expect("the next fire's run");
    assert_eq!(run.outcome, None);
    assert!(run.job_id.is_some());
    fixture.runner.release();
    until("the replacement fire to finish", || {
        finished(&fixture.schedules, &created.id, 1)
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn dropping_a_run_now_request_does_not_cancel_its_spawned_fire() {
    use std::sync::atomic::Ordering;

    let fixture = fixture(FakeRunner::gated()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    fixture
        .schedules
        .fire_pause
        .enabled
        .store(true, Ordering::SeqCst);
    let reached = fixture.schedules.fire_pause.reached.notified();
    let schedules = fixture.schedules.clone();
    let id = created.id.clone();
    let request = tokio::spawn(async move { schedules.run_now(&id).await });
    reached.await;

    request.abort();
    request.await.expect_err("request future is dropped");
    fixture
        .schedules
        .fire_pause
        .enabled
        .store(false, Ordering::SeqCst);
    fixture.schedules.fire_pause.resume.notify_one();
    until("the independent fire to reach the runner", || async {
        fixture.runner.calls().len() == 1
    })
    .await;
    let recorded = runs(&fixture.schedules, &created.id).await;
    assert_eq!(recorded.len(), 1);
    assert_ne!(recorded[0].outcome, Some(ScheduleOutcome::Skipped));

    fixture.runner.release();
    until("the independent fire to finish", || {
        finished(&fixture.schedules, &created.id, 1)
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_deduplicated_never_started_run_clears_its_job_and_log() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    let old_job = fixture.schedules.jobs.submit(
        JobKind::ScheduledTask,
        created.id.to_string(),
        "old scheduled run",
        true,
        false,
        |context| async move {
            context.cancel.cancelled().await;
            Err(crate::DaemonError::Cancelled)
        },
    );
    let id = created.id.clone();
    let recorded_job = old_job.clone();
    fixture
        .schedules
        .store
        .transaction(move |document| {
            let schedule = super::find_mut(document, &id)?;
            schedule.runs.push(ScheduleRun {
                job_id: Some(recorded_job),
                started_at: "2026-09-24T09:45:00Z".to_owned(),
                ended_at: Some("2026-09-24T09:46:00Z".to_owned()),
                outcome: Some(ScheduleOutcome::Succeeded),
                summary: None,
                cost_usd: None,
                log_path: Some("/old.log".to_owned()),
            });
            Ok(())
        })
        .await
        .expect("seed previous run");

    let answer = fixture
        .schedules
        .run_now(&created.id)
        .await
        .expect("deduplicated fire is recorded");
    let run = answer.runs.last().expect("new run");
    assert_eq!(run.outcome, Some(ScheduleOutcome::Skipped));
    assert_eq!(run.job_id, None);
    assert_eq!(run.log_path, None);

    fixture
        .schedules
        .jobs
        .cancel(&old_job)
        .expect("cancel old job");
    fixture
        .schedules
        .jobs
        .wait(&old_job)
        .await
        .expect("old job stops");
}

#[tokio::test(start_paused = true)]
async fn deleting_between_start_and_submit_cancels_the_submitted_job() {
    use std::sync::atomic::Ordering;

    let fixture = fixture(FakeRunner::gated()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    fixture
        .schedules
        .fire_pause
        .enabled
        .store(true, Ordering::SeqCst);
    let reached = fixture.schedules.fire_pause.reached.notified();
    let schedules = fixture.schedules.clone();
    let id = created.id.clone();
    let request = tokio::spawn(async move { schedules.run_now(&id).await });
    reached.await;

    fixture
        .schedules
        .delete(&created.id)
        .await
        .expect("delete in submit window");
    fixture
        .schedules
        .fire_pause
        .enabled
        .store(false, Ordering::SeqCst);
    fixture.schedules.fire_pause.resume.notify_one();
    assert!(request.await.expect("request task").is_err());

    until("the submitted job to be cancelled", || async {
        fixture
            .schedules
            .jobs
            .list()
            .iter()
            .any(|job| job.kind == JobKind::ScheduledTask && job.status == JobStatus::Cancelled)
    })
    .await;
    assert!(!fixture.home.schedule_dir(&created.id).exists());
}

/// A schedule whose board went without it — a create racing the board's delete, or a cascade
/// that failed — launches nothing when it fires: it records why and is disabled.
#[tokio::test(start_paused = true)]
async fn a_schedule_whose_board_is_gone_is_disabled_when_it_fires() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    // The board alone, without the dispatch cascade that would take its schedules too.
    fixture
        ._services
        .boards
        .delete(&fixture.board)
        .await
        .expect("the board is deleted");

    let answer = fixture
        .schedules
        .run_now(&created.id)
        .await
        .expect("the orphaned fire is recorded");

    assert!(fixture.runner.calls().is_empty());
    assert!(!answer.enabled);
    let run = answer.runs.last().expect("the refused fire's run");
    assert_eq!(run.outcome, Some(ScheduleOutcome::Failed));
    assert_eq!(
        run.summary.as_deref(),
        Some(
            format!(
                "board {} no longer exists; the schedule is disabled",
                fixture.board
            )
            .as_str()
        )
    );
}

/// A delete that lands just after the fire attached its job still finds that job and cancels it.
#[tokio::test(start_paused = true)]
async fn deleting_just_after_the_job_is_attached_cancels_it() {
    use std::sync::atomic::Ordering;

    let fixture = fixture(FakeRunner::gated()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    fixture
        .schedules
        .attach_pause
        .enabled
        .store(true, Ordering::SeqCst);
    let reached = fixture.schedules.attach_pause.reached.notified();
    let schedules = fixture.schedules.clone();
    let id = created.id.clone();
    let request = tokio::spawn(async move { schedules.run_now(&id).await });
    reached.await;

    fixture
        .schedules
        .delete(&created.id)
        .await
        .expect("delete after the attach");
    fixture
        .schedules
        .attach_pause
        .enabled
        .store(false, Ordering::SeqCst);
    fixture.schedules.attach_pause.resume.notify_one();
    assert!(request.await.expect("request task").is_err());

    until("the attached job to be cancelled", || async {
        fixture
            .schedules
            .jobs
            .list()
            .iter()
            .any(|job| job.kind == JobKind::ScheduledTask && job.status == JobStatus::Cancelled)
    })
    .await;
    assert!(!fixture.home.schedule_dir(&created.id).exists());
}

#[tokio::test(start_paused = true)]
async fn concurrent_run_now_answers_each_end_with_their_own_run() {
    let fixture = fixture(FakeRunner::gated()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");

    let (first, second) = tokio::join!(
        fixture.schedules.run_now(&created.id),
        fixture.schedules.run_now(&created.id)
    );
    let first = first.expect("first answer");
    let second = second.expect("second answer");
    let first_last = first.runs.last().expect("first run");
    let second_last = second.runs.last().expect("second run");
    assert_ne!(first_last.started_at, second_last.started_at);
    assert_eq!(
        [first.runs.len(), second.runs.len()].into_iter().min(),
        Some(1)
    );
    assert_eq!(
        [first.runs.len(), second.runs.len()].into_iter().max(),
        Some(2)
    );

    fixture.runner.release();
    until("the live concurrent run to finish", || {
        finished(&fixture.schedules, &created.id, 2)
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_disabled_schedule_never_fires() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(ScheduleDraft {
            enabled: Some(false),
            ..draft(&fixture.board, every(5))
        })
        .await
        .expect("create");
    assert_eq!(created.next_run_at, None);
    let (shutdown, handle) = start_loop(&fixture.schedules);

    tokio::time::sleep(Duration::from_secs(3_600)).await;

    assert!(fixture.runner.calls().is_empty());
    stop(shutdown, handle).await;
}

#[tokio::test(start_paused = true)]
async fn a_once_schedule_fires_once_and_then_has_no_next_run() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, once(fixture.now)))
        .await
        .expect("create");
    let (shutdown, handle) = start_loop(&fixture.schedules);

    until("the run to finish", || {
        finished(&fixture.schedules, &created.id, 1)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(600)).await;

    assert_eq!(fixture.runner.calls().len(), 1);
    let schedule = &fixture.schedules.list(None).await.expect("list")[0];
    assert_eq!(schedule.next_run_at, None);
    assert_eq!(schedule.runs.len(), 1);
    stop(shutdown, handle).await;
}

#[tokio::test(start_paused = true)]
async fn an_update_wakes_the_loop_without_waiting_a_minute() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(
            &fixture.board,
            once(fixture.now + chrono::Duration::days(1)),
        ))
        .await
        .expect("create");
    let (shutdown, handle) = start_loop(&fixture.schedules);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let updated_at = tokio::time::Instant::now();

    fixture
        .schedules
        .update(
            &created.id,
            SchedulePatch {
                cadence: Some(once(fixture.now)),
                ..SchedulePatch::default()
            },
        )
        .await
        .expect("update");
    until("the run to start", || async {
        !fixture.runner.calls().is_empty()
    })
    .await;

    let fired_at = fixture.runner.calls()[0].2;
    assert!(
        fired_at.duration_since(updated_at) < MAX_WAIT,
        "the update re-planned the loop at once"
    );
    stop(shutdown, handle).await;
}

#[tokio::test(start_paused = true)]
async fn a_run_interrupted_by_a_daemon_stop_is_marked_failed_on_start() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(ScheduleDraft {
            enabled: Some(false),
            ..draft(&fixture.board, every(5))
        })
        .await
        .expect("create");
    let id = created.id.clone();
    fixture
        .schedules
        .store
        .transaction(move |document| {
            let schedule = super::find_mut(document, &id)?;
            schedule.runs.push(ScheduleRun {
                job_id: None,
                started_at: "2026-09-24T09:00:00Z".to_owned(),
                ended_at: None,
                outcome: None,
                summary: None,
                cost_usd: None,
                log_path: None,
            });
            Ok(())
        })
        .await
        .expect("seed an interrupted run");
    let (shutdown, handle) = start_loop(&fixture.schedules);

    until("the interrupted run to be marked", || {
        finished(&fixture.schedules, &created.id, 1)
    })
    .await;

    let run = &runs(&fixture.schedules, &created.id).await[0];
    assert_eq!(run.outcome, Some(ScheduleOutcome::Failed));
    assert_eq!(run.summary.as_deref(), Some(INTERRUPTED_SUMMARY));
    assert_eq!(run.ended_at.as_deref(), Some("2026-09-24T10:00:00Z"));
    assert!(fixture.runner.calls().is_empty());
    stop(shutdown, handle).await;
}

#[tokio::test(start_paused = true)]
async fn recovery_does_not_mark_the_exact_live_run_owned_by_this_daemon() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    let id = created.id.clone();
    fixture
        .schedules
        .store
        .transaction(move |document| {
            super::find_mut(document, &id)?.runs.push(ScheduleRun {
                job_id: None,
                started_at: "2026-09-24T10:00:00Z".to_owned(),
                ended_at: None,
                outcome: None,
                summary: None,
                cost_usd: None,
                log_path: None,
            });
            Ok(())
        })
        .await
        .expect("seed live run");
    super::lock(&fixture.schedules.live).insert(
        created.id.clone(),
        LiveRun {
            started_at: Some("2026-09-24T10:00:00Z".to_owned()),
            job: None,
        },
    );

    fixture
        .schedules
        .recover_interrupted()
        .await
        .expect("recover");

    assert_eq!(runs(&fixture.schedules, &created.id).await[0].outcome, None);
}

#[tokio::test(start_paused = true)]
async fn recovery_marks_an_older_unfinished_run_on_a_live_schedule() {
    let fixture = fixture(FakeRunner::new()).await;
    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(15)))
        .await
        .expect("create");
    let id = created.id.clone();
    fixture
        .schedules
        .store
        .transaction(move |document| {
            let schedule = super::find_mut(document, &id)?;
            for started_at in ["2026-09-24T09:00:00Z", "2026-09-24T10:00:00Z"] {
                schedule.runs.push(ScheduleRun {
                    job_id: None,
                    started_at: started_at.to_owned(),
                    ended_at: None,
                    outcome: None,
                    summary: None,
                    cost_usd: None,
                    log_path: None,
                });
            }
            Ok(())
        })
        .await
        .expect("seed unfinished runs");
    super::lock(&fixture.schedules.live).insert(
        created.id.clone(),
        LiveRun {
            started_at: Some("2026-09-24T10:00:00Z".to_owned()),
            job: None,
        },
    );

    fixture
        .schedules
        .recover_interrupted()
        .await
        .expect("recover");

    let recorded = runs(&fixture.schedules, &created.id).await;
    assert_eq!(recorded[0].outcome, Some(ScheduleOutcome::Failed));
    assert_eq!(recorded[0].summary.as_deref(), Some(INTERRUPTED_SUMMARY));
    assert_eq!(
        recorded[0].ended_at.as_deref(),
        Some("2026-09-24T10:00:00Z")
    );
    assert_eq!(recorded[1].outcome, None);
}

#[tokio::test(start_paused = true)]
async fn deleting_a_board_deletes_its_schedules_and_their_directories() {
    let fixture = fixture(FakeRunner::new()).await;
    let first = fixture
        .schedules
        .create(ScheduleDraft {
            enabled: Some(false),
            ..draft(&fixture.board, every(5))
        })
        .await
        .expect("create");
    let second = fixture
        .schedules
        .create(ScheduleDraft {
            enabled: Some(false),
            ..draft(&fixture.board, every(60))
        })
        .await
        .expect("create");
    let work = fixture.home.schedule_work_dir(&first.id);
    std::fs::create_dir_all(&work).expect("work dir");
    let mut receiver = fixture.events.subscribe();

    fixture
        .schedules
        .delete_for_board(&fixture.board)
        .await
        .expect("cascade");

    assert!(fixture.schedules.list(None).await.expect("list").is_empty());
    assert!(!fixture.home.schedule_dir(&first.id).exists());
    assert!(fixture.schedules.runs(&second.id).await.is_err());
    assert_eq!(
        receiver.recv().await.expect("event"),
        Event::SchedulesChanged {
            board_id: fixture.board.clone()
        }
    );
    // A board with no schedules left is a no-op, not an error.
    fixture
        .schedules
        .delete_for_board(&fixture.board)
        .await
        .expect("empty cascade");
}

#[tokio::test(start_paused = true)]
async fn crud_publishes_changes_and_refuses_an_unknown_board() {
    let fixture = fixture(FakeRunner::new()).await;
    let mut receiver = fixture.events.subscribe();

    let refused = fixture
        .schedules
        .create(draft(&"missing".parse().expect("board id"), every(5)))
        .await
        .expect_err("unknown board");
    assert!(
        refused
            .to_string()
            .ends_with("invalid board_id: no board missing"),
        "{refused}"
    );

    let created = fixture
        .schedules
        .create(draft(&fixture.board, every(5)))
        .await
        .expect("create");
    let updated = fixture
        .schedules
        .update(
            &created.id,
            SchedulePatch {
                name: Some("Renamed".to_owned()),
                ..SchedulePatch::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.name, "Renamed");
    let invalid = fixture
        .schedules
        .update(
            &created.id,
            SchedulePatch {
                cadence: Some(every(1)),
                ..SchedulePatch::default()
            },
        )
        .await
        .expect_err("too frequent");
    assert!(
        invalid
            .to_string()
            .ends_with("every must be between 5 and 1440 minutes")
    );
    fixture.schedules.delete(&created.id).await.expect("delete");
    assert!(fixture.schedules.delete(&created.id).await.is_err());

    for _ in 0..3 {
        assert_eq!(
            receiver.recv().await.expect("event"),
            Event::SchedulesChanged {
                board_id: fixture.board.clone()
            }
        );
    }
    assert!(receiver.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_the_loop_within_the_join_timeout() {
    let fixture = fixture(FakeRunner::new()).await;
    let (shutdown, handle) = start_loop(&fixture.schedules);
    tokio::time::sleep(Duration::from_secs(1)).await;
    stop(shutdown, handle).await;
}

/// A worktree's deletion takes its board to the trash, and the board's schedules with it: none
/// keeps firing at a board that is gone.
#[tokio::test(start_paused = true)]
async fn deleting_a_worktree_deletes_its_board_schedules() {
    use crate::services::worktrees::WorktreeCascade;

    let fixture = fixture(FakeRunner::new()).await;
    let worktree: fleet_core::ids::WorktreeId = "acme/api#feature".parse().expect("worktree id");
    fixture
        ._services
        .state
        .transaction(|state| {
            state.repos.push(
                serde_json::from_value(serde_json::json!({
                    "id": "acme/api", "owner": "acme", "name": "api",
                    "url": "https://example.invalid/acme/api.git", "contextId": "work",
                    "defaultBranch": "main", "path": "/tmp/acme-api", "clonedAt": "now"
                }))
                .expect("a repo from its wire fields"),
            );
            state.worktrees.push(
                serde_json::from_value(serde_json::json!({
                    "id": "acme/api#feature", "repoId": "acme/api", "slug": "feature",
                    "branch": "feature", "baseRef": "origin/main", "path": "/tmp/acme-api-feature",
                    "session": "api/feature", "createdAt": "now"
                }))
                .expect("a worktree from its wire fields"),
            );
            Ok(())
        })
        .await
        .expect("seed the worktree");
    let board = fixture
        ._services
        .boards
        .ensure_for_worktree(&worktree)
        .await
        .expect("the worktree board")
        .board
        .id;
    let doomed = fixture
        .schedules
        .create(ScheduleDraft {
            enabled: Some(false),
            ..draft(&board, every(5))
        })
        .await
        .expect("a schedule on the worktree board");
    let kept = fixture
        .schedules
        .create(ScheduleDraft {
            enabled: Some(false),
            ..draft(&fixture.board, every(5))
        })
        .await
        .expect("a schedule on the context board");
    let trash = fixture.home.trash_dir().join("acme-api-feature");
    std::fs::create_dir_all(&trash).expect("trash dir");

    fixture
        .schedules
        .delete_for_worktree(&worktree, &trash)
        .await
        .expect("the cascade");

    let left: Vec<ScheduleId> = fixture
        .schedules
        .list(None)
        .await
        .expect("list")
        .into_iter()
        .map(|schedule| schedule.id)
        .collect();
    assert_eq!(left, vec![kept.id]);
    assert!(fixture.schedules.runs(&doomed.id).await.is_err());
}
