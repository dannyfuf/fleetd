//! The firing loop: sleeps until the next due schedule, fires it as a `ScheduledTask` job, and
//! records the run.

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use fleet_core::{
    ids::{BoardId, ScheduleId},
    schedule::{Schedule, ScheduleOutcome, ScheduleRun, next_run_at, push_run, render_prompt},
};
use fleet_proto::job::JobKind;
use tokio_util::sync::CancellationToken;

use crate::{DaemonError, DaemonResult, services::maintenance::RepeatedFailure};

use super::{
    LiveRun, LiveRuns, RunResult, Schedules, find_mut, lock, remove_dir, remove_logs, replan,
    run_stamp, timestamp,
};

/// Longest the loop sleeps, so a wall-clock jump or a machine sleep is noticed within a minute.
pub(super) const MAX_WAIT: Duration = Duration::from_secs(60);
/// Summary of a fire that came while the schedule's previous run was still live.
pub(super) const SKIPPED_SUMMARY: &str = "the previous run was still going";
/// Summary of a run the daemon found unfinished when it started.
pub(super) const INTERRUPTED_SUMMARY: &str = "the daemon stopped during this run";
/// Summary of a run whose job ended without reporting (cancelled past its grace, or panicked).
pub(super) const STOPPED_SUMMARY: &str = "the run stopped before it finished";

