//! Scheduled agent tasks: the store, the headless runner, the firing loop, and the CRUD verbs
//! the wire serves (`docs/BOARD.md` §12).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, SecondsFormat, Utc};
use fleet_core::{
    ids::{BoardId, JobId, ScheduleId, WorktreeId, new_schedule_id},
    paths::FleetHome,
    schedule::{
        Schedule, ScheduleDraft, ScheduleError, SchedulePatch, ScheduleRun, SchedulesDocument,
        apply_draft, apply_patch, next_run_at, validate_schedule,
    },
};
use fleet_proto::event::Event;
use tokio::sync::Notify;

use crate::{
    DaemonError, DaemonResult, adapters::clock::Clock, jobs::JobManager, server::BroadcastBus,
    stores::schedules::ScheduleStore,
};

use super::boards::Boards;

pub mod runner;
mod tick;

#[cfg(test)]
mod tests;

pub use runner::{HeadlessRunner, RunResult, ScheduleRunner};

/// The run this daemon owns for one schedule, before or after its job is submitted.
#[derive(Clone)]
struct LiveRun {
    started_at: Option<String>,
    job: Option<JobId>,
}

/// The live run of each schedule currently owned by this daemon.
type LiveRuns = std::sync::Mutex<BTreeMap<ScheduleId, LiveRun>>;

#[cfg(test)]
#[derive(Default)]
struct FirePause {
    enabled: std::sync::atomic::AtomicBool,
    reached: Notify,
    resume: Notify,
}

#[cfg(test)]
impl FirePause {
    /// Stops the fire here, once enabled, until the test resumes it.
    async fn pause(&self) {
        use std::sync::atomic::Ordering;

        if self.enabled.load(Ordering::SeqCst) {
            self.reached.notify_one();
            self.resume.notified().await;
        }
    }
}

/// Owns the schedule store and the runner, and fires due schedules as `ScheduledTask` jobs.
#[derive(Clone)]
pub struct Schedules {
    store: Arc<ScheduleStore>,
    runner: Arc<dyn ScheduleRunner>,
    jobs: Arc<JobManager>,
    clock: Arc<dyn Clock>,
    events: BroadcastBus,
    /// Used to check that a schedule's board exists.
    boards: Arc<Boards>,
    /// Where each schedule's work and log directories live.
    home: FleetHome,
    /// The live run of each schedule currently owned by this daemon.
    live: Arc<LiveRuns>,
    /// Woken by every mutation so the firing loop re-plans at once.
    changed: Arc<Notify>,
    /// Deterministic test seam for deleting a schedule after its run is recorded but before submit.
    #[cfg(test)]
    fire_pause: Arc<FirePause>,
    /// Stops a fire just after its job is attached to the run.
    #[cfg(test)]
    attach_pause: Arc<FirePause>,
}

impl Schedules {
    /// Composes the service around its store, runner and the daemon's shared resources.
    #[must_use]
    pub fn new(
        store: Arc<ScheduleStore>,
        runner: Arc<dyn ScheduleRunner>,
        jobs: Arc<JobManager>,
        clock: Arc<dyn Clock>,
        events: BroadcastBus,
        boards: Arc<Boards>,
    ) -> Self {
        // `schedules.json` sits at the root of the Fleet home, so its parent is the home the
        // per-schedule directories hang off.
        let home = FleetHome::new(
            store
                .path()
                .parent()
                .map_or_else(PathBuf::new, Path::to_path_buf),
        );
        Self {
            store,
            runner,
            jobs,
            clock,
            events,
            boards,
            home,
            live: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            changed: Arc::new(Notify::new()),
            #[cfg(test)]
            fire_pause: Arc::new(FirePause::default()),
            #[cfg(test)]
            attach_pause: Arc::new(FirePause::default()),
        }
    }

    /// Schedules of one board, or of every board when `board` is `None`.
    pub async fn list(&self, board: Option<&BoardId>) -> DaemonResult<Vec<Schedule>> {
        let document = self.store.load().await?;
        Ok(document
            .schedules
            .into_iter()
            .filter(|schedule| board.is_none_or(|board| schedule.board_id == *board))
            .collect())
    }

