//! Turn-level writes that settle a turn the harness never settled itself.

use anyhow::{Context, bail};
use fleet_core::agents::{ThreadId, TurnId, TurnOutcome, TurnState};
use rusqlite::{Transaction, params};

use super::{
    codec::{OptionalRow, execute, json_text},
    items::close_open_items,
};

/// Fails the running turn, if there is one, as the reducer does on an unexpected exit or a fatal
/// runtime error.
pub(super) fn fail_active_turn(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    message: &str,
    seq: i64,
    at: i64,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    let running: Option<String> = transaction
        .query_row(
            "SELECT running_turn_id FROM threads WHERE thread_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read the running turn of thread {thread}"))?
        .flatten();
    let Some(running) = running else {
        return Ok(());
    };
    let Ok(turn) = running.parse::<TurnId>() else {
        bail!("thread {thread} records an unparsable running turn `{running}`");
    };
    close_open_items(transaction, thread, turn, seq, at)?;
    let outcome_json = json_text(&TurnOutcome::Error {
        message: Some(message.to_owned()),
    })?;
    execute(
        transaction,
        "UPDATE turns SET end_seq = ?3, state = 'failed', outcome = ?4, completed_at = ?5, \
         duration_ms = MAX(0, ?5 - started_at) \
         WHERE thread_id = ?1 AND turn_id = ?2 AND end_seq IS NULL",
        params![id, running, seq, outcome_json, at],
        "fail the running turn",
    )?;
    execute(
        transaction,
        "UPDATE threads SET running_turn_id = NULL, last_outcome = ?2, turn_json = ?3 \
         WHERE thread_id = ?1",
        params![
            id,
            outcome_json,
            json_text(&TurnState::Settled(
                turn,
                TurnOutcome::Error {
                    message: Some(message.to_owned())
                }
            ))?
        ],
        "clear the failed running turn",
    )
}
