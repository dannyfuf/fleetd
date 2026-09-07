use super::*;

impl SessionRuntime {
    pub(crate) fn update_observation(
        &self,
        terminal: TerminalId,
        foreground_command: Option<String>,
        keep_alive: Vec<String>,
        agent: Option<String>,
    ) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session_id) = registry.terminal_sessions.get(&terminal).cloned() else {
            return;
        };
        let Some(session) = registry.sessions.get_mut(&session_id) else {
            return;
        };
        let mut changed = false;
        if let Some(entry) = session
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
            && (entry.foreground_command != foreground_command || entry.keep_alive != keep_alive)
        {
            entry.foreground_command = foreground_command;
            entry.keep_alive = keep_alive;
            changed = true;
        }
        changed |= registry.observed_agents.get(&terminal) != Some(&agent);
        registry.observed_agents.insert(terminal, agent);
        drop(registry);
        if changed {
            self.notify_session(&session_id);
        }
    }

    pub(crate) fn observe_agent_activities(&self, now: Instant) -> Vec<AgentActivityTransition> {
        let observations = {
            let registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry
                .hosts
                .iter()
                .filter_map(|(terminal, host)| {
                    registry
                        .observed_agents
                        .get(terminal)
                        .cloned()
                        .map(|agent| (*terminal, host.activity(), agent))
                })
                .collect::<Vec<_>>()
        };
        let changed_at = chrono::Utc::now().to_rfc3339();
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut transitions = Vec::new();
        for (terminal, activity, agent) in observations {
            let Some(next) = registry
                .activity_trackers
                .entry(terminal)
                .or_default()
                .observe(now, activity, agent.as_deref())
            else {
                continue;
            };
            let Some(session) = registry.terminal_sessions.get(&terminal).cloned() else {
                continue;
            };
            registry
                .activity_changed_at
                .insert(terminal, changed_at.clone());
            transitions.push(AgentActivityTransition {
                session,
                terminal,
                agent,
                activity: next,
                changed_at: changed_at.clone(),
            });
        }
        transitions
    }

    pub(crate) fn set_agent_activity(
        &self,
        session: &SessionId,
        terminal: TerminalId,
        activity: AgentActivity,
        now: Instant,
    ) -> DaemonResult<Option<AgentActivityTransition>> {
        let changed_at = chrono::Utc::now().to_rfc3339();
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owns_terminal = registry
            .sessions
            .get(session)
            .is_some_and(|entry| entry.terminals.iter().any(|entry| entry.id == terminal));
        if !owns_terminal {
            return Err(DaemonError::NotFound(format!(
                "terminal `{terminal}` in session `{session}`"
            )));
        }
        let output_bytes_total = registry
            .hosts
            .get(&terminal)
            .map(|host| host.activity().output_bytes_total);
        let changed = registry
            .activity_trackers
            .entry(terminal)
            .or_default()
            .set_explicit(activity, now, output_bytes_total);
        let Some(activity) = changed else {
            return Ok(None);
        };
        registry
            .activity_changed_at
            .insert(terminal, changed_at.clone());
        Ok(Some(AgentActivityTransition {
            session: session.clone(),
            terminal,
            agent: registry.observed_agents.get(&terminal).cloned().flatten(),
            activity,
            changed_at,
        }))
    }
}

impl Sessions {
    /// Computes local worktree status from the runtime registry.
    pub async fn refresh_statuses(
        &self,
        repo: Option<RepoId>,
    ) -> DaemonResult<Vec<WorktreeStatus>> {
        let state = self.state.load().await?;
        let registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(state
            .worktrees
            .iter()
            .filter(|worktree| repo.as_ref().is_none_or(|repo| &worktree.repo_id == repo))
            .map(|worktree| {
                if worktree.host.is_some() {
                    return unknown_status(worktree.id.clone());
                }
                let Ok(session_id) = SessionId::try_from(worktree.session.as_str()) else {
                    return unknown_status(worktree.id.clone());
                };
                let Some(session) = registry.sessions.get(&session_id) else {
                    return WorktreeStatus {
                        worktree_id: worktree.id.clone(),
                        session: SessionState::None,
                        windows: Vec::new(),
                        running: Vec::new(),
                        agent_activity: AgentActivity::Unknown,
                        agent_activity_changed_at: None,
                    };
                };
                // A native tab is never attached in the daemon's sense — the client draws it
                // — so it must not be able to report a session as attached on its own.
                let attached = session
                    .terminals
                    .iter()
                    .filter(|terminal| !terminal.is_native())
                    .any(|terminal| {
                        registry.attachments.get(&terminal.id).copied().unwrap_or(0) > 0
                    });
                let windows = session
                    .terminals
                    .iter()
                    .enumerate()
                    .map(|(index, terminal)| WorktreeWindowStatus {
                        index: u32::try_from(index).unwrap_or(u32::MAX),
                        name: terminal.name.clone(),
                        command: terminal
                            .foreground_command
                            .clone()
                            .unwrap_or_else(|| terminal.command.clone()),
                        keep_alive: terminal.keep_alive.clone(),
                        agent: registry
                            .observed_agents
                            .get(&terminal.id)
                            .cloned()
                            .flatten(),
                        agent_activity: registry
                            .activity_trackers
                            .get(&terminal.id)
                            .map_or(AgentActivity::Unknown, AgentActivityTracker::activity),
                        agent_activity_changed_at: registry
                            .activity_changed_at
                            .get(&terminal.id)
                            .cloned(),
                    })
                    .collect::<Vec<_>>();
                let (agent_activity, agent_activity_changed_at) =
                    aggregate_agent_activity(&windows);
                let mut running = Vec::new();
                for label in session
                    .terminals
                    .iter()
                    .flat_map(|terminal| terminal.keep_alive.iter())
                {
                    if !running.contains(label) {
                        running.push(label.clone());
                    }
                }
                WorktreeStatus {
                    worktree_id: worktree.id.clone(),
                    session: if attached {
                        SessionState::Attached
                    } else {
                        SessionState::Detached
                    },
                    windows,
                    running,
                    agent_activity,
                    agent_activity_changed_at,
                }
            })
            .collect())
    }

    pub(crate) fn observe_agent_activities(&self, now: Instant) -> Vec<AgentActivityTransition> {
        self.runtime.observe_agent_activities(now)
    }

    pub(crate) fn set_agent_activity(
        &self,
        session: &SessionId,
        terminal: TerminalId,
        activity: AgentActivity,
        now: Instant,
    ) -> DaemonResult<Option<AgentActivityTransition>> {
        self.runtime
            .set_agent_activity(session, terminal, activity, now)
    }
}
