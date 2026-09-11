//! The single owned writer: one thread, one read-write connection, an `mpsc` inbox and a
//! `oneshot` reply.
//!
//! One `std::thread` named `fleet-agent-db` owns the only read-write `Connection` for the process
//! lifetime and loops on an unbounded `mpsc` inbox; every command carries a `oneshot` the caller
//! awaits. That is the whole concurrency model: **one writer, FIFO, no interleaving, a
//! promise-shaped API.**
//!
//! Not `spawn_blocking`: a blocking task that never returns occupies a slot in the pool for the
//! life of the daemon, and that pool is shared with PTY and git work
//! (`rust-async-background-work` Rule 10). A named `std::thread` is honest about what it is, and
//! it is the reason no `.await` can ever happen while a `Transaction<'_>` is alive — this thread
//! has no async context at all.
//!
//! Not `Arc<Mutex<Connection>>` either: that is the same serialization with none of the clarity,
//! and it invites a lock held across an await.
//!
//! **Batching.** One transaction per `ContentDelta` would be thousands of commits per turn, so a
//! commit takes whatever else is already queued behind it, up to
//! [`WRITE_BATCH_MAX_EVENTS`]/[`WRITE_BATCH_MAX_BYTES`]. The 16 ms *waiting* window the design
//! describes is deliberately not armed here: every command this seam can produce carries a reply
//! a caller is already awaiting, so waiting for company would be pure added latency. It belongs
//! with the fire-and-forget streaming append, which arrives with the live path.
//!
//! **Durability.** The connection runs at `synchronous = NORMAL`, which loses commits to power
//! loss but never to a process crash. A batch that contains an event the old store fsynced —
//! plus the two the new model adds, a checkpoint and an abort — is bracketed by
//! `synchronous = FULL`. That is `store.rs`'s selective-fsync policy, transliterated.
//!
//! **Shutdown.** Dropping the handle closes the inbox; the thread drains what is queued, commits
//! it, runs `PRAGMA optimize`, and signals. The drop waits [`SHUTDOWN_DEADLINE`] for that signal
//! and then gives up rather than hanging a daemon exit on a wedged write.

use std::{
    path::Path,
    sync::{
        Mutex,
        mpsc::{Receiver, RecvTimeoutError, Sender},
    },
    thread::JoinHandle,
    time::Duration,
};

use anyhow::{Context, anyhow};
use fleet_core::{
    agents::{AgentEvent, AgentThreadSummary, Seq, SeqEvent, ThreadId},
    ids::HostId,
};
use rusqlite::{Connection, TransactionBehavior};
use tokio::sync::{mpsc, oneshot};

use super::{
    AgentIndex, AgentThreadRecord, index, migrations, mirror,
    project::{self, StagedEvent},
};

/// Commands per transaction. Bounds how long one commit can hold the writer.
const WRITE_BATCH_MAX_EVENTS: usize = 64;

/// Serialized payload bytes per transaction.
const WRITE_BATCH_MAX_BYTES: usize = 256 * 1024;

/// How long a drop waits for the writer to drain, commit and optimize.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(2);

/// The thread name, so the writer is identifiable in a stack trace or `top`.
const WRITER_THREAD_NAME: &str = "fleet-agent-db";

/// What the writer answers with. `()` because a write's only product is durability.
type Reply = oneshot::Sender<anyhow::Result<()>>;

/// One unit of work for the writer, without its reply channel.
enum Work {
    /// Append one event, project it, and advance the head — one transaction, in that order.
    Append {
        thread: ThreadId,
        staged: Box<StagedEvent>,
    },
    /// Quarantine everything after `last` and rebuild the thread's read model.
    TruncateAfter { thread: ThreadId, last: Option<Seq> },
    /// Replay one thread's log over a cleared read model.
    Rebuild { thread: ThreadId },
    /// Replace the thread index.
    WriteIndex { index: Box<AgentIndex> },
    /// Upsert one thread's durable metadata.
    WriteRecord { record: Box<AgentThreadRecord> },
    /// Mark one thread's session failed, with the reason.
    FailSession { thread: ThreadId, message: String },
    /// Record the header of a thread another host owns.
    MirrorClaim {
        host: HostId,
        summary: Box<AgentThreadSummary>,
    },
    /// Append owner-sequenced events to a mirrored thread's prefix.
    MirrorAppend {
        host: HostId,
        thread: ThreadId,
        staged: Vec<StagedEvent>,
        /// The owner's head as the answer that carried these events reported it.
        owner_head: Option<Seq>,
    },
    /// Throw one mirrored thread's cached transcript away, leaving its header.
    MirrorDiscard { thread: ThreadId },
}

