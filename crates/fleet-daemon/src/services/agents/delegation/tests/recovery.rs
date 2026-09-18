use chrono::Utc;
use fleet_core::agents::{
    AbortReason, AgentEvent, AgentKind, Delegation, DelegationId, DelegationResult,
    DelegationStatus, DeliveryState, ItemId, ItemKind, MessageOrigin, PermissionMode, ResultSource,
    SessionState, ThreadId, TurnId, TurnOutcome, TurnState, Usage, UserInput,
};
use fleet_proto::response::ResponseBody;
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use crate::services::agents::{
    AgentSessionManager,
    manager::tests::{Harness as ManagerHarness, full},
    store::{OutboxAction, SqliteAgentStore, delegations},
};

use super::{
    CompleteRequest, DelegationWorker,
    tests::{
        run::{request, started},
        worker::Harness,
    },
    worker::drain,
};

async fn running_caller(harness: &Harness) -> ThreadId {
    let caller = harness.create_thread().await;
    tokio::time::resume();
    harness.send(caller, "hold caller turn").await;
    harness
        .wait_for(caller, |projection| {
            matches!(projection.turn, TurnState::Running(_))
        })
        .await;
    tokio::time::pause();
    caller
}

async fn start_delegation(harness: &Harness, caller: ThreadId, eager: bool) -> Delegation {
    tokio::time::resume();
    let mut run = request(caller);
    run.eager = eager;
    let response = harness
        .service
        .run(run)
        .await
        .expect("start recovery-test delegation");
    let (delegation, _) = started(response);
    harness
        .wait_for(delegation.child, |projection| {
            matches!(projection.turn, TurnState::Running(_))
        })
        .await;
    tokio::time::pause();
    delegation
}

async fn start_worker(harness: &mut Harness) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let worker = harness.take_worker();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(worker.run(shutdown.clone()));
    (shutdown, task)
}

async fn stop_worker(shutdown: CancellationToken, task: tokio::task::JoinHandle<()>) {
    shutdown.cancel();
    task.await.expect("delegation worker stops cleanly");
}

fn spawn_worker(worker: DelegationWorker) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(worker.run(shutdown.clone()));
    (shutdown, task)
}

async fn configure(harness: &ManagerHarness, thread: ThreadId, cursor: &str) {
    harness
        .emit(AgentEvent::SessionConfigured {
            provider: AgentKind::Claude,
            resume_cursor: Some(cursor.to_owned()),
            model: None,
            models: Vec::new(),
            mode: PermissionMode::Ask,
            tools: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
        })
        .await;
    harness
        .settle(thread, "ready", |projection| {
            projection.session == SessionState::Ready
        })
        .await;
}

async fn wait_for_send(harness: &ManagerHarness, needle: &str) -> TurnId {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(turn) = harness.sent_turn_containing(needle) {
                return turn;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("provider did not receive {needle:?}"))
}

async fn wait_for_delegation(
    store: &SqliteAgentStore,
    id: DelegationId,
    predicate: impl Fn(&Delegation) -> bool,
) -> Delegation {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(delegation) = store
                .delegation(id)
                .await
                .expect("read restart-matrix delegation")
                && predicate(&delegation)
            {
                return delegation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delegation did not reach the expected recovery state")
}

async fn wait_for_empty_outbox(store: &SqliteAgentStore) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if store
                .delegation_outbox()
                .await
                .expect("read restart-matrix outbox")
                .is_empty()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("initial mirror did not drain");
}

async fn wait_for_running_turn(manager: &AgentSessionManager, thread: ThreadId, turn: TurnId) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if manager
                .projection(thread)
                .await
                .is_ok_and(|projection| projection.turn == TurnState::Running(turn))
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider turn did not start");
}

