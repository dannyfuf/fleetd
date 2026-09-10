//! Manager lifecycle tests driven by a scripted in-process provider.

use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use fleet_core::{
    agents::{
        Attention, AttentionKind, Capabilities, GateKind, PermissionChoice, StreamKind, ToolKind,
        Usage,
    },
    ids::{ContextId, RepoId},
    model::{Context as ContextRecord, Repo, RepoHooks, Worktree},
    state::default_state,
};
use tokio::sync::{broadcast, mpsc};

use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    services::{
        agents::providers::{ProviderResult, ProviderSink, empty_events},
        sessions::Sessions,
    },
    stores::{config::ConfigStore, state::StateStore},
};

use super::*;

const SETTLE: Duration = Duration::from_secs(5);

#[test]
fn restart_recovery_never_advances_memory_past_a_log_it_could_not_write() {
    // The store's root is a *file*, so every append fails. §6 makes the log what the reducer
    // wrote before the event became visible: a projection that ran ahead of a failed append
    // would make the next real append leave a hole on disk, and the following start would
    // quarantine the whole tail of the transcript behind it.
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("agents");
    std::fs::write(&root, b"not a directory").expect("occupy the store root");
    let store = AgentStore::new(root);

    let thread = ThreadId::new();
    let worktree = WorktreeId::try_from("acme/api#feature").expect("worktree");
    let created = Utc::now();
    let mut record = AgentThreadRecord {
        thread,
        worktree: worktree.clone(),
        provider: AgentKind::Claude,
        title: "Claude".to_owned(),
        created,
        last_activity: created,
        resume_cursor: None,
        model: None,
        mode: PermissionMode::Ask,
        last_outcome: None,
    };
    let mut projection = ThreadProjection::new(thread, worktree, AgentKind::Claude);
    let gate = GateId::new();
    projection
        .apply(&SeqEvent {
            seq: Seq(1),
            at: created,
            raw: None,
            event: AgentEvent::GateOpened {
                gate,
                turn: None,
                kind: GateKind::Plan {
                    markdown: "plan".to_owned(),
                    steps: Vec::new(),
                },
            },
        })
        .expect("open a gate");
    projection.session = SessionState::Running;

    recover_orphan(&store, &mut record, &mut projection);

    assert_eq!(
        projection.last_seq,
        Seq(1),
        "the projection advanced past an event the log never received",
    );
    assert!(
        !projection.gates.is_empty(),
        "the gate was settled in memory only",
    );
}

/// One observed provider command, in call order.
#[derive(Debug, Clone, PartialEq)]
enum FakeCall {
    Start(ThreadId, Option<String>),
    Send(TurnId, String),
    Interrupt(TurnId),
    Respond(GateId),
    SetMode(PermissionMode),
    SetModel(ModelSelection),
    Stop,
}

/// Shared control surface for every provider the factory hands to the manager.
struct FakeScript {
    calls: StdMutex<Vec<FakeCall>>,
    senders: StdMutex<Vec<ProviderSink>>,
    capabilities: Capabilities,
    unavailable: AtomicBool,
    /// Makes every `stop` fail, as a child that exits from the stdin close does.
    stop_fails: AtomicBool,
}

impl FakeScript {
    fn new(capabilities: Capabilities) -> Arc<Self> {
        Arc::new(Self {
            calls: StdMutex::new(Vec::new()),
            senders: StdMutex::new(Vec::new()),
            capabilities,
            unavailable: AtomicBool::new(false),
            stop_fails: AtomicBool::new(false),
        })
    }

    fn record(&self, call: FakeCall) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    fn calls(&self) -> Vec<FakeCall> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn starts(&self) -> usize {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Pushes one normalized event into the newest provider's stream.
    async fn emit(&self, event: AgentEvent) {
        let sender = self
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .cloned()
            .expect("a provider must have been started");
        sender.send(event.into()).expect("provider stream is open");
    }
}

/// Scripted `AgentProvider` that records commands and replays test-driven events.
struct FakeProvider {
    kind: AgentKind,
    script: Arc<FakeScript>,
    events: Option<ProviderEvents>,
}

#[async_trait]
impl AgentProvider for FakeProvider {
    fn kind(&self) -> AgentKind {
        self.kind
    }

