//! Tests for the SQLite agent store, every one of them against a tempdir database.
//!
//! Nothing here sleeps and nothing polls a wall clock. The two properties that need concurrency —
//! a reader taking a consistent snapshot while the writer commits, and a batch of appends landing
//! in FIFO order — are driven by a multi-threaded runtime and joined, never timed.

use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{
        AbortReason, AgentEvent, AgentKind, Delegation, DelegationId, DelegationResult,
        DelegationStatus, DeliveryState, GateAnswer, GateId, GateKind, GateResolver, ItemId,
        ItemKind, ItemPatch, ItemPayloadPatch, ItemStatus, MessageOrigin, ModelDescriptor,
        ModelSelection, PermissionChoice, PermissionMode, PermissionOption, ProviderOptionId,
        ReasoningEffortDescriptor, ResultSource, Seq, SeqEvent, SessionState, StopCause,
        StreamKind, ThreadId, ThreadProjection, ToolCall, ToolKind, TurnId, TurnOutcome, TurnState,
        Usage,
    },
    ids::WorktreeId,
};
use rusqlite::{Connection, OpenFlags, params};

use super::{
    AgentIndex, AgentThreadRecord, DelegationHooks, OutboxAction, SqliteAgentStore, delegations,
    read,
};
use crate::services::agents::delegation::transition::DelegationFacts;

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

#[tokio::test]
async fn seen_cursors_are_monotonic_and_isolated_per_client() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();

    store
        .mark_seen("client-a".to_owned(), thread, Seq(8), 100)
        .await?;
    store
        .mark_seen("client-a".to_owned(), thread, Seq(3), 200)
        .await?;
    store
        .mark_seen("client-b".to_owned(), thread, Seq(5), 300)
        .await?;

    assert_eq!(
        store.seen_seq("client-a".to_owned(), thread).await?,
        Some(Seq(8)),
        "an older mark never moves a cursor backwards"
    );
    assert_eq!(
        store.seen_seq("client-b".to_owned(), thread).await?,
        Some(Seq(5)),
        "another installation owns an independent cursor"
    );
    assert_eq!(
        store.seen_cursors("client-a".to_owned()).await?,
        vec![fleet_proto::agents::AgentSeenCursor {
            thread,
            seq: Seq(8),
        }]
    );
    Ok(())
}

#[tokio::test]
async fn a_clean_stop_advances_only_cursors_that_were_already_caught_up() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    store.write_record(&record(thread, "seen-stop", 1)).await?;
    store
        .append(thread, &event(1, AgentEvent::Notice("visible".to_owned())))
        .await?;
    store
        .mark_seen("caught-up".to_owned(), thread, Seq(1), 100)
        .await?;
    store
        .mark_seen("behind".to_owned(), thread, Seq(0), 100)
        .await?;

    store
        .append(
            thread,
            &event(
                2,
                AgentEvent::SessionStateChanged(fleet_core::agents::SessionState::Stopped),
            ),
        )
        .await?;

    assert_eq!(
        store.seen_seq("caught-up".to_owned(), thread).await?,
        Some(Seq(2)),
        "clean shutdown bookkeeping is not unread transcript output"
    );
    assert_eq!(
        store.seen_seq("behind".to_owned(), thread).await?,
        Some(Seq(0)),
        "a clean stop cannot hide output an installation had not read"
    );
    Ok(())
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
            models: Vec::new(),
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
                origin: Default::default(),
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
                item: None,
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
async fn session_descriptors_and_skill_refresh_round_trip_through_the_projector()
-> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let thread = ThreadId::new();
    let descriptor = ModelDescriptor {
        id: "gpt-5.6-sol".to_owned(),
        display_name: "GPT-5.6 Sol".to_owned(),
        efforts: vec![ReasoningEffortDescriptor {
            id: "high".to_owned(),
            description: "Deep reasoning".to_owned(),
        }],
        default_effort: Some("high".to_owned()),
    };
    store
        .append(
            thread,
            &event(
                1,
                AgentEvent::SessionConfigured {
                    provider: AgentKind::Codex,
                    resume_cursor: Some("codex-session".to_owned()),
                    model: Some(ModelSelection {
                        model: descriptor.id.clone(),
                        effort: descriptor.default_effort.clone(),
                        provider: None,
                    }),
                    models: vec![descriptor.clone()],
                    mode: PermissionMode::Ask,
                    tools: Vec::new(),
                    commands: Vec::new(),
                    skills: vec!["review".to_owned()],
                },
            ),
        )
        .await?;
    store
        .append(
            thread,
            &event(
                2,
                AgentEvent::MetadataChanged {
                    title: None,
                    mode: None,
                    model: None,
                    skills: Some(vec!["review".to_owned(), "ship".to_owned()]),
                },
            ),
        )
        .await?;

    let runtime = store
        .session_runtime(thread)
        .await?
        .ok_or_else(|| anyhow::anyhow!("session runtime was not projected"))?;
    assert_eq!(runtime.models, vec![descriptor]);
    assert_eq!(runtime.skills, ["review", "ship"]);
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
        parent: None,
        delegation: None,
        worktree: worktree(),
        provider: AgentKind::Claude,
        title: title.to_owned(),
        created: stamp(created_ms),
        last_activity: stamp(created_ms + 5),
        resume_cursor: Some(format!("resume-{title}")),
        model: None,
        mode: PermissionMode::Ask,
        last_outcome: Some(TurnOutcome::Completed),
        stop_cause: None,
    }
}

