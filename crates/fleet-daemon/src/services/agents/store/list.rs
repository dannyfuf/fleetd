//! The thread list: one `SELECT` against `threads`, and nothing else.
//!
//! This module is the reason the `threads` table carries denormalized columns at all
//! (`docs/NATIVE-AGENTS.md` §8). Answering `AgentThreadList` used to mean replaying every
//! thread's whole log at daemon start and then mapping over the in-memory projections it built;
//! it now means reading one row per thread from one table. `items`, `turns`, `gates` and
//! `agent_events` are never touched, and neither is any log.
//!
//! Every column this query reads is written by the projector inside the same transaction as the
//! event that changed it, so the list is never behind the log — and it cannot be *ahead* of it
//! either, because the projector cursor and the head advance together.
//!
//! `attention` is stored as this daemon derives it with **nothing seen**, and `last_completed_seq`
//! / `last_nonterminal_seq` travel with it so the reading client can re-derive its own two
//! seen-relative states with [`fleet_core::agents::AgentThreadSummary::attention_for`] (§3.3). The
//! daemon deliberately keeps no client cursor here.

use anyhow::Context;
use fleet_core::{
    agents::{AgentKind, AgentThreadSummary, Attention, Seq, SessionState, ThreadId, TurnState},
    ids::{HostId, WorktreeId},
};
use rusqlite::{Connection, params};

/// Rows per statement. The list must be *complete* — it is the daemon's snapshot — so it pages on
/// `(created_at, thread_id)` rather than truncating, which is the same discipline
/// [`super::index::read`] uses and the same reason: every read in this store carries an explicit
/// `LIMIT`.
const LIST_PAGE: usize = 500;

/// The list query, hoisted so a test can assert its plan against the exact text the read runs.
pub(super) const LIST_SQL: &str = "\
SELECT thread_id, worktree_id, owner_host, provider, title, attention, session_state, turn_json, \
       head_seq, last_activity_at, last_completed_seq, last_nonterminal_seq, exit_code, created_at \
  FROM threads \
 WHERE deleted_at IS NULL \
   AND (created_at > ?1 OR (created_at = ?1 AND thread_id > ?2)) \
 ORDER BY created_at ASC, thread_id ASC \
 LIMIT ?3";

/// Reads every listed thread as a summary, in creation order.
///
/// A row this build cannot decode — a provider or a worktree another build wrote — is skipped
/// with a warning rather than failing the list: one unreadable thread must not take the whole
/// list out of the daemon's snapshot.
pub(super) fn summaries(conn: &Connection) -> anyhow::Result<Vec<AgentThreadSummary>> {
    let mut summaries = Vec::new();
    let mut after: Option<(i64, String)> = None;
    loop {
        let (created_at, thread_id) = after.clone().unwrap_or((i64::MIN, String::new()));
        let mut statement = conn
            .prepare_cached(LIST_SQL)
            .context("prepare the agent thread list query")?;
        let page = statement
            .query_map(
                params![
                    created_at,
                    thread_id,
                    i64::try_from(LIST_PAGE).unwrap_or(i64::MAX)
                ],
                decode_row,
            )
            .context("query the agent thread list")?
            .collect::<Result<Vec<_>, _>>()
            .context("read the agent thread list")?;
        if page.is_empty() {
            return Ok(summaries);
        }
        after = page.last().map(|row| (row.created_at, row.thread.clone()));
        for row in page {
            let id = row.thread.clone();
            match row.into_summary() {
                Ok(summary) => summaries.push(summary),
                Err(error) => tracing::warn!(
                    thread = %id,
                    %error,
                    "skipping a native-agent thread this build cannot list"
                ),
            }
        }
    }
}

/// One list row, still encoded.
struct ListRow {
    thread: String,
    worktree: String,
    owner_host: Option<String>,
    provider: String,
    title: String,
    attention: String,
    session_state: String,
    turn_json: String,
    head_seq: i64,
    last_activity_at: i64,
    last_completed_seq: Option<i64>,
    last_nonterminal_seq: Option<i64>,
    exit_code: Option<i64>,
    created_at: i64,
}

