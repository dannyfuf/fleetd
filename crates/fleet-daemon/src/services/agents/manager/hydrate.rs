//! Lazy per-thread hydration, restart recovery, and the background boot repair.
//!
//! **What this module removes.** The manager used to replay *every* thread's whole log in its
//! constructor, so `fleetd` start cost grew without bound with transcript history and answering
//! `AgentThreadList` meant mapping over the projections that replay had built. Neither is true
//! any more: a start reads nothing, the list is one `SELECT` against `threads`
//! (`docs/NATIVE-AGENTS.md` §8), and a thread's reducer state is built the first time something
//! actually needs it — an open, a send, a resume.
//!
//! **What still has to happen at start.** Two things, both per thread, neither urgent, so both run
//! in the background off a census the store took when it opened the database:
//!
//! - A thread whose `projected_seq` fell behind its `head_seq` is replayed once through the same
//!   projector a live append uses. One that cannot be replayed is marked `session_state = 'error'`
//!   and the daemon starts anyway — it stays browsable read-only up to `projected_seq`, and no
//!   other thread is affected.
//! - A thread the previous run left claiming a provider it no longer has is an orphan, and §6
//!   settles an orphan **explicitly, as appended events**, never as a silent state edit. That
//!   needs the reducer, so an orphan is hydrated — which is the only replay a start performs, and
//!   only for the threads that were live when the daemon went away.
//!
//! The census is taken before the writer thread exists, which is what makes the background pass
//! safe: a thread created after the store opened can never be in it, so a live thread can never be
//! mistaken for an orphan of the previous run.

use chrono::Utc;
use fleet_core::agents::{
    AbortReason, AgentEvent, GateResolver, SeqEvent, SessionState, ThreadId, ThreadProjection,
    TurnState,
};
use fleet_proto::error::ProtoError;

use super::{
    AgentSessionManager, AgentThreadRecord, ManagerInner,
    apply::{closed_gate_answer, update_record},
    not_found, storage_error,
    thread::ThreadRuntime,
};

impl AgentSessionManager {
    /// The runtime for one thread, hydrating it from the database if this is its first use.
    ///
    /// The fast path is a read lock and a clone. The slow path takes the manager's hydration gate
    /// so two concurrent first-uses of the same thread cannot each build a projection: two
    /// runtimes for one thread would be two writers minting the same sequence, and restart
    /// recovery would append its settlement events twice.
    pub(super) async fn runtime(&self, thread: ThreadId) -> Result<ThreadRuntime, ProtoError> {
        if let Some(runtime) = self.hydrated(thread) {
            return Ok(runtime);
        }
        let _hydrating = self.inner.hydration.lock().await;
        // Re-checked under the gate: the thread may have been hydrated while we waited for it.
        if let Some(runtime) = self.hydrated(thread) {
            return Ok(runtime);
        }
        let record = self
            .inner
            .store()
            .map_err(storage_error)?
            .read_record(thread)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| not_found(format!("agent thread {thread}")))?;
        let runtime = hydrate(&self.inner, record).await.map_err(storage_error)?;
        self.inner
            .threads
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(thread, runtime.clone());
        Ok(runtime)
    }

    /// The runtime for one thread if it is already hydrated, without touching the database.
    pub(super) fn hydrated(&self, thread: ThreadId) -> Option<ThreadRuntime> {
        self.inner
            .threads
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&thread)
            .cloned()
    }

    /// Works through the store's boot census, one thread at a time, off the start path.
    pub(super) async fn repair(self) {
        let Ok(store) = self.inner.store() else {
            return;
        };
        let work = store.boot_work().clone();
        for thread in work.unprojected {
            match store.rebuild(thread).await {
                Ok(()) => {
                    tracing::info!(%thread, "replayed a native-agent read model that lagged its log")
                }
                Err(error) => {
                    let reason = format!("{error:#}");
                    tracing::warn!(%thread, error = %reason, "could not replay a native-agent read model");
                    if let Err(error) = store.fail_session(thread, reason).await {
                        tracing::warn!(%thread, %error, "could not record an unrepairable native-agent thread");
                    }
                }
            }
        }
        for thread in work.orphans {
            // Hydrating is what settles it: `hydrate` appends the recovery events §6 requires.
            if let Err(error) = self.runtime(thread).await {
                tracing::warn!(
                    %thread,
                    error = %error.message,
                    "could not settle a native-agent thread the restart orphaned"
                );
            }
        }
    }
}

/// Builds one thread's reducer state from its log, settling it first if the restart orphaned it.
async fn hydrate(
    inner: &ManagerInner,
    mut record: AgentThreadRecord,
) -> anyhow::Result<ThreadRuntime> {
    let mut projection = load_projection(inner, &record).await?;
    if orphaned(&projection) {
        recover_orphan(inner, &mut record, &mut projection).await;
        if let Err(error) = inner.store()?.write_record(&record).await {
            tracing::warn!(thread = %record.thread, %error, "could not persist native-agent restart recovery");
        }
    }
    Ok(ThreadRuntime::new(projection, record))
}

