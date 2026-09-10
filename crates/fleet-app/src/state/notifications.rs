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

fn aggregate_agent_attention(
    windows: &[fleet_core::sessions::WorktreeWindowStatus],
) -> Option<AttentionKind> {
    highest_agent_attention(windows.iter().filter_map(|window| window.agent_attention))
}

fn aggregate_session_agent_attention(session: &Session) -> Option<AttentionKind> {
    highest_agent_attention(
        session
            .terminals
            .iter()
            .filter_map(|terminal| terminal.agent_attention),
    )
}

fn highest_agent_attention(
    attention: impl Iterator<Item = AttentionKind>,
) -> Option<AttentionKind> {
    attention.max_by_key(|kind| fleet_core::agents::Attention::NeedsYou(*kind).rank())
}

/// Aggregate terminal-agent state keyed by the real session id represented in a snapshot.
fn snapshot_agent_states(
    snapshot: &Snapshot,
) -> impl Iterator<Item = (&SessionId, AgentActivity, Option<AttentionKind>)> {
    let mut statuses = HashMap::with_capacity(snapshot.statuses.len());
    for status in &snapshot.statuses {
        statuses.entry(&status.worktree_id).or_insert((
            status.agent_activity,
            aggregate_agent_attention(&status.windows),
        ));
    }
    snapshot.sessions.iter().map(move |session| {
        let (activity, status_attention) = match &session.kind {
            SessionKind::Worktree(worktree) => statuses
                .get(worktree)
                .copied()
                .unwrap_or((AgentActivity::Unknown, None)),
            SessionKind::Agent(_) => (AgentActivity::Unknown, None),
        };
        let attention = aggregate_session_agent_attention(session).or(status_attention);
        (&session.id, activity, attention)
    })
}