fn delegation(child: ThreadId) -> Delegation {
    Delegation {
        id: DelegationId::new(),
        caller: ThreadId::new(),
        caller_turn: TurnId::new(),
        caller_item: ItemId::new(),
        child,
        provider: AgentKind::Codex,
        depth: 3,
        brief: "inspect the storage boundary".to_owned(),
        expectation: "report every changed file".to_owned(),
        eager: true,
        status: DelegationStatus::Blocked,
        status_payload: Some("waiting for a decision".to_owned()),
        result: Some(DelegationResult {
            text: "the first report".to_owned(),
            files_changed: vec!["src/store.rs".to_owned(), "src/tests.rs".to_owned()],
            source: ResultSource::LastAssistantText,
            elided: true,
        }),
        nudges: 2,
        recoveries: 1,
        delivery: DeliveryState::Delivered {
            seq: Seq(42),
            turn: TurnId::new(),
        },
        created: stamp(70),
        finished: Some(stamp(71)),
        headline: Some("checking transactions".to_owned()),
    }
}

fn live_delegation(child: ThreadId, caller: ThreadId, status: DelegationStatus) -> Delegation {
    Delegation {
        id: DelegationId::new(),
        caller,
        caller_turn: TurnId::new(),
        caller_item: ItemId::new(),
        child,
        provider: AgentKind::Codex,
        depth: 1,
        brief: "exercise the transactional transition".to_owned(),
        expectation: "the row and outbox commit together".to_owned(),
        eager: false,
        status,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: stamp(1),
        finished: None,
        headline: None,
    }
}

type ChangedDelegations = Arc<Mutex<Vec<Delegation>>>;

fn capture_delegation_hooks(
    store: &SqliteAgentStore,
) -> (tokio::sync::mpsc::UnboundedReceiver<()>, ChangedDelegations) {
    let (wake, wakes) = tokio::sync::mpsc::unbounded_channel();
    let changed = ChangedDelegations::default();
    let captured = Arc::clone(&changed);
    store.install_delegation_hooks(DelegationHooks {
        wake,
        changed: Arc::new(move |delegation| {
            captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(delegation);
        }),
    });
    (wakes, changed)
}

async fn insert_test_delegation(
    store: &SqliteAgentStore,
    delegation: &Delegation,
) -> anyhow::Result<()> {
    let delegation = delegation.clone();
    store
        .delegation_write("insert transition test delegation", move |tx| {
            delegations::insert(tx, &delegation, "transition-test-token")?;
            Ok(((), false))
        })
        .await
}

struct ChildTransitionObservation {
    stored: Delegation,
    actions: Vec<OutboxAction>,
    woke: bool,
    changed: Vec<Delegation>,
}

