//! The bridge between gpui's foreground executor and `fleet-client`'s tokio runtime.
//!
//! Commands cross to a Tokio runtime; daemon events return through a foreground channel.
//! Request admission, health checks, and connection attempts progress independently.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use fleet_client::{Client, ensure_daemon};
use fleet_core::{
    agents::{
        AgentKind, AgentThreadSummary, GateAnswer, GateId, ModelSelection, PermissionMode, Seq,
        SeqEvent, ThreadId, UserInput,
    },
    config::Config,
    ids::WorktreeId,
    paths::FleetHome,
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    request::RequestBody,
    response::ResponseBody,
    snapshot::Snapshot,
};

use crate::state::{daemon_log_path, reconnect_backoff};

mod connection;
mod requests;
mod runtime;
#[cfg(test)]
mod tests;

/// How often the bridge probes daemon liveness.
const HEALTH_INTERVAL: Duration = Duration::from_secs(2);
/// How long a liveness probe may take before the daemon counts as gone.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
/// Identity-bearing pings stay rare; health pings cover ordinary liveness.
const IDENTITY_INTERVAL: Duration = Duration::from_secs(60);
/// How many lines of `fleetd.log` the "will not start" surface shows (§3.12 B).
const LOG_TAIL_LINES: usize = 3;
/// Requests waiting to enter the runtime before older fire-and-forget work is coalesced.
const COMMAND_CAPACITY: usize = 512;
/// UI events waiting for the foreground executor; backpressure is converted to a lag signal.
const EVENT_CAPACITY: usize = 1_024;

/// The effective settings consumed by the app without retaining the persisted config shape.
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    /// Effective terminal settings.
    pub terminal: fleet_core::config::TerminalConfig,
    /// Effective agent-attention notification settings.
    pub notifications: fleet_core::config::NotificationsConfig,
    /// Whether active daemon jobs require quit confirmation.
    pub warn_before_quit: bool,
    /// How long pull-request results remain fresh.
    pub pr_ttl: Duration,
}

impl EffectiveConfig {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self {
            terminal: config.terminal.clone(),
            notifications: config.ui.notifications.clone(),
            warn_before_quit: config.jobs.warn_before_quit,
            pr_ttl: Duration::from_secs(
                u64::try_from(config.github.pr_ttl_seconds)
                    .unwrap_or_default()
                    .max(1),
            ),
        }
    }
}

/// Everything the background thread tells the UI.
#[derive(Debug, Clone)]
pub enum BridgeEvent {
    /// A sequenced native-agent event ready for the foreground mirror.
    Agent {
        /// Owning thread.
        thread: ThreadId,
        /// Ordered event payload.
        event: SeqEvent,
    },
    /// A compact native-agent summary update.
    AgentSummary(AgentThreadSummary),
    /// The daemon answered and sent its first snapshot (§3.12 A resolved).
    Connected(Box<Snapshot>),
    /// The daemon could not be started (§3.12 B).
    ConnectFailed {
        /// The failure, verbatim.
        message: String,
        /// The last few lines of `~/.fleet/logs/fleetd.log`.
        log_tail: Vec<String>,
        /// Whether a stale socket file is the known cause.
        stale_socket: bool,
    },
    /// The daemon answered with a typed unsupported-protocol failure.
    ProtocolMismatch {
        /// The daemon's failure text, without presentation classification.
        message: String,
        /// The last few lines of `~/.fleet/logs/fleetd.log`.
        log_tail: Vec<String>,
    },
    /// A health ping failed: the daemon died while we were attached (§3.12 C).
    Disconnected {
        /// How many reconnect attempts have failed so far.
        attempt: u32,
    },
    /// The daemon answered again.
    Reconnected {
        /// True when the PID changed, so fleetd restarted and the PTYs did not survive.
        restarted: bool,
        /// The fresh snapshot.
        snapshot: Box<Snapshot>,
    },
    /// Effective app settings loaded on connection or a config response.
    EffectiveConfig(EffectiveConfig),
    /// Capabilities advertised by the daemon's mandatory Hello response.
    Capabilities(Vec<String>),
    /// A fire-and-forget command could not be admitted or delivered.
    MutationFailed {
        /// Stable user-facing failure detail.
        message: String,
    },
    /// An ordinary daemon event.
    Daemon(Box<Event>),
    /// A bridge or client buffer overflowed and state must be re-synchronized.
    ///
    /// Terminal frames are diffs: the rows changed inside the gap are never re-sent, so every
    /// mirror has to be re-primed from a full frame before it may accept another diff.
    EventsLagged {
        /// How many events the buffer dropped.
        dropped: u64,
    },
}