impl Schedules {
    /// Fires every due schedule until `shutdown` is cancelled.
    pub async fn run_schedules(self, shutdown: CancellationToken) {
        if let Err(error) = self.recover_interrupted().await {
            tracing::warn!(%error, "failed to mark interrupted schedule runs");
        }
        let mut failures = RepeatedFailure::new("failed to fire due schedules");
        let mut failed = false;
        loop {
            let wait = if failed {
                // A store that keeps failing is retried once a minute, not in a hot loop.
                MAX_WAIT
            } else {
                match self.store.load().await {
                    Ok(document) => wait_for(&document.schedules, self.clock.now()),
                    Err(error) => {
                        failures.report(&error);
                        MAX_WAIT
                    }
                }
            };
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                () = self.changed.notified() => {
                    failed = false;
                    continue;
                }
                () = tokio::time::sleep(wait) => {}
            }
            failed = match self.fire_due().await {
                Ok(()) => false,
                Err(error) => {
                    failures.report(&error);
                    true
                }
            };
        }
    }

    /// Fires one schedule and returns its `started_at` key, recording `Skipped` instead of
    /// submitting a job when the previous run is still live.
    pub(super) async fn fire(&self, id: &ScheduleId) -> DaemonResult<String> {
        let Some(mut reservation) = LiveReservation::acquire(Arc::clone(&self.live), id.clone())
        else {
            return self.skip(id).await;
        };
        if let Some(board) = self.orphaned(id).await? {
            // The reservation is released as it drops: nothing was launched.
            return self.retire_orphan(id, &board).await;
        }
        let started = self.start(id).await?;
        let Started {
            schedule,
            prompt,
            started_at,
            dropped_logs,
        } = started;
        remove_logs(&self.home, id, dropped_logs).await;
        #[cfg(test)]
        self.pause_before_submit().await;
        let title = format!("Scheduled: {}", schedule.name);
        let board = schedule.board_id.clone();
        let ticket = Arc::new(RunTicket {
            schedules: self.clone(),
            schedule: id.clone(),
            board: schedule.board_id.clone(),
            started_at: started_at.clone(),
            state: std::sync::Mutex::new(TicketState::Pending),
        });
        let run_schedule = schedule.clone();
        let operation = {
            let ticket = Arc::clone(&ticket);
            move |context: crate::jobs::JobCtx| async move {
                ticket.set(TicketState::Started);
                let result = ticket
                    .schedules
                    .runner
                    .run(&run_schedule, &prompt, context.cancel.clone())
                    .await;
                let cancelled = context.cancel.is_cancelled();
                ticket.finish(result, cancelled).await
            }
        };
        let job = self.jobs.submit(
            JobKind::ScheduledTask,
            id.to_string(),
            title,
            true,
            false,
            operation,
        );
        // The operation owns one ticket reference when admitted. If the job manager de-duplicates
        // it, dropping our last reference below records this run as skipped instead.
        reservation.handoff();
        let target = id.clone();
        let recorded_job = job.clone();
        let recorded_started_at = started_at.clone();
        let live = Arc::clone(&self.live);
        let recorded =
            self.store
                .transaction(move |document| {
                    let Ok(schedule) = find_mut(document, &target) else {
                        // Deleted after `start` but before this transaction. The caller cancels the
                        // submitted job and removes any directory it managed to recreate.
                        return Ok(None);
                    };
                    // `started_at` is unique within the schedule (`run_stamp`), so a skipped fire
                    // recorded in the same second can never take this run's job.
                    let job_is_already_owned = schedule.runs.iter().any(|run| {
                        run.started_at != recorded_started_at
                            && run.job_id.as_ref() == Some(&recorded_job)
                    });
                    let mut attached = false;
                    if !job_is_already_owned
                        && let Some(run) = schedule.runs.iter_mut().rev().find(|run| {
                            run.started_at == recorded_started_at && run.job_id.is_none()
                        })
                    {
                        run.job_id = Some(recorded_job.clone());
                        attached = true;
                        // Published under the store gate, like `start`'s `started_at`: a delete
                        // serialised after this transaction always finds the job to cancel.
                        if let Some(entry) = lock(&live).get_mut(&target)
                            && entry.started_at.as_deref() == Some(recorded_started_at.as_str())
                        {
                            entry.job = Some(recorded_job);
                        }
                    }
                    Ok(Some(attached))
                })
                .await?;
        match recorded {
            None => {
                match self.jobs.cancel(&job) {
                    Ok(_) => {
                        if let Err(error) = self.jobs.wait(&job).await {
                            tracing::warn!(%error, schedule = %id, job = %job, "deleted schedule's submitted job did not stop cleanly");
                        }
                    }
                    Err(error) => {
                        tracing::debug!(%error, schedule = %id, job = %job, "deleted schedule's submitted job was no longer cancellable");
                    }
                }
                remove_dir(&self.home.schedule_dir(id)).await;
            }
            Some(true) => {
                #[cfg(test)]
                self.pause_after_attach().await;
            }
            Some(false) => {
                ticket.set(TicketState::Done);
                lock(&self.live).remove(id);
                self.record_outcome(
                    id,
                    started_at.clone(),
                    RunResult {
                        outcome: ScheduleOutcome::Skipped,
                        summary: Some(SKIPPED_SUMMARY.to_owned()),
                        cost_usd: None,
                    },
                    true,
                )
                .await?;
            }
        }
        // The operation's ticket now owns the live entry. A de-duplicated operation was dropped
        // by the job manager, so this drop records its never-started run as skipped.
        drop(ticket);
        self.published(&board);
        Ok(started_at)
    }

    /// Fires every enabled schedule whose next run is due now.
    async fn fire_due(&self) -> DaemonResult<()> {
        let now = self.clock.now();
        let due: Vec<ScheduleId> = self
            .store
            .load()
            .await?
            .schedules
            .iter()
            .filter(|schedule| next_run_at(schedule, now).is_some_and(|next| next <= now))
            .map(|schedule| schedule.id.clone())
            .collect();
        let mut first_error = None;
        for id in due {
            if let Err(error) = self.fire(&id).await {
                tracing::warn!(%error, schedule = %id, "failed to fire a due schedule");
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// The board a schedule names when that board no longer exists.
    ///
    /// A schedule created while its board was being deleted, or one a failed board-delete
    /// cascade left behind, would otherwise launch an agent on its cadence forever against a
    /// board nothing can reach it from.
    async fn orphaned(&self, id: &ScheduleId) -> DaemonResult<Option<BoardId>> {
        let Some(board) = self
            .store
            .load()
            .await?
            .schedules
            .into_iter()
            .find(|schedule| schedule.id == *id)
            .map(|schedule| schedule.board_id)
        else {
            // `start` answers a schedule that is gone in its own words.
            return Ok(None);
        };
        match self.boards.get(&board).await {
            Ok(_) => Ok(None),
            Err(DaemonError::NotFound(_)) => Ok(Some(board)),
            Err(error) => Err(error),
        }
    }

    /// Records a `Failed` fire that launched nothing and disables the schedule, because its board
    /// is gone. Disabled rather than deleted: a board whose document was quarantined comes back
    /// when the file is repaired, and its schedules with it.
    async fn retire_orphan(&self, id: &ScheduleId, board: &BoardId) -> DaemonResult<String> {
        tracing::warn!(schedule = %id, %board, "a schedule's board no longer exists; disabling it");
        let now = self.clock.now();
        let target = id.clone();
        let summary = format!("board {board} no longer exists; the schedule is disabled");
        let (dropped, started_at) = self
            .store
            .transaction(move |document| {
                let schedule = find_mut(document, &target)?;
                let stamp = run_stamp(schedule, now);
                let dropped = push_run(
                    schedule,
                    ScheduleRun {
                        job_id: None,
                        started_at: stamp.clone(),
                        ended_at: Some(stamp.clone()),
                        outcome: Some(ScheduleOutcome::Failed),
                        summary: Some(summary),
                        cost_usd: None,
                        log_path: None,
                    },
                );
                schedule.enabled = false;
                replan(schedule, now);
                Ok((dropped, stamp))
            })
            .await?;
        remove_logs(&self.home, id, dropped).await;
        self.published(board);
        Ok(started_at)
    }

    /// Records the fire as `Skipped` because the schedule's previous run is still live.
    async fn skip(&self, id: &ScheduleId) -> DaemonResult<String> {
        let now = self.clock.now();
        let target = id.clone();
        let (board, dropped, started_at) = self
            .store
            .transaction(move |document| {
                let schedule = find_mut(document, &target)?;
                let stamp = run_stamp(schedule, now);
                let dropped = push_run(
                    schedule,
                    ScheduleRun {
                        job_id: None,
                        started_at: stamp.clone(),
                        ended_at: Some(stamp.clone()),
                        outcome: Some(ScheduleOutcome::Skipped),
                        summary: Some(SKIPPED_SUMMARY.to_owned()),
                        cost_usd: None,
                        log_path: None,
                    },
                );
                replan(schedule, now);
                Ok((schedule.board_id.clone(), dropped, stamp))
            })
            .await?;
        remove_logs(&self.home, id, dropped).await;
        self.published(&board);
        Ok(started_at)
    }

    /// Renders the prompt and records the run as started.
    async fn start(&self, id: &ScheduleId) -> DaemonResult<Started> {
        let now = self.clock.now();
        // The footer names the very `fleet` the runner puts first on the child's `PATH`.
        let fleet = self.runner.fleet_program().map_or_else(
            || "fleet".to_owned(),
            |program| shell_quote(&program.to_string_lossy()),
        );
        let home = self.home.clone();
        let target = id.clone();
        let live = Arc::clone(&self.live);
        self.store
            .transaction(move |document| {
                let schedule = find_mut(document, &target)?;
                let started_at = run_stamp(schedule, now);
                // Claimed in the same transaction that records the run, so a boot recovery
                // serialised after it always knows this run is live, never between the two.
                if let Some(entry) = lock(&live).get_mut(&target) {
                    entry.started_at = Some(started_at.clone());
                }
                // Rendered before the run is pushed: `{last_run_at}` is the previous run.
                let prompt = render_prompt(schedule, &fleet, &started_at);
                let log_path = home.schedule_log_path(&target, &started_at);
                let dropped_logs = push_run(
                    schedule,
                    ScheduleRun {
                        job_id: None,
                        started_at: started_at.clone(),
                        ended_at: None,
                        outcome: None,
                        summary: None,
                        cost_usd: None,
                        log_path: Some(log_path.to_string_lossy().into_owned()),
                    },
                );
                replan(schedule, now);
                Ok(Started {
                    schedule: schedule.clone(),
                    prompt,
                    started_at,
                    dropped_logs,
                })
            })
            .await
    }

    /// Writes how a run ended onto its recorded run, matched by `started_at`.
    async fn record_outcome(
        &self,
        id: &ScheduleId,
        started_at: String,
        result: RunResult,
        clear_artifacts: bool,
    ) -> DaemonResult<()> {
        let now = self.clock.now();
        let target = id.clone();
        let recorded = self
            .store
            .transaction(move |document| {
                let Ok(schedule) = find_mut(document, &target) else {
                    // Deleted while it ran: nothing is left to record on.
                    return Ok(false);
                };
                if let Some(run) = schedule
                    .runs
                    .iter_mut()
                    .rev()
                    .find(|run| run.started_at == started_at && run.outcome.is_none())
                {
                    run.outcome = Some(result.outcome);
                    run.summary = result.summary;
                    run.cost_usd = result.cost_usd;
                    run.ended_at = Some(timestamp(now));
                    if clear_artifacts {
                        run.job_id = None;
                        run.log_path = None;
                    }
                }
                replan(schedule, now);
                Ok(true)
            })
            .await?;
        if !recorded {
            tracing::debug!(schedule = %id, "a finished run's schedule was deleted meanwhile");
        }
        Ok(())
    }

    /// Marks every run left without an outcome, by a daemon that died mid-run, as `Failed`.
    pub(super) async fn recover_interrupted(&self) -> DaemonResult<()> {
        let interrupted = self
            .store
            .load()
            .await?
            .schedules
            .iter()
            .any(|schedule| schedule.runs.iter().any(|run| run.outcome.is_none()));
        if !interrupted {
            return Ok(());
        }
        let now = self.clock.now();
        let live = Arc::clone(&self.live);
        let boards = self
            .store
            .transaction(move |document| {
                let live = lock(&live);
                let mut boards = Vec::<BoardId>::new();
                for schedule in &mut document.schedules {
                    let owned = live
                        .get(&schedule.id)
                        .and_then(|entry| entry.started_at.as_deref());
                    let mut touched = false;
                    for run in schedule.runs.iter_mut().filter(|run| {
                        run.outcome.is_none() && Some(run.started_at.as_str()) != owned
                    }) {
                        run.outcome = Some(ScheduleOutcome::Failed);
                        run.summary = Some(INTERRUPTED_SUMMARY.to_owned());
                        run.ended_at = Some(timestamp(now));
                        touched = true;
                    }
                    if touched {
                        replan(schedule, now);
                        if !boards.contains(&schedule.board_id) {
                            boards.push(schedule.board_id.clone());
                        }
                    }
                }
                Ok(boards)
            })
            .await?;
        for board in &boards {
            self.published(board);
        }
        Ok(())
    }

    #[cfg(test)]
    async fn pause_before_submit(&self) {
        self.fire_pause.pause().await;
    }

    #[cfg(test)]
    async fn pause_after_attach(&self) {
        self.attach_pause.pause().await;
    }
}

/// Owns a schedule's reservation until its submitted job's [`RunTicket`] takes over.
struct LiveReservation {
    live: Arc<LiveRuns>,
    schedule: ScheduleId,
    handed_over: bool,
}

impl LiveReservation {
    fn acquire(live: Arc<LiveRuns>, schedule: ScheduleId) -> Option<Self> {
        let mut entries = lock(&live);
        if entries.contains_key(&schedule) {
            return None;
        }
        entries.insert(
            schedule.clone(),
            LiveRun {
                started_at: None,
                job: None,
            },
        );
        drop(entries);
        Some(Self {
            live,
            schedule,
            handed_over: false,
        })
    }

    fn handoff(&mut self) {
        self.handed_over = true;
    }
}

impl Drop for LiveReservation {
    fn drop(&mut self) {
        if !self.handed_over {
            lock(&self.live).remove(&self.schedule);
        }
    }
}

/// How long the loop sleeps before the earliest due schedule, capped at [`MAX_WAIT`].
fn wait_for(schedules: &[Schedule], now: DateTime<Utc>) -> Duration {
    schedules
        .iter()
        .filter_map(|schedule| next_run_at(schedule, now))
        .min()
        .map_or(MAX_WAIT, |next| {
            (next - now)
                .to_std()
                .unwrap_or(Duration::ZERO)
                .min(MAX_WAIT)
        })
}

/// What `start` hands the job: the schedule as recorded, its rendered prompt and the run key.
struct Started {
    schedule: Schedule,
    prompt: String,
    started_at: String,
    dropped_logs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TicketState {
    /// Submitted; the job's operation has not begun.
    Pending,
    /// The runner is running.
    Started,
    /// The outcome is recorded.
    Done,
}

/// Owns one schedule's `live` entry for the life of its job's operation.
///
/// It releases the entry exactly once: when the outcome is recorded, or, when the operation is
/// dropped first, from `Drop`. The job manager drops an operation it never starts when an older
/// job for the same schedule is still finishing, and drops a started one that outlives its
/// cancellation grace or panics; neither may leave the schedule looking live forever.
struct RunTicket {
    schedules: Schedules,
    schedule: ScheduleId,
    board: BoardId,
    started_at: String,
    state: std::sync::Mutex<TicketState>,
}

impl RunTicket {
    fn set(&self, state: TicketState) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    fn get(&self) -> TicketState {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records the runner's result, releases the schedule and answers the job's result.
    async fn finish(&self, result: RunResult, cancelled: bool) -> DaemonResult<()> {
        let outcome = result.outcome;
        let summary = result.summary.clone();
        self.schedules
            .record_outcome(&self.schedule, self.started_at.clone(), result, false)
            .await?;
        self.set(TicketState::Done);
        lock(&self.schedules.live).remove(&self.schedule);
        self.schedules.published(&self.board);
        if cancelled {
            return Err(DaemonError::Cancelled);
        }
        let reason = || summary.clone().unwrap_or_else(|| "no summary".to_owned());
        match outcome {
            ScheduleOutcome::Succeeded | ScheduleOutcome::Skipped => Ok(()),
            ScheduleOutcome::TimedOut => Err(DaemonError::Timeout(reason())),
            ScheduleOutcome::Failed => Err(DaemonError::Shell(reason())),
        }
    }
}

impl Drop for RunTicket {
    fn drop(&mut self) {
        let (outcome, summary, clear_artifacts) = match self.get() {
            TicketState::Done => return,
            TicketState::Pending => (ScheduleOutcome::Skipped, SKIPPED_SUMMARY, true),
            TicketState::Started => (ScheduleOutcome::Failed, STOPPED_SUMMARY, false),
        };
        lock(&self.schedules.live).remove(&self.schedule);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(schedule = %self.schedule, "an unfinished schedule run could not be recorded outside the runtime");
            return;
        };
        let schedules = self.schedules.clone();
        let id = self.schedule.clone();
        let board = self.board.clone();
        let started_at = self.started_at.clone();
        // Detached: it only writes the outcome and logs its own failure.
        runtime.spawn(async move {
            let result = RunResult {
                outcome,
                summary: Some(summary.to_owned()),
                cost_usd: None,
            };
            match schedules
                .record_outcome(&id, started_at, result, clear_artifacts)
                .await
            {
                Ok(()) => schedules.published(&board),
                Err(error) => {
                    tracing::warn!(%error, schedule = %id, "failed to record an unfinished schedule run");
                }
            }
        });
    }
}

/// Single-quotes `text` for a POSIX shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