/// Patches an activity event into the current snapshot and returns its new session aggregate.
fn patch_snapshot_agent_activity(
    snapshot: &mut Snapshot,
    session: &SessionId,
    terminal_id: TerminalId,
    agent: Option<String>,
    activity: AgentActivity,
    attention: Option<AttentionKind>,
    changed_at: String,
) -> Option<(AgentActivity, Option<AttentionKind>)> {
    let (worktree, index, session_attention) = {
        let session = snapshot
            .sessions
            .iter_mut()
            .find(|candidate| &candidate.id == session)?;
        let index = session
            .terminals
            .iter()
            .position(|terminal| terminal.id == terminal_id)?;
        session.terminals[index].agent_attention = attention;
        let session_attention = aggregate_session_agent_attention(session);
        let SessionKind::Worktree(worktree) = &session.kind else {
            return Some((activity, session_attention));
        };
        (
            worktree.clone(),
            u32::try_from(index).ok()?,
            session_attention,
        )
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
    window.agent_attention = attention;
    window.agent_activity_changed_at = Some(changed_at);
    let (aggregate, changed_at) = aggregate_agent_activity(&status.windows);
    status.agent_activity = aggregate;
    status.agent_activity_changed_at = changed_at;
    Some((
        aggregate,
        aggregate_agent_attention(&status.windows).or(session_attention),
    ))
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
            .copied()
            .unwrap_or(AgentActivity::Unknown)
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
    pub(super) fn seed_agent_activity(&mut self, snapshot: &Snapshot, _now: Instant) {
        self.agents.seed(&snapshot.agent_threads);
        self.last_agent_activity = snapshot_agent_states(snapshot)
            .map(|(session, activity, _)| (session.clone(), activity))
            .collect();
        self.last_agent_attention = snapshot_agent_states(snapshot)
            .map(|(session, _, attention)| (session.clone(), attention))
            .collect();
    }

    /// Records status-only heuristic activity. It never creates user notifications.
    fn observe_agent_activity(&mut self, session: SessionId, activity: AgentActivity) {
        self.last_agent_activity.insert(session, activity);
    }

    /// Records semantic attention and returns a new reason exactly once per session edge.
    fn observe_agent_attention(
        &mut self,
        session: SessionId,
        attention: Option<AttentionKind>,
    ) -> Option<AttentionKind> {
        let previous = self
            .last_agent_attention
            .insert(session, attention)
            .flatten();
        attention.filter(|attention| Some(*attention) != previous)
    }

    /// Presents one native-agent attention edge through the same channels (§9, §3.3).
    ///
    /// Both structured threads and explicit terminal hooks know why they want the user, so the
    /// copy names that shared reason and the notification channels remain unchanged.
    pub(super) fn notify_agent_thread(
        &mut self,
        label: &str,
        attention: fleet_core::agents::Attention,
        now: Instant,
    ) {
        use fleet_core::agents::{Attention, AttentionKind};
        let (text, icon, tone) = match attention {
            Attention::NeedsYou(AttentionKind::Permission) => (
                format!("{label}: needs permission"),
                Icon::Lock,
                Tone::Warning,
            ),
            Attention::NeedsYou(AttentionKind::Question) => (
                format!("{label}: asks a question"),
                Icon::CircleQuestionMark,
                Tone::Warning,
            ),
            Attention::NeedsYou(AttentionKind::Plan) => (
                format!("{label}: proposed a plan"),
                Icon::ClipboardCheck,
                Tone::Warning,
            ),
            // §2/§3.3: amber is "needs you", and the Signals board maps *agent finished* to
            // exactly that; red is reserved for broken.
            Attention::NeedsYou(AttentionKind::Finished) => (
                format!("{label}: agent finished"),
                Icon::CircleCheck,
                Tone::Warning,
            ),
            Attention::Failed => (format!("{label}: failed"), Icon::CircleX, Tone::Danger),
            Attention::Working | Attention::Unread | Attention::Idle => return,
        };
        if self.notifications.toast {
            self.toast(
                Toast::new(text).icon(icon).tone(tone),
                now,
                dwell_for(ToastDuration::Normal),
            );
        }
        if self.notifications.sound {
            self.notification_sound.play();
        }
    }

    /// Compares all aggregate terminal-agent state in a full snapshot.
    pub(super) fn observe_snapshot_agent_activity(&mut self, snapshot: &Snapshot, now: Instant) {
        let live: HashSet<_> = snapshot
            .sessions
            .iter()
            .map(|session| &session.id)
            .collect();
        let mut attention_edges = Vec::new();
        for (session, activity, attention) in snapshot_agent_states(snapshot) {
            self.observe_agent_activity(session.clone(), activity);
            if let Some(attention) = self.observe_agent_attention(session.clone(), attention) {
                attention_edges.push((session, attention));
            }
        }
        self.last_agent_activity
            .retain(|session, _| live.contains(session));
        self.last_agent_attention
            .retain(|session, _| live.contains(session));
        for (session, attention) in attention_edges {
            self.notify_agent_thread(
                session.as_str(),
                fleet_core::agents::Attention::NeedsYou(attention),
                now,
            );
        }
    }

    /// Applies one terminal status/attention edge and presents only semantic attention.
    pub fn apply_agent_activity(
        &mut self,
        session: SessionId,
        terminal_id: TerminalId,
        agent: Option<String>,
        state: (AgentActivity, Option<AttentionKind>),
        changed_at: String,
        now: Instant,
    ) {
        let (activity, attention) = state;
        let (aggregate, attention) = self
            .snapshot
            .as_mut()
            .and_then(|snapshot| {
                patch_snapshot_agent_activity(
                    snapshot,
                    &session,
                    terminal_id,
                    agent,
                    activity,
                    attention,
                    changed_at,
                )
            })
            .unwrap_or((activity, attention));
        self.observe_agent_activity(session.clone(), aggregate);
        if let Some(attention) = self.observe_agent_attention(session.clone(), attention) {
            self.notify_agent_thread(
                session.as_str(),
                fleet_core::agents::Attention::NeedsYou(attention),
                now,
            );
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
