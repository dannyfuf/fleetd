//! Persistence for durable delegations and their transactional follow-up outbox.
//!
//! These are the only production statements that touch `delegations` or
//! `delegation_outbox`. Keeping them together makes the phase-3 transition path able to compose
//! one state change and one outbox action in the writer's existing transaction.
// The service halves that call these statements land later in phase 3 (`run.rs`, `complete.rs`,
// `worker.rs`) together with the transition half that runs them inside `project_event`'s
// transaction. Until they do, the statements are reachable only from the store's own tests; the
// allowance comes off with the first production caller.
#![allow(dead_code)]

use std::{fmt::Display, str::FromStr};

use anyhow::{Context, anyhow, bail};
use chrono::{DateTime, Utc};
use fleet_core::agents::{
    Delegation, DelegationId, DelegationResult, DeliveryState, Seq, ThreadId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::de::DeserializeOwned;

use super::project::discriminant;

/// Rows per delegation read. Every read in this store carries an explicit `LIMIT`; a daemon that
/// somehow held more than this many delegations is one whose ceilings already failed.
const READ_LIMIT: i64 = 1_000;

/// A durable action the delegation worker must perform after the state transaction commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboxAction {
    /// Deliver a child result to its caller.
    Deliver,
    /// Ask a settled child for a final report.
    Nudge,
    /// Finish a delegation whose child has settled.
    Settle,
    /// Recover a child whose provider exited.
    Recover,
    /// Cancel live descendants of a cancelled delegation.
    CancelChildren,
    /// Publish the changed delegation to mirrors.
    Mirror,
}

impl OutboxAction {
    /// The word stored in `delegation_outbox.action`, which is also what an operator greps for.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Deliver => "deliver",
            Self::Nudge => "nudge",
            Self::Settle => "settle",
            Self::Recover => "recover",
            Self::CancelChildren => "cancel_children",
            Self::Mirror => "mirror",
        }
    }

    /// Decodes a stored word. An action this build does not know fails the read rather than
    /// being skipped: an outbox row nobody can run is work that would be silently dropped.
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "deliver" => Ok(Self::Deliver),
            "nudge" => Ok(Self::Nudge),
            "settle" => Ok(Self::Settle),
            "recover" => Ok(Self::Recover),
            "cancel_children" => Ok(Self::CancelChildren),
            "mirror" => Ok(Self::Mirror),
            other => bail!("unknown delegation outbox action `{other}`"),
        }
    }
}

/// One unfinished delegation outbox action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutboxRow {
    /// Row identity, which is also the order the worker drains rows in.
    pub id: i64,
    /// The delegation the action belongs to.
    pub delegation: DelegationId,
    /// What the worker must do.
    pub action: OutboxAction,
    /// When the transaction that enqueued the row committed.
    pub created: DateTime<Utc>,
}

/// Inserts the immutable identity and the initial mutable state of a delegation.
pub(super) fn insert(
    tx: &Transaction<'_>,
    delegation: &Delegation,
    token_sha256: &str,
) -> anyhow::Result<()> {
    let encoded = EncodedDelegation::from_delegation(delegation)?;
    tx.execute(
        "INSERT INTO delegations (id, token_sha256, caller_thread, caller_turn, caller_item, \
         child_thread, provider, depth, brief, expectation, eager, status, status_payload, \
         result, result_source, result_files, result_elided, nudges, recoveries, delivery, \
         delivered_seq, delivered_turn, delivery_reason, headline, created, finished) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        params![
            delegation.id.to_string(),
            token_sha256,
            delegation.caller.to_string(),
            delegation.caller_turn.to_string(),
            delegation.caller_item.to_string(),
            delegation.child.to_string(),
            encoded.provider,
            i64::from(delegation.depth),
            delegation.brief,
            delegation.expectation,
            i64::from(delegation.eager),
            encoded.status,
            delegation.status_payload,
            encoded.result,
            encoded.result_source,
            encoded.result_files,
            encoded.result_elided,
            i64::from(delegation.nudges),
            i64::from(delegation.recoveries),
            encoded.delivery,
            encoded.delivered_seq,
            encoded.delivered_turn,
            encoded.delivery_reason,
            delegation.headline,
            timestamp(delegation.created),
            delegation.finished.map(timestamp),
        ],
    )
    .with_context(|| format!("insert delegation {}", delegation.id))?;
    Ok(())
}

