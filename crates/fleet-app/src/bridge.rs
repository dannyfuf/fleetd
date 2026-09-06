//! The bridge between gpui's foreground executor and `fleet-client`'s tokio runtime.
//!
//! gpui owns the main thread and is not a tokio runtime; `fleet-client` is tokio through and
//! through. [`Bridge::start`] therefore parks a multi-threaded tokio runtime on **one**
//! background thread, runs `ensure_daemon` and the connection actor there, and exchanges two
//! `async-channel` streams with the UI:
//!
//! ```text
//!   gpui foreground                    background thread (tokio)
//!   ──────────────                     ─────────────────────────
//!   Bridge::request(body) ──command──▶ Client::request(body) ─socket─▶ fleetd
//!   AppState  ◀──BridgeEvent────────── daemon events, health pings
//! ```
//!
//! `async-channel` is the only channel type both sides can await, so neither thread blocks the
//! other. The shell drains [`Bridge::events`] in one `cx.spawn` loop and applies each event to
//! the [`crate::state::AppState`] entity; nothing else in the app talks to the daemon.
//!
//! The bridge also answers the §3.12 liveness question. `fleet-client` reconnects its socket
//! transparently, so "is fleetd alive" is decided by a health ping every [`HEALTH_INTERVAL`].
//! When a ping fails the bridge reports [`BridgeEvent::Disconnected`] and retries on the
//! [`crate::state::reconnect_backoff`] schedule; when the daemon answers again it compares the
//! daemon PID with the one it had, which is how the app knows whether to show the mandatory
//! "terminal sessions did not survive" sentence (§3.12 D-17) or a plain `reconnected`.

use std::{
    fmt,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use fleet_client::{Client, ensure_daemon};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    paths::socket_path,
    request::RequestBody,
    response::ResponseBody,
    snapshot::Snapshot,
};
use tokio::sync::broadcast;

use crate::state::{daemon_log_path, reconnect_backoff};

/// How often the bridge pings the daemon to decide whether it is still there.
pub const HEALTH_INTERVAL: Duration = Duration::from_secs(2);
/// How long a health ping may take before the daemon counts as gone.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
/// How many lines of `fleetd.log` the "will not start" surface shows (§3.12 B).
pub const LOG_TAIL_LINES: usize = 3;

/// The name this client reports in the protocol handshake.
const CLIENT_NAME: &str = concat!("fleet-app/", env!("CARGO_PKG_VERSION"));

/// Everything the background thread tells the UI.
#[derive(Debug, Clone)]
pub enum BridgeEvent {
    /// The daemon answered and sent its first snapshot (§3.12 A resolved).
    Connected(Box<Snapshot>),
    /// The daemon could not be started (§3.12 B).
    ConnectFailed {
        /// The failure, verbatim.
        message: String,
        /// The last [`LOG_TAIL_LINES`] lines of `~/.fleet/logs/fleetd.log`.
        log_tail: Vec<String>,
        /// Whether a stale socket file is the known cause.
        stale_socket: bool,
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
    /// Effective terminal settings loaded on connection or a config response.
    TerminalConfig(fleet_core::config::TerminalConfig),
    /// Effective agent-finished notification settings.
    NotificationConfig(fleet_core::config::NotificationsConfig),
    /// An ordinary daemon event.
    Daemon(Box<Event>),
    /// The client's broadcast buffer overflowed and events were dropped.
    ///
    /// Terminal frames are diffs: the rows changed inside the gap are never re-sent, so every
    /// mirror has to be re-primed from a full frame before it may accept another diff.
    EventsLagged {
        /// How many events the buffer dropped.
        dropped: u64,
    },
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
    home: PathBuf,
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
        let (commands, command_rx) = async_channel::unbounded();
        let (event_tx, events) = async_channel::unbounded();
        let thread_home = home.clone();
        let thread_events = event_tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("fleet-daemon-bridge".to_owned())
            .spawn(move || run_thread(thread_home, command_rx, thread_events))
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
            home,
        }
    }

    /// `$FLEET_HOME`.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The stream of daemon events. The shell drains it in one `cx.spawn` loop.
    #[must_use]
    pub fn events(&self) -> Receiver<BridgeEvent> {
        self.events.clone()
    }

    /// Sends a request and forgets about the answer.
    ///
    /// This is the right call for anything whose outcome arrives as an event anyway — every
    /// mutation does, because the daemon re-broadcasts its snapshot.
    pub fn send(&self, body: RequestBody) {
        let _ignored = self.commands.try_send(Command::Request {
            body: Box::new(body),
            reply: None,
        });
    }

    /// Sends a request and returns the channel its single answer arrives on.
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
        if self
            .commands
            .try_send(Command::Request {
                body: Box::new(body),
                reply: Some(reply.clone()),
            })
            .is_err()
        {
            let _ignored = reply.try_send(Err(offline("the Fleet daemon bridge is closed")));
        }
        answer
    }

    /// Retries starting or reaching the daemon now.
    pub fn reconnect(&self) {
        let _ignored = self.commands.try_send(Command::Reconnect);
    }

    /// Stops the runtime thread.
    pub fn shutdown(&self) {
        let _ignored = self.commands.try_send(Command::Shutdown);
    }
}

