//! Tests for the SQLite agent store, every one of them against a tempdir database.
//!
//! Nothing here sleeps and nothing polls a wall clock. The two properties that need concurrency —
//! a reader taking a consistent snapshot while the writer commits, and a batch of appends landing
//! in FIFO order — are driven by a multi-threaded runtime and joined, never timed.

use anyhow::Context;
use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, GateAnswer, GateId, GateKind, GateResolver, ItemId, ItemKind,
        ItemPatch, ItemPayloadPatch, ItemStatus, PermissionChoice, PermissionMode,
        PermissionOption, ProviderOptionId, Seq, SeqEvent, StreamKind, ThreadId, ThreadProjection,
        ToolCall, ToolKind, TurnId, TurnOutcome, TurnState, Usage,
    },
    ids::WorktreeId,
};
use rusqlite::{Connection, OpenFlags, params};

use super::{AgentIndex, AgentThreadRecord, SqliteAgentStore, read};

/// A store on a fresh tempdir. The directory is returned because dropping it deletes the database.
fn store() -> anyhow::Result<(tempfile::TempDir, SqliteAgentStore)> {
    let directory = tempfile::tempdir().context("create an agent store directory")?;
    let store = SqliteAgentStore::open(
        fleet_core::paths::FleetHome::new(directory.path()).agents_db_path(),
    )?;
    Ok((directory, store))
}

/// The database file of a store, for the probes below.
pub(super) fn database(store: &SqliteAgentStore) -> std::path::PathBuf {
    store.root().join("state.sqlite")
}

/// A read-only probe on the same file, for asserting on columns no seam method returns.
pub(super) fn probe(store: &SqliteAgentStore) -> anyhow::Result<Connection> {
    let path = database(store);
    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("probe the agent database `{}`", path.display()))
}

/// A read-write handle standing in for the daemon-runtime writes a later stage adds.
///
/// The single-writer rule is about the daemon's own paths; a test that has to seed a
/// daemon-owned column has nowhere else to write it from yet.
fn runtime_writer(store: &SqliteAgentStore) -> anyhow::Result<Connection> {
    let path = database(store);
    let conn = Connection::open(&path)
        .with_context(|| format!("open the agent database `{}`", path.display()))?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .context("set the probe busy timeout")?;
    Ok(conn)
}

/// Millisecond-precision stamps, because the schema stores milliseconds: a reducer stamp is
/// truncated by a round trip through the log, and comparing whole events would otherwise compare
/// nanoseconds nothing keeps.
fn stamp(seq: u64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(1_700_000_000_000 + i64::try_from(seq).unwrap_or_default())
        .unwrap_or_else(Utc::now)
}

fn event(seq: u64, event: AgentEvent) -> SeqEvent {
    SeqEvent {
        seq: Seq(seq),
        at: stamp(seq),
        raw: Some("test-frame".to_owned()),
        event,
    }
}

fn worktree() -> WorktreeId {
    WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}"))
}

/// The shape of a real turn: a session, a prompt, streamed prose, a gated tool, and a completion.
struct Fixture {
    events: Vec<SeqEvent>,
    turn: TurnId,
    gate: GateId,
    tool: ItemId,
}

