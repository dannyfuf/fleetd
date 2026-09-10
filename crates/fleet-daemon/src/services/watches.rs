//! Runtime-only cooperative and discovered watches. Mutations share one lock.

use crate::{DaemonError, DaemonResult, server::BroadcastBus};
use fleet_core::{
    ids::{SessionId, TerminalId},
    watches::{Watch, WatchBuffer, WatchId, WatchSource, WatchStatus, WatchStream},
};
use fleet_proto::{event::Event, watch::WatchTail};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) const RETENTION: Duration = Duration::from_secs(30 * 60);

/// One watch plus the output retained for it and the connection that may write to it.
struct Entry {
    watch: Watch,
    output: WatchBuffer,
    published_seq: u64,
    owner: Option<u64>,
    finished_at: Option<Instant>,
}

/// Every watch in the daemon, indexed by the session and terminal it belongs to.
#[derive(Default)]
struct Registry {
    entries: BTreeMap<WatchId, Entry>,
    sessions: BTreeMap<SessionId, BTreeSet<WatchId>>,
    terminals: BTreeMap<TerminalId, BTreeSet<WatchId>>,
    next_id: u64,
    next_owner: u64,
    events: Option<BroadcastBus>,
}

/// Shared watch registry; never holds handles capable of killing a child.
#[derive(Clone, Default)]
pub struct Watches {
    registry: Arc<Mutex<Registry>>,
}

/// Marks outstanding watches interrupted even if the connection actor is cancelled.
pub(crate) struct WatchOwner {
    pub(crate) id: u64,
    watches: Watches,
}

impl Drop for WatchOwner {
    fn drop(&mut self) {
        self.watches.disconnect(self.id);
    }
}

