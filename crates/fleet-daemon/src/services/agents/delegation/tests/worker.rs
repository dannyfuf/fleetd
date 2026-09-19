use std::{os::unix::fs::PermissionsExt as _, sync::Arc};

use chrono::Utc;
use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationId, DelegationResult, DelegationStatus, DeliveryState,
        ItemId, ItemKind, MessageOrigin, PermissionMode, ResultSource, SessionState, StopCause,
        ThreadId, ThreadProjection, TurnId, TurnState, UserInput,
    },
    ids::{ContextId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    paths::FleetHome,
    state::default_state,
};
use fleet_proto::response::ResponseBody;
use tokio_util::sync::CancellationToken;

use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::BroadcastBus,
    services::{
        agents::{
            AgentSessionManager,
            store::{OutboxAction, SqliteAgentStore, delegations},
        },
        sessions::Sessions,
        worktrees::Worktrees,
    },
    stores::{config::ConfigStore, state::StateStore},
};

use super::super::{
    DelegationService, DelegationWorker,
    footer::{NUDGE, RESUME_NUDGE},
    limits::{RETRY_TICK, SETTLE_GRACE},
    worker::drain,
};

pub(crate) struct Harness {
    _directory: tempfile::TempDir,
    log: std::path::PathBuf,
    config: Arc<ConfigStore>,
    pub(crate) manager: AgentSessionManager,
    events: BroadcastBus,
    pub(crate) store: SqliteAgentStore,
    pub(crate) service: DelegationService,
    worker: Option<DelegationWorker>,
    worktree: WorktreeId,
    worktrees: Worktrees,
}