    fn capabilities(&self) -> Capabilities {
        self.script.capabilities
    }

    async fn start(&mut self, req: StartRequest) -> ProviderResult<()> {
        self.script
            .record(FakeCall::Start(req.thread, req.resume_cursor));
        Ok(())
    }

    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<()> {
        self.script.record(FakeCall::Send(turn, input.text));
        Ok(())
    }

    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()> {
        self.script.record(FakeCall::Interrupt(turn));
        Ok(())
    }

    async fn respond(&mut self, gate: GateId, _answer: GateAnswer) -> ProviderResult<()> {
        self.script.record(FakeCall::Respond(gate));
        Ok(())
    }

    async fn set_mode(&mut self, mode: PermissionMode) -> ProviderResult<PermissionMode> {
        self.script.record(FakeCall::SetMode(mode));
        Ok(mode)
    }

    async fn set_model(&mut self, model: ModelSelection) -> ProviderResult<()> {
        self.script.record(FakeCall::SetModel(model));
        Ok(())
    }

    async fn stop(&mut self) -> ProviderResult<()> {
        self.script.record(FakeCall::Stop);
        if self.script.stop_fails.load(Ordering::SeqCst) {
            return Err(ProviderError::Exited { code: None });
        }
        Ok(())
    }

    fn events(&mut self) -> ProviderEvents {
        self.events.take().unwrap_or_else(empty_events)
    }
}

fn factory(script: &Arc<FakeScript>) -> Arc<ProviderFactory> {
    let script = Arc::clone(script);
    Arc::new(
        move |kind: AgentKind, _request: &StartRequest, _commands: &AgentCommands| {
            if script.unavailable.load(Ordering::SeqCst) {
                return Err(anyhow::Error::new(ProviderError::Unavailable {
                    reason: format!("the {} executable was not found", kind.executable()),
                }));
            }
            let (sender, receiver) = mpsc::unbounded_channel();
            script
                .senders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(sender);
            Ok(Box::new(FakeProvider {
                kind,
                script: Arc::clone(&script),
                events: Some(receiver),
            }) as Box<dyn AgentProvider>)
        },
    )
}

/// A temporary daemon home with one published worktree and a scripted provider.
struct Harness {
    _temp: tempfile::TempDir,
    home: PathBuf,
    manager: AgentSessionManager,
    events: BroadcastBus,
    worktrees: Worktrees,
    worktree: WorktreeId,
    script: Arc<FakeScript>,
}

impl Harness {
    async fn start(capabilities: Capabilities) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("fleet");
        let repos = home.join("repos");
        let worktree_path = home.join("worktrees/repo/feature");
        std::fs::create_dir_all(&repos).expect("create repos");
        std::fs::create_dir_all(&worktree_path).expect("create worktree");
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [repos.clone(), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
        let context = ContextId::try_from("team").expect("context id");
        let repo = RepoId::try_from("owner/repo").expect("repo id");
        let worktree = WorktreeId::try_from("owner/repo#feature").expect("worktree id");
        let mut persisted = default_state();
        persisted.contexts.push(ContextRecord {
            id: context.clone(),
            name: "Team".to_owned(),
            owners: Vec::new(),
            created_at: "2026-09-07T12:00:00Z".to_owned(),
        });
        persisted.repos.push(Repo {
            id: repo.clone(),
            owner: "owner".to_owned(),
            name: "repo".to_owned(),
            url: "https://example.invalid/owner/repo".to_owned(),
            context_id: context,
            default_branch: "main".to_owned(),
            path: repos.join("owner/repo").to_string_lossy().into_owned(),
            cloned_at: "2026-09-07T12:00:00Z".to_owned(),
            hooks: RepoHooks::default(),
        });
        persisted.worktrees.push(Worktree {
            id: worktree.clone(),
            repo_id: repo,
            slug: "feature".to_owned(),
            branch: "feature".to_owned(),
            base_ref: "main".to_owned(),
            path: worktree_path.to_string_lossy().into_owned(),
            session: "repo/feature".to_owned(),
            host: None,
            created_at: "2026-09-07T12:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        });
        state.save(persisted).await.expect("seed state");
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
        let worktrees = Worktrees::new(
            Arc::clone(&config),
            Arc::clone(&state),
            Arc::new(JobManager::new(&home)),
            &Adapters::system(files),
            sessions,
        );
        let events = BroadcastBus::new(1_024);
        let script = FakeScript::new(capabilities);
        let manager = AgentSessionManager::new_with_factory(
            AgentStore::new(home.join("agents")),
            events.clone(),
            worktrees.clone(),
            None,
            factory(&script),
        );
        Self {
            _temp: temp,
            home,
            manager,
            events,
            worktrees,
            worktree,
            script,
        }
    }

    /// Rebuilds the manager over the same store, as a daemon restart would.
    fn restart(&self) -> AgentSessionManager {
        AgentSessionManager::new_with_factory(
            AgentStore::new(self.home.join("agents")),
            self.events.clone(),
            self.worktrees.clone(),
            None,
            factory(&self.script),
        )
    }

    async fn create(&self, resume_cursor: Option<String>) -> AgentThreadSummary {
        let response = self
            .manager
            .create(
                self.worktree.clone(),
                AgentKind::Claude,
                None,
                PermissionMode::Ask,
                resume_cursor,
                None,
            )
            .await
            .expect("create agent thread");
        match response {
            ResponseBody::AgentThreadCreated(summary) => summary,
            other => panic!("expected AgentThreadCreated, got {other:?}"),
        }
    }

    async fn projection(&self, thread: ThreadId) -> ThreadProjection {
        match self.manager.open(thread, None).await.expect("open thread") {
            ResponseBody::AgentThreadSnapshot { projection, .. } => projection,
            other => panic!("expected AgentThreadSnapshot, got {other:?}"),
        }
    }

    /// Waits until `predicate` holds of the projection, failing the test on timeout.
    async fn settle(
        &self,
        thread: ThreadId,
        what: &str,
        predicate: impl Fn(&ThreadProjection) -> bool,
    ) -> ThreadProjection {
        // Subscribed before the first read: every applied event is broadcast, so the wait is on
        // the manager's own signal rather than on a wall clock, and an event published while
        // this is between a read and a `recv` is buffered rather than missed. `SETTLE` stays
        // only as a backstop, so a manager that publishes nothing fails instead of hanging.
        let mut updates = self.events.subscribe();
        tokio::time::timeout(SETTLE, async {
            loop {
                let projection = self.projection(thread).await;
                if predicate(&projection) {
                    return projection;
                }
                next_update(&mut updates, what).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("agent thread never reached {what}"))
    }
}

/// Awaits the manager's next broadcast.
async fn next_update(updates: &mut broadcast::Receiver<Event>, what: &str) {
    match updates.recv().await {
        // A lagged receiver missed frames the projection it re-reads already reflects.
        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
        Err(broadcast::error::RecvError::Closed) => {
            panic!("the event bus closed before the thread reached {what}")
        }
    }
}

#[tokio::test]
async fn remote_worktree_guard_precedes_path_provider_and_store_access() {
    let harness = Harness::start(Capabilities::default()).await;
    let host = HostId::try_from("dev-box").expect("host");
    let guarded_worktree = harness.worktree.clone();
    harness
        .manager
        .set_remote_host_resolver(Arc::new(move |worktree| {
            (worktree == &guarded_worktree).then(|| host.clone())
        }));

    let error = harness
        .manager
        .create(
            harness.worktree.clone(),
            AgentKind::Claude,
            None,
            PermissionMode::Ask,
            None,
            None,
        )
        .await
        .expect_err("remote worktree must not reach the local manager");

    assert_eq!(error.kind, ErrorKind::Remote);
    assert!(error.message.contains("dev-box"));
    assert!(
        harness.script.calls().is_empty(),
        "provider was not started"
    );
    assert!(
        !harness.home.join("agents").exists(),
        "local agent transcript store was not created"
    );
}

fn drain(receiver: &mut broadcast::Receiver<Event>) -> Vec<Event> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    events
}

fn attentions(events: &[Event]) -> Vec<Attention> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::AgentSummary(summary) => Some(summary.attention),
            _ => None,
        })
        .collect()
}

fn full() -> Capabilities {
    Capabilities {
        resume: true,
        fork: false,
        steer: true,
        interrupt: true,
        modes: true,
        models: true,
    }
}

fn assistant_text(projection: &ThreadProjection) -> Option<&str> {
    projection
        .items
        .iter()
        .find(|entry| matches!(entry.kind, ItemKind::AssistantText))
        .and_then(|entry| entry.text.as_deref())
}

fn permission_gate(gate: GateId) -> AgentEvent {
    AgentEvent::GateOpened {
        gate,
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "Run a command".to_owned(),
            payload: "cargo test".to_owned(),
            rationale: None,
            options: Vec::new(),
        },
    }
}

