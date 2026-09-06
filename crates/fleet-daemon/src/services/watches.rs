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

const RETENTION: Duration = Duration::from_secs(30 * 60);

struct Entry {
    watch: Watch,
    output: WatchBuffer,
    published_seq: u64,
    owner: Option<u64>,
    finished_at: Option<Instant>,
}
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
    pub(crate) fn with_events(&self, events: BroadcastBus) {
        self.lock().events = Some(events);
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn owner(&self) -> WatchOwner {
        let mut r = self.lock();
        r.next_owner += 1;
        WatchOwner {
            id: r.next_owner,
            watches: self.clone(),
        }
    }
    pub(crate) fn start(&self, owner: u64, mut watch: Watch) -> WatchId {
        watch.source = WatchSource::Cooperative;
        watch.log_file = None;
        self.insert(Some(owner), watch, None)
    }
    pub(crate) fn start_discovered(
        &self,
        watch: Watch,
        initial_output: Option<String>,
    ) -> Option<WatchId> {
        let pid = watch.pid?;
        let mut r = self.lock();
        if r.entries.values().any(|entry| entry.watch.pid == Some(pid)) {
            return None;
        }
        Some(r.insert(None, watch, initial_output))
    }
    fn insert(&self, owner: Option<u64>, watch: Watch, initial_output: Option<String>) -> WatchId {
        let mut r = self.lock();
        r.insert(owner, watch, initial_output)
    }
    pub(crate) fn watch_for_pid(&self, pid: u32) -> Option<Watch> {
        self.lock()
            .entries
            .values()
            .find(|entry| entry.watch.pid == Some(pid))
            .map(|entry| entry.watch.clone())
    }
    pub(crate) fn update_discovered(
        &self,
        id: WatchId,
        label: String,
        log_file: Option<std::path::PathBuf>,
    ) -> DaemonResult<()> {
        let mut r = self.lock();
        let changed = {
            let entry = r.entry(id)?;
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
            r.publish(Event::WatchStarted(watch));
        }
        Ok(())
    }
    pub(crate) fn finish_discovered(&self, id: WatchId, code: Option<i32>) -> DaemonResult<()> {
        let mut r = self.lock();
        if r.entry(id)?.watch.source != WatchSource::Discovered {
            return Err(DaemonError::Conflict("watch is not discovered".into()));
        }
        r.finish(id, code, None, Instant::now())
    }
    pub(crate) fn append_discovered(&self, id: WatchId, text: String) -> DaemonResult<()> {
        let mut r = self.lock();
        let entry = r.entry(id)?;
        if entry.watch.source != WatchSource::Discovered {
            return Err(DaemonError::Conflict("watch is not discovered".into()));
        }
        if entry.watch.status != WatchStatus::Running {
            return Ok(());
        }
        entry.output.append(WatchStream::Stdout, text);
        Ok(())
    }
    pub(crate) fn require_owner(&self, id: WatchId, owner: u64) -> DaemonResult<()> {
        let r = self.lock();
        let entry = r
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
        let mut r = self.lock();
        let entry = r.entry(id)?;
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
        let r = self.lock();
        r.sessions
            .get(session)
            .into_iter()
            .flatten()
            .filter_map(|id| r.entries.get(id).map(|e| e.watch.clone()))
            .collect()
    }
    /// Takes an atomic metadata and retained-output snapshot.
    pub fn tail(&self, id: WatchId, from_seq: Option<u64>) -> DaemonResult<WatchTail> {
        let mut r = self.lock();
        let entry = r.entry(id)?;
        Ok(WatchTail {
            watch: entry.watch.clone(),
            chunks: entry.output.tail(from_seq),
            first_retained_seq: entry.output.first_retained_seq(),
            next_seq: entry.output.next_seq(),
        })
    }
    /// Removes only completed watches; Running returns Conflict.
    pub fn dismiss(&self, id: WatchId) -> DaemonResult<()> {
        let mut r = self.lock();
        if r.entry(id)?.watch.status == WatchStatus::Running {
            return Err(DaemonError::Conflict(
                "cannot dismiss a running watch".into(),
            ));
        }
        r.remove(id);
        Ok(())
    }
    pub(crate) fn remove_terminal(&self, terminal: TerminalId) {
        let mut r = self.lock();
        let ids = r.terminals.get(&terminal).cloned().unwrap_or_default();
        for id in ids {
            r.remove(id);
        }
    }
    fn disconnect(&self, owner: u64) {
        let mut r = self.lock();
        let ids: Vec<_> = r
            .entries
            .iter()
            .filter(|(_, e)| e.owner == Some(owner) && e.watch.status == WatchStatus::Running)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let _ = r.finish(id, None, Some(9), Instant::now());
        }
    }
    /// Flushes coalesced output and expires completed watches.
    pub fn tick(&self, now: Instant) {
        let mut r = self.lock();
        let ids: Vec<_> = r.entries.keys().copied().collect();
        for id in ids {
            if r.entries
                .get(&id)
                .and_then(|e| e.finished_at)
                .is_some_and(|at| now.saturating_duration_since(at) >= RETENTION)
            {
                r.remove(id);
            } else {
                r.flush(id);
            }
        }
    }
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
                signal: Some(9)
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
