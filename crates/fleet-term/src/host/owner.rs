use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use async_channel::{Sender, TrySendError};
use fleet_core::ids::TerminalId;
use fleet_proto::terminal::{FrameUpdate, ScrollCommand};
use tracing::warn;

use super::{
    CommandReservation, HOST_EVENT_MAX_BYTES, HostCommand, HostEvent, TerminalActivity,
    host_event_bytes,
};
use crate::{
    GhosttyEngine,
    engine::{EngineError, EngineEvent, VtEngine, WheelAction},
    pty::{Pty, PtyError},
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
const EVENT_RETRY_INTERVAL: Duration = Duration::from_millis(16);
const PENDING_EVENT_BYTES: usize = 1024 * 1024;

pub(super) enum OwnerEvent {
    #[cfg(test)]
    Command(HostCommand),
    BudgetedCommand {
        command: HostCommand,
        _reservation: CommandReservation,
    },
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
    pending_events: VecDeque<HostEvent>,
    pending_event_bytes: usize,
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
            pending_events: VecDeque::new(),
            pending_event_bytes: 0,
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
                    self.kill();
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
            .chain((!self.pending_events.is_empty()).then(|| Instant::now() + EVENT_RETRY_INTERVAL))
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
            match self.engine.try_compress_idle() {
                Ok(()) => {
                    self.compression_at = self
                        .engine
                        .compression_pending()
                        .then(|| Instant::now() + COMPRESSION_INTERVAL);
                }
                Err(error) => {
                    warn!(%error, terminal = %self.terminal, "failed to compress terminal history");
                    self.compression_at = Some(Instant::now() + COMPRESSION_INTERVAL);
                }
            }
        }
    }

    fn take_frame(&mut self, full: bool) -> Result<FrameUpdate, EngineError> {
        #[cfg(test)]
        {
            self.frames_taken += 1;
        }
        let mut frame = self.engine.try_take_frame(full)?;
        self.compression_at
            .get_or_insert(Instant::now() + COMPRESSION_INTERVAL);
        frame.terminal = self.terminal;
        frame.seq = self.sequence;
        Ok(frame)
    }

    fn frame_sent(&mut self) {
        self.sequence = self.sequence.saturating_add(1);
        self.frame_delivered();
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
            let frame = match self.take_frame(self.force_full) {
                Ok(frame) => frame,
                Err(error) => {
                    warn!(%error, terminal = %self.terminal, "failed to snapshot terminal frame");
                    self.dirty = true;
                    self.force_full = true;
                    self.last_frame_at = Instant::now();
                    return true;
                }
            };
            if self.try_send_event(HostEvent::Frame(frame)).is_err() {
                if self.events.is_closed() {
                    self.kill();
                    return false;
                }
                self.dirty = true;
                self.force_full = true;
                self.last_frame_at = Instant::now();
                return true;
            }
            self.frame_sent();
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
            match self.take_frame(self.force_full) {
                Ok(frame) => {
                    if self.try_send_event(HostEvent::Frame(frame)).is_err() {
                        let frame = match self.take_frame(true) {
                            Ok(frame) => frame,
                            Err(error) => {
                                warn!(%error, terminal = %self.terminal, "failed to recover final terminal frame");
                                self.force_send_event(HostEvent::Exited(code));
                                return true;
                            }
                        };
                        if self.force_send_event(HostEvent::Frame(frame)) {
                            self.frame_sent();
                        }
                    } else {
                        self.frame_sent();
                    }
                }
                Err(error) => {
                    warn!(%error, terminal = %self.terminal, "failed to take final terminal frame");
                    self.dirty = true;
                    self.force_full = true;
                    self.exit = Some((code, Instant::now()));
                    return false;
                }
            }
        }
        if self.events.is_closed() {
            return true;
        }
        if self.try_send_event(HostEvent::Exited(code)).is_err() {
            self.force_send_event(HostEvent::Exited(code));
        }
        true
    }

    fn forward_engine_events(&mut self) {
        while let Some(event) = self.pending_events.pop_front() {
            self.pending_event_bytes = self
                .pending_event_bytes
                .saturating_sub(host_event_bytes(&event));
            if let Err(event) = self.try_send_event(event) {
                self.pending_event_bytes = self
                    .pending_event_bytes
                    .saturating_add(host_event_bytes(&event));
                self.pending_events.push_front(event);
                break;
            }
        }
        for event in self.engine.take_events() {
            let event = match event {
                EngineEvent::PtyWrite(bytes) => {
                    if let Err(error) = self.write(&bytes, None) {
                        warn!(%error, terminal = %self.terminal, "failed to write terminal PTY reply");
                    }
                    continue;
                }
                EngineEvent::Title(title) => HostEvent::Title(title),
                EngineEvent::Bell => HostEvent::Bell,
                EngineEvent::Cwd(cwd) => HostEvent::Cwd(cwd),
                EngineEvent::ClipboardWrite { mime, data } => {
                    HostEvent::ClipboardWrite { mime, data }
                }
            };
            if !self.pending_events.is_empty() {
                self.queue_pending_event(event);
            } else if let Err(event) = self.try_send_event(event) {
                self.queue_pending_event(event);
            }
        }
    }

    fn try_send_event(&self, event: HostEvent) -> Result<(), HostEvent> {
        if host_event_bytes(&event) > HOST_EVENT_MAX_BYTES {
            warn!(terminal = %self.terminal, "terminal event exceeded the queue byte budget");
            return Err(event);
        }
        self.events.try_send(event).map_err(|error| match error {
            TrySendError::Full(event) | TrySendError::Closed(event) => event,
        })
    }

    fn force_send_event(&self, event: HostEvent) -> bool {
        if host_event_bytes(&event) > HOST_EVENT_MAX_BYTES {
            warn!(terminal = %self.terminal, "terminal event exceeded the queue byte budget");
            return false;
        }
        self.events.force_send(event).is_ok()
    }

    fn queue_pending_event(&mut self, event: HostEvent) {
        let same_kind = |queued: &HostEvent| {
            matches!(
                (queued, &event),
                (HostEvent::Title(_), HostEvent::Title(_))
                    | (HostEvent::Cwd(_), HostEvent::Cwd(_))
                    | (HostEvent::Bell, HostEvent::Bell)
            )
        };
        if let Some(index) = self.pending_events.iter().position(same_kind)
            && let Some(removed) = self.pending_events.remove(index)
        {
            self.pending_event_bytes = self
                .pending_event_bytes
                .saturating_sub(host_event_bytes(&removed));
        }
        let bytes = host_event_bytes(&event);
        if bytes > PENDING_EVENT_BYTES
            || self.pending_event_bytes.saturating_add(bytes) > PENDING_EVENT_BYTES
        {
            warn!(terminal = %self.terminal, "terminal side effect exceeded the pending-event budget");
            return;
        }
        self.pending_event_bytes = self.pending_event_bytes.saturating_add(bytes);
        self.pending_events.push_back(event);
    }

    fn record_input(&mut self) {
        let now = Instant::now();
        self.activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_input_at = now;
        self.compression_at = Some(now + COMPRESSION_IDLE);
    }

    fn write(
        &mut self,
        bytes: &[u8],
        reservation: Option<CommandReservation>,
    ) -> Result<(), PtyError> {
        match reservation {
            Some(reservation) => self.pty.write_permitted(bytes, reservation),
            None => self.pty.write(bytes),
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