/// Rewrites every mutable delegation column while preserving its identity and token hash.
pub(super) fn update(tx: &Transaction<'_>, delegation: &Delegation) -> anyhow::Result<()> {
    let encoded = EncodedDelegation::from_delegation(delegation)?;
    let changed = tx
        .execute(
            "UPDATE delegations SET status = ?2, status_payload = ?3, result = ?4, \
             result_source = ?5, result_files = ?6, result_elided = ?7, nudges = ?8, \
             recoveries = ?9, delivery = ?10, delivered_seq = ?11, delivered_turn = ?12, \
             delivery_reason = ?13, headline = ?14, finished = ?15 WHERE id = ?1",
            params![
                delegation.id.to_string(),
                encoded.status,
                delegation.status_payload,
                encoded.result,
                encoded.result_source,
                encoded.result_files,
                encoded.result_elided,
                i64::from(delegation.nudges),
                i64::from(delegation.recoveries),
                encoded.delivery,
                encoded.delivered_seq,
                encoded.delivered_turn,
                encoded.delivery_reason,
                delegation.headline,
                delegation.finished.map(timestamp),
            ],
        )
        .with_context(|| format!("update delegation {}", delegation.id))?;
    if changed == 0 {
        bail!("delegation {} does not exist", delegation.id);
    }
    Ok(())
}

/// Reads one delegation by id.
pub(super) fn get(conn: &Connection, id: DelegationId) -> anyhow::Result<Option<Delegation>> {
    read_one(
        conn,
        &format!("SELECT {DELEGATION_COLUMNS} FROM delegations WHERE id = ?1"),
        id.to_string(),
        format!("read delegation {id}"),
    )
}

/// Reads the delegation a child thread belongs to; `child_thread` is `UNIQUE`.
pub(super) fn get_by_child(
    conn: &Connection,
    child: ThreadId,
) -> anyhow::Result<Option<Delegation>> {
    read_one(
        conn,
        &format!("SELECT {DELEGATION_COLUMNS} FROM delegations WHERE child_thread = ?1"),
        child.to_string(),
        format!("read delegation for child {child}"),
    )
}

/// Reads the stored SHA-256 of a delegation's completion token. The plaintext is never persisted.
pub(super) fn token_hash(conn: &Connection, id: DelegationId) -> anyhow::Result<Option<String>> {
    conn.query_row(
        "SELECT token_sha256 FROM delegations WHERE id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )
    .optional()
    .with_context(|| format!("read the token hash of delegation {id}"))
}

/// Lists delegations newest first, optionally narrowed to one caller.
pub(super) fn list(conn: &Connection, caller: Option<ThreadId>) -> anyhow::Result<Vec<Delegation>> {
    read_many(conn, caller, false)
}

/// The same list narrowed to non-terminal delegations, which the ceilings are counted from.
pub(super) fn live(conn: &Connection, caller: Option<ThreadId>) -> anyhow::Result<Vec<Delegation>> {
    read_many(conn, caller, true)
}

/// Records one follow-up action for the worker to run after this transaction commits.
pub(super) fn enqueue(
    tx: &Transaction<'_>,
    delegation: DelegationId,
    action: OutboxAction,
    now: DateTime<Utc>,
) -> anyhow::Result<i64> {
    tx.execute(
        "INSERT INTO delegation_outbox (delegation, action, created) VALUES (?1, ?2, ?3)",
        params![delegation.to_string(), action.as_str(), timestamp(now)],
    )
    .with_context(|| format!("enqueue {} for delegation {delegation}", action.as_str()))?;
    Ok(tx.last_insert_rowid())
}

/// Every unfinished row in id order — the durable work list one worker pass drains.
pub(super) fn open_rows(conn: &Connection) -> anyhow::Result<Vec<OutboxRow>> {
    read_outbox(
        conn,
        "SELECT id, delegation, action, created FROM delegation_outbox \
         WHERE done IS NULL ORDER BY id ASC LIMIT ?1",
        params![READ_LIMIT],
        "read the delegation outbox",
    )
}

/// The same list narrowed to one delegation.
pub(super) fn open_rows_for(
    conn: &Connection,
    delegation: DelegationId,
) -> anyhow::Result<Vec<OutboxRow>> {
    read_outbox(
        conn,
        "SELECT id, delegation, action, created FROM delegation_outbox \
         WHERE done IS NULL AND delegation = ?1 ORDER BY id ASC LIMIT ?2",
        params![delegation.to_string(), READ_LIMIT],
        "read one delegation's outbox",
    )
}

/// Closes one outbox row. Guarded on `done IS NULL`, so a replayed pass cannot move the stamp
/// that says when the action actually finished.
pub(super) fn mark_done(tx: &Transaction<'_>, row: i64, now: DateTime<Utc>) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE delegation_outbox SET done = ?2 WHERE id = ?1 AND done IS NULL",
        params![row, timestamp(now)],
    )
    .with_context(|| format!("mark delegation outbox row {row} done"))?;
    Ok(())
}

