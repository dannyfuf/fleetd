//! The SQLite store for native-agent transcripts.
//!
//! One database per daemon at `$FLEET_HOME/agents/state.sqlite` holds the append-only event log
//! and every read model derived from it (`docs/NATIVE-AGENTS.md` §8,
//! `docs/decisions/0013-sqlite-agent-transcripts.md`). SQLite lives in `fleet-daemon` and nowhere
//! else: in `fleet-core` it would hand `fleet-app`, `fleet-cli` and `fleet-ui-kit` a transitive C
//! dependency and break the rule that `fleet-core` and `fleet-proto` are I/O-free.
//!
//! ```text
//! append(thread, event) ─► mpsc::UnboundedSender<Queued>  (each carries a oneshot)
//!                                     │
//!                                     ▼
//!                          one owned writer thread, "fleet-agent-db"
//!                          one rusqlite::Connection (read-write)
//!                                     │
//!                        BEGIN IMMEDIATE … COMMIT      (one per batch)
//!                          log row + projections + head, together
//!                                     │
//!                        oneshot reply ─► caller broadcasts
//!
//! summaries / load / window / read_record ─► Semaphore(2) ─► spawn_blocking ─► read-only conn
//! ```
//!
//! **The invariant this file exists to protect:** the event append, every projection row it
//! produces, and the head and cursor bumps all happen inside *one* transaction, and that
//! transaction commits *before* the reply resolves and therefore before anything is broadcast. By
//! the time [`SqliteAgentStore::append`] returns, every read model already reflects the event.
//! That is what makes reads instant and what lets the thread list repaint from denormalized
//! counters in one frame; there is no eventually-consistent projector anywhere on the write path.
//!
//! **Module layout.** [`schema`] is the SQL text, [`migrations`] the forward-only ladder,
//! [`import`] the one-shot NDJSON migration, [`project`] the event → rows projector, [`list`] the
//! one-`SELECT` thread list, [`index`] the `AgentThreadRecord` mapping, [`writer`] the owned
//! writer thread, [`read`] the read-only pool and its bounded queries, [`cursor`] the pagination
//! cursor.
//!
//! **Two deviations from the NDJSON seam this replaces,** both forced by the design:
//!
//! - `open` is fallible. A database the daemon cannot open or migrate leaves the agent service
//!   with half a truth, and an infallible constructor could only hide that until the first append.
//! - The I/O methods are `async`. A write is answered by the owned writer thread through a
//!   `oneshot` the caller awaits, and a read runs under `spawn_blocking`; blocking a tokio worker
//!   on either is the thing this store was built to stop doing.

// The pagination cursor and the bounded window read below it are the read path
// `docs/NATIVE-AGENTS.md` §9 specifies, and they are complete and tested. Nothing in production
// reaches them yet: `AgentThreadSnapshot` carries no cursor field, so exposing a page needs a
// `fleet-proto` change that belongs with the live path, not with this store.
#[allow(dead_code)]
mod cursor;
mod import;
mod index;
mod list;
mod migrations;
mod project;
mod read;
mod schema;
#[cfg(test)]
mod tests;
mod writer;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use fleet_core::agents::{AgentThreadSummary, Seq, SeqEvent, ThreadId};

use super::{AGENT_INDEX_VERSION, AgentIndex, AgentThreadRecord};
use cursor::TranscriptCursor;
pub(crate) use list::BootWork;
use read::ReaderPool;
pub(crate) use read::TranscriptWindow;
use writer::Writer;

/// The ceiling on one page, whatever a caller asks for.
///
/// A page's size is a wire-budget decision, not a caller's: the budget it protects is first paint
/// staying around 100 KB on the heaviest threads. A caller that wants more history pages for it
/// with the cursor the previous page returned.
#[allow(dead_code)]
const MAX_PAGE_EVENTS: usize = 256;

/// The native-agent transcript store: one log, every read model, one owned writer.
///
/// Cloning shares the writer thread and the reader pool. The last clone to drop shuts the writer
/// down: it drains its inbox, commits, runs `PRAGMA optimize`, and is given two seconds to finish.
#[derive(Clone)]
pub(crate) struct SqliteAgentStore {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    boot: BootWork,
    writer: Writer,
    readers: ReaderPool,
}

