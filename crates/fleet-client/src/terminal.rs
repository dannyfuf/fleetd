//! Client-side terminal attachment and mirrored frame updates.

use fleet_core::ids::TerminalId;
use fleet_proto::{
    event::Event,
    request::RequestBody,
    terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand},
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
    client: Client,
    terminal_id: TerminalId,
    updates: mpsc::Receiver<TerminalUpdate>,
    router: JoinHandle<()>,
    detached: bool,
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

        if let Err(error) = self.attach_terminal(terminal_id, cols, rows).await {
            router.abort();
            return Err(error);
        }

        Ok(TerminalHandle {
            client: self.clone(),
            terminal_id,
            updates,
            router,
            detached: false,
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
        self.client.terminal_key(self.terminal_id, key).await
    }

    /// Sends raw, already encoded input bytes to the attached terminal.
    pub async fn send_input(&self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        self.client
            .terminal_input(self.terminal_id, bytes.into())
            .await
    }

    /// Sends a semantic mouse event to the attached terminal.
    pub async fn send_mouse(&self, mouse: MouseEvent) -> Result<()> {
        self.client.terminal_mouse(self.terminal_id, mouse).await
    }

    /// Pastes text using the daemon's current bracketed-paste mode.
    pub async fn paste(&self, text: impl Into<String>) -> Result<()> {
        self.client
            .paste_terminal(self.terminal_id, text.into())
            .await
    }

    /// Resizes the attached terminal grid.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.client
            .resize_terminal(self.terminal_id, cols, rows)
            .await
    }

    /// Moves the attached terminal's server-side viewport.
    pub async fn scroll(&self, scroll: ScrollCommand) -> Result<()> {
        self.client.scroll_terminal(self.terminal_id, scroll).await
    }

    /// Requests a complete replacement frame.
    pub async fn request_full_frame(&self) -> Result<()> {
        self.client.request_full_frame(self.terminal_id).await
    }

    /// Explicitly detaches this handle and waits for daemon acknowledgement.
    pub async fn detach(mut self) -> Result<()> {
        self.router.abort();
        self.updates.close();
        self.detached = true;
        self.client.detach_terminal(self.terminal_id).await
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        self.router.abort();
        if !self.detached {
            self.client.send_background(RequestBody::DetachTerminal {
                terminal: self.terminal_id,
            });
        }
    }
}

async fn route_updates(
    client: Client,
    terminal_id: TerminalId,
    mut events: broadcast::Receiver<Event>,
    updates: mpsc::Sender<TerminalUpdate>,
) {
    let mut received_full = false;
    loop {
        match events.recv().await {
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
