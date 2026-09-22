use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, Delegation, DelegationCaller, DelegationId, DelegationStatus,
        DelegationUsage, DeliveryState, ItemId, PermissionMode, Seq, SeqEvent, ThreadId, TurnId,
        TurnOutcome, Usage,
    },
    paths::FleetHome,
};
use fleet_proto::{error::ErrorKind, response::ResponseBody};

use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::BroadcastBus,
    services::{
        agents::{
            AgentSessionManager,
            store::{SqliteAgentStore, delegations},
        },
        sessions::Sessions,
        worktrees::Worktrees,
    },
    stores::{config::ConfigStore, state::StateStore},
};

use super::super::DelegationService;

struct Harness {
    _directory: tempfile::TempDir,
    service: DelegationService,
    store: SqliteAgentStore,
}

impl Harness {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create delegation test directory");
        let home = directory.path().join("fleet");
        let files = Arc::new(RealFiles::new(home.join("trash"), [home.clone()]));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
        let worktrees = Worktrees::new(
            Arc::clone(&config),
            state,
            Arc::new(JobManager::new(&home)),
            &Adapters::system(files),
            sessions,
        );
        let events = BroadcastBus::new(16);
        let manager = AgentSessionManager::new(
            FleetHome::new(&home).agents_db_path(),
            events.clone(),
            worktrees.clone(),
            Arc::clone(&config),
        );
        let store = manager
            .delegation_store()
            .expect("the test agent database opens");
        let (service, _worker) =
            DelegationService::new(store.clone(), manager, events, config, worktrees);
        Self {
            _directory: directory,
            service,
            store,
        }
    }

    async fn insert(&self, delegation: Delegation) {
        let stored = delegation.clone();
        self.store
            .delegation_write("insert query-test delegation", move |tx| {
                delegations::insert(
                    tx,
                    &stored,
                    "test-token",
                    &std::collections::BTreeMap::new(),
                )?;
                Ok(((), false))
            })
            .await
            .expect("insert delegation");
    }

    /// Gives one child thread a settled turn with usage, and answers what a reader should see.
    ///
    /// The numbers are the log's, not a restatement of them: `store::usage`'s own tests pin the
    /// arithmetic against `ThreadProjection`, and what these tests need to know is only that the
    /// read verbs attach it to the right row.
    async fn spend(&self, child: ThreadId) -> DelegationUsage {
        let turn = TurnId::new();
        let usage = Usage {
            input_tokens: 900,
            output_tokens: 120,
            total_tokens: 1_020,
            ..Usage::default()
        };
        let log = [
            AgentEvent::SessionConfigured {
                provider: AgentKind::Codex,
                resume_cursor: None,
                model: None,
                models: Vec::new(),
                mode: PermissionMode::Ask,
                tools: Vec::new(),
                commands: Vec::new(),
                skills: Vec::new(),
            },
            AgentEvent::TurnStarted {
                turn,
                user_item: ItemId::new(),
            },
            AgentEvent::TokenUsage {
                turn,
                usage: usage.clone(),
                context_pct: 18.5,
                cost_usd: Some(0.27),
            },
            AgentEvent::TurnSettled {
                turn,
                outcome: TurnOutcome::Completed,
                usage: usage.clone(),
                duration_ms: 1_000,
                files_changed: Vec::new(),
            },
        ];
        for (position, event) in log.into_iter().enumerate() {
            let seq = u64::try_from(position).unwrap_or_default() + 1;
            self.store
                .append(
                    child,
                    &SeqEvent {
                        seq: Seq(seq),
                        at: stamp(i64::try_from(seq).unwrap_or_default()),
                        raw: None,
                        event,
                    },
                )
                .await
                .expect("append a child event");
        }
        DelegationUsage {
            usage,
            cost_usd: Some(0.27),
            context_pct: 18.5,
        }
    }

    async fn finish(&self, mut delegation: Delegation, status: DelegationStatus) {
        delegation.status = status;
        delegation.finished = Some(stamp(99));
        let stored = delegation.clone();
        self.store
            .delegation_write("finish query-test delegation", move |tx| {
                delegations::update(tx, &stored)?;
                Ok(((), false))
            })
            .await
            .expect("finish delegation");
        self.service.publish_changed(delegation);
    }
}

