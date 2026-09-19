use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, Delegation, DelegationId, DelegationStatus, DeliveryState, ItemId,
        ItemKind, PermissionMode, ResultSource, Seq, SeqEvent, SessionState, ThreadId, TurnId,
        TurnOutcome, Usage,
    },
    ids::WorktreeId,
    paths::FleetHome,
};
use fleet_proto::{error::ErrorKind, event::Event, response::ResponseBody};
use sha2::{Digest, Sha256};

use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::BroadcastBus,
    services::{
        agents::{
            AgentSessionManager, AgentThreadRecord,
            delegation::transition::DelegationFacts,
            store::{OutboxAction, SqliteAgentStore, delegations},
        },
        sessions::Sessions,
        worktrees::Worktrees,
    },
    stores::{config::ConfigStore, state::StateStore},
};

use super::super::{CompleteRequest, DelegationService, limits::RESULT_CAP_BYTES};
use super::run::{Harness as ScriptedHarness, request as run_request, started};

const TOKEN: &str = "only-the-child-knows-this";

struct Harness {
    _directory: tempfile::TempDir,
    service: DelegationService,
    store: SqliteAgentStore,
    events: BroadcastBus,
}

impl Harness {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create complete test directory");
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
        let events = BroadcastBus::new(32);
        let manager = AgentSessionManager::new(
            FleetHome::new(&home).agents_db_path(),
            events.clone(),
            worktrees.clone(),
            Arc::clone(&config),
        );
        let store = manager
            .delegation_store()
            .expect("the complete test database opens");
        let (service, _worker) =
            DelegationService::new(store.clone(), manager, events.clone(), config, worktrees);
        Self {
            _directory: directory,
            service,
            store,
            events,
        }
    }

    async fn insert(&self, delegation: Delegation) {
        self.insert_with_background(delegation, false).await;
    }

    async fn insert_with_background(&self, delegation: Delegation, background: bool) {
        self.store
            .write_record(&child_record(&delegation))
            .await
            .expect("write child record");
        if delegation.status == DelegationStatus::Settling {
            // A bare record hydrates as a restart orphan and emits failure events. Give this
            // already-settled fixture a terminal session event before its delegation exists, so
            // `complete` can safely inspect its empty background-task set.
            let turn = TurnId::new();
            let background_item = ItemId::new();
            let events = if background {
                vec![
                    AgentEvent::SessionStateChanged(SessionState::Ready),
                    AgentEvent::TurnStarted {
                        turn,
                        user_item: ItemId::new(),
                    },
                    AgentEvent::ItemStarted {
                        turn,
                        item: background_item,
                        kind: ItemKind::Subagent {
                            name: "background".to_owned(),
                            description: "still working".to_owned(),
                            result: None,
                        },
                        parent: None,
                    },
                    AgentEvent::TurnSettled {
                        turn,
                        outcome: TurnOutcome::Completed,
                        usage: Usage::default(),
                        duration_ms: 1,
                        files_changed: Vec::new(),
                    },
                    AgentEvent::SessionExited {
                        code: Some(0),
                        expected: true,
                    },
                ]
            } else {
                vec![
                    AgentEvent::SessionStateChanged(SessionState::Ready),
                    AgentEvent::TurnStarted {
                        turn,
                        user_item: ItemId::new(),
                    },
                    AgentEvent::TurnSettled {
                        turn,
                        outcome: TurnOutcome::Completed,
                        usage: Usage::default(),
                        duration_ms: 1,
                        files_changed: Vec::new(),
                    },
                    AgentEvent::SessionExited {
                        code: Some(0),
                        expected: true,
                    },
                ]
            };
            for (index, event) in events.into_iter().enumerate() {
                self.store
                    .append(
                        delegation.child,
                        &SeqEvent {
                            seq: Seq(u64::try_from(index + 1).expect("small fixture sequence")),
                            at: stamp(3 + i64::try_from(index).expect("small fixture time")),
                            raw: None,
                            event,
                        },
                    )
                    .await
                    .expect("settle fixture child event");
            }
        } else {
            self.store
                .append(
                    delegation.child,
                    &SeqEvent {
                        seq: Seq(1),
                        at: stamp(3),
                        raw: None,
                        event: AgentEvent::SessionExited {
                            code: Some(0),
                            expected: true,
                        },
                    },
                )
                .await
                .expect("stop fixture child session");
        }
        self.store
            .delegation_write("insert complete-test delegation", move |tx| {
                delegations::insert(tx, &delegation, &sha256(TOKEN))?;
                Ok(((), false))
            })
            .await
            .expect("insert delegation");
    }

    async fn stored(&self, id: DelegationId) -> Delegation {
        self.store
            .delegation(id)
            .await
            .expect("read delegation")
            .expect("delegation exists")
    }

    async fn report_meta(&self, id: DelegationId) -> (String, DateTime<Utc>) {
        self.store
            .delegation_write("read complete-test report metadata", move |tx| {
                Ok((delegations::report_meta(tx, id)?, false))
            })
            .await
            .expect("read report metadata")
            .expect("report metadata exists")
    }

    async fn settle(&self, delegation: &Delegation) {
        let seq = self
            .store
            .load(delegation.child)
            .await
            .expect("read child fixture events")
            .last()
            .map_or(Seq(1), |event| Seq(event.seq.0.saturating_add(1)));
        self.store
            .append_with_facts(
                delegation.child,
                &SeqEvent {
                    seq,
                    at: stamp(50),
                    raw: None,
                    event: AgentEvent::TurnSettled {
                        turn: TurnId::new(),
                        outcome: TurnOutcome::Completed,
                        usage: Usage::default(),
                        duration_ms: 1,
                        files_changed: Vec::new(),
                    },
                },
                DelegationFacts::default(),
            )
            .await
            .expect("settle child");
    }
}

