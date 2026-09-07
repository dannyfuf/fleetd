use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use async_channel::Sender;
use fleet_core::ids::TerminalId;
use fleet_proto::terminal::{FrameUpdate, ScrollCommand};
use tracing::warn;

use super::{HostCommand, HostEvent, TerminalActivity};
use crate::{
    GhosttyEngine,
    engine::{EngineEvent, VtEngine, WheelAction},
    pty::Pty,
};

mod commands;
#[cfg(test)]
mod tests;

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const PROMPT_AFTER_OUTPUT: Duration = Duration::from_millis(150);
const PROMPT_FALLBACK: Duration = Duration::from_millis(500);
const EXIT_DRAIN_GRACE: Duration = Duration::from_millis(100);
const COMPRESSION_IDLE: Duration = Duration::from_millis(250);
const COMPRESSION_INTERVAL: Duration = Duration::from_millis(16);
const OUTPUT_BATCH_BYTES: usize = 256 * 1024;
const OUTPUT_BATCH_TIME: Duration = Duration::from_millis(2);
const COMMAND_BATCH_LIMIT: usize = 1024;

pub(super) enum OwnerEvent {
    Command(HostCommand),
    CommandsClosed,
    PtyReady,
}

/// Readiness is coalesced, but output bytes and commands keep their FIFO queues.
pub(super) struct PtyWakeup {
    pending: AtomicBool,
    events: mpsc::Sender<OwnerEvent>,
}

impl PtyWakeup {
    pub(super) fn new(events: mpsc::Sender<OwnerEvent>) -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicBool::new(false),
            events,
        })
    }

    pub(super) fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.events.send(OwnerEvent::PtyReady);
        }
    }
}

pub(super) struct TerminalOwner {
    terminal: TerminalId,
    pty: Pty,
    engine: GhosttyEngine,
    inbox: mpsc::Receiver<OwnerEvent>,
    events: Sender<HostEvent>,
    activity: Arc<Mutex<TerminalActivity>>,
    wakeup: Arc<PtyWakeup>,
    /// The one shell command typed after the prompt-readiness heuristic fires.
    pending_command: Option<(String, Instant)>,
    spawned_at: Instant,
    first_output_at: Option<Instant>,
    dirty: bool,
    force_full: bool,
    viewport_moved: bool,
    sequence: u64,
    last_frame_at: Instant,
    compression_at: Option<Instant>,
    exit: Option<(Option<i32>, Instant)>,
    commands_closed: bool,
    #[cfg(test)]
    iterations: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    frames_taken: usize,
}

impl TerminalOwner {
    pub(super) fn new(
        terminal: TerminalId,
        pty: Pty,
        engine: GhosttyEngine,
        inbox: mpsc::Receiver<OwnerEvent>,
        events: Sender<HostEvent>,
        activity: Arc<Mutex<TerminalActivity>>,
        wakeup: Arc<PtyWakeup>,
    ) -> Self {
        let spawned_at = Instant::now();
        Self {
            terminal,
            pty,
            engine,
            inbox,
            events,
            activity,
            wakeup,
            pending_command: None,
            spawned_at,
            first_output_at: None,
            dirty: false,
            force_full: false,
            viewport_moved: false,
            sequence: 0,
            last_frame_at: spawned_at.checked_sub(FRAME_INTERVAL).unwrap_or(spawned_at),
            compression_at: Some(spawned_at + COMPRESSION_IDLE),
            exit: None,
            commands_closed: false,
            #[cfg(test)]
            iterations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            frames_taken: 0,
        }
    }

    pub(super) fn run(mut self, initial_command: Option<String>, starting_sequence: u64) {
        self.sequence = starting_sequence;
        self.pending_command =
            initial_command.map(|command| (command, self.spawned_at + PROMPT_FALLBACK));
        let mut received = None;
        loop {
            #[cfg(test)]
            self.iterations.fetch_add(1, Ordering::Relaxed);
            self.drain_output();
            self.drain_commands(received.take());
            self.compress_if_ready();
            self.forward_engine_events();
            self.type_ready_command();
            if !self.deliver_frame_if_ready() || self.finish_if_exited() {
                break;
            }
            if self.commands_closed {
                self.kill();
                break;
            }
            // A full output batch leaves work queued, even if its readiness was coalesced.
            if self.pty.has_output() {
                continue;
            }
            received = self.wait_for_event();
        }
    }

