//! The terminal host thread that owns PTYs and virtual-terminal engines.

use std::{thread, time::Duration, time::Instant};

use async_channel::{Receiver, Sender, TryRecvError};
use fleet_core::ids::TerminalId;
use fleet_proto::terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand};
use thiserror::Error;
use tracing::warn;

use crate::{
    GhosttyEngine,
    engine::{EngineError, EngineEvent, VtEngine},
    pty::{Pty, PtyError, PtyOptions},
};

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const PROMPT_AFTER_OUTPUT: Duration = Duration::from_millis(150);
const PROMPT_FALLBACK: Duration = Duration::from_millis(500);
const EXIT_DRAIN_GRACE: Duration = Duration::from_millis(100);

/// Construction settings for one daemon-owned terminal host.
#[derive(Debug, Clone)]
pub struct TerminalHostOptions {
    /// Stable terminal identifier stamped onto emitted frames.
    pub terminal: TerminalId,
    /// PTY child-process settings.
    pub pty: PtyOptions,
    /// Maximum retained scrollback rows.
    pub scrollback_lines: usize,
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
    /// Types a shell command after the prompt readiness heuristic.
    TypeCommand(String),
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
    terminal: TerminalId,
    commands: Sender<HostCommand>,
    events: Receiver<HostEvent>,
    join: Option<thread::JoinHandle<()>>,
}

impl TerminalHost {
    /// Starts a PTY and Ghostty engine on a dedicated terminal thread.
    pub fn spawn(options: TerminalHostOptions) -> Result<Self, HostError> {
        let cols = options.pty.cols;
        let rows = options.pty.rows;
        let pty = Pty::spawn(options.pty)?;
        let engine = GhosttyEngine::new(cols, rows, options.scrollback_lines)?;
        let (command_sender, command_receiver) = async_channel::unbounded();
        let (event_sender, event_receiver) = async_channel::unbounded();
        let terminal = options.terminal;
        let initial_command = options.initial_command;
        let join = thread::Builder::new()
            .name(format!("fleet-terminal-{terminal}"))
            .spawn(move || {
                run_host(
                    terminal,
                    pty,
                    Box::new(engine),
                    command_receiver,
                    event_sender,
                    initial_command,
                );
            })?;
        Ok(Self {
            terminal,
            commands: command_sender,
            events: event_receiver,
            join: Some(join),
        })
    }

    /// Returns the stable identifier of the hosted terminal.
    #[must_use]
    pub fn terminal_id(&self) -> TerminalId {
        self.terminal
    }

    /// Returns a cloneable sender for low-level command dispatch.
    #[must_use]
    pub fn command_sender(&self) -> Sender<HostCommand> {
        self.commands.clone()
    }