fn completed(turn: TurnId) -> AgentEvent {
    AgentEvent::TurnCompleted {
        turn,
        outcome: TurnOutcome::Completed,
        usage: Usage::default(),
        duration_ms: 12,
        files_changed: Vec::new(),
    }
}

#[tokio::test]
async fn create_starts_the_provider_and_persists_the_thread() {
    let harness = Harness::start(full()).await;
    let summary = harness.create(None).await;

    assert_eq!(summary.provider, AgentKind::Claude);
    assert_eq!(summary.session, SessionState::Starting);
    assert_eq!(summary.worktree, harness.worktree);
    assert_eq!(
        harness.script.calls(),
        vec![FakeCall::Start(summary.thread, None)]
    );
    let index = AgentStore::new(harness.home.join("agents"))
        .read_index()
        .expect("read index");
    assert_eq!(index.threads.len(), 1);
    assert_eq!(index.threads[0].thread, summary.thread);
    assert_eq!(
        harness
            .manager
            .summaries()
            .into_iter()
            .map(|summary| summary.thread)
            .collect::<Vec<_>>(),
        vec![summary.thread]
    );
}

#[tokio::test]
async fn a_turn_streams_items_and_moves_attention_to_finished() {
    let harness = Harness::start(full()).await;
    let mut receiver = harness.events.subscribe();
    let thread = harness.create(None).await.thread;
    harness
        .script
        .emit(AgentEvent::SessionStarted {
            provider: AgentKind::Claude,
            resume_cursor: None,
            model: None,
            mode: PermissionMode::Ask,
            tools: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
        })
        .await;
    let ready = harness
        .settle(thread, "ready", |projection| {
            projection.session == SessionState::Ready
        })
        .await;
    harness
        .manager
        .mark_seen(thread, ready.last_seq)
        .await
        .expect("mark seen");

    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "ship it".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send");
    let Some(FakeCall::Send(turn, sent)) = harness.script.calls().last().cloned() else {
        panic!("send must reach the provider with the allocated turn");
    };
    assert_eq!(sent, "ship it");
    let user_item = ItemId::new();
    let assistant = ItemId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted { turn, user_item })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;
    harness
        .script
        .emit(AgentEvent::ItemStarted {
            turn,
            item: assistant,
            kind: ItemKind::AssistantText,
            parent: None,
        })
        .await;
    for delta in ["hel", "lo"] {
        harness
            .script
            .emit(AgentEvent::ContentDelta {
                item: assistant,
                stream: StreamKind::AssistantText,
                delta: delta.to_owned(),
            })
            .await;
    }
    harness
        .script
        .emit(AgentEvent::ItemCompleted {
            item: assistant,
            status: ItemStatus::Done,
        })
        .await;
    harness.script.emit(completed(turn)).await;

    let projection = harness
        .settle(thread, "a completed turn", |projection| {
            matches!(projection.turn, TurnState::Completed(finished, _) if finished == turn)
        })
        .await;
    assert_eq!(assistant_text(&projection), Some("hello"));
    assert!(projection.items.iter().any(|item| matches!(
        &item.kind,
        ItemKind::UserMessage { text, .. } if text == "ship it"
    )));
    let summary = harness
        .manager
        .summaries()
        .pop()
        .expect("one thread summary");
    assert_eq!(
        summary.attention,
        Attention::NeedsYou(AttentionKind::Finished)
    );
    let observed = attentions(&drain(&mut receiver));
    assert!(observed.contains(&Attention::Working), "{observed:?}");
    assert!(
        observed.contains(&Attention::NeedsYou(AttentionKind::Finished)),
        "{observed:?}"
    );

    // §3.3 keeps the seen cursor "per client, not persisted daemon-side": one broadcast
    // summary reaches every subscriber, so it keeps reporting the unread completion and the
    // client that read the thread narrows it against the cursor it holds itself.
    harness
        .manager
        .mark_seen(thread, projection.last_seq)
        .await
        .expect("mark seen");
    let summary = harness
        .manager
        .summaries()
        .pop()
        .expect("one thread summary");
    assert_eq!(
        summary.attention,
        Attention::NeedsYou(AttentionKind::Finished),
        "the daemon must not fold one client's cursor into the shared summary"
    );
    assert_eq!(
        summary.attention_for(projection.last_seq),
        Attention::Idle,
        "a fully seen idle thread has no attention for the client that saw it"
    );
}

