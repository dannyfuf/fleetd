use super::*;

const ATTACH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl Sessions {
    /// Attaches a client, makes its size authoritative, and emits a full frame.
    pub async fn attach(&self, terminal: TerminalId, cols: u16, rows: u16) -> DaemonResult<()> {
        let host = self.host_or_not_found(terminal)?;
        let deadline = tokio::time::Instant::now() + ATTACH_TIMEOUT;
        let frame = await_attach(
            terminal,
            deadline,
            host.attach(cols, rows, deadline.into_std()),
        )
        .await?;
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !registry.terminal_sessions.contains_key(&terminal) {
            return Err(DaemonError::NotFound(terminal.to_string()));
        }
        let next = registry.next_sequences.entry(terminal).or_insert(1);
        *next = (*next).max(frame.seq.saturating_add(1));
        let _ = self.runtime.frames.send(frame);
        *registry.attachments.entry(terminal).or_default() += 1;
        drop(registry);
        self.runtime.notify_terminal(terminal);
        Ok(())
    }

    /// Removes one client attachment without stopping the terminal.
    pub async fn detach(&self, terminal: TerminalId) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?;
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = registry.attachments.get_mut(&terminal) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                registry.attachments.remove(&terminal);
            }
        }
        drop(registry);
        self.runtime.notify_terminal(terminal);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn attachment_count(&self, terminal: TerminalId) -> usize {
        self.runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attachments
            .get(&terminal)
            .copied()
            .unwrap_or_default()
    }

    /// Writes already encoded bytes to the terminal process.
    pub async fn input(&self, terminal: TerminalId, bytes: Vec<u8>) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .write(bytes)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Encodes a semantic key using current terminal modes.
    pub async fn key(&self, terminal: TerminalId, key: KeyEvent) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .key(key)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Encodes mouse reporting only when the terminal requests it.
    pub async fn mouse(&self, terminal: TerminalId, mouse: MouseEvent) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .mouse(mouse)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Resizes a PTY, with the most recent client dimensions winning.
    pub async fn resize(&self, terminal: TerminalId, cols: u16, rows: u16) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .resize(cols, rows)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Moves the server-side scrollback viewport without affecting the process.
    pub async fn scroll(&self, terminal: TerminalId, scroll: ScrollCommand) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .scroll(scroll)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Routes wheel input on the owning host thread.
    pub async fn wheel(&self, terminal: TerminalId, wheel: WheelEvent) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .wheel(wheel)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Routes a viewport shortcut on the owning host thread.
    pub async fn scroll_or_key(
        &self,
        terminal: TerminalId,
        scroll: ScrollCommand,
        key: KeyEvent,
    ) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .scroll_or_key(scroll, key)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Emits a complete replacement frame after attach or sequence loss.
    pub async fn request_full_frame(&self, terminal: TerminalId) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .request_full()
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Pastes text with bracketed-paste encoding when enabled.
    pub async fn paste(&self, terminal: TerminalId, text: String) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .paste(text)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Subscribes a connection actor to all terminal frames for attachment filtering.
    pub fn subscribe_frames(&self) -> broadcast::Receiver<FrameUpdate> {
        self.runtime.frames.subscribe()
    }

    fn host_or_not_found(&self, terminal: TerminalId) -> DaemonResult<Arc<TerminalHost>> {
        self.runtime
            .host(terminal)
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))
    }
}

pub(super) async fn await_attach(
    terminal: TerminalId,
    deadline: tokio::time::Instant,
    attach: impl std::future::Future<Output = Result<FrameUpdate, fleet_term::HostError>>,
) -> DaemonResult<FrameUpdate> {
    tokio::time::timeout_at(deadline, attach)
        .await
        .map_err(|_| DaemonError::Timeout(format!("terminal {terminal} attachment")))?
        .map_err(|error| terminal_error(terminal, error))
}

impl SessionRuntime {
    pub(super) fn notify_session(&self, id: &SessionId) {
        let session = self.session(id);
        let events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(events) = events {
            if let Some(session) = session {
                events.publish(Event::SessionChanged(session));
            }
            events.request_snapshot_current();
        }
    }

    pub(super) fn notify_snapshot(&self) {
        if let Some(events) = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            events.request_snapshot_current();
        }
    }

    fn notify_terminal(&self, terminal: TerminalId) {
        let session = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal_sessions
            .get(&terminal)
            .cloned();
        if let Some(session) = session {
            self.notify_session(&session);
        } else {
            self.notify_snapshot();
        }
    }

    pub(super) fn publish(&self, event: Event) {
        if let Some(events) = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            events.publish(event);
        }
    }
}

