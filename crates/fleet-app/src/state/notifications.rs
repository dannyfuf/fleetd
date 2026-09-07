use super::*;

/// A toast plus the instants that govern its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveToast {
    /// The kit toast being rendered.
    pub toast: Toast,
    /// When it first appeared. Coalescing compares against this.
    pub shown_at: Instant,
    /// When it must be removed.
    pub expires_at: Instant,
}

/// Pushes a toast, applying the §2.7 law: identical text within one second coalesces into
/// `×n`, and the stack never exceeds [`MAX_TOASTS`].
///
/// Errors are never toasts (§1.8); a `Danger` tone is downgraded to `Warning` so a caller
/// cannot smuggle one in.
pub fn push_toast(toasts: &mut Vec<LiveToast>, toast: Toast, now: Instant, dwell: Duration) {
    let mut toast = toast;
    if toast.tone == Tone::Danger {
        toast.tone = Tone::Warning;
    }
    if let Some(existing) = toasts
        .iter_mut()
        .find(|live| live.toast.text == toast.text && now - live.shown_at <= TOAST_COALESCE_WINDOW)
    {
        existing.toast.count += 1;
        existing.expires_at = now + dwell;
        return;
    }
    toasts.push(LiveToast {
        toast,
        shown_at: now,
        expires_at: now + dwell,
    });
    while toasts.len() > MAX_TOASTS {
        toasts.remove(0);
    }
}

/// Removes expired toasts, returning whether anything was removed.
pub fn expire_toasts(toasts: &mut Vec<LiveToast>, now: Instant) -> bool {
    let before = toasts.len();
    toasts.retain(|live| live.expires_at > now);
    toasts.len() != before
}

/// The status bar's sticky error slot: the last failed job, addressable with `!` (§1.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickyError {
    /// The one-line message.
    pub text: String,
    /// The failed job, when the error came from one.
    pub job: Option<JobId>,
    /// Whether `R` can retry it.
    pub retryable: bool,
}

/// The newest failed job, which owns the sticky error slot.
#[must_use]
pub fn latest_failed_job(jobs: &[JobRecord]) -> Option<&JobRecord> {
    crate::presentation::latest_unseen_failure(jobs, |_| false)
}

/// Every job that is queued, running or cancelling — the work the quit dialogs enumerate.
#[must_use]
pub fn running_jobs(jobs: &[JobRecord]) -> Vec<&JobRecord> {
    jobs.iter()
        .filter(|job| crate::presentation::is_active(&job.status))
        .collect()
}

/// Aggregate activity keyed by the real session id represented in a snapshot.
fn snapshot_agent_activities(
    snapshot: &Snapshot,
) -> impl Iterator<Item = (&SessionId, AgentActivity)> {
    let mut statuses = HashMap::with_capacity(snapshot.statuses.len());
    for status in &snapshot.statuses {
        statuses
            .entry(&status.worktree_id)
            .or_insert(status.agent_activity);
    }
    snapshot.sessions.iter().filter_map(move |session| {
        let SessionKind::Worktree(worktree) = &session.kind else {
            return None;
        };
        let activity = statuses
            .get(worktree)
            .copied()
            .unwrap_or(AgentActivity::Unknown);
        Some((&session.id, activity))
    })
}

/// Patches an activity event into the current snapshot and returns its new session aggregate.
fn patch_snapshot_agent_activity(
    snapshot: &mut Snapshot,
    session: &SessionId,
    terminal_id: TerminalId,
    agent: Option<String>,
    activity: AgentActivity,
    changed_at: String,
) -> Option<AgentActivity> {
    let (worktree, index) = {
        let session = snapshot
            .sessions
            .iter()
            .find(|candidate| &candidate.id == session)?;
        let SessionKind::Worktree(worktree) = &session.kind else {
            return None;
        };
        let index = session
            .terminals
            .iter()
            .position(|terminal| terminal.id == terminal_id)?;
        (worktree.clone(), u32::try_from(index).ok()?)
    };
    let status = snapshot
        .statuses
        .iter_mut()
        .find(|status| status.worktree_id == worktree)?;
    let window = status
        .windows
        .iter_mut()
        .find(|window| window.index == index)?;
    window.agent = agent;
    window.agent_activity = activity;
    window.agent_activity_changed_at = Some(changed_at);
    let (aggregate, changed_at) = aggregate_agent_activity(&status.windows);
    status.agent_activity = aggregate;
    status.agent_activity_changed_at = changed_at;
    Some(aggregate)
}

/// The dwell of a kit toast duration, as milliseconds are a theme token the state cannot read.
#[must_use]
pub fn dwell_for(duration: ToastDuration) -> Duration {
    let motion = fleet_ui_kit::theme::Motion::default();
    Duration::from_millis(match duration {
        ToastDuration::Short => motion.toast_short,
        ToastDuration::Normal => motion.toast_normal,
    })
}

impl AppState {
    /// Latest aggregate activity for a daemon session, shared by Workspace status and the popup.
    #[must_use]
    pub fn session_agent_activity(&self, session: &SessionId) -> AgentActivity {
        self.last_agent_activity
            .get(session)
            .map_or(AgentActivity::Unknown, |(activity, _)| *activity)
    }

    /// Records a toast under the §2.7 law.
    pub fn toast(&mut self, toast: Toast, now: Instant, dwell: Duration) {
        push_toast(&mut self.toasts, toast, now, dwell);
    }

    /// Records a short clipboard-style acknowledgement.
    pub fn toast_short(&mut self, text: impl Into<gpui::SharedString>, icon: Icon, now: Instant) {
        self.toast(
            Toast::new(text).icon(icon).short(),
            now,
            dwell_for(ToastDuration::Short),
        );
    }

