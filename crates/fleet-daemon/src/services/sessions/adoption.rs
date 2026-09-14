//! Reattaching at startup to the PTY holders that outlived the previous daemon.

use super::*;

use std::sync::atomic::Ordering;

use holder::DiscoveredSidecar;
use host_bridge::{HolderAttachFailure, HolderAttachment};

/// Ceiling on the whole adoption pass, however many holders were recorded.
///
/// Adoption runs before the daemon serves anyone, so its cost is startup latency the app's
/// readiness probe is waiting on. Each holder is attached concurrently under this one deadline
/// rather than sequentially under its own, so ten slow holders cost one timeout, not ten.
const ADOPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long `DaemonShutdown { stop_sessions: true }` waits for the killed holders to go away.
const STOP_HOLDERS_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
/// Interval between holder liveness checks while waiting for them to stop.
const STOP_HOLDERS_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// What one recorded holder turned into.
enum Adopted {
    /// The terminal is back in the registry.
    Terminal(Box<AdoptedTerminal>),
    /// Nothing to adopt; the record was dealt with.
    Nothing,
}

/// A reattached terminal, ready to be inserted once every holder has answered.
struct AdoptedTerminal {
    sidecar: holder::PtySidecar,
    host: TerminalHost,
    shell_pid: Option<u32>,
}

impl Sessions {
    /// Reattaches every live holder recorded under `<fleet-home>/pty`, dropping the stale records.
    ///
    /// Runs once during startup, before the daemon serves any client, so the first snapshot a
    /// client receives already lists the sessions that survived the restart. Terminal identifiers
    /// are reused verbatim, which is what lets the app's tabs reconnect without noticing.
    ///
    /// A holder that is alive but does not answer keeps its record: it may simply be slow, and the
    /// next daemon gets another chance. The one thing this never does is unlink the socket of a
    /// live holder — that would strand the user's shell with no way back to it.
    pub async fn adopt_holders(&self) -> DaemonResult<usize> {
        let discovered = holder::discover_sidecars(&self.home).await?;
        if discovered.is_empty() {
            return Ok(0);
        }
        let scrollback_bytes = self.config.load().await?.terminal.scrollback_bytes;
        let mut attempts = tokio::task::JoinSet::new();
        for entry in discovered {
            let sessions = self.clone();
            attempts.spawn(async move { sessions.adopt_one(entry, scrollback_bytes).await });
        }

        let mut ready = Vec::new();
        let collected = tokio::time::timeout(ADOPT_TIMEOUT, async {
            while let Some(joined) = attempts.join_next().await {
                match joined {
                    Ok(Adopted::Terminal(terminal)) => ready.push(*terminal),
                    Ok(Adopted::Nothing) => {}
                    Err(error) => tracing::warn!(%error, "a pty holder adoption panicked"),
                }
            }
        })
        .await;
        if collected.is_err() {
            // The records of the holders that did not answer are left exactly as they were.
            tracing::warn!("gave up on the pty holders that had not answered yet");
            attempts.abort_all();
        }

        // Deterministic order regardless of which holder answered first: a session's tabs are
        // numbered by position, and `ctrl-s <n>` must not depend on a race.
        ready.sort_by_key(|terminal| terminal.sidecar.terminal.0);
        let adopted = ready.len();
        for terminal in ready {
            if let Err(error) = self.insert_adopted(terminal) {
                tracing::warn!(%error, "failed to register a reattached terminal");
            }
        }
        if adopted > 0 {
            tracing::info!(
                adopted,
                "reattached terminals that outlived the previous daemon"
            );
            self.runtime.notify_snapshot();
        }
        Ok(adopted)
    }