impl Harness {
    pub(crate) async fn start() -> Self {
        let directory = tempfile::tempdir().expect("create worker test directory");
        let home = directory.path().join("fleet");
        let repos = home.join("repos");
        let worktree_path = home.join("worktrees/owner/repo/feature");
        std::fs::create_dir_all(&repos).expect("create repositories directory");
        std::fs::create_dir_all(&worktree_path).expect("create worktree directory");

        let log = home.join("provider-input.ndjson");
        let executable = home.join("fake-claude");
        std::fs::write(&executable, provider_script(&log)).expect("write fake Claude executable");
        let mut permissions = std::fs::metadata(&executable)
            .expect("read fake Claude metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).expect("make fake Claude executable");

        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [repos.clone(), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        let mut effective = config.load().await.expect("load test config");
        effective.agent_binaries.claude = executable.display().to_string();
        config.save(effective).await.expect("save test config");

        let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
        let context = ContextId::try_from("team").expect("context id");
        let repo = RepoId::try_from("owner/repo").expect("repo id");
        let worktree = WorktreeId::try_from("owner/repo#feature").expect("worktree id");
        let mut persisted = default_state();
        persisted.contexts.push(Context {
            id: context.clone(),
            name: "Team".to_owned(),
            owners: Vec::new(),
            created_at: Utc::now().to_rfc3339(),
        });
        persisted.repos.push(Repo {
            id: repo.clone(),
            owner: "owner".to_owned(),
            name: "repo".to_owned(),
            url: "https://example.invalid/owner/repo".to_owned(),
            context_id: context,
            default_branch: "main".to_owned(),
            path: repos.join("owner/repo").display().to_string(),
            cloned_at: Utc::now().to_rfc3339(),
            hooks: RepoHooks::default(),
        });
        persisted.worktrees.push(Worktree {
            id: worktree.clone(),
            repo_id: repo,
            slug: "feature".to_owned(),
            branch: "feature".to_owned(),
            base_ref: "main".to_owned(),
            path: worktree_path.display().to_string(),
            session: "owner/repo/feature".to_owned(),
            host: None,
            created_at: Utc::now().to_rfc3339(),
            last_opened_at: None,
            degraded: None,
        });
        state.save(persisted).await.expect("seed test state");
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
        let worktrees = Worktrees::new(
            Arc::clone(&config),
            state,
            Arc::new(JobManager::new(&home)),
            &Adapters::system(files),
            sessions,
        );
        let events = BroadcastBus::new(256);
        let database = FleetHome::new(&home).agents_db_path();
        let manager = AgentSessionManager::new(
            database.clone(),
            events.clone(),
            worktrees.clone(),
            Arc::clone(&config),
        );
        let (service, worker) = super::super::install(&manager, &events, &config, &worktrees)
            .expect("the worker test database opens");
        let store = manager
            .delegation_store()
            .expect("the worker test database remains available");
        Self {
            _directory: directory,
            log,
            config,
            manager,
            events,
            store,
            service,
            worker: Some(worker),
            worktree,
            worktrees,
        }
    }

    pub(crate) async fn create_thread(&self) -> ThreadId {
        // Process startup is OS work, so a paused runtime would auto-advance the provider probe's
        // timeout before the executable gets scheduled. Worker behaviour is tested paused; only
        // this external-process fixture setup runs against real time.
        tokio::time::resume();
        let response = self
            .manager
            .create(
                self.worktree.clone(),
                AgentKind::Claude,
                None,
                Some(PermissionMode::Ask),
                None,
                None,
            )
            .await
            .expect("create test agent");
        let thread = match response {
            ResponseBody::AgentThreadCreated(summary) => summary.thread,
            other => panic!("expected AgentThreadCreated, got {other:?}"),
        };
        self.wait_for(thread, |projection| {
            projection.session == SessionState::Ready
        })
        .await;
        tokio::time::pause();
        thread
    }

    pub(crate) async fn send(&self, thread: ThreadId, text: &str) {
        self.manager
            .send(
                thread,
                UserInput {
                    text: text.to_owned(),
                    ..UserInput::default()
                },
            )
            .await
            .expect("send scripted input");
    }

    pub(crate) async fn wait_for(
        &self,
        thread: ThreadId,
        predicate: impl Fn(&ThreadProjection) -> bool,
    ) -> ThreadProjection {
        // Subscribe before reading so an event committed between the read and the wait remains
        // queued. Provider-backed tests then wait on actual progress instead of a scheduler-
        // sensitive number of yields.
        let mut events = self.events.subscribe();
        loop {
            if let Ok(projection) = self.manager.projection(thread).await
                && predicate(&projection)
            {
                return projection;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed while waiting for thread {thread}")
                }
            }
        }
    }

    pub(crate) async fn wait_for_stop_cause(&self, thread: ThreadId, cause: StopCause) {
        let mut events = self.events.subscribe();
        loop {
            if self
                .manager
                .record(thread)
                .await
                .is_ok_and(|record| record.stop_cause == Some(cause))
            {
                return;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed while waiting for {cause:?} on thread {thread}")
                }
            }
        }
    }