/// Closes every open row of one action for one delegation, and answers how many it closed.
pub(super) fn mark_done_for(
    tx: &Transaction<'_>,
    delegation: DelegationId,
    action: OutboxAction,
    now: DateTime<Utc>,
) -> anyhow::Result<usize> {
    tx.execute(
        "UPDATE delegation_outbox SET done = ?3 \
         WHERE delegation = ?1 AND action = ?2 AND done IS NULL",
        params![delegation.to_string(), action.as_str(), timestamp(now)],
    )
    .with_context(|| {
        format!(
            "mark {} rows done for delegation {delegation}",
            action.as_str()
        )
    })
}

/// Whether a caller thread is still listed, which is what refuses a run against a deleted thread.
pub(super) fn caller_exists(conn: &Connection, caller: ThreadId) -> anyhow::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE thread_id = ?1 AND deleted_at IS NULL)",
        [caller.to_string()],
        |row| row.get(0),
    )
    .with_context(|| format!("check whether delegation caller {caller} exists"))
}

/// The projection every delegation read selects, in the order [`RawDelegation::read`] decodes.
///
/// One list rather than one per statement: a column added to the table without being added here
/// is a silent drop, and a column added in a different order is a silent mistranslation.
const DELEGATION_COLUMNS: &str = "id, caller_thread, caller_turn, caller_item, child_thread, \
provider, depth, brief, expectation, eager, status, status_payload, result, result_source, \
result_files, result_elided, nudges, recoveries, delivery, delivered_seq, delivered_turn, \
delivery_reason, headline, created, finished";

fn read_many(
    conn: &Connection,
    caller: Option<ThreadId>,
    live_only: bool,
) -> anyhow::Result<Vec<Delegation>> {
    let caller_clause = if caller.is_some() {
        " AND caller_thread = ?1"
    } else {
        ""
    };
    let live_clause = if live_only {
        " AND status NOT IN ('succeeded', 'incomplete', 'failed', 'cancelled')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT {DELEGATION_COLUMNS} FROM delegations WHERE 1 = 1{caller_clause}{live_clause} \
         ORDER BY created DESC LIMIT ?2"
    );
    let (caller_value, limit) = match caller {
        Some(caller) => (Some(caller.to_string()), READ_LIMIT),
        None => (None, READ_LIMIT),
    };
    let mut statement = conn
        .prepare(&sql)
        .context("prepare the delegation list query")?;
    let raw = if caller_value.is_some() {
        statement
            .query_map(params![caller_value, limit], RawDelegation::read)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        // Keep LIMIT in slot 2 in both statement shapes so the SQL above is stable.
        statement
            .query_map(params![rusqlite::types::Null, limit], RawDelegation::read)?
            .collect::<Result<Vec<_>, _>>()?
    };
    raw.into_iter().map(RawDelegation::decode).collect()
}

