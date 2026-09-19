use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationId, DelegationStatus, DeliveryState, ItemId, ThreadId,
        TurnId,
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
                delegations::insert(tx, &stored, "test-token")?;
                Ok(((), false))
            })
            .await
            .expect("insert delegation");
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
        caller,
        caller_turn: TurnId::new(),
        caller_item: ItemId::new(),
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