async fn apply_child_transition(
    current: Delegation,
    agent_event: AgentEvent,
    facts: DelegationFacts,
) -> anyhow::Result<ChildTransitionObservation> {
    let (_directory, store) = store()?;
    let child = current.child;
    let id = current.id;
    // Keep the caller visible too: caller-side matching must still be scoped to the event thread.
    store
        .write_record(&record(current.caller, "transition caller", 1))
        .await?;
    let sequence = match &agent_event {
        AgentEvent::GateResolved { gate, .. } | AgentEvent::GateWithdrawn { gate } => {
            store
                .append(
                    child,
                    &event(
                        1,
                        AgentEvent::GateOpened {
                            gate: *gate,
                            turn: None,
                            kind: GateKind::Plan {
                                markdown: "seed gate".to_owned(),
                                steps: Vec::new(),
                            },
                        },
                    ),
                )
                .await?;
            2
        }
        _ => 1,
    };
    insert_test_delegation(&store, &current).await?;
    let (mut wakes, changed) = capture_delegation_hooks(&store);
    store
        .append_with_facts(child, &event(sequence, agent_event), facts)
        .await?;

    let stored = store
        .delegation(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("transition removed delegation {id}"))?;
    let actions = store
        .delegation_outbox()
        .await?
        .into_iter()
        .map(|row| row.action)
        .collect();
    let woke = wakes.try_recv().is_ok();
    assert!(
        wakes.try_recv().is_err(),
        "one event may wake the worker at most once"
    );
    let changed = changed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Ok(ChildTransitionObservation {
        stored,
        actions,
        woke,
        changed,
    })
}

fn configured_event() -> AgentEvent {
    AgentEvent::SessionConfigured {
        provider: AgentKind::Codex,
        resume_cursor: None,
        model: None,
        models: Vec::new(),
        mode: PermissionMode::FullAccess,
        tools: Vec::new(),
        commands: Vec::new(),
        skills: Vec::new(),
    }
}

fn settled_event(outcome: TurnOutcome) -> AgentEvent {
    AgentEvent::TurnSettled {
        turn: TurnId::new(),
        outcome,
        usage: Usage::default(),
        duration_ms: 1,
        files_changed: Vec::new(),
    }
}

fn assert_child_observation(
    observation: &ChildTransitionObservation,
    status: DelegationStatus,
    actions: &[OutboxAction],
    changed: bool,
) {
    assert_eq!(observation.stored.status, status);
    assert_eq!(observation.actions, actions);
    assert_eq!(observation.woke, !actions.is_empty());
    if changed {
        assert_eq!(observation.changed, vec![observation.stored.clone()]);
    } else {
        assert!(observation.changed.is_empty());
    }
}

fn result_facts(background_live: bool) -> DelegationFacts {
    DelegationFacts {
        background_live,
        last_assistant_text: Some("Finished the store transition.\nMore detail.".to_owned()),
        files_changed: vec!["src/store.rs".to_owned()],
        ..DelegationFacts::default()
    }
}

#[tokio::test]
async fn child_lifecycle_transitions_commit_row_outbox_wake_and_changed_hook() -> anyhow::Result<()>
{
    let child = ThreadId::new();
    let caller = ThreadId::new();

    let observation = apply_child_transition(
        live_delegation(child, caller, DelegationStatus::Starting),
        configured_event(),
        DelegationFacts::default(),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Running,
        &[OutboxAction::Mirror],
        true,
    );

    let observation = apply_child_transition(
        live_delegation(child, caller, DelegationStatus::Running),
        AgentEvent::GateOpened {
            gate: GateId::new(),
            turn: None,
            kind: GateKind::Plan {
                markdown: "approve".to_owned(),
                steps: Vec::new(),
            },
        },
        DelegationFacts::default(),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Blocked,
        &[OutboxAction::Mirror],
        true,
    );

    for event in [
        AgentEvent::GateResolved {
            gate: GateId::new(),
            answer: GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
            by: GateResolver::User,
        },
        AgentEvent::GateWithdrawn {
            gate: GateId::new(),
        },
    ] {
        let observation = apply_child_transition(
            live_delegation(child, caller, DelegationStatus::Blocked),
            event,
            DelegationFacts::default(),
        )
        .await?;
        assert_child_observation(
            &observation,
            DelegationStatus::Running,
            &[OutboxAction::Mirror],
            true,
        );
    }

    let mut reported = live_delegation(child, caller, DelegationStatus::Running);
    reported.result = Some(DelegationResult {
        text: "reported".to_owned(),
        files_changed: Vec::new(),
        source: ResultSource::Reported,
        elided: false,
    });
    let observation = apply_child_transition(
        reported.clone(),
        settled_event(TurnOutcome::Completed),
        result_facts(false),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Succeeded,
        &[OutboxAction::Deliver],
        true,
    );
    assert_eq!(observation.stored.result, reported.result);
    assert_eq!(observation.stored.finished, Some(stamp(1)));

    let observation = apply_child_transition(
        reported,
        settled_event(TurnOutcome::Completed),
        result_facts(true),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Settling,
        &[OutboxAction::Settle],
        true,
    );

    let observation = apply_child_transition(
        live_delegation(child, caller, DelegationStatus::Running),
        settled_event(TurnOutcome::Completed),
        result_facts(false),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Settling,
        &[OutboxAction::Nudge],
        true,
    );
    assert!(matches!(
        observation.stored.result,
        Some(DelegationResult {
            source: ResultSource::LastAssistantText,
            ..
        })
    ));

    let mut exhausted = live_delegation(child, caller, DelegationStatus::Running);
    exhausted.nudges = 2;
    let observation = apply_child_transition(
        exhausted,
        settled_event(TurnOutcome::Completed),
        result_facts(false),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Incomplete,
        &[OutboxAction::Deliver],
        true,
    );

    let mut reported_blocked = live_delegation(child, caller, DelegationStatus::Blocked);
    reported_blocked.status_payload = Some("reported blocked".to_owned());
    let observation = apply_child_transition(
        reported_blocked,
        settled_event(TurnOutcome::Completed),
        result_facts(false),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Failed,
        &[OutboxAction::Deliver],
        true,
    );
    assert_eq!(
        observation.stored.status_payload.as_deref(),
        Some("reported blocked")
    );
    Ok(())
}

