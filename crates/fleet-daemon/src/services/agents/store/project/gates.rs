//! Gate lifecycle writes and the blocked-window accounting they share.
//!
//! A run of overlapping gates is one wait: whichever gate closes last charges the window to the
//! turn that was parked on it, and a gate that outlives an earlier one inherits its start.

use anyhow::{Context, bail};
use fleet_core::agents::{GateAnswer, GateId, GateResolver, ThreadId};
use rusqlite::{Transaction, params};

use super::codec::{OptionalRow, discriminant, execute, json_text};

/// Closes a gate and charges the wait it ends to the turn that was blocked on it.
pub(super) fn resolve_gate(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    gate: &GateId,
    answer: &GateAnswer,
    by: &fleet_core::agents::GateResolver,
    seq: i64,
    at: i64,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    // The thread has been blocked since the earliest gate of this window opened, so the window
    // start is read *before* this gate leaves the open set.
    let since: Option<i64> = transaction
        .query_row(
            "SELECT MIN(blocked_since) FROM gates WHERE thread_id = ?1 AND status = 'open'",
            params![id],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read the blocked window of thread {thread}"))?
        .flatten();
    let owner: Option<Option<String>> = transaction
        .query_row(
            "SELECT turn_id FROM gates WHERE thread_id = ?1 AND gate_id = ?2 AND status = 'open'",
            params![id, gate.to_string()],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read gate {gate} of thread {thread}"))?;
    let Some(owner) = owner else {
        bail!("resolution of unknown or already resolved gate {gate} of thread {thread}");
    };
    execute(
        transaction,
        "UPDATE gates SET status = 'resolved', answer_json = ?3, resolved_by = ?4, \
         resolved_seq = ?5, resolved_at = ?6, blocked_since = NULL \
         WHERE thread_id = ?1 AND gate_id = ?2",
        params![
            id,
            gate.to_string(),
            json_text(answer)?,
            discriminant(by, "GateResolver")?,
            seq,
            at,
        ],
        "resolve a gate",
    )?;

    let still_open: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM gates WHERE thread_id = ?1 AND status = 'open'",
            params![id],
            |row| row.get(0),
        )
        .with_context(|| format!("count the open gates of thread {thread}"))?;
    let Some(since) = since else {
        return Ok(());
    };
    if still_open > 0 {
        // Answering the first gate of an overlapping run must not restart the clock for the
        // second, so whatever stays open inherits the window's start.
        execute(
            transaction,
            "UPDATE gates SET blocked_since = MIN(COALESCE(blocked_since, ?2), ?2) \
             WHERE thread_id = ?1 AND status = 'open'",
            params![id, since],
            "carry a blocked window over to the gates still open",
        )?;
        return Ok(());
    }
    // The gate names its own turn when the provider supplied one; otherwise the turn that is
    // still running owns the wait, because that is the footer the user is about to read.
    let blocked = at.saturating_sub(since).max(0);
    execute(
        transaction,
        "UPDATE turns SET gate_blocked_ms = gate_blocked_ms + ?3 \
         WHERE thread_id = ?1 AND end_seq IS NULL AND turn_id = COALESCE( \
             ?2, (SELECT running_turn_id FROM threads WHERE thread_id = ?1))",
        params![id, owner, blocked],
        "charge a blocked window to its turn",
    )
}

/// Withdraws a provider-owned gate without inventing a user answer.
pub(super) fn withdraw_gate(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    gate: &GateId,
    seq: i64,
    at: i64,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    let since: Option<i64> = transaction
        .query_row(
            "SELECT MIN(blocked_since) FROM gates WHERE thread_id = ?1 AND status = 'open'",
            params![id],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read the blocked window of thread {thread}"))?
        .flatten();
    let owner: Option<Option<String>> = transaction
        .query_row(
            "SELECT turn_id FROM gates WHERE thread_id = ?1 AND gate_id = ?2 AND status = 'open'",
            params![id, gate.to_string()],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read withdrawn gate {gate} of thread {thread}"))?;
    let Some(owner) = owner else {
        bail!("withdrawal of unknown or closed gate {gate} of thread {thread}");
    };
    execute(
        transaction,
        "UPDATE gates SET status = 'resolved', resolved_by = ?3, resolved_seq = ?4, \
         resolved_at = ?5, blocked_since = NULL WHERE thread_id = ?1 AND gate_id = ?2",
        params![
            id,
            gate.to_string(),
            discriminant(&GateResolver::ProviderClosed, "GateResolver")?,
            seq,
            at
        ],
        "withdraw a gate",
    )?;

    let still_open: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM gates WHERE thread_id = ?1 AND status = 'open'",
            params![id],
            |row| row.get(0),
        )
        .with_context(|| format!("count the open gates of thread {thread}"))?;
    let Some(since) = since else {
        return Ok(());
    };
    if still_open > 0 {
        execute(
            transaction,
            "UPDATE gates SET blocked_since = MIN(COALESCE(blocked_since, ?2), ?2) \
             WHERE thread_id = ?1 AND status = 'open'",
            params![id, since],
            "carry a blocked window over to the gates still open",
        )?;
        return Ok(());
    }
    let blocked = at.saturating_sub(since).max(0);
    execute(
        transaction,
        "UPDATE turns SET gate_blocked_ms = gate_blocked_ms + ?3 \
         WHERE thread_id = ?1 AND end_seq IS NULL AND turn_id = COALESCE( \
             ?2, (SELECT running_turn_id FROM threads WHERE thread_id = ?1))",
        params![id, owner, blocked],
        "charge a withdrawn gate window to its turn",
    )
}