impl Watches {
    /// Publishes watch lifecycle events onto the daemon-wide bus.
    pub(crate) fn with_events(&self, events: BroadcastBus) {
        self.lock().events = Some(events);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Issues the token one connection uses to write to the watches it started.
    pub(crate) fn owner(&self) -> WatchOwner {
        let mut registry = self.lock();
        registry.next_owner += 1;
        WatchOwner {
            id: registry.next_owner,
            watches: self.clone(),
        }
    }

    /// Registers a watch a client drives itself; its output arrives over the socket.
    pub(crate) fn start(&self, owner: u64, mut watch: Watch) -> WatchId {
        watch.source = WatchSource::Cooperative;
        watch.log_file = None;
        self.lock().insert(Some(owner), watch, None)
    }

    /// Registers a watch discovered from a running process, ignoring a PID already watched.
    pub(crate) fn start_discovered(
        &self,
        watch: Watch,
        initial_output: Option<String>,
    ) -> Option<WatchId> {
        let pid = watch.pid?;
        let mut registry = self.lock();
        if registry
            .entries
            .values()
            .any(|entry| entry.watch.pid == Some(pid))
        {
            return None;
        }
        Some(registry.insert(None, watch, initial_output))
    }

    /// Ids of every watch the registry still holds, including finished ones awaiting expiry.
    pub(crate) fn ids(&self) -> BTreeSet<WatchId> {
        self.lock().entries.keys().copied().collect()
    }

    /// Returns the watch observing `pid`, if discovery already registered one.
    pub(crate) fn watch_for_pid(&self, pid: u32) -> Option<Watch> {
        self.lock()
            .entries
            .values()
            .find(|entry| entry.watch.pid == Some(pid))
            .map(|entry| entry.watch.clone())
    }

    /// Upserts a discovered watch's label and log file, republishing only real changes.
    pub(crate) fn update_discovered(
        &self,
        id: WatchId,
        label: String,
        log_file: Option<std::path::PathBuf>,
    ) -> DaemonResult<()> {
        let mut registry = self.lock();
        let changed = {
            let entry = registry.entry(id)?;
            if entry.watch.source != WatchSource::Discovered {
                return Err(DaemonError::Conflict("watch is not discovered".into()));
            }
            if entry.watch.label == label && entry.watch.log_file == log_file {
                None
            } else {
                entry.watch.label = label;
                entry.watch.log_file = log_file;
                Some(entry.watch.clone())
            }
        };
        if let Some(watch) = changed {
            // Duplicate starts are metadata upserts in clients and do not reopen a hidden pane.
            registry.publish(Event::WatchStarted(watch));
        }
        Ok(())
    }

    /// Records the exit of a discovered watch.
    pub(crate) fn finish_discovered(&self, id: WatchId, code: Option<i32>) -> DaemonResult<()> {
        let mut registry = self.lock();
        if registry.entry(id)?.watch.source != WatchSource::Discovered {
            return Err(DaemonError::Conflict("watch is not discovered".into()));
        }
        registry.finish(id, code, None, Instant::now())
    }

    /// Retires a discovered watch whose PID now names a different process.
    pub(crate) fn replace_discovered(&self, id: WatchId) -> DaemonResult<()> {
        let mut registry = self.lock();
        let entry = registry.entry(id)?;
        if entry.watch.source != WatchSource::Discovered {
            return Err(DaemonError::Conflict("watch is not discovered".into()));
        }
        entry.published_seq = entry.output.next_seq();
        registry.finish(id, None, None, Instant::now())?;
        registry.remove(id);
        Ok(())
    }

    /// Appends discovered output, dropping anything that arrives after the exit.
    pub(crate) fn append_discovered(&self, id: WatchId, text: String) -> DaemonResult<()> {
        let mut registry = self.lock();
        let entry = registry.entry(id)?;
        if entry.watch.source != WatchSource::Discovered {
            return Err(DaemonError::Conflict("watch is not discovered".into()));
        }
        if entry.watch.status != WatchStatus::Running {
            return Ok(());
        }
        entry.output.append(WatchStream::Stdout, text);
        Ok(())
    }
    /// Rejects writes from any connection but the one that started a cooperative watch.
    pub(crate) fn require_owner(&self, id: WatchId, owner: u64) -> DaemonResult<()> {
        let registry = self.lock();
        let entry = registry
            .entries
            .get(&id)
            .ok_or_else(|| DaemonError::NotFound(format!("watch {id}")))?;
        if entry.watch.source == WatchSource::Discovered {
            return Err(DaemonError::Conflict(
                "discovered watches are read-only".into(),
            ));
        }
        if entry.owner != Some(owner) {
            return Err(DaemonError::Conflict(
                "watch output and completion require the starting connection".into(),
            ));
        }
        Ok(())
    }

    /// Appends output to a watch the calling connection started.
    pub(crate) fn append_owned(
        &self,
        id: WatchId,
        owner: u64,
        stream: WatchStream,
        text: String,
    ) -> DaemonResult<()> {
        self.require_owner(id, owner)?;
        self.append(id, stream, text)
    }

    /// Records the exit of a watch the calling connection started.
    pub(crate) fn finish_owned(
        &self,
        id: WatchId,
        owner: u64,
        code: Option<i32>,
        signal: Option<i32>,
    ) -> DaemonResult<()> {
        self.require_owner(id, owner)?;
        self.finish(id, code, signal)
    }

    /// Appends while Running; completed watches reject further output.
    pub fn append(&self, id: WatchId, stream: WatchStream, text: String) -> DaemonResult<()> {
        let mut registry = self.lock();
        let entry = registry.entry(id)?;
        if entry.watch.status != WatchStatus::Running {
            return Err(DaemonError::Conflict("watch has finished".into()));
        }
        entry.output.append(stream, text);
        Ok(())
    }
    /// Records completion once. Output is flushed before the exit event.
    pub fn finish(&self, id: WatchId, code: Option<i32>, signal: Option<i32>) -> DaemonResult<()> {
        self.lock().finish(id, code, signal, Instant::now())
    }
    /// Returns watches in registration order for one session.
    #[must_use]
    pub fn list(&self, session: &SessionId) -> Vec<Watch> {
        let registry = self.lock();
        registry
            .sessions
            .get(session)
            .into_iter()
            .flatten()
            .filter_map(|id| registry.entries.get(id).map(|entry| entry.watch.clone()))
            .collect()
    }
    /// Takes an atomic metadata and retained-output snapshot.
    pub fn tail(&self, id: WatchId, from_seq: Option<u64>) -> DaemonResult<WatchTail> {
        let mut registry = self.lock();
        let entry = registry.entry(id)?;
        Ok(WatchTail {
            watch: entry.watch.clone(),
            chunks: entry.output.tail(from_seq),
            first_retained_seq: entry.output.first_retained_seq(),
            next_seq: entry.output.next_seq(),
        })
    }
    /// Removes only completed watches; Running returns Conflict.
    pub fn dismiss(&self, id: WatchId) -> DaemonResult<()> {
        let mut registry = self.lock();
        if registry.entry(id)?.watch.status == WatchStatus::Running {
            return Err(DaemonError::Conflict(
                "cannot dismiss a running watch".into(),
            ));
        }
        registry.remove(id);
        Ok(())
    }

    /// Forgets every watch of a closed terminal, running or not.
    pub(crate) fn remove_terminal(&self, terminal: TerminalId) {
        let mut registry = self.lock();
        let ids = registry
            .terminals
            .get(&terminal)
            .cloned()
            .unwrap_or_default();
        for id in ids {
            registry.remove(id);
        }
    }

    /// Marks a departed connection's still-running watches interrupted.
    fn disconnect(&self, owner: u64) {
        let mut registry = self.lock();
        let ids = registry
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.owner == Some(owner) && entry.watch.status == WatchStatus::Running
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in ids {
            let _ignored = registry.finish(id, None, None, Instant::now());
        }
    }
    /// Flushes coalesced output and expires completed watches.
    pub fn tick(&self, now: Instant) {
        let mut registry = self.lock();
        let ids = registry.entries.keys().copied().collect::<Vec<_>>();
        for id in ids {
            if registry
                .entries
                .get(&id)
                .and_then(|entry| entry.finished_at)
                .is_some_and(|at| now.saturating_duration_since(at) >= RETENTION)
            {
                registry.remove(id);
            } else {
                registry.flush(id);
            }
        }
    }

    /// Ticks every 50 ms until the daemon shuts down.
    pub(crate) async fn run(self, shutdown: tokio_util::sync::CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = interval.tick() => self.tick(Instant::now()),
            }
        }
    }
}

