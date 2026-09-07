//! Bounded, sequence-aware watch mirrors and the display text panes render from.
//!
//! A mirror owns the retained lines of one child and the shared line/tone arrays built from
//! them, including the one presentation rule that belongs to the output itself: stderr reads
//! back dimmer than stdout. Nothing here starts, stops, or otherwise controls a child.

use fleet_core::{
    ids::SessionId,
    watches::{Watch, WatchChunk, WatchId, WatchStatus, WatchStream},
};
use fleet_proto::watch::WatchTail;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::Instant,
};

mod history;
mod removed;
#[cfg(test)]
mod tests;

pub use history::{WatchDisplay, WatchMirror};
use removed::RemovedWatches;

/// Local selection/visibility survives session navigation and catch-up requests.
#[derive(Debug, Default)]
pub struct WatchPaneState {
    /// Selected watch, if the session has watches.
    pub selected: Option<WatchId>,
    /// Whether the split is shown.
    pub visible: bool,
}

/// All watch mirrors and catch-up work that the foreground bridge must perform.
#[derive(Debug, Default)]
pub struct Watches {
    /// Metadata and bounded output, keyed by daemon-local ID.
    pub entries: BTreeMap<WatchId, WatchMirror>,
    /// Per-session local pane state.
    pub panes: HashMap<SessionId, WatchPaneState>,
    removed: RemovedWatches,
    started_events: BTreeSet<WatchId>,
    pending: BTreeMap<WatchId, Option<u64>>,
    inflight: BTreeSet<WatchId>,
    synced: Option<(SessionId, u64)>,
}

impl Watches {
    /// Records a new event. Only a previously unseen WatchStarted overrides local hiding.
    pub fn started(&mut self, watch: Watch, now: Instant) {
        if self.removed.contains(&watch.id) {
            return;
        }
        let is_new = self.started_events.insert(watch.id);
        let (id, session) = (watch.id, watch.session.clone());
        self.upsert(watch, now);
        if is_new {
            let pane = self.panes.entry(session).or_default();
            pane.selected = Some(id);
            pane.visible = true;
        }
    }