fn stamp(offset: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(1_700_000_000_000 + offset)
        .single()
        .expect("the fixed test timestamp is valid")
}

fn delegation(status: DelegationStatus) -> Delegation {
    Delegation {
        id: DelegationId::new(),
        caller: ThreadId::new(),
        caller_turn: TurnId::new(),
        caller_item: ItemId::new(),
        child: ThreadId::new(),
        provider: AgentKind::Codex,
        depth: 1,
        brief: "verify complete".to_owned(),
        expectation: "record exactly one report".to_owned(),
        eager: false,
        status,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: stamp(1),
        finished: status.is_terminal().then(|| stamp(2)),
        headline: None,
        usage: None,
    }
}

fn child_record(delegation: &Delegation) -> AgentThreadRecord {
    AgentThreadRecord {
        thread: delegation.child,
        parent: Some(delegation.caller),
        delegation: Some(delegation.id),
        worktree: WorktreeId::try_from("owner/repo#complete-test").expect("test worktree id"),
        provider: delegation.provider,
        title: "complete child".to_owned(),
        created: delegation.created,
        last_activity: delegation.created,
        resume_cursor: None,
        model: None,
        mode: PermissionMode::FullAccess,
        last_outcome: None,
        stop_cause: None,
    }
}

fn request(delegation: &Delegation, token: &str, result: &str, blocked: bool) -> CompleteRequest {
    CompleteRequest {
        delegation: delegation.id,
        child: delegation.child,
        token: token.to_owned(),
        result: result.to_owned(),
        blocked,
    }
}

fn one(response: ResponseBody) -> Delegation {
    match response {
        ResponseBody::Delegation(delegation) => delegation,
        other => panic!("expected one delegation, got {other:?}"),
    }
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[tokio::test]
async fn unknown_delegation_is_not_found() {
    let harness = Harness::new();
    let missing = delegation(DelegationStatus::Running);

    let error = harness
        .service
        .complete(request(&missing, TOKEN, "report", false))
        .await
        .expect_err("an unknown delegation is refused");

    assert_eq!(error.kind, ErrorKind::NotFound);
    assert_eq!(
        error.message,
        format!("delegation {} does not exist", missing.id)
    );
}

#[tokio::test(start_paused = true)]
async fn token_mismatch_is_validation_and_changes_nothing() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Running);
    harness.insert(current.clone()).await;

    let error = harness
        .service
        .complete(request(&current, "wrong token", "report", false))
        .await
        .expect_err("the wrong token is refused");

    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(error.message, "delegation token does not match");
    assert_eq!(harness.stored(current.id).await, current);
}

