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
        agents::{AgentSessionManager, store::SqliteAgentStore},
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
                tx.execute(
                    "INSERT INTO delegations (id, token_sha256, caller_thread, caller_turn, \
                     caller_item, child_thread, provider, depth, brief, expectation, eager, \
                     status, delivery, created, finished) \
                     VALUES (?1, 'test-token', ?2, ?3, ?4, ?5, 'codex', ?6, ?7, ?8, ?9, \
                             ?10, 'pending', ?11, ?12)",
                    rusqlite::params![
                        stored.id.to_string(),
                        stored.caller.to_string(),
                        stored.caller_turn.to_string(),
                        stored.caller_item.to_string(),
                        stored.child.to_string(),
                        i64::from(stored.depth),
                        stored.brief,
                        stored.expectation,
                        i64::from(stored.eager),
                        status_word(stored.status),
                        stored.created.to_rfc3339(),
                        stored.finished.map(|stamp| stamp.to_rfc3339()),
                    ],
                )?;
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
                tx.execute(
                    "UPDATE delegations SET status = ?2, finished = ?3 WHERE id = ?1",
                    rusqlite::params![
                        stored.id.to_string(),
                        status_word(stored.status),
                        stored.finished.map(|stamp| stamp.to_rfc3339()),
                    ],
                )?;
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
    }
}

const fn status_word(status: DelegationStatus) -> &'static str {
    match status {
        DelegationStatus::Starting => "starting",
        DelegationStatus::Running => "running",
        DelegationStatus::Blocked => "blocked",
        DelegationStatus::Settling => "settling",
        DelegationStatus::Succeeded => "succeeded",
        DelegationStatus::Incomplete => "incomplete",
        DelegationStatus::Failed => "failed",
        DelegationStatus::Cancelled => "cancelled",
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

    let waiting = harness.service.wait(running.id, 10_000);
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
        .wait(running.id, 0)
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
