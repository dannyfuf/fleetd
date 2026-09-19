//! What one thread has spent, answered from SQL instead of from a hydrated projection.
//!
//! [`fleet_core::agents::ThreadProjection`] already carries `cumulative_usage`,
//! `cumulative_cost_usd` and `context_pct`, and the GUI footer renders them — but reaching them
//! costs a `runtime(thread)` hydrate, which replays a cold thread's whole event log and then
//! clones the entire projection, items and turns included. `fleet subagent list` would pay that
//! once per row for children that are almost always terminal and therefore almost always cold,
//! which is exactly the cost [`super::list`] exists to avoid. These statements answer the same
//! three numbers from the read models the projector already wrote.
//!
//! **The definition, which must stay the reducer's definition.** `usage` is the fold over the
//! thread's **settled** turns plus, while a turn is in flight, that turn's latest reported
//! `TokenUsage` (`fleet-core` `projection/reduce.rs`). `cost_usd` is the latest *reported* cost
//! since the session was last configured — a frame that omits it reports nothing, not zero — and
//! `context_pct` is the latest non-zero utilisation. Every number is the named thread's **own**;
//! a delegated grandchild's spend is not summed in.
//!
//! `store::usage::tests::the_sql_read_equals_the_projection_footer` is what says so if the two
//! ever drift: it replays one fixture through both and compares field by field.

use anyhow::Context;
use fleet_core::agents::{AgentEvent, DelegationUsage, ThreadId, Usage};
use rusqlite::{Connection, OptionalExtension, params};

/// The settled turns of `thread`, folded, plus its in-flight turn's latest report.
///
/// `None` means the thread has spent nothing measurable at all — no settled turn carried usage
/// and no `TokenUsage` was ever observed — so a caller can render "unknown" rather than a zero it
/// would have to pretend is a measurement.
pub(super) fn thread_usage(
    conn: &Connection,
    thread: ThreadId,
) -> anyhow::Result<Option<DelegationUsage>> {
    let (mut usage, settled) = settled_usage(conn, thread)?;
    let latest = latest_token_usage(conn, thread)?;
    let Some(latest) = latest else {
        return Ok((settled > 0).then_some(DelegationUsage {
            usage,
            cost_usd: None,
            context_pct: 0.0,
        }));
    };
    // The reducer adds the newest report on top of the settled fold only while its turn is still
    // running; once the turn settles, `turns.usage_json` already holds that same number and adding
    // it again would double-count it.
    if latest.live {
        add_usage(&mut usage, &latest.usage);
    }
    let (cost_usd, context_pct) = latest_cost_and_context(conn, thread)?;
    Ok(Some(DelegationUsage {
        usage,
        cost_usd,
        context_pct,
    }))
}

/// Folds `turns.usage_json` over the turns that have ended, and counts how many carried one.
///
/// `end_seq IS NOT NULL` is the whole of "settled": `TurnCompleted` writes the turn's final usage
/// and its `end_seq` together, `TurnAborted` and the failed-turn repair set `end_seq` and leave
/// the last `TokenUsage` observation in place — which is precisely the usage the reducer freezes
/// on those turns. A *running* turn also carries `usage_json` (the projector writes it on every
/// `TokenUsage`), so the predicate is load-bearing, not decoration.
fn settled_usage(conn: &Connection, thread: ThreadId) -> anyhow::Result<(Usage, usize)> {
    let mut statement = conn
        .prepare(
            "SELECT usage_json FROM turns \
             WHERE thread_id = ?1 AND end_seq IS NOT NULL AND usage_json IS NOT NULL",
        )
        .context("prepare the settled-turn usage query")?;
    let rows = statement
        .query_map(params![thread.to_string()], |row| row.get::<_, String>(0))
        .with_context(|| format!("read the settled turn usage of thread {thread}"))?;
    let mut total = Usage::default();
    let mut counted = 0usize;
    for row in rows {
        let json = row.with_context(|| format!("read a settled turn of thread {thread}"))?;
        let usage: Usage = serde_json::from_str(&json)
            .with_context(|| format!("decode a settled turn's usage on thread {thread}"))?;
        add_usage(&mut total, &usage);
        counted += 1;
    }
    Ok((total, counted))
}

