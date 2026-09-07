use super::{Result, expect_ack, unexpected};
use crate::Client;
use fleet_core::{
    ids::{SessionId, TerminalId},
    sessions::{Session, Terminal},
};
use fleet_proto::{
    request::RequestBody,
    response::ResponseBody,
    terminal::{KeyEvent, MouseEvent, ScrollCommand, WheelEvent},
};

impl Client {
    /// Adds a terminal to a session.
    pub async fn new_terminal(
        &self,
        session: SessionId,
        name: impl Into<String>,
        command: impl Into<String>,
        cwd: impl Into<String>,
    ) -> Result<Terminal> {
        match self
            .request(RequestBody::NewTerminal {
                session,
                name: name.into(),
                command: command.into(),
                cwd: cwd.into(),
            })
            .await?
        {
            ResponseBody::Terminal(terminal) => Ok(terminal),
            response => Err(unexpected("new_terminal", response)),
        }
    }

    /// Closes a terminal.
    pub async fn close_terminal(&self, terminal: TerminalId) -> Result<()> {
        expect_ack(
            "close_terminal",
            self.request(RequestBody::CloseTerminal { terminal })
                .await?,
        )
    }

    /// Restarts an exited terminal and returns its updated metadata.
    pub async fn restart_terminal(&self, terminal: TerminalId) -> Result<Terminal> {
        match self
            .request(RequestBody::RestartTerminal { terminal })
            .await?
        {
            ResponseBody::Terminal(terminal) => Ok(terminal),
            response => Err(unexpected("restart_terminal", response)),
        }
    }

    /// Renames a terminal and returns its updated metadata.
    pub async fn rename_terminal(
        &self,
        terminal: TerminalId,
        name: impl Into<String>,
    ) -> Result<Terminal> {
        match self
            .request(RequestBody::RenameTerminal {
                terminal,
                name: name.into(),
            })
            .await?
        {
            ResponseBody::Terminal(terminal) => Ok(terminal),
            response => Err(unexpected("rename_terminal", response)),
        }
    }

    /// Selects a session's active terminal and returns the updated session.
    pub async fn select_terminal(
        &self,
        session: SessionId,
        terminal: TerminalId,
    ) -> Result<Session> {
        match self
            .request(RequestBody::SelectTerminal { session, terminal })
            .await?
        {
            ResponseBody::Session(session) => Ok(session),
            response => Err(unexpected("select_terminal", response)),
        }
    }

    /// Attaches this connection to a terminal.
    pub async fn attach_terminal(&self, terminal: TerminalId, cols: u16, rows: u16) -> Result<()> {
        expect_ack(
            "attach_terminal",
            self.request(RequestBody::AttachTerminal {
                terminal,
                cols,
                rows,
            })
            .await?,
        )
    }

    /// Detaches this connection from a terminal.
    pub async fn detach_terminal(&self, terminal: TerminalId) -> Result<()> {
        expect_ack(
            "detach_terminal",
            self.request(RequestBody::DetachTerminal { terminal })
                .await?,
        )
    }

    /// Writes already encoded bytes to a terminal PTY.
    pub async fn terminal_input(&self, terminal: TerminalId, bytes: Vec<u8>) -> Result<()> {
        expect_ack(
            "terminal_input",
            self.request(RequestBody::TerminalInput { terminal, bytes })
                .await?,
        )
    }

    /// Sends a semantic key event to a terminal.
    pub async fn terminal_key(&self, terminal: TerminalId, key: KeyEvent) -> Result<()> {
        expect_ack(
            "terminal_key",
            self.request(RequestBody::TerminalKey { terminal, key })
                .await?,
        )
    }

    /// Sends a semantic mouse event to a terminal.
    pub async fn terminal_mouse(&self, terminal: TerminalId, mouse: MouseEvent) -> Result<()> {
        expect_ack(
            "terminal_mouse",
            self.request(RequestBody::TerminalMouse { terminal, mouse })
                .await?,
        )
    }

    /// Resizes a terminal PTY.
    pub async fn resize_terminal(&self, terminal: TerminalId, cols: u16, rows: u16) -> Result<()> {
        expect_ack(
            "resize_terminal",
            self.request(RequestBody::ResizeTerminal {
                terminal,
                cols,
                rows,
            })
            .await?,
        )
    }

    /// Moves a terminal's server-side scrollback viewport.
    pub async fn scroll_terminal(&self, terminal: TerminalId, scroll: ScrollCommand) -> Result<()> {
        expect_ack(
            "scroll_terminal",
            self.request(RequestBody::ScrollTerminal { terminal, scroll })
                .await?,
        )
    }

    /// Enqueues wheel input in order without waiting for the daemon's response.
    pub async fn wheel_terminal(&self, terminal: TerminalId, wheel: WheelEvent) -> Result<()> {
        self.request_background(RequestBody::WheelTerminal { terminal, wheel })
            .await
    }

    /// Enqueues a viewport shortcut in order without waiting for acknowledgement.
    pub async fn scroll_or_key_terminal(
        &self,
        terminal: TerminalId,
        scroll: ScrollCommand,
        key: KeyEvent,
    ) -> Result<()> {
        self.request_background(RequestBody::ScrollOrKeyTerminal {
            terminal,
            scroll,
            key,
        })
        .await
    }

    /// Requests a complete frame for a terminal.
    pub async fn request_full_frame(&self, terminal: TerminalId) -> Result<()> {
        expect_ack(
            "request_full_frame",
            self.request(RequestBody::RequestFullFrame { terminal })
                .await?,
        )
    }

    /// Pastes text with mode-aware bracketed-paste handling.
    pub async fn paste_terminal(
        &self,
        terminal: TerminalId,
        text: impl Into<String>,
    ) -> Result<()> {
        expect_ack(
            "paste_terminal",
            self.request(RequestBody::PasteTerminal {
                terminal,
                text: text.into(),
            })
            .await?,
        )
    }
}