async fn seed_running_delegation(
    harness: &ManagerHarness,
    manager: &AgentSessionManager,
    store: &SqliteAgentStore,
    token: &str,
) -> (ThreadId, Delegation) {
    let caller = harness
        .create(Some("caller-cursor".to_owned()))
        .await
        .thread;
    configure(harness, caller, "caller-cursor").await;
    manager
        .send(
            caller,
            UserInput {
                text: "hold caller turn".to_owned(),
                ..UserInput::default()
            },
        )
        .await
        .expect("start scripted caller turn");
    let caller_turn = wait_for_send(harness, "hold caller turn").await;
    harness
        .emit(AgentEvent::TurnStarted {
            turn: caller_turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(caller, "a running caller", |projection| {
            projection.turn == TurnState::Running(caller_turn)
        })
        .await;

    let delegation_id = DelegationId::new();
    let child = harness
        .create_delegated(
            caller,
            delegation_id,
            Some("child-cursor".to_owned()),
            token,
        )
        .await
        .thread;
    let now = Utc::now();
    let delegation = Delegation {
        id: delegation_id,
        caller,
        caller_turn,
        caller_item: ItemId::new(),
        child,
        provider: AgentKind::Claude,
        depth: 1,
        brief: "hold delegated work across the restart".to_owned(),
        expectation: "report the recovered result".to_owned(),
        eager: true,
        status: DelegationStatus::Starting,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: now,
        finished: None,
        headline: None,
    };
    let stored = delegation.clone();
    let token_sha256 = format!("{:x}", Sha256::digest(token.as_bytes()));
    store
        .delegation_write("seed restart-matrix delegation", move |tx| {
            delegations::insert(tx, &stored, &token_sha256)?;
            Ok(((), false))
        })
        .await
        .expect("insert restart-matrix delegation");
    manager
        .append_item(
            caller,
            caller_turn,
            delegation.caller_item,
            ItemKind::Delegation {
                id: delegation.id,
                provider: delegation.provider,
                child,
                status: DelegationStatus::Starting,
            },
        )
        .await
        .expect("append restart-matrix caller item");
    configure(harness, child, "child-cursor").await;
    manager
        .send(
            child,
            UserInput {
                text: delegation.brief.clone(),
                ..UserInput::default()
            },
        )
        .await
        .expect("start delegated child turn");
    let child_turn = wait_for_send(harness, &delegation.brief).await;
    harness
        .emit(AgentEvent::TurnStarted {
            turn: child_turn,
            user_item: ItemId::new(),
        })
        .await;
    wait_for_delegation(store, delegation.id, |current| {
        current.status == DelegationStatus::Running
    })
    .await;
    (caller, delegation)
}

#[tokio::test]
async fn a_manager_restart_resumes_once_then_succeeds_and_delivers_once() {
    const TOKEN: &str = "restart-matrix-token";
    let harness = ManagerHarness::start(full()).await;
    let (manager, events, config, worktrees) = harness.delegation_parts();
    let store = manager.delegation_store().expect("agent store opens");
    let (_service, worker) = super::install(&manager, &events, &config, &worktrees)
        .expect("install initial delegation service");
    let (shutdown, task) = spawn_worker(worker);
    let (caller, delegation) = seed_running_delegation(&harness, &manager, &store, TOKEN).await;
    wait_for_empty_outbox(&store).await;
    stop_worker(shutdown, task).await;

    let restarted = harness.restart().await;
    let restarted_store = restarted.delegation_store().expect("restarted store opens");
    let (service, worker) = super::install(&restarted, &events, &config, &worktrees)
        .expect("install restarted delegation service");
    let (shutdown, task) = spawn_worker(worker);
    let recovered = wait_for_delegation(&restarted_store, delegation.id, |current| {
        current.recoveries == 1
    })
    .await;
    assert_eq!(recovered.status, DelegationStatus::Running);
    let resumed_turn = wait_for_send(&harness, super::footer::RESUME_NUDGE).await;
    harness
        .emit(AgentEvent::TurnStarted {
            turn: resumed_turn,
            user_item: ItemId::new(),
        })
        .await;
    wait_for_running_turn(&restarted, delegation.child, resumed_turn).await;
    let resumed_env = harness.started_env(delegation.child);
    let resumed_token = resumed_env
        .get("FLEET_DELEGATION_TOKEN")
        .cloned()
        .expect("resumed child receives a rotated token");
    assert_ne!(resumed_token, TOKEN);
    assert_eq!(
        resumed_env.get("FLEET_DELEGATION").map(String::as_str),
        Some(delegation.id.to_string().as_str())
    );
    stop_worker(shutdown, task).await;
    let stale = service
        .complete(CompleteRequest {
            delegation: delegation.id,
            child: delegation.child,
            token: TOKEN.to_owned(),
            result: "stale token result".to_owned(),
            blocked: false,
        })
        .await
        .expect_err("the pre-restart token is revoked");
    assert_eq!(stale.kind, fleet_proto::error::ErrorKind::Validation);
    service
        .complete(CompleteRequest {
            delegation: delegation.id,
            child: delegation.child,
            token: resumed_token,
            result: "recovered result".to_owned(),
            blocked: false,
        })
        .await
        .expect("report recovered result");
    harness
        .emit(AgentEvent::TurnSettled {
            turn: resumed_turn,
            outcome: TurnOutcome::Completed,
            usage: Usage::default(),
            duration_ms: 12,
            files_changed: Vec::new(),
        })
        .await;
    let delivery_marker = format!("[fleet subagent {} finished", delegation.id);
    for _ in 0..4 {
        drain(&service).await.expect("drain recovered result");
        if harness.sent_count_containing(&delivery_marker) == 1 {
            break;
        }
    }
    let delivered_turn = wait_for_send(&harness, &delivery_marker).await;
    harness
        .emit(AgentEvent::TurnStarted {
            turn: delivered_turn,
            user_item: ItemId::new(),
        })
        .await;
    let succeeded = wait_for_delegation(&restarted_store, delegation.id, |current| {
        current.status == DelegationStatus::Succeeded
            && matches!(current.delivery, DeliveryState::Delivered { .. })
    })
    .await;
    let (_service, worker) = super::install(&restarted, &events, &config, &worktrees)
        .expect("reinstall delegation worker after delivered item");
    let (shutdown, task) = spawn_worker(worker);
    wait_for_empty_outbox(&restarted_store).await;
    stop_worker(shutdown, task).await;

    assert_eq!(succeeded.recoveries, 1);
    assert_eq!(
        harness.sent_count_containing(super::footer::RESUME_NUDGE),
        1
    );
    assert_eq!(harness.sent_count_containing(&delivery_marker), 1);
    assert_eq!(
        restarted
            .projection(caller)
            .await
            .expect("read restarted caller")
            .items
            .into_iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    ItemKind::UserMessage {
                        origin: MessageOrigin::Delegation { id },
                        ..
                    } if id == delegation.id
                )
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn restart_delivers_to_an_idle_ready_caller_exactly_once() {
    let harness = ManagerHarness::start(full()).await;
    let (manager, events, config, worktrees) = harness.delegation_parts();
    let store = manager.delegation_store().expect("agent store opens");
    let (_service, worker) = super::install(&manager, &events, &config, &worktrees)
        .expect("install initial delegation service");
    drop(worker);
    let (caller, delegation) =
        seed_running_delegation(&harness, &manager, &store, "idle-caller-token").await;
    harness
        .emit_to(
            caller,
            AgentEvent::TurnSettled {
                turn: delegation.caller_turn,
                outcome: TurnOutcome::Completed,
                usage: Usage::default(),
                duration_ms: 8,
                files_changed: Vec::new(),
            },
        )
        .await;
    harness
        .settle(caller, "idle ready caller", |projection| {
            projection.session == SessionState::Ready
                && matches!(
                    projection.turn,
                    TurnState::Settled(_, TurnOutcome::Completed)
                )
        })
        .await;

    let terminal = Delegation {
        status: DelegationStatus::Succeeded,
        result: Some(DelegationResult {
            text: "result recovered for an idle caller".to_owned(),
            files_changed: Vec::new(),
            source: ResultSource::Reported,
            elided: false,
        }),
        finished: Some(Utc::now()),
        ..delegation.clone()
    };
    let stored = terminal.clone();
    store
        .delegation_write("stage idle-caller restart delivery", move |tx| {
            delegations::update(tx, &stored)?;
            tx.execute(
                "UPDATE delegation_outbox SET done = ?2 WHERE delegation = ?1 AND done IS NULL",
                rusqlite::params![stored.id.to_string(), Utc::now().timestamp_millis()],
            )?;
            delegations::enqueue(tx, stored.id, OutboxAction::Deliver, Utc::now())?;
            Ok(((), false))
        })
        .await
        .expect("stage terminal delivery");

    let restarted = harness.restart().await;
    assert_eq!(
        restarted
            .record(caller)
            .await
            .expect("read recovered caller")
            .stop_cause,
        Some(fleet_core::agents::StopCause::ProviderExit),
    );
    let restarted_store = restarted.delegation_store().expect("restarted store opens");
    let (service, worker) = super::install(&restarted, &events, &config, &worktrees)
        .expect("install restarted delegation service");
    drop(worker);
    for _ in 0..4 {
        drain(&service).await.expect("drain idle-caller delivery");
        if restarted_store
            .delegation(terminal.id)
            .await
            .expect("read delivery")
            .is_some_and(|current| matches!(current.delivery, DeliveryState::Delivered { .. }))
        {
            break;
        }
    }

    let marker = format!("[fleet subagent {} finished", terminal.id);
    assert_eq!(harness.sent_count_containing(&marker), 1);
    assert_eq!(
        restarted
            .projection(caller)
            .await
            .expect("read delivered caller")
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    ItemKind::UserMessage {
                        origin: MessageOrigin::Delegation { id },
                        ..
                    } if id == terminal.id
                )
            })
            .count(),
        1,
    );
}