    fn drain_output(&mut self) {
        let mut output_bytes = 0;
        let started = Instant::now();
        while output_bytes < OUTPUT_BATCH_BYTES && started.elapsed() < OUTPUT_BATCH_TIME {
            match self.pty.try_read() {
                Ok(Some(bytes)) => {
                    let now = Instant::now();
                    if self.first_output_at.is_none() {
                        self.first_output_at = Some(now);
                        if let Some((_, deadline)) = &mut self.pending_command {
                            *deadline = (*deadline).min(now + PROMPT_AFTER_OUTPUT);
                        }
                    }
                    output_bytes += bytes.len();
                    {
                        let mut activity = self
                            .activity
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        activity.last_output_at = now;
                        activity.output_bytes_total = activity
                            .output_bytes_total
                            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                    }
                    self.compression_at = Some(now + COMPRESSION_IDLE);
                    self.engine.feed(&bytes);
                    self.dirty = true;
                }
                Ok(None) => break,
                Err(error) => {
                    warn!(%error, terminal = %self.terminal, "terminal PTY reader failed");
                    break;
                }
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.pending_command
            .iter()
            .map(|(_, deadline)| *deadline)
            .chain(self.compression_at)
            .chain(self.dirty.then_some(self.last_frame_at + FRAME_INTERVAL))
            .chain(self.exit.map(|(_, observed)| observed + EXIT_DRAIN_GRACE))
            .min()
    }

    fn wait_for_event(&mut self) -> Option<OwnerEvent> {
        let result = match self.next_deadline() {
            Some(deadline) => self
                .inbox
                .recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => self
                .inbox
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match result {
            Ok(event) => Some(event),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.commands_closed = true;
                None
            }
        }
    }

    fn compress_if_ready(&mut self) {
        if self
            .compression_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.engine.compress_idle();
            self.compression_at = self
                .engine
                .compression_pending()
                .then(|| Instant::now() + COMPRESSION_INTERVAL);
        }
    }

    fn take_frame(&mut self, full: bool) -> FrameUpdate {
        #[cfg(test)]
        {
            self.frames_taken += 1;
        }
        let mut frame = self.engine.take_frame(full);
        self.compression_at
            .get_or_insert(Instant::now() + COMPRESSION_INTERVAL);
        frame.terminal = self.terminal;
        frame.seq = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        frame
    }

    fn frame_delivered(&mut self) {
        self.dirty = false;
        self.force_full = false;
        self.viewport_moved = false;
        self.last_frame_at = Instant::now();
    }

    fn deliver_frame_if_ready(&mut self) -> bool {
        if self.force_full
            || self.viewport_moved
            || (self.dirty && self.last_frame_at.elapsed() >= FRAME_INTERVAL)
        {
            let frame = self.take_frame(self.force_full);
            if self.events.send_blocking(HostEvent::Frame(frame)).is_err() {
                self.kill();
                return false;
            }
            self.frame_delivered();
        }
        true
    }

    fn finish_if_exited(&mut self) -> bool {
        if self.exit.is_none() {
            match self.pty.try_wait() {
                Ok(Some(code)) => self.exit = Some((Some(code), Instant::now())),
                Ok(None) => {}
                Err(error) => {
                    warn!(%error, terminal = %self.terminal, "failed to observe terminal child exit");
                    self.exit = Some((None, Instant::now()));
                }
            }
        }
        let Some((code, observed)) = self.exit else {
            return false;
        };
        if !self.pty.output_closed() && observed.elapsed() < EXIT_DRAIN_GRACE {
            return false;
        }
        if self.dirty || self.force_full || self.viewport_moved {
            let remaining = FRAME_INTERVAL.saturating_sub(self.last_frame_at.elapsed());
            thread::sleep(remaining);
            let frame = self.take_frame(self.force_full);
            let _ = self.events.send_blocking(HostEvent::Frame(frame));
        }
        let _ = self.events.send_blocking(HostEvent::Exited(code));
        true
    }

    fn forward_engine_events(&mut self) {
        for event in self.engine.take_events() {
            let event = match event {
                EngineEvent::PtyWrite(bytes) => {
                    self.write(&bytes);
                    continue;
                }
                EngineEvent::Title(title) => HostEvent::Title(title),
                EngineEvent::Bell => HostEvent::Bell,
                EngineEvent::Cwd(cwd) => HostEvent::Cwd(cwd),
                EngineEvent::ClipboardWrite { mime, data } => {
                    HostEvent::ClipboardWrite { mime, data }
                }
            };
            let _ = self.events.send_blocking(event);
        }
    }

    fn record_input(&mut self) {
        let now = Instant::now();
        self.activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_input_at = now;
        self.compression_at = Some(now + COMPRESSION_IDLE);
    }

    fn write(&mut self, bytes: &[u8]) {
        if !bytes.is_empty()
            && let Err(error) = self.pty.write(bytes)
        {
            warn!(%error, terminal = %self.terminal, "failed to write terminal PTY");
        }
    }

    fn kill(&mut self) {
        if let Err(error) = self.pty.kill() {
            warn!(%error, terminal = %self.terminal, "failed to kill terminal child");
        }
    }
}

impl Drop for TerminalOwner {
    fn drop(&mut self) {
        if self.exit.is_none() {
            self.kill();
        }
    }
}