impl ListRow {
    fn into_summary(self) -> anyhow::Result<AgentThreadSummary> {
        let thread = self
            .thread
            .parse::<ThreadId>()
            .with_context(|| format!("decode thread id `{}`", self.thread))?;
        let provider: AgentKind = serde_json::from_value(serde_json::Value::String(
            self.provider.clone(),
        ))
        .with_context(|| {
            format!(
                "thread {thread} names provider `{}`, which this build does not implement",
                self.provider
            )
        })?;
        let session = decode_session_state(&self.session_state).with_context(|| {
            format!(
                "thread {thread} names session state `{}`",
                self.session_state
            )
        })?;
        let attention: Attention = serde_json::from_str(&self.attention)
            .with_context(|| format!("decode the attention of thread {thread}"))?;
        let turn: TurnState = serde_json::from_str(&self.turn_json)
            .with_context(|| format!("decode the turn state of thread {thread}"))?;
        let host = self
            .owner_host
            .as_deref()
            .map(|host| HostId::try_from(host.to_owned()))
            .transpose()
            .with_context(|| format!("thread {thread} names an unusable owner host"))?;
        Ok(AgentThreadSummary {
            thread,
            worktree: WorktreeId::try_from(self.worktree.clone())
                .with_context(|| format!("thread {thread} names worktree `{}`", self.worktree))?,
            host,
            provider,
            title: self.title,
            attention,
            session,
            turn,
            last_seq: seq(self.head_seq),
            // The reducer holds `last_activity` only once an event has been applied, and
            // `last_activity_at` starts life as `created_at`, so the head is what distinguishes
            // "no events yet" from "an event at creation time".
            last_activity: (self.head_seq > 0)
                .then(|| chrono::DateTime::from_timestamp_millis(self.last_activity_at))
                .flatten(),
            last_completed_seq: self.last_completed_seq.map(seq),
            last_nonterminal_seq: self.last_nonterminal_seq.map(seq),
            exit_code: self.exit_code.and_then(|code| i32::try_from(code).ok()),
        })
    }
}

fn decode_session_state(encoded: &str) -> anyhow::Result<SessionState> {
    match encoded {
        "starting" => Ok(SessionState::Starting),
        "ready" => Ok(SessionState::Ready),
        "running" => Ok(SessionState::Running),
        "stopped" => Ok(SessionState::Stopped),
        "error" => Ok(SessionState::Error),
        // Only `Waiting` has payload. New rows retain it as JSON while unit states remain the
        // indexed discriminants shipped by the schema.
        other => serde_json::from_str(other).context("decode a payload-bearing session state"),
    }
}

fn seq(value: i64) -> Seq {
    Seq(u64::try_from(value).unwrap_or_default())
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ListRow> {
    Ok(ListRow {
        thread: row.get(0)?,
        worktree: row.get(1)?,
        owner_host: row.get(2)?,
        provider: row.get(3)?,
        title: row.get(4)?,
        attention: row.get(5)?,
        session_state: row.get(6)?,
        turn_json: row.get(7)?,
        head_seq: row.get(8)?,
        last_activity_at: row.get(9)?,
        last_completed_seq: row.get(10)?,
        last_nonterminal_seq: row.get(11)?,
        exit_code: row.get(12)?,
        created_at: row.get(13)?,
    })
}

/// What one start owes the database, taken as a census the moment it is opened.
///
/// Both lists are per thread and neither is urgent, which is the whole point: a daemon start does
/// no replay at all, and the manager works through these in the background one thread at a time
/// (`docs/NATIVE-AGENTS.md` §8). The census is taken before the writer thread exists, so a thread
/// created after the store opens can never be in it — and therefore a live thread can never be
/// mistaken for an orphan of the previous run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BootWork {
    /// Threads whose projector cursor fell behind their log head and need one replay.
    pub(crate) unprojected: Vec<ThreadId>,
    /// Threads the previous run left claiming a provider it no longer has (§6). They are settled
    /// explicitly, as appended events, when the manager hydrates them.
    ///
    /// Mirrored threads are excluded by the query, and that is an authority rule rather than an
    /// optimisation: settling an orphan *appends events*, and the local daemon must never mint a
    /// sequence for a thread it does not own (`docs/NATIVE-AGENTS.md` §9.3).
    pub(crate) orphans: Vec<ThreadId>,
    /// Threads another host owns, as the durable mirror recorded them.
    ///
    /// The router's id mappings are rebuilt from a live link, so without this census a mirrored
    /// thread would classify as *local* between a daemon start and the owner's first snapshot —
    /// which is exactly the window in which a mutation must be refused rather than applied.
    pub(crate) mirrored: Vec<(ThreadId, HostId)>,
}