#[tokio::test]
async fn a_second_provider_exit_fails_and_survives_the_next_restart() {
    let harness = ManagerHarness::start(full()).await;
    let (manager, events, config, worktrees) = harness.delegation_parts();
    let store = manager.delegation_store().expect("agent store opens");
    let (_service, worker) = super::install(&manager, &events, &config, &worktrees)
        .expect("install initial delegation service");
    let (shutdown, task) = spawn_worker(worker);
    let (caller, delegation) =
        seed_running_delegation(&harness, &manager, &store, "twice-token").await;
    wait_for_empty_outbox(&store).await;
    stop_worker(shutdown, task).await;

    let once = harness.restart().await;
    let once_store = once
        .delegation_store()
        .expect("first restarted store opens");
    let (_service, worker) = super::install(&once, &events, &config, &worktrees)
        .expect("install first restarted delegation service");
    let (shutdown, task) = spawn_worker(worker);
    wait_for_delegation(&once_store, delegation.id, |current| {
        current.recoveries == 1
    })
    .await;
    let resumed_turn = wait_for_send(&harness, super::footer::RESUME_NUDGE).await;
    harness
        .emit(AgentEvent::TurnStarted {
            turn: resumed_turn,
            user_item: ItemId::new(),
        })
        .await;
    wait_for_running_turn(&once, delegation.child, resumed_turn).await;
    stop_worker(shutdown, task).await;
    harness
        .emit(AgentEvent::TurnAborted {
            turn: resumed_turn,
            reason: AbortReason::ProviderExited,
        })
        .await;
    let failed = wait_for_delegation(&once_store, delegation.id, |current| {
        current.status == DelegationStatus::Failed
    })
    .await;

    let twice = harness.restart().await;
    let twice_store = twice
        .delegation_store()
        .expect("second restarted store opens");
    let (service, _worker) = super::install(&twice, &events, &config, &worktrees)
        .expect("install second restarted delegation service");
    drain(&service).await.expect("drain failed delivery");
    let delivery_marker = format!("[fleet subagent {} finished", delegation.id);
    let delivered_turn = wait_for_send(&harness, &delivery_marker).await;
    harness
        .emit(AgentEvent::TurnStarted {
            turn: delivered_turn,
            user_item: ItemId::new(),
        })
        .await;
    wait_for_delegation(&twice_store, delegation.id, |current| {
        matches!(current.delivery, DeliveryState::Delivered { .. })
    })
    .await;

    assert_eq!(failed.recoveries, 1);
    assert_eq!(
        failed.status_payload.as_deref(),
        Some("provider exited twice")
    );
    assert_eq!(harness.sent_count_containing(&delivery_marker), 1);
    assert_eq!(
        twice
            .projection(caller)
            .await
            .expect("read twice-restarted caller")
            .items
            .into_iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    ItemKind::UserMessage {
                        origin: MessageOrigin::Delegation { id },
                        ..
                    } if id == delegation.id
                )
            })
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn an_open_delivery_survives_restart_without_a_duplicate_caller_item() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, true, false).await;
    let delegation = delegations[0].clone();

    let mut restarted = harness.restart().await;
    let (shutdown, task) = start_worker(&mut restarted).await;
    tokio::time::resume();
    restarted
        .wait_for_delegation(delegation.id, |current| {
            matches!(current.delivery, DeliveryState::Delivered { .. })
        })
        .await;
    restarted.wait_for_delegation_origins(caller, 1).await;
    stop_worker(shutdown, task).await;
    tokio::time::pause();

    let mut restarted_again = restarted.restart().await;
    let (shutdown, task) = start_worker(&mut restarted_again).await;
    tokio::time::resume();
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        restarted_again
            .origins(caller)
            .await
            .into_iter()
            .filter(|origin| { *origin == MessageOrigin::Delegation { id: delegation.id } })
            .count(),
        1
    );
    assert!(
        restarted_again
            .store
            .delegation_outbox()
            .await
            .expect("read restarted outbox")
            .is_empty()
    );
    stop_worker(shutdown, task).await;
    tokio::time::pause();
}

