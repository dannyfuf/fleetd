use super::*;
use tokio::sync::broadcast;

/// One connected daemon plus the task forwarding its events.
pub(super) struct Link {
    pub(super) client: Client,
    pub(super) pid: u32,
    forwarder: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.forwarder.abort();
    }
}

/// Why a connection attempt failed, in the shape §3.12 B renders.
pub(super) struct Failure {
    message: String,
    log_tail: Vec<String>,
    stale_socket: bool,
}

impl Failure {
    pub(super) fn into_event(self) -> BridgeEvent {
        BridgeEvent::ConnectFailed {
            message: self.message,
            log_tail: self.log_tail,
            stale_socket: self.stale_socket,
        }
    }
}

pub(super) async fn is_alive(client: &Client) -> bool {
    matches!(
        tokio::time::timeout(HEALTH_TIMEOUT, client.daemon_ping()).await,
        Ok(Ok(()))
    )
}

/// Connects, spawning fleetd when the socket is dead, and reads the first snapshot.
pub(super) async fn open(
    home: &Path,
    events: &Sender<BridgeEvent>,
) -> Result<(Link, Snapshot), Failure> {
    let client = match ensure_daemon(home, None).await {
        Ok(client) => client,
        Err(error) => {
            return Err(Failure {
                message: error.to_string(),
                log_tail: log_tail(home).await,
                stale_socket: FleetHome::new(home).socket_path().exists(),
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