#[tokio::test(start_paused = true)]
async fn a_grandchild_cannot_complete_its_parents_delegation() {
    let harness = ScriptedHarness::start().await;
    let (caller, _) = harness.running_caller().await;
    let (parent, _) = started(
        harness
            .run(run_request(caller))
            .await
            .expect("start parent delegation"),
    );
    let token = harness.token(&parent).await;
    tokio::time::resume();
    harness
        .wait_for(parent.child, |projection| {
            matches!(projection.turn, fleet_core::agents::TurnState::Running(_))
        })
        .await;
    tokio::time::pause();
    let (grandchild, _) = started(
        harness
            .run(run_request(parent.child))
            .await
            .expect("start grandchild delegation"),
    );

    let error = harness
        .service()
        .complete(CompleteRequest {
            delegation: parent.id,
            child: grandchild.child,
            token,
            result: "wrong generation".to_owned(),
            blocked: false,
        })
        .await
        .expect_err("a grandchild is not its parent's reporting child");

    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(error.message, "this delegation belongs to another thread");
}

#[tokio::test]
async fn a_valid_token_cannot_complete_another_childs_delegation() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Running);
    harness.insert(current.clone()).await;
    let mut wrong_child = request(&current, TOKEN, "report", false);
    wrong_child.child = ThreadId::new();

    let error = harness
        .service
        .complete(wrong_child)
        .await
        .expect_err("a different child is refused");

    assert_eq!(error.kind, ErrorKind::Validation);
    assert_eq!(error.message, "this delegation belongs to another thread");
    assert_eq!(harness.stored(current.id).await, current);
}

#[tokio::test]
async fn an_unreported_terminal_delegation_is_refused() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Failed);
    harness.insert(current.clone()).await;

    let error = harness
        .service
        .complete(request(&current, TOKEN, "too late", false))
        .await
        .expect_err("a terminal delegation is refused");

    assert_eq!(error.kind, ErrorKind::Conflict);
    assert_eq!(
        error.message,
        format!("delegation {} is already terminal (failed)", current.id)
    );
    assert_eq!(harness.stored(current.id).await, current);
}

#[tokio::test]
async fn an_identical_repeat_is_idempotent_and_publishes_only_the_first_change() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Running);
    harness.insert(current.clone()).await;
    let mut events = harness.events.subscribe();

    let first = one(harness
        .service
        .complete(request(&current, TOKEN, "the report", false))
        .await
        .expect("the first report succeeds"));
    let published = events.try_recv().expect("the accepted report is published");
    assert_eq!(published, Event::DelegationChanged(first.clone()));

    let repeated = one(harness
        .service
        .complete(request(&current, TOKEN, "the report", false))
        .await
        .expect("the identical report succeeds"));

    assert_eq!(repeated, first);
    assert!(
        events.try_recv().is_err(),
        "a repeat changes and publishes nothing"
    );
}

#[tokio::test]
async fn a_different_repeat_names_the_first_reports_time() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Running);
    harness.insert(current.clone()).await;
    harness
        .service
        .complete(request(&current, TOKEN, "first", false))
        .await
        .expect("the first report succeeds");
    let (_, reported_at) = harness.report_meta(current.id).await;

    let error = harness
        .service
        .complete(request(&current, TOKEN, "different", false))
        .await
        .expect_err("a different repeat is refused");

    assert_eq!(error.kind, ErrorKind::Conflict);
    assert_eq!(
        error.message,
        format!(
            "delegation {} already reported at {}",
            current.id,
            reported_at.to_rfc3339()
        )
    );
}

#[tokio::test]
async fn an_oversized_report_is_truncated_on_a_character_boundary_and_marked_elided() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Running);
    harness.insert(current.clone()).await;
    let mut report = "a".repeat(RESULT_CAP_BYTES - 1);
    report.push('é');
    report.push('z');

    let accepted = one(harness
        .service
        .complete(request(&current, TOKEN, &report, false))
        .await
        .expect("the oversized report succeeds"));
    let result = accepted.result.expect("the report is stored");
    let (stored_hash, _) = harness.report_meta(current.id).await;

    assert_eq!(result.text, "a".repeat(RESULT_CAP_BYTES - 1));
    assert_eq!(result.source, ResultSource::Reported);
    assert!(result.elided);
    assert_eq!(
        stored_hash,
        sha256(&report),
        "the hash covers the full report"
    );
}