    fn upsert(&mut self, watch: Watch, now: Instant) {
        if self.removed.contains(&watch.id) {
            return;
        }
        let session = watch.session.clone();
        let id = watch.id;
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.metadata(watch, now);
        } else {
            self.entries.insert(id, WatchMirror::new(watch, now));
        }
        self.panes.entry(session).or_insert(WatchPaneState {
            selected: Some(id),
            visible: true,
        });
    }

    /// Applies ordered chunks; skips duplicates and asks for a tail on a gap.
    pub fn output(&mut self, id: WatchId, chunks: Vec<WatchChunk>) {
        if self.removed.contains(&id) {
            return;
        }
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.chunks(chunks);
            if entry.observed_next > entry.next_seq {
                self.pending.entry(id).or_insert(Some(entry.next_seq));
            }
        } else {
            self.pending.insert(id, None);
        }
    }

    /// Freezes the observed elapsed duration and keeps the output available.
    pub fn exited(&mut self, watch: Watch, now: Instant) {
        let id = watch.id;
        self.upsert(watch, now);
        // Also recovers missing final output when the last output event was lost.
        if let Some(entry) = self.entries.get(&id) {
            self.pending.entry(id).or_insert(Some(entry.next_seq));
        }
    }

    /// Removes output and selects the next tab, wrapping at the end.
    pub fn dismissed(&mut self, id: WatchId) {
        self.removed.insert(id);
        self.started_events.remove(&id);
        self.pending.remove(&id);
        self.inflight.remove(&id);
        if let Some(entry) = self.entries.remove(&id) {
            let next = self.ids(&entry.watch.session);
            if let Some(pane) = self.panes.get_mut(&entry.watch.session) {
                if pane.selected == Some(id) {
                    pane.selected = next
                        .iter()
                        .copied()
                        .find(|n| *n > id)
                        .or_else(|| next.first().copied());
                }
                if next.is_empty() {
                    pane.visible = false;
                }
            }
        }
    }

    /// Registration-order watch IDs for one session.
    #[must_use]
    pub fn ids(&self, session: &SessionId) -> Vec<WatchId> {
        self.entries
            .iter()
            .filter(|(_, e)| &e.watch.session == session)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Toggles local visibility, returning false if there are no watches.
    pub fn toggle(&mut self, session: &SessionId) -> bool {
        if !self
            .entries
            .values()
            .any(|entry| &entry.watch.session == session)
        {
            return false;
        }
        if let Some(pane) = self.panes.get_mut(session) {
            pane.visible = !pane.visible;
        }
        true
    }

    /// Shows and cycles through this session's watches in registration order.
    /// Returns false only when there are no watches.
    pub fn cycle(&mut self, session: &SessionId, forward: bool) -> bool {
        let ids = self.ids(session);
        if ids.is_empty() {
            return false;
        }
        let pane = self.panes.entry(session.clone()).or_default();
        let current = pane
            .selected
            .and_then(|id| ids.iter().position(|candidate| *candidate == id));
        let index = match current {
            Some(index) if forward => (index + 1) % ids.len(),
            Some(index) => (index + ids.len() - 1) % ids.len(),
            None if forward => 0,
            None => ids.len() - 1,
        };
        pane.selected = Some(ids[index]);
        pane.visible = true;
        true
    }

    /// Hides the pane without changing or dismissing any watch.
    pub fn hide(&mut self, session: &SessionId) {
        if let Some(pane) = self.panes.get_mut(session) {
            pane.visible = false;
        }
    }

    /// Selects a tab only when it belongs to this session.
    pub fn select(&mut self, session: &SessionId, id: WatchId) {
        if self
            .entries
            .get(&id)
            .is_some_and(|e| &e.watch.session == session)
            && let Some(pane) = self.panes.get_mut(session)
        {
            pane.selected = Some(id);
        }
    }

    /// Marks a session entry or reconnect as needing an authoritative list.
    pub fn enter(&mut self, session: Option<SessionId>, generation: u64) -> Option<SessionId> {
        let next = session.clone().map(|session| (session, generation));
        if self.synced == next {
            return None;
        }
        self.synced = next;
        session
    }

    /// Re-list on event receiver lag, including watches whose start/dismiss event was lost.
    pub fn invalidate(&mut self) {
        self.synced = None;
    }

    /// A new connection invalidates outstanding request bookkeeping, not local preferences.
    pub fn reconnect(&mut self) {
        self.inflight.clear();
        self.pending.clear();
        self.invalidate();
    }

    /// Reconciles only IDs known at request time, preserving concurrent starts and dismissals.
    pub fn listed(
        &mut self,
        session: &SessionId,
        known: Vec<WatchId>,
        watches: Vec<Watch>,
        now: Instant,
    ) {
        let had_selection = self
            .panes
            .get(session)
            .is_some_and(|p| p.selected.is_some());
        let ids: BTreeSet<_> = watches.iter().map(|w| w.id).collect();
        for id in known {
            if !ids.contains(&id) {
                self.dismissed(id);
            }
        }
        for watch in watches {
            if &watch.session != session || self.removed.contains(&watch.id) {
                continue;
            }
            self.pending.insert(watch.id, None);
            self.upsert(watch, now);
        }
        // Initial discovery selects the newest when the session has no existing selection.
        let last = self.ids(session).last().copied();
        if let Some(pane) = self.panes.get_mut(session)
            && !had_selection
        {
            pane.selected = last;
            pane.visible = pane.selected.is_some();
        }
    }

    /// Drains catch-up requests while allowing at most one tail per watch in flight.
    pub fn take_tails(&mut self) -> Vec<(WatchId, Option<u64>)> {
        let ids: Vec<_> = self
            .pending
            .keys()
            .filter(|id| !self.inflight.contains(id))
            .copied()
            .collect();
        ids.into_iter()
            .map(|id| {
                self.inflight.insert(id);
                (id, self.pending.remove(&id).flatten())
            })
            .collect()
    }

    /// Merges an atomic tail without replaying lines or reviving dismissed watches.
    pub fn tailed(&mut self, tail: WatchTail, now: Instant) {
        let id = tail.watch.id;
        self.inflight.remove(&id);
        self.upsert(tail.watch, now);
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if tail.first_retained_seq > entry.next_seq {
            entry.trimmed = true;
            entry.clear_partial();
            entry.next_seq = tail.first_retained_seq;
        }
        entry.observed_next = entry.observed_next.max(tail.next_seq);
        entry.chunks(tail.chunks);
        if entry.next_seq < entry.observed_next {
            self.pending.insert(id, Some(entry.next_seq));
        } else if self.pending.get(&id).is_some_and(Option::is_some) {
            self.pending.remove(&id);
        }
    }

    /// Releases a failed tail; subsequent events or session entry can retry it.
    pub fn tail_failed(&mut self, id: WatchId) {
        self.inflight.remove(&id);
        self.pending.remove(&id);
    }

    /// Reclaims empty pane records after their session leaves the authoritative snapshot.
    pub(crate) fn reconcile_sessions(&mut self, live: &HashSet<&SessionId>) {
        let retained: HashSet<_> = self
            .entries
            .values()
            .map(|entry| &entry.watch.session)
            .collect();
        self.panes
            .retain(|session, _| live.contains(session) || retained.contains(session));
    }

    /// Whether a visible selected watch needs its elapsed label repainted.
    #[must_use]
    pub fn running_visible(&self, session: &SessionId) -> bool {
        self.panes
            .get(session)
            .filter(|p| p.visible)
            .and_then(|p| p.selected)
            .and_then(|id| self.entries.get(&id))
            .is_some_and(|e| e.watch.status == WatchStatus::Running)
    }
}
