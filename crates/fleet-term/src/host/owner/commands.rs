use super::*;

/// Fold signed/absolute moves while preserving clamping at each command boundary.
pub(super) struct PendingViewport {
    offset: usize,
    history: usize,
    rows: u16,
}

impl PendingViewport {
    pub(super) fn new(engine: &dyn VtEngine) -> Self {
        let viewport = engine.viewport();
        Self {
            offset: viewport.offset,
            history: viewport.scrollback_len,
            rows: engine.rows(),
        }
    }

    pub(super) fn push(&mut self, command: ScrollCommand) {
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

pub(super) fn flush_viewport(
    pending: &mut Option<PendingViewport>,
    engine: &mut dyn VtEngine,
    viewport_moved: &mut bool,
) {
    if let Some(pending) = pending.take()
        && pending.offset != engine.viewport_offset()
    {
        engine.scroll(ScrollCommand::ToOffset(pending.offset));
        *viewport_moved = true;
    }
}

fn follow_input(engine: &mut dyn VtEngine, viewport_moved: &mut bool) {
    if engine.viewport_offset() > 0 {
        engine.scroll(ScrollCommand::Bottom);
        *viewport_moved = true;
    }
}

fn resize(
    pty: &dyn PtyBackend,
    engine: &mut dyn VtEngine,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    pty.resize(cols, rows).map_err(|error| error.to_string())?;
    engine.resize(cols, rows).map_err(|error| error.to_string())
}

impl TerminalOwner {
    pub(super) fn drain_commands(&mut self, first: Option<OwnerEvent>) {
        let mut first = first;
        let mut viewport = None;
        // Bound input work as well as PTY output, so neither can starve frame delivery.
        for _ in 0..COMMAND_BATCH_LIMIT {
            let (command, reservation) = match first
                .take()
                .map(Ok)
                .unwrap_or_else(|| self.inbox.try_recv())
            {
                #[cfg(test)]
                Ok(OwnerEvent::Command(command)) => (command, None),
                Ok(OwnerEvent::BudgetedCommand {
                    command,
                    _reservation,
                }) => (command, Some(_reservation)),
                Ok(OwnerEvent::PtyReady) => {
                    self.wakeup.pending.store(false, Ordering::Release);
                    continue;
                }
                Ok(OwnerEvent::CommandsClosed) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.commands_closed = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            };
            let command = match command {
                HostCommand::Wheel(event) => match self.engine.try_wheel(&event) {
                    Ok(WheelAction::Viewport(steps)) => {
                        HostCommand::Scroll(ScrollCommand::Lines(steps))
                    }
                    Ok(WheelAction::Pty(bytes)) => {
                        self.flush_viewport(&mut viewport);
                        self.write_command_input(&bytes, reservation);
                        continue;
                    }
                    Ok(WheelAction::Drop) => continue,
                    Err(error) => {
                        tracing::warn!(%error, terminal = %self.terminal, "failed to encode terminal wheel event");
                        continue;
                    }
                },
                HostCommand::ScrollOrKey { scroll, key } => {
                    if self.engine.modes().alt_screen {
                        HostCommand::Key(key)
                    } else {
                        HostCommand::Scroll(scroll)
                    }
                }
                other => other,
            };
            if let HostCommand::Scroll(command) = command {
                viewport
                    .get_or_insert_with(|| PendingViewport::new(&self.engine))
                    .push(command);
                continue;
            }
            // Preserve input/resize ordering across scroll batches.
            self.flush_viewport(&mut viewport);
            self.apply_command(command, reservation);
        }
        self.flush_viewport(&mut viewport);
        self.forward_engine_events();
    }

    fn apply_command(&mut self, command: HostCommand, reservation: Option<CommandReservation>) {
        match command {
            HostCommand::Write(bytes) => {
                follow_input(&mut self.engine, &mut self.viewport_moved);
                self.write_command_input(&bytes, reservation);
            }
            HostCommand::Key(event) => {
                follow_input(&mut self.engine, &mut self.viewport_moved);
                match self.engine.try_encode_key(&event) {
                    Ok(bytes) => self.write_command_input(&bytes, reservation),
                    Err(error) => {
                        tracing::warn!(%error, terminal = %self.terminal, "failed to encode terminal key")
                    }
                }
            }
            HostCommand::Mouse(event) => match self.engine.try_encode_mouse(&event) {
                Ok(bytes) => self.write_command_input(&bytes, reservation),
                Err(error) => {
                    tracing::warn!(%error, terminal = %self.terminal, "failed to encode terminal mouse event")
                }
            },
            HostCommand::Paste(text) => {
                follow_input(&mut self.engine, &mut self.viewport_moved);
                match self.engine.try_encode_paste(&text) {
                    Ok(bytes) => self.write_command_input(&bytes, reservation),
                    Err(error) => {
                        tracing::warn!(%error, terminal = %self.terminal, "failed to encode terminal paste")
                    }
                }
            }
            HostCommand::Resize { cols, rows } => {
                match resize(self.pty.as_ref(), &mut self.engine, cols, rows) {
                    Ok(()) => {
                        self.force_full = true;
                        self.compression_at
                            .get_or_insert(Instant::now() + COMPRESSION_IDLE);
                    }
                    Err(error) => {
                        tracing::warn!(%error, terminal = %self.terminal, "failed to resize terminal")
                    }
                }
            }
            HostCommand::Attach {
                cols,
                rows,
                deadline,
                reply,
            } => self.attach(cols, rows, deadline, reply),
            HostCommand::RequestFull => self.force_full = true,
            HostCommand::Kill => self.kill(),
            HostCommand::Scroll(_) | HostCommand::Wheel(_) | HostCommand::ScrollOrKey { .. } => {
                unreachable!("viewport commands are resolved before ordered dispatch")
            }
        }
    }

    fn write_command_input(&mut self, bytes: &[u8], reservation: Option<CommandReservation>) {
        match self.write(bytes, reservation) {
            Ok(()) => self.record_input(),
            Err(error) => {
                tracing::warn!(%error, terminal = %self.terminal, "failed to write terminal PTY input")
            }
        }
    }

    /// Reports why an attachment was refused; the requester is gone only when it already
    /// gave up on its own deadline, so a failed send is an anomaly worth a line.
    fn refuse_attach(&self, reply: &Sender<Result<FrameUpdate, String>>, error: String) {
        if reply.try_send(Err(error)).is_err() {
            tracing::warn!(terminal = %self.terminal, "attach requester stopped waiting");
        }
    }

    fn attach(
        &mut self,
        cols: u16,
        rows: u16,
        deadline: Instant,
        reply: Sender<Result<FrameUpdate, String>>,
    ) {
        if deadline <= Instant::now() {
            self.refuse_attach(&reply, "attachment deadline elapsed".to_owned());
            return;
        }
        if let Err(error) = resize(self.pty.as_ref(), &mut self.engine, cols, rows) {
            self.refuse_attach(&reply, error);
            return;
        }
        self.compression_at
            .get_or_insert(Instant::now() + COMPRESSION_IDLE);
        let mut frame = match self.take_frame(true) {
            Ok(frame) => frame,
            Err(error) => {
                self.dirty = true;
                self.force_full = true;
                self.refuse_attach(&reply, error.to_string());
                return;
            }
        };
        if deadline <= Instant::now() {
            self.dirty = true;
            self.force_full = true;
            self.refuse_attach(&reply, "attachment deadline elapsed".to_owned());
            return;
        }
        if reply.try_send(Ok(frame.clone())).is_err() {
            self.dirty = true;
            self.force_full = true;
            return;
        }
        // Keep both delivery sequences, but snapshot the emulator only once.
        self.sequence = self.sequence.saturating_add(1);
        frame.seq = self.sequence;
        if self.try_send_event(HostEvent::Frame(frame)).is_ok() {
            self.sequence = self.sequence.saturating_add(1);
            self.frame_delivered();
        } else {
            self.dirty = true;
            self.force_full = true;
        }
    }

    fn flush_viewport(&mut self, pending: &mut Option<PendingViewport>) {
        flush_viewport(pending, &mut self.engine, &mut self.viewport_moved);
        if self.viewport_moved {
            self.compression_at
                .get_or_insert(Instant::now() + COMPRESSION_IDLE);
        }
    }

    pub(super) fn type_ready_command(&mut self) {
        match self.pending_command.take() {
            Some((mut command, deadline)) if deadline <= Instant::now() => {
                command.push('\r');
                self.record_input();
                if let Err(error) = self.pty.write(command.as_bytes()) {
                    tracing::warn!(%error, terminal = %self.terminal, "failed to type initial terminal command");
                }
            }
            pending => self.pending_command = pending,
        }
    }
}