impl Registry {
    fn insert(
        &mut self,
        owner: Option<u64>,
        mut watch: Watch,
        initial_output: Option<String>,
    ) -> WatchId {
        self.next_id += 1;
        watch.id = WatchId(self.next_id);
        let id = watch.id;
        self.sessions
            .entry(watch.session.clone())
            .or_default()
            .insert(id);
        self.terminals.entry(watch.terminal).or_default().insert(id);
        self.publish(Event::WatchStarted(watch.clone()));
        let mut output = WatchBuffer::default();
        if let Some(text) = initial_output {
            output.append(WatchStream::Stdout, text);
        }
        self.entries.insert(
            id,
            Entry {
                watch,
                output,
                published_seq: 0,
                owner,
                finished_at: None,
            },
        );
        id
    }
    fn entry(&mut self, id: WatchId) -> DaemonResult<&mut Entry> {
        self.entries
            .get_mut(&id)
            .ok_or_else(|| DaemonError::NotFound(format!("watch {id}")))
    }
    fn publish(&self, event: Event) {
        if let Some(events) = &self.events {
            events.publish(event);
        }
    }
    fn flush(&mut self, id: WatchId) {
        if let Some(entry) = self.entries.get_mut(&id) {
            let chunks = entry.output.tail(Some(entry.published_seq));
            entry.published_seq = entry.output.next_seq();
            if !chunks.is_empty() {
                self.publish(Event::WatchOutput { watch: id, chunks });
            }
        }
    }
    fn finish(
        &mut self,
        id: WatchId,
        code: Option<i32>,
        signal: Option<i32>,
        now: Instant,
    ) -> DaemonResult<()> {
        let entry = self.entry(id)?;
        if entry.watch.status != WatchStatus::Running {
            return Ok(());
        }
        entry.watch.status = WatchStatus::Exited { code, signal };
        entry.finished_at = Some(now);
        let watch = entry.watch.clone();
        self.flush(id);
        self.publish(Event::WatchExited(watch));
        Ok(())
    }
    fn remove(&mut self, id: WatchId) {
        if let Some(entry) = self.entries.remove(&id) {
            if let Some(ids) = self.sessions.get_mut(&entry.watch.session) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.sessions.remove(&entry.watch.session);
                }
            }
            if let Some(ids) = self.terminals.get_mut(&entry.watch.terminal) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.terminals.remove(&entry.watch.terminal);
                }
            }
            self.publish(Event::WatchDismissed(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn watch() -> Watch {
        Watch {
            id: WatchId(0),
            session: "repo/main".parse().unwrap(),
            terminal: TerminalId(1),
            label: "test".into(),
            command: vec!["sh".into()],
            cwd: None,
            pid: None,
            started_at: "2026-09-05T00:00:00Z".into(),
            status: WatchStatus::Running,
            source: WatchSource::Cooperative,
            log_file: None,
        }
    }
    #[test]
    fn lifecycle_output_batching_disconnect_close_and_ttl() {
        let watches = Watches::default();
        let bus = BroadcastBus::default();
        let mut events = bus.subscribe();
        watches.with_events(bus);
        let owner = watches.owner();
        let id = watches.start(owner.id, watch());
        assert!(matches!(events.try_recv().unwrap(), Event::WatchStarted(_)));
        assert!(watches.dismiss(id).is_err());
        watches
            .append(id, WatchStream::Stdout, "out".into())
            .unwrap();
        watches
            .append(id, WatchStream::Stderr, "err".into())
            .unwrap();
        assert!(events.try_recv().is_err());
        watches.tick(Instant::now());
        assert!(
            matches!(events.try_recv().unwrap(), Event::WatchOutput { chunks, .. } if chunks.len() == 2)
        );
        assert_eq!(watches.tail(id, Some(1)).unwrap().chunks.len(), 1);
        watches.finish(id, Some(3), None).unwrap();
        assert!(
            watches
                .append(id, WatchStream::Stdout, "late".into())
                .is_err()
        );
        assert_eq!(
            watches.list(&watch().session)[0].status,
            WatchStatus::Exited {
                code: Some(3),
                signal: None
            }
        );
        watches.dismiss(id).unwrap();
        assert!(watches.tail(id, None).is_err());
        let id = watches.start(owner.id, watch());
        drop(owner);
        assert_eq!(
            watches.tail(id, None).unwrap().watch.status,
            WatchStatus::Exited {
                code: None,
                signal: None
            }
        );
        watches.tick(Instant::now() + RETENTION);
        assert!(watches.list(&watch().session).is_empty());
        let id = watches.start(0, watch());
        watches.remove_terminal(TerminalId(1));
        assert!(watches.tail(id, None).is_err());
        assert!(watches.lock().sessions.is_empty());
        assert!(watches.lock().terminals.is_empty());
    }
}