#[tokio::test]
async fn steering_a_running_turn_records_a_user_message_in_that_turn() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "also update the docs".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("steer");

    let projection = harness.projection(thread).await;
    let steered = projection
        .items
        .iter()
        .find(|item| matches!(&item.kind, ItemKind::UserMessage { text, .. } if text == "also update the docs"))
        .expect("the steered message is recorded in the running turn");
    assert_eq!(steered.turn, turn);
    assert_eq!(steered.status, ItemStatus::Done);
    assert!(matches!(
        harness.script.calls().last(),
        Some(FakeCall::Send(sent, _)) if *sent == turn
    ));
}

#[tokio::test]
async fn gates_open_for_attention_and_respond_reaches_the_provider() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let gate = GateId::new();
    harness.script.emit(permission_gate(gate)).await;
    harness
        .settle(thread, "an open gate", |projection| {
            !projection.gates.is_empty()
        })
        .await;
    assert_eq!(
        harness.manager.summaries()[0].attention,
        Attention::NeedsYou(AttentionKind::Permission)
    );

    let answer = GateAnswer::Permission {
        choice: PermissionChoice::AllowOnce,
        edited_payload: None,
    };
    let unknown = harness
        .manager
        .respond(thread, GateId::new(), answer.clone())
        .await
        .expect_err("an unknown gate is rejected");
    assert_eq!(unknown.kind, ErrorKind::Conflict);

    harness
        .manager
        .respond(thread, gate, answer)
        .await
        .expect("respond");
    assert!(harness.script.calls().contains(&FakeCall::Respond(gate)));

    harness
        .script
        .emit(AgentEvent::GateResolved {
            gate,
            answer: GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
            by: fleet_core::agents::GateResolver::User,
        })
        .await;
    harness
        .settle(thread, "a closed gate", |projection| {
            projection.gates.is_empty()
        })
        .await;
}

