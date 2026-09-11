//! Row codecs: the enum discriminants, JSON columns and timestamps every table shares.
//!
//! Taking a `kind`/`state` column's value from serde rather than from a hand-written match is the
//! decision here: a variant added or renamed in `fleet-core` changes the wire, the log and the
//! column together, and a hand-written match is the one of the four that would silently disagree.

use anyhow::Context;
use chrono::{DateTime, Utc};
use fleet_core::agents::SeqEvent;
use rusqlite::Transaction;
use serde::Serialize;
use serde_json::Value;

/// One event with its log columns already computed.
///
/// Serialization happens before the transaction opens, so the CPU cost of a large payload is
/// never paid with the write lock held, and the writer's byte budget is exact rather than a guess.
pub(crate) struct StagedEvent {
    /// The event itself, which the projector reads.
    pub(crate) event: SeqEvent,
    /// Its `AgentEvent` discriminant, as the `kind` column stores it.
    pub(super) kind: String,
    /// Its `serde_json` form, as the `payload` column stores it.
    pub(super) payload: String,
}

impl StagedEvent {
    /// Computes the log columns of one event.
    pub(crate) fn prepare(event: &SeqEvent) -> anyhow::Result<Self> {
        Ok(Self {
            kind: discriminant(&event.event, "AgentEvent")?,
            payload: serde_json::to_string(&event.event).with_context(|| {
                format!("encode agent event {} for the agent database", event.seq)
            })?,
            event: event.clone(),
        })
    }

    /// The serialized size this event contributes to a batch.
    pub(crate) fn bytes(&self) -> usize {
        self.payload.len()
    }
}

/// One statement, with the failure named in the caller's words.
pub(crate) fn execute(
    transaction: &Transaction<'_>,
    sql: &str,
    parameters: impl rusqlite::Params,
    what: &str,
) -> anyhow::Result<()> {
    transaction
        .execute(sql, parameters)
        .with_context(|| format!("{what} in the agent database"))?;
    Ok(())
}

/// The `session_state` column of one [`SessionState`], payload-bearing variants included.
///
/// Every unit variant stores its bare discriminant, which is what the partial indexes and the
/// boot census match on; only `Waiting` carries a payload, and it stores JSON so nothing is lost.
/// One function rather than the idiom repeated at each write site, because the list read has to
/// decode exactly what the writes produce.
pub(crate) fn session_state_column(
    state: &fleet_core::agents::SessionState,
) -> anyhow::Result<String> {
    if matches!(state, fleet_core::agents::SessionState::Waiting(_)) {
        json_text(state)
    } else {
        discriminant(state, "SessionState")
    }
}

/// The serde discriminant of an enum, which is what every `kind`/`state` column stores.
///
/// Taking it from serde rather than from a hand-written match is what keeps the columns from
/// drifting away from the wire and the log when a variant is added or renamed.
pub(crate) fn discriminant(value: &impl Serialize, what: &str) -> anyhow::Result<String> {
    match serde_json::to_value(value)
        .with_context(|| format!("encode {what} for the agent database"))?
    {
        Value::String(text) => Ok(text),
        Value::Object(map) => map
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("{what} serialized without a `type` discriminant")),
        other => Err(anyhow::anyhow!(
            "{what} serialized as {other}, which names no discriminant"
        )),
    }
}

/// Compact JSON for a column. All JSON in this database is `TEXT`, never `BLOB`, so it stays
/// greppable from `sqlite3` during an incident.
pub(crate) fn json_text(value: &impl Serialize) -> anyhow::Result<String> {
    serde_json::to_string(value).context("encode a JSON column for the agent database")
}

/// Compact JSON for a nullable column.
pub(crate) fn optional_json(value: Option<&impl Serialize>) -> anyhow::Result<Option<String>> {
    value.map(json_text).transpose()
}

/// Unix milliseconds UTC, which is how every timestamp in this database is stored.
pub(crate) const fn ms(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

/// Rebuilds one [`SeqEvent`] from the columns the log stores it in.
///
/// `at` is milliseconds because the schema stores milliseconds, so a reducer stamp is truncated to
/// the millisecond by a round trip through the log. That is the schema's decision — an integer
/// millisecond is smaller, unambiguous and sorts as a keyset — and nothing in the transcript is
/// ordered by anything finer than `seq`.
pub(crate) fn decode_event(
    seq: i64,
    at: i64,
    raw: Option<String>,
    payload: &str,
) -> anyhow::Result<SeqEvent> {
    let at = DateTime::from_timestamp_millis(at)
        .ok_or_else(|| anyhow::anyhow!("event {seq} carries an out-of-range timestamp {at}"))?;
    Ok(SeqEvent {
        seq: fleet_core::agents::Seq(u64::try_from(seq).unwrap_or_default()),
        at,
        raw,
        event: serde_json::from_str(payload)
            .with_context(|| format!("decode the payload of event {seq}"))?,
    })
}

pub(crate) fn seq_of(event: &SeqEvent) -> i64 {
    i64::try_from(event.seq.0).unwrap_or(i64::MAX)
}

/// `query_row` for a row that may legitimately not exist.
pub(crate) trait OptionalRow<T> {
    /// Turns `QueryReturnedNoRows` into `None` and leaves every other error alone.
    fn optional_row(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalRow<T> for rusqlite::Result<T> {
    fn optional_row(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
