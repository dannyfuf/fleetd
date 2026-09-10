//! The scripted in-process provider and the temporary daemon home every manager test is built
//! over.
//!
//! Nothing here spawns a real agent: [`FakeProvider`] records the commands the manager issues and
//! replays whatever events a test feeds it, so a lifecycle assertion is about the manager and never
//! about a child process. [`lifecycle`] holds the live-session tests, [`restart`] the ones that
//! rebuild a manager over the same database.

use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use fleet_core::{
    agents::{
        AbortReason, Attention, AttentionKind, Capabilities, GateAnswer, GateId, GateKind,
        ItemKind, PermissionChoice, Seq, SeqEvent, StreamKind, ThreadProjection, ToolKind,
        TurnOutcome, Usage,
    },
    ids::{ContextId, RepoId},
    model::{Context as ContextRecord, Repo, RepoHooks, Worktree},
    paths::FleetHome,
    state::default_state,
};
use fleet_proto::event::Event;
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

mod lifecycle;
mod restart;

const SETTLE: Duration = Duration::from_secs(5);

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

/// A worktree service over an empty home, for a test that never resolves a path through it.
fn empty_worktrees(home: &std::path::Path) -> Worktrees {
    let files = Arc::new(RealFiles::new(home.join("trash"), [home.to_path_buf()]));
    let config = Arc::new(ConfigStore::new(home, files.clone()));
    let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    Worktrees::new(
        config,
        state,
        Arc::new(JobManager::new(home)),
        &Adapters::system(files),
        sessions,
    )
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
            FleetHome::new(&home).agents_db_path(),
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

    /// Rebuilds the manager over the same database, as a daemon restart would.
    ///
    /// The boot repair is awaited rather than left to the task the constructor spawns: a restart
    /// no longer replays anything synchronously, so a test that asserts on settled orphans has to
    /// say when the background pass is done. Running it twice is harmless — a rebuild is
    /// idempotent and a thread already hydrated is no longer an orphan.
    async fn restart(&self) -> AgentSessionManager {
        let manager = AgentSessionManager::new_with_factory(
            FleetHome::new(&self.home).agents_db_path(),
            self.events.clone(),
            self.worktrees.clone(),
            None,
            factory(&self.script),
        );
        manager.clone().repair().await;
        manager
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
