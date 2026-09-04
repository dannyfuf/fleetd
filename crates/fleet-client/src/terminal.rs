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

const FRAME_CAPACITY: usize = 128;

/// An attached terminal with a filtered stream of rendered frame updates.
#[derive(Debug)]
pub struct TerminalHandle {
    client: Client,
    terminal_id: TerminalId,
    frames: mpsc::Receiver<FrameUpdate>,
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
        let (frame_tx, frames) = mpsc::channel(FRAME_CAPACITY);
        let route_client = self.clone();
        let router = tokio::spawn(route_frames(route_client, terminal_id, events, frame_tx));

        if let Err(error) = self.attach_terminal(terminal_id, cols, rows).await {
            router.abort();
            return Err(error);
        }

        Ok(TerminalHandle {
            client: self.clone(),
            terminal_id,
            frames,
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

    /// Returns the terminal's frame channel.
    pub fn frames(&mut self) -> &mut mpsc::Receiver<FrameUpdate> {
        &mut self.frames
    }

    /// Waits for the next terminal frame update.
    pub async fn next_frame(&mut self) -> Option<FrameUpdate> {
        self.frames.recv().await
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
        self.frames.close();
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

async fn route_frames(
    client: Client,
    terminal_id: TerminalId,
    mut events: broadcast::Receiver<Event>,
    frames: mpsc::Sender<FrameUpdate>,
) {
    let mut received_full = false;
    loop {
        match events.recv().await {
            Ok(Event::TerminalFrame(frame)) if frame.terminal == terminal_id => {
                if !received_full && !frame.full {
                    continue;
                }
                received_full = true;
                if frames.send(frame).await.is_err() {
                    return;
                }
            }
            Ok(Event::TerminalExited { terminal, .. }) if terminal == terminal_id => return,
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