#[tokio::test]
async fn interrupting_a_settled_turn_acks_while_a_live_one_reaches_the_provider() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    // `esc` and the `result` frame that ends a turn race by milliseconds, and the client cannot
    // see the settle coming: an interrupt with nothing to interrupt is a no-op, not a conflict
    // the status bar has to show the user.
    assert!(matches!(
        harness.manager.interrupt(thread).await,
        Ok(ResponseBody::AgentAck)
    ));
    assert!(
        !harness
            .script
            .calls()
            .iter()
            .any(|call| matches!(call, FakeCall::Interrupt(_))),
        "an idle thread must not reach the provider at all"
    );

    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;
    harness.manager.interrupt(thread).await.expect("interrupt");
    assert!(harness.script.calls().contains(&FakeCall::Interrupt(turn)));

    harness
        .script
        .emit(AgentEvent::TurnAborted {
            turn,
            reason: AbortReason::User,
        })
        .await;
    let projection = harness
        .settle(thread, "an interrupted turn", |projection| {
            projection.turn == TurnState::Interrupted(turn)
        })
        .await;
    assert!(projection.gates.is_empty());
}

#[tokio::test]
async fn stop_exits_the_session_and_refuses_later_sends() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness.manager.stop(thread).await.expect("stop");
    assert!(harness.script.calls().contains(&FakeCall::Stop));
    let projection = harness.projection(thread).await;
    assert_eq!(projection.session, SessionState::Stopped);
    assert_eq!(projection.turn, TurnState::Interrupted(turn));

    let refused = harness
        .manager
        .send(
            thread,
            UserInput {
                text: "hello".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect_err("a stopped thread cannot be sent to");
    assert_eq!(refused.kind, ErrorKind::Conflict);
    assert_eq!(
        harness.manager.summaries()[0].session,
        SessionState::Stopped
    );
}

#[tokio::test]
async fn open_returns_only_events_after_the_requested_cursor() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    for message in ["one", "two", "three"] {
        harness
            .script
            .emit(AgentEvent::Notice(message.to_owned()))
            .await;
    }
    harness
        .settle(thread, "three notices", |projection| {
            projection.last_seq == Seq(3)
        })
        .await;

    let ResponseBody::AgentThreadSnapshot {
        projection,
        events_after,
    } = harness
        .manager
        .open(thread, Some(Seq(1)))
        .await
        .expect("open from a cursor")
    else {
        panic!("expected a snapshot");
    };
    assert_eq!(projection.last_seq, Seq(1));
    assert_eq!(
        events_after
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![Seq(2), Seq(3)]
    );
    let ahead = harness
        .manager
        .open(thread, Some(Seq(9)))
        .await
        .expect_err("a cursor beyond the log is rejected");
    assert_eq!(ahead.kind, ErrorKind::Validation);
}