impl SqliteAgentStore {
    /// Opens and migrates the database at `database`, imports any legacy NDJSON logs beside it,
    /// and starts the writer.
    ///
    /// `database` is `FleetHome::agents_db_path()`; its parent directory is the store root, which
    /// also holds `attachments/` and the imported logs.
    ///
    /// Fatal for the agent service by design: a database this build cannot open or migrate leaves
    /// it with half a truth, exactly as an unreadable `state.json` does for the daemon.
    ///
    /// Everything here runs on the caller's thread, on the writer's own connection, **before** the
    /// writer thread exists — so the migration, the import and the boot-work census have the only
    /// handle to the file and no concurrency to reason about. The census is taken here for a
    /// reason that matters: the repair list is the state of the database at open, so a thread
    /// created after this returns can never appear in it and can never be "recovered" while live.
    ///
    /// It is also the one place in this store that blocks its caller on the disk. That is
    /// deliberate: it runs from `Services::build`, before the socket accepts a connection, so
    /// there is no request to starve, and a migration or an import that had not finished before
    /// the first request would be answering from a database in an unknown shape. Every path *after*
    /// construction is either an awaited `oneshot` on the writer thread or a `spawn_blocking` read.
    pub(crate) fn open(database: PathBuf) -> anyhow::Result<Self> {
        let root = database
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "the agent database path `{}` has no parent directory to use as the store root",
                    database.display()
                )
            })?;
        std::fs::create_dir_all(&root)
            .with_context(|| format!("create the agent store root `{}`", root.display()))?;

        let mut conn = writer::open(&database)?;
        import::run(&mut conn, &root).with_context(|| {
            format!(
                "import the legacy native-agent transcripts under `{}`",
                root.display()
            )
        })?;
        let boot = list::boot_work(&conn)?;
        let writer = Writer::spawn(conn)?;
        // After the writer, never before: the file and its schema have to exist before a
        // read-only open, which does not create one.
        let readers = ReaderPool::open(&database)?;
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                boot,
                writer,
                readers,
            }),
        })
    }

    /// The store root, which also holds `attachments/` and the imported NDJSON logs.
    ///
    /// Unused until attachments land: nothing above the store needs to know where the file is.
    #[allow(dead_code)]
    pub(crate) fn root(&self) -> &Path {
        &self.inner.root
    }

    /// The repair census taken when the database was opened.
    ///
    /// Nothing in it is urgent enough to hold up a daemon start, and everything in it is per
    /// thread, so the manager works through it in the background one thread at a time.
    pub(crate) fn boot_work(&self) -> &BootWork {
        &self.inner.boot
    }

    /// Appends one sequenced event, projects it, and advances the head — one transaction, durable
    /// before this returns.
    pub(crate) async fn append(&self, thread: ThreadId, event: &SeqEvent) -> anyhow::Result<()> {
        self.inner.writer.append(thread, event).await
    }

    /// Replays every retained event of a thread in sequence order.
    ///
    /// Bounded per statement rather than in total, because a caller rebuilding an in-memory
    /// projection needs the log dense from sequence 1. For the newest N events, use
    /// [`SqliteAgentStore::window`], which is bounded in total and pages backwards.
    pub(crate) async fn load(&self, thread: ThreadId) -> anyhow::Result<Vec<SeqEvent>> {
        self.inner
            .readers
            .read("replay a transcript", move |conn| {
                read::replay(conn, thread)
            })
            .await
    }

    /// Reads one bounded page of a transcript, with the watermark it is consistent with.
    ///
    /// `cursor` is the opaque token from a previous page. A malformed, foreign or
    /// unknown-version token degrades to the newest page and never errors: the client that sends
    /// one is usually a client that just reconnected.
    ///
    /// Unused until the wire carries a cursor; see the note on [`cursor`].
    #[allow(dead_code)]
    pub(crate) async fn window(
        &self,
        thread: ThreadId,
        cursor: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<TranscriptWindow> {
        let cursor = cursor.and_then(|token| TranscriptCursor::decode(token, thread));
        let limit = limit.min(MAX_PAGE_EVENTS);
        self.inner
            .readers
            .read("page a transcript", move |conn| {
                read::window(conn, thread, cursor, limit)
            })
            .await
    }

    /// Quarantines every logged event after `last` and rebuilds the thread's read model.
    ///
    /// The events are moved to `agent_events_quarantine`, never dropped: an event the reducer
    /// refused on replay has to leave the log so the next append does not collide with it, and it
    /// is the only evidence of why the replay stopped. `None` empties the thread's log.
    pub(crate) async fn truncate_after(
        &self,
        thread: ThreadId,
        last: Option<Seq>,
    ) -> anyhow::Result<()> {
        self.inner.writer.truncate_after(thread, last).await
    }

    /// Rebuilds one thread's read model by replaying its log over cleared projection rows.
    ///
    /// Boot repair and an explicit rebuild are the same code path. It is safe to run on a live
    /// thread: every upsert is keyed on a content-derived id, and the daemon-owned columns of
    /// `sessions` are left untouched so restart recovery does not lose a running provider.
    pub(crate) async fn rebuild(&self, thread: ThreadId) -> anyhow::Result<()> {
        self.inner.writer.rebuild(thread).await
    }

    /// Marks one thread's session failed, with the reason, and leaves its rows alone.
    ///
    /// This is what a thread whose replay cannot be repaired gets instead of a failed daemon
    /// start: it stays browsable read-only up to `projected_seq`, and no other thread is
    /// affected.
    pub(crate) async fn fail_session(
        &self,
        thread: ThreadId,
        message: String,
    ) -> anyhow::Result<()> {
        self.inner.writer.fail_session(thread, message).await
    }

    /// Reads every listed thread as a summary — one `SELECT` against `threads`, no log touched.
    pub(crate) async fn summaries(&self) -> anyhow::Result<Vec<AgentThreadSummary>> {
        self.inner
            .readers
            .read("list the agent threads", list::summaries)
            .await
    }

    /// Reads one thread's durable metadata, or `None` when no listed thread has that id.
    pub(crate) async fn read_record(
        &self,
        thread: ThreadId,
    ) -> anyhow::Result<Option<AgentThreadRecord>> {
        self.inner
            .readers
            .read("read an agent thread record", move |conn| {
                index::read_one(conn, thread)
            })
            .await
    }

    /// Upserts one thread's durable metadata, touching no projected column.
    ///
    /// This is what replaced the whole-file `index.json` rewrite the old store paid on every
    /// metadata transition: one row, one statement, no other thread read or written.
    pub(crate) async fn write_record(&self, record: &AgentThreadRecord) -> anyhow::Result<()> {
        self.inner.writer.write_record(record).await
    }

    /// Reads the whole thread index.
    ///
    /// Retained as the index seam `docs/NATIVE-AGENTS.md` §8 specifies, and exercised by
    /// [`tests`]; the daemon reads one record at a time through
    /// [`SqliteAgentStore::read_record`], because a manager that hydrates one thread must not
    /// read every other thread's row to find it.
    #[allow(dead_code)]
    pub(crate) async fn read_index(&self) -> anyhow::Result<AgentIndex> {
        self.inner
            .readers
            .read("read the agent index", read::read_index)
            .await
    }

    /// Replaces the thread index. A thread the new index omits is hidden, not erased.
    ///
    /// Retained for the same reason as [`SqliteAgentStore::read_index`]: hiding a thread by
    /// omission is the only delete semantics the store has, and the thread-delete verb §8 owes is
    /// what will call it.
    #[allow(dead_code)]
    pub(crate) async fn write_index(&self, index: &AgentIndex) -> anyhow::Result<()> {
        self.inner.writer.write_index(index).await
    }
}

impl std::fmt::Debug for SqliteAgentStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteAgentStore")
            .field("root", &self.inner.root)
            .finish_non_exhaustive()
    }
}
