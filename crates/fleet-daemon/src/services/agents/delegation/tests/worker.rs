use std::{os::unix::fs::PermissionsExt as _, sync::Arc};

use chrono::Utc;
use fleet_core::{
    agents::{
        AgentKind, Delegation, DelegationId, DelegationResult, DelegationStatus, DeliveryState,
        ItemKind, MessageOrigin, PermissionMode, ResultSource, SessionState, StopCause, ThreadId,
        ThreadProjection, TurnId, TurnState, UserInput,
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
        agents::{AgentSessionManager, store::SqliteAgentStore},
        sessions::Sessions,
        worktrees::Worktrees,
    },
    stores::{config::ConfigStore, state::StateStore},
};

use super::super::{
    DelegationService, DelegationWorker, footer::NUDGE, limits::RETRY_TICK, worker::drain,
};

struct Harness {
    _directory: tempfile::TempDir,
    log: std::path::PathBuf,
    cursor_marker: std::path::PathBuf,
    manager: AgentSessionManager,
    store: SqliteAgentStore,
    service: DelegationService,
    worker: Option<DelegationWorker>,
    worktree: WorktreeId,
}

impl Harness {
    async fn start() -> Self {
        let directory = tempfile::tempdir().expect("create worker test directory");
        let home = directory.path().join("fleet");
        let repos = home.join("repos");
        let worktree_path = home.join("worktrees/owner/repo/feature");
        std::fs::create_dir_all(&repos).expect("create repositories directory");
        std::fs::create_dir_all(&worktree_path).expect("create worktree directory");

        let log = home.join("provider-input.ndjson");
        let cursor_marker = home.join("omit-resume-cursor");
        let executable = home.join("fake-claude");
        std::fs::write(&executable, provider_script(&log, &cursor_marker))
            .expect("write fake Claude executable");
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
        let manager = AgentSessionManager::new(
            FleetHome::new(&home).agents_db_path(),
            events.clone(),
            worktrees.clone(),
            Arc::clone(&config),
        );
        let store = manager
            .delegation_store()
            .expect("the worker test database opens");
        let (service, worker) = DelegationService::new(
            store.clone(),
            manager.clone(),
            events.clone(),
            Arc::clone(&config),
            worktrees.clone(),
        );
        Self {
            _directory: directory,
            log,
            cursor_marker,
            manager,
            store,
            service,
            worker: Some(worker),
            worktree,
        }
    }

