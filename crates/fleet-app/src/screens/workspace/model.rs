use super::*;

/// A path interpreted either locally or by a named remote daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub host: Option<HostId>,
    pub path: String,
}

impl Location {
    /// Returns a filesystem path only when this location belongs to the local machine.
    #[must_use]
    pub fn local_path(&self) -> Option<PathBuf> {
        self.host.is_none().then(|| PathBuf::from(&self.path))
    }
}

/// Everything one frame of the Workspace needs, read from [`AppState`] exactly once.
pub(super) struct Model {
    pub(super) session: SessionId,
    pub(super) link_generation: u64,
    pub(super) mode: TerminalMode,
    pub(super) zoomed: bool,
    pub(super) terminal: Option<TerminalId>,
    pub(super) grid_cols: Option<u16>,
    pub(super) history_epoch: Option<u64>,
    pub(super) primed: bool,
    pub(super) alt_screen: bool,
    pub(super) scroll_offset: usize,
    pub(super) scrollback_len: usize,
    pub(super) exit_code: Option<Option<i32>>,
    pub(super) title: SharedString,
    pub(super) branch_key: Option<String>,
    pub(super) repo: Option<RepoId>,
    pub(super) host: Option<(SharedString, HostReachability)>,
    pub(super) status: StatusKind,
    pub(super) keep_alive: Vec<SharedString>,
    pub(super) running_jobs: usize,
    pub(super) failed_jobs: usize,
    pub(super) waking: bool,
    /// The VT modes the active terminal's last frame reported (§3.6: badged in the header).
    pub(super) modes: Vec<KitTerminalMode>,
    /// Whether the active tab is drawn by Fleet rather than by a PTY.
    pub(super) native: bool,
    /// The native agent thread the strip has selected, when an agent tab is active.
    pub(super) agent: Option<ThreadId>,
    /// The worktree the session belongs to, and its path on disk — what a pane is built from.
    pub(super) worktree: Option<(WorktreeId, PathBuf)>,
    /// Whether a Fleet overlay owns the keyboard, in which case no pane may hold it.
    pub(super) overlay_open: bool,
    /// The popup is showing this exact terminal and temporarily owns its PTY dimensions.
    pub(super) popup_owns_terminal: bool,
    /// The popup terminal that must remain attached if this Workspace switches away from it.
    pub(super) popup_terminal: Option<TerminalId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostReachability {
    Unknown,
    Reachable,
    Unreachable,
}

impl HostReachability {
    const fn from_observation(reachable: Option<bool>) -> Self {
        match reachable {
            Some(true) => Self::Reachable,
            Some(false) => Self::Unreachable,
            None => Self::Unknown,
        }
    }

    pub(super) const fn is_reachable(self) -> bool {
        matches!(self, Self::Reachable)
    }
}

impl Model {
    /// The terminal this client attaches to, which is never a Fleet-drawn or agent tab.
    pub(super) const fn attach_target(&self) -> Option<TerminalId> {
        if self.native || self.agent.is_some() {
            None
        } else {
            self.terminal
        }
    }

    pub(super) fn build(app: &AppState, session: &Session) -> Self {
        let terminal = session.active_terminal;
        let grid = terminal.and_then(|id| app.grids.get(&id));
        let popup_terminal = app
            .agent_popup_session()
            .and_then(|session| session.terminals.first())
            .map(|terminal| terminal.id);
        let popup_owns_terminal = terminal.is_some() && popup_terminal == terminal;
        let worktree = worktree_of(app, session);
        // An agent tab is client-side selection over daemon-listed threads, so a thread the
        // snapshot no longer lists silently returns the strip to its terminals.
        let agent = worktree.and_then(|worktree| {
            let thread = app.agents.active(&worktree.id)?;
            app.agents.summary(thread).map(|summary| summary.thread)
        });
        let (title, branch_key, repo, host) = match worktree {
            Some(worktree) => (
                SharedString::new(&worktree.branch),
                Some(worktree.branch.clone()),
                Some(worktree.repo_id.clone()),
                worktree.host.as_ref().map(|host| {
                    let reachable = app.snapshot.as_ref().and_then(|snapshot| {
                        snapshot
                            .hosts
                            .iter()
                            .find(|candidate| &candidate.id == host)
                            .map(|candidate| candidate.reachable)
                    });
                    (
                        SharedString::from(host.to_string()),
                        HostReachability::from_observation(reachable),
                    )
                }),
            ),
            // An agent session has no worktree: its own name is the only identity it has.
            None => (SharedString::from(session.id.to_string()), None, None, None),
        };

        let status = worktree.map_or(StatusKind::Attached, |worktree| {
            let runtime_status = app.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .statuses
                    .iter()
                    .find(|status| status.worktree_id == worktree.id)
            });
            workspace_status(
                runtime_status.map(|status| (status.session, status.agent_activity)),
                session.slept_at.is_some(),
                worktree.degraded.is_some(),
                host.as_ref().map(|(_, reachability)| *reachability),
            )
        });