fn offline(message: &str) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: message.to_owned(),
    }
}

fn run_thread(home: PathBuf, commands: Receiver<Command>, events: Sender<BridgeEvent>) {
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
    runtime.block_on(run(&home, &commands, &events));
}

/// One connected daemon plus the task forwarding its events.
struct Link {
    client: Client,
    pid: u32,
    forwarder: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.forwarder.abort();
    }
}

/// Why a connection attempt failed, in the shape §3.12 B renders.
struct Failure {
    message: String,
    log_tail: Vec<String>,
    stale_socket: bool,
}

/// What the loop is doing while it is not connected.
enum Backoff {
    /// Connected, or waiting for the user to press `r` — nothing is scheduled.
    Idle,
    /// §3.12 C: the daemon died while attached and the loop is retrying on the backoff.
    Reconnecting {
        /// The pid of the daemon that died, so a restart can be told from a reconnect.
        previous_pid: u32,
        /// How many attempts have already failed.
        attempt: u32,
    },
}

/// Resolves after `delay`, or never when there is nothing scheduled.
///
/// A `select!` arm needs a future either way; `pending()` is how "this arm is disabled this
/// round" is spelled without duplicating the whole loop.
async fn after(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending().await,
    }
}

async fn run(home: &Path, commands: &Receiver<Command>, events: &Sender<BridgeEvent>) {
    let mut link = match open(home, events).await {
        Ok((link, snapshot)) => {
            if events
                .send(BridgeEvent::Connected(Box::new(snapshot)))
                .await
                .is_err()
            {
                return;
            }
            Some(link)
        }
        Err(failure) => {
            if events.send(failure.into_event()).await.is_err() {
                return;
            }
            None
        }
    };

    let mut ticker = tokio::time::interval(HEALTH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The reconnect is a *state*, not an inline loop. Sleeping through the backoff inside the
    // ticker arm stopped `commands.recv()` from being polled at all, so `Shutdown` and every
    // pending `Request` piled up unanswered — and `ctrl-shift-q`, which awaits its reply
    // before quitting, hung the window for as long as fleetd stayed down.
    let mut backoff = Backoff::Idle;

    loop {
        let retry_in = match &backoff {
            Backoff::Reconnecting { attempt, .. } => Some(reconnect_backoff(*attempt)),
            Backoff::Idle => None,
        };

        tokio::select! {
            command = commands.recv() => match command {
                Ok(Command::Request { body, reply }) => {
                    // With no link this answers `offline` immediately, which is what lets a
                    // caller awaiting its reply make progress while the daemon is down.
                    match reply {
                        Some(reply) => dispatch(
                            link.as_ref().map(|link| link.client.clone()),
                            *body,
                            reply,
                            events.clone(),
                        ),
                        None => {
                            // Fire-and-forget mutations still have an ordering contract. In
                            // particular, one TerminalInput request is one key press; spawning
                            // each request before it reaches the client's FIFO can scramble a
                            // fast typist's bytes. Await only the enqueue, never the response.
                            if let Some(client) = link.as_ref().map(|link| link.client.clone()) {
                                let _ignored = client.request_background(*body).await;
                            }
                        }
                    }
                }
                Ok(Command::Reconnect) => {
                    if link.is_none() {
                        link = match open(home, events).await {
                            Ok((link, snapshot)) => {
                                if events.send(BridgeEvent::Connected(Box::new(snapshot))).await.is_err() {
                                    return;
                                }
                                backoff = Backoff::Idle;
                                Some(link)
                            }
                            Err(failure) => {
                                if events.send(failure.into_event()).await.is_err() {
                                    return;
                                }
                                None
                            }
                        };
                    }
                }
                Ok(Command::Shutdown) | Err(_) => return,
            },
            () = after(retry_in) => {
                let Backoff::Reconnecting { previous_pid, attempt } = backoff else {
                    continue;
                };
                match open(home, events).await {
                    Ok((recovered, snapshot)) => {
                        let restarted = recovered.pid != previous_pid;
                        if events
                            .send(BridgeEvent::Reconnected {
                                restarted,
                                snapshot: Box::new(snapshot),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                        link = Some(recovered);
                        backoff = Backoff::Idle;
                    }
                    Err(_) => {
                        let attempt = attempt.saturating_add(1);
                        if events.send(BridgeEvent::Disconnected { attempt }).await.is_err() {
                            return;
                        }
                        backoff = Backoff::Reconnecting { previous_pid, attempt };
                    }
                }
            },
            _ = ticker.tick() => {
                let lost_pid = match link.as_ref() {
                    Some(current) => {
                        if is_alive(&current.client).await { None } else { Some(current.pid) }
                    }
                    None => None,
                };
                if let Some(previous) = lost_pid {
                    drop(link.take());
                    if events.send(BridgeEvent::Disconnected { attempt: 0 }).await.is_err() {
                        return;
                    }
                    backoff = Backoff::Reconnecting { previous_pid: previous, attempt: 0 };
                }
            }
        }
    }
}

impl Failure {
    fn into_event(self) -> BridgeEvent {
        BridgeEvent::ConnectFailed {
            message: self.message,
            log_tail: self.log_tail,
            stale_socket: self.stale_socket,
        }
    }
}

/// Sends one request on the runtime without blocking the command loop.
fn dispatch(
    client: Option<Client>,
    body: RequestBody,
    reply: Sender<Result<ResponseBody, ProtoError>>,
    events: Sender<BridgeEvent>,
) {
    let Some(client) = client else {
        let _ignored = reply.try_send(Err(offline("the Fleet daemon is not connected")));
        return;
    };
    tokio::spawn(async move {
        let result = client.request(body).await;
        if let Ok(ResponseBody::Config(config)) = &result {
            let _ = events
                .send(BridgeEvent::TerminalConfig(config.terminal.clone()))
                .await;
            let _ = events
                .send(BridgeEvent::NotificationConfig(
                    config.ui.notifications.clone(),
                ))
                .await;
        }
        let _ignored = reply.send(result).await;
    });
}

async fn is_alive(client: &Client) -> bool {
    matches!(
        tokio::time::timeout(HEALTH_TIMEOUT, client.daemon_ping()).await,
        Ok(Ok(()))
    )
}

/// Connects, spawning fleetd when the socket is dead, and reads the first snapshot.
async fn open(home: &Path, events: &Sender<BridgeEvent>) -> Result<(Link, Snapshot), Failure> {
    let client = match ensure_daemon(home, None).await {
        Ok(client) => client,
        Err(error) => {
            return Err(Failure {
                message: error.to_string(),
                log_tail: log_tail(home).await,
                stale_socket: socket_path(home).exists(),
            });
        }
    };
    let _ignored = client.hello(CLIENT_NAME).await;
    let forwarder = spawn_forwarder(client.events(), events.clone());
    if let Ok(config) = client.get_config().await {
        let _ = events
            .send(BridgeEvent::TerminalConfig(config.terminal.clone()))
            .await;
        let _ = events
            .send(BridgeEvent::NotificationConfig(config.ui.notifications))
            .await;
    }
    match client.get_snapshot().await {
        Ok(snapshot) => {
            let pid = snapshot.daemon.pid;
            Ok((
                Link {
                    client,
                    pid,
                    forwarder,
                },
                snapshot,
            ))
        }
        Err(error) => {
            forwarder.abort();
            Err(Failure {
                message: error.message,
                log_tail: log_tail(home).await,
                stale_socket: false,
            })
        }
    }
}

/// Forwards every daemon event into the UI channel.
///
/// A lagging receiver is impossible on the UI side (that channel is unbounded), so the only lag
/// comes from the client's broadcast buffer. That lag is **not** self-healing: a snapshot event
/// resynchronises the snapshot mirror, but a terminal grid is rebuilt from diffs, and the rows
/// that changed inside the gap are never sent again. `fleet-app` consumes raw
/// `Event::TerminalFrame`s rather than `fleet-client`'s `TerminalHandle`, so nothing else asks
/// for a full frame on its behalf — [`BridgeEvent::EventsLagged`] is what makes the shell do it.
fn spawn_forwarder(
    mut source: broadcast::Receiver<Event>,
    events: Sender<BridgeEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match source.recv().await {
                Ok(event) => {
                    if events
                        .send(BridgeEvent::Daemon(Box::new(event)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    if events
                        .send(BridgeEvent::EventsLagged { dropped })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    })
}

/// The last [`LOG_TAIL_LINES`] non-empty lines of `fleetd.log`, for §3.12 B.
async fn log_tail(home: &Path) -> Vec<String> {
    let path = daemon_log_path(home);
    let Ok(contents) = tokio::fs::read_to_string(&path).await else {
        return Vec::new();
    };
    let mut lines: Vec<String> = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(LOG_TAIL_LINES)
        .map(str::to_owned)
        .collect();
    lines.reverse();
    lines
}
