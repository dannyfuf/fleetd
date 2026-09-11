//! `AgentIndex` over SQL: the seam's `read_index` / `write_index` against the `threads` table.
//!
//! `index.json` is gone. The whole-file rewrite on every metadata change, the linear
//! `iter_mut().find(…)` on every persisted event, the atomic-rename dance, the
//! `index.json.broken-*` quarantine and the orphan directory scan all collapse into upserts on
//! one row (`docs/NATIVE-AGENTS.md` §8).
//!
//! Two mappings carry the semantics of the file this replaces:
//!
//! - **A thread missing from a written index is soft-deleted, not erased.** The old store's index
//!   was authoritative for *visibility* only: writing an index without a thread hid it and left
//!   its log untouched on disk. `deleted_at` reproduces that exactly, and a later index that
//!   names the thread again clears it — which the old store also did, by rewriting the file.
//! - **`provider` is stored as the open string the wire carries**, so a thread written by a build
//!   that knows a provider this one does not is skipped on read with a warning rather than
//!   failing the whole index. The database must never be the thing that cannot store a value the
//!   wire can carry.

use anyhow::Context;
use chrono::DateTime;
use fleet_core::agents::{
    AgentKind, ModelSelection, PermissionMode, SessionState, ThreadId, TurnOutcome,
};
use rusqlite::{Connection, Transaction, params, params_from_iter};
use serde_json::Value;

use super::{
    AgentIndex, AgentThreadRecord,
    project::{OptionalRow as _, discriminant, json_text, ms, optional_json},
};

/// Threads read per statement. Every read in this store carries an explicit `LIMIT`; the index is
/// the one read that must still be *complete*, so it pages on `(created_at, thread_id)` instead of
/// truncating.
const INDEX_PAGE: usize = 500;

/// Replaces the index: upserts every record, and soft-deletes every thread the index omits.
pub(super) fn write(transaction: &Transaction<'_>, index: &AgentIndex) -> anyhow::Result<()> {
    validate(index)?;
    for record in &index.threads {
        upsert(transaction, record)?;
    }

    let ids = index
        .threads
        .iter()
        .map(|record| record.thread.to_string())
        .collect::<Vec<_>>();
    let mut parameters: Vec<rusqlite::types::Value> =
        vec![chrono::Utc::now().timestamp_millis().into()];
    parameters.extend(ids.iter().map(|id| id.clone().into()));
    let placeholders = (0..ids.len())
        .map(|position| format!("?{}", position + 2))
        .collect::<Vec<_>>()
        .join(", ");
    let statement = if ids.is_empty() {
        "UPDATE threads SET deleted_at = ?1 WHERE deleted_at IS NULL".to_owned()
    } else {
        format!(
            "UPDATE threads SET deleted_at = ?1 \
             WHERE deleted_at IS NULL AND thread_id NOT IN ({placeholders})"
        )
    };
    let hidden = transaction
        .execute(&statement, params_from_iter(parameters.iter()))
        .context("hide the agent threads an index rewrite dropped")?;
    if hidden > 0 {
        tracing::info!(
            hidden,
            "an index rewrite hid native-agent threads; their logs are untouched"
        );
    }
    Ok(())
}

/// Reads one thread's record, or `None` when no listed thread has that id.
///
/// This is the per-thread half of [`read`], and it is what lazy hydration uses: a manager that
/// hydrates one thread must not read every other thread's row to find it.
pub(super) fn read_one(
    conn: &Connection,
    thread: ThreadId,
) -> anyhow::Result<Option<AgentThreadRecord>> {
    let row = conn
        .prepare_cached(
            "SELECT t.thread_id, t.worktree_id, t.provider, t.title, t.created_at, \
                    t.last_activity_at, t.last_outcome, s.resume_cursor, s.model_json, s.mode \
               FROM threads t LEFT JOIN sessions s ON s.thread_id = t.thread_id \
              WHERE t.thread_id = ?1 AND t.deleted_at IS NULL",
        )
        .context("prepare the single-thread index query")?
        .query_row(params![thread.to_string()], |row| {
            Ok(IndexRow {
                thread: row.get(0)?,
                worktree: row.get(1)?,
                provider: row.get(2)?,
                title: row.get(3)?,
                created_at: row.get(4)?,
                last_activity_at: row.get(5)?,
                last_outcome: row.get(6)?,
                resume_cursor: row.get(7)?,
                model_json: row.get(8)?,
                mode: row.get(9)?,
            })
        })
        .optional_row()
        .with_context(|| format!("read the index row of thread {thread}"))?;
    row.map(IndexRow::into_record).transpose()
}

/// Reads the index back, newest metadata included, in creation order.
pub(super) fn read(conn: &Connection) -> anyhow::Result<AgentIndex> {
    let mut threads = Vec::new();
    let mut after: Option<(i64, String)> = None;
    loop {
        let (created_at, thread_id) = after.clone().unwrap_or_else(|| (i64::MIN, String::new()));
        let mut statement = conn
            .prepare_cached(
                "SELECT t.thread_id, t.worktree_id, t.provider, t.title, t.created_at, \
                        t.last_activity_at, t.last_outcome, s.resume_cursor, s.model_json, s.mode \
                   FROM threads t LEFT JOIN sessions s ON s.thread_id = t.thread_id \
                  WHERE t.deleted_at IS NULL \
                    AND (t.created_at > ?1 OR (t.created_at = ?1 AND t.thread_id > ?2)) \
                  ORDER BY t.created_at ASC, t.thread_id ASC \
                  LIMIT ?3",
            )
            .context("prepare the agent index query")?;
        let page = statement
            .query_map(
                params![
                    created_at,
                    thread_id,
                    i64::try_from(INDEX_PAGE).unwrap_or(i64::MAX)
                ],
                |row| {
                    Ok(IndexRow {
                        thread: row.get(0)?,
                        worktree: row.get(1)?,
                        provider: row.get(2)?,
                        title: row.get(3)?,
                        created_at: row.get(4)?,
                        last_activity_at: row.get(5)?,
                        last_outcome: row.get(6)?,
                        resume_cursor: row.get(7)?,
                        model_json: row.get(8)?,
                        mode: row.get(9)?,
                    })
                },
            )
            .context("query the agent index")?
            .collect::<Result<Vec<_>, _>>()
            .context("decode the agent index")?;
        if page.is_empty() {
            return Ok(AgentIndex {
                version: super::AGENT_INDEX_VERSION,
                threads,
            });
        }
        after = page.last().map(|row| (row.created_at, row.thread.clone()));
        for row in page {
            match row.into_record() {
                Ok(record) => threads.push(record),
                Err(error) => {
                    tracing::warn!(%error, "skipping a native-agent thread this build cannot read");
                }
            }
        }
    }
}