    /// Creates a schedule on an existing board.
    pub async fn create(&self, draft: ScheduleDraft) -> DaemonResult<Schedule> {
        match self.boards.get(&draft.board_id).await {
            Ok(_) => {}
            Err(DaemonError::NotFound(_)) => {
                return Err(schedule_error(ScheduleError::Invalid {
                    field: "board_id".to_owned(),
                    reason: format!("no board {}", draft.board_id),
                }));
            }
            Err(error) => return Err(error),
        }
        let now = self.clock.now();
        let stamp = timestamp(now);
        let created = self
            .store
            .transaction(move |document| {
                let mut id = new_schedule_id();
                while find(document, &id).is_some() {
                    id = new_schedule_id();
                }
                let mut schedule = apply_draft(draft, id, &stamp);
                validate_schedule(&schedule).map_err(schedule_error)?;
                replan(&mut schedule, now);
                document.schedules.push(schedule.clone());
                Ok(schedule)
            })
            .await?;
        self.published(&created.board_id);
        Ok(created)
    }

    /// Changes a schedule and re-plans its next fire.
    pub async fn update(&self, id: &ScheduleId, patch: SchedulePatch) -> DaemonResult<Schedule> {
        let now = self.clock.now();
        let id = id.clone();
        let updated = self
            .store
            .transaction(move |document| {
                let schedule = find_mut(document, &id)?;
                apply_patch(schedule, patch, &timestamp(now));
                validate_schedule(schedule).map_err(schedule_error)?;
                replan(schedule, now);
                Ok(schedule.clone())
            })
            .await?;
        self.published(&updated.board_id);
        Ok(updated)
    }

    /// Deletes a schedule and its directory, cancelling its live run.
    pub async fn delete(&self, id: &ScheduleId) -> DaemonResult<()> {
        let target = id.clone();
        let removed = self
            .store
            .transaction(move |document| {
                let index = document
                    .schedules
                    .iter()
                    .position(|schedule| schedule.id == target)
                    .ok_or_else(|| schedule_error(ScheduleError::NotFound(target.to_string())))?;
                Ok(document.schedules.remove(index))
            })
            .await?;
        self.forget(&removed.id).await;
        self.published(&removed.board_id);
        Ok(())
    }

    /// Deletes every schedule of a board and their directories; the board-delete cascade.
    pub async fn delete_for_board(&self, board: &BoardId) -> DaemonResult<()> {
        // Read first so a board without schedules never writes (or creates) `schedules.json`.
        let owned = self
            .store
            .load()
            .await?
            .schedules
            .iter()
            .any(|schedule| schedule.board_id == *board);
        if !owned {
            return Ok(());
        }
        let target = board.clone();
        let removed = self
            .store
            .transaction(move |document| {
                let (removed, kept) = std::mem::take(&mut document.schedules)
                    .into_iter()
                    .partition::<Vec<_>, _>(|schedule| schedule.board_id == target);
                document.schedules = kept;
                Ok(removed)
            })
            .await?;
        for schedule in &removed {
            self.forget(&schedule.id).await;
        }
        if !removed.is_empty() {
            self.published(board);
        }
        Ok(())
    }

    /// Fires a schedule now, whatever its cadence.
    ///
    /// The fire runs in its own task so dropping the request cannot cancel it halfway. The answer
    /// is truncated after the run this request recorded, so that run is always its newest one.
    pub async fn run_now(&self, id: &ScheduleId) -> DaemonResult<Schedule> {
        let schedules = self.clone();
        let target = id.clone();
        let join = tokio::spawn(async move { schedules.fire(&target).await });
        let started_at = match join.await {
            Ok(result) => result?,
            Err(error) => {
                tracing::warn!(%error, schedule = %id, "schedule fire task failed");
                return Err(DaemonError::Join(error.to_string()));
            }
        };
        let mut schedule = self
            .store
            .load()
            .await?
            .schedules
            .into_iter()
            .find(|schedule| schedule.id == *id)
            .ok_or_else(|| schedule_error(ScheduleError::NotFound(id.to_string())))?;
        let index = schedule
            .runs
            .iter()
            .position(|run| run.started_at == started_at)
            .ok_or_else(|| {
                DaemonError::NotFound(format!("run of schedule {id} started at {started_at}"))
            })?;
        schedule.runs.truncate(index + 1);
        Ok(schedule)
    }