pub(super) async fn spawn_terminal(
    terminal: TerminalId,
    session: SessionId,
    name: String,
    command: String,
    cwd: String,
    scrollback_bytes: usize,
    starting_sequence: u64,
) -> DaemonResult<(Terminal, TerminalHost)> {
    tokio::task::spawn_blocking(move || {
        let pty = PtyOptions::shell(
            PathBuf::from(&cwd),
            session.as_str(),
            &name,
            terminal,
            INITIAL_COLS,
            INITIAL_ROWS,
        );
        let host = TerminalHost::spawn(TerminalHostOptions {
            terminal,
            pty,
            scrollback_bytes,
            starting_sequence,
            initial_command: Some(command.clone()),
        })
        .map_err(|error| terminal_error(terminal, error))?;
        let shell_pid = host.child_pid();
        let Some(shell_pid) = shell_pid else {
            let _ = host.kill();
            return Err(DaemonError::Process(format!(
                "terminal `{terminal}` did not report its login-shell pid"
            )));
        };
        Ok((
            Terminal {
                id: terminal,
                name,
                command,
                cwd,
                shell_pid: Some(shell_pid),
                foreground_command: None,
                status: TerminalStatus::Running,
                title: None,
                keep_alive: Vec::new(),
                has_unseen_output: false,
                agent_attention: None,
                kind: TerminalKind::Pty,
            },
            host,
        ))
    })
    .await
    .map_err(|error| DaemonError::Join(error.to_string()))?
}

pub(super) struct HostEventForwarder {
    start: std::sync::mpsc::SyncSender<()>,
}

impl HostEventForwarder {
    pub(super) fn start(self) -> DaemonResult<()> {
        self.start.send(()).map_err(|_| {
            DaemonError::Process("terminal event thread stopped before registration".to_owned())
        })
    }
}

pub(super) fn prepare_host_events(
    runtime: Arc<SessionRuntime>,
    terminal: TerminalId,
    host: Arc<TerminalHost>,
) -> DaemonResult<HostEventForwarder> {
    let receiver = host.event_receiver();
    let runtime = Arc::downgrade(&runtime);
    let (start, registered) = std::sync::mpsc::sync_channel(0);
    thread::Builder::new()
        .name(format!("fleet-terminal-events-{terminal}"))
        .spawn(move || {
            if registered.recv().is_err() {
                return;
            }
            while let Ok(event) = receiver.recv_blocking() {
                let Some(runtime) = runtime.upgrade() else {
                    break;
                };
                match event {
                    HostEvent::Frame(frame) => {
                        let output_bytes = host.activity().output_bytes_total;
                        let mut registry = runtime
                            .registry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if !registry.terminal_sessions.contains_key(&terminal) {
                            continue;
                        }
                        let next = registry.next_sequences.entry(terminal).or_insert(1);
                        *next = (*next).max(frame.seq.saturating_add(1));
                        let became_unseen =
                            record_frame_activity(&mut registry, terminal, output_bytes);
                        drop(registry);
                        let _ = runtime.frames.send(frame);
                        // One notification per background terminal, when its unseen-output
                        // flag first turns on. Subsequent frames change no session state.
                        if let Some(session_id) = became_unseen {
                            runtime.notify_session(&session_id);
                        }
                    }
                    HostEvent::Exited(code) => {
                        update_terminal(&runtime, terminal, |entry| {
                            entry.status = TerminalStatus::Exited { code };
                            entry.foreground_command = None;
                        });
                        runtime.publish(Event::TerminalExited { terminal, code });
                    }
                    HostEvent::Title(title) => {
                        update_terminal(&runtime, terminal, |entry| {
                            entry.title = Some(title.clone());
                        });
                        runtime.publish(Event::TerminalTitle { terminal, title });
                    }
                    HostEvent::Bell | HostEvent::Cwd(_) | HostEvent::ClipboardWrite { .. } => {}
                }
            }
        })
        .map_err(|error| DaemonError::Process(format!("terminal event thread: {error}")))?;
    Ok(HostEventForwarder { start })
}

pub(super) fn record_frame_activity(
    registry: &mut Registry,
    terminal: TerminalId,
    output_bytes: u64,
) -> Option<SessionId> {
    let previous_output = registry
        .observed_output_bytes
        .insert(terminal, output_bytes)
        .unwrap_or(0);
    if output_bytes <= previous_output {
        return None;
    }
    let session_id = registry.terminal_sessions.get(&terminal)?.clone();
    let session = registry.sessions.get_mut(&session_id)?;
    if session.active_terminal == Some(terminal) {
        return None;
    }
    let entry = session
        .terminals
        .iter_mut()
        .find(|entry| entry.id == terminal)?;
    if entry.has_unseen_output {
        return None;
    }
    entry.has_unseen_output = true;
    Some(session_id)
}

fn update_terminal(
    runtime: &SessionRuntime,
    terminal: TerminalId,
    update: impl FnOnce(&mut Terminal),
) {
    let mut registry = runtime
        .registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session_id) = registry.terminal_sessions.get(&terminal).cloned() else {
        return;
    };
    let Some(entry) = registry.sessions.get_mut(&session_id).and_then(|session| {
        session
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
    }) else {
        return;
    };
    update(entry);
    drop(registry);
    runtime.notify_session(&session_id);
}

fn terminal_error(terminal: TerminalId, error: impl std::fmt::Display) -> DaemonError {
    DaemonError::Process(format!("terminal `{terminal}`: {error}"))
}