/// A handle on the owned writer thread.
pub(super) struct Writer {
    /// `None` only while dropping. Unbounded on purpose: the reducer has already accepted the
    /// event it is asking us to persist, so the log must take it — blocking the caller instead
    /// would apply back-pressure to a thread that is holding the reply it is waiting for. The
    /// real bound is the number of in-flight callers, each of which is parked on its `oneshot`.
    commands: Option<mpsc::UnboundedSender<Queued>>,
    /// Signalled once, after the writer has drained, committed and optimized. Behind a mutex
    /// only because a `std::sync::mpsc::Receiver` is `Send` but not `Sync`, and this handle is
    /// shared across tokio tasks; nothing ever contends for it but the drop.
    finished: Mutex<Receiver<()>>,
    join: Option<JoinHandle<()>>,
}

/// A command as it travels the inbox: the work plus the caller's reply channel.
struct Queued {
    work: Work,
    reply: Reply,
}

/// Opens the read-write connection and brings its schema to head.
///
/// It runs on the caller's thread and hands the connection back rather than keeping it, so the
/// store can do the rest of its start-time work — the one-shot NDJSON import, the boot-work census
/// — on the same handle before any other thread can reach the file. A database this build cannot
/// open or migrate fails here, which is exactly as an unreadable `state.json` fails, and for the
/// same reason: nothing must run on half a truth.
pub(super) fn open(path: &Path) -> anyhow::Result<Connection> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("open the agent database `{}`", path.display()))?;
    migrations::run(&mut conn, None)
        .with_context(|| format!("migrate the agent database `{}`", path.display()))?;
    Ok(conn)
}

impl Writer {
    /// Takes ownership of the connection [`open`] returned and spawns the writer thread.
    ///
    /// From here on this connection is reachable only from that thread, which is what makes "one
    /// writer, FIFO, no interleaving" a property of the type and not of a convention.
    pub(super) fn spawn(conn: Connection) -> anyhow::Result<Self> {
        let (commands, inbox) = mpsc::unbounded_channel();
        let (finished_sender, finished) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name(WRITER_THREAD_NAME.to_owned())
            .spawn(move || run(conn, inbox, finished_sender))
            .context("spawn the fleet-agent-db writer thread")?;
        Ok(Self {
            commands: Some(commands),
            finished: Mutex::new(finished),
            join: Some(join),
        })
    }

    /// Appends one event, projects it, and advances the head, in one transaction.
    pub(super) async fn append(&self, thread: ThreadId, event: &SeqEvent) -> anyhow::Result<()> {
        // Serializing here rather than on the writer keeps the CPU cost of a large payload off
        // the transaction, and it lets the batch's byte budget be exact instead of a guess.
        let staged = Box::new(StagedEvent::prepare(event)?);
        self.request(Work::Append { thread, staged }).await
    }

    /// Quarantines everything after `last` and rebuilds the thread.
    pub(super) async fn truncate_after(
        &self,
        thread: ThreadId,
        last: Option<Seq>,
    ) -> anyhow::Result<()> {
        self.request(Work::TruncateAfter { thread, last }).await
    }

    /// Rebuilds one thread's read model from its log.
    pub(super) async fn rebuild(&self, thread: ThreadId) -> anyhow::Result<()> {
        self.request(Work::Rebuild { thread }).await
    }

    /// Replaces the thread index.
    pub(super) async fn write_index(&self, index: &AgentIndex) -> anyhow::Result<()> {
        self.request(Work::WriteIndex {
            index: Box::new(index.clone()),
        })
        .await
    }

    /// Upserts one thread's durable metadata.
    pub(super) async fn write_record(&self, record: &AgentThreadRecord) -> anyhow::Result<()> {
        self.request(Work::WriteRecord {
            record: Box::new(record.clone()),
        })
        .await
    }

    /// Records the header of a thread another host owns.
    pub(super) async fn mirror_claim(
        &self,
        host: HostId,
        summary: &AgentThreadSummary,
    ) -> anyhow::Result<()> {
        self.request(Work::MirrorClaim {
            host,
            summary: Box::new(summary.clone()),
        })
        .await
    }

    /// Appends owner-sequenced events to a mirrored thread's prefix.
    ///
    /// Staging runs on the caller's thread, exactly as [`Writer::append`] does, so a large tool
    /// payload is serialized without the write lock held.
    pub(super) async fn mirror_append(
        &self,
        host: HostId,
        thread: ThreadId,
        events: &[SeqEvent],
        owner_head: Option<Seq>,
    ) -> anyhow::Result<()> {
        let staged = mirror::stage(events)?;
        self.request(Work::MirrorAppend {
            host,
            thread,
            staged,
            owner_head,
        })
        .await
    }

    /// Throws one mirrored thread's cached transcript away.
    pub(super) async fn mirror_discard(&self, thread: ThreadId) -> anyhow::Result<()> {
        self.request(Work::MirrorDiscard { thread }).await
    }