    /// Returns a clone of the terminal event receiver.
    #[must_use]
    pub fn event_receiver(&self) -> Receiver<HostEvent> {
        self.events.clone()
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

    /// Types `command` followed by carriage return after the shell prompt heuristic fires.
    pub fn type_command(&self, command: impl Into<String>) -> Result<(), HostError> {
        self.send(HostCommand::TypeCommand(command.into()))
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
            .send_blocking(command)
            .map_err(|_| HostError::Closed)
    }
}

fn run_host(
    terminal: TerminalId,
    mut pty: Pty,
    mut engine: Box<dyn VtEngine>,
    commands: Receiver<HostCommand>,
    events: Sender<HostEvent>,
    initial_command: Option<String>,
) {
    let spawned_at = Instant::now();
    let mut pending_commands = initial_command
        .into_iter()
        .map(|command| (command, spawned_at + PROMPT_FALLBACK))
        .collect::<Vec<_>>();
    let mut first_output_at = None;
    let mut dirty = false;
    let mut force_full = false;
    let mut sequence = 0_u64;
    let mut last_frame_at = spawned_at.checked_sub(FRAME_INTERVAL).unwrap_or(spawned_at);
    let mut exit = None;
    let mut exit_observed_at = None;
    let mut commands_closed = false;

    loop {
        drain_commands(
            terminal,
            &commands,
            &events,
            &mut pty,
            engine.as_mut(),
            &mut pending_commands,
            spawned_at,
            first_output_at,
            &mut force_full,
            &mut sequence,
            &mut commands_closed,
        );

        loop {
            match pty.try_read() {
                Ok(Some(bytes)) => {
                    if first_output_at.is_none() {
                        let observed = Instant::now();
                        first_output_at = Some(observed);
                        let prompt_deadline = observed + PROMPT_AFTER_OUTPUT;
                        for (_, deadline) in &mut pending_commands {
                            *deadline = (*deadline).min(prompt_deadline);
                        }
                    }
                    engine.feed(&bytes);
                    dirty = true;
                }
                Ok(None) => break,
                Err(error) => {
                    warn!(%error, %terminal, "terminal PTY reader failed");
                    break;
                }
            }
        }

        forward_engine_events(engine.as_mut(), &events);
        type_ready_commands(&mut pending_commands, &mut pty);

        let now = Instant::now();
        if (dirty || force_full) && now.duration_since(last_frame_at) >= FRAME_INTERVAL {
            let frame = stamp_frame(engine.take_frame(force_full), terminal, &mut sequence);
            if events.send_blocking(HostEvent::Frame(frame)).is_err() {
                let _ = pty.kill();
                break;
            }
            dirty = false;
            force_full = false;
            last_frame_at = now;
        }

        if exit.is_none() {
            match pty.try_wait() {
                Ok(Some(code)) => {
                    exit = Some(Some(code));
                    exit_observed_at = Some(now);
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(%error, %terminal, "failed to poll terminal child");
                    exit = Some(None);
                    exit_observed_at = Some(now);
                }
            }
        }

        if let (Some(code), Some(observed_at)) = (exit, exit_observed_at)
            && (pty.output_closed() || now.duration_since(observed_at) >= EXIT_DRAIN_GRACE)
        {
            if dirty || force_full {
                let since_last_frame = Instant::now().duration_since(last_frame_at);
                if since_last_frame < FRAME_INTERVAL {
                    thread::sleep(FRAME_INTERVAL - since_last_frame);
                }
                let frame = stamp_frame(engine.take_frame(force_full), terminal, &mut sequence);
                let _ = events.send_blocking(HostEvent::Frame(frame));
            }
            let _ = events.send_blocking(HostEvent::Exited(code));
            break;
        }

        if commands_closed {
            let _ = pty.kill();
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "host loop state is intentionally explicit"
)]
fn drain_commands(
    terminal: TerminalId,
    commands: &Receiver<HostCommand>,
    events: &Sender<HostEvent>,
    pty: &mut Pty,
    engine: &mut dyn VtEngine,
    pending_commands: &mut Vec<(String, Instant)>,
    spawned_at: Instant,
    first_output_at: Option<Instant>,
    force_full: &mut bool,
    sequence: &mut u64,
    commands_closed: &mut bool,
) {
    loop {
        let command = match commands.try_recv() {
            Ok(command) => command,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Closed) => {
                *commands_closed = true;
                break;
            }
        };
        match command {
            HostCommand::Write(bytes) => write_or_warn(pty, &bytes, terminal),
            HostCommand::Key(event) => {
                let bytes = engine.encode_key(&event);
                write_or_warn(pty, &bytes, terminal);
            }
            HostCommand::Mouse(event) => {
                let bytes = engine.encode_mouse(&event);
                write_or_warn(pty, &bytes, terminal);
            }
            HostCommand::Paste(text) => {
                let bytes = engine.encode_paste(&text);
                write_or_warn(pty, &bytes, terminal);
            }
            HostCommand::Resize { cols, rows } => match resize(pty, engine, cols, rows) {
                Ok(()) => *force_full = true,
                Err(error) => warn!(%error, %terminal, "failed to resize terminal"),
            },
            HostCommand::Scroll(command) => {
                engine.scroll(command);
                *force_full = true;
            }
            HostCommand::Attach { cols, rows, reply } => match resize(pty, engine, cols, rows) {
                Ok(()) => {
                    let frame = stamp_frame(engine.take_frame(true), terminal, sequence);
                    let _ = reply.send_blocking(Ok(frame));
                    *force_full = true;
                }
                Err(error) => {
                    let _ = reply.send_blocking(Err(error));
                }
            },
            HostCommand::RequestFull => *force_full = true,
            HostCommand::Kill => {
                if let Err(error) = pty.kill() {
                    warn!(%error, %terminal, "failed to kill terminal child");
                }
            }
            HostCommand::TypeCommand(command) => {
                let deadline = first_output_at
                    .map(|observed| observed + PROMPT_AFTER_OUTPUT)
                    .unwrap_or(spawned_at + PROMPT_FALLBACK)
                    .min(spawned_at + PROMPT_FALLBACK);
                pending_commands.push((command, deadline));
            }
        }
    }
    forward_engine_events(engine, events);
}

fn resize(pty: &Pty, engine: &mut dyn VtEngine, cols: u16, rows: u16) -> Result<(), String> {
    pty.resize(cols, rows).map_err(|error| error.to_string())?;
    engine.resize(cols, rows).map_err(|error| error.to_string())
}

fn type_ready_commands(pending: &mut Vec<(String, Instant)>, pty: &mut Pty) {
    let now = Instant::now();
    let mut index = 0;
    while index < pending.len() {
        if pending[index].1 <= now {
            let (mut command, _) = pending.remove(index);
            command.push('\r');
            if let Err(error) = pty.write(command.as_bytes()) {
                warn!(%error, "failed to type initial terminal command");
            }
        } else {
            index += 1;
        }
    }
}

fn forward_engine_events(engine: &mut dyn VtEngine, events: &Sender<HostEvent>) {
    for event in engine.take_events() {
        let event = match event {
            EngineEvent::Title(title) => HostEvent::Title(title),
            EngineEvent::Bell => HostEvent::Bell,
            EngineEvent::Cwd(cwd) => HostEvent::Cwd(cwd),
            EngineEvent::ClipboardWrite { mime, data } => HostEvent::ClipboardWrite { mime, data },
        };
        let _ = events.send_blocking(event);
    }
}

fn stamp_frame(mut frame: FrameUpdate, terminal: TerminalId, sequence: &mut u64) -> FrameUpdate {
    *sequence = sequence.saturating_add(1);
    frame.terminal = terminal;
    frame.seq = *sequence;
    frame
}

fn write_or_warn(pty: &mut Pty, bytes: &[u8], terminal: TerminalId) {
    if !bytes.is_empty()
        && let Err(error) = pty.write(bytes)
    {
        warn!(%error, %terminal, "failed to write terminal PTY");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn host_emits_command_output_before_exit() {
        let options = TerminalHostOptions {
            terminal: TerminalId(17),
            pty: PtyOptions::command(
                "/bin/sh",
                ["-c", "printf OK"],
                PathBuf::from(env!("CARGO_MANIFEST_DIR")),
                20,
                4,
            ),
            scrollback_lines: 100,
            initial_command: None,
        };
        let host = TerminalHost::spawn(options)
            .unwrap_or_else(|error| panic!("failed to spawn terminal host: {error}"));
        let receiver = host.event_receiver();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_ok = false;
        let mut saw_exit = false;
        while Instant::now() < deadline && !saw_exit {
            match receiver.try_recv() {
                Ok(HostEvent::Frame(frame)) => {
                    saw_ok |= frame.rows_changed.iter().any(|row| {
                        row.cells
                            .iter()
                            .map(|cell| cell.text.as_str())
                            .collect::<String>()
                            .contains("OK")
                    });
                }
                Ok(HostEvent::Exited(code)) => {
                    assert_eq!(code, Some(0));
                    saw_exit = true;
                }
                Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                Err(TryRecvError::Closed) => break,
            }
        }
        assert!(saw_ok, "no frame contained command output");
        assert!(saw_exit, "host did not report child exit");
        host.join()
            .unwrap_or_else(|_| panic!("terminal host thread panicked"));
    }
}