/// Takes the census. Both queries are index-only lookups against `threads`.
pub(super) fn boot_work(conn: &Connection) -> anyhow::Result<BootWork> {
    Ok(BootWork {
        // Served by `idx_threads_unprojected`, a partial index, so this costs nothing on a
        // database where every thread is healthy.
        unprojected: ids(
            conn,
            "SELECT thread_id FROM threads WHERE projected_seq < head_seq LIMIT ?1",
            "the unprojected threads",
        )?,
        // `starting`, `ready` and `running` are all live-provider states, and a running turn is
        // one too: the child is gone either way after a restart.
        orphans: ids(
            conn,
            "SELECT thread_id FROM threads \
              WHERE deleted_at IS NULL AND owner_host IS NULL \
                AND (session_state IN ('starting', 'ready', 'running') \
                     OR running_turn_id IS NOT NULL) \
              ORDER BY last_activity_at DESC LIMIT ?1",
            "the orphaned threads",
        )?,
        // Served by `idx_threads_owner`, a partial index, so this costs nothing on a daemon that
        // federates nothing.
        mirrored: mirrored(conn)?,
    })
}

/// Threads the census may name in one pass.
///
/// Bounded like every other read here. A start that finds more than this many threads to repair
/// repairs the most recently active ones and the next start takes the rest, which is strictly
/// better than a start that walks an unbounded list.
const CENSUS_LIMIT: usize = 1_000;

/// The durable owner of every mirrored thread, newest first.
fn mirrored(conn: &Connection) -> anyhow::Result<Vec<(ThreadId, HostId)>> {
    let mut statement = conn
        .prepare(
            "SELECT thread_id, owner_host FROM threads \
              WHERE owner_host IS NOT NULL AND deleted_at IS NULL \
              ORDER BY last_activity_at DESC LIMIT ?1",
        )
        .context("prepare the query for the mirrored threads")?;
    let rows = statement
        .query_map(
            params![i64::try_from(CENSUS_LIMIT).unwrap_or(i64::MAX)],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .context("query the mirrored threads")?
        .collect::<Result<Vec<_>, _>>()
        .context("read the mirrored threads")?;
    Ok(rows
        .into_iter()
        .filter_map(|(thread, host)| {
            match (thread.parse::<ThreadId>(), HostId::try_from(host.clone())) {
                (Ok(thread), Ok(host)) => Some((thread, host)),
                (thread_id, owner) => {
                    tracing::warn!(
                        %thread,
                        %host,
                        thread_error = thread_id.err().map(|error| error.to_string()),
                        host_error = owner.err().map(|error| error.to_string()),
                        "skipping an unusable mirrored thread in the boot census"
                    );
                    None
                }
            }
        })
        .collect())
}

fn ids(conn: &Connection, sql: &str, what: &str) -> anyhow::Result<Vec<ThreadId>> {
    let mut statement = conn
        .prepare(sql)
        .with_context(|| format!("prepare the query for {what}"))?;
    let rows = statement
        .query_map(
            params![i64::try_from(CENSUS_LIMIT).unwrap_or(i64::MAX)],
            |row| row.get::<_, String>(0),
        )
        .with_context(|| format!("query {what}"))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("read {what}"))?;
    Ok(rows
        .into_iter()
        .filter_map(|id| match id.parse::<ThreadId>() {
            Ok(thread) => Some(thread),
            Err(error) => {
                tracing::warn!(%id, %error, "skipping an unparsable thread id in the boot census");
                None
            }
        })
        .collect())
}