fn stamp(offset: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(1_700_000_000_000 + offset)
        .single()
        .expect("the fixed test timestamp is valid")
}

fn delegation(caller: ThreadId, created_offset: i64) -> Delegation {
    Delegation {
        id: DelegationId::new(),
        caller: DelegationCaller::Thread(caller),
        caller_turn: Some(TurnId::new()),
        caller_item: Some(ItemId::new()),
        child: ThreadId::new(),
        provider: AgentKind::Codex,
        depth: 1,
        brief: "inspect the query path".to_owned(),
        expectation: "return the current durable record".to_owned(),
        eager: false,
        status: DelegationStatus::Running,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: stamp(created_offset),
        finished: None,
        headline: None,
        usage: None,
    }
}

fn one(response: ResponseBody) -> Delegation {
    match response {
        ResponseBody::Delegation(delegation) => delegation,
        other => panic!("expected one delegation, got {other:?}"),
    }
}

#[tokio::test]
async fn wait_returns_on_the_terminal_event() {
    let harness = Harness::new();
    let running = delegation(ThreadId::new(), 1);
    harness.insert(running.clone()).await;

    let waiting = harness.service.wait(running.id, 10_000, None);
    let finishing = async {
        // `join!` polls `waiting` first, so its subscribe-before-read invariant is active before
        // this update. Yielding lets that initial database read finish without sleeping.
        tokio::task::yield_now().await;
        harness
            .finish(running.clone(), DelegationStatus::Succeeded)
            .await;
    };
    let (response, ()) = tokio::join!(waiting, finishing);

    let finished = one(response.expect("wait succeeds"));
    assert_eq!(finished.status, DelegationStatus::Succeeded);
}

#[tokio::test]
async fn wait_times_out_with_the_current_record() {
    let harness = Harness::new();
    let running = delegation(ThreadId::new(), 1);
    harness.insert(running.clone()).await;

    let current = one(harness
        .service
        .wait(running.id, 0, None)
        .await
        .expect("wait succeeds"));

    assert_eq!(current, running);
}

#[tokio::test]
async fn cancel_refuses_a_terminal_delegation() {
    let harness = Harness::new();
    let mut terminal = delegation(ThreadId::new(), 1);
    terminal.status = DelegationStatus::Cancelled;
    terminal.finished = Some(stamp(2));
    harness.insert(terminal.clone()).await;

    let error = harness
        .service
        .cancel(terminal.id)
        .await
        .expect_err("a terminal delegation cannot be cancelled again");

    assert_eq!(error.kind, ErrorKind::Conflict);
    assert_eq!(
        one(harness
            .service
            .get(terminal.id)
            .await
            .expect("get succeeds")),
        terminal
    );
}

/// The duplicate this whole field exists to stop: an orchestrator that waited on eight children
/// used to be sent all eight results again as user messages once its turn settled.
#[tokio::test]
async fn a_callers_own_wait_consumes_a_terminal_delivery() {
    let harness = Harness::new();
    let caller = ThreadId::new();
    let finished = delegation(caller, 1);
    harness.insert(finished.clone()).await;
    harness
        .finish(finished.clone(), DelegationStatus::Succeeded)
        .await;

    let answered = one(harness
        .service
        .wait(finished.id, 0, Some(caller))
        .await
        .expect("wait succeeds"));

    assert_eq!(answered.delivery, DeliveryState::Consumed);
    assert_eq!(
        harness
            .store
            .delegation(finished.id)
            .await
            .expect("read delegation")
            .expect("delegation exists")
            .delivery,
        DeliveryState::Consumed
    );
}

/// Idempotent on purpose: a caller that waits twice, or that races the delivery worker and loses,
/// gets an answer rather than an error.
#[tokio::test]
async fn consuming_a_delivery_twice_still_answers_the_record() {
    let harness = Harness::new();
    let caller = ThreadId::new();
    let finished = delegation(caller, 1);
    harness.insert(finished.clone()).await;
    harness
        .finish(finished.clone(), DelegationStatus::Succeeded)
        .await;

    for _ in 0..2 {
        let answered = one(harness
            .service
            .wait(finished.id, 0, Some(caller))
            .await
            .expect("wait succeeds"));
        assert_eq!(answered.delivery, DeliveryState::Consumed);
    }
}