#[tokio::test]
async fn a_blocked_report_marks_the_delegation_and_settlement_fails_it() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Running);
    harness.insert(current.clone()).await;

    let blocked = one(harness
        .service
        .complete(request(&current, TOKEN, "need credentials", true))
        .await
        .expect("the blocked report succeeds"));

    assert_eq!(blocked.status, DelegationStatus::Blocked);
    assert_eq!(blocked.status_payload.as_deref(), Some("reported blocked"));
    assert_eq!(
        blocked.result.as_ref().map(|result| result.source),
        Some(ResultSource::Reported)
    );

    harness.settle(&blocked).await;
    let failed = harness.stored(blocked.id).await;
    assert_eq!(failed.status, DelegationStatus::Failed);
    assert_eq!(failed.status_payload.as_deref(), Some("reported blocked"));
    assert_eq!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read outbox")[0]
            .action,
        OutboxAction::Deliver
    );
}

#[tokio::test]
async fn a_blocked_report_after_settlement_fails_and_replaces_obsolete_work() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Settling);
    harness.insert(current.clone()).await;
    harness
        .store
        .delegation_write("stage obsolete settlement work", {
            let current = current.clone();
            move |tx| {
                delegations::enqueue(tx, current.id, OutboxAction::Nudge, Utc::now())?;
                delegations::enqueue(tx, current.id, OutboxAction::Settle, Utc::now())?;
                Ok(((), false))
            }
        })
        .await
        .expect("stage obsolete settlement work");

    let failed = one(harness
        .service
        .complete(request(&current, TOKEN, "need credentials", true))
        .await
        .expect("the late blocked report succeeds"));
    let rows = harness
        .store
        .delegation_outbox()
        .await
        .expect("read outbox");

    assert_eq!(failed.status, DelegationStatus::Failed);
    assert_eq!(failed.status_payload.as_deref(), Some("reported blocked"));
    assert!(failed.finished.is_some());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, OutboxAction::Deliver);
}

#[tokio::test]
async fn complete_then_settle_finishes_succeeded_and_enqueues_delivery() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Running);
    harness.insert(current.clone()).await;

    let reported = one(harness
        .service
        .complete(request(&current, TOKEN, "done", false))
        .await
        .expect("the report succeeds"));
    assert_eq!(reported.status, DelegationStatus::Running);
    assert!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read outbox")
            .is_empty()
    );

    harness.settle(&reported).await;
    let succeeded = harness.stored(reported.id).await;
    assert_eq!(succeeded.status, DelegationStatus::Succeeded);
    assert!(succeeded.finished.is_some());
    assert_eq!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read outbox")[0]
            .action,
        OutboxAction::Deliver
    );
}

#[tokio::test]
async fn settle_then_complete_finishes_and_enqueues_delivery_in_the_accepting_write() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Settling);
    harness.insert(current.clone()).await;

    let succeeded = one(harness
        .service
        .complete(request(&current, TOKEN, "done after settle", false))
        .await
        .expect("the late report succeeds"));
    let rows = harness
        .store
        .delegation_outbox()
        .await
        .expect("read outbox");

    assert_eq!(succeeded.status, DelegationStatus::Succeeded);
    assert!(succeeded.finished.is_some());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, OutboxAction::Deliver);
    assert_eq!(harness.stored(current.id).await, succeeded);

    let repeated = one(harness
        .service
        .complete(request(&current, TOKEN, "done after settle", false))
        .await
        .expect("an identical retry after terminal success is idempotent"));
    assert_eq!(repeated, succeeded);
    assert_eq!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read outbox")
            .len(),
        1,
        "the retry does not enqueue a second delivery"
    );
}

#[tokio::test]
async fn late_report_with_background_work_replaces_nudge_with_settle() {
    let harness = Harness::new();
    let current = delegation(DelegationStatus::Settling);
    harness.insert_with_background(current.clone(), true).await;
    let delegation_id = current.id;
    harness
        .store
        .delegation_write("seed obsolete nudge", move |tx| {
            delegations::enqueue(tx, delegation_id, OutboxAction::Nudge, stamp(20))?;
            Ok(((), false))
        })
        .await
        .expect("seed nudge");

    let reported = one(harness
        .service
        .complete(request(&current, TOKEN, "done after background", false))
        .await
        .expect("accept late report"));
    let rows = harness
        .store
        .delegation_outbox()
        .await
        .expect("read outbox");

    assert_eq!(reported.status, DelegationStatus::Settling);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, OutboxAction::Settle);
}