        let mut keep_alive: Vec<SharedString> = Vec::new();
        for terminal in &session.terminals {
            for label in &terminal.keep_alive {
                let label = SharedString::new(label);
                if !keep_alive.contains(&label) {
                    keep_alive.push(label);
                }
            }
        }

        let targets: Vec<String> = worktree.map_or_else(Vec::new, |worktree| {
            vec![
                worktree.id.to_string(),
                worktree.repo_id.to_string(),
                worktree.slug.clone(),
            ]
        });
        let (running_jobs, failed_jobs) = app
            .snapshot
            .as_ref()
            .map_or((0, 0), |snapshot| job_counts(&snapshot.jobs, &targets));

        let exit_code = session
            .terminals
            .iter()
            .find(|candidate| Some(candidate.id) == terminal)
            .and_then(|candidate| match candidate.status {
                TerminalStatus::Exited { code } => Some(code),
                TerminalStatus::Running | TerminalStatus::Starting => None,
            });

        Self {
            session: session.id.clone(),
            link_generation: app.link_generation,
            mode: app.terminal_mode,
            zoomed: app.zoomed,
            terminal,
            grid_cols: grid.map(|grid| grid.cols),
            history_epoch: grid.map(|grid| grid.viewport.history_epoch),
            primed: grid.is_some_and(|grid| grid.primed),
            alt_screen: grid.is_some_and(|grid| grid.modes.alt_screen),
            scroll_offset: grid.map_or(0, |grid| grid.viewport.offset),
            scrollback_len: grid.map_or(0, |grid| grid.viewport.scrollback_len),
            exit_code,
            title,
            branch_key,
            repo,
            host,
            status,
            keep_alive,
            running_jobs,
            failed_jobs,
            native: agent.is_none()
                && session
                    .terminals
                    .iter()
                    .any(|entry| Some(entry.id) == terminal && entry.is_native()),
            agent,
            worktree: worktree.map(|worktree| (worktree.id.clone(), PathBuf::from(&worktree.path))),
            overlay_open: app.overlay.is_some() || app.agent_popup.is_some(),
            popup_owns_terminal,
            popup_terminal,
            waking: session.slept_at.is_some()
                && session
                    .terminals
                    .iter()
                    .any(|terminal| terminal.status == TerminalStatus::Starting),
            modes: grid.map_or_else(Vec::new, |grid| grid_modes(&grid.modes)),
        }
    }
}

/// The badge a pull request's facts add up to (§3.5 priority order).
#[must_use]
pub(super) fn badge_state(
    is_draft: bool,
    checks: PrChecks,
    review: PrReviewDecision,
) -> PrBadgeState {
    crate::presentation::pr_badge_state(derive_pr_state(is_draft, checks, review))
}

/// How many of a snapshot's jobs belong to this session, running and failed (§3.6 `⟳n` / `⚠n`).
#[must_use]
pub(super) fn job_counts(jobs: &[JobRecord], targets: &[String]) -> (usize, usize) {
    let mut running = 0;
    let mut failed = 0;
    for job in jobs.iter().filter(|job| {
        let target = crate::presentation::job_target(&job.kind, &job.target);
        targets.iter().any(|candidate| candidate == target)
    }) {
        match job.status {
            JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling => running += 1,
            JobStatus::Failed { .. } => failed += 1,
            JobStatus::Succeeded | JobStatus::Cancelled => {}
        }
    }
    (running, failed)
}

#[must_use]
pub(super) fn workspace_status(
    runtime: Option<(SessionState, AgentActivity)>,
    sleeping: bool,
    degraded: bool,
    host: Option<HostReachability>,
) -> StatusKind {
    match host {
        Some(HostReachability::Unreachable) => return StatusKind::HostUnreachable,
        Some(HostReachability::Unknown) => return StatusKind::Unknown,
        Some(HostReachability::Reachable) | None => {}
    }
    let (session, activity) = runtime.unwrap_or((SessionState::Unknown, AgentActivity::Unknown));
    status_kind(session, sleeping, activity, degraded)
}

/// The status glyph a session shows, identical to the Hub's for the same worktree (§2.5).
#[must_use]
pub(crate) fn status_kind(
    session: SessionState,
    sleeping: bool,
    agent_activity: AgentActivity,
    degraded: bool,
) -> StatusKind {
    if degraded {
        return StatusKind::Degraded;
    }
    session_glyph(session, sleeping, agent_activity)
}

/// The worktree a session belongs to, when it is a worktree session.
pub(super) fn worktree_of<'a>(app: &'a AppState, session: &Session) -> Option<&'a Worktree> {
    let SessionKind::Worktree(id) = &session.kind else {
        return None;
    };
    app.snapshot
        .as_ref()?
        .worktrees
        .iter()
        .find(|worktree| &worktree.id == id)
}