    /// Waits for the holders killed by a stopping daemon to actually exit, then insists.
    ///
    /// `DaemonShutdown { stop_sessions: true }` promises the terminals are gone when it answers.
    /// A holder that has not gone by the deadline is killed outright and its record removed:
    /// leaving it would mean the next daemon reattaches the very sessions `ctrl-shift-q` destroyed.
    pub async fn wait_for_holders_to_stop(&self) {
        let deadline = tokio::time::Instant::now() + STOP_HOLDERS_GRACE;
        loop {
            let remaining = self.live_holder_records().await;
            if remaining.is_empty() {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                for (path, sidecar) in remaining {
                    tracing::warn!(
                        terminal = %sidecar.terminal,
                        pid = sidecar.holder_pid,
                        "pty holder ignored the stop request; ending it outright"
                    );
                    kill_holder(sidecar.holder_pid);
                    holder::remove_record(&path, Some(&sidecar.socket)).await;
                }
                return;
            }
            tokio::time::sleep(STOP_HOLDERS_POLL).await;
        }
    }

    /// Returns the records whose holder process is still running.
    async fn live_holder_records(&self) -> Vec<(std::path::PathBuf, holder::PtySidecar)> {
        match holder::discover_sidecars(&self.home).await {
            Ok(discovered) => discovered
                .into_iter()
                .filter_map(|entry| {
                    let sidecar = entry.sidecar?;
                    holder::holder_is_alive(&sidecar).then_some((entry.path, sidecar))
                })
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "failed to check whether the pty holders stopped");
                Vec::new()
            }
        }
    }

    async fn adopt_one(&self, entry: DiscoveredSidecar, scrollback_bytes: usize) -> Adopted {
        let Some(sidecar) = entry.sidecar else {
            // A record this daemon cannot read still names its terminal, and the socket still
            // carries it: enough to stop the holder rather than orphan the shell behind it.
            holder::stop_unreadable_record(&self.home, &entry.path).await;
            return Adopted::Nothing;
        };
        if !holder::holder_is_alive(&sidecar) {
            tracing::info!(
                terminal = %sidecar.terminal,
                pid = sidecar.holder_pid,
                "dropping the record of a pty holder that is no longer running"
            );
            holder::remove_record(&entry.path, Some(&sidecar.socket)).await;
            return Adopted::Nothing;
        }
        if !self.session_still_exists(&sidecar).await {
            tracing::warn!(
                terminal = %sidecar.terminal,
                session = %sidecar.session,
                "stopping a pty holder whose worktree is gone"
            );
            holder::stop_and_remove(&entry.path, &sidecar.socket).await;
            return Adopted::Nothing;
        }

        // A reattach never retypes the terminal's command: that shell already ran it, and the
        // replay this attach receives is the proof.
        let attached = host_bridge::attach_holder(HolderAttachment {
            terminal: sidecar.terminal,
            socket: sidecar.socket.clone(),
            scrollback_bytes,
            starting_sequence: 1,
            initial_command: None,
        })
        .await;
        let host = match attached {
            Ok(host) => host,
            Err(HolderAttachFailure::Incompatible(error)) => {
                // A holder from a build this one cannot speak to will never be reachable. Ending
                // it deliberately is the only alternative to a shell nothing can ever reach.
                tracing::warn!(
                    %error,
                    terminal = %sidecar.terminal,
                    "stopping a pty holder from an incompatible build"
                );
                holder::stop_and_remove(&entry.path, &sidecar.socket).await;
                return Adopted::Nothing;
            }
            Err(HolderAttachFailure::Unreachable(error)) => {
                // Keep everything. The holder is alive, so its socket is the user's only way back
                // to that shell, and the next daemon start gets another chance at it.
                tracing::warn!(
                    %error,
                    terminal = %sidecar.terminal,
                    pid = sidecar.holder_pid,
                    "a live pty holder did not answer; keeping its record for the next start"
                );
                return Adopted::Nothing;
            }
        };

        let shell_pid = host.child_pid().or(sidecar.shell_pid);
        Adopted::Terminal(Box::new(AdoptedTerminal {
            sidecar,
            host,
            shell_pid,
        }))
    }

    /// Whether the worktree a recorded session belongs to is still published.
    ///
    /// A holder can outlive the worktree it was opened in. Adopting it would put a session in the
    /// registry that `EnsureSession` then refuses with a `Conflict`, so the terminal would be
    /// visible and unusable; stopping it is the honest outcome.
    async fn session_still_exists(&self, sidecar: &holder::PtySidecar) -> bool {
        let SessionKind::Worktree(worktree) = &sidecar.session_kind else {
            return true;
        };
        match self.state.load().await {
            Ok(state) => state.worktrees.iter().any(|entry| &entry.id == worktree),
            Err(error) => {
                // An unreadable state file is not evidence the worktree is gone; keep the terminal.
                tracing::warn!(%error, "failed to check a reattached session's worktree");
                true
            }
        }
    }

    fn insert_adopted(&self, adopted: AdoptedTerminal) -> DaemonResult<()> {
        let AdoptedTerminal {
            sidecar,
            host,
            shell_pid,
        } = adopted;
        let host = Arc::new(host);
        let forwarder = host_bridge::prepare_host_events(
            Arc::clone(&self.runtime),
            sidecar.terminal,
            Arc::clone(&host),
        )?;
        {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session = registry
                .sessions
                .entry(sidecar.session.clone())
                .or_insert_with(|| sidecar.session());
            let mut terminal = sidecar.terminal();
            terminal.shell_pid = shell_pid;
            session.terminals.push(terminal);
            session.active_terminal.get_or_insert(sidecar.terminal);
            registry
                .terminal_sessions
                .insert(sidecar.terminal, sidecar.session.clone());
            registry.hosts.insert(sidecar.terminal, host);
            registry.observed_output_bytes.insert(sidecar.terminal, 0);
            registry.next_terminal = registry
                .next_terminal
                .max(sidecar.terminal.0.saturating_add(1));
        }
        // The shared allocator must not hand out an identifier an adopted terminal already owns.
        self.terminal_id_counter()
            .fetch_max(sidecar.terminal.0.saturating_add(1), Ordering::Relaxed);
        forwarder.start()?;
        tracing::info!(
            terminal = %sidecar.terminal,
            session = %sidecar.session,
            pid = sidecar.holder_pid,
            "adopted a pty holder"
        );
        Ok(())
    }
}