fn fixture() -> Fixture {
    let turn = TurnId::new();
    let user = ItemId::new();
    let assistant = ItemId::new();
    let tool = ItemId::new();
    let gate = GateId::new();
    let mut events = Vec::new();
    let mut next = {
        let mut seq = 0;
        move || {
            seq += 1;
            seq
        }
    };
    events.push(event(
        next(),
        AgentEvent::SessionConfigured {
            provider: AgentKind::Claude,
            resume_cursor: Some("resume-1".to_owned()),
            model: None,
            mode: PermissionMode::Ask,
            tools: vec!["Bash".to_owned()],
            commands: Vec::new(),
            skills: Vec::new(),
        },
    ));
    events.push(event(
        next(),
        AgentEvent::TurnStarted {
            turn,
            user_item: user,
        },
    ));
    events.push(event(
        next(),
        AgentEvent::ItemStarted {
            turn,
            item: user,
            kind: ItemKind::UserMessage {
                text: "list the crates".to_owned(),
                attachments: Vec::new(),
                steered: false,
            },
            parent: None,
        },
    ));
    events.push(event(
        next(),
        AgentEvent::ItemCompleted {
            item: user,
            status: ItemStatus::Completed,
        },
    ));
    events.push(event(
        next(),
        AgentEvent::ItemStarted {
            turn,
            item: assistant,
            kind: ItemKind::AssistantText {
                text: String::new(),
            },
            parent: None,
        },
    ));
    for chunk in ["Looking", " at", " the", " workspace"] {
        events.push(event(
            next(),
            AgentEvent::ContentDelta {
                item: assistant,
                stream: StreamKind::AssistantText,
                delta: chunk.to_owned(),
            },
        ));
    }
    events.push(event(
        next(),
        AgentEvent::ItemStarted {
            turn,
            item: tool,
            kind: ItemKind::Tool(Box::new(ToolCall {
                kind: ToolKind::Bash,
                name: "Bash".to_owned(),
                input: serde_json::json!({ "command": "ls crates" }),
                summary: None,
                result: None,
                output: String::new(),
                diff: None,
                exit_code: None,
                duration_ms: None,
                extra: Default::default(),
            })),
            parent: None,
        },
    ));
    events.push(event(
        next(),
        AgentEvent::GateOpened {
            gate,
            turn: Some(turn),
            kind: GateKind::Permission {
                tool: ToolKind::Bash,
                title: "Run ls".to_owned(),
                payload: "ls crates".to_owned(),
                rationale: None,
                options: vec![PermissionOption {
                    id: ProviderOptionId("allow".to_owned()),
                    label: PermissionChoice::AllowOnce,
                }],
            },
        },
    ));
    events.push(event(
        next(),
        AgentEvent::GateResolved {
            gate,
            answer: GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
            by: GateResolver::User,
        },
    ));
    events.push(event(
        next(),
        AgentEvent::ContentDelta {
            item: tool,
            stream: StreamKind::CommandOutput,
            delta: "fleet-core\nfleet-daemon\n".to_owned(),
        },
    ));
    events.push(event(
        next(),
        AgentEvent::ItemUpdated {
            item: tool,
            patch: ItemPatch {
                payload: Some(ItemPayloadPatch::Tool(Box::new(
                    fleet_core::agents::ToolPatch {
                        summary: Some("2 crates".to_owned()),
                        result: Some(serde_json::json!("ok")),
                        ..fleet_core::agents::ToolPatch::default()
                    },
                ))),
                ..ItemPatch::default()
            },
        },
    ));
    events.push(event(
        next(),
        AgentEvent::ItemCompleted {
            item: tool,
            status: ItemStatus::Completed,
        },
    ));
    events.push(event(next(), AgentEvent::Notice("using --json".to_owned())));
    events.push(event(
        next(),
        AgentEvent::TurnSettled {
            turn,
            outcome: TurnOutcome::Completed,
            usage: Usage {
                input_tokens: 12,
                output_tokens: 34,
                ..Usage::default()
            },
            duration_ms: 900,
            files_changed: Vec::new(),
        },
    ));
    Fixture {
        events,
        turn,
        gate,
        tool,
    }
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

#[tokio::test]
async fn append_then_load_round_trips_a_realistic_turn() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let fixture = fixture();

    append_all(&store, thread, &fixture.events).await?;
    let loaded = store.load(thread).await?;

    assert_eq!(loaded, fixture.events);
    Ok(())
}

#[tokio::test]
async fn a_window_is_bounded_and_never_reads_the_whole_thread() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let events = (1..=300)
        .map(|seq| event(seq, AgentEvent::Notice(format!("notice {seq}"))))
        .collect::<Vec<_>>();
    append_all(&store, thread, &events).await?;

    let newest = store.window(thread, None, 25).await?;
    assert_eq!(newest.events.len(), 25, "a page is bounded by its limit");
    assert_eq!(newest.events.first().map(|event| event.seq), Some(Seq(276)));
    assert_eq!(newest.events.last().map(|event| event.seq), Some(Seq(300)));
    assert_eq!(newest.head_seq, Seq(300));
    assert_eq!(
        newest.projected_seq, newest.head_seq,
        "a healthy thread has projected every event it logged"
    );

    // The page's own cursor walks backwards without ever widening the read.
    let older = store
        .window(
            thread,
            Some(&newest.next.expect("an older page").to_string()),
            25,
        )
        .await?;
    assert_eq!(older.events.len(), 25);
    assert_eq!(older.events.last().map(|event| event.seq), Some(Seq(275)));

    // And the planner really serves it from the keyset index: an index it stops choosing returns
    // the same rows by scanning the whole thread, which is the failure this store replaced.
    let plan = probe(&store)?
        .query_row(
            &format!("EXPLAIN QUERY PLAN {}", read::WINDOW_SQL),
            params![thread.to_string(), 300, 25],
            |row| row.get::<_, String>(3),
        )
        .context("explain the window query")?;
    assert!(
        plan.contains("USING INDEX idx_agent_events_thread_seq"),
        "the window must be served by the keyset index, not by `{plan}`"
    );
    assert!(
        !plan.contains("SCAN agent_events"),
        "the window must not scan the log: `{plan}`"
    );
    Ok(())
}