    pub(crate) async fn wait_for_delegation(
        &self,
        id: DelegationId,
        predicate: impl Fn(&Delegation) -> bool,
    ) -> Delegation {
        let mut events = self.events.subscribe();
        loop {
            if let Some(delegation) = self
                .store
                .delegation(id)
                .await
                .expect("read recovery-test delegation")
                && predicate(&delegation)
            {
                return delegation;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed while waiting for delegation {id}")
                }
            }
        }
    }

    /// Waits until the worker has closed every open outbox row.
    ///
    /// Subscribe before the first read so a row closed between the read and the wait still wakes
    /// this loop: every path that closes a row publishes the changed delegation on the same bus.
    /// A counted number of yields would instead pass or fail on how the whole-crate run happened
    /// to schedule the worker task.
    pub(crate) async fn wait_for_empty_outbox(&self) {
        let mut events = self.events.subscribe();
        loop {
            if self
                .store
                .delegation_outbox()
                .await
                .expect("read the delegation outbox")
                .is_empty()
            {
                return;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed while waiting for the delegation outbox to drain")
                }
            }
        }
    }

    pub(crate) async fn caller_with_delegations(
        &self,
        count: usize,
        eager: bool,
        settle_caller: bool,
    ) -> (ThreadId, Vec<Delegation>) {
        let caller = self.create_thread().await;
        tokio::time::resume();
        self.send(caller, "hold setup turn").await;
        let running = self
            .wait_for(caller, |projection| {
                matches!(projection.turn, TurnState::Running(_))
            })
            .await;
        let turn = match running.turn {
            TurnState::Running(turn) => turn,
            other => panic!("expected a running turn, got {other:?}"),
        };
        let mut delegations = Vec::with_capacity(count);
        for index in 0..count {
            let id = DelegationId::new();
            let child = ThreadId::new();
            let item = ItemId::new();
            self.manager
                .append_item(
                    caller,
                    turn,
                    item,
                    ItemKind::Delegation {
                        id,
                        provider: AgentKind::Claude,
                        child,
                        status: DelegationStatus::Succeeded,
                    },
                )
                .await
                .expect("append caller delegation item");
            delegations.push(finished_delegation(
                id, caller, turn, item, child, eager, index,
            ));
        }
        if settle_caller {
            self.send(caller, "finish setup turn").await;
            self.wait_for(caller, |projection| {
                matches!(projection.turn, TurnState::Settled(_, _))
            })
            .await;
        }
        tokio::time::pause();
        for delegation in &delegations {
            self.insert(delegation.clone(), OutboxAction::Deliver).await;
        }
        (caller, delegations)
    }

    pub(crate) async fn insert(&self, delegation: Delegation, action: OutboxAction) {
        let stored = delegation.clone();
        self.store
            .delegation_write("insert worker-test delegation", move |tx| {
                delegations::insert(
                    tx,
                    &stored,
                    "test-token",
                    &std::collections::BTreeMap::new(),
                )?;
                delegations::enqueue(tx, stored.id, action, Utc::now())?;
                Ok(((), false))
            })
            .await
            .expect("insert worker-test delegation");
    }

    pub(crate) async fn enqueue(&self, delegation: DelegationId, action: OutboxAction) {
        self.store
            .delegation_write("enqueue worker-test action", move |tx| {
                delegations::enqueue(tx, delegation, action, Utc::now())?;
                Ok(((), false))
            })
            .await
            .expect("enqueue worker-test action");
    }

    pub(crate) async fn origins(&self, caller: ThreadId) -> Vec<MessageOrigin> {
        self.manager
            .projection(caller)
            .await
            .expect("read caller projection")
            .items
            .into_iter()
            .filter_map(|item| match item.kind {
                ItemKind::UserMessage { origin, .. } => Some(origin),
                _ => None,
            })
            .collect()
    }

    pub(crate) async fn wait_for_delegation_origins(&self, caller: ThreadId, count: usize) {
        self.wait_for(caller, |projection| {
            projection
                .items
                .iter()
                .filter(|item| {
                    matches!(
                        item.kind,
                        ItemKind::UserMessage {
                            origin: MessageOrigin::Delegation { .. },
                            ..
                        }
                    )
                })
                .count()
                >= count
        })
        .await;
    }

    pub(crate) fn take_worker(&mut self) -> DelegationWorker {
        self.worker.take().expect("delegation worker is available")
    }

    pub(crate) async fn restart(self) -> Self {
        let Self {
            _directory,
            log,
            config,
            manager,
            events,
            store,
            service,
            worker,
            worktree,
            worktrees,
        } = self;
        drop(worker);
        drop(service);
        tokio::task::yield_now().await;
        let (service, worker) = super::super::install(&manager, &events, &config, &worktrees)
            .expect("the restarted worker test database opens");
        Self {
            _directory,
            log,
            config,
            manager,
            events,
            store,
            service,
            worker: Some(worker),
            worktree,
            worktrees,
        }
    }
}

fn provider_script(log: &std::path::Path) -> String {
    format!(
        r##"#!/bin/sh
case " $* " in
  *" --version "*) printf '%s\n' '2.1.266 (Claude Code)'; exit 0 ;;
esac
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"worker-cursor","model":"test","tools":[],"slash_commands":[],"capabilities":["interrupt_receipt_v1","interrupt_cancel_queued_v1","msg_lifecycle_v1"]}}'
printf 'ENV|%s|%s|%s\n' "$FLEET_SESSION" "$FLEET_DELEGATION" "$FLEET_DELEGATION_TOKEN" >> '{}'
count=0
while IFS= read -r line; do
  count=$((count + 1))
  printf '%s\n' "$line" >> '{}'
  case "$line" in
    *exit-now*) exit 17 ;;
    *clear-background*)
      printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[]}}'
      ;;
    *background*)
      printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"bg-1","task_type":"explore","description":"still running"}}]}}'
      ;;
  esac
  printf '%s\n' "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-$count\",\"content\":[{{\"type\":\"text\",\"text\":\"scripted answer\"}}]}},\"parent_tool_use_id\":null}}"
  case "$line" in
    *hold*|*"The session was restarted."*) ;;
    *) printf '%s\n' '{{"type":"result","subtype":"success","terminal_reason":"completed","usage":{{}}}}' ;;
  esac
