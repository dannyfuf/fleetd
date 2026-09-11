//! The read side: two read-only connections behind a semaphore, used from `spawn_blocking`.
//!
//! Reads never touch the writer's connection. Under WAL a reader sees a consistent snapshot for
//! the life of its transaction *while the writer commits*, which is the property that makes an
//! atomic `(window, watermark)` read possible with no lock — and it is the reason the pool exists
//! at all rather than one shared handle behind a mutex of one.
//!
//! Two rules hold everywhere in this file:
//!
//! 1. **Every statement carries an explicit `LIMIT`.** A read that is bounded by "how much is
//!    there" is the exact failure the NDJSON store was replaced for, and it is invisible until a
//!    transcript is large. A read that must be *complete* pages instead of truncating.
//! 2. **A window and its watermark are read in one transaction.** A projector commit landing
//!    between two separate reads would return a head ahead of the window, the client would resume
//!    from too far, and the events in between would never be sent and never replayed — silent,
//!    permanent, and invisible until a user says "it skipped a message".

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, anyhow};
use fleet_core::agents::{Seq, SeqEvent, ThreadId, TurnId};
use rusqlite::{Connection, OpenFlags, params};
use tokio::sync::Semaphore;

use super::{
    AgentIndex,
    cursor::TranscriptCursor,
    index, migrations,
    project::{self, OptionalRow as _},
};
use fleet_core::agents::{ModelSelection, PermissionMode};

/// Read-only connections. Two, because a snapshot read and a pagination read are the two things
/// that can legitimately be in flight at once; a third would only queue behind the same disk.
const READERS: usize = 2;

/// Events per statement on the unbounded replay path, so peak memory is a chunk and not a
/// transcript.
const REPLAY_CHUNK: usize = 512;

/// The bounded page read. Hoisted so a test can assert its query plan against the exact text the
/// read path runs: an index the planner stops choosing costs a whole-thread scan, returns the
/// right rows, and is invisible until a transcript is large.
#[allow(dead_code)]
pub(super) const WINDOW_SQL: &str = "\
SELECT seq, at, raw, payload FROM agent_events \
 WHERE thread_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3";

/// The chunked replay read, hoisted for the same reason.
pub(super) const REPLAY_SQL: &str = "\
SELECT seq, at, raw, payload FROM agent_events \
 WHERE thread_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3";

/// One page of a transcript, with the watermark it is consistent with.
#[allow(dead_code)]
pub(crate) struct TranscriptWindow {
    /// The page, oldest first.
    pub(crate) events: Vec<SeqEvent>,
    /// The cursor for the next, older page, or `None` at the beginning of the thread.
    pub(crate) next: Option<TranscriptCursor>,
    /// The thread's log head, read in the same transaction as `events`.
    pub(crate) head_seq: Seq,
    /// The projector cursor. When it differs from `head_seq` this thread is mid-rebuild and the
    /// page is a prefix of the truth, which the caller must say rather than present as complete.
    pub(crate) projected_seq: Seq,
}