/// Upserts the metadata half of one thread row plus the session columns the index carries.
///
/// It touches no projected column: `head_seq`, `projected_seq`, `open_gate_count`, `attention`,
/// `running_turn_id` and the `last_*` cursors belong to the projector, and an index write that
/// reset them would make the thread list disagree with the log until the next event.
pub(super) fn upsert(
    transaction: &Transaction<'_>,
    record: &AgentThreadRecord,
) -> anyhow::Result<()> {
    let provider = discriminant(&record.provider, "AgentKind")?;
    let mode = discriminant(&record.mode, "PermissionMode")?;
    let outcome = optional_json(record.last_outcome.as_ref())?;
    transaction
        .execute(
            "INSERT INTO threads (thread_id, worktree_id, provider, title, created_at, \
             last_activity_at, session_state, attention, last_outcome) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT (thread_id) DO UPDATE SET \
             worktree_id = excluded.worktree_id, provider = excluded.provider, \
             title = excluded.title, created_at = excluded.created_at, \
             last_activity_at = MAX(threads.last_activity_at, excluded.last_activity_at), \
             last_outcome = excluded.last_outcome, deleted_at = NULL",
            params![
                record.thread.to_string(),
                record.worktree.to_string(),
                provider,
                record.title,
                ms(record.created),
                ms(record.last_activity),
                discriminant(&SessionState::Starting, "SessionState")?,
                json_text(&fleet_core::agents::Attention::Idle)?,
                outcome,
            ],
        )
        .with_context(|| format!("record thread {} in the agent index", record.thread))?;
    transaction
        .execute(
            "INSERT INTO sessions (thread_id, provider, resume_cursor, model_json, mode, state) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT (thread_id) DO UPDATE SET \
             provider = excluded.provider, resume_cursor = excluded.resume_cursor, \
             model_json = excluded.model_json, mode = excluded.mode",
            params![
                record.thread.to_string(),
                provider,
                record.resume_cursor,
                optional_json(record.model.as_ref())?,
                mode,
                discriminant(&SessionState::Starting, "SessionState")?,
            ],
        )
        .with_context(|| format!("record the session of thread {}", record.thread))?;
    Ok(())
}

/// Refuses an index this build must not write, exactly as the file store did.
fn validate(index: &AgentIndex) -> anyhow::Result<()> {
    if index.version != super::AGENT_INDEX_VERSION {
        anyhow::bail!(
            "unsupported native-agent index version {}; expected {}",
            index.version,
            super::AGENT_INDEX_VERSION
        );
    }
    let mut seen = std::collections::HashSet::new();
    for record in &index.threads {
        if !seen.insert(record.thread) {
            anyhow::bail!("duplicate native-agent thread {} in index", record.thread);
        }
    }
    Ok(())
}

/// One joined row, before it is decoded into a record.
struct IndexRow {
    thread: String,
    worktree: String,
    provider: String,
    title: String,
    created_at: i64,
    last_activity_at: i64,
    last_outcome: Option<String>,
    resume_cursor: Option<String>,
    model_json: Option<String>,
    mode: Option<String>,
}

impl IndexRow {
    fn into_record(self) -> anyhow::Result<AgentThreadRecord> {
        let thread = self
            .thread
            .parse::<ThreadId>()
            .with_context(|| format!("decode thread id `{}`", self.thread))?;
        let provider: AgentKind = serde_json::from_value(Value::String(self.provider.clone()))
            .with_context(|| {
                format!(
                    "thread {thread} names provider `{}`, which this build does not implement",
                    self.provider
                )
            })?;
        let mode = match self.mode {
            Some(mode) => serde_json::from_value(Value::String(mode.clone()))
                .with_context(|| format!("thread {thread} names permission mode `{mode}`"))?,
            None => PermissionMode::default(),
        };
        let model: Option<ModelSelection> = self
            .model_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .with_context(|| format!("decode the model selection of thread {thread}"))?;
        let last_outcome: Option<TurnOutcome> = self
            .last_outcome
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .with_context(|| format!("decode the last turn outcome of thread {thread}"))?;
        Ok(AgentThreadRecord {
            thread,
            worktree: fleet_core::ids::WorktreeId::try_from(self.worktree.clone())
                .with_context(|| format!("thread {thread} names worktree `{}`", self.worktree))?,
            provider,
            title: self.title,
            created: time(self.created_at, thread)?,
            last_activity: time(self.last_activity_at, thread)?,
            resume_cursor: self.resume_cursor,
            model,
            mode,
            last_outcome,
        })
    }
}

fn time(at: i64, thread: ThreadId) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    DateTime::from_timestamp_millis(at)
        .ok_or_else(|| anyhow::anyhow!("thread {thread} carries an out-of-range timestamp {at}"))
}
