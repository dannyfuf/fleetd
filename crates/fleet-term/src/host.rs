//! The terminal host thread that owns PTYs and virtual-terminal engines.

use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Instant,
};

use async_channel::{Receiver, Sender};
use fleet_core::ids::TerminalId;
use fleet_proto::terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand, WheelEvent};
use thiserror::Error;

use crate::{
    GhosttyEngine,
    engine::EngineError,
    pty::{Pty, PtyError, PtyOptions},
};

mod owner;

use owner::{OwnerEvent, TerminalOwner};

/// Construction settings for one daemon-owned terminal host.
#[derive(Debug, Clone)]
pub struct TerminalHostOptions {
    /// Stable terminal identifier stamped onto emitted frames.
    pub terminal: TerminalId,
    /// Sequence carried by the first frame, including after restart.
    pub starting_sequence: u64,
    /// PTY child-process settings.
    pub pty: PtyOptions,
    /// Maximum retained scrollback bytes.
    pub scrollback_bytes: usize,
    /// Command typed into the login shell after its prompt is likely ready.
    pub initial_command: Option<String>,
}

/// A command executed serially by a terminal's owning thread.
#[derive(Debug)]
pub enum HostCommand {
    /// Writes already encoded bytes directly to the PTY.
    Write(Vec<u8>),
    /// Encodes and writes a keyboard event.
    Key(KeyEvent),
    /// Encodes and writes a mouse event.
    Mouse(MouseEvent),
    /// Encodes and writes pasted text.
    Paste(String),
    /// Resizes the PTY and emulator.
    Resize {
        /// New column count.
        cols: u16,
        /// New row count.
        rows: u16,
    },
    /// Moves the emulator's scrollback viewport.
    Scroll(ScrollCommand),
    /// Routes wheel input using live VT modes.
    Wheel(WheelEvent),
    /// Scrolls on the primary screen or forwards a key on the alternate screen.
    ScrollOrKey {
        /// Primary-screen viewport movement.
        scroll: ScrollCommand,
        /// Alternate-screen input.
        key: KeyEvent,
    },
    /// Attaches a client, makes its dimensions authoritative, and returns a full frame.
    Attach {
        /// Attaching client's column count.
        cols: u16,
        /// Attaching client's row count.
        rows: u16,
        /// One-shot response channel for the full frame.
        reply: Sender<Result<FrameUpdate, String>>,
    },
    /// Emits a complete frame on the host event channel.
    RequestFull,
    /// Terminates the PTY child.
    Kill,
}

/// An event emitted by a terminal host thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// A complete or dirty-row terminal frame.
    Frame(FrameUpdate),
    /// The PTY child exited, optionally with a process status code.
    Exited(Option<i32>),
    /// The terminal title changed.
    Title(String),
    /// The terminal emitted a bell.
    Bell,
    /// The terminal reported its working directory.
    Cwd(String),
    /// The terminal requested a clipboard write.
    ClipboardWrite {
        /// MIME type of the preferred representation.
        mime: String,
        /// Decoded clipboard data.
        data: String,
    },
}

/// Monotonic input/output activity observed by a terminal host.
#[derive(Debug, Clone, Copy)]
pub struct TerminalActivity {
    /// Most recent PTY output.
    pub last_output_at: Instant,
    /// Most recent input written to the PTY.
    pub last_input_at: Instant,
    /// Total bytes read from the PTY since the host started.
    pub output_bytes_total: u64,
}

