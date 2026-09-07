//! The bridge between gpui's foreground executor and `fleet-client`'s tokio runtime.
//!
//! Commands cross to a Tokio runtime; daemon events return through a foreground channel.
//! Request admission, health checks, and connection attempts progress independently.

use std::{
    fmt,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use fleet_client::{Client, ensure_daemon};
use fleet_core::paths::FleetHome;
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

/// How often the bridge pings the daemon to decide whether it is still there.
const HEALTH_INTERVAL: Duration = Duration::from_secs(2);
/// How long a health ping may take before the daemon counts as gone.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
/// How many lines of `fleetd.log` the "will not start" surface shows (§3.12 B).
const LOG_TAIL_LINES: usize = 3;

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
        /// The last few lines of `~/.fleet/logs/fleetd.log`.
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
        let thread_events = event_tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("fleet-daemon-bridge".to_owned())
            .spawn(move || run_thread(home, command_rx, thread_events))
        {
            let _ignored = event_tx.try_send(BridgeEvent::ConnectFailed {
                message: format!("could not start the daemon bridge thread: {error}"),
                log_tail: Vec::new(),
                stale_socket: false,
            });
        }
        Self { commands, events }
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
    runtime.block_on(runtime::run(&home, &commands, &events));
}