#[tokio::test(start_paused = true)]
async fn startup_marks_a_terminal_result_with_a_deleted_caller_undeliverable() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;
    let delegation = delegations[0].clone();
    harness
        .store
        .delegation_write("delete recovery-test caller", move |tx| {
            tx.execute(
                "UPDATE threads SET deleted_at = ?2 WHERE thread_id = ?1",
                rusqlite::params![caller.to_string(), Utc::now().timestamp_millis()],
            )?;
            Ok(((), false))
        })
        .await
        .expect("delete recovery-test caller");

    drain(&harness.service)
        .await
        .expect("run startup missing-caller repair");
    let repaired = harness
        .store
        .delegation(delegation.id)
        .await
        .expect("read repaired delegation")
        .expect("delegation survives caller deletion");
    assert_eq!(repaired.result, delegation.result);
    assert_eq!(
        repaired.delivery,
        DeliveryState::Undeliverable {
            reason: "caller deleted".to_owned()
        }
    );
    assert!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read repaired outbox")
            .is_empty()
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_parent_cancels_its_descendants_before_the_parent() {
    let harness = Harness::start().await;
    let caller = running_caller(&harness).await;
    let parent = start_delegation(&harness, caller, false).await;
    let grandchild = start_delegation(&harness, parent.child, false).await;
    tokio::time::resume();
    let response = harness
        .service
        .cancel(parent.id)
        .await
        .expect("cancel the delegation tree");
    tokio::time::pause();
    let ResponseBody::Delegation(cancelled_parent) = response else {
        panic!("expected cancelled delegation response");
    };
    assert_eq!(cancelled_parent.status, DelegationStatus::Cancelled);

    let stored_grandchild = harness
        .store
        .delegation(grandchild.id)
        .await
        .expect("read cancelled grandchild")
        .expect("grandchild exists");
    assert_eq!(stored_grandchild.status, DelegationStatus::Cancelled);
    let rows = harness
        .store
        .delegation_outbox()
        .await
        .expect("read cancellation delivery rows");
    let grandchild_delivery = rows
        .iter()
        .position(|row| row.delegation == grandchild.id && row.action == OutboxAction::Deliver)
        .expect("grandchild delivery was recorded");
    let parent_delivery = rows
        .iter()
        .position(|row| row.delegation == parent.id && row.action == OutboxAction::Deliver)
        .expect("parent delivery was recorded");
    assert!(
        grandchild_delivery < parent_delivery,
        "the grandchild must be cancelled before its parent"
    );
    assert!(
        rows.iter()
            .any(|row| { row.delegation == grandchild.id && row.action == OutboxAction::Deliver })
    );
    assert!(
        rows.iter()
            .any(|row| row.delegation == parent.id && row.action == OutboxAction::Deliver)
    );
}

