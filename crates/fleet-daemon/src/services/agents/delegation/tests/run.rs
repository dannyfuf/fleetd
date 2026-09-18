use std::{os::unix::fs::PermissionsExt as _, sync::Arc};

use chrono::Utc;
use fleet_core::{
    agents::{
        AgentKind, AgentThreadSummary, Delegation, DelegationId, DelegationStatus, DeliveryState,
        ItemId, ItemKind, MessageOrigin, PermissionMode, SessionState, ThreadId, ThreadProjection,
        TurnId, TurnState,
    },
    ids::{ContextId, HostId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    paths::FleetHome,
    state::default_state,
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

use super::super::{
    DelegationService, RunRequest,
    footer::{SAME_WORKTREE_WARNING, first_message},
};

pub(crate) struct Harness {
    _directory: tempfile::TempDir,
    environment_log: std::path::PathBuf,
    input_log: std::path::PathBuf,
    executable: String,
    config: Arc<ConfigStore>,
    events: BroadcastBus,
    manager: AgentSessionManager,
    store: SqliteAgentStore,
    service: DelegationService,
    worktree: WorktreeId,
}

impl Harness {
    pub(crate) async fn start() -> Self {
        let directory = tempfile::tempdir().expect("create run test directory");
        let home = directory.path().join("fleet");
        let repos = home.join("repos");
        let worktree_path = home.join("worktrees/owner/repo/feature");
        std::fs::create_dir_all(&repos).expect("create repositories directory");
        std::fs::create_dir_all(&worktree_path).expect("create worktree directory");

        let environment_log = home.join("provider-environment.log");
        let input_log = home.join("provider-input.log");
        let executable_path = home.join("fake-claude");
        std::fs::write(
            &executable_path,
            provider_script(&environment_log, &input_log),
        )
        .expect("write scripted provider");
        let mut permissions = std::fs::metadata(&executable_path)
            .expect("read scripted provider metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable_path, permissions)
            .expect("make scripted provider executable");
        let executable = executable_path.display().to_string();

        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [repos.clone(), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        let mut effective = config.load().await.expect("load test config");
        effective.agent_binaries.claude.clone_from(&executable);
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
            .expect("the run test database opens");
        let (service, _worker) = DelegationService::new(
            store.clone(),
            manager.clone(),
            events.clone(),
            Arc::clone(&config),
            worktrees,
        );
        Self {
            _directory: directory,
            environment_log,
            input_log,
            executable,
            config,
            events: events.clone(),
            manager,
            store,
            service,
            worktree,
        }
    }

    async fn create_thread(&self) -> AgentThreadSummary {
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
            .expect("create test caller");
        let summary = match response {
            ResponseBody::AgentThreadCreated(summary) => summary,
            other => panic!("expected AgentThreadCreated, got {other:?}"),
        };
        self.wait_for(summary.thread, |projection| {
            projection.session == SessionState::Ready
        })
        .await;
        tokio::time::pause();
        summary
    }

    pub(crate) async fn running_caller(&self) -> (ThreadId, TurnId) {
        let caller = self.create_thread().await.thread;
        tokio::time::resume();
        self.manager
            .send(
                caller,
                fleet_core::agents::UserInput {
                    text: "hold caller turn".to_owned(),
                    ..fleet_core::agents::UserInput::default()
                },
            )
            .await
            .expect("start caller turn");
        let projection = self
            .wait_for(caller, |projection| {
                matches!(projection.turn, TurnState::Running(_))
            })
            .await;
        tokio::time::pause();
        match projection.turn {
            TurnState::Running(turn) => (caller, turn),
            other => panic!("expected a running caller turn, got {other:?}"),
        }
    }

    pub(crate) async fn wait_for(
        &self,
        thread: ThreadId,
        predicate: impl Fn(&ThreadProjection) -> bool,
    ) -> ThreadProjection {
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

    async fn wait_for_log(&self, path: &std::path::Path, needle: &str) -> String {
        tokio::time::resume();
        let mut events = self.events.subscribe();
        loop {
            let contents = std::fs::read_to_string(path).unwrap_or_default();
            if contents.contains(needle) {
                tokio::time::pause();
                return contents;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tokio::time::pause();
                    panic!("event bus closed while waiting for {}", path.display())
                }
            }
        }
    }

    pub(super) async fn run(
        &self,
        request: RunRequest,
    ) -> Result<ResponseBody, fleet_proto::error::ProtoError> {
        tokio::time::resume();
        let response = self.service.run(request).await;
        tokio::time::pause();
        response
    }

    pub(super) async fn run_pair(
        &self,
        left: RunRequest,
        right: RunRequest,
    ) -> (
        Result<ResponseBody, fleet_proto::error::ProtoError>,
        Result<ResponseBody, fleet_proto::error::ProtoError>,
    ) {
        tokio::time::resume();
        let responses = tokio::join!(self.service.run(left), self.service.run(right));
        tokio::time::pause();
        responses
    }

    pub(super) async fn send(&self, thread: ThreadId, text: &str) {
        tokio::time::resume();
        self.manager
            .send(
                thread,
                fleet_core::agents::UserInput {
                    text: text.to_owned(),
                    ..fleet_core::agents::UserInput::default()
                },
            )
            .await
            .expect("send scripted input");
        tokio::time::pause();
    }

    pub(super) async fn stop(&self, thread: ThreadId) {
        tokio::time::resume();
        self.manager
            .stop(thread)
            .await
            .expect("stop scripted agent");
        tokio::time::pause();
    }

    pub(super) async fn drain(&self) {
        tokio::time::resume();
        super::super::worker::drain(&self.service)
            .await
            .expect("drain delegation outbox");
        tokio::time::pause();
    }

    pub(super) async fn wait_for_delegation(
        &self,
        id: DelegationId,
        predicate: impl Fn(&Delegation) -> bool,
    ) -> Delegation {
        tokio::time::resume();
        let mut events = self.events.subscribe();
        loop {
            if let Some(delegation) = self
                .store
                .delegation(id)
                .await
                .expect("read scripted delegation")
                && predicate(&delegation)
            {
                tokio::time::pause();
                return delegation;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tokio::time::pause();
                    panic!("event bus closed while waiting for delegation {id}")
                }
            }
        }
    }

    pub(super) async fn wait_for_outbox(
        &self,
        delegation: DelegationId,
        action: crate::services::agents::store::OutboxAction,
    ) {
        tokio::time::resume();
        let mut events = self.events.subscribe();
        loop {
            if self
                .store
                .delegation_outbox()
                .await
                .expect("read scripted delegation outbox")
                .iter()
                .any(|row| row.delegation == delegation && row.action == action)
            {
                tokio::time::pause();
                return;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tokio::time::pause();
                    panic!("event bus closed while waiting for {action:?} on {delegation}")
                }
            }
        }
    }

    pub(super) async fn token(&self, delegation: &Delegation) -> String {
        let environment = self
            .wait_for_log(&self.environment_log, &delegation.id.to_string())
            .await;
        token_from_environment(&environment, delegation)
    }

    pub(crate) fn service(&self) -> &DelegationService {
        &self.service
    }

    #[cfg(feature = "real-agents")]
    pub(crate) async fn allow_permission(
        &self,
        thread: ThreadId,
        gate: fleet_core::agents::GateId,
    ) {
        self.manager
            .respond(
                thread,
                gate,
                fleet_core::agents::GateAnswer::Permission {
                    choice: fleet_core::agents::PermissionChoice::AllowOnce,
                    edited_payload: None,
                },
            )
            .await
            .expect("allow live-provider permission gate");
    }

    pub(super) async fn insert_live(&self, caller: ThreadId, child: ThreadId, depth: u8) {
        let delegation = Delegation {
            id: DelegationId::new(),
            caller,
            caller_turn: TurnId::new(),
            caller_item: ItemId::new(),
            child,
            provider: AgentKind::Claude,
            depth,
            brief: "seed live delegation".to_owned(),
            expectation: "remain live for limit validation".to_owned(),
            eager: false,
            status: DelegationStatus::Starting,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: Utc::now(),
            finished: None,
            headline: None,
        };
        self.store
            .delegation_write("seed run-test delegation", move |tx| {
                crate::services::agents::store::delegations::insert(
                    tx,
                    &delegation,
                    "test-token-hash",
                )?;
                Ok(((), false))
            })
            .await
            .expect("seed live delegation");
    }

    pub(crate) async fn set_claude_binary(&self, binary: &str) {
        let mut config = self.config.load().await.expect("load test config");
        config.agent_binaries.claude = binary.to_owned();
        self.config.save(config).await.expect("save test config");
    }

    #[cfg(feature = "real-agents")]
    pub(crate) async fn set_codex_binary(&self, binary: &str) {
        let mut config = self.config.load().await.expect("load test config");
        config.agent_binaries.codex = binary.to_owned();
        self.config.save(config).await.expect("save test config");
    }
}

fn provider_script(environment_log: &std::path::Path, input_log: &std::path::Path) -> String {
    format!(
        r##"#!/bin/sh
case " $* " in
  *" --version "*) printf '%s\n' '2.1.266 (Claude Code)'; exit 0 ;;
esac
printf '%s|%s|%s\n' "$FLEET_SESSION" "$FLEET_DELEGATION" "$FLEET_DELEGATION_TOKEN" >> '{}'
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"run-cursor","model":"test","tools":[],"slash_commands":[],"capabilities":["interrupt_receipt_v1","interrupt_cancel_queued_v1","msg_lifecycle_v1"]}}'
count=0
while IFS= read -r line; do
  count=$((count + 1))
  printf '%s|%s\n' "$FLEET_SESSION" "$line" >> '{}'
  printf '%s\n' "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-$count\",\"content\":[{{\"type\":\"text\",\"text\":\"scripted answer\"}}]}},\"parent_tool_use_id\":null}}"
  case "$line" in
    *hold*) ;;
    *fail-outcome*) printf '%s\n' '{{"type":"result","subtype":"error","is_error":true,"terminal_reason":"api_error","result":"scripted failure","usage":{{}}}}' ;;
    *) printf '%s\n' '{{"type":"result","subtype":"success","terminal_reason":"completed","usage":{{}}}}' ;;
  esac
