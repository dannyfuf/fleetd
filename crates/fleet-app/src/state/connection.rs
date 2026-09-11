use super::*;

/// What FleetView knows about why its daemon link was lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonLossReason {
    /// The existing socket and a fresh recovery connection both failed.
    ConnectionLost,
    /// The daemon explicitly announced that it was shutting down.
    Stopped,
}

/// The three daemon situations of §3.12, plus the two transient banners that follow case C.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonLink {
    /// A. Cold start: `Starting fleetd…`, full window, no chrome, nothing bound.
    Starting,
    /// B. `fleetd could not start.` Full window with the log tail and recovery keys.
    Failed {
        /// The failure, as reported by the client.
        message: String,
        /// The last lines of `~/.fleet/logs/fleetd.log`.
        log_tail: Vec<String>,
        /// Whether a stale socket is the known cause.
        stale_socket: bool,
        /// Whether the daemon answered with an unsupported protocol.
        protocol_mismatch: bool,
    },
    /// Connected and answering pings.
    Connected,
    /// C. The daemon link was lost while attached; the client is backing off between reconnects.
    Lost {
        /// How many reconnect attempts have failed.
        attempt: u32,
        /// Whether `Esc` dismissed the banner. The dot stays red either way.
        dismissed: bool,
        /// Whether the daemon stopped or only the client connection was lost.
        reason: DaemonLossReason,
    },
    /// The daemon came back. The wording depends on whether the PTYs died with it (§3.12 D-17).
    Reconnected {
        /// True when fleetd restarted, so terminal sessions did **not** survive.
        restarted: bool,
        /// When the banner appeared.
        since: Instant,
    },
}

impl DaemonLink {
    /// Whether the app is fully usable.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        matches!(self, Self::Connected | Self::Reconnected { .. })
    }

    /// Whether mutating keys must be refused and terminal grids veiled (§3.12 C).
    #[must_use]
    pub const fn is_lost(&self) -> bool {
        matches!(self, Self::Lost { .. })
    }
}

/// The reconnect backoff of §3.12 C: 1, 2, 4, 8 seconds, capped at 8.
#[must_use]
pub fn reconnect_backoff(attempt: u32) -> Duration {
    let seconds = 1_u64 << attempt.min(3);
    Duration::from_secs(seconds.min(8))
}

/// `<home>/logs/fleetd.log`.
#[must_use]
pub fn daemon_log_path(home: &Path) -> PathBuf {
    home.join("logs").join("fleetd.log")
}

impl AppState {
    /// Whether keys typed into a terminal grid must be dropped rather than buffered (§3.12 C).
    #[must_use]
    pub const fn drops_terminal_keys(&self) -> bool {
        self.daemon.is_lost()
    }

    /// Whether a mutating key should flash the banner instead of acting (§3.12 C).
    #[must_use]
    pub const fn refuses_mutations(&self) -> bool {
        self.daemon.is_lost()
    }

    /// The path of `fleetd.log`, which the daemon surfaces open (§3.12).
    #[must_use]
    pub fn daemon_log_path(&self) -> PathBuf {
        daemon_log_path(&self.home)
    }