/// Identity, not authorisation: a stranger is answered the same record and changes nothing.
#[tokio::test]
async fn a_wait_from_anyone_but_the_caller_leaves_the_delivery_pending() {
    let harness = Harness::new();
    let finished = delegation(ThreadId::new(), 1);
    harness.insert(finished.clone()).await;
    harness
        .finish(finished.clone(), DelegationStatus::Succeeded)
        .await;

    for waiter in [None, Some(ThreadId::new())] {
        let answered = one(harness
            .service
            .wait(finished.id, 0, waiter)
            .await
            .expect("wait succeeds"));
        assert_eq!(answered.delivery, DeliveryState::Pending);
    }
    assert_eq!(
        harness
            .store
            .delegation(finished.id)
            .await
            .expect("read delegation")
            .expect("delegation exists")
            .delivery,
        DeliveryState::Pending
    );
}

/// A wait that times out answers a live record, and a live record has nothing to consume.
#[tokio::test]
async fn a_wait_that_times_out_consumes_nothing() {
    let harness = Harness::new();
    let caller = ThreadId::new();
    let running = delegation(caller, 1);
    harness.insert(running.clone()).await;

    let current = one(harness
        .service
        .wait(running.id, 0, Some(caller))
        .await
        .expect("wait succeeds"));

    assert_eq!(current, running);
}

#[tokio::test]
async fn list_is_newest_first_and_filters_by_caller() {
    let harness = Harness::new();
    let caller = ThreadId::new();
    let oldest = delegation(caller, 1);
    let newest = delegation(caller, 3);
    let other = delegation(ThreadId::new(), 4);
    for delegation in [oldest.clone(), newest.clone(), other] {
        harness.insert(delegation).await;
    }

    let listed = harness
        .service
        .list(Some(caller))
        .await
        .expect("list succeeds");

    assert_eq!(listed, ResponseBody::Delegations(vec![newest, oldest]));
}

/// The cost-visibility item: a caller reading its child's record can see what the child spent,
/// without the daemon hydrating that child's transcript to answer.
#[tokio::test]
async fn get_and_wait_carry_what_the_child_spent() {
    let harness = Harness::new();
    let caller = ThreadId::new();
    let finished = delegation(caller, 1);
    harness.insert(finished.clone()).await;
    let spent = harness.spend(finished.child).await;
    harness
        .finish(finished.clone(), DelegationStatus::Succeeded)
        .await;

    let read = one(harness
        .service
        .get(finished.id)
        .await
        .expect("get succeeds"));
    assert_eq!(read.usage, Some(spent.clone()));
    // On the `wait` path the record is re-read by the consume, so the fill has to come after it.
    let answered = one(harness
        .service
        .wait(finished.id, 0, Some(caller))
        .await
        .expect("wait succeeds"));
    assert_eq!(answered.delivery, DeliveryState::Consumed);
    assert_eq!(answered.usage, Some(spent));
}

/// Every row of a page carries its own child's numbers, and a child that spent nothing carries
/// `None` rather than a zero the CLI would have to render as a measurement.
#[tokio::test]
async fn list_carries_what_every_child_spent() {
    let harness = Harness::new();
    let caller = ThreadId::new();
    let quiet = delegation(caller, 1);
    let spender = delegation(caller, 2);
    harness.insert(quiet.clone()).await;
    harness.insert(spender.clone()).await;
    let spent = harness.spend(spender.child).await;

    let ResponseBody::Delegations(listed) = harness
        .service
        .list(Some(caller))
        .await
        .expect("list succeeds")
    else {
        panic!("list answers a page of delegations");
    };

    assert_eq!(
        listed
            .iter()
            .map(|row| (row.id, row.usage.clone()))
            .collect::<Vec<_>>(),
        vec![(spender.id, Some(spent)), (quiet.id, None)],
        "newest first, each row with its own child's spend"
    );
}
