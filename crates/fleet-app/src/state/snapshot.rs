use super::*;
use fleet_core::sessions::SessionState;

/// The zero-suppressed counters of §2.3.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChipCounts {
    /// Running jobs.
    pub running: usize,
    /// Failed jobs that have not been seen.
    pub failed: usize,
    /// Attached sessions.
    pub live: usize,
    /// Detached sessions, awake or slept.
    pub sleeping: usize,
    /// Unknown sessions plus unreachable hosts. Never collapsed into another chip (§D-1).
    pub unknown: usize,
    /// Pull requests waiting for review.
    pub review: usize,
}

impl ChipCounts {
    /// Counts the chips of §2.3 from the snapshot they summarize.
    #[must_use]
    pub fn from_snapshot(snapshot: &Snapshot, review: usize) -> Self {
        let mut counts = Self {
            review,
            ..Self::default()
        };
        for job in &snapshot.jobs {
            counts.running += usize::from(crate::presentation::is_active(&job.status));
            counts.failed += usize::from(matches!(job.status, JobStatus::Failed { .. }));
        }
        for status in &snapshot.statuses {
            match status.session {
                SessionState::Attached => counts.live += 1,
                SessionState::Detached => counts.sleeping += 1,
                SessionState::Unknown => counts.unknown += 1,
                SessionState::None => {}
            }
        }
        counts.unknown += snapshot.hosts.iter().filter(|host| !host.reachable).count();
        counts
    }
}

/// The status bar's `context › repo › row` breadcrumb (§2.2), zero-suppressed.
#[must_use]
pub fn breadcrumb(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" › ")
}

impl AppState {
    /// Whether the first-run card replaces the whole window (§3.13).
    #[must_use]
    pub fn is_first_run(&self) -> bool {
        !self.has_seen_non_empty_state
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.contexts.is_empty()
                    && snapshot.repos.is_empty()
                    && snapshot.clones.is_empty()
            })
    }

    /// Replaces the snapshot mirror and re-derives everything that hangs off it.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot, now: Instant) {
        self.observe_snapshot_agent_activity(&snapshot, now);
        self.has_seen_non_empty_state |= !snapshot.contexts.is_empty()
            || !snapshot.repos.is_empty()
            || !snapshot.clones.is_empty();
        let live_jobs: HashSet<_> = snapshot.jobs.iter().map(|job| &job.id).collect();
        self.seen_failed.retain(|id| live_jobs.contains(id));
        notifications::refresh_job_sticky_error(
            &mut self.sticky_error,
            &snapshot.jobs,
            &self.seen_failed,
        );
        self.cursors.repos = clamp_cursor(self.cursors.repos, snapshot.repos.len() + 1);
        self.cursors.worktrees = clamp_cursor(self.cursors.worktrees, snapshot.worktrees.len());
        self.cursors.jobs = clamp_cursor(self.cursors.jobs, snapshot.jobs.len());
        self.forget_vanished(&snapshot);
        self.snapshot = Some(snapshot);
        self.snapshot_at = Some(now);
        self.bump_snapshot_revision();
        self.sync_terminal_mode();
    }

    /// Records that the snapshot mirror changed, invalidating every projection keyed on it.
    pub(super) fn bump_snapshot_revision(&mut self) {
        self.snapshot_revision = self.snapshot_revision.wrapping_add(1);
    }

    /// Reclaims terminal mirrors and local indexes missing from an authoritative snapshot.
    pub(super) fn forget_vanished(&mut self, snapshot: &Snapshot) {
        let live_terminals: HashSet<TerminalId> = snapshot
            .sessions
            .iter()
            .flat_map(|session| session.terminals.iter().map(|terminal| terminal.id))
            .collect();
        self.grids
            .retain(|terminal, _| live_terminals.contains(terminal));

        let live_sessions: HashSet<&SessionId> = snapshot
            .sessions
            .iter()
            .map(|session| &session.id)
            .collect();
        self.watches.reconcile_sessions(&live_sessions);
        self.session_mru
            .retain(|session| live_sessions.contains(session));
        self.renamed_terminals
            .retain(|terminal| live_terminals.contains(terminal));
        self.terminal_mru
            .retain(|session, _| live_sessions.contains(session));
        for mru in self.terminal_mru.values_mut() {
            mru.retain(|terminal| live_terminals.contains(terminal));
        }
    }

    /// The active context, when the snapshot names one.
    #[must_use]
    pub fn active_context(&self) -> Option<&ContextId> {
        self.snapshot.as_ref()?.active_context.as_ref()
    }

    /// The age of the snapshot in seconds, for the `stale · <age>` stamp (§1.3).
    #[must_use]
    pub fn snapshot_age(&self, now: Instant) -> Option<u64> {
        self.snapshot_at
            .map(|at| now.saturating_duration_since(at).as_secs())
    }

    /// Patches one session into the snapshot mirror.
    pub fn apply_session(&mut self, session: Session) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        match snapshot
            .sessions
            .iter_mut()
            .find(|existing| existing.id == session.id)
        {
            Some(existing) => *existing = session,
            None => snapshot.sessions.push(session),
        }
        self.bump_snapshot_revision();
    }
}

#[cfg(test)]
mod tests;