/// Typed native-agent commands accepted by the app bridge.
#[derive(Debug, Clone)]
pub enum BridgeCommand {
    /// List native-agent threads.
    AgentThreadList,
    /// Create and start a thread.
    AgentThreadCreate {
        /// Owning worktree.
        worktree: WorktreeId,
        /// Provider kind.
        provider: AgentKind,
        /// Optional model.
        model: Option<ModelSelection>,
        /// Initial permission mode.
        mode: PermissionMode,
        /// Optional provider cursor.
        resume_cursor: Option<String>,
        /// Optional display title.
        title: Option<String>,
    },
    /// Open a projection and event tail.
    AgentThreadOpen {
        /// Target thread.
        thread: ThreadId,
        /// Optional last-applied cursor.
        from_seq: Option<Seq>,
    },
    /// Close a client thread lease.
    AgentThreadClose {
        /// Target thread.
        thread: ThreadId,
    },
    /// Send or steer input.
    AgentSend {
        /// Target thread.
        thread: ThreadId,
        /// Text and attachments.
        input: UserInput,
    },
    /// Interrupt active work.
    AgentInterrupt {
        /// Target thread.
        thread: ThreadId,
    },
    /// Answer a provider gate.
    AgentRespond {
        /// Target thread.
        thread: ThreadId,
        /// Target gate.
        gate: GateId,
        /// Normalized answer.
        answer: GateAnswer,
    },
    /// Change permission mode.
    AgentSetMode {
        /// Target thread.
        thread: ThreadId,
        /// New mode.
        mode: PermissionMode,
    },
    /// Change the model.
    AgentSetModel {
        /// Target thread.
        thread: ThreadId,
        /// New model selection.
        model: ModelSelection,
    },
    /// Mark a sequence viewed.
    AgentMarkSeen {
        /// Target thread.
        thread: ThreadId,
        /// Viewed cursor.
        seq: Seq,
    },
    /// Stop the provider while retaining its transcript.
    AgentStop {
        /// Target thread.
        thread: ThreadId,
    },
}

impl From<BridgeCommand> for RequestBody {
    fn from(command: BridgeCommand) -> Self {
        match command {
            BridgeCommand::AgentThreadList => Self::AgentThreadList,
            BridgeCommand::AgentThreadCreate {
                worktree,
                provider,
                model,
                mode,
                resume_cursor,
                title,
            } => Self::AgentThreadCreate {
                worktree,
                provider,
                model,
                mode,
                resume_cursor,
                title,
            },
            BridgeCommand::AgentThreadOpen { thread, from_seq } => {
                Self::AgentThreadOpen { thread, from_seq }
            }
            BridgeCommand::AgentThreadClose { thread } => Self::AgentThreadClose { thread },
            BridgeCommand::AgentSend { thread, input } => Self::AgentSend { thread, input },
            BridgeCommand::AgentInterrupt { thread } => Self::AgentInterrupt { thread },
            BridgeCommand::AgentRespond {
                thread,
                gate,
                answer,
            } => Self::AgentRespond {
                thread,
                gate,
                answer,
            },
            BridgeCommand::AgentSetMode { thread, mode } => Self::AgentSetMode { thread, mode },
            BridgeCommand::AgentSetModel { thread, model } => Self::AgentSetModel { thread, model },
            BridgeCommand::AgentMarkSeen { thread, seq } => Self::AgentMarkSeen { thread, seq },
            BridgeCommand::AgentStop { thread } => Self::AgentStop { thread },
        }
    }
}

/// A command sent to the background thread.
enum Command {
    /// Send a request; the answer is forwarded when a channel was supplied.
    Request {
        body: Box<RequestBody>,
        reply: Option<Sender<Result<ResponseBody, ProtoError>>>,
    },
    /// Retry `ensure_daemon` now (`r` on either daemon surface).
    Reconnect,
    /// Stop the runtime; the app is quitting.
    Shutdown,
}

impl fmt::Debug for Command {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request { body, .. } => formatter
                .debug_struct("Request")
                .field("body", body)
                .finish_non_exhaustive(),
            Self::Reconnect => formatter.write_str("Reconnect"),
            Self::Shutdown => formatter.write_str("Shutdown"),
        }
    }
}

/// The handle the UI keeps. Cloning it is cheap and safe from any thread.
#[derive(Clone, Debug)]
pub struct Bridge {
    commands: Sender<Command>,
    events: Receiver<BridgeEvent>,
    event_tx: Sender<BridgeEvent>,
    resync_pending: Arc<AtomicBool>,
}

impl Bridge {
    /// Starts the runtime thread and immediately begins connecting.
    ///
    /// The thread outlives every UI object and stops on [`Bridge::shutdown`] or when the last
    /// handle is dropped. A thread that cannot start reports itself as a §3.12 B failure
    /// rather than leaving the app on the cold-start splash forever.
    #[must_use]
    pub fn start(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let (commands, command_rx) = async_channel::bounded(COMMAND_CAPACITY);
        let (event_tx, events) = async_channel::bounded(EVENT_CAPACITY);
        let resync_pending = Arc::new(AtomicBool::new(false));
        let thread_events = event_tx.clone();
        let thread_resync = resync_pending.clone();
        if let Err(error) = thread::Builder::new()
            .name("fleet-daemon-bridge".to_owned())
            .spawn(move || run_thread(home, command_rx, thread_events, thread_resync))
        {
            let _ignored = event_tx.try_send(BridgeEvent::ConnectFailed {
                message: format!("could not start the daemon bridge thread: {error}"),
                log_tail: Vec::new(),
                stale_socket: false,
            });
        }
        Self {
            commands,
            events,
            event_tx,
            resync_pending,
        }
    }

