//! The two denormalized columns the thread list paints from.
//!
//! `attention` and `open_gate_count` exist so a list read never touches `items`, `turns` or
//! `gates` — that is what lets the list repaint in one frame — and they are recomputed only for
//! the events that can change them. A `ContentDelta` touches the thread row for `head_seq`,
//! `projected_seq` and `last_activity_at` and nothing else.
//!
//! The derivation mirrors `ThreadProjection::attention` with *nothing seen*, because "seen" is
//! per client and lives in the `seen` table: `Unread` is the only state defined against a client
//! cursor, and it is applied over this value at read time.

use anyhow::Context;
use fleet_core::agents::{AgentEvent, Attention, AttentionKind, GateKind, ThreadId};
use rusqlite::{Transaction, params};

use super::{
    codec::{OptionalRow as _, execute, json_text},
    items::TERMINAL_ITEM_STATUSES,
};

/// Recomputes the two denormalized columns the thread list paints from.
///
/// `attention` is stored as this daemon derives it with nothing seen, because "seen" is per client
/// and lives in the `seen` table; a client's own read cursor is applied over this value at read
/// time. Everything the derivation needs is a column or a partial-index lookup, so a list read
/// never touches `items`, `turns` or `gates`.
pub(super) fn recompute_attention(
    transaction: &Transaction<'_>,
    thread: ThreadId,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    let mut statement = transaction
        .prepare_cached("SELECT kind_json FROM gates WHERE thread_id = ?1 AND status = 'open'")
        .context("prepare the open-gate kind query")?;
    let encoded_gates = statement
        .query_map(params![id], |row| row.get::<_, String>(0))
        .context("query the open gate kinds")?
        .collect::<Result<Vec<_>, _>>()
        .context("decode the open gate kinds")?;
    drop(statement);
    let gates = encoded_gates
        .iter()
        .map(|encoded| serde_json::from_str::<GateKind>(encoded))
        .collect::<Result<Vec<_>, _>>()
        .context("decode the open gate payloads")?;

    let gate_attention = if gates
        .iter()
        .any(|gate| matches!(gate, GateKind::Permission { .. }))
    {
        Some(AttentionKind::Permission)
    } else if gates.iter().any(|gate| {
        matches!(gate, GateKind::Question { questions } if questions.iter().any(|question| question.blocking))
    }) {
        Some(AttentionKind::Question)
    } else if gates
        .iter()
        .any(|gate| matches!(gate, GateKind::Plan { .. }))
    {
        Some(AttentionKind::Plan)
    } else {
        None
    };

    let attention = if let Some(kind) = gate_attention {
        Attention::NeedsYou(kind)
    } else {
        derive_attention(transaction, &id)?
    };

    execute(
        transaction,
        "UPDATE threads SET attention = ?2, open_gate_count = ?3 WHERE thread_id = ?1",
        params![
            id,
            json_text(&attention)?,
            kinds_open_count(transaction, &id)?
        ],
        "record derived attention",
    )
}

fn kinds_open_count(transaction: &Transaction<'_>, id: &str) -> anyhow::Result<i64> {
    transaction
        .query_row(
            "SELECT COUNT(*) FROM gates WHERE thread_id = ?1 AND status = 'open'",
            params![id],
            |row| row.get(0),
        )
        .context("count the open gates of a thread")
}

/// The gate-free half of the attention table: work, then failure, then a fresh completion.
fn derive_attention(transaction: &Transaction<'_>, id: &str) -> anyhow::Result<Attention> {
    let (session_state, running, retrying, completed): (String, bool, bool, Option<i64>) =
        transaction
            .query_row(
                "SELECT session_state, running_turn_id IS NOT NULL, retrying_json IS NOT NULL, \
                 last_completed_seq FROM threads WHERE thread_id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .context("read the denormalized attention inputs of a thread")?;
    let background: i64 = transaction
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM items WHERE thread_id = ?1 AND kind = 'subagent' \
                 AND status NOT IN {TERMINAL_ITEM_STATUSES}"
            ),
            params![id],
            |row| row.get(0),
        )
        .context("count the background tasks of a thread")?;
    let latest_turn: Option<String> = transaction
        .query_row(
            "SELECT state FROM turns WHERE thread_id = ?1 ORDER BY start_seq DESC LIMIT 1",
            params![id],
            |row| row.get(0),
        )
        .optional_row()
        .context("read the latest turn state of a thread")?;

    // A live turn and a dead session are both facts about *now*, and an older completed turn
    // nobody has looked at yet may not shadow either of them.
    if session_state == "running" || running || background > 0 || retrying {
        return Ok(Attention::Working);
    }
    if session_state == "waiting" || session_state.starts_with("{\"type\":\"waiting\"") {
        return Ok(Attention::Waiting);
    }
    if session_state == "error" || latest_turn.as_deref() == Some("failed") {
        return Ok(Attention::Failed);
    }
    // A finished turn needs the user whether or not the tab was ever opened; `Unread` is the only
    // state defined against a client's seen cursor, and that is applied at read time.
    if completed.is_some_and(|completed| completed > 0) {
        return Ok(Attention::NeedsYou(AttentionKind::Finished));
    }
    Ok(Attention::Idle)
}

/// Whether this event can change `attention` or `open_gate_count`.
///
/// The short-circuit is the point: a `ContentDelta` never touches the thread row beyond
/// `head_seq`, `projected_seq` and `last_activity_at`, which is what keeps the streaming write to
/// two statements. The set extends the eight lifecycle events with the three item events, because
/// a subagent item is a background task and background tasks are part of the derivation.
pub(super) fn changes_attention(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::GateOpened { .. }
            | AgentEvent::GateResolved { .. }
            | AgentEvent::GateWithdrawn { .. }
            | AgentEvent::PlanProposed { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::TurnSettled { .. }
            | AgentEvent::TurnAborted { .. }
            | AgentEvent::SessionConfigured { .. }
            | AgentEvent::SessionStateChanged(_)
            | AgentEvent::SessionExited { .. }
            | AgentEvent::RuntimeError { .. }
            | AgentEvent::Retrying { .. }
            | AgentEvent::ItemStarted { .. }
            | AgentEvent::ItemUpdated { .. }
            | AgentEvent::ItemCompleted { .. }
    )
}

/// Clears a recorded retry, which every event except a gate, a checkpoint and a notice does.
pub(super) fn clear_retrying(
    transaction: &Transaction<'_>,
    thread: ThreadId,
) -> anyhow::Result<()> {
    execute(
        transaction,
        "UPDATE threads SET retrying_json = NULL WHERE thread_id = ?1 AND retrying_json IS NOT NULL",
        params![thread.to_string()],
        "clear a recorded provider retry",
    )
}
