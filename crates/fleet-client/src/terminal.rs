//! Client-side terminal attachment and mirrored frame updates.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use fleet_core::ids::TerminalId;
use fleet_proto::{
    event::Event,
    request::RequestBody,
    terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand, WheelEvent},
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};

use crate::{api::Result, connection::Client};

const UPDATE_CAPACITY: usize = 128;

/// One terminal-specific update forwarded by an attachment handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalUpdate {
    /// A complete or dirty-row rendered frame.
    Frame(FrameUpdate),
    /// The PTY exited, optionally with a process exit code.
    Exited(Option<i32>),
    /// The PTY reported a new title.
    Title(String),
}

/// An attached terminal with a filtered stream of rendered frame updates.
#[derive(Debug)]
pub struct TerminalHandle {
    lease: Arc<AttachmentLease>,
    terminal_id: TerminalId,
    updates: mpsc::Receiver<TerminalUpdate>,
    router: JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LeaseKey {
    client: usize,
    terminal: TerminalId,
}

#[derive(Debug)]
struct AttachmentLease {
    client: Client,
    key: LeaseKey,
    detached: std::sync::atomic::AtomicBool,
}

#[derive(Debug)]
struct PendingAttach {
    lease: Arc<AttachmentLease>,
    router: Option<JoinHandle<()>>,
}

impl Client {
    /// Attaches to a terminal and prepares a frame stream whose first item is a full frame.
    pub async fn attach(
        &self,
        terminal_id: TerminalId,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalHandle> {
        let events = self.events();
        let (update_tx, updates) = mpsc::channel(UPDATE_CAPACITY);
        let route_client = self.clone();
        let router = tokio::spawn(route_updates(route_client, terminal_id, events, update_tx));
        let mut pending = PendingAttach {
            lease: attachment_lease(self, terminal_id),
            router: Some(router),
        };

        self.attach_terminal(terminal_id, cols, rows).await?;

        Ok(TerminalHandle {
            lease: pending.lease.clone(),
            terminal_id,
            updates,
            router: pending
                .router
                .take()
                .ok_or_else(|| fleet_proto::error::ProtoError {
                    kind: fleet_proto::error::ErrorKind::Unknown,
                    message: "terminal attachment router was unavailable".to_owned(),
                })?,
        })
    }
}

impl TerminalHandle {
    /// Returns the attached terminal identifier.
    #[must_use]
    pub const fn terminal_id(&self) -> TerminalId {
        self.terminal_id
    }

    /// Returns the terminal's frame, exit, and title update channel.
    pub fn updates(&mut self) -> &mut mpsc::Receiver<TerminalUpdate> {
        &mut self.updates
    }

    /// Waits for the next terminal-specific update.
    pub async fn next_update(&mut self) -> Option<TerminalUpdate> {
        self.updates.recv().await
    }

    /// Sends a semantic key event to the attached terminal.
    pub async fn send_key(&self, key: KeyEvent) -> Result<()> {
        self.lease.client.terminal_key(self.terminal_id, key).await
    }

    /// Sends raw, already encoded input bytes to the attached terminal.
    pub async fn send_input(&self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        self.lease
            .client
            .terminal_input(self.terminal_id, bytes.into())
            .await
    }

    /// Sends a semantic mouse event to the attached terminal.
    pub async fn send_mouse(&self, mouse: MouseEvent) -> Result<()> {
        self.lease
            .client
            .terminal_mouse(self.terminal_id, mouse)
            .await
    }

    /// Pastes text using the daemon's current bracketed-paste mode.
    pub async fn paste(&self, text: impl Into<String>) -> Result<()> {
        self.lease
            .client
            .paste_terminal(self.terminal_id, text.into())
            .await
    }

    /// Resizes the attached terminal grid.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.lease
            .client
            .resize_terminal(self.terminal_id, cols, rows)
            .await
    }

    /// Moves the attached terminal's server-side viewport.
    pub async fn scroll(&self, scroll: ScrollCommand) -> Result<()> {
        self.lease
            .client
            .scroll_terminal(self.terminal_id, scroll)
            .await
    }

    /// Enqueues a wheel event without waiting for acknowledgement.
    pub async fn wheel(&self, wheel: WheelEvent) -> Result<()> {
        self.lease
            .client
            .wheel_terminal(self.terminal_id, wheel)
            .await
    }

    /// Enqueues a viewport shortcut without waiting for acknowledgement.
    pub async fn scroll_or_key(&self, scroll: ScrollCommand, key: KeyEvent) -> Result<()> {
        self.lease
            .client
            .scroll_or_key_terminal(self.terminal_id, scroll, key)
            .await
    }

    /// Requests a complete replacement frame.
    pub async fn request_full_frame(&self) -> Result<()> {
        self.lease.client.request_full_frame(self.terminal_id).await
    }