#[tokio::test]
async fn restart_settles_a_ready_thread_and_lets_open_resume_it() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-ready".to_owned())).await.thread;
    harness
        .script
        .emit(AgentEvent::SessionStateChanged(SessionState::Ready))
        .await;
    harness
        .settle(thread, "a ready session", |projection| {
            projection.session == SessionState::Ready
        })
        .await;

    // The restart killed the child: a thread still claiming a live session is an orphan, and
    // one the log leaves `Ready` is exactly as orphaned as one it leaves `Running` (§6).
    let restarted = harness.restart();
    let summary = restarted
        .summaries()
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_eq!(summary.session, SessionState::Stopped);
    assert_eq!(harness.script.starts(), 1, "restart never reattaches");

    restarted
        .open(thread, None)
        .await
        .expect("open resumes a recovered thread");
    assert_eq!(harness.script.starts(), 2, "open starts a resumed provider");
    restarted
        .send(
            thread,
            UserInput {
                text: "still usable".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("a recovered thread accepts a send");
}

#[tokio::test]
async fn a_catch_up_open_never_starts_a_provider() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-gap".to_owned())).await.thread;
    harness.manager.stop(thread).await.expect("stop the thread");
    assert_eq!(
        harness
            .manager
            .summaries()
            .into_iter()
            .find(|summary| summary.thread == thread)
            .map(|summary| summary.session),
        Some(SessionState::Stopped)
    );
    let starts = harness.script.starts();

    // A client repairing a sequence gap asks from its last applied cursor. That is not a user
    // opening a tab, so it must not relaunch the provider (§6).
    harness
        .manager
        .open(thread, Some(Seq(1)))
        .await
        .expect("catch-up open");
    assert_eq!(harness.script.starts(), starts);
}

#[tokio::test]
async fn a_mode_change_is_a_durable_event_every_mirror_sees() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let mut events = harness.events.subscribe();
    harness
        .manager
        .set_mode(thread, PermissionMode::AcceptEdits)
        .await
        .expect("set mode");

    let broadcast = drain(&mut events);
    assert!(
        broadcast.iter().any(|event| matches!(
            event,
            Event::Agent {
                event: SeqEvent {
                    event: AgentEvent::MetadataChanged {
                        mode: Some(PermissionMode::AcceptEdits),
                        ..
                    },
                    ..
                },
                ..
            }
        )),
        "a mode change reaches clients as an event: {broadcast:?}"
    );
    assert_eq!(
        harness.projection(thread).await.mode,
        PermissionMode::AcceptEdits
    );
}