    #[cfg(test)]
    fn with_channels(
        commands: Sender<Command>,
        events: Receiver<BridgeEvent>,
        event_tx: Sender<BridgeEvent>,
        resync_pending: Arc<AtomicBool>,
    ) -> Self {
        Self {
            commands,
            events,
            event_tx,
            resync_pending,
        }
    }

    /// The stream of daemon events. The shell drains it in one `cx.spawn` loop.
    #[must_use]
    pub fn events(&self) -> Receiver<BridgeEvent> {
        self.events.clone()
    }

    /// Sends a request and forgets about the answer.
    ///
    /// Use `request` when the caller needs acknowledgement or a daemon rejection.
    pub fn send(&self, body: RequestBody) {
        let command = Command::Request {
            body: Box::new(body),
            reply: None,
        };
        match self.commands.try_send(command) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Full(_)) => {
                self.resync_pending.store(true, Ordering::Release);
                self.report_mutation_failure("the Fleet daemon bridge queue was saturated");
            }
            Err(async_channel::TrySendError::Closed(_)) => {
                self.report_mutation_failure("the Fleet daemon bridge is closed");
            }
        }
    }

    /// Sends a typed native-agent command and forgets its response.
    pub fn send_agent(&self, command: BridgeCommand) {
        self.send(command.into());
    }

    /// Sends a typed native-agent command and returns its correlated response channel.
    #[must_use]
    pub fn request_agent(
        &self,
        command: BridgeCommand,
    ) -> Receiver<Result<ResponseBody, ProtoError>> {
        self.request(command.into())
    }

    /// Sends a request and returns the channel its single answer arrives on.
    ///
    /// Board replies follow the same path as worktree paths and PR slices: no typed
    /// Board bridge events are emitted. The caller receives ResponseBody::Board
    /// or ResponseBody::Card and applies AppState::apply_board_view / apply_card.
    /// screens::board::ensure_current implements EnsureBoard with context/generation guards.
    ///
    /// ```ignore
    /// let reply = bridge.request(RequestBody::GetSnapshot);
    /// cx.spawn(async move |_, _| {
    ///     if let Ok(Ok(ResponseBody::Snapshot(snapshot))) = reply.recv().await { /* … */ }
    /// })
    /// .detach();
    /// ```
    #[must_use]
    pub fn request(&self, body: RequestBody) -> Receiver<Result<ResponseBody, ProtoError>> {
        let (reply, answer) = async_channel::bounded(1);
        match self.commands.try_send(Command::Request {
            body: Box::new(body),
            reply: Some(reply.clone()),
        }) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Full(_)) => {
                let _ignored =
                    reply.try_send(Err(offline("the Fleet daemon bridge queue was saturated")));
            }
            Err(async_channel::TrySendError::Closed(_)) => {
                let _ignored = reply.try_send(Err(offline("the Fleet daemon bridge is closed")));
            }
        }
        answer
    }

    /// Retries starting or reaching the daemon now.
    pub fn reconnect(&self) {
        self.send_control(Command::Reconnect);
    }

    /// Stops the runtime thread.
    pub fn shutdown(&self) {
        self.send_control(Command::Shutdown);
    }

    fn send_control(&self, command: Command) {
        if let Err(async_channel::TrySendError::Full(command)) = self.commands.try_send(command)
            && let Ok(Some(displaced)) = self.commands.force_send(command)
        {
            self.reject(displaced, "the Fleet daemon bridge queue was saturated");
        }
    }

    fn reject(&self, command: Command, message: &str) {
        if let Command::Request { reply, .. } = command {
            if let Some(reply) = reply {
                let _ignored = reply.try_send(Err(offline(message)));
            } else {
                self.report_mutation_failure(message);
            }
        }
    }

    fn report_mutation_failure(&self, message: &str) {
        publish_mutation_failure(&self.event_tx, message);
    }
}

fn offline(message: &str) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: message.to_owned(),
    }
}

fn publish_mutation_failure(events: &Sender<BridgeEvent>, message: &str) {
    let _ignored = events.try_send(BridgeEvent::MutationFailed {
        message: message.to_owned(),
    });
}

fn run_thread(
    home: PathBuf,
    commands: Receiver<Command>,
    events: Sender<BridgeEvent>,
    resync_pending: Arc<AtomicBool>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ignored = events.try_send(BridgeEvent::ConnectFailed {
                message: format!("could not start the client runtime: {error}"),
                log_tail: Vec::new(),
                stale_socket: false,
            });
            return;
        }
    };
    runtime.block_on(runtime::run(&home, &commands, &events, &resync_pending));
}