/// Replays a thread's log as far as the reducer accepts it, quarantining anything it does not.
///
/// One event the reducer rejects — a shape an older daemon wrote, a tail an import could only
/// partly read — must cost that event and the ones behind it, never the whole thread: a thread
/// that fails to hydrate answers `NotFound` for good. The log is trimmed to what replayed, so the
/// next append continues from the sequence the projection actually holds instead of colliding with
/// the events it skipped, and the trimmed events are moved to `agent_events_quarantine` with their
/// reason rather than dropped.
async fn load_projection(
    inner: &ManagerInner,
    record: &AgentThreadRecord,
) -> anyhow::Result<ThreadProjection> {
    let mut projection = projection_seed(record);
    let events = inner.store()?.load(record.thread).await?;
    let logged = events.last().map(|event| event.seq);
    let mut replayed = None;
    for event in &events {
        if let Err(error) = projection.apply(event) {
            tracing::warn!(
                thread = %record.thread,
                seq = %event.seq,
                %error,
                "quarantining a native-agent event the reducer rejected on replay"
            );
            break;
        }
        replayed = Some(event.seq);
    }
    if replayed != logged
        && let Err(error) = inner.store()?.truncate_after(record.thread, replayed).await
    {
        tracing::warn!(thread = %record.thread, %error, "could not trim a native-agent log");
    }
    Ok(projection)
}

/// The projection a thread starts from before its log is replayed over it.
pub(super) fn projection_seed(record: &AgentThreadRecord) -> ThreadProjection {
    let mut projection =
        ThreadProjection::new(record.thread, record.worktree.clone(), record.provider);
    projection.title.clone_from(&record.title);
    projection.model.clone_from(&record.model);
    projection.mode = record.mode;
    projection
}

/// Whether the log leaves this thread claiming a provider the restart already killed.
///
/// §6 settles an orphan explicitly. `Ready` is as much a live-provider state as `Running` is —
/// the child is gone either way — so leaving it out would strand the thread advertising a
/// session it does not have, with no state `open` is willing to resume from.
fn orphaned(projection: &ThreadProjection) -> bool {
    matches!(
        projection.session,
        SessionState::Starting | SessionState::Ready | SessionState::Running
    ) || matches!(projection.turn, TurnState::Running(_))
}

/// Settles an orphaned thread as appended events, in the reducer's own order.
pub(super) async fn recover_orphan(
    inner: &ManagerInner,
    record: &mut AgentThreadRecord,
    projection: &mut ThreadProjection,
) {
    // §3.3 rule 4 and §6: a gate the restart orphaned is settled explicitly, as appended
    // events. Otherwise the card outlives the adapter that could answer it and the thread stays
    // on the highest-priority `NeedsYou` forever.
    for gate in projection.gates.clone() {
        if !append_recovery(
            inner,
            record,
            projection,
            AgentEvent::GateResolved {
                gate: gate.id,
                answer: closed_gate_answer(&gate.kind),
                by: GateResolver::ProviderClosed,
            },
        )
        .await
        {
            return;
        }
    }
    if record.resume_cursor.is_some() {
        if let TurnState::Running(turn) = projection.turn
            && !append_recovery(
                inner,
                record,
                projection,
                AgentEvent::TurnAborted {
                    turn,
                    reason: AbortReason::ProviderExited,
                },
            )
            .await
        {
            return;
        }
        append_recovery(
            inner,
            record,
            projection,
            AgentEvent::SessionStateChanged(SessionState::Stopped),
        )
        .await;
    } else {
        append_recovery(
            inner,
            record,
            projection,
            AgentEvent::SessionExited {
                code: None,
                expected: false,
            },
        )
        .await;
    }
}

/// Appends one recovery event in the same order the reducer uses: settle, persist, apply.
///
/// Returns whether the caller may append another one. Applying before persisting would put
/// `last_seq` ahead of the log, so the next append would leave a hole the log's unique index
/// refuses. Nothing else can reach this thread yet — it is not in the hydrated map until
/// `runtime` puts it there — so no operation gate is needed and none exists to hold.
async fn append_recovery(
    inner: &ManagerInner,
    record: &mut AgentThreadRecord,
    projection: &mut ThreadProjection,
    event: AgentEvent,
) -> bool {
    let sequenced = SeqEvent {
        seq: projection.last_seq.next(),
        at: Utc::now(),
        raw: Some("daemon_restart_recovery".to_owned()),
        event,
    };
    if let Err(error) = projection.accepts(&sequenced) {
        tracing::warn!(thread = %record.thread, %error, "could not reduce restart recovery");
        return true;
    }
    let appended = match inner.store() {
        Ok(store) => store.append(record.thread, &sequenced).await,
        Err(error) => Err(error),
    };
    if let Err(error) = appended {
        tracing::warn!(thread = %record.thread, %error, "could not persist restart recovery");
        return false;
    }
    if let Err(error) = projection.apply(&sequenced) {
        tracing::warn!(thread = %record.thread, %error, "could not reduce restart recovery");
        return false;
    }
    update_record(record, &sequenced, &projection.title);
    true
}
