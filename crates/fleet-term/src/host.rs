//! The terminal host thread that owns PTYs and virtual-terminal engines.

use std::{sync::mpsc, thread, time::Duration, time::Instant};

use async_channel::{Receiver, Sender};
use fleet_core::ids::TerminalId;
use fleet_proto::terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand, WheelEvent};
use thiserror::Error;
use tracing::warn;

use crate::{
    GhosttyEngine,
    engine::{EngineError, EngineEvent, VtEngine, WheelAction},
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
    child_pid: Option<u32>,
    commands: mpsc::Sender<HostCommand>,
    events: Receiver<HostEvent>,
    join: Option<thread::JoinHandle<()>>,
}

impl TerminalHost {
    /// Starts a PTY and Ghostty engine on a dedicated terminal thread.
    pub fn spawn(options: TerminalHostOptions) -> Result<Self, HostError> {
        let cols = options.pty.cols;
        let rows = options.pty.rows;
        let pty = Pty::spawn(options.pty)?;
        let child_pid = pty.child_pid();
        let engine = GhosttyEngine::new(cols, rows, options.scrollback_bytes)?;
        let (command_sender, command_receiver) = mpsc::channel();
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
                    options.starting_sequence,
                );
            })?;
        Ok(Self {
            terminal,
            child_pid,
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

    /// Returns the hosted PTY child process identifier when the platform exposes it.
    #[must_use]
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// Returns a cloneable sender for low-level command dispatch.
    #[must_use]
    pub fn command_sender(&self) -> mpsc::Sender<HostCommand> {
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
        self.commands.send(command).map_err(|_| HostError::Closed)
    }
}

fn run_host(
    terminal: TerminalId,
    mut pty: Pty,
    mut engine: Box<dyn VtEngine>,
    commands: mpsc::Receiver<HostCommand>,
    events: Sender<HostEvent>,
    initial_command: Option<String>,
    starting_sequence: u64,
) {
    let spawned_at = Instant::now();
    let mut pending_commands = initial_command
        .into_iter()
        .map(|command| (command, spawned_at + PROMPT_FALLBACK))
        .collect::<Vec<_>>();
    let mut first_output_at = None;
    let mut dirty = false;
    let mut force_full = false;
    let mut viewport_moved = false;
    let mut sequence = starting_sequence;
    let mut last_frame_at = spawned_at.checked_sub(FRAME_INTERVAL).unwrap_or(spawned_at);
    let mut exit = None;
    let mut exit_observed_at = None;
    let mut commands_closed = false;
    let mut received = None;
    let mut last_activity = spawned_at;
    let mut last_compression = spawned_at;

    loop {
        let mut output_bytes = 0;
        let output_started = Instant::now();
        while output_bytes < 256 * 1024 && output_started.elapsed() < Duration::from_millis(2) {
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
                    output_bytes += bytes.len();
                    last_activity = Instant::now();
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

        let activity = drain_commands(
            terminal,
            &commands,
            received.take(),
            &events,
            &mut pty,
            engine.as_mut(),
            &mut pending_commands,
            spawned_at,
            first_output_at,
            &mut force_full,
            &mut viewport_moved,
            &mut sequence,
            &mut commands_closed,
        );

        if activity {
            last_activity = Instant::now();
        }
        if last_activity.elapsed() >= Duration::from_millis(250)
            && last_compression.elapsed() >= Duration::from_millis(16)
        {
            engine.compress_idle();
            last_compression = Instant::now();
        }

        forward_engine_events(engine.as_mut(), &mut pty, terminal, &events);
        type_ready_commands(&mut pending_commands, &mut pty);

        let now = Instant::now();
        if force_full
            || viewport_moved
            || (dirty && now.duration_since(last_frame_at) >= FRAME_INTERVAL)
        {
            let frame = stamp_frame(engine.take_frame(force_full), terminal, &mut sequence);
            if events.send_blocking(HostEvent::Frame(frame)).is_err() {
                let _ = pty.kill();
                break;
            }
            dirty = false;
            force_full = false;
            viewport_moved = false;
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
            if dirty || force_full || viewport_moved {
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
        // Commands wake the owner immediately; timeout bounds PTY output/prompt latency.
        match commands.recv_timeout(Duration::from_millis(4)) {
            Ok(command) => received = Some(command),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = pty.kill();
                break;
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "host loop state is intentionally explicit"
)]
fn drain_commands(
    terminal: TerminalId,
    commands: &mpsc::Receiver<HostCommand>,
    first: Option<HostCommand>,
    events: &Sender<HostEvent>,
    pty: &mut Pty,
    engine: &mut dyn VtEngine,
    pending_commands: &mut Vec<(String, Instant)>,
    spawned_at: Instant,
    first_output_at: Option<Instant>,
    force_full: &mut bool,
    viewport_moved: &mut bool,
    sequence: &mut u64,
    commands_closed: &mut bool,
) -> bool {
    let mut first = first;
    let mut activity = false;
    let mut viewport = None;
    // A bounded batch also prevents an input producer from starving frame emission.
    for _ in 0..1024 {
        let command = match first.take().map(Ok).unwrap_or_else(|| commands.try_recv()) {
            Ok(command) => command,
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                *commands_closed = true;
                break;
            }
        };
        activity = true;
        let command = match command {
            HostCommand::Wheel(event) => match engine.wheel(&event) {
                WheelAction::Viewport(steps) => HostCommand::Scroll(ScrollCommand::Lines(steps)),
                WheelAction::Pty(bytes) => {
                    flush_viewport(&mut viewport, engine, viewport_moved);
                    write_or_warn(pty, &bytes, terminal);
                    continue;
                }
                WheelAction::Drop => continue,
            },
            HostCommand::ScrollOrKey { scroll, key } => {
                if engine.modes().alt_screen {
                    HostCommand::Key(key)
                } else {
                    HostCommand::Scroll(scroll)
                }
            }
            other => other,
        };
        if let HostCommand::Scroll(command) = command {
            let pending = viewport.get_or_insert_with(|| PendingViewport::new(engine));
            pending.push(command);
            continue;
        }
        // Preserve input/resize ordering across scroll batches.
        flush_viewport(&mut viewport, engine, viewport_moved);
        match command {
            HostCommand::Write(bytes) => {
                follow_input(engine, viewport_moved);
                write_or_warn(pty, &bytes, terminal);
            }
            HostCommand::Key(event) => {
                follow_input(engine, viewport_moved);
                let bytes = engine.encode_key(&event);
                write_or_warn(pty, &bytes, terminal);
            }
            HostCommand::Mouse(event) => {
                let bytes = engine.encode_mouse(&event);
                write_or_warn(pty, &bytes, terminal);
            }
            HostCommand::Paste(text) => {
                follow_input(engine, viewport_moved);
                let bytes = engine.encode_paste(&text);
                write_or_warn(pty, &bytes, terminal);
            }
            HostCommand::Resize { cols, rows } => match resize(pty, engine, cols, rows) {
                Ok(()) => *force_full = true,
                Err(error) => warn!(%error, %terminal, "failed to resize terminal"),
            },
            HostCommand::Scroll(_) | HostCommand::Wheel(_) | HostCommand::ScrollOrKey { .. } => {
                unreachable!("handled above")
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
    flush_viewport(&mut viewport, engine, viewport_moved);
    forward_engine_events(engine, pty, terminal, events);
    activity
}

/// Fold signed/absolute moves while preserving clamping at each command boundary.
struct PendingViewport {
    offset: usize,
    history: usize,
    rows: u16,
}

impl PendingViewport {
    fn new(engine: &dyn VtEngine) -> Self {
        let viewport = engine.viewport();
        Self {
            offset: viewport.offset,
            history: viewport.scrollback_len,
            rows: engine.rows(),
        }
    }

    fn push(&mut self, command: ScrollCommand) {
        let delta = match command {
            ScrollCommand::Lines(lines) => i64::from(lines),
            ScrollCommand::Pages(pages) => i64::from(pages) * i64::from(self.rows),
            ScrollCommand::Top => {
                self.offset = self.history;
                return;
            }
            ScrollCommand::Bottom => {
                self.offset = 0;
                return;
            }
            ScrollCommand::ToOffset(offset) => {
                self.offset = offset.min(self.history);
                return;
            }
        };
        self.offset = if delta < 0 {
            self.offset
                .saturating_add(delta.unsigned_abs() as usize)
                .min(self.history)
        } else {
            self.offset.saturating_sub(delta as usize)
        };
    }
}

fn flush_viewport(
    pending: &mut Option<PendingViewport>,
    engine: &mut dyn VtEngine,
    force_full: &mut bool,
) {
    if let Some(pending) = pending.take()
        && pending.offset != engine.viewport_offset()
    {
        engine.scroll(ScrollCommand::ToOffset(pending.offset));
        *force_full = true;
    }
}

fn follow_input(engine: &mut dyn VtEngine, force_full: &mut bool) {
    if engine.viewport_offset() > 0 {
        engine.scroll(ScrollCommand::Bottom);
        *force_full = true;
    }
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

fn forward_engine_events(
    engine: &mut dyn VtEngine,
    pty: &mut Pty,
    terminal: TerminalId,
    events: &Sender<HostEvent>,
) {
    for event in engine.take_events() {
        let event = match event {
            EngineEvent::PtyWrite(bytes) => {
                write_or_warn(pty, &bytes, terminal);
                continue;
            }
            EngineEvent::Title(title) => HostEvent::Title(title),
            EngineEvent::Bell => HostEvent::Bell,
            EngineEvent::Cwd(cwd) => HostEvent::Cwd(cwd),
            EngineEvent::ClipboardWrite { mime, data } => HostEvent::ClipboardWrite { mime, data },
        };
        let _ = events.send_blocking(event);
    }
}

fn stamp_frame(mut frame: FrameUpdate, terminal: TerminalId, sequence: &mut u64) -> FrameUpdate {
    frame.terminal = terminal;
    frame.seq = *sequence;
    *sequence = sequence.saturating_add(1);
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
    use std::{path::PathBuf, time::SystemTime};

    use super::*;
    use async_channel::TryRecvError;

    #[test]
    fn large_paste_into_echoing_cat_does_not_block_frames_or_commands() {
        let host = TerminalHost::spawn(TerminalHostOptions {
            terminal: TerminalId(21),
            pty: PtyOptions::command(
                "/bin/cat",
                std::iter::empty::<&str>(),
                PathBuf::from("/tmp"),
                80,
                24,
            ),
            scrollback_bytes: 1024 * 1024,
            initial_command: None,
            starting_sequence: 1,
        })
        .unwrap();
        let events = host.event_receiver();
        // 2 MiB with short lines stays below canonical line limits and exercises echo backpressure.
        host.paste(format!("{}\n", "x".repeat(63)).repeat(32 * 1024))
            .unwrap();
        let (reply, attached) = async_channel::bounded(1);
        host.command_sender()
            .send(HostCommand::Attach {
                cols: 81,
                rows: 24,
                reply,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut processed_command = false;
        let mut produced_output = false;
        while Instant::now() < deadline && !(processed_command && produced_output) {
            if let Ok(frame) = attached.try_recv() {
                let frame = frame.unwrap();
                assert!(frame.full);
                assert_eq!(frame.cols, 81);
                processed_command = true;
            }
            if let Ok(HostEvent::Frame(frame)) = events.try_recv() {
                produced_output |= frame.viewport.scrollback_len > 0;
            }
            thread::sleep(Duration::from_millis(1));
        }
        // A regression blocks the owner in write_all, so kill the child independently on failure.
        if !(processed_command && produced_output) {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", &host.child_pid().unwrap().to_string()])
                .status();
        }
        host.kill().unwrap();
        host.join().unwrap();
        assert!(
            processed_command,
            "large paste blocked the subsequent attach command"
        );
        assert!(produced_output, "large paste blocked output frames");
    }

    #[test]
    fn scroll_or_key_uses_live_screen_modes() {
        use fleet_proto::terminal::{Key, KeyAction, Modifiers};
        let mut pty = Pty::spawn(PtyOptions::command(
            "/bin/sh",
            ["-c", "stty raw -echo; printf READY; exec /bin/cat"],
            PathBuf::from("/tmp"),
            40,
            10,
        ))
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        while Instant::now() < deadline && output != b"READY" {
            if let Some(bytes) = pty.try_read().unwrap() {
                output.extend(bytes);
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(output, b"READY");
        let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
        engine.feed("line\r\n".repeat(100).as_bytes());
        engine.take_frame(true);
        let (sender, receiver) = mpsc::channel();
        let (events, _) = async_channel::unbounded();
        let mut full = false;
        let mut moved = false;
        let mut sequence = 1;
        let mut closed = false;
        let mut pending = Vec::new();
        for alternate in [false, true, false] {
            engine.feed(if alternate {
                b"\x1b[?1049h"
            } else {
                b"\x1b[?1049l"
            });
            let before = engine.viewport_offset();
            sender
                .send(HostCommand::ScrollOrKey {
                    scroll: ScrollCommand::Pages(-1),
                    key: KeyEvent {
                        key: Key::PageUp,
                        mods: Modifiers::SHIFT,
                        text: None,
                        action: KeyAction::Press,
                    },
                })
                .unwrap();
            drain_commands(
                TerminalId(22),
                &receiver,
                None,
                &events,
                &mut pty,
                &mut engine,
                &mut pending,
                Instant::now(),
                None,
                &mut full,
                &mut moved,
                &mut sequence,
                &mut closed,
            );
            if alternate {
                assert_eq!(engine.viewport_offset(), 0);
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut output = Vec::new();
                while Instant::now() < deadline && output.len() < 6 {
                    if let Some(bytes) = pty.try_read().unwrap() {
                        output.extend(bytes);
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(output, b"\x1b[5;2~");
            } else {
                assert_eq!(engine.viewport_offset(), before + 10);
                assert!(moved);
                assert!(pty.try_read().unwrap().is_none());
            }
        }
        pty.kill().unwrap();
    }

    #[test]
    fn coalesced_scrolls_match_sequential_clamping() {
        let mut batched = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
        let mut sequential = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
        for engine in [&mut batched, &mut sequential] {
            engine.feed("line\r\n".repeat(100).as_bytes());
        }
        let commands = [
            ScrollCommand::Lines(10),
            ScrollCommand::Lines(-3),
            ScrollCommand::Pages(-2),
            ScrollCommand::Top,
            ScrollCommand::Lines(-99),
            ScrollCommand::Lines(5),
            ScrollCommand::Bottom,
            ScrollCommand::ToOffset(500),
            ScrollCommand::Pages(1),
        ];
        let mut pending = PendingViewport::new(&batched);
        for command in commands {
            pending.push(command);
            sequential.scroll(command);
        }
        let mut moved = false;
        flush_viewport(&mut Some(pending), &mut batched, &mut moved);
        assert!(moved);
        assert_eq!(batched.viewport_offset(), sequential.viewport_offset());
        assert_eq!(
            batched.take_frame(true).rows_changed,
            sequential.take_frame(true).rows_changed
        );
    }

    #[test]
    fn queued_wheels_coalesce_and_input_returns_to_bottom() {
        use fleet_proto::terminal::{Key, KeyAction, Modifiers};
        let mut pty = Pty::spawn(PtyOptions::command(
            "/bin/cat",
            std::iter::empty::<&str>(),
            PathBuf::from("/tmp"),
            40,
            10,
        ))
        .unwrap();
        let mut engine = GhosttyEngine::new(40, 10, 1024 * 1024).unwrap();
        engine.feed("line\r\n".repeat(100).as_bytes());
        engine.take_frame(true);
        let (sender, receiver) = mpsc::channel();
        let (events, _) = async_channel::unbounded();
        for _ in 0..3 {
            sender
                .send(HostCommand::Wheel(WheelEvent {
                    steps: -1,
                    col: 0,
                    row: 0,
                    mods: Modifiers::empty(),
                }))
                .unwrap();
        }
        let mut full = false;
        let mut moved = false;
        let mut seq = 0;
        let mut closed = false;
        let mut pending = Vec::new();
        drain_commands(
            TerminalId(1),
            &receiver,
            None,
            &events,
            &mut pty,
            &mut engine,
            &mut pending,
            Instant::now(),
            None,
            &mut full,
            &mut moved,
            &mut seq,
            &mut closed,
        );
        assert_eq!(engine.viewport_offset(), 3);
        assert!(moved && !full);
        let frame = engine.take_frame(false);
        assert_eq!(frame.shift, Some(-3));
        assert_eq!(frame.rows_changed.len(), 3);
        for input in [
            HostCommand::Key(KeyEvent {
                key: Key::Char('x'),
                mods: Modifiers::empty(),
                text: Some("x".into()),
                action: KeyAction::Press,
            }),
            HostCommand::Paste("paste".into()),
            HostCommand::Write(b"raw".to_vec()),
        ] {
            engine.scroll(ScrollCommand::Top);
            sender.send(input).unwrap();
            drain_commands(
                TerminalId(1),
                &receiver,
                None,
                &events,
                &mut pty,
                &mut engine,
                &mut pending,
                Instant::now(),
                None,
                &mut full,
                &mut moved,
                &mut seq,
                &mut closed,
            );
            assert_eq!(engine.viewport_offset(), 0);
        }
        pty.kill().unwrap();
    }

    #[test]
    fn continuous_output_does_not_starve_viewport_commands() {
        let host = TerminalHost::spawn(TerminalHostOptions {
            terminal: TerminalId(20),
            pty: PtyOptions::command("/usr/bin/yes", ["output"], PathBuf::from("/tmp"), 80, 24),
            scrollback_bytes: 1024 * 1024,
            initial_command: None,
            starting_sequence: 1,
        })
        .unwrap();
        let events = host.event_receiver();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline {
            if let Ok(HostEvent::Frame(frame)) = events.try_recv()
                && frame.viewport.scrollback_len > 100
            {
                ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(ready, "continuous-output child produced no history");
        host.scroll(ScrollCommand::Lines(-20)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut moved = false;
        while Instant::now() < deadline {
            if let Ok(HostEvent::Frame(frame)) = events.try_recv()
                && frame.viewport.offset > 0
            {
                moved = true;
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        host.kill().unwrap();
        host.join().unwrap();
        assert!(moved, "continuous PTY output starved the scroll frame");
    }

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
            scrollback_bytes: 100,
            initial_command: None,
            starting_sequence: 1,
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

    #[test]
    fn terminal_query_reply_reaches_the_pty_child() {
        let options = TerminalHostOptions {
            terminal: TerminalId(18),
            pty: PtyOptions::command(
                "/bin/sh",
                [
                    "-c",
                    r#"stty raw -echo; printf '\033[6n'; dd bs=1 count=6 >/dev/null 2>&1; stty sane; printf 'GOT\n'"#,
                ],
                PathBuf::from(env!("CARGO_MANIFEST_DIR")),
                20,
                4,
            ),
            scrollback_bytes: 100,
            initial_command: None,
            starting_sequence: 1,
        };
        let host = TerminalHost::spawn(options)
            .unwrap_or_else(|error| panic!("failed to spawn terminal host: {error}"));
        let receiver = host.event_receiver();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = String::new();
        let mut saw_exit = false;
        while Instant::now() < deadline && !saw_exit {
            match receiver.try_recv() {
                Ok(HostEvent::Frame(frame)) => {
                    for row in frame.rows_changed {
                        output.extend(row.cells.iter().map(|cell| cell.text.as_str()));
                        output.push('\n');
                    }
                }
                Ok(HostEvent::Exited(code)) => {
                    assert_eq!(code, Some(0));
                    saw_exit = true;
                }
                Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                Err(TryRecvError::Closed) => break,
            }
        }
        assert!(
            output.contains("GOT"),
            "the child did not receive the cursor-position response: {output:?}"
        );
        assert!(saw_exit, "querying child did not exit");
        host.join()
            .unwrap_or_else(|_| panic!("terminal host thread panicked"));
    }

    #[test]
    fn nvim_can_enter_insert_escape_and_quit() {
        let has_nvim = std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|directory| directory.join("nvim").is_file())
        });
        if !has_nvim {
            // The byte-level regression above always runs; this end-to-end proof is additive on
            // developer and CI machines that have nvim installed.
            return;
        }

        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("fleet-nvim-{nonce}"));
        std::fs::create_dir(&directory).unwrap_or_else(|error| {
            panic!(
                "failed to create nvim test directory {}: {error}",
                directory.display()
            )
        });
        let path = directory.join("buffer.txt");
        let options = TerminalHostOptions {
            terminal: TerminalId(19),
            pty: PtyOptions::command(
                "nvim",
                [
                    "--clean".into(),
                    "-n".into(),
                    "-u".into(),
                    "NONE".into(),
                    "--cmd".into(),
                    "set noswapfile".into(),
                    path.as_os_str().to_owned(),
                ],
                directory.clone(),
                80,
                24,
            ),
            scrollback_bytes: 100,
            initial_command: None,
            starting_sequence: 1,
        };
        let host = TerminalHost::spawn(options)
            .unwrap_or_else(|error| panic!("failed to spawn nvim terminal: {error}"));
        let receiver = host.event_receiver();
        let ready_by = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < ready_by && !ready {
            match receiver.try_recv() {
                Ok(HostEvent::Frame(frame)) => ready = frame.modes.alt_screen,
                Ok(HostEvent::Exited(code)) => panic!("nvim exited before input: {code:?}"),
                Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                Err(TryRecvError::Closed) => break,
            }
        }
        assert!(ready, "nvim never entered its alternate screen");

        let send = |key, mods, text: Option<&str>| {
            host.key(KeyEvent {
                key,
                mods,
                text: text.map(str::to_owned),
                action: fleet_proto::terminal::KeyAction::Press,
            })
            .unwrap_or_else(|error| panic!("failed to send nvim key: {error}"));
        };
        let plain = fleet_proto::terminal::Modifiers::empty();
        for character in ['i', 'f', 'l', 'e', 'e', 't'] {
            send(
                fleet_proto::terminal::Key::Char(character),
                plain,
                Some(&character.to_string()),
            );
        }
        send(fleet_proto::terminal::Key::Escape, plain, None);
        send(
            fleet_proto::terminal::Key::Char('a'),
            fleet_proto::terminal::Modifiers::SHIFT,
            Some("A"),
        );
        send(
            fleet_proto::terminal::Key::Char('['),
            fleet_proto::terminal::Modifiers::CTRL,
            None,
        );
        send(
            fleet_proto::terminal::Key::Char(';'),
            fleet_proto::terminal::Modifiers::SHIFT,
            Some(":"),
        );
        for character in ['w', 'q'] {
            send(
                fleet_proto::terminal::Key::Char(character),
                plain,
                Some(&character.to_string()),
            );
        }
        send(fleet_proto::terminal::Key::Enter, plain, None);

        let exit_by = Instant::now() + Duration::from_secs(5);
        let mut exit = None;
        while Instant::now() < exit_by && exit.is_none() {
            match receiver.try_recv() {
                Ok(HostEvent::Exited(code)) => exit = Some(code),
                Ok(_) | Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                Err(TryRecvError::Closed) => break,
            }
        }
        if exit.is_none() {
            let _ignored = host.kill();
        }
        assert_eq!(
            exit,
            Some(Some(0)),
            "nvim did not accept Esc, Ctrl-[, then :wq"
        );
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("nvim did not write {}: {error}", path.display())),
            "fleet\n"
        );
        std::fs::remove_file(&path)
            .unwrap_or_else(|error| panic!("failed to remove {}: {error}", path.display()));
        let _ignored = std::fs::remove_file(directory.join(".nvimlog"));
        std::fs::remove_dir(&directory).unwrap_or_else(|error| {
            panic!(
                "failed to remove nvim test directory {}: {error}",
                directory.display()
            )
        });
        host.join()
            .unwrap_or_else(|_| panic!("terminal host thread panicked"));
    }
}