done
"##,
        log.display(),
        log.display()
    )
}

fn finished_delegation(
    id: DelegationId,
    caller: ThreadId,
    caller_turn: TurnId,
    caller_item: fleet_core::agents::ItemId,
    child: ThreadId,
    eager: bool,
    index: usize,
) -> Delegation {
    let now = Utc::now();
    Delegation {
        id,
        caller,
        caller_turn,
        caller_item,
        child,
        provider: AgentKind::Claude,
        depth: 1,
        brief: format!("delegated task {index}"),
        expectation: "report the result".to_owned(),
        eager,
        status: DelegationStatus::Succeeded,
        status_payload: None,
        result: Some(DelegationResult {
            text: format!("result {index}"),
            files_changed: Vec::new(),
            source: ResultSource::Reported,
            elided: false,
        }),
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: now,
        finished: Some(now),
        headline: None,
        usage: None,
    }
}

#[tokio::test(start_paused = true)]
async fn missing_child_reservation_does_not_abort_the_outbox_drain() {
    let harness = Harness::start().await;
    let (_caller, valid) = harness.caller_with_delegations(1, true, false).await;
    let mut missing = finished_delegation(
        DelegationId::new(),
        ThreadId::new(),
        TurnId::new(),
        ItemId::new(),
        ThreadId::new(),
        false,
        9,
    );
    missing.status = DelegationStatus::Starting;
    missing.result = None;
    missing.finished = None;
    harness.insert(missing.clone(), OutboxAction::Recover).await;

    tokio::time::resume();
    drain(&harness.service)
        .await
        .expect("missing child is repaired without aborting the pass");
    tokio::time::pause();

    assert!(
        harness
            .store
            .delegation(missing.id)
            .await
            .expect("read repaired reservation")
            .is_none()
    );
    assert!(matches!(
        harness
            .store
            .delegation(valid[0].id)
            .await
            .expect("read valid delegation")
            .expect("valid delegation remains")
            .delivery,
        DeliveryState::Delivered { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn settle_waits_for_the_background_task_then_finalizes() {
    let harness = Harness::start().await;
    let child = harness.create_thread().await;
    // The scripted provider is an OS process. Keep Tokio's paused clock from auto-advancing its
    // protocol deadlines while the process and pipe reader need real scheduler time.
    tokio::time::resume();
    harness.send(child, "background work").await;
    harness
        .wait_for(child, |projection| {
            matches!(projection.turn, TurnState::Settled(_, _))
                && !projection.background_tasks.is_empty()
        })
        .await;
    tokio::time::pause();
    let mut delegation = finished_delegation(
        DelegationId::new(),
        ThreadId::new(),
        TurnId::new(),
        fleet_core::agents::ItemId::new(),
        child,
        false,
        0,
    );
    delegation.status = DelegationStatus::Settling;
    delegation.finished = None;
    harness
        .insert(delegation.clone(), OutboxAction::Settle)
        .await;

    drain(&harness.service)
        .await
        .expect("drain live background settle");
    assert_eq!(
        harness
            .store
            .delegation(delegation.id)
            .await
            .expect("read delegation")
            .expect("delegation exists")
            .status,
        DelegationStatus::Settling
    );

    tokio::time::resume();
    harness.send(child, "clear-background").await;
    harness
        .wait_for(child, |projection| projection.background_tasks.is_empty())
        .await;
    tokio::time::pause();
    drain(&harness.service)
        .await
        .expect("drain completed settle");
    assert_eq!(
        harness
            .store
            .delegation(delegation.id)
            .await
            .expect("read delegation")
            .expect("delegation exists")
            .status,
        DelegationStatus::Succeeded
    );
}

#[tokio::test(start_paused = true)]
async fn settle_grace_is_measured_from_delegation_creation() {
    let harness = Harness::start().await;
    let child = harness.create_thread().await;
    // The provider is an OS process, so let it run on real scheduler time while it creates the
    // deliberately live background item.
    tokio::time::resume();
    harness.send(child, "background work").await;
    harness
        .wait_for(child, |projection| {
            matches!(projection.turn, TurnState::Settled(_, _))
                && !projection.background_tasks.is_empty()
        })
        .await;
    tokio::time::pause();

    let mut delegation = finished_delegation(
        DelegationId::new(),
        ThreadId::new(),
        TurnId::new(),
        fleet_core::agents::ItemId::new(),
        child,
        false,
        0,
    );
    delegation.status = DelegationStatus::Settling;
    delegation.finished = None;
    delegation.created = Utc::now()
        - chrono::Duration::from_std(SETTLE_GRACE).expect("settle grace converts to chrono");
    harness
        .insert(delegation.clone(), OutboxAction::Settle)
        .await;

    drain(&harness.service)
        .await
        .expect("drain expired background settle");
    assert_eq!(
        harness
            .store
            .delegation(delegation.id)
            .await
            .expect("read delegation")
            .expect("delegation exists")
            .status,
        DelegationStatus::Succeeded
    );
}

#[tokio::test(start_paused = true)]
async fn retry_tick_sends_and_counts_the_nudge() {
    let mut harness = Harness::start().await;
    let child = harness.create_thread().await;
    let mut delegation = finished_delegation(
        DelegationId::new(),
        ThreadId::new(),
        TurnId::new(),
        fleet_core::agents::ItemId::new(),
        child,
        false,
        0,
    );
    delegation.status = DelegationStatus::Settling;
    delegation.finished = None;
    delegation.result = None;

    harness
        .insert(delegation.clone(), OutboxAction::Recover)
        .await;
    let worker = harness.worker.take().expect("worker is available");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(worker.run(shutdown.clone()));
    // The worker's startup pass owns the Recover row, and resuming the child is an OS process, so
    // let both run on real scheduler time and wait on the row actually closing. A counted number
    // of yields instead passed or failed on how the whole-crate run happened to schedule them.
    tokio::time::resume();
    harness.wait_for_empty_outbox().await;
    harness
        .wait_for(child, |projection| {
            projection.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemKind::UserMessage { text, .. } if text == RESUME_NUDGE
                )
            })
        })
        .await;
    tokio::time::pause();
    harness.enqueue(delegation.id, OutboxAction::Nudge).await;
    tokio::time::advance(RETRY_TICK).await;
    let stored = harness
        .wait_for_delegation(delegation.id, |current| current.nudges == 1)
        .await;
    assert_eq!(stored.nudges, 1);
    tokio::time::resume();
    harness
        .wait_for(child, |projection| {
            projection.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemKind::UserMessage { text, .. } if text == NUDGE
                )
            })
        })
        .await;
    tokio::time::pause();

    let stored = harness
        .store
        .delegation(delegation.id)
        .await
        .expect("read delegation")
        .expect("delegation exists");
    assert_eq!(stored.nudges, 1);
    shutdown.cancel();
    task.await.expect("worker stops cleanly");
}