    /// Explicitly detaches this handle and waits for daemon acknowledgement.
    pub async fn detach(mut self) -> Result<()> {
        self.router.abort();
        self.updates.close();
        if Arc::strong_count(&self.lease) == 1 {
            self.lease.detach().await
        } else {
            Ok(())
        }
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        self.router.abort();
    }
}

impl PendingAttach {
    fn abort_router(&mut self) {
        if let Some(router) = self.router.take() {
            router.abort();
        }
    }
}

impl Drop for PendingAttach {
    fn drop(&mut self) {
        self.abort_router();
    }
}

impl AttachmentLease {
    async fn detach(&self) -> Result<()> {
        self.detached
            .store(true, std::sync::atomic::Ordering::Release);
        self.client.detach_terminal(self.key.terminal).await
    }
}

impl Drop for AttachmentLease {
    fn drop(&mut self) {
        let mut leases = attachment_leases()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owns_entry = leases
            .get(&self.key)
            .is_some_and(|lease| std::ptr::eq(lease.as_ptr(), std::ptr::from_ref(self)));
        if owns_entry {
            leases.remove(&self.key);
        }
        drop(leases);
        if owns_entry && !self.detached.load(std::sync::atomic::Ordering::Acquire) {
            self.client.send_background(RequestBody::DetachTerminal {
                terminal: self.key.terminal,
            });
        }
    }
}

fn attachment_lease(client: &Client, terminal: TerminalId) -> Arc<AttachmentLease> {
    let key = LeaseKey {
        client: Arc::as_ptr(&client.inner) as usize,
        terminal,
    };
    let mut leases = attachment_leases()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(lease) = leases.get(&key).and_then(Weak::upgrade) {
        return lease;
    }
    let lease = Arc::new(AttachmentLease {
        client: client.clone(),
        key,
        detached: std::sync::atomic::AtomicBool::new(false),
    });
    leases.insert(key, Arc::downgrade(&lease));
    lease
}

fn attachment_leases() -> &'static Mutex<HashMap<LeaseKey, Weak<AttachmentLease>>> {
    static LEASES: OnceLock<Mutex<HashMap<LeaseKey, Weak<AttachmentLease>>>> = OnceLock::new();
    LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn route_updates(
    client: Client,
    terminal_id: TerminalId,
    mut events: broadcast::Receiver<Event>,
    updates: mpsc::Sender<TerminalUpdate>,
) {
    let mut received_full = false;
    loop {
        let event = tokio::select! {
            _ = updates.closed() => return,
            event = events.recv() => event,
        };
        match event {
            Ok(Event::TerminalFrame(frame)) if frame.terminal == terminal_id => {
                if !received_full && !frame.full {
                    continue;
                }
                received_full = true;
                if updates.send(TerminalUpdate::Frame(frame)).await.is_err() {
                    return;
                }
            }
            Ok(Event::TerminalExited { terminal, code }) if terminal == terminal_id => {
                let _result = updates.send(TerminalUpdate::Exited(code)).await;
                return;
            }
            Ok(Event::TerminalTitle { terminal, title }) if terminal == terminal_id => {
                if updates.send(TerminalUpdate::Title(title)).await.is_err() {
                    return;
                }
            }
            Ok(Event::DaemonShuttingDown) | Err(broadcast::error::RecvError::Closed) => return,
            Err(broadcast::error::RecvError::Lagged(_)) => {
                received_full = false;
                client.send_background(RequestBody::RequestFullFrame {
                    terminal: terminal_id,
                });
            }
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::atomic::AtomicBool, time::Duration};

    use fleet_proto::{
        PROTOCOL_VERSION,
        codec::FleetCodec,
        request::{Request, RequestBody},
        response::{Response, ResponseBody},
    };
    use futures_util::{SinkExt, StreamExt};
    use tempfile::TempDir;
    use tokio::{net::UnixListener, sync::oneshot};
    use tokio_util::codec::Framed;

    use super::*;

    type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

    #[tokio::test]
    async fn newer_lease_survives_stale_drop_during_attach() {
        let home = TempDir::new().unwrap();
        let listener = bind(home.path()).await;
        let (attach_seen_tx, attach_seen_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut transport = Framed::new(socket, FleetCodec::new());
            authenticate(&mut transport).await;

            let attach = next_request(&mut transport).await;
            assert!(matches!(
                attach.body,
                RequestBody::AttachTerminal {
                    terminal: TerminalId(31),
                    ..
                }
            ));
            attach_seen_tx.send(()).unwrap();

            let ping = next_request(&mut transport).await;
            assert_eq!(ping.body, RequestBody::DaemonPing);
            send_response(&mut transport, ping.id, ResponseBody::Pong).await;
            send_response(&mut transport, attach.id, ResponseBody::Ack).await;

            let detach = next_request(&mut transport).await;
            assert_eq!(
                detach.body,
                RequestBody::DetachTerminal {
                    terminal: TerminalId(31)
                }
            );
        });

        let client = Client::connect(home.path()).await.unwrap();
        let stale = attachment_lease(&client, TerminalId(31));
        let replacement = Arc::new(AttachmentLease {
            client: client.clone(),
            key: stale.key,
            detached: AtomicBool::new(false),
        });
        attachment_leases()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(stale.key, Arc::downgrade(&replacement));

        let attach_client = client.clone();
        let attaching =
            tokio::spawn(
                async move { attach_client.attach(TerminalId(31), 80, 24).await.unwrap() },
            );
        attach_seen_rx.await.unwrap();
        drop(replacement);
        drop(stale);
        client.daemon_ping().await.unwrap();

        let handle = attaching.await.unwrap();
        drop(handle);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("replacement attachment was detached by a stale lease")
            .unwrap();
    }

    async fn bind(home: &Path) -> UnixListener {
        UnixListener::bind(home.join("fleetd.sock")).unwrap()
    }

    async fn authenticate(transport: &mut ServerTransport) {
        let hello = next_request(transport).await;
        assert!(matches!(
            hello.body,
            RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                ..
            }
        ));
        send_response(
            transport,
            hello.id,
            ResponseBody::Hello {
                protocol: PROTOCOL_VERSION,
                server: "test-daemon".to_owned(),
            },
        )
        .await;
        let subscribe = next_request(transport).await;
        assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
        send_response(transport, subscribe.id, ResponseBody::Ack).await;
    }

    async fn next_request(transport: &mut ServerTransport) -> Request {
        transport.next().await.expect("connection closed").unwrap()
    }

    async fn send_response(transport: &mut ServerTransport, id: u64, body: ResponseBody) {
        transport
            .send(
                serde_json::to_value(Response {
                    id,
                    result: Ok(body),
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
}