#[tokio::test]
async fn truncate_after_quarantines_rather_than_loses() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let events = (1..=5)
        .map(|seq| event(seq, AgentEvent::Notice(format!("notice {seq}"))))
        .collect::<Vec<_>>();
    append_all(&store, thread, &events).await?;

    store.truncate_after(thread, Some(Seq(3))).await?;

    let kept = store.load(thread).await?;
    assert_eq!(kept, events[..3], "the replayable prefix stays in the log");
    let probe = probe(&store)?;
    let quarantined: Vec<String> = probe
        .prepare(
            "SELECT payload FROM agent_events_quarantine WHERE thread_id = ?1 ORDER BY seq ASC",
        )?
        .query_map(params![thread.to_string()], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    assert_eq!(
        quarantined.len(),
        2,
        "the refused tail is kept, not deleted"
    );
    assert!(
        quarantined[0].contains("notice 4"),
        "the quarantined bytes are the events themselves: {quarantined:?}"
    );
    let head: i64 = probe.query_row(
        "SELECT head_seq FROM threads WHERE thread_id = ?1",
        params![thread.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(head, 3, "the head follows the log it describes");

    // The next append lands where the replay stopped rather than colliding with a quarantined row.
    store
        .append(
            thread,
            &event(4, AgentEvent::Notice("replacement".to_owned())),
        )
        .await?;
    assert_eq!(store.load(thread).await?.len(), 4);
    Ok(())
}

#[tokio::test]
async fn the_index_round_trips_and_hides_rather_than_erases() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let first = record(ThreadId::new(), "first", 10);
    let second = record(ThreadId::new(), "second", 20);
    let index = AgentIndex {
        version: super::AGENT_INDEX_VERSION,
        threads: vec![first.clone(), second.clone()],
    };

    store.write_index(&index).await?;

    assert_eq!(store.read_index().await?, index);

    // A rewrite that omits a thread hides it, exactly as the file store's rewrite did; its log
    // stays where it is, and naming it again brings it back.
    store
        .write_index(&AgentIndex {
            version: super::AGENT_INDEX_VERSION,
            threads: vec![first.clone()],
        })
        .await?;
    assert_eq!(
        store.read_index().await?.threads,
        vec![first.clone()],
        "an omitted thread is hidden"
    );
    store.write_index(&index).await?;
    assert_eq!(store.read_index().await?.threads, vec![first, second]);
    Ok(())
}

#[tokio::test]
async fn an_index_version_this_build_does_not_know_is_refused() -> anyhow::Result<()> {
    let (_directory, store) = store()?;

    let refused = store
        .write_index(&AgentIndex {
            version: super::AGENT_INDEX_VERSION + 1,
            threads: Vec::new(),
        })
        .await;

    assert!(
        refused.is_err(),
        "an index another build wrote is not ours to rewrite: {refused:?}"
    );
    Ok(())
}

fn record(thread: ThreadId, title: &str, created_ms: u64) -> AgentThreadRecord {
    AgentThreadRecord {
        thread,
        worktree: worktree(),
        provider: AgentKind::Claude,
        title: title.to_owned(),
        created: stamp(created_ms),
        last_activity: stamp(created_ms + 5),
        resume_cursor: Some(format!("resume-{title}")),
        model: None,
        mode: PermissionMode::Ask,
        last_outcome: Some(TurnOutcome::Completed),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_sees_a_consistent_snapshot_while_the_writer_commits() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let events = (1..=200)
        .map(|seq| event(seq, AgentEvent::Notice(format!("notice {seq}"))))
        .collect::<Vec<_>>();

    let writing = {
        let store = store.clone();
        let events = events.clone();
        tokio::spawn(async move { append_all(&store, thread, &events).await })
    };

    // Every one of these races the writer. Under WAL each is a snapshot, so each must be a dense
    // prefix of the log and each must agree with the head it was read with — a page ahead of its
    // watermark is the bug that makes a client resume past events it never received.
    for _ in 0..40 {
        let replayed = store.load(thread).await?;
        for (position, event) in replayed.iter().enumerate() {
            assert_eq!(
                event.seq,
                Seq(u64::try_from(position).unwrap_or_default() + 1),
                "a snapshot is dense from 1"
            );
        }
        let window = store.window(thread, None, 25).await?;
        if let Some(newest) = window.events.last() {
            assert!(
                newest.seq <= window.head_seq,
                "a page must never be ahead of the head it was read with"
            );
            assert_eq!(
                window.projected_seq, window.head_seq,
                "the projection commits with the append, so it is never behind"
            );
        }
    }
    writing.await.context("join the writer task")??;

    assert_eq!(store.load(thread).await?, events);
    Ok(())
}

#[tokio::test]
async fn the_projected_counters_match_a_full_replay_of_the_log() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let fixture = fixture();
    // Two extra events, so the assertions are made against an *open* gate and a *running* turn as
    // well as against the settled ones the fixture ends on.
    let mut events = fixture.events.clone();
    let second_turn = TurnId::new();
    let second_gate = GateId::new();
    events.push(event(
        u64::try_from(events.len()).unwrap_or_default() + 1,
        AgentEvent::TurnStarted {
            turn: second_turn,
            user_item: ItemId::new(),
        },
    ));
    events.push(event(
        u64::try_from(events.len()).unwrap_or_default() + 1,
        AgentEvent::GateOpened {
            gate: second_gate,
            turn: Some(second_turn),
            kind: GateKind::Plan {
                markdown: "# plan".to_owned(),
                steps: vec!["step".to_owned()],
            },
        },
    ));

    append_all(&store, thread, &events).await?;

    let mut replay = ThreadProjection::new(thread, worktree(), AgentKind::Claude);
    for event in &events {
        replay
            .apply(event)
            .map_err(|error| anyhow::anyhow!("the fixture is not a valid log: {error}"))?;
    }

    let probe = probe(&store)?;
    let (head, projected, gates, running, completed): (i64, i64, i64, Option<String>, Option<i64>) =
        probe.query_row(
            "SELECT head_seq, projected_seq, open_gate_count, running_turn_id, \
             last_completed_seq FROM threads WHERE thread_id = ?1",
            params![thread.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
    assert_eq!(u64::try_from(head).unwrap_or_default(), replay.last_seq.0);
    assert_eq!(
        head, projected,
        "head_seq == projected_seq is the healthy state"
    );
    assert_eq!(
        usize::try_from(gates).unwrap_or_default(),
        replay.gates.len(),
        "open_gate_count is the reducer's open gate set"
    );
    let expected_running = match replay.turn {
        TurnState::Running(turn) => Some(turn.to_string()),
        _ => None,
    };
    assert_eq!(running, expected_running);
    assert_eq!(
        completed.map(|seq| u64::try_from(seq).unwrap_or_default()),
        replay.last_completed_seq.map(|seq| seq.0)
    );

    // The item row the transcript paints from is the reducer's item, concatenated in SQL.
    let (text, output, status): (String, String, String) = probe.query_row(
        "SELECT text, output, status FROM items WHERE thread_id = ?1 AND item_id = ?2",
        params![thread.to_string(), fixture.tool.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(text, "", "a tool's prose channel stays empty");
    assert_eq!(output, "fleet-core\nfleet-daemon\n");
    assert_eq!(status, "completed");
    let resolved: String = probe.query_row(
        "SELECT status FROM gates WHERE thread_id = ?1 AND gate_id = ?2",
        params![thread.to_string(), fixture.gate.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(resolved, "resolved");
    let blocked: i64 = probe.query_row(
        "SELECT gate_blocked_ms FROM turns WHERE thread_id = ?1 AND turn_id = ?2",
        params![thread.to_string(), fixture.turn.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(
        blocked,
        replay
            .turns
            .iter()
            .find(|record| record.id == fixture.turn)
            .map_or(0, |record| i64::try_from(record.blocked_ms).unwrap_or(0)),
        "the wait charged to the turn is the reducer's blocked window"
    );
    Ok(())
}

/// The one-`SELECT` list row must be the reducer's summary, field for field.
///
/// This is the assertion the whole thread-list design rests on: `AgentThreadList` no longer maps
/// over replayed projections, so if a denormalized column drifts from `ThreadProjection` the list
/// lies and nothing else notices. `attention` and `turn` are the two that would drift silently —
/// one is derived from four columns, the other is stored as the reducer's enum precisely so it
/// cannot be re-derived wrongly.
#[tokio::test]
async fn the_list_row_is_the_reducers_summary_field_for_field() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let fixture = fixture();
    // The title is deliberately not the provider's display name: the reducer renames a
    // default-titled thread after its first user message and the store records only the title a
    // record hands it, so a default title would compare the manager's derivation against nothing.
    let record = record(thread, "ship the docs", 0);
    store.write_record(&record).await?;

    // Ends on an open gate and a running turn, so the non-idle half of the attention table and
    // every `TurnState` the fixture can reach are covered rather than just the settled ones.
    let mut events = fixture.events.clone();
    let second_turn = TurnId::new();
    events.push(event(
        u64::try_from(events.len()).unwrap_or_default() + 1,
        AgentEvent::TurnStarted {
            turn: second_turn,
            user_item: ItemId::new(),
        },
    ));
    events.push(event(
        u64::try_from(events.len()).unwrap_or_default() + 1,
        AgentEvent::GateOpened {
            gate: GateId::new(),
            turn: Some(second_turn),
            kind: GateKind::Plan {
                markdown: "# plan".to_owned(),
                steps: vec!["step".to_owned()],
            },
        },
    ));
    append_all(&store, thread, &events).await?;

    let mut replay = ThreadProjection::new(thread, record.worktree.clone(), record.provider);
    replay.title.clone_from(&record.title);
    replay.model.clone_from(&record.model);
    replay.mode = record.mode;
    for event in &events {
        replay
            .apply(event)
            .map_err(|error| anyhow::anyhow!("the fixture is not a valid log: {error}"))?;
    }

    assert_eq!(
        store.summaries().await?,
        vec![replay.summary(Seq::default())]
    );
    Ok(())
}

/// A settled turn keeps its identity in the list row, which `running_turn_id` alone cannot.
#[tokio::test]
async fn a_settled_turn_keeps_its_state_and_identity_in_the_list_row() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let fixture = fixture();
    let thread = ThreadId::new();
    let record = record(thread, "ship the docs", 0);
    store.write_record(&record).await?;
    append_all(&store, thread, &fixture.events).await?;

    let mut replay = ThreadProjection::new(thread, record.worktree.clone(), record.provider);
    replay.title.clone_from(&record.title);
    replay.mode = record.mode;
    for event in &fixture.events {
        replay
            .apply(event)
            .map_err(|error| anyhow::anyhow!("the fixture is not a valid log: {error}"))?;
    }

    let listed = store.summaries().await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].turn, replay.turn);
    assert!(
        matches!(listed[0].turn, TurnState::Settled(turn, _) if turn == fixture.turn),
        "a settled turn lost its identity: {:?}",
        listed[0].turn
    );
    Ok(())
}

#[tokio::test]
async fn a_rebuild_reproduces_every_projection_row() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let fixture = fixture();
    append_all(&store, thread, &fixture.events).await?;
    let before = dump(&probe(&store)?, thread)?;

    store.rebuild(thread).await?;

    assert_eq!(
        dump(&probe(&store)?, thread)?,
        before,
        "a replay over cleared rows reproduces the projection exactly"
    );
    Ok(())
}

#[tokio::test]
async fn a_rebuild_preserves_the_daemon_owned_session_columns() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let fixture = fixture();
    append_all(&store, thread, &fixture.events).await?;
    runtime_writer(&store)?.execute(
        "UPDATE sessions SET started_at = 111, last_seen_at = 222, restart_count = 3 \
         WHERE thread_id = ?1",
        params![thread.to_string()],
    )?;

    store.rebuild(thread).await?;

    let runtime: (Option<i64>, Option<i64>, i64) = probe(&store)?.query_row(
        "SELECT started_at, last_seen_at, restart_count FROM sessions WHERE thread_id = ?1",
        params![thread.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(
        runtime,
        (Some(111), Some(222), 3),
        "a rebuild must not clear the bookkeeping of a live provider process"
    );
    Ok(())
}

#[tokio::test]
async fn streamed_tool_output_is_elided_while_the_log_keeps_every_byte() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let tool = ItemId::new();
    let mut events = vec![
        event(
            1,
            AgentEvent::TurnStarted {
                turn,
                user_item: ItemId::new(),
            },
        ),
        event(
            2,
            AgentEvent::ItemStarted {
                turn,
                item: tool,
                kind: ItemKind::Tool(Box::new(ToolCall {
                    kind: ToolKind::Bash,
                    name: "Bash".to_owned(),
                    input: serde_json::Value::Null,
                    summary: None,
                    result: None,
                    output: String::new(),
                    diff: None,
                    exit_code: None,
                    duration_ms: None,
                    extra: Default::default(),
                })),
                parent: None,
            },
        ),
    ];
    // Twelve 8 KiB chunks, so the row crosses the 64 KiB inline ceiling and then keeps growing.
    let mut streamed = 0_i64;
    for chunk in 0..12_u64 {
        let delta = char::from_digit(u32::try_from(chunk % 10).unwrap_or_default(), 10)
            .unwrap_or('x')
            .to_string()
            .repeat(8 * 1024);
        streamed += i64::try_from(delta.len()).unwrap_or_default();
        events.push(event(
            3 + chunk,
            AgentEvent::ContentDelta {
                item: tool,
                stream: StreamKind::CommandOutput,
                delta,
            },
        ));
    }
    append_all(&store, thread, &events).await?;

    let (output, bytes, elided): (String, i64, i64) = probe(&store)?.query_row(
        "SELECT output, output_bytes, output_elided FROM items \
         WHERE thread_id = ?1 AND item_id = ?2",
        params![thread.to_string(), tool.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(elided, 1, "past the ceiling the row is a window");
    assert_eq!(bytes, streamed, "output_bytes counts the true total");
    assert!(
        output.chars().count() <= 2 * 8 * 1024 + 8,
        "the window is bounded whatever the true size: {} chars",
        output.chars().count()
    );
    assert!(output.starts_with('0'), "the head is the first bytes");
    assert!(output.ends_with('1'), "the tail is the last bytes");
    assert_eq!(
        store.load(thread).await?,
        events,
        "the log keeps every byte the row elided"
    );
    Ok(())
}

/// Every projection row of one thread, as comparable text.
pub(super) fn dump(conn: &Connection, thread: ThreadId) -> anyhow::Result<Vec<String>> {
    let mut rows = Vec::new();
    for (table, order) in [
        ("turns", "start_seq"),
        ("items", "start_seq, item_id"),
        ("gates", "opened_seq"),
        ("checkpoints", "ordinal"),
        ("threads", "thread_id"),
    ] {
        let sql = format!("SELECT * FROM {table} WHERE thread_id = ?1 ORDER BY {order} LIMIT 1000");
        let mut statement = conn
            .prepare(&sql)
            .with_context(|| format!("prepare the {table} dump"))?;
        let columns = statement.column_count();
        let dumped = statement
            .query_map(params![thread.to_string()], |row| {
                let mut text = String::from(table);
                for column in 0..columns {
                    text.push('|');
                    text.push_str(&format!("{:?}", row.get_ref(column)?));
                }
                Ok(text)
            })
            .with_context(|| format!("query the {table} dump"))?
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("read the {table} dump"))?;
        rows.extend(dumped);
    }
    Ok(rows)
}

/// A store root that already holds a database is opened, not recreated.
#[tokio::test]
async fn reopening_a_store_keeps_its_log() -> anyhow::Result<()> {
    let directory = tempfile::tempdir().context("create an agent store directory")?;
    let home = fleet_core::paths::FleetHome::new(directory.path());
    let thread = ThreadId::new();
    let events = fixture().events;
    {
        let store = SqliteAgentStore::open(home.agents_db_path())?;
        append_all(&store, thread, &events).await?;
    }

    let reopened = SqliteAgentStore::open(home.agents_db_path())?;

    assert_eq!(reopened.load(thread).await?, events);
    assert_eq!(reopened.root(), home.agents_path().as_path());
    Ok(())
}