#[tokio::test(start_paused = true)]
async fn a_failed_nudge_ack_reconciles_the_stable_item_without_resending() {
    let harness = Harness::start().await;
    let child = harness.create_thread().await;
    let mut delegation = finished_delegation(
        DelegationId::new(),
        ThreadId::new(),
        TurnId::new(),
        ItemId::new(),
        child,
        false,
        0,
    );
    delegation.status = DelegationStatus::Settling;
    delegation.finished = None;
    delegation.result = None;
    harness
        .insert(delegation.clone(), OutboxAction::Nudge)
        .await;
    let nudge_row = harness
        .store
        .delegation_outbox()
        .await
        .expect("read nudge row")
        .into_iter()
        .find(|row| row.delegation == delegation.id && row.action == OutboxAction::Nudge)
        .expect("open nudge row")
        .id;
    harness
        .store
        .delegation_write("install nudge acknowledgement failure", |tx| {
            tx.execute_batch(
                "CREATE TRIGGER fail_nudge_ack BEFORE UPDATE OF nudges ON delegations \
                 BEGIN SELECT RAISE(FAIL, 'forced nudge acknowledgement failure'); END;",
            )?;
            Ok(((), false))
        })
        .await
        .expect("install acknowledgement failure");

    tokio::time::resume();
    drain(&harness.service).await.expect("first nudge drain");
    harness
        .wait_for(child, |projection| {
            projection.items.iter().any(
                |item| matches!(&item.kind, ItemKind::UserMessage { text, .. } if text == NUDGE),
            )
        })
        .await;
    tokio::time::pause();
    harness
        .store
        .delegation_write("remove nudge acknowledgement failure", |tx| {
            tx.execute_batch("DROP TRIGGER fail_nudge_ack")?;
            Ok(((), false))
        })
        .await
        .expect("remove acknowledgement failure");

    drain(&harness.service)
        .await
        .expect("reconcile committed nudge");
    let stored = harness
        .store
        .delegation(delegation.id)
        .await
        .expect("read delegation")
        .expect("delegation exists");
    assert_eq!(stored.nudges, 1);
    assert!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read reconciled outbox")
            .iter()
            .all(|row| row.id != nudge_row)
    );
    assert_eq!(
        std::fs::read_to_string(&harness.log)
            .expect("read provider input")
            .matches(NUDGE)
            .count(),
        1,
    );
}