/// The newest `TokenUsage` observation, and whether the turn it belongs to is still running.
struct LatestReport {
    usage: Usage,
    live: bool,
}

fn latest_token_usage(conn: &Connection, thread: ThreadId) -> anyhow::Result<Option<LatestReport>> {
    // Served by `idx_agent_events_thread_kind_seq`, so this is a seek to the end of one thread's
    // `token_usage` events rather than a scan of its log.
    conn.query_row(
        "SELECT e.payload, EXISTS (\
             SELECT 1 FROM turns t \
             WHERE t.thread_id = e.thread_id \
               AND t.turn_id = json_extract(e.payload, '$.data.turn') \
               AND t.end_seq IS NULL\
           ) \
         FROM agent_events e \
         WHERE e.thread_id = ?1 AND e.kind = 'token_usage' \
         ORDER BY e.seq DESC LIMIT 1",
        params![thread.to_string()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
    )
    .optional()
    .with_context(|| format!("read the newest token usage of thread {thread}"))?
    .map(|(payload, live)| {
        let event: AgentEvent = serde_json::from_str(&payload)
            .with_context(|| format!("decode the newest token usage of thread {thread}"))?;
        let AgentEvent::TokenUsage { usage, .. } = event else {
            anyhow::bail!(
                "thread {thread} stored a `token_usage` event that decodes as something else"
            );
        };
        Ok(LatestReport { usage, live })
    })
    .transpose()
}

/// The latest reported cost and the latest non-zero context utilisation.
///
/// Neither is simply "the newest report's field". A frame that omits `cost_usd` is not a report of
/// zero and must not blank a cost the row already shows; an unmeasured context window is not a
/// report of no occupancy. Cost is cumulative *for the process*, so a re-configured session starts
/// a new one and observations older than the newest `session_configured` are not this session's;
/// context has no such boundary. Both mirror `projection/reduce.rs`.
fn latest_cost_and_context(
    conn: &Connection,
    thread: ThreadId,
) -> anyhow::Result<(Option<f64>, f32)> {
    let (cost, context): (Option<f64>, Option<f64>) = conn
        .query_row(
            "SELECT \
               (SELECT json_extract(payload, '$.data.cost_usd') FROM agent_events \
                 WHERE thread_id = ?1 AND kind = 'token_usage' \
                   AND seq > COALESCE((SELECT MAX(seq) FROM agent_events \
                                        WHERE thread_id = ?1 AND kind = 'session_configured'), 0) \
                   AND json_extract(payload, '$.data.cost_usd') IS NOT NULL \
                 ORDER BY seq DESC LIMIT 1), \
               (SELECT json_extract(payload, '$.data.context_pct') FROM agent_events \
                 WHERE thread_id = ?1 AND kind = 'token_usage' \
                   AND json_extract(payload, '$.data.context_pct') > 0 \
                 ORDER BY seq DESC LIMIT 1)",
            params![thread.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .with_context(|| format!("read the latest cost and context of thread {thread}"))?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`context_pct` is an f32 on the wire and in the projection; SQLite has only f64"
    )]
    Ok((cost, context.unwrap_or_default() as f32))
}

/// The reducer's own arithmetic, restated here because `projection::usage::add_usage` is private
/// to `fleet-core` and this crate must not widen it to reach one store query.
///
/// Saturating, field by field, with `extra` merged last-writer-wins — the drift risk that copy
/// creates is what the equivalence test below exists to catch.
fn add_usage(cumulative: &mut Usage, usage: &Usage) {
    cumulative.input_tokens = cumulative.input_tokens.saturating_add(usage.input_tokens);
    cumulative.output_tokens = cumulative.output_tokens.saturating_add(usage.output_tokens);
    cumulative.reasoning_tokens = cumulative
        .reasoning_tokens
        .saturating_add(usage.reasoning_tokens);
    cumulative.cache_read_tokens = cumulative
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    cumulative.cache_write_tokens = cumulative
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
    cumulative.total_tokens = cumulative.total_tokens.saturating_add(usage.total_tokens);
    cumulative.web_search_requests = cumulative
        .web_search_requests
        .saturating_add(usage.web_search_requests);
    cumulative.tool_uses = cumulative.tool_uses.saturating_add(usage.tool_uses);
    for (key, value) in &usage.extra {
        cumulative.extra.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests;