/// The turn keyset one bounded window covers, newest-first from `before`.
///
/// `(turn_id, start_seq)` and nothing else: the window's *content* comes from the reducer's
/// projection, and all this read has to establish is which turns the window contains and where
/// the next older page starts. It is one index-only statement against `idx_turns_keyset`, so the
/// `LIMIT` genuinely bounds the scan instead of sorting every turn in the thread first, and no
/// payload column is touched.
pub(super) fn turn_keyset(
    conn: &Connection,
    thread: ThreadId,
    before: Option<Seq>,
    limit: usize,
) -> anyhow::Result<TurnKeyset> {
    let id = thread.to_string();
    let before = before.map_or(i64::MAX, |seq| i64::try_from(seq.0).unwrap_or(i64::MAX));
    let mut statement = conn
        .prepare_cached(
            "SELECT turn_id, start_seq FROM turns \
              WHERE thread_id = ?1 AND start_seq < ?2 \
              ORDER BY start_seq DESC LIMIT ?3",
        )
        .context("prepare the turn keyset query")?;
    let mut rows = statement
        .query_map(
            params![id, before, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .context("query a turn keyset")?
        .collect::<Result<Vec<_>, _>>()
        .context("read a turn keyset")?;
    drop(statement);
    rows.reverse();
    let oldest = rows.first().map(|(_, start_seq)| *start_seq);
    // An exact probe rather than fetching limit+1 and slicing: the off-by-one in the slice
    // version only shows up on the page boundary.
    let has_more = match oldest {
        Some(oldest) => conn
            .prepare_cached("SELECT 1 FROM turns WHERE thread_id = ?1 AND start_seq < ?2 LIMIT 1")
            .context("prepare the older-turn probe")?
            .query_row(params![id, oldest], |row| row.get::<_, i64>(0))
            .optional_row()
            .context("probe for an older turn page")?
            .is_some(),
        None => false,
    };
    let turns = rows
        .into_iter()
        .filter_map(|(turn, start_seq)| match turn.parse::<TurnId>() {
            Ok(turn) => Some((turn, Seq(u64::try_from(start_seq).unwrap_or_default()))),
            Err(error) => {
                tracing::warn!(%thread, %turn, %error, "skipping an unparsable turn id in a window");
                None
            }
        })
        .collect::<Vec<_>>();
    Ok(TurnKeyset {
        oldest_seq: turns.first().map(|(_, seq)| *seq),
        turns,
        has_more,
    })
}

/// Which turns one window covers, and whether older ones exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnKeyset {
    /// The window's turns with their start sequences, chronological.
    pub(crate) turns: Vec<(TurnId, Seq)>,
    /// The start sequence of the oldest turn in the window, the page's lower bound.
    pub(crate) oldest_seq: Option<Seq>,
    /// Whether a turn older than `oldest_seq` exists.
    pub(crate) has_more: bool,
}

/// The harness runtime one thread's `sessions` row last recorded.
///
/// The composer gates its controls on these (`docs/NATIVE-AGENTS.md` §4.5), and they are *not* in
/// the reducer's projection: the projector writes them to `sessions` from `SessionConfigured` and
/// the projection keeps only what a transcript row needs. A windowed open that skipped this read
/// would blank every gated control on a cold open, which reads as a bug in the control rather
/// than as a missing read.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionRuntime {
    /// Active model, when the harness has reported one.
    pub(crate) model: Option<ModelSelection>,
    /// Active permission policy.
    pub(crate) mode: PermissionMode,
    /// Harness-native tool names.
    pub(crate) tools: Vec<String>,
    /// Harness-native slash command names.
    pub(crate) commands: Vec<String>,
    /// Harness-native skill names.
    pub(crate) skills: Vec<String>,
}

/// Reads one thread's session runtime, or `None` when no session row exists yet.
///
/// A column this build cannot decode degrades to its default rather than failing the read: a
/// model selection written by a newer daemon must not make a transcript unopenable.
pub(super) fn session_runtime(
    conn: &Connection,
    thread: ThreadId,
) -> anyhow::Result<Option<SessionRuntime>> {
    let row = conn
        .prepare_cached(
            "SELECT model_json, mode, tools_json, commands_json, skills_json \
               FROM sessions WHERE thread_id = ?1",
        )
        .context("prepare the session runtime query")?
        .query_row(params![thread.to_string()], |row| {
            Ok(SessionRow {
                model_json: row.get(0)?,
                mode: row.get(1)?,
                tools_json: row.get(2)?,
                commands_json: row.get(3)?,
                skills_json: row.get(4)?,
            })
        })
        .optional_row()
        .with_context(|| format!("read the session runtime of thread {thread}"))?;
    Ok(row.map(|row| row.decode(thread)))
}

/// One session row, still encoded.
struct SessionRow {
    model_json: Option<String>,
    mode: Option<String>,
    tools_json: Option<String>,
    commands_json: Option<String>,
    skills_json: Option<String>,
}