    async fn create_thread(&self) -> ThreadId {
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
                PermissionMode::Ask,
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

    async fn send(&self, thread: ThreadId, text: &str) {
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

    async fn wait_for(
        &self,
        thread: ThreadId,
        predicate: impl Fn(&ThreadProjection) -> bool,
    ) -> ThreadProjection {
        for _ in 0..2_000 {
            if let Ok(projection) = self.manager.projection(thread).await
                && predicate(&projection)
            {
                return projection;
            }
            tokio::task::yield_now().await;
        }
        panic!("thread {thread} did not reach the expected state")
    }

    async fn wait_for_stop_cause(&self, thread: ThreadId, cause: StopCause) {
        for _ in 0..2_000 {
            if self
                .manager
                .record(thread)
                .await
                .is_ok_and(|record| record.stop_cause == Some(cause))
            {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("thread {thread} did not record {cause:?}")
    }

    async fn caller_with_delegations(
        &self,
        count: usize,
        eager: bool,
        settle_caller: bool,
    ) -> (ThreadId, Vec<Delegation>) {
        let caller = self.create_thread().await;
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
            let item = self
                .manager
                .append_item(
                    caller,
                    turn,
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
        for delegation in &delegations {
            self.insert(delegation.clone(), "deliver").await;
        }
        (caller, delegations)
    }

    async fn insert(&self, delegation: Delegation, action: &'static str) {
        let stored = delegation.clone();
        self.store
            .delegation_write("insert worker-test delegation", move |tx| {
                let result = stored.result.as_ref().expect("test delegation result");
                tx.execute(
                    "INSERT INTO delegations (id, token_sha256, caller_thread, caller_turn, \
                     caller_item, child_thread, provider, depth, brief, expectation, eager, \
                     status, result, result_source, result_files, result_elided, nudges, \
                     recoveries, delivery, created, finished) VALUES (?1, 'test-token', ?2, ?3, \
                     ?4, ?5, 'claude', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                     'pending', ?17, ?18)",
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
                        result.text,
                        "reported",
                        serde_json::to_string(&result.files_changed)?,
                        i64::from(result.elided),
                        i64::from(stored.nudges),
                        i64::from(stored.recoveries),
                        stored.created.to_rfc3339(),
                        stored.finished.map(|stamp| stamp.to_rfc3339()),
                    ],
                )?;
                tx.execute(
                    "INSERT INTO delegation_outbox (delegation, action, created) \
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![stored.id.to_string(), action, Utc::now().to_rfc3339()],
                )?;
                Ok(((), false))
            })
            .await
            .expect("insert worker-test delegation");
    }

    async fn enqueue(&self, delegation: DelegationId, action: &'static str) {
        self.store
            .delegation_write("enqueue worker-test action", move |tx| {
                tx.execute(
                    "INSERT INTO delegation_outbox (delegation, action, created) \
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![delegation.to_string(), action, Utc::now().to_rfc3339()],
                )?;
                Ok(((), false))
            })
            .await
            .expect("enqueue worker-test action");
    }

    async fn origins(&self, caller: ThreadId) -> Vec<MessageOrigin> {
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

    async fn wait_for_delegation_origins(&self, caller: ThreadId, count: usize) {
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

    fn provider_log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn omit_resume_cursor(&self) {
        std::fs::write(&self.cursor_marker, b"").expect("write no-cursor marker");
    }
}

fn provider_script(log: &std::path::Path, cursor_marker: &std::path::Path) -> String {
    format!(
        r##"#!/bin/sh
case " $* " in
  *" --version "*) printf '%s\n' '2.1.266 (Claude Code)'; exit 0 ;;
esac
if [ -f '{}' ]; then
  printf '%s\n' '{{"type":"system","subtype":"init","model":"test","tools":[],"slash_commands":[],"capabilities":["interrupt_receipt_v1","interrupt_cancel_queued_v1","msg_lifecycle_v1"]}}'
else
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"worker-cursor","model":"test","tools":[],"slash_commands":[],"capabilities":["interrupt_receipt_v1","interrupt_cancel_queued_v1","msg_lifecycle_v1"]}}'
fi
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
    *hold*) ;;
    *) printf '%s\n' '{{"type":"result","subtype":"success","terminal_reason":"completed","usage":{{}}}}' ;;
  esac
done
"##,
        cursor_marker.display(),
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

#[tokio::test(start_paused = true)]
async fn idle_delivery_starts_a_caller_turn_with_delegation_origin() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;

    drain(&harness.service).await.expect("drain delivery");
    harness.wait_for_delegation_origins(caller, 1).await;

    assert!(
        harness
            .origins(caller)
            .await
            .contains(&MessageOrigin::Delegation {
                id: delegations[0].id
            })
    );
}

#[tokio::test(start_paused = true)]
async fn running_then_idle_waits_for_the_callers_settle() {
    let harness = Harness::start().await;
    let (caller, _) = harness.caller_with_delegations(1, false, false).await;

    drain(&harness.service).await.expect("drain while running");
    assert!(
        harness
            .origins(caller)
            .await
            .iter()
            .all(MessageOrigin::is_user)
    );

    harness.send(caller, "finish setup turn").await;
    harness
        .wait_for(caller, |projection| {
            matches!(projection.turn, TurnState::Settled(_, _))
        })
        .await;
    drain(&harness.service).await.expect("drain after settle");
    harness.wait_for_delegation_origins(caller, 1).await;
}

#[tokio::test(start_paused = true)]
async fn eager_delivery_steers_the_running_turn() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, true, false).await;

    drain(&harness.service).await.expect("drain eager delivery");
    harness.wait_for_delegation_origins(caller, 1).await;

    assert!(
        harness
            .origins(caller)
            .await
            .contains(&MessageOrigin::Delegation {
                id: delegations[0].id
            })
    );
}

