//! The item projections: the transcript rows, and the two text channels concatenated in SQL.
//!
//! `items` is where a streaming turn spends almost all of its writes, so the shape of these
//! statements is the shape of the write path's cost. Two rules:
//!
//! - **Text is appended by SQLite, never by this process.** `text = text || ?` is one statement
//!   with the row's pages already in cache; read-concatenate-write is O(body²) over a turn and is
//!   exactly what makes a long assistant message get slower the longer it gets.
//! - **`items.output` is the only lossy column in the schema, and it says so in two adjacent
//!   columns.** Past [`INLINE_OUTPUT_MAX`] the row becomes a bounded head+tail window;
//!   `output_bytes` still counts the true total and `output_elided` marks the window. The log
//!   keeps every byte, and an expanded output is reassembled from it by request.

use anyhow::{Context, bail};
use fleet_core::agents::{ItemId, ItemStatus, ThreadId, TurnId};
use rusqlite::{Transaction, params};
use serde_json::Value;

use super::codec::OptionalRow as _;

/// Tool output kept inline before the row is rewritten as a head+tail window.
///
/// The log keeps every byte; this bounds only the read model. A user who expands an elided output
/// gets the rest from `agent_events` by request, which is a cold, explicit, paginated path that
/// costs nothing on the streaming one.
pub(super) const INLINE_OUTPUT_MAX: usize = 64 * 1024;

/// Characters kept at each end of an elided output window.
const OUTPUT_WINDOW: usize = 8 * 1024;

/// What replaces the elided middle. Its length is part of [`ELIDED_OUTPUT_APPEND`]'s arithmetic.
const ELISION_MARKER: &str = "\n…\n";

/// Statuses `ItemStatus::terminal()` reports as settled, as SQL sees them.
pub(super) const TERMINAL_ITEM_STATUSES: &str = "('done', 'error', 'denied')";

/// Appends to one of the two unbounded text channels in SQL rather than in this process.
///
/// The concatenation happens inside SQLite because the alternative — read the row, concatenate in
/// Rust, write it back — is O(body²) over a turn and is exactly what makes a long assistant
/// message get slower as it grows.
pub(super) fn append_text(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    item: &str,
    column: &'static str,
    delta: &str,
    at: i64,
) -> anyhow::Result<()> {
    // `column` is one of two literals chosen by the match above, never caller data.
    let statement = format!(
        "UPDATE items SET {column} = {column} || ?3, updated_at = ?4 \
         WHERE thread_id = ?1 AND item_id = ?2"
    );
    let updated = transaction
        .execute(&statement, params![thread.to_string(), item, delta, at])
        .with_context(|| format!("append streamed {column} to item {item}"))?;
    if updated == 0 {
        bail!("content delta for unknown item {item} of thread {thread}");
    }
    Ok(())
}

/// Appends tool output, rewriting the row as a bounded head+tail window once it grows past
/// [`INLINE_OUTPUT_MAX`].
///
/// `output_bytes` always counts the true total and `output_elided` says whether the column is a
/// window, so `items.output` is the only lossy column in the schema and it declares it in two
/// adjacent columns.
pub(super) fn append_output(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    item: &str,
    delta: &str,
    at: i64,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    let added = i64::try_from(delta.len()).unwrap_or(i64::MAX);
    let appended = transaction
        .execute(
            "UPDATE items SET output = output || ?3, output_bytes = output_bytes + ?4, \
             updated_at = ?5 WHERE thread_id = ?1 AND item_id = ?2 AND output_elided = 0",
            params![id, item, delta, added, at],
        )
        .with_context(|| format!("append streamed output to item {item}"))?;
    if appended == 0 {
        // Either the row is already a window, or there is no row. The second is a daemon bug.
        let rewritten = transaction
            .execute(
                ELIDED_OUTPUT_APPEND,
                params![
                    id,
                    item,
                    i64::try_from(OUTPUT_WINDOW).unwrap_or(i64::MAX),
                    ELISION_MARKER,
                    i64::try_from(OUTPUT_WINDOW + ELISION_MARKER.chars().count() + 1)
                        .unwrap_or(i64::MAX),
                    delta,
                    added,
                    at,
                ],
            )
            .with_context(|| format!("append streamed output to elided item {item}"))?;
        if rewritten == 0 {
            bail!("tool output delta for unknown item {item} of thread {thread}");
        }
        return Ok(());
    }
    transaction
        .execute(
            "UPDATE items SET output = substr(output, 1, ?3) || ?4 || substr(output, -?3), \
             output_elided = 1 \
             WHERE thread_id = ?1 AND item_id = ?2 AND output_elided = 0 AND output_bytes > ?5",
            params![
                id,
                item,
                i64::try_from(OUTPUT_WINDOW).unwrap_or(i64::MAX),
                ELISION_MARKER,
                i64::try_from(INLINE_OUTPUT_MAX).unwrap_or(i64::MAX),
            ],
        )
        .with_context(|| format!("elide the tool output of item {item}"))?;
    Ok(())
}

/// Appends to an already-elided output window, rewriting only its tail.
///
/// The row is bounded at two windows plus a marker, so this is constant work per delta however
/// large the true output grows.
const ELIDED_OUTPUT_APPEND: &str = "\
UPDATE items \
   SET output = substr(output, 1, ?3) || ?4 || substr(substr(output, ?5) || ?6, -?3), \
       output_bytes = output_bytes + ?7, \
       updated_at = ?8 \
 WHERE thread_id = ?1 AND item_id = ?2 AND output_elided = 1";

/// The `detail_json` object of one item, as a map ready to be merged into.
pub(super) fn item_detail(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    item: ItemId,
) -> anyhow::Result<serde_json::Map<String, Value>> {
    let stored: Option<Option<String>> = transaction
        .query_row(
            "SELECT detail_json FROM items WHERE thread_id = ?1 AND item_id = ?2",
            params![thread.to_string(), item.to_string()],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read the detail of item {item}"))?;
    match stored {
        None => bail!("item patch for unknown item {item} of thread {thread}"),
        Some(None) => Ok(serde_json::Map::new()),
        Some(Some(text)) => match serde_json::from_str(&text) {
            Ok(Value::Object(map)) => Ok(map),
            // A detail that is not an object is a row an older build wrote; the patch replaces it
            // rather than failing the user's turn over a field nothing reads structurally.
            _ => Ok(serde_json::Map::new()),
        },
    }
}

/// Settles every still-open item of a turn, the way the reducer does when the turn ends.
pub(super) fn close_open_items(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    turn: TurnId,
    seq: i64,
    at: i64,
) -> anyhow::Result<()> {
    let statement = format!(
        "UPDATE items SET status = CASE \
             WHEN json_extract(detail_json, '$.result') IS NOT NULL THEN 'done' \
             WHEN kind IN ('tool', 'error') THEN 'error' \
             ELSE 'done' END, \
         end_seq = ?3, updated_at = ?4 \
         WHERE thread_id = ?1 AND turn_id = ?2 AND status NOT IN {TERMINAL_ITEM_STATUSES}"
    );
    transaction
        .execute(
            &statement,
            params![thread.to_string(), turn.to_string(), seq, at],
        )
        .with_context(|| format!("close the open items of turn {turn}"))?;
    Ok(())
}

pub(super) fn is_terminal(status: ItemStatus) -> bool {
    matches!(
        status,
        ItemStatus::Done | ItemStatus::Error | ItemStatus::Denied
    )
}