    /// Marks one thread's session failed, with the reason.
    pub(super) async fn fail_session(
        &self,
        thread: ThreadId,
        message: String,
    ) -> anyhow::Result<()> {
        self.request(Work::FailSession { thread, message }).await
    }

    /// Queues one command and awaits the commit it produces.
    ///
    /// The reply resolves strictly after `COMMIT`, which is the "durable before visible" rule the
    /// old store held and this one strengthens: a caller cannot broadcast, answer a request, or
    /// read a projection that is not yet on disk.
    async fn request(&self, work: Work) -> anyhow::Result<()> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .as_ref()
            .ok_or_else(|| anyhow!("the agent database writer is shutting down"))?
            .send(Queued { work, reply })
            .map_err(|_closed| anyhow!("the agent database writer thread is gone"))?;
        answer
            .await
            .map_err(|_dropped| anyhow!("the agent database writer dropped a reply"))?
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Closing the inbox is what makes the writer finish: tokio delivers every queued command
        // before the receiver observes the close, so the drain is a property of the channel and
        // not something this thread has to orchestrate.
        drop(self.commands.take());
        let finished = self
            .finished
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match finished.recv_timeout(SHUTDOWN_DEADLINE) {
            Ok(()) => {
                if let Some(join) = self.join.take()
                    && join.join().is_err()
                {
                    tracing::error!("the fleet-agent-db writer thread panicked");
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                tracing::error!("the fleet-agent-db writer thread ended without finishing");
            }
            Err(RecvTimeoutError::Timeout) => {
                // Never block a daemon exit on a wedged write. The transaction is atomic, so the
                // database is consistent whether or not the last commit landed.
                tracing::warn!(
                    deadline_ms = SHUTDOWN_DEADLINE.as_millis(),
                    "the fleet-agent-db writer did not finish in time; leaving it to process exit"
                );
            }
        }
    }
}

/// The writer thread body.
fn run(mut conn: Connection, mut inbox: mpsc::UnboundedReceiver<Queued>, finished: Sender<()>) {
    // `blocking_recv` is correct precisely because this thread has no async context: it is a
    // dedicated OS thread, not a tokio worker, so parking it costs the runtime nothing.
    while let Some(first) = inbox.blocking_recv() {
        let mut batch = vec![first];
        let mut bytes = size_of_work(&batch[0].work);
        while batch.len() < WRITE_BATCH_MAX_EVENTS && bytes < WRITE_BATCH_MAX_BYTES {
            match inbox.try_recv() {
                Ok(next) => {
                    bytes = bytes.saturating_add(size_of_work(&next.work));
                    batch.push(next);
                }
                Err(_empty_or_closed) => break,
            }
        }
        commit(&mut conn, batch);
    }

    // `optimize` runs ANALYZE only on the tables whose statistics went stale, so it is cheap and
    // it is what keeps the partial indexes the read path depends on being chosen next boot.
    if let Err(error) = conn.execute_batch("PRAGMA optimize") {
        tracing::warn!(%error, "could not optimize the agent database on shutdown");
    }
    if finished.send(()).is_err() {
        // The handle was already gone, so nobody is waiting on the deadline. Not an error.
        tracing::debug!("nothing was waiting for the agent database writer to finish");
    }
}

/// Commits one batch, then answers every caller in it.
///
/// A batch is one `BEGIN IMMEDIATE … COMMIT`. If it fails, each command is retried in a
/// transaction of its own, so one poisonous command fails only its own caller instead of taking
/// every write queued behind it. The retry is safe because the batch rolled back whole: nothing
/// it contained was applied.
fn commit(conn: &mut Connection, batch: Vec<Queued>) {
    let durable = batch.iter().any(|queued| needs_durability(&queued.work));
    if durable {
        set_synchronous(conn, "FULL");
    }
    let outcome = transact(conn, batch.iter().map(|queued| &queued.work));
    let bisect = outcome.is_err() && batch.len() > 1;
    if let Err(error) = &outcome
        && bisect
    {
        tracing::warn!(
            %error,
            commands = batch.len(),
            "an agent database batch failed; retrying its commands one at a time"
        );
    }
    for queued in batch {
        let answer = if bisect {
            transact(conn, std::iter::once(&queued.work))
        } else {
            match &outcome {
                Ok(()) => Ok(()),
                // anyhow::Error is not cloneable, and every caller in a failed batch needs the
                // same story, so the message is carried rather than the error object.
                Err(error) => Err(anyhow!("{error:#}")),
            }
        };
        if queued.reply.send(answer).is_err() {
            tracing::debug!("an agent database write reply had no receiver left");
        }
    }
    if durable {
        set_synchronous(conn, "NORMAL");
    }
}