done
"##,
        environment_log.display(),
        input_log.display()
    )
}

fn token_from_environment(environment: &str, delegation: &Delegation) -> String {
    environment
        .lines()
        .find(|line| line.starts_with(&delegation.child.to_string()))
        .and_then(|line| line.split('|').nth(2))
        .map(str::to_owned)
        .expect("child environment carries its delegation token")
}

pub(crate) fn request(caller: ThreadId) -> RunRequest {
    RunRequest {
        caller,
        provider: AgentKind::Claude,
        brief: "hold delegated task".to_owned(),
        expectation: "return a verified report".to_owned(),
        worktree: None,
        mode: None,
        model: None,
        title: None,
        eager: false,
    }
}

pub(crate) fn started(response: ResponseBody) -> (Delegation, Option<String>) {
    match response {
        ResponseBody::DelegationStarted {
            delegation,
            warning,
        } => (delegation, warning),
        other => panic!("expected DelegationStarted, got {other:?}"),
    }
}

pub(super) async fn assert_refusal(
    harness: &Harness,
    request: RunRequest,
    kind: ErrorKind,
    rule: &str,
) {
    let error = harness
        .run(request)
        .await
        .expect_err("delegation run should be refused");
    assert_eq!(error.kind, kind);
    assert!(
        error.message.contains(rule),
        "refusal did not name {rule}: {}",
        error.message
    );
}