    /// Applies a daemon toast event. Errors are sticky, never transient (§1.8).
    pub fn apply_toast_event(&mut self, level: ToastLevel, message: String, now: Instant) {
        match level {
            ToastLevel::Error => {
                self.sticky_error = Some(StickyError {
                    text: message,
                    job: None,
                    retryable: false,
                });
            }
            ToastLevel::Warning => self.toast(
                Toast::new(message).icon(Icon::Info).tone(Tone::Warning),
                now,
                dwell_for(ToastDuration::Normal),
            ),
            ToastLevel::Info => self.toast(
                Toast::new(message).icon(Icon::Info),
                now,
                dwell_for(ToastDuration::Normal),
            ),
        }
    }

    /// Replaces agent-activity baselines without presenting historical completions.
    pub(super) fn seed_agent_activity(&mut self, snapshot: &Snapshot, now: Instant) {
        self.last_agent_activity = snapshot_agent_activities(snapshot)
            .map(|(session, activity)| (session.clone(), (activity, now)))
            .collect();
    }

    /// Records one aggregate activity observation and returns whether it completed real work.
    fn observe_agent_activity(
        &mut self,
        session: SessionId,
        activity: AgentActivity,
        now: Instant,
    ) -> bool {
        let finished = self
            .last_agent_activity
            .get(&session)
            .is_some_and(|(previous, seen_at)| {
                *previous == AgentActivity::Working
                    && activity == AgentActivity::Idle
                    && now.saturating_duration_since(*seen_at) >= AGENT_FINISH_MIN_WORKING
            });
        match self.last_agent_activity.get_mut(&session) {
            Some((previous, _)) if *previous == activity => {}
            Some(entry) => *entry = (activity, now),
            None => {
                self.last_agent_activity.insert(session, (activity, now));
            }
        }
        finished
    }

    /// Presents an agent completion through each enabled notification channel.
    fn notify_agent_finished(&mut self, label: &str, now: Instant) {
        if self.notifications.toast {
            self.toast(
                Toast::new(format!("{label}: agent finished"))
                    .icon(Icon::CircleCheck)
                    .tone(Tone::Success),
                now,
                dwell_for(ToastDuration::Normal),
            );
        }
        if self.notifications.sound {
            self.notification_sound.play();
        }
    }

    /// Compares all aggregate session activities in a full snapshot.
    pub(super) fn observe_snapshot_agent_activity(&mut self, snapshot: &Snapshot, now: Instant) {
        let live: HashSet<_> = snapshot
            .sessions
            .iter()
            .map(|session| &session.id)
            .collect();
        let mut finished = Vec::new();
        for (session, activity) in snapshot_agent_activities(snapshot) {
            if self.observe_agent_activity(session.clone(), activity, now) {
                finished.push(session);
            }
        }
        self.last_agent_activity
            .retain(|session, _| live.contains(session));
        for session in finished {
            self.notify_agent_finished(session.as_str(), now);
        }
    }

    /// Applies one terminal activity edge and presents a qualifying aggregate completion.
    pub fn apply_agent_activity(
        &mut self,
        session: SessionId,
        terminal_id: TerminalId,
        agent: Option<String>,
        activity: AgentActivity,
        changed_at: String,
        now: Instant,
    ) {
        let aggregate = self
            .snapshot
            .as_mut()
            .and_then(|snapshot| {
                patch_snapshot_agent_activity(
                    snapshot,
                    &session,
                    terminal_id,
                    agent,
                    activity,
                    changed_at,
                )
            })
            .unwrap_or(activity);
        if self.observe_agent_activity(session.clone(), aggregate, now) {
            self.notify_agent_finished(session.as_str(), now);
        }
    }

    /// Patches one job into the snapshot mirror and re-derives the sticky error slot.
    pub fn apply_job(&mut self, job: JobRecord, now: Instant) {
        let outcome = crate::presentation::job_outcome_toast(
            &job,
            matches!(self.overlay, Some(Overlay::Jobs)),
        );
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        match snapshot
            .jobs
            .iter_mut()
            .find(|existing| existing.id == job.id)
        {
            Some(existing) => *existing = job,
            None => snapshot.jobs.push(job),
        }
        refresh_job_sticky_error(&mut self.sticky_error, &snapshot.jobs, &self.seen_failed);
        self.bump_snapshot_revision();
        if let Some(text) = outcome {
            self.toast(
                Toast::new(text).icon(Icon::CircleCheck),
                now,
                dwell_for(ToastDuration::Normal),
            );
        }
    }
}

/// Refreshes a job-owned slot without erasing an unrelated explicit error.
pub(super) fn refresh_job_sticky_error(
    slot: &mut Option<StickyError>,
    jobs: &[JobRecord],
    seen: &HashSet<JobId>,
) {
    if slot.as_ref().is_some_and(|error| error.job.is_none()) {
        return;
    }
    *slot = sticky_error_for(jobs, seen);
}

/// Derives job failures without coupling the reducer to a view.
pub(super) fn sticky_error_for(jobs: &[JobRecord], seen: &HashSet<JobId>) -> Option<StickyError> {
    let job = crate::presentation::latest_unseen_failure(jobs, |id| seen.contains(id))?;
    let JobStatus::Failed { error } = &job.status else {
        return None;
    };
    Some(StickyError {
        text: error.clone(),
        job: Some(job.id.clone()),
        retryable: job.retryable,
    })
}

#[cfg(test)]
mod tests;