#[tokio::test]
async fn child_terminal_transitions_commit_every_follow_up_action() -> anyhow::Result<()> {
    let child = ThreadId::new();
    let caller = ThreadId::new();
    for (outcome, payload) in [
        (
            TurnOutcome::Error {
                message: Some("boom".to_owned()),
            },
            "error: boom",
        ),
        (TurnOutcome::MaxTurns, "max turns"),
        (TurnOutcome::BudgetExhausted, "budget exhausted"),
        (TurnOutcome::Denied, "denied"),
        (
            TurnOutcome::Other {
                reason: "lost".to_owned(),
            },
            "other: lost",
        ),
    ] {
        let observation = apply_child_transition(
            live_delegation(child, caller, DelegationStatus::Running),
            settled_event(outcome),
            result_facts(false),
        )
        .await?;
        assert_child_observation(
            &observation,
            DelegationStatus::Failed,
            &[OutboxAction::Deliver],
            true,
        );
        assert_eq!(observation.stored.status_payload.as_deref(), Some(payload));
    }

    let observation = apply_child_transition(
        live_delegation(child, caller, DelegationStatus::Running),
        settled_event(TurnOutcome::Interrupted),
        result_facts(false),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Cancelled,
        &[OutboxAction::Deliver, OutboxAction::CancelChildren],
        true,
    );

    for reason in [
        AbortReason::User,
        AbortReason::SessionStopped,
        AbortReason::Timeout,
        AbortReason::Superseded,
        AbortReason::Other("adapter stopped".to_owned()),
    ] {
        let observation = apply_child_transition(
            live_delegation(child, caller, DelegationStatus::Running),
            AgentEvent::TurnAborted {
                turn: TurnId::new(),
                reason,
            },
            DelegationFacts::default(),
        )
        .await?;
        assert_child_observation(
            &observation,
            DelegationStatus::Cancelled,
            &[OutboxAction::Deliver, OutboxAction::CancelChildren],
            true,
        );
    }

    let observation = apply_child_transition(
        live_delegation(child, caller, DelegationStatus::Running),
        AgentEvent::TurnAborted {
            turn: TurnId::new(),
            reason: AbortReason::ProviderExited,
        },
        DelegationFacts::default(),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Running,
        &[OutboxAction::Recover],
        true,
    );
    assert_eq!(observation.stored.recoveries, 1);
    assert_eq!(observation.stored.status_payload, None);

    let mut recovering = live_delegation(child, caller, DelegationStatus::Running);
    recovering.recoveries = 1;
    let observation = apply_child_transition(
        recovering,
        AgentEvent::TurnAborted {
            turn: TurnId::new(),
            reason: AbortReason::ProviderExited,
        },
        DelegationFacts::default(),
    )
    .await?;
    assert_child_observation(
        &observation,
        DelegationStatus::Failed,
        &[OutboxAction::Deliver],
        true,
    );
    assert_eq!(
        observation.stored.status_payload.as_deref(),
        Some("provider exited twice")
    );

    for (event, status, actions) in [
        (
            AgentEvent::RuntimeError {
                fatal: true,
                message: "runtime failed".to_owned(),
            },
            DelegationStatus::Failed,
            vec![OutboxAction::Deliver],
        ),
        (
            AgentEvent::SessionExited {
                code: Some(1),
                expected: false,
            },
            DelegationStatus::Failed,
            vec![OutboxAction::Deliver],
        ),
        (
            AgentEvent::SessionExited {
                code: Some(0),
                expected: true,
            },
            DelegationStatus::Cancelled,
            vec![OutboxAction::Deliver, OutboxAction::CancelChildren],
        ),
    ] {
        let observation = apply_child_transition(
            live_delegation(child, caller, DelegationStatus::Running),
            event,
            DelegationFacts::default(),
        )
        .await?;
        assert_child_observation(&observation, status, &actions, true);
    }
    Ok(())
}