#[tokio::test(start_paused = true)]
async fn run_carries_the_token_and_seeds_both_transcripts() {
    let harness = Harness::start().await;
    let (caller, caller_turn) = harness.running_caller().await;

    let (delegation, warning) = started(
        harness
            .run(request(caller))
            .await
            .expect("start delegation"),
    );

    assert_eq!(delegation.caller_turn, caller_turn);
    assert_eq!(delegation.depth, 1);
    assert_eq!(warning.as_deref(), Some(SAME_WORKTREE_WARNING));

    let environment = harness
        .wait_for_log(&harness.environment_log, &delegation.id.to_string())
        .await;
    let child_environment = environment
        .lines()
        .find(|line| line.starts_with(&delegation.child.to_string()))
        .expect("child environment was logged");
    let mut environment_fields = child_environment.split('|');
    assert_eq!(
        environment_fields.next(),
        Some(delegation.child.to_string().as_str())
    );
    assert_eq!(
        environment_fields.next(),
        Some(delegation.id.to_string().as_str())
    );
    let token = environment_fields
        .next()
        .expect("child environment carries the token");
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let caller_projection = harness
        .wait_for(caller, |projection| {
            projection.items.iter().any(|item| {
                item.id == delegation.caller_item
                    && item.turn == caller_turn
                    && matches!(
                        item.kind,
                        ItemKind::Delegation {
                            id,
                            child,
                            status: DelegationStatus::Starting,
                            ..
                        } if id == delegation.id && child == delegation.child
                    )
            })
        })
        .await;
    assert!(
        caller_projection.items.iter().any(|item| {
            item.id == delegation.caller_item && item.turn == delegation.caller_turn
        })
    );

    let expected_message = first_message(&delegation.brief, delegation.id, &delegation.expectation);
    let child_projection = harness
        .wait_for(delegation.child, |projection| {
            projection.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemKind::UserMessage { text, origin, .. }
                        if text == &expected_message && *origin == MessageOrigin::User
                )
            })
        })
        .await;
    assert!(child_projection.items.iter().any(|item| {
        matches!(
            &item.kind,
            ItemKind::UserMessage { text, origin, .. }
                if text == &expected_message && *origin == MessageOrigin::User
        )
    }));
    let child_input = harness
        .wait_for_log(&harness.input_log, &delegation.id.to_string())
        .await;
    assert!(child_input.contains(&delegation.brief));
    assert!(child_input.contains(&delegation.expectation));
}