fn read_one(
    conn: &Connection,
    sql: &str,
    parameter: String,
    context: String,
) -> anyhow::Result<Option<Delegation>> {
    let row = conn
        .query_row(sql, [parameter], RawDelegation::read)
        .optional()
        .with_context(|| context.clone())?;
    row.map(RawDelegation::decode)
        .transpose()
        .with_context(|| context)
}

fn read_outbox<P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    parameters: P,
    context: &str,
) -> anyhow::Result<Vec<OutboxRow>> {
    let mut statement = conn
        .prepare(sql)
        .with_context(|| format!("prepare to {context}"))?;
    let rows = statement
        .query_map(parameters, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .with_context(|| format!("query to {context}"))?;
    let mut decoded = Vec::new();
    for row in rows {
        let (id, delegation, action, created) =
            row.with_context(|| format!("decode row to {context}"))?;
        decoded.push(OutboxRow {
            id,
            delegation: parse_id(&delegation, "delegation id")?,
            action: OutboxAction::parse(&action)?,
            created: parse_timestamp(&created, "outbox creation time")?,
        });
    }
    Ok(decoded)
}

struct EncodedDelegation {
    provider: String,
    status: String,
    result: Option<String>,
    result_source: Option<String>,
    result_files: Option<String>,
    result_elided: i64,
    delivery: String,
    delivered_seq: Option<i64>,
    delivered_turn: Option<String>,
    delivery_reason: Option<String>,
}

impl EncodedDelegation {
    fn from_delegation(delegation: &Delegation) -> anyhow::Result<Self> {
        let (result, result_source, result_files, result_elided) = match &delegation.result {
            Some(result) => (
                Some(result.text.clone()),
                Some(discriminant(&result.source, "ResultSource")?),
                Some(
                    serde_json::to_string(&result.files_changed)
                        .context("encode delegation result files")?,
                ),
                i64::from(result.elided),
            ),
            None => (None, None, None, 0),
        };
        let (delivered_seq, delivered_turn, delivery_reason) = match &delegation.delivery {
            DeliveryState::Pending => (None, None, None),
            DeliveryState::Delivered { seq, turn } => (
                Some(i64::try_from(seq.0).unwrap_or(i64::MAX)),
                Some(turn.to_string()),
                None,
            ),
            DeliveryState::Undeliverable { reason } => (None, None, Some(reason.clone())),
        };
        Ok(Self {
            provider: discriminant(&delegation.provider, "AgentKind")?,
            status: discriminant(&delegation.status, "DelegationStatus")?,
            result,
            result_source,
            result_files,
            result_elided,
            delivery: delegation.delivery.word().to_owned(),
            delivered_seq,
            delivered_turn,
            delivery_reason,
        })
    }
}

struct RawDelegation {
    id: String,
    caller: String,
    caller_turn: String,
    caller_item: String,
    child: String,
    provider: String,
    depth: i64,
    brief: String,
    expectation: String,
    eager: i64,
    status: String,
    status_payload: Option<String>,
    result: Option<String>,
    result_source: Option<String>,
    result_files: Option<String>,
    result_elided: i64,
    nudges: i64,
    recoveries: i64,
    delivery: String,
    delivered_seq: Option<i64>,
    delivered_turn: Option<String>,
    delivery_reason: Option<String>,
    headline: Option<String>,
    created: String,
    finished: Option<String>,
}

impl RawDelegation {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            caller: row.get(1)?,
            caller_turn: row.get(2)?,
            caller_item: row.get(3)?,
            child: row.get(4)?,
            provider: row.get(5)?,
            depth: row.get(6)?,
            brief: row.get(7)?,
            expectation: row.get(8)?,
            eager: row.get(9)?,
            status: row.get(10)?,
            status_payload: row.get(11)?,
            result: row.get(12)?,
            result_source: row.get(13)?,
            result_files: row.get(14)?,
            result_elided: row.get(15)?,
            nudges: row.get(16)?,
            recoveries: row.get(17)?,
            delivery: row.get(18)?,
            delivered_seq: row.get(19)?,
            delivered_turn: row.get(20)?,
            delivery_reason: row.get(21)?,
            headline: row.get(22)?,
            created: row.get(23)?,
            finished: row.get(24)?,
        })
    }

    fn decode(self) -> anyhow::Result<Delegation> {
        let id = parse_id(&self.id, "delegation id")?;
        let result = match self.result {
            Some(text) => {
                let source = self
                    .result_source
                    .as_deref()
                    .ok_or_else(|| anyhow!("delegation {id} has result text without a source"))?;
                let files_changed = self
                    .result_files
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .with_context(|| format!("decode result files of delegation {id}"))?
                    .unwrap_or_default();
                Some(DelegationResult {
                    text,
                    files_changed,
                    source: parse_enum(source, "result source")?,
                    elided: self.result_elided != 0,
                })
            }
            None => None,
        };
        let delivery = match self.delivery.as_str() {
            "pending" => DeliveryState::Pending,
            "delivered" => {
                let seq = self.delivered_seq.ok_or_else(|| {
                    anyhow!("delegation {id} is delivered without a delivered sequence")
                })?;
                let turn = self.delivered_turn.as_deref().ok_or_else(|| {
                    anyhow!("delegation {id} is delivered without a delivered turn")
                })?;
                DeliveryState::Delivered {
                    seq: Seq(u64::try_from(seq).with_context(|| {
                        format!("delegation {id} has a negative delivered sequence")
                    })?),
                    turn: parse_id(turn, "delivered turn")?,
                }
            }
            "undeliverable" => DeliveryState::Undeliverable {
                reason: self
                    .delivery_reason
                    .ok_or_else(|| anyhow!("delegation {id} is undeliverable without a reason"))?,
            },
            other => bail!("delegation {id} has unknown delivery state `{other}`"),
        };
        Ok(Delegation {
            id,
            caller: parse_id(&self.caller, "caller thread")?,
            caller_turn: parse_id(&self.caller_turn, "caller turn")?,
            caller_item: parse_id(&self.caller_item, "caller item")?,
            child: parse_id(&self.child, "child thread")?,
            provider: parse_enum(&self.provider, "provider")?,
            depth: u8::try_from(self.depth)
                .with_context(|| format!("delegation {id} has an invalid depth"))?,
            brief: self.brief,
            expectation: self.expectation,
            eager: self.eager != 0,
            status: parse_enum(&self.status, "status")?,
            status_payload: self.status_payload,
            result,
            nudges: u8::try_from(self.nudges)
                .with_context(|| format!("delegation {id} has invalid nudges"))?,
            recoveries: u8::try_from(self.recoveries)
                .with_context(|| format!("delegation {id} has invalid recoveries"))?,
            delivery,
            created: parse_timestamp(&self.created, "creation time")?,
            finished: self
                .finished
                .as_deref()
                .map(|value| parse_timestamp(value, "finish time"))
                .transpose()?,
            headline: self.headline,
        })
    }
}

/// Decodes a word this file wrote with [`discriminant`], which is what keeps the two directions
/// from drifting when a variant is renamed.
fn parse_enum<T: DeserializeOwned>(word: &str, what: &str) -> anyhow::Result<T> {
    serde_json::from_value(serde_json::Value::String(word.to_owned()))
        .with_context(|| format!("decode delegation {what} `{word}`"))
}

fn parse_id<T>(value: &str, what: &str) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: Display + Send + Sync + 'static,
{
    value
        .parse()
        .map_err(|error| anyhow!("{error}"))
        .with_context(|| format!("decode delegation {what} `{value}`"))
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

fn parse_timestamp(value: &str, what: &str) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .with_context(|| format!("decode delegation {what} `{value}`"))
}