#[tokio::test]
async fn answering_one_gate_twice_writes_one_response() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let gate = GateId::new();
    harness
        .script
        .emit(AgentEvent::GateOpened {
            gate,
            turn: None,
            kind: GateKind::Permission {
                tool: ToolKind::Bash,
                title: "Run command?".to_owned(),
                payload: "cargo test".to_owned(),
                rationale: None,
                options: Vec::new(),
            },
        })
        .await;
    harness
        .settle(thread, "an open gate", |projection| {
            projection.gates.len() == 1
        })
        .await;

    let answer = GateAnswer::Permission {
        choice: PermissionChoice::AllowOnce,
        edited_payload: None,
    };
    for _ in 0..2 {
        harness
            .manager
            .respond(thread, gate, answer.clone())
            .await
            .expect("a repeated answer is idempotent");
    }
    assert_eq!(
        harness
            .script
            .calls()
            .iter()
            .filter(|call| **call == FakeCall::Respond(gate))
            .count(),
        1
    );
}

#[tokio::test]
async fn restart_fails_orphaned_threads_without_a_resume_cursor() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    let restarted = harness.restart();
    let summary = restarted
        .summaries()
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_eq!(summary.session, SessionState::Error);
    assert_eq!(summary.turn, TurnState::Failed(turn));
    assert_eq!(summary.attention, Attention::Failed);
    assert_eq!(harness.script.starts(), 1, "restart never reattaches");
}

#[tokio::test]
async fn restart_stops_resumable_threads_and_resumes_them_on_open() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-1".to_owned())).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    let restarted = harness.restart();
    let summary = restarted
        .summaries()
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_eq!(summary.session, SessionState::Stopped);
    assert_eq!(summary.turn, TurnState::Interrupted(turn));
    assert_eq!(harness.script.starts(), 1, "restart never reattaches");

    let ResponseBody::AgentThreadSnapshot { projection, .. } = restarted
        .open(thread, None)
        .await
        .expect("open resumes a stopped thread")
    else {
        panic!("expected a snapshot");
    };
    assert_eq!(harness.script.starts(), 2, "open starts a resumed provider");
    assert!(
        harness
            .script
            .calls()
            .contains(&FakeCall::Start(thread, Some("cursor-1".to_owned())))
    );
    assert!(
        projection.items.is_empty() || projection.last_seq > Seq(0),
        "a resumed thread keeps its transcript"
    );
}

#[tokio::test]
async fn a_session_that_ends_settles_the_gates_it_leaves_open() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let gate = GateId::new();
    harness.script.emit(permission_gate(gate)).await;
    harness
        .settle(thread, "an open gate", |projection| {
            !projection.gates.is_empty()
        })
        .await;

    harness
        .script
        .emit(AgentEvent::SessionExited {
            code: Some(137),
            expected: false,
        })
        .await;
    let projection = harness
        .settle(thread, "a dead session", |projection| {
            projection.session == SessionState::Error
        })
        .await;

    // §3.3 rule 4 and §6: the card cannot outlive the adapter that could answer it.
    assert!(
        projection.gates.is_empty(),
        "a dead session leaves no unanswerable card behind"
    );
    assert_eq!(
        harness.manager.summaries()[0].attention,
        Attention::Failed,
        "the tab reports the dead session, not the gate it left"
    );
}

#[tokio::test]
async fn a_restart_settles_the_gates_the_log_left_open() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-9".to_owned())).await.thread;
    harness.script.emit(permission_gate(GateId::new())).await;
    harness
        .settle(thread, "an open gate", |projection| {
            !projection.gates.is_empty()
        })
        .await;

    let restarted = harness.restart();
    let summary = restarted
        .summaries()
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_ne!(
        summary.attention,
        Attention::NeedsYou(AttentionKind::Permission),
        "a gate no adapter can answer does not survive the restart"
    );
}

#[tokio::test]
async fn send_resumes_a_stopped_thread_rather_than_refusing_it() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-2".to_owned())).await.thread;
    harness
        .manager
        .stop(thread)
        .await
        .expect("stop the live thread");

    // §6 resumes lazily on use; §7 makes `send` one of those uses.
    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "carry on".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send resumes the stopped thread");
    assert_eq!(
        harness.script.starts(),
        2,
        "send started a resumed provider"
    );
    assert!(
        harness
            .script
            .calls()
            .contains(&FakeCall::Start(thread, Some("cursor-2".to_owned())))
    );
}

