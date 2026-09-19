//! Tests for the SQL usage read, every one of them against a tempdir database.
//!
//! The load-bearing one is [`the_sql_read_equals_the_projection_footer`]: this module restates
//! arithmetic that lives in `fleet-core`'s reducer, and the only honest defence against that copy
//! drifting is to replay one log through both and compare the answers field by field.

use anyhow::Context;
use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, DelegationUsage, ItemId, PermissionMode, Seq, SeqEvent, ThreadId,
        ThreadProjection, TurnId, TurnOutcome, Usage,
    },
    ids::WorktreeId,
};

use super::super::SqliteAgentStore;

/// A store on a fresh tempdir. The directory is returned because dropping it deletes the database.
fn store() -> anyhow::Result<(tempfile::TempDir, SqliteAgentStore)> {
    let directory = tempfile::tempdir().context("create a usage test directory")?;
    let store = SqliteAgentStore::open(
        fleet_core::paths::FleetHome::new(directory.path()).agents_db_path(),
    )?;
    Ok((directory, store))
}

fn worktree() -> WorktreeId {
    WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}"))
}

/// A log under construction: the reducer refuses an out-of-order sequence, so the counter is the
/// fixture rather than a number each call has to restate.
#[derive(Default)]
struct Log {
    events: Vec<SeqEvent>,
}

impl Log {
    fn push(&mut self, event: AgentEvent) -> &mut Self {
        let seq = u64::try_from(self.events.len()).unwrap_or_default() + 1;
        self.events.push(SeqEvent {
            seq: Seq(seq),
            at: stamp(seq),
            raw: Some("test-frame".to_owned()),
            event,
        });
        self
    }

    fn configured(&mut self) -> &mut Self {
        self.push(AgentEvent::SessionConfigured {
            provider: AgentKind::Codex,
            resume_cursor: None,
            model: None,
            models: Vec::new(),
            mode: PermissionMode::Ask,
            tools: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
        })
    }

    fn started(&mut self, turn: TurnId) -> &mut Self {
        self.push(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
    }

    fn reported(
        &mut self,
        turn: TurnId,
        usage: Usage,
        context_pct: f32,
        cost_usd: Option<f64>,
    ) -> &mut Self {
        self.push(AgentEvent::TokenUsage {
            turn,
            usage,
            context_pct,
            cost_usd,
        })
    }

    fn settled(&mut self, turn: TurnId, usage: Usage) -> &mut Self {
        self.push(AgentEvent::TurnSettled {
            turn,
            outcome: TurnOutcome::Completed,
            usage,
            duration_ms: 1_000,
            files_changed: Vec::new(),
        })
    }
}

fn stamp(seq: u64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(1_700_000_000_000 + i64::try_from(seq).unwrap_or_default())
        .unwrap_or_else(Utc::now)
}

/// Distinguishable counters, so a fold that drops or double-counts a turn cannot come out right.
fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        tool_uses: 1,
        ..Usage::default()
    }
}

/// The three numbers `ThreadProjection` answers with, shaped as the store answers them.
fn footer(thread: ThreadId, events: &[SeqEvent]) -> anyhow::Result<DelegationUsage> {
    let mut replay = ThreadProjection::new(thread, worktree(), AgentKind::Codex);
    for event in events {
        replay
            .apply(event)
            .map_err(|error| anyhow::anyhow!("the fixture is not a valid log: {error}"))?;
    }
    Ok(DelegationUsage {
        usage: replay.cumulative_usage,
        cost_usd: replay.cumulative_cost_usd,
        context_pct: replay.context_pct,
    })
}

async fn append_all(
    store: &SqliteAgentStore,
    thread: ThreadId,
    events: &[SeqEvent],
) -> anyhow::Result<()> {
    for event in events {
        store.append(thread, event).await?;
    }
    Ok(())
}

/// Two settled turns and one still running, with a cost reported by a frame that is not the
/// newest — which is the case a naive "read the last payload" would get wrong.
fn fixture(first: TurnId, second: TurnId, third: TurnId) -> Vec<SeqEvent> {
    let mut log = Log::default();
    log.configured()
        .started(first)
        .reported(first, usage(10, 5), 12.0, Some(0.10))
        .settled(first, usage(12, 6))
        .started(second)
        .reported(second, usage(20, 7), 30.0, None)
        .reported(second, usage(21, 8), 34.5, Some(0.42))
        .settled(second, usage(22, 9))
        .started(third)
        .reported(third, usage(5, 1), 37.25, None);
    log.events
}