/// Failure while starting or communicating with a terminal host.
#[derive(Debug, Error)]
pub enum HostError {
    /// PTY startup or operation failed.
    #[error(transparent)]
    Pty(#[from] PtyError),
    /// Virtual-terminal startup or resize failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// The terminal thread is no longer accepting commands.
    #[error("terminal host command channel is closed")]
    Closed,
    /// The terminal thread stopped before answering an attachment request.
    #[error("terminal host did not return an attachment frame: {0}")]
    AttachFailed(String),
    /// The operating system could not create the terminal host thread.
    #[error("failed to spawn terminal host thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// Handle used by daemon services to control and observe one terminal thread.
pub struct TerminalHost {
    child_pid: Option<u32>,
    commands: mpsc::Sender<OwnerEvent>,
    events: Receiver<HostEvent>,
    activity: Arc<Mutex<TerminalActivity>>,
    join: Option<thread::JoinHandle<()>>,
}

impl TerminalHost {
    /// Starts a PTY and Ghostty engine on a dedicated terminal thread.
    pub fn spawn(options: TerminalHostOptions) -> Result<Self, HostError> {
        let engine =
            GhosttyEngine::new(options.pty.cols, options.pty.rows, options.scrollback_bytes)?;
        let (commands, inbox) = mpsc::channel();
        let wakeup = owner::PtyWakeup::new(commands.clone());
        let notify = Arc::clone(&wakeup);
        let pty = Pty::spawn_notifying(options.pty, Arc::new(move || notify.notify()))?;
        let child_pid = pty.child_pid();
        let (event_sender, event_receiver) = async_channel::unbounded();
        let terminal = options.terminal;
        let started_at = Instant::now();
        let activity = Arc::new(Mutex::new(TerminalActivity {
            last_output_at: started_at,
            last_input_at: started_at,
            output_bytes_total: 0,
        }));
        let owner = TerminalOwner::new(
            terminal,
            pty,
            engine,
            inbox,
            event_sender,
            Arc::clone(&activity),
            wakeup,
        );
        let join = thread::Builder::new()
            .name(format!("fleet-terminal-{terminal}"))
            .spawn(move || owner.run(options.initial_command, options.starting_sequence))?;
        Ok(Self {
            child_pid,
            commands,
            events: event_receiver,
            activity,
            join: Some(join),
        })
    }

    /// Returns the hosted PTY child process identifier when the platform exposes it.
    #[must_use]
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// Returns a clone of the terminal event receiver.
    #[must_use]
    pub fn event_receiver(&self) -> Receiver<HostEvent> {
        self.events.clone()
    }

    /// Returns the latest monotonic PTY input/output activity counters.
    #[must_use]
    pub fn activity(&self) -> TerminalActivity {
        *self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Writes already encoded bytes to the PTY.
    pub fn write(&self, bytes: Vec<u8>) -> Result<(), HostError> {
        self.send(HostCommand::Write(bytes))
    }

    /// Sends a keyboard event for mode-aware encoding.
    pub fn key(&self, event: KeyEvent) -> Result<(), HostError> {
        self.send(HostCommand::Key(event))
    }

    /// Sends a mouse event for mode-aware encoding.
    pub fn mouse(&self, event: MouseEvent) -> Result<(), HostError> {
        self.send(HostCommand::Mouse(event))
    }

    /// Sends text through the engine's paste encoder.
    pub fn paste(&self, text: impl Into<String>) -> Result<(), HostError> {
        self.send(HostCommand::Paste(text.into()))
    }

    /// Changes the authoritative PTY and emulator dimensions.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), HostError> {
        self.send(HostCommand::Resize { cols, rows })
    }

    /// Moves the visible scrollback viewport.
    pub fn scroll(&self, command: ScrollCommand) -> Result<(), HostError> {
        self.send(HostCommand::Scroll(command))
    }

    /// Sends whole-row wheel input for live-mode routing.
    pub fn wheel(&self, event: WheelEvent) -> Result<(), HostError> {
        self.send(HostCommand::Wheel(event))
    }

    /// Routes a viewport shortcut using live screen modes.
    pub fn scroll_or_key(&self, scroll: ScrollCommand, key: KeyEvent) -> Result<(), HostError> {
        self.send(HostCommand::ScrollOrKey { scroll, key })
    }

    /// Attaches at the supplied dimensions and synchronously returns a complete frame.
    pub fn attach(&self, cols: u16, rows: u16) -> Result<FrameUpdate, HostError> {
        if cols == 0 || rows == 0 {
            return Err(EngineError::InvalidSize { cols, rows }.into());
        }
        let (reply, response) = async_channel::bounded(1);
        self.send(HostCommand::Attach { cols, rows, reply })?;
        response
            .recv_blocking()
            .map_err(|_| HostError::AttachFailed("response channel closed".to_owned()))?
            .map_err(HostError::AttachFailed)
    }

    /// Requests a complete frame on the event channel.
    pub fn request_full(&self) -> Result<(), HostError> {
        self.send(HostCommand::RequestFull)
    }

    /// Requests termination of the child process.
    pub fn kill(&self) -> Result<(), HostError> {
        self.send(HostCommand::Kill)
    }

    /// Waits for the terminal thread to finish after the child exits or all senders close.
    pub fn join(mut self) -> thread::Result<()> {
        let join = self.join.take();
        drop(self);
        match join {
            Some(join) => join.join(),
            None => Ok(()),
        }
    }

    fn send(&self, command: HostCommand) -> Result<(), HostError> {
        self.commands
            .send(OwnerEvent::Command(command))
            .map_err(|_| HostError::Closed)
    }
}

impl Drop for TerminalHost {
    /// Releasing the handle stops the owner once every queued command has been applied.
    fn drop(&mut self) {
        let _ = self.commands.send(OwnerEvent::CommandsClosed);
    }
}
