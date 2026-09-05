//! Bounded, sequence-aware watch mirrors. No UI or process-control operations.

use fleet_core::{
    ids::SessionId,
    watches::{Watch, WatchChunk, WatchId, WatchStatus, WatchStream},
};
use fleet_proto::watch::WatchTail;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    time::{Duration, Instant},
};

const BYTE_CAP: usize = 1024 * 1024;
const LINE_CAP: usize = 20_000;

/// A completed or still-being-written output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchLine {
    /// Original child output channel.
    pub stream: WatchStream,
    /// Line text without its newline.
    pub text: String,
}

/// One child and its retained display history.
#[derive(Debug)]
pub struct WatchMirror {
    /// Latest known metadata. Completion never regresses to Running.
    pub watch: Watch,
    /// Completed newline-delimited lines.
    pub lines: VecDeque<WatchLine>,
    /// Inclusive next expected chunk cursor.
    pub next_seq: u64,
    /// Earlier output is unavailable locally or on the daemon.
    pub trimmed: bool,
    partial: [String; 2],
    line_bytes: usize,
    observed_next: u64,
    observed_at: Instant,
    initial_age: Duration,
    final_age: Option<Duration>,
}
impl WatchMirror {
    fn new(watch: Watch, now: Instant) -> Self {
        let initial_age = chrono::DateTime::parse_from_rfc3339(&watch.started_at)
            .ok()
            .and_then(|start| {
                (chrono::Utc::now() - start.with_timezone(&chrono::Utc))
                    .to_std()
                    .ok()
            })
            .unwrap_or_default();
        let final_age = (watch.status != WatchStatus::Running).then_some(initial_age);
        Self {
            watch,
            lines: VecDeque::new(),
            next_seq: 0,
            trimmed: false,
            partial: Default::default(),
            line_bytes: 0,
            observed_next: 0,
            observed_at: now,
            initial_age,
            final_age,
        }
    }
    /// Elapsed duration, frozen at the first observed completion.
    #[must_use]
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.final_age
            .unwrap_or_else(|| self.initial_age + now.saturating_duration_since(self.observed_at))
    }
    fn metadata(&mut self, watch: Watch, now: Instant) {
        if self.watch.status != WatchStatus::Running && watch.status == WatchStatus::Running {
            return;
        }
        if watch.status != WatchStatus::Running && self.final_age.is_none() {
            self.final_age = Some(self.elapsed(now));
        }
        self.watch = watch;
    }
    /// Completed lines followed by the live partial line of each stream.
    pub fn display_lines(&self) -> impl Iterator<Item = WatchLine> + '_ {
        self.lines.iter().cloned().chain(
            self.partial
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.is_empty())
                .map(|(i, text)| WatchLine {
                    stream: if i == 0 {
                        WatchStream::Stdout
                    } else {
                        WatchStream::Stderr
                    },
                    text: text.clone(),
                }),
        )
    }
    fn append(&mut self, chunk: WatchChunk) {
        let index = usize::from(chunk.stream == WatchStream::Stderr);
        for part in chunk.text.split_inclusive('\n') {
            self.partial[index].push_str(part.strip_suffix('\n').unwrap_or(part));
            if part.ends_with('\n') {
                let mut text = std::mem::take(&mut self.partial[index]);
                if text.ends_with('\r') {
                    text.pop();
                }
                self.line_bytes += text.len();
                self.lines.push_back(WatchLine {
                    stream: chunk.stream,
                    text,
                });
            }
            self.bound();
        }
        self.next_seq += 1;
    }
    fn bound(&mut self) {
        while self.lines.len() + self.partial.iter().filter(|s| !s.is_empty()).count() > LINE_CAP
            || self.line_bytes + self.partial.iter().map(String::len).sum::<usize>() > BYTE_CAP
        {
            self.trimmed = true;
            if let Some(line) = self.lines.pop_front() {
                self.line_bytes -= line.text.len();
            } else {
                let total: usize = self.partial.iter().map(String::len).sum();
                let i = usize::from(self.partial[1].len() > self.partial[0].len());
                let text = &mut self.partial[i];
                let mut cut = (total - BYTE_CAP).min(text.len());
                while !text.is_char_boundary(cut) {
                    cut += 1;
                }
                text.drain(..cut);
                text.shrink_to_fit();
            }
        }
    }
    fn chunks(&mut self, chunks: Vec<WatchChunk>) {
        for chunk in chunks {
            self.observed_next = self.observed_next.max(chunk.seq.saturating_add(1));
            if chunk.seq == self.next_seq {
                self.append(chunk);
            }
        }
    }
}

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
    removed: BTreeSet<WatchId>,
    started_events: BTreeSet<WatchId>,
    pending: BTreeMap<WatchId, Option<u64>>,
    inflight: BTreeSet<WatchId>,
    synced: Option<(SessionId, u64)>,
}
impl Watches {
    /// Records a new event. Only a previously unseen WatchStarted overrides local hiding.
    pub fn started(&mut self, watch: Watch, now: Instant) {
        let is_new = self.started_events.insert(watch.id) && !self.removed.contains(&watch.id);
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
        if self.ids(session).is_empty() {
            return false;
        }
        if let Some(pane) = self.panes.get_mut(session) {
            pane.visible = !pane.visible;
        }
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
            entry.partial = Default::default();
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

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::ids::TerminalId;
    fn watch(id: u64) -> Watch {
        Watch {
            id: WatchId(id),
            session: "repo/main".parse().unwrap(),
            terminal: TerminalId(1),
            label: format!("child {id}"),
            command: vec!["sh".into()],
            cwd: None,
            pid: None,
            started_at: chrono::Utc::now().to_rfc3339(),
            status: WatchStatus::Running,
        }
    }
    fn chunk(seq: u64, stream: WatchStream, text: &str) -> WatchChunk {
        WatchChunk {
            seq,
            stream,
            text: text.into(),
        }
    }
    #[test]
    fn a_new_start_still_opens_when_its_list_response_arrived_first() {
        let mut state = Watches::default();
        let now = Instant::now();
        let w = watch(1);
        state.listed(&w.session, vec![], vec![w.clone()], now);
        state.hide(&w.session);
        state.started(w.clone(), now);
        assert!(state.panes[&w.session].visible);
        state.hide(&w.session);
        state.started(w.clone(), now);
        assert!(!state.panes[&w.session].visible);
    }

    #[test]
    fn lines_are_assembled_per_stream_with_live_partials_and_no_duplicate_replay() {
        let mut state = Watches::default();
        let now = Instant::now();
        let w = watch(1);
        state.started(w.clone(), now);
        state.output(
            w.id,
            vec![
                chunk(0, WatchStream::Stdout, "hel"),
                chunk(1, WatchStream::Stderr, "warn"),
                chunk(2, WatchStream::Stdout, "lo\nsecond\n"),
                chunk(3, WatchStream::Stderr, "ing\npartial"),
            ],
        );
        let mirror = &state.entries[&w.id];
        assert_eq!(
            mirror.display_lines().collect::<Vec<_>>(),
            vec![
                WatchLine {
                    stream: WatchStream::Stdout,
                    text: "hello".into()
                },
                WatchLine {
                    stream: WatchStream::Stdout,
                    text: "second".into()
                },
                WatchLine {
                    stream: WatchStream::Stderr,
                    text: "warning".into()
                },
                WatchLine {
                    stream: WatchStream::Stderr,
                    text: "partial".into()
                }
            ]
        );
        state.output(w.id, vec![chunk(2, WatchStream::Stdout, "duplicated\n")]);
        assert_eq!(state.entries[&w.id].next_seq, 4);
        assert_eq!(state.entries[&w.id].lines.len(), 3);
        assert!(state.take_tails().is_empty());
    }
    #[test]
    fn gap_requests_one_tail_and_merges_overlapping_live_output() {
        let mut state = Watches::default();
        let now = Instant::now();
        let w = watch(1);
        state.started(w.clone(), now);
        state.output(
            w.id,
            vec![
                chunk(0, WatchStream::Stdout, "one\n"),
                chunk(2, WatchStream::Stdout, "three\n"),
            ],
        );
        assert_eq!(state.take_tails(), vec![(w.id, Some(1))]);
        assert!(state.take_tails().is_empty());
        state.output(w.id, vec![chunk(3, WatchStream::Stdout, "four\n")]);
        state.tailed(
            WatchTail {
                watch: w.clone(),
                chunks: vec![
                    chunk(0, WatchStream::Stdout, "one\n"),
                    chunk(1, WatchStream::Stdout, "two\n"),
                    chunk(2, WatchStream::Stdout, "three\n"),
                ],
                first_retained_seq: 0,
                next_seq: 3,
            },
            now,
        );
        assert_eq!(state.entries[&w.id].lines.len(), 3);
        assert_eq!(state.take_tails(), vec![(w.id, Some(3))]);
        state.tailed(
            WatchTail {
                watch: w.clone(),
                chunks: vec![chunk(3, WatchStream::Stdout, "four\n")],
                first_retained_seq: 0,
                next_seq: 4,
            },
            now,
        );
        assert_eq!(state.entries[&w.id].next_seq, 4);
        assert!(state.take_tails().is_empty());
    }
    #[test]
    fn retention_gap_does_not_join_partial_text_across_lost_output() {
        let mut state = Watches::default();
        let now = Instant::now();
        let w = watch(1);
        state.started(w.clone(), now);
        state.output(w.id, vec![chunk(0, WatchStream::Stdout, "partial")]);
        state.tailed(
            WatchTail {
                watch: w.clone(),
                chunks: vec![chunk(8, WatchStream::Stdout, "retained\n")],
                first_retained_seq: 8,
                next_seq: 9,
            },
            now,
        );
        let mirror = &state.entries[&w.id];
        assert!(mirror.trimmed);
        assert_eq!(mirror.next_seq, 9);
        assert_eq!(mirror.lines[0].text, "retained");
    }
    #[test]
    fn completed_and_partial_lines_are_bounded_including_multibyte_text() {
        let now = Instant::now();
        let mut mirror = WatchMirror::new(watch(1), now);
        mirror.append(chunk(
            0,
            WatchStream::Stdout,
            &"line\n".repeat(LINE_CAP + 1),
        ));
        assert!(mirror.trimmed);
        assert_eq!(mirror.lines.len(), LINE_CAP);
        mirror.append(chunk(1, WatchStream::Stderr, &"é".repeat(BYTE_CAP)));
        assert!(
            mirror.line_bytes + mirror.partial.iter().map(String::len).sum::<usize>() <= BYTE_CAP
        );
        assert!(mirror.lines.is_empty());
        assert!(mirror.partial[1].is_char_boundary(mirror.partial[1].len()));
    }
    #[test]
    fn hiding_survives_duplicates_output_exit_and_list_until_a_new_start() {
        let now = Instant::now();
        let mut state = Watches::default();
        let w = watch(1);
        let session = w.session.clone();
        assert!(!state.toggle(&session));
        state.started(w.clone(), now);
        assert!(state.panes[&session].visible);
        assert!(state.toggle(&session));
        state.started(w.clone(), now);
        state.output(w.id, vec![chunk(0, WatchStream::Stdout, "x\n")]);
        let mut exited = w.clone();
        exited.status = WatchStatus::Exited {
            code: Some(3),
            signal: None,
        };
        state.exited(exited.clone(), now + Duration::from_secs(3));
        state.listed(&session, vec![w.id], vec![exited], now);
        assert!(!state.panes[&session].visible);
        let elapsed = state.entries[&w.id].elapsed(now + Duration::from_secs(10));
        assert!(elapsed >= Duration::from_secs(3) && elapsed < Duration::from_secs(4));
        state.started(w, now);
        assert!(matches!(
            state.entries[&WatchId(1)].watch.status,
            WatchStatus::Exited { .. }
        ));
        state.started(watch(2), now);
        assert!(state.panes[&session].visible);
        assert_eq!(state.panes[&session].selected, Some(WatchId(2)));
        state.toggle(&session);
        state.toggle(&session);
        assert_eq!(state.panes[&session].selected, Some(WatchId(2)));
    }
    #[test]
    fn dismissal_selects_next_closes_last_and_stale_replies_cannot_resurrect() {
        let now = Instant::now();
        let mut state = Watches::default();
        let w = watch(1);
        let session = w.session.clone();
        state.started(w.clone(), now);
        state.started(watch(2), now);
        state.dismissed(WatchId(2));
        assert_eq!(state.panes[&session].selected, Some(w.id));
        state.dismissed(w.id);
        assert!(!state.panes[&session].visible);
        state.tailed(
            WatchTail {
                watch: w.clone(),
                chunks: vec![],
                first_retained_seq: 0,
                next_seq: 0,
            },
            now,
        );
        state.listed(&session, vec![], vec![w], now);
        assert!(state.entries.is_empty());
        assert_eq!(state.panes[&session].selected, None);
    }
    #[test]
    fn list_reconciles_known_ids_but_preserves_concurrent_starts_and_local_hiding() {
        let now = Instant::now();
        let mut state = Watches::default();
        let session = watch(1).session;
        state.listed(&session, vec![], vec![watch(1), watch(2)], now);
        assert_eq!(state.panes[&session].selected, Some(WatchId(2)));
        assert_eq!(
            state.take_tails(),
            vec![(WatchId(1), None), (WatchId(2), None)]
        );
        state.hide(&session);
        state.started(watch(3), now);
        state.hide(&session);
        state.listed(&session, vec![WatchId(1), WatchId(2)], vec![watch(2)], now);
        assert!(!state.entries.contains_key(&WatchId(1)));
        assert!(state.entries.contains_key(&WatchId(3)));
        assert_eq!(state.panes[&session].selected, Some(WatchId(3)));
        assert!(!state.panes[&session].visible);
    }
    #[test]
    fn session_entry_reconnect_and_lag_request_fresh_lists() {
        let mut state = Watches::default();
        let session = watch(1).session;
        assert_eq!(state.enter(Some(session.clone()), 1), Some(session.clone()));
        assert_eq!(state.enter(Some(session.clone()), 1), None);
        assert_eq!(state.enter(None, 1), None);
        assert_eq!(state.enter(Some(session.clone()), 1), Some(session.clone()));
        assert_eq!(state.enter(Some(session.clone()), 2), Some(session.clone()));
        state.invalidate();
        assert_eq!(state.enter(Some(session.clone()), 2), Some(session));
    }
}