/// BH1: a `kill -9` mid-turn left the dead adapter in the slot, and every later send failed
/// with `turn … was sent while turn … is active` until the daemon was restarted.
#[tokio::test]
async fn a_crashed_provider_is_dropped_so_the_next_send_resumes_the_thread() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-crash".to_owned())).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness
        .script
        .emit(AgentEvent::SessionExited {
            code: Some(137),
            expected: false,
        })
        .await;
    // Read through `summaries`, not `open`: opening the tab is itself one of the lazy resume
    // points, and it would hide the state this test is about.
    let mut updates = harness.events.subscribe();
    tokio::time::timeout(SETTLE, async {
        loop {
            let summary = harness
                .manager
                .summaries()
                .into_iter()
                .find(|summary| summary.thread == thread)
                .expect("the thread exists");
            if summary.session == SessionState::Error {
                return;
            }
            next_update(&mut updates, "a dead session").await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("agent thread never reached a dead session"));

    // §6: the thread is resumed lazily with a *new* adapter, not refused by the dead one.
    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "carry on".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send resumes a thread whose provider crashed");
    assert_eq!(
        harness.script.starts(),
        2,
        "the crashed adapter was replaced rather than reused"
    );
}

/// BH: `pending_claude_inputs` was drained only while the *front* entry matched the turn that
/// had just started, so one prompt whose `TurnStarted` never arrived blocked the queue: every
/// later prompt was written to Claude and never recorded, and §6's log stopped being the
/// transcript — the model answering a question the tab does not show.
#[tokio::test]
async fn a_prompt_whose_turn_never_started_does_not_swallow_the_next_one() {
    let harness = Harness::start(full()).await;
    let thread = harness
        .create(Some("cursor-strand".to_owned()))
        .await
        .thread;

    // The fake provider echoes nothing, so this prompt's `TurnStarted` never lands.
    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "one".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send the first prompt");
    harness
        .script
        .emit(AgentEvent::SessionExited {
            code: None,
            expected: false,
        })
        .await;
    // The session dying is the last chance to write that prompt down, so the transcript has it
    // before anything else happens. Reading the thread is also what resumes it (§6).
    harness
        .settle(
            thread,
            "the stranded prompt in the transcript",
            |projection| user_messages(projection) == ["one"],
        )
        .await;

    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "two".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send resumes the thread");
    let resumed = harness
        .script
        .calls()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            FakeCall::Send(turn, text) if text == "two" => Some(turn),
            _ => None,
        })
        .expect("the second prompt reached a provider");
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn: resumed,
            user_item: ItemId::new(),
        })
        .await;

    let projection = harness
        .settle(
            thread,
            "the second prompt in the transcript",
            |projection| user_messages(projection).len() == 2,
        )
        .await;
    assert_eq!(
        user_messages(&projection),
        ["one", "two"],
        "the log is the transcript: every prompt Claude was given is in it, in order",
    );
}

/// The user prompts the transcript holds, in order.
fn user_messages(projection: &ThreadProjection) -> Vec<&str> {
    projection
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::UserMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// BH: a stop that races the child's own exit answers `agent provider exited`, and `stop`
/// returned on it before settling anything — the turn stayed `Running` on a dead process, so
/// §2's tab spun forever and `attention()` never left `Working`.
#[tokio::test]
async fn stop_settles_the_turn_even_when_the_provider_will_not_stop() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    harness.script.stop_fails.store(true, Ordering::SeqCst);
    harness
        .manager
        .stop(thread)
        .await
        .expect("a provider that will not stop still settles the thread");

    let projection = harness.projection(thread).await;
    assert_eq!(projection.turn, TurnState::Interrupted(turn));
    assert_eq!(projection.session, SessionState::Stopped);
}

#[tokio::test]
async fn an_unavailable_provider_reports_the_terminal_fallback_hint() {
    let harness = Harness::start(full()).await;
    harness.script.unavailable.store(true, Ordering::SeqCst);
    let error = harness
        .manager
        .create(
            harness.worktree.clone(),
            AgentKind::Claude,
            None,
            PermissionMode::Ask,
            None,
            None,
        )
        .await
        .expect_err("an unavailable provider fails creation");

    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(error.message.contains("terminal fallback"), "{error:?}");
    assert!(harness.manager.summaries().is_empty());
}
