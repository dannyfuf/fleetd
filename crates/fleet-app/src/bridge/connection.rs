use super::*;
use fleet_client::{ConnectError, SpawnError};
use tokio::sync::broadcast;

/// One connected daemon plus the task forwarding its events.
pub(super) struct Link {
    pub(super) client: Client,
    pub(super) pid: u32,
    pub(super) boot_id: Option<String>,
    forwarder: Forwarder,
}

pub(super) struct Forwarder {
    pending: Option<broadcast::Receiver<Event>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Forwarder {
    pub(super) fn pending(source: broadcast::Receiver<Event>) -> Self {
        Self {
            pending: Some(source),
            task: None,
        }
    }

    pub(super) fn start(&mut self, events: Sender<BridgeEvent>) {
        if let Some(source) = self.pending.take() {
            self.task = Some(spawn_forwarder(source, events));
        }
    }
}

impl Link {
    pub(super) fn start_forwarding(&mut self, events: Sender<BridgeEvent>) {
        self.forwarder.start(events);
    }
}

/// Why a connection attempt failed, in the shape §3.12 B renders.
pub(super) struct Failure {
    message: String,
    log_tail: Vec<String>,
    stale_socket: bool,
    cause: FailureCause,
}

#[derive(Clone, Copy)]
enum FailureCause {
    Unavailable,
    ProtocolMismatch,
}

impl Failure {
    pub(super) fn into_event(self) -> BridgeEvent {
        match self.cause {
            FailureCause::Unavailable => BridgeEvent::ConnectFailed {
                message: self.message,
                log_tail: self.log_tail,
                stale_socket: self.stale_socket,
            },
            FailureCause::ProtocolMismatch => BridgeEvent::ProtocolMismatch {
                message: self.message,
                log_tail: self.log_tail,
            },
        }
    }
}

fn spawn_failure_cause(error: &SpawnError) -> FailureCause {
    match error {
        SpawnError::Connect(ConnectError::Protocol(error))
            if error.kind == ErrorKind::Unsupported =>
        {
            FailureCause::ProtocolMismatch
        }
        _ => FailureCause::Unavailable,
    }
}

#[derive(Debug)]
pub(super) enum HealthCheckError {
    Timeout,
    Request(ProtoError),
}

impl std::fmt::Display for HealthCheckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(formatter, "timed out after {HEALTH_TIMEOUT:?}"),
            Self::Request(error) => write!(formatter, "{error}"),
        }
    }
}

pub(super) async fn check_health(client: &Client) -> Result<(), HealthCheckError> {
    tokio::time::timeout(HEALTH_TIMEOUT, client.daemon_ping())
        .await
        .map_err(|_| HealthCheckError::Timeout)?
        .map_err(HealthCheckError::Request)
}

pub(super) async fn daemon_identity(client: &Client) -> Option<(u32, String)> {
    client.daemon_ping().await.ok()?;
    client.daemon_identity()
}

/// Connects, spawning fleetd when the socket is dead, and reads the first snapshot.
pub(super) async fn open(
    home: &Path,
    events: &Sender<BridgeEvent>,
) -> Result<(Link, Snapshot), Failure> {
    let client = match ensure_daemon(home, None).await {
        Ok(client) => client,
        Err(error) => {
            let cause = spawn_failure_cause(&error);
            return Err(Failure {
                message: error.to_string(),
                log_tail: log_tail(home).await,
                stale_socket: FleetHome::new(home).socket_path().exists(),
                cause,
            });
        }
    };
    if events
        .send(BridgeEvent::Capabilities(client.capabilities()))
        .await
        .is_err()
    {
        return Err(Failure {
            message: "the app stopped receiving daemon connection metadata".to_owned(),
            log_tail: Vec::new(),
            stale_socket: false,
            cause: FailureCause::Unavailable,
        });
    }
    let forwarder = Forwarder::pending(client.events());
    if let Ok(config) = client.get_config().await {
        let _ = events
            .send(BridgeEvent::EffectiveConfig(EffectiveConfig::from_config(
                &config,
            )))
            .await;
    }
    match client.get_snapshot().await {
        Ok(snapshot) => {
            let pid = snapshot.daemon.pid;
            Ok((
                Link {
                    client,
                    pid,
                    boot_id: None,
                    forwarder,
                },
                snapshot,
            ))
        }
        Err(error) => Err(Failure {
            message: error.message,
            log_tail: log_tail(home).await,
            stale_socket: false,
            cause: if error.kind == ErrorKind::Unsupported {
                FailureCause::ProtocolMismatch
            } else {
                FailureCause::Unavailable
            },
        }),
    }
}

/// Forward ordered events and expose broadcast lag so Shell can request full terminal frames.
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

const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// Read only a bounded suffix. Discard an incomplete first line and retain the existing
/// nonempty-line policy; one pathological line cannot allocate the entire daemon log.
pub(super) async fn log_tail(home: &Path) -> Vec<String> {
    let path = daemon_log_path(home);
    match tokio::task::spawn_blocking(move || read_log_tail(&path)).await {
        Ok(Ok(lines)) => lines,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Ok(Err(error)) => {
            tracing::warn!(%error, "could not read daemon log tail");
            Vec::new()
        }
        Err(error) => {
            tracing::warn!(%error, "daemon log reader stopped");
            Vec::new()
        }
    }
}

fn read_log_tail(path: &Path) -> std::io::Result<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let offset = length.saturating_sub(LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(length.min(LOG_TAIL_BYTES) as usize);
    file.take(LOG_TAIL_BYTES).read_to_end(&mut bytes)?;
    let start = if offset == 0 {
        0
    } else {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1)
    };
    let contents = std::str::from_utf8(&bytes[start..])
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mut lines: Vec<_> = contents
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(LOG_TAIL_LINES)
        .map(str::to_owned)
        .collect();
    lines.reverse();
    Ok(lines)
}