/// Ends a holder that ignored a stop request.
fn kill_holder(pid: u32) {
    let Ok(pid) = libc::pid_t::try_from(pid).map(|pid| pid.max(0)) else {
        return;
    };
    if pid == 0 {
        return;
    }
    // SAFETY: `kill` takes scalar arguments, and this pid came from a record this daemon's own
    // lineage wrote. Signal zero is never sent, so no process group is addressed by accident.
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(%error, pid, "failed to end a pty holder that ignored its stop request");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::adapters::{clock::SystemClock, files::RealFiles};

    fn sessions(home: &std::path::Path) -> Sessions {
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        Sessions::new(
            Arc::new(ConfigStore::new(home, Arc::clone(&files) as Arc<_>)),
            Arc::new(StateStore::new(home, files, Arc::new(SystemClock))),
        )
    }

    /// A record for a session with no worktree, so adoption turns only on the holder itself.
    fn sidecar(home: &fleet_core::paths::FleetHome, terminal: TerminalId) -> holder::PtySidecar {
        holder::PtySidecar {
            version: holder::SIDECAR_VERSION,
            terminal,
            session: SessionId::try_from("agent/session").expect("session id"),
            session_kind: SessionKind::Agent(fleet_core::config::Agent::Claude),
            session_cwd: home.root().display().to_string(),
            name: "cc".to_owned(),
            command: "claude".to_owned(),
            cwd: home.root().display().to_string(),
            // A process identifier `kill(2)` never reports as alive.
            holder_pid: 0,
            shell_pid: None,
            socket: fleet_core::paths::pty_socket_path(home, terminal, "0123456789abcdef"),
            created_at: "2026-09-12T00:00:00Z".to_owned(),
        }
    }

    #[tokio::test]
    async fn adoption_is_a_no_op_without_any_holder_records() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let sessions = sessions(temp.path());
        assert_eq!(
            sessions
                .adopt_holders()
                .await
                .unwrap_or_else(|error| panic!("{error}")),
            0
        );
        assert!(sessions.snapshot().is_empty());
    }

    #[tokio::test]
    async fn adoption_deletes_records_whose_holder_is_gone_and_adopts_nothing() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = fleet_core::paths::FleetHome::new(temp.path());
        std::fs::create_dir_all(home.pty_dir()).unwrap_or_else(|error| panic!("{error}"));
        let record = sidecar(&home, TerminalId(5));
        std::fs::create_dir_all(
            record
                .socket
                .parent()
                .unwrap_or_else(|| panic!("socket directory")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&record.socket, b"").unwrap_or_else(|error| panic!("{error}"));
        holder::write_sidecar(&home, &record)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        // An unreadable record is removed on the same pass.
        std::fs::write(home.pty_dir().join("6.json"), b"{ broken")
            .unwrap_or_else(|error| panic!("{error}"));

        let sessions = sessions(temp.path());
        assert_eq!(
            sessions
                .adopt_holders()
                .await
                .unwrap_or_else(|error| panic!("{error}")),
            0
        );
        assert!(sessions.snapshot().is_empty());
        assert!(!home.pty_sidecar_path(TerminalId(5)).exists());
        assert!(!home.pty_dir().join("6.json").exists());
        assert!(!record.socket.exists());
    }

    /// A live holder's socket is the user's only way back to their shell; losing it strands them.
    #[tokio::test]
    async fn a_live_holder_that_does_not_answer_keeps_its_record_and_its_socket() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = fleet_core::paths::FleetHome::new(temp.path());
        std::fs::create_dir_all(home.pty_dir()).unwrap_or_else(|error| panic!("{error}"));
        let mut record = sidecar(&home, TerminalId(9));
        // This process is certainly alive, and it certainly does not speak the holder protocol.
        record.holder_pid = std::process::id();
        std::fs::create_dir_all(
            record
                .socket
                .parent()
                .unwrap_or_else(|| panic!("socket directory")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&record.socket, b"").unwrap_or_else(|error| panic!("{error}"));
        holder::write_sidecar(&home, &record)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let sessions = sessions(temp.path());
        assert_eq!(
            sessions
                .adopt_holders()
                .await
                .unwrap_or_else(|error| panic!("{error}")),
            0
        );
        assert!(
            home.pty_sidecar_path(TerminalId(9)).exists(),
            "a live holder's record must survive for the next daemon to retry"
        );
        assert!(
            record.socket.exists(),
            "a live holder's socket must never be unlinked out from under it"
        );
    }

    /// A terminal whose worktree is gone would be adopted into a session `EnsureSession` refuses.
    #[tokio::test]
    async fn a_holder_whose_worktree_is_gone_is_stopped_rather_than_adopted() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = fleet_core::paths::FleetHome::new(temp.path());
        std::fs::create_dir_all(home.pty_dir()).unwrap_or_else(|error| panic!("{error}"));
        let mut record = sidecar(&home, TerminalId(4));
        record.holder_pid = std::process::id();
        record.session = SessionId::try_from("api/feature").expect("session id");
        record.session_kind =
            SessionKind::Worktree("acme/api#feature".parse().expect("worktree id"));
        std::fs::create_dir_all(
            record
                .socket
                .parent()
                .unwrap_or_else(|| panic!("socket directory")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&record.socket, b"").unwrap_or_else(|error| panic!("{error}"));
        holder::write_sidecar(&home, &record)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let sessions = sessions(temp.path());
        assert_eq!(
            sessions
                .adopt_holders()
                .await
                .unwrap_or_else(|error| panic!("{error}")),
            0
        );
        assert!(sessions.snapshot().is_empty());
        assert!(
            !home.pty_sidecar_path(TerminalId(4)).exists(),
            "a holder with nowhere to belong must be stopped, not left recorded"
        );
    }
}