#[tokio::test]
async fn child_headline_and_terminal_noop_rules_run_through_the_writer() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let child = ThreadId::new();
    let caller = ThreadId::new();
    let delegation = live_delegation(child, caller, DelegationStatus::Running);
    let id = delegation.id;
    insert_test_delegation(&store, &delegation).await?;
    let (_wakes, changed) = capture_delegation_hooks(&store);
    let turn = TurnId::new();
    let item = ItemId::new();

    store
        .append_with_facts(
            child,
            &event(
                1,
                AgentEvent::ItemStarted {
                    turn,
                    item,
                    kind: ItemKind::Tool(Box::new(ToolCall {
                        kind: ToolKind::Bash,
                        name: "Bash".to_owned(),
                        input: serde_json::json!({ "command": "cargo test" }),
                        summary: Some("run the tests".to_owned()),
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
            DelegationFacts::default(),
        )
        .await?;
    assert_eq!(
        store
            .delegation(id)
            .await?
            .and_then(|delegation| delegation.headline),
        Some("run the tests".to_owned())
    );

    store
        .append_with_facts(
            child,
            &event(
                2,
                AgentEvent::ContentDelta {
                    item,
                    stream: StreamKind::CommandOutput,
                    delta: "ok".to_owned(),
                },
            ),
            DelegationFacts::default(),
        )
        .await?;
    store
        .append_with_facts(
            child,
            &event(
                3,
                AgentEvent::ItemUpdated {
                    item,
                    patch: ItemPatch {
                        status: Some(ItemStatus::Completed),
                        ..ItemPatch::default()
                    },
                },
            ),
            DelegationFacts::default(),
        )
        .await?;
    assert_eq!(
        store
            .delegation(id)
            .await?
            .and_then(|delegation| delegation.headline),
        None
    );
    assert_eq!(
        changed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        2,
        "the content delta is deliberately not a delegation change"
    );

    let terminal = live_delegation(ThreadId::new(), caller, DelegationStatus::Succeeded);
    let terminal_id = terminal.id;
    insert_test_delegation(&store, &terminal).await?;
    store
        .append_with_facts(
            terminal.child,
            &event(1, configured_event()),
            DelegationFacts::default(),
        )
        .await?;
    assert_eq!(store.delegation(terminal_id).await?, Some(terminal));
    assert!(store.delegation_outbox().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_terminal_child_item_wakes_an_open_settle_row_after_commit() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let child = ThreadId::new();
    let caller = ThreadId::new();
    let delegation = live_delegation(child, caller, DelegationStatus::Settling);
    let id = delegation.id;
    insert_test_delegation(&store, &delegation).await?;
    store
        .delegation_write("seed background settle", move |tx| {
            delegations::enqueue(tx, id, OutboxAction::Settle, stamp(1))?;
            Ok(((), false))
        })
        .await?;
    let (mut wakes, _changed) = capture_delegation_hooks(&store);
    let turn = TurnId::new();
    let item = ItemId::new();
    store
        .append_with_facts(
            child,
            &event(
                1,
                AgentEvent::ItemStarted {
                    turn,
                    item,
                    kind: ItemKind::Subagent {
                        name: "background".to_owned(),
                        description: "still working".to_owned(),
                        result: None,
                    },
                    parent: None,
                },
            ),
            DelegationFacts::default(),
        )
        .await?;
    assert!(wakes.try_recv().is_err());

    store
        .append_with_facts(
            child,
            &event(
                2,
                AgentEvent::ItemCompleted {
                    item,
                    status: ItemStatus::Completed,
                },
            ),
            DelegationFacts {
                background_live: true,
                ..DelegationFacts::default()
            },
        )
        .await?;

    wakes
        .try_recv()
        .context("terminal background item did not wake settle")?;
    assert!(wakes.try_recv().is_err(), "one commit sends one wake");
    assert_eq!(store.delegation_outbox().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn caller_delivery_records_the_message_sequence_and_finishes_deliver_work()
-> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let caller = ThreadId::new();
    let child = ThreadId::new();
    let delegation = live_delegation(child, caller, DelegationStatus::Succeeded);
    let id = delegation.id;
    insert_test_delegation(&store, &delegation).await?;
    store
        .delegation_write("seed caller delivery", move |tx| {
            delegations::enqueue(tx, id, OutboxAction::Deliver, stamp(1))?;
            Ok(((), false))
        })
        .await?;
    let (mut wakes, changed) = capture_delegation_hooks(&store);
    let turn = TurnId::new();
    let item = ItemId::new();
    let delivered = event(
        7,
        AgentEvent::ItemStarted {
            turn,
            item,
            kind: ItemKind::UserMessage {
                text: "child result".to_owned(),
                attachments: Vec::new(),
                steered: true,
                origin: MessageOrigin::Delegation { id },
            },
            parent: None,
        },
    );
    store
        .append_with_facts(caller, &delivered, DelegationFacts::default())
        .await?;

    let stored = store
        .delegation(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("delivered delegation disappeared"))?;
    assert_eq!(
        stored.delivery,
        DeliveryState::Delivered { seq: Seq(7), turn }
    );
    assert!(store.delegation_outbox().await?.is_empty());
    assert!(
        wakes.try_recv().is_err(),
        "delivery itself queues no new work"
    );
    assert_eq!(
        changed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[stored]
    );
    let item_detail: String = probe(&store)?.query_row(
        "SELECT detail_json FROM items WHERE thread_id = ?1 AND item_id = ?2",
        params![caller.to_string(), item.to_string()],
        |row| row.get(0),
    )?;
    assert!(
        item_detail.contains(&id.to_string()),
        "the caller item must retain the delegation origin: {item_detail}"
    );
    Ok(())
}

#[tokio::test]
async fn every_caller_progress_event_wakes_an_open_delivery_once() -> anyhow::Result<()> {
    for caller_event in [
        settled_event(TurnOutcome::Completed),
        configured_event(),
        AgentEvent::GateResolved {
            gate: GateId::new(),
            answer: GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
            by: GateResolver::User,
        },
        AgentEvent::SessionStateChanged(SessionState::Ready),
    ] {
        let (_directory, store) = store()?;
        let caller = ThreadId::new();
        let delegation = live_delegation(ThreadId::new(), caller, DelegationStatus::Succeeded);
        let id = delegation.id;
        let sequence = if let AgentEvent::GateResolved { gate, .. } = &caller_event {
            store
                .append(
                    caller,
                    &event(
                        1,
                        AgentEvent::GateOpened {
                            gate: *gate,
                            turn: None,
                            kind: GateKind::Plan {
                                markdown: "seed gate".to_owned(),
                                steps: Vec::new(),
                            },
                        },
                    ),
                )
                .await?;
            2
        } else {
            1
        };
        insert_test_delegation(&store, &delegation).await?;
        store
            .delegation_write("seed open delivery", move |tx| {
                delegations::enqueue(tx, id, OutboxAction::Deliver, stamp(1))?;
                Ok(((), false))
            })
            .await?;
        let (mut wakes, changed) = capture_delegation_hooks(&store);
        store
            .append_with_facts(
                caller,
                &event(sequence, caller_event),
                DelegationFacts::default(),
            )
            .await?;

        wakes
            .try_recv()
            .context("caller progress did not wake delivery")?;
        assert!(wakes.try_recv().is_err(), "one commit sends only one wake");
        assert!(
            changed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "a wake-only caller event does not mutate the delegation"
        );
        assert_eq!(store.delegation_outbox().await?.len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn a_rolled_back_event_runs_neither_delegation_hook() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let child = ThreadId::new();
    let caller = ThreadId::new();
    store
        .write_record(&record(child, "rollback child", 1))
        .await?;
    let delegation = live_delegation(child, caller, DelegationStatus::Starting);
    let id = delegation.id;
    insert_test_delegation(&store, &delegation).await?;
    let (mut wakes, changed) = capture_delegation_hooks(&store);
    runtime_writer(&store)?.execute_batch(&format!(
        "CREATE TRIGGER fail_transition_head \
         BEFORE UPDATE OF head_seq ON threads \
         WHEN NEW.thread_id = '{}' AND NEW.head_seq <> OLD.head_seq \
         BEGIN SELECT RAISE(FAIL, 'forced rollback after delegation transition'); END;",
        child
    ))?;

    let failed = store
        .append_with_facts(
            child,
            &event(1, configured_event()),
            DelegationFacts::default(),
        )
        .await;
    assert!(
        failed.is_err(),
        "the trigger must abort the event transaction"
    );
    assert_eq!(store.delegation(id).await?, Some(delegation));
    assert!(store.delegation_outbox().await?.is_empty());
    assert!(store.load(child).await?.is_empty());
    assert!(
        wakes.try_recv().is_err(),
        "a rollback cannot wake the worker"
    );
    assert!(
        changed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a rollback cannot publish an uncommitted delegation"
    );
    Ok(())
}

#[tokio::test]
async fn thread_delegation_metadata_round_trips_and_parent_reaches_the_summary()
-> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let parent = ThreadId::new();
    let mut child = record(ThreadId::new(), "child", 60);
    child.parent = Some(parent);
    child.delegation = Some(DelegationId::new());
    child.stop_cause = Some(StopCause::ProviderExit);

    store.write_record(&child).await?;

    assert_eq!(store.read_record(child.thread).await?, Some(child.clone()));
    let listed = store.summaries().await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].parent, Some(parent));
    Ok(())
}

#[tokio::test]
async fn delegation_rows_round_trip_every_column_and_update_mutable_state() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let stored = delegation(ThreadId::new());
    let expected = stored.clone();
    let id = stored.id;
    let child = stored.child;
    let caller = stored.caller;

    store
        .delegation_write("insert test delegation", move |tx| {
            delegations::insert(tx, &stored, "token-hash")?;
            Ok(((), false))
        })
        .await?;

    assert_eq!(store.delegation(id).await?, Some(expected.clone()));
    assert_eq!(
        store.delegation_by_child(child).await?,
        Some(expected.clone())
    );
    assert_eq!(
        store.delegation_token_hash(id).await?,
        Some("token-hash".to_owned())
    );
    assert_eq!(
        store.delegations(Some(caller)).await?,
        vec![expected.clone()]
    );
    assert_eq!(store.live_delegations(None).await?, vec![expected.clone()]);

    let mut updated = expected;
    updated.status = DelegationStatus::Succeeded;
    updated.status_payload = None;
    updated.result = Some(DelegationResult {
        text: "final report".to_owned(),
        files_changed: vec!["src/final.rs".to_owned()],
        source: ResultSource::Reported,
        elided: false,
    });
    updated.nudges = 3;
    updated.recoveries = 2;
    updated.delivery = DeliveryState::Undeliverable {
        reason: "caller stopped".to_owned(),
    };
    updated.finished = Some(stamp(80));
    updated.headline = Some("done".to_owned());
    let expected = updated.clone();
    store
        .delegation_write("update test delegation", move |tx| {
            delegations::update(tx, &updated)?;
            Ok(((), false))
        })
        .await?;

    assert_eq!(store.delegation(id).await?, Some(expected));
    assert!(store.live_delegations(None).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn delegation_report_and_its_idempotence_metadata_round_trip() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let stored = delegation(ThreadId::new());
    let id = stored.id;
    store
        .delegation_write("insert report test delegation", move |tx| {
            delegations::insert(tx, &stored, "report-token")?;
            Ok(((), false))
        })
        .await?;
    assert_eq!(delegations::report_meta(&probe(&store)?, id)?, None);

    let reported_at = stamp(81);
    let report_sha256 = "full-untruncated-report-hash";
    let report = DelegationResult {
        text: "stored report".to_owned(),
        files_changed: vec!["src/report.rs".to_owned()],
        source: ResultSource::Reported,
        elided: true,
    };
    let expected_report = report.clone();
    store
        .delegation_write("store delegation report", move |tx| {
            delegations::set_report(tx, id, &report, report_sha256, reported_at)?;
            Ok(((), false))
        })
        .await?;

    assert_eq!(
        delegations::report_meta(&probe(&store)?, id)?,
        Some((report_sha256.to_owned(), reported_at))
    );
    assert_eq!(
        store.delegation(id).await?.and_then(|row| row.result),
        Some(expected_report)
    );
    Ok(())
}

#[tokio::test]
async fn one_child_cannot_belong_to_two_delegations() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let child = ThreadId::new();
    let first = delegation(child);
    let second = delegation(child);
    store
        .delegation_write("insert first child delegation", move |tx| {
            delegations::insert(tx, &first, "first-token")?;
            Ok(((), false))
        })
        .await?;

    let duplicate = store
        .delegation_write("insert duplicate child delegation", move |tx| {
            delegations::insert(tx, &second, "second-token")?;
            Ok(((), false))
        })
        .await;

    assert!(
        duplicate.is_err(),
        "a child thread is unique: {duplicate:?}"
    );
    Ok(())
}

#[tokio::test]
async fn outbox_rows_are_ordered_and_done_rows_are_not_claimed() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let id = DelegationId::new();
    let (first, second, third) = store
        .delegation_write("enqueue ordered test actions", move |tx| {
            let first = delegations::enqueue(tx, id, OutboxAction::Nudge, stamp(90))?;
            let second = delegations::enqueue(tx, id, OutboxAction::Settle, stamp(91))?;
            let third = delegations::enqueue(tx, id, OutboxAction::Recover, stamp(92))?;
            Ok(((first, second, third), false))
        })
        .await?;
    assert!(first < second && second < third);
    assert_eq!(
        store
            .delegation_outbox()
            .await?
            .into_iter()
            .map(|row| row.action)
            .collect::<Vec<_>>(),
        vec![
            OutboxAction::Nudge,
            OutboxAction::Settle,
            OutboxAction::Recover
        ]
    );

    store
        .delegation_write("finish test actions", move |tx| {
            delegations::mark_done(tx, first, stamp(93))?;
            let finished = delegations::mark_done_for(tx, id, OutboxAction::Recover, stamp(94))?;
            Ok((finished, false))
        })
        .await?;
    let remaining = store
        .delegation_write("read one delegation outbox", move |tx| {
            let rows = delegations::open_rows_for(tx, id)?;
            Ok((rows, false))
        })
        .await?;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, second);
    assert_eq!(remaining[0].action, OutboxAction::Settle);
    Ok(())
}

#[tokio::test]
async fn delegation_write_commits_before_it_pokes_the_wake_channel() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let id = DelegationId::new();
    let (wake, mut wakes) = tokio::sync::mpsc::unbounded_channel();
    store.install_delegation_hooks(DelegationHooks {
        wake,
        changed: Arc::new(|_delegation| {}),
    });

    store
        .delegation_write("enqueue and wake", move |tx| {
            delegations::enqueue(tx, id, OutboxAction::Deliver, stamp(100))?;
            Ok(((), true))
        })
        .await?;

    wakes.try_recv().context("receive the post-commit wake")?;
    assert_eq!(store.delegation_outbox().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn delegation_write_needs_no_installed_hooks() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let id = DelegationId::new();

    store
        .delegation_write("wake without hooks", move |tx| {
            delegations::enqueue(tx, id, OutboxAction::Mirror, stamp(101))?;
            Ok(((), true))
        })
        .await?;

    assert_eq!(store.delegation_outbox().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn caller_exists_requires_a_visible_thread() -> anyhow::Result<()> {
    let (_directory, store) = store()?;
    let caller = ThreadId::new();
    let unknown = ThreadId::new();
    store.write_record(&record(caller, "caller", 110)).await?;

    let (listed, never_seen) = store
        .delegation_write("check caller existence", move |tx| {
            Ok((
                (
                    delegations::caller_exists(tx, caller)?,
                    delegations::caller_exists(tx, unknown)?,
                ),
                false,
            ))
        })
        .await?;

    assert!(listed);
    assert!(
        !never_seen,
        "a thread this daemon never recorded is not a caller"
    );

    // Hiding a thread by omitting it from an index rewrite is the store's only delete, so a
    // delegation must not be startable against one that was hidden.
    store
        .write_index(&AgentIndex {
            version: super::AGENT_INDEX_VERSION,
            threads: Vec::new(),
        })
        .await?;
    let hidden = store
        .delegation_write("check a hidden caller", move |tx| {
            Ok((delegations::caller_exists(tx, caller)?, false))
        })
        .await?;

    assert!(!hidden, "a soft-deleted thread is not a caller");
    Ok(())
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