#[tokio::test]
async fn the_sql_read_equals_the_projection_footer() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let events = fixture(TurnId::new(), TurnId::new(), TurnId::new());
    append_all(&store, thread, &events).await?;

    let expected = footer(thread, &events)?;
    // The fixture is only evidence if it actually exercises the three hard cases.
    assert_eq!(
        expected.usage,
        sum(&[usage(12, 6), usage(22, 9), usage(5, 1)]),
        "two settled turns plus the live turn's latest report"
    );
    assert_eq!(
        expected.cost_usd,
        Some(0.42),
        "a frame that omits the cost reports nothing, not zero"
    );
    assert!(
        (expected.context_pct - 37.25).abs() < f32::EPSILON,
        "the latest non-zero utilisation wins"
    );

    assert_eq!(store.delegation_usage(thread).await?, Some(expected));
    Ok(())
}

/// The same equivalence once the turn the last report belongs to has settled: the reducer stops
/// adding it on top, and so must the read, or every terminal child is counted one turn too rich.
#[tokio::test]
async fn the_sql_read_equals_the_projection_footer_once_every_turn_has_settled()
-> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let third = TurnId::new();
    let mut log = Log {
        events: fixture(TurnId::new(), TurnId::new(), third),
    };
    log.settled(third, usage(6, 2));
    let events = log.events;
    append_all(&store, thread, &events).await?;

    assert_eq!(
        store.delegation_usage(thread).await?,
        Some(footer(thread, &events)?)
    );
    Ok(())
}

/// Cost is cumulative *for the process*, so re-configuring the session starts a new one and the
/// old total must not be reported against it. Context has no such boundary.
#[tokio::test]
async fn a_reconfigured_session_drops_the_cost_it_reported_before() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let (first, second) = (TurnId::new(), TurnId::new());
    let mut log = Log::default();
    log.configured()
        .started(first)
        .reported(first, usage(10, 5), 22.0, Some(0.31))
        .settled(first, usage(10, 5))
        .configured()
        .started(second)
        .reported(second, usage(3, 1), 0.0, None);
    let events = log.events;
    append_all(&store, thread, &events).await?;

    let read = store
        .delegation_usage(thread)
        .await?
        .context("the thread reported usage")?;
    assert_eq!(read.cost_usd, None, "a new session has spent nothing yet");
    assert!(
        (read.context_pct - 22.0).abs() < f32::EPSILON,
        "an unmeasured context window is not a report of no occupancy"
    );
    assert_eq!(read, footer(thread, &events)?);
    Ok(())
}

#[tokio::test]
async fn a_thread_that_never_spent_anything_has_no_usage_to_report() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let mut log = Log::default();
    log.configured();
    append_all(&store, thread, &log.events).await?;

    assert_eq!(
        store.delegation_usage(thread).await?,
        None,
        "nothing measured is not the same answer as zero measured"
    );
    assert_eq!(
        store.delegation_usage(ThreadId::new()).await?,
        None,
        "a thread this daemon has never seen is the same answer"
    );
    Ok(())
}

/// One trip to the reader pool answers every row, and a row whose child spent nothing is absent
/// rather than zero.
#[tokio::test]
async fn the_batch_read_answers_every_thread_it_is_given() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let (spender, quiet, unknown) = (ThreadId::new(), ThreadId::new(), ThreadId::new());
    let events = fixture(TurnId::new(), TurnId::new(), TurnId::new());
    append_all(&store, spender, &events).await?;
    let mut quiet_log = Log::default();
    quiet_log.configured();
    append_all(&store, quiet, &quiet_log.events).await?;

    let read = store
        .delegation_usages(vec![spender, quiet, unknown, spender])
        .await?;
    assert_eq!(read.len(), 1, "only the thread that spent anything answers");
    assert_eq!(read.get(&spender), Some(&footer(spender, &events)?));
    Ok(())
}

/// The read digs `cost_usd` and `context_pct` out of the stored payload with `json_extract`, so
/// the JSON path is part of the contract: a `rename_all` on the event would otherwise turn every
/// cost into a silent `None` with no test failing.
#[test]
fn the_stored_token_usage_payload_keeps_the_json_paths_the_read_digs_through() {
    let payload = serde_json::to_value(AgentEvent::TokenUsage {
        turn: TurnId::new(),
        usage: usage(1, 2),
        context_pct: 41.5,
        cost_usd: Some(0.17),
    })
    .expect("a token usage event serializes");
    assert_eq!(payload["type"], "token_usage");
    assert!(payload["data"]["turn"].is_string());
    assert_eq!(payload["data"]["cost_usd"], 0.17);
    assert_eq!(payload["data"]["context_pct"], 41.5);
}

/// The expectation in the equivalence test, summed independently of the `add_usage` it checks.
fn sum(parts: &[Usage]) -> Usage {
    parts.iter().fold(Usage::default(), |mut total, part| {
        total.input_tokens += part.input_tokens;
        total.output_tokens += part.output_tokens;
        total.total_tokens += part.total_tokens;
        total.tool_uses += part.tool_uses;
        total
    })
}