    /// Advances everything that expires on its own: toasts, the reconnect banner, the
    /// cold-start splash. Returns whether the frame has to be repainted.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = expire_toasts(&mut self.toasts, now)
            || self
                .active_session()
                .is_some_and(|s| self.watches.running_visible(&s.id));
        match self.daemon {
            DaemonLink::Reconnected { restarted, since } => {
                let dwell = if restarted {
                    RESTART_BANNER_DWELL
                } else {
                    RECONNECT_BANNER_DWELL
                };
                if now.saturating_duration_since(since) >= dwell {
                    self.daemon = DaemonLink::Connected;
                }
                changed = true;
            }
            // The spinner, the 3 s socket line and the reconnect countdown all animate.
            DaemonLink::Starting | DaemonLink::Lost { .. } => changed = true,
            DaemonLink::Connected | DaemonLink::Failed { .. } => {}
        }
        changed
    }

    /// Applies one message from the daemon bridge.
    pub fn apply_bridge_event(&mut self, event: BridgeEvent, now: Instant) {
        match event {
            BridgeEvent::Capabilities(capabilities) => {
                self.daemon_capabilities = capabilities.into_iter().collect();
            }
            BridgeEvent::EffectiveConfig(config) => {
                self.terminal_config = config.terminal;
                self.notifications = config.notifications;
                self.warn_before_quit = config.warn_before_quit;
            }
            BridgeEvent::Connected(snapshot) => {
                self.daemon = DaemonLink::Connected;
                self.daemon_since = now;
                self.link_generation = self.link_generation.wrapping_add(1);
                self.clear_board();
                self.watches.reconnect();
                self.seed_agent_activity(&snapshot, now);
                self.apply_snapshot(*snapshot, now);
            }
            BridgeEvent::ConnectFailed {
                message,
                log_tail,
                stale_socket,
            } => {
                self.daemon_capabilities.clear();
                self.daemon = DaemonLink::Failed {
                    message,
                    log_tail,
                    stale_socket,
                    protocol_mismatch: false,
                };
                self.daemon_since = now;
                // §3.12 B takes the whole window and draws no overlay layer, so anything that
                // was open would keep its key context alive with nothing on screen to close.
                self.overlay = None;
            }
            BridgeEvent::ProtocolMismatch { message, log_tail } => {
                self.daemon_capabilities.clear();
                self.daemon = DaemonLink::Failed {
                    message,
                    log_tail,
                    stale_socket: false,
                    protocol_mismatch: true,
                };
                self.daemon_since = now;
                self.overlay = None;
            }
            BridgeEvent::Disconnected { attempt } => {
                self.clear_board();
                self.daemon_capabilities.clear();
                let (dismissed, reason) = match self.daemon {
                    DaemonLink::Lost {
                        dismissed, reason, ..
                    } => (dismissed, reason),
                    _ => (false, DaemonLossReason::ConnectionLost),
                };
                self.daemon = DaemonLink::Lost {
                    attempt,
                    dismissed,
                    reason,
                };
                self.daemon_since = now;
            }
            BridgeEvent::Reconnected {
                restarted,
                snapshot,
            } => {
                if restarted {
                    // PTYs do not survive a daemon restart.
                    self.grids.clear();
                    self.watches = crate::watches::Watches::default();
                    self.renamed_terminals.clear();
                    self.terminal_mru.clear();
                }
                self.daemon = DaemonLink::Reconnected {
                    restarted,
                    since: now,
                };
                self.daemon_since = now;
                // The daemon-side connection is new and holds no attachments, whether or not
                // fleetd itself restarted.
                self.link_generation = self.link_generation.wrapping_add(1);
                self.clear_board();
                self.watches.reconnect();
                self.seed_agent_activity(&snapshot, now);
                self.apply_snapshot(*snapshot, now);
            }
            BridgeEvent::Daemon(event) => self.apply_daemon_event(*event, now),
            BridgeEvent::Agent { thread, event } => {
                if let Some(stale) = self.apply_agent_event(thread, &event) {
                    self.agents.mark_resync(stale);
                }
                self.notify_agent_attention(now);
            }
            BridgeEvent::AgentSummary(summary) => self.apply_agent_summary(summary, now),
            BridgeEvent::MutationFailed { message } => {
                self.sticky_error = Some(StickyError {
                    text: message,
                    job: None,
                    retryable: false,
                });
            }
            // Lag also affects quiet terminals. Shell requests full frames for these mirrors.
            BridgeEvent::EventsLagged { .. } => {
                self.board_stale = true;
                self.desync_grids();
                self.watches.invalidate();
            }
        }
    }

    /// Applies one ordinary daemon event to the mirror.
    pub fn apply_daemon_event(&mut self, event: Event, now: Instant) {
        match event {
            Event::BoardChanged { board_id, .. } => {
                if self.board().is_some_and(|view| view.board.id == board_id) || self.board.loading
                {
                    self.board_stale = true;
                }
            }
            Event::WatchStarted(watch) => self.watches.started(watch, now),
            Event::WatchOutput { watch, chunks } => self.watches.output(watch, chunks),
            Event::WatchExited(watch) => self.watches.exited(watch, now),
            Event::WatchDismissed(id) => self.watches.dismissed(id),
            Event::SnapshotChanged(snapshot) => self.apply_snapshot(snapshot, now),
            Event::JobUpdated(job) => self.apply_job(job, now),
            Event::SessionChanged(session) => self.apply_session(session),
            Event::AgentActivityChanged {
                session,
                terminal_id,
                agent,
                activity,
                attention,
                changed_at,
            } => self.apply_agent_activity(
                session,
                terminal_id,
                agent,
                (activity, attention),
                changed_at,
                now,
            ),
            Event::Agent { thread, event } => {
                if let Some(stale) = self.apply_agent_event(thread, &event) {
                    self.agents.mark_resync(stale);
                }
                self.notify_agent_attention(now);
            }
            Event::AgentSummary(summary) => self.apply_agent_summary(summary, now),
            // Backpressure dropped this connection's tail for one thread. Re-opening from the
            // cursor the daemon names is the whole repair, and it is never a silent drop.
            Event::AgentResync { thread, from_seq } => {
                self.agents.mark_resync_from(thread, from_seq);
            }
            // The only transition into live: a mirror never fabricates one.
            Event::AgentSynchronized { thread } => self.agents.synchronize(thread),
            // The local daemon refilled a mirrored thread's stored window, so the client
            // re-reads the window it has open rather than trusting what it cached.
            Event::AgentWindow { thread } => self.agents.mark_resync(thread),
            // An event family this build does not understand is ignored, never re-broadcast.
            Event::Unknown => {}
            Event::TerminalFrame(frame) => {
                self.apply_frame(&frame);
            }
            Event::TerminalExited { terminal, code } => self.apply_terminal_exit(terminal, code),
            Event::TerminalTitle { terminal, title } => self.apply_terminal_title(terminal, title),
            Event::HostLinkChanged {
                host,
                link,
                version,
                error,
            } => self.apply_host_link(&host, link, version, error),
            // §3: the daemon rebuilt this client's attachment set behind an id that did not
            // change, so only an explicit re-attach brings the mirror back. The surface that
            // owns the terminal consumes the request on its next frame.
            Event::TerminalReattach { terminal } => {
                self.reattach_pending.insert(terminal);
            }
            Event::Toast { level, message } => self.apply_toast_event(level, message, now),
            Event::DaemonShuttingDown => {
                self.daemon = DaemonLink::Lost {
                    attempt: 0,
                    dismissed: false,
                    reason: DaemonLossReason::Stopped,
                };
                self.daemon_since = now;
            }
        }
    }
}

#[cfg(test)]
mod tests;