impl SessionRow {
    fn decode(self, thread: ThreadId) -> SessionRuntime {
        SessionRuntime {
            model: decode_column(self.model_json.as_deref(), thread, "model selection"),
            // The column stores the bare discriminant, so it becomes JSON before it decodes.
            mode: self
                .mode
                .as_deref()
                .and_then(|mode| {
                    decode_column(
                        Some(&serde_json::Value::String(mode.to_owned()).to_string()),
                        thread,
                        "permission mode",
                    )
                })
                .unwrap_or_default(),
            tools: decode_column(self.tools_json.as_deref(), thread, "tool names")
                .unwrap_or_default(),
            commands: decode_column(self.commands_json.as_deref(), thread, "command names")
                .unwrap_or_default(),
            skills: decode_column(self.skills_json.as_deref(), thread, "skill names")
                .unwrap_or_default(),
        }
    }
}

/// Decodes one JSON column, warning and degrading rather than failing the read.
fn decode_column<T: serde::de::DeserializeOwned>(
    encoded: Option<&str>,
    thread: ThreadId,
    what: &str,
) -> Option<T> {
    let encoded = encoded?;
    match serde_json::from_str(encoded) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(%thread, %error, what, "ignoring a session column this build cannot decode");
            None
        }
    }
}

/// A fixed pool of read-only connections.
pub(super) struct ReaderPool {
    path: PathBuf,
    slots: Semaphore,
    /// Connections not currently checked out. A panicking read loses its connection, so a
    /// checkout opens a replacement rather than assuming the stack is full.
    idle: Mutex<Vec<Connection>>,
}

impl ReaderPool {
    /// Opens the pool. The database must already exist: the writer creates and migrates it first,
    /// and a read-only open of a missing file is an error rather than a fresh database.
    pub(super) fn open(path: &Path) -> anyhow::Result<Self> {
        let mut idle = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            idle.push(open_reader(path)?);
        }
        Ok(Self {
            path: path.to_path_buf(),
            slots: Semaphore::new(READERS),
            idle: Mutex::new(idle),
        })
    }

    /// Runs one blocking read on a pooled connection.
    ///
    /// `spawn_blocking` is right here and wrong for the writer: a read returns, so it occupies a
    /// pool slot for the length of a query rather than the life of the daemon.
    pub(super) async fn read<T, F>(&self, what: &'static str, task: F) -> anyhow::Result<T>
    where
        F: FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .slots
            .acquire()
            .await
            .map_err(|_closed| anyhow!("the agent database reader pool is closed"))?;
        let conn = self.checkout()?;
        let joined = tokio::task::spawn_blocking(move || {
            let result = task(&conn);
            (conn, result)
        })
        .await;
        let outcome = match joined {
            Ok((conn, result)) => {
                self.checkin(conn);
                result
            }
            // The connection went down with the task, so the pool is one short until the next
            // checkout opens a replacement. That is why `checkout` can open.
            Err(error) => Err(anyhow!("the agent database read `{what}` failed: {error}")),
        };
        drop(permit);
        outcome
    }

    fn checkout(&self) -> anyhow::Result<Connection> {
        let taken = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop();
        match taken {
            Some(conn) => Ok(conn),
            None => open_reader(&self.path),
        }
    }

    fn checkin(&self, conn: Connection) {
        let mut idle = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if idle.len() < READERS {
            idle.push(conn);
        }
    }
}

fn open_reader(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open the agent database `{}` read-only", path.display()))?;
    migrations::configure_reader(&conn)?;
    Ok(conn)
}

/// Replays one thread's whole log, one bounded chunk at a time.
///
/// This is the seam's `load`, and it exists for the caller that must rebuild an in-memory
/// projection from sequence 1. It is bounded per *statement*, not in total: the events are dense
/// from 1 and a reducer that received a suffix would refuse every one of them. A caller that only
/// needs the newest turns wants [`window`], which is bounded in total.
pub(super) fn replay(conn: &Connection, thread: ThreadId) -> anyhow::Result<Vec<SeqEvent>> {
    // One snapshot for the whole replay, so a commit landing mid-chunk cannot produce a hole.
    let transaction = conn
        .unchecked_transaction()
        .context("begin an agent database read snapshot")?;
    let id = thread.to_string();
    let mut events = Vec::new();
    let mut after = 0_i64;
    loop {
        let mut statement = transaction
            .prepare_cached(REPLAY_SQL)
            .context("prepare the transcript replay query")?;
        let chunk = statement
            .query_map(
                params![id, after, i64::try_from(REPLAY_CHUNK).unwrap_or(i64::MAX)],
                decode_row,
            )
            .context("query a transcript replay chunk")?
            .collect::<Result<Vec<_>, _>>()
            .context("read a transcript replay chunk")?;
        let short = chunk.len() < REPLAY_CHUNK;
        for row in chunk {
            let event = row.decode(thread)?;
            after = i64::try_from(event.seq.0).unwrap_or(i64::MAX);
            events.push(event);
        }
        if short {
            return Ok(events);
        }
    }
}