#[tokio::test]
async fn cancellation_stops_the_child_even_when_interrupt_times_out() {
    let harness = ManagerHarness::start(full()).await;
    let (manager, events, config, worktrees) = harness.delegation_parts();
    let store = manager.delegation_store().expect("agent store opens");
    let (service, _worker) =
        super::install(&manager, &events, &config, &worktrees).expect("install delegation service");
    let (_caller, delegation) =
        seed_running_delegation(&harness, &manager, &store, "cancel-token").await;
    harness.fail_interrupts();

    let response = service
        .cancel(delegation.id)
        .await
        .expect("stop makes cancellation succeed after interrupt timeout");
    let ResponseBody::Delegation(cancelled) = response else {
        panic!("expected cancelled delegation");
    };
    assert_eq!(cancelled.status, DelegationStatus::Cancelled);
}

#[tokio::test]
async fn cancellation_with_an_empty_provider_slot_supersedes_recovery() {
    let harness = ManagerHarness::start(full()).await;
    let (manager, events, config, worktrees) = harness.delegation_parts();
    let store = manager.delegation_store().expect("agent store opens");
    let (service, _worker) =
        super::install(&manager, &events, &config, &worktrees).expect("install delegation service");
    let (_caller, delegation) =
        seed_running_delegation(&harness, &manager, &store, "empty-slot-token").await;
    let id = delegation.id;
    store
        .delegation_write("seed recover before cancellation", move |tx| {
            delegations::enqueue(tx, id, OutboxAction::Recover, Utc::now())?;
            Ok(((), false))
        })
        .await
        .expect("seed recover row");
    harness.drop_provider(delegation.child).await;
    assert!(
        store
            .delegation_outbox()
            .await
            .expect("read recovered outbox")
            .iter()
            .any(|row| row.delegation == delegation.id && row.action == OutboxAction::Recover),
        "restart recovery queued the action cancellation must supersede"
    );

    let response = service
        .cancel(delegation.id)
        .await
        .expect("empty provider slot still cancels durably");
    let ResponseBody::Delegation(cancelled) = response else {
        panic!("expected cancelled delegation");
    };
    assert_eq!(cancelled.status, DelegationStatus::Cancelled);
    assert!(
        store
            .delegation_outbox()
            .await
            .expect("read cancellation outbox")
            .iter()
            .all(|row| row.delegation != delegation.id || row.action != OutboxAction::Recover),
        "terminal cancellation closes the stale recovery action"
    );
}