/// Applies work in one transaction: the append, its projections, and the head bump commit together
/// or not at all.
fn transact<'work>(
    conn: &mut Connection,
    work: impl Iterator<Item = &'work Work>,
) -> anyhow::Result<()> {
    // IMMEDIATE, so a competing writer is refused at BEGIN rather than after the first statement
    // has already been applied.
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin an agent database write transaction")?;
    for unit in work {
        match unit {
            Work::Append { thread, staged } => {
                project::append_event(&transaction, *thread, staged)?;
                project::project_event(&transaction, *thread, &staged.event)?;
                project::advance_head(&transaction, *thread, &staged.event)?;
            }
            Work::TruncateAfter { thread, last } => {
                project::quarantine_after(
                    &transaction,
                    *thread,
                    *last,
                    "the reducer refused this event on replay",
                )?;
            }
            Work::Rebuild { thread } => project::rebuild_thread(&transaction, *thread)?,
            Work::WriteIndex { index } => index::write(&transaction, index)?,
            Work::WriteRecord { record } => index::upsert(&transaction, record)?,
            Work::FailSession { thread, message } => {
                project::fail_session(&transaction, *thread, message)?;
            }
            Work::MirrorClaim { host, summary } => mirror::claim(&transaction, host, summary)?,
            Work::MirrorAppend {
                host,
                thread,
                staged,
                owner_head,
            } => {
                for event in staged {
                    mirror::append_from_owner(&transaction, host, *thread, event)?;
                }
                if let Some(owner_head) = owner_head {
                    mirror::note_owner_head(&transaction, *thread, *owner_head)?;
                }
            }
            Work::MirrorDiscard { thread } => mirror::discard(&transaction, *thread)?,
        }
    }
    transaction
        .commit()
        .context("commit an agent database write transaction")
}

/// Whether this work must survive power loss, not merely a process crash.
///
/// The four event kinds are `store.rs`'s policy verbatim, including its justification for both
/// ends of a gate: §3 rule 4 closes a gate only on the resolution event, so a durable open with a
/// lost close replays a card no adapter can answer. The new model adds `Checkpoint`, which is a
/// git ref the user can revert to, and `TurnAborted`, which is the record that an interrupt
/// landed. An index write and a quarantine are durable because both change what the daemon
/// believes exists.
fn needs_durability(work: &Work) -> bool {
    match work {
        Work::Append { staged, .. } => matches!(
            staged.event.event,
            AgentEvent::TurnSettled { .. }
                | AgentEvent::TurnAborted { .. }
                | AgentEvent::GateOpened { .. }
                | AgentEvent::GateResolved { .. }
                | AgentEvent::SessionExited { .. }
                | AgentEvent::Compacted(_)
        ),
        Work::TruncateAfter { .. } | Work::WriteIndex { .. } | Work::WriteRecord { .. } => true,
        // A mirrored append is the owner's durable event reaching this daemon; it earns the same
        // fsync policy as a local one. A claim and a discard change what the daemon believes
        // exists, exactly as an index write does.
        Work::MirrorAppend { staged, .. } => staged.iter().any(|event| {
            matches!(
                event.event.event,
                AgentEvent::TurnSettled { .. }
                    | AgentEvent::TurnAborted { .. }
                    | AgentEvent::GateOpened { .. }
                    | AgentEvent::GateResolved { .. }
                    | AgentEvent::SessionExited { .. }
                    | AgentEvent::Compacted(_)
            )
        }),
        Work::MirrorClaim { .. } | Work::MirrorDiscard { .. } => true,
        // A rebuild derives rows that are already derivable from a durable log, and a failed
        // session is re-derived by the next start's census from the same rows.
        Work::Rebuild { .. } | Work::FailSession { .. } => false,
    }
}

/// What this work will add to the batch's byte budget.
fn size_of_work(work: &Work) -> usize {
    match work {
        Work::Append { staged, .. } => staged.bytes(),
        Work::MirrorAppend { staged, .. } => staged
            .iter()
            .map(StagedEvent::bytes)
            .fold(0, usize::saturating_add),
        // A truncate rebuilds one thread and an index write touches one row per thread; neither
        // is measured in payload bytes, so each counts as a whole batch's worth and flushes.
        Work::TruncateAfter { .. }
        | Work::WriteIndex { .. }
        | Work::WriteRecord { .. }
        | Work::Rebuild { .. }
        | Work::FailSession { .. }
        | Work::MirrorClaim { .. }
        | Work::MirrorDiscard { .. } => WRITE_BATCH_MAX_BYTES,
    }
}

/// Raises or lowers durability around a batch. A failure here is not worth failing a write over:
/// the batch still commits, at the durability the connection already had.
fn set_synchronous(conn: &Connection, level: &str) {
    if let Err(error) = conn.pragma_update(None, "synchronous", level) {
        tracing::warn!(%error, level, "could not change agent database durability");
    }
}