#[tokio::test(start_paused = true)]
async fn run_refusals_follow_the_validation_order_and_name_each_rule() {
    let harness = Harness::start().await;

    assert_refusal(
        &harness,
        request(ThreadId::new()),
        ErrorKind::NotFound,
        "caller-exists rule",
    )
    .await;

    let mut mirrored = harness.create_thread().await;
    mirrored.thread = ThreadId::new();
    let host = HostId::try_from(uuid::Uuid::new_v4().to_string().as_str()).expect("host id");
    mirrored.host = Some(host.clone());
    harness
        .store
        .mirror_claim(host, &mirrored)
        .await
        .expect("seed mirrored caller");
    assert_refusal(
        &harness,
        request(mirrored.thread),
        ErrorKind::Unsupported,
        "caller-locality rule",
    )
    .await;

    let idle = harness.create_thread().await.thread;
    assert_refusal(
        &harness,
        request(idle),
        ErrorKind::Conflict,
        "running-turn rule",
    )
    .await;

    let (caller, _) = harness.running_caller().await;
    harness.set_claude_binary("'").await;
    let mut missing_worktree = request(caller);
    missing_worktree.worktree = Some(
        WorktreeId::try_from("owner/repo#missing").expect("syntactically valid missing worktree"),
    );
    assert_refusal(
        &harness,
        missing_worktree,
        ErrorKind::Unsupported,
        "provider-binary rule",
    )
    .await;

    harness.set_claude_binary(&harness.executable).await;
    let mut missing_worktree = request(caller);
    missing_worktree.worktree = Some(
        WorktreeId::try_from("owner/repo#missing").expect("syntactically valid missing worktree"),
    );
    assert_refusal(
        &harness,
        missing_worktree,
        ErrorKind::NotFound,
        "worktree-resolution rule",
    )
    .await;
}