/// Reads one bounded page of a transcript, newest first, with the watermark it is consistent with.
///
/// `cursor` is exclusive: the page is strictly older than the sequence the client already has.
/// `None` serves the newest page, which is also what a malformed or foreign cursor degrades to.
#[allow(dead_code)]
pub(super) fn window(
    conn: &Connection,
    thread: ThreadId,
    cursor: Option<TranscriptCursor>,
    limit: usize,
) -> anyhow::Result<TranscriptWindow> {
    let transaction = conn
        .unchecked_transaction()
        .context("begin an agent database window snapshot")?;
    let id = thread.to_string();
    let before = cursor.map_or(i64::MAX, |cursor| {
        i64::try_from(cursor.before_seq().0).unwrap_or(i64::MAX)
    });
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);

    let mut statement = transaction
        .prepare_cached(WINDOW_SQL)
        .context("prepare the transcript window query")?;
    let mut rows = statement
        .query_map(params![id, before, limit], decode_row)
        .context("query a transcript window")?
        .collect::<Result<Vec<_>, _>>()
        .context("read a transcript window")?;
    drop(statement);
    rows.reverse();
    let oldest = rows.first().map(|row| row.seq);
    let events = rows
        .into_iter()
        .map(|row| row.decode(thread))
        .collect::<anyhow::Result<Vec<_>>>()?;

    // An exact probe rather than fetching limit+1 and slicing: the off-by-one in the slice
    // version is the kind of bug that only shows up on the page boundary.
    let more = match oldest {
        Some(oldest) => transaction
            .query_row(
                "SELECT 1 FROM agent_events WHERE thread_id = ?1 AND seq < ?2 LIMIT 1",
                params![id, oldest],
                |row| row.get::<_, i64>(0),
            )
            .optional_row()
            .context("probe for an older transcript page")?
            .is_some(),
        None => false,
    };

    let (head_seq, projected_seq) = watermark(&transaction, &id)?;
    Ok(TranscriptWindow {
        events,
        next: more.then(|| {
            TranscriptCursor::new(
                thread,
                Seq(oldest.map_or(0, |seq| u64::try_from(seq).unwrap_or_default())),
            )
        }),
        head_seq,
        projected_seq,
    })
}

/// Reads the thread index.
pub(super) fn read_index(conn: &Connection) -> anyhow::Result<AgentIndex> {
    index::read(conn)
}

/// The log head and the projector cursor of one thread.
#[allow(dead_code)]
fn watermark(transaction: &rusqlite::Transaction<'_>, id: &str) -> anyhow::Result<(Seq, Seq)> {
    let row: Option<(i64, i64)> = transaction
        .query_row(
            "SELECT head_seq, projected_seq FROM threads WHERE thread_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional_row()
        .context("read a thread watermark")?;
    let (head, projected) = row.unwrap_or((0, 0));
    Ok((
        Seq(u64::try_from(head).unwrap_or_default()),
        Seq(u64::try_from(projected).unwrap_or_default()),
    ))
}

/// One log row, still encoded.
struct EventRow {
    seq: i64,
    at: i64,
    raw: Option<String>,
    payload: String,
}

impl EventRow {
    fn decode(self, thread: ThreadId) -> anyhow::Result<SeqEvent> {
        project::decode_event(self.seq, self.at, self.raw, &self.payload)
            .with_context(|| format!("decode event {} of thread {thread}", self.seq))
    }
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        seq: row.get(0)?,
        at: row.get(1)?,
        raw: row.get(2)?,
        payload: row.get(3)?,
    })
}