    /// The recorded runs of one schedule, oldest first.
    pub async fn runs(&self, id: &ScheduleId) -> DaemonResult<Vec<ScheduleRun>> {
        self.store
            .load()
            .await?
            .schedules
            .into_iter()
            .find(|schedule| schedule.id == *id)
            .map(|schedule| schedule.runs)
            .ok_or_else(|| schedule_error(ScheduleError::NotFound(id.to_string())))
    }

    /// Tells clients a board's schedules changed and wakes the firing loop to re-plan.
    fn published(&self, board: &BoardId) {
        self.events.publish(Event::SchedulesChanged {
            board_id: board.clone(),
        });
        // A stored permit: a mutation landing while the loop is busy still wakes its next wait.
        self.changed.notify_one();
    }

    /// Cancels a removed schedule's live run and deletes its directory.
    async fn forget(&self, id: &ScheduleId) {
        let live = lock(&self.live).get(id).and_then(|entry| entry.job.clone());
        if let Some(job) = live
            && let Err(error) = self.jobs.cancel(&job)
        {
            tracing::debug!(%error, schedule = %id, job = %job, "deleted schedule's run was not cancellable");
        }
        remove_dir(&self.home.schedule_dir(id)).await;
    }
}

/// The worktree cascade the daemon installs: the worktree's boards go, and then their schedules,
/// so a schedule never keeps firing at a board that went to the trash with its worktree
/// (`docs/BOARD.md` §12).
#[async_trait::async_trait]
impl super::worktrees::WorktreeCascade for Schedules {
    async fn delete_for_worktree(&self, worktree: &WorktreeId, trash: &Path) -> DaemonResult<()> {
        for board in self.boards.delete_for_worktree(worktree, trash).await? {
            // The board is already gone, so this cannot be rolled back; it is logged exactly as
            // the context and board deletes log it.
            if let Err(error) = self.delete_for_board(&board).await {
                tracing::warn!(%board, %error, "worktree board deleted but its schedules could not be");
            }
        }
        Ok(())
    }

    async fn restore_for_worktree(
        &self,
        worktree: &WorktreeId,
        destination: &Path,
    ) -> DaemonResult<()> {
        self.boards
            .restore_for_worktree(worktree, destination)
            .await
    }
}

/// RFC 3339 in UTC to the second, the form every schedule time is stored in.
fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The `started_at` of a run recorded at `now`: `now` to the second, moved on a second at a time
/// past any run of this schedule already holding it.
///
/// It is the run's key — the job id, the outcome and the log file are all matched by it — so
/// two fires in one second (a double "Run now", a manual run while the loop fires) must not
/// share one: the second would take the first one's job, and both would write one log.
fn run_stamp(schedule: &Schedule, now: DateTime<Utc>) -> String {
    let mut at = now;
    loop {
        let stamp = timestamp(at);
        if !schedule.runs.iter().any(|run| run.started_at == stamp) {
            return stamp;
        }
        at += chrono::Duration::seconds(1);
    }
}

/// Stores the schedule's next fire as of `now`.
fn replan(schedule: &mut Schedule, now: DateTime<Utc>) {
    schedule.next_run_at = next_run_at(schedule, now).map(timestamp);
}

fn find<'a>(document: &'a SchedulesDocument, id: &ScheduleId) -> Option<&'a Schedule> {
    document
        .schedules
        .iter()
        .find(|schedule| schedule.id == *id)
}

fn find_mut<'a>(
    document: &'a mut SchedulesDocument,
    id: &ScheduleId,
) -> DaemonResult<&'a mut Schedule> {
    document
        .schedules
        .iter_mut()
        .find(|schedule| schedule.id == *id)
        .ok_or_else(|| schedule_error(ScheduleError::NotFound(id.to_string())))
}

fn schedule_error(error: ScheduleError) -> DaemonError {
    match error {
        ScheduleError::Invalid { .. } => DaemonError::Validation(error.to_string()),
        ScheduleError::NotFound(id) => DaemonError::NotFound(format!("schedule {id}")),
    }
}

fn lock(live: &LiveRuns) -> std::sync::MutexGuard<'_, BTreeMap<ScheduleId, LiveRun>> {
    live.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Removes a directory tree; one that is already gone is not an error.
async fn remove_dir(path: &Path) {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "failed to remove schedule directory");
        }
    }
}

/// Removes the log files of runs `push_run` dropped; one that is already gone is not an error.
async fn remove_logs(paths: Vec<String>) {
    for path in paths {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, %path, "failed to remove a dropped schedule run log");
            }
        }
    }
}