#[tokio::test(start_paused = true)]
async fn a_reopened_delivery_row_reconciles_its_committed_item_without_resending() {
    let harness = Harness::start().await;
    let (_caller, delegations) = harness.caller_with_delegations(1, true, false).await;
    let delegation = delegations[0].clone();
    let row = harness
        .store
        .delegation_outbox()
        .await
        .expect("read delivery row")
        .into_iter()
        .find(|row| row.delegation == delegation.id && row.action == OutboxAction::Deliver)
        .expect("open delivery row");

    tokio::time::resume();
    drain(&harness.service).await.expect("submit delivery");
    harness
        .wait_for_delegation(delegation.id, |current| {
            matches!(current.delivery, DeliveryState::Delivered { .. })
        })
        .await;
    tokio::time::pause();
    let row_id = row.id;
    let submitted = harness
        .store
        .delegation_write("read delivery submission marker", move |tx| {
            let submitted = tx.query_row(
                "SELECT submitted FROM delegation_outbox WHERE id = ?1",
                [row_id],
                |record| record.get::<_, Option<String>>(0),
            )?;
            Ok((submitted, false))
        })
        .await
        .expect("read submitted delivery row");
    assert!(
        submitted.is_some(),
        "the provider call must have a durable pre-send marker"
    );
    harness
        .store
        .delegation_write("reopen committed delivery row", move |tx| {
            tx.execute(
                "UPDATE delegation_outbox SET done = NULL WHERE id = ?1",
                [row.id],
            )?;
            Ok(((), false))
        })
        .await
        .expect("reopen delivery row");

    drain(&harness.service)
        .await
        .expect("reconcile committed delivery");
    assert!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read reconciled delivery outbox")
            .iter()
            .all(|open| open.id != row.id)
    );
    let marker = format!("[fleet subagent {} finished", delegation.id);
    assert_eq!(
        std::fs::read_to_string(&harness.log)
            .expect("read provider input")
            .matches(&marker)
            .count(),
        1,
    );
}

#[tokio::test(start_paused = true)]
async fn deferred_delivery_does_not_starve_cancellation_for_the_same_caller() {
    let harness = Harness::start().await;
    let (_caller, delegations) = harness.caller_with_delegations(1, false, false).await;
    let delegation = &delegations[0];
    harness
        .enqueue(delegation.id, OutboxAction::CancelChildren)
        .await;

    drain(&harness.service)
        .await
        .expect("drain cancellation behind deferred delivery");

    let rows = harness
        .store
        .delegation_outbox()
        .await
        .expect("read remaining outbox");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, OutboxAction::Deliver);
}