#[tokio::test(start_paused = true)]
async fn provider_exit_delivery_resumes_and_sends() {
    let harness = Harness::start().await;
    let (caller, _) = harness.caller_with_delegations(1, false, true).await;
    tokio::time::resume();
    harness.send(caller, "exit-now").await;
    harness
        .wait_for(caller, |projection| {
            projection.session == SessionState::Error
        })
        .await;
    harness
        .wait_for_stop_cause(caller, StopCause::ProviderExit)
        .await;

    drain(&harness.service)
        .await
        .expect("drain resume delivery");
    harness.wait_for_delegation_origins(caller, 1).await;
    tokio::time::pause();
}

#[tokio::test(start_paused = true)]
async fn user_stop_holds_the_delivery() {
    let harness = Harness::start().await;
    let (caller, _) = harness.caller_with_delegations(1, false, true).await;
    tokio::time::resume();
    harness.manager.stop(caller).await.expect("stop caller");
    harness
        .wait_for(caller, |projection| {
            projection.session == SessionState::Stopped
        })
        .await;
    harness.wait_for_stop_cause(caller, StopCause::User).await;
    tokio::time::pause();

    drain(&harness.service).await.expect("drain stopped caller");

    assert!(
        harness
            .origins(caller)
            .await
            .iter()
            .all(MessageOrigin::is_user)
    );
    assert_eq!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("open rows")
            .len(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn stopped_caller_without_a_cursor_becomes_undeliverable() {
    let harness = Harness::start().await;
    harness.omit_resume_cursor();
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;
    tokio::time::resume();
    harness.send(caller, "exit-now").await;
    harness
        .wait_for(caller, |projection| {
            projection.session == SessionState::Error
        })
        .await;
    harness
        .wait_for_stop_cause(caller, StopCause::ProviderExit)
        .await;
    tokio::time::pause();

    drain(&harness.service)
        .await
        .expect("drain no-cursor delivery");

    let stored = harness
        .store
        .delegation(delegations[0].id)
        .await
        .expect("read delegation")
        .expect("delegation exists");
    assert!(matches!(
        stored.delivery,
        DeliveryState::Undeliverable { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn two_children_of_one_caller_deliver_as_two_turns() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(2, false, true).await;

    drain(&harness.service).await.expect("drain first child");
    harness.wait_for_delegation_origins(caller, 1).await;
    harness
        .wait_for(caller, |projection| {
            matches!(projection.turn, TurnState::Settled(_, _))
        })
        .await;
    drain(&harness.service).await.expect("drain second child");
    harness.wait_for_delegation_origins(caller, 2).await;

    let origins = harness.origins(caller).await;
    for delegation in delegations {
        assert!(origins.contains(&MessageOrigin::Delegation { id: delegation.id }));
    }
}

#[tokio::test(start_paused = true)]
async fn settle_waits_for_the_background_task_then_finalizes() {
    let harness = Harness::start().await;
    let child = harness.create_thread().await;
    harness.send(child, "background work").await;
    harness
        .wait_for(child, |projection| {
            matches!(projection.turn, TurnState::Settled(_, _))
                && !projection.background_tasks.is_empty()
        })
        .await;
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
    harness.insert(delegation.clone(), "settle").await;

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

    harness.send(child, "clear-background").await;
    harness
        .wait_for(child, |projection| projection.background_tasks.is_empty())
        .await;
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

    harness.insert(delegation.clone(), "recover").await;
    let worker = harness.worker.take().expect("worker is available");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(worker.run(shutdown.clone()));
    for _ in 0..200 {
        if harness
            .store
            .delegation_outbox()
            .await
            .expect("read startup rows")
            .is_empty()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("read drained startup rows")
            .is_empty()
    );
    harness.enqueue(delegation.id, "nudge").await;
    tokio::time::advance(RETRY_TICK).await;
    tokio::time::resume();
    for _ in 0..200 {
        if harness.provider_log().contains(NUDGE) {
            break;
        }
        tokio::task::yield_now().await;
    }
    tokio::time::pause();

    let stored = harness
        .store
        .delegation(delegation.id)
        .await
        .expect("read delegation")
        .expect("delegation exists");
    assert_eq!(stored.nudges, 1);
    assert!(harness.provider_log().contains(NUDGE));
    shutdown.cancel();
    task.await.expect("worker stops cleanly");
}
