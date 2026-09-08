use super::*;

impl Sessions {
    /// Ensures a worktree or agent session and repairs configured terminals missing by name.
    pub async fn ensure(
        &self,
        worktree: Option<WorktreeId>,
        agent: Option<Agent>,
        sleep_previous: bool,
    ) -> DaemonResult<Session> {
        let _worktree_lifecycle = if let Some(worktree) = worktree.as_ref() {
            Some(
                self.runtime
                    .claim_worktree_lifecycle(worktree.clone())
                    .await,
            )
        } else {
            None
        };
        let config = self.config.load().await?;
        let (session_id, kind, cwd, specs) = match (worktree, agent) {
            (Some(worktree_id), None) => {
                let state = self.state.load().await?;
                let worktree = state
                    .worktrees
                    .iter()
                    .find(|entry| entry.id == worktree_id)
                    .ok_or_else(|| DaemonError::NotFound(worktree_id.to_string()))?;
                if worktree.host.is_some() {
                    return Err(remote_unsupported());
                }
                let session_id = SessionId::try_from(worktree.session.as_str())
                    .map_err(|error| DaemonError::Validation(error.to_string()))?;
                let specs = default_terminals(&config, config.agent, false);
                (
                    session_id,
                    SessionKind::Worktree(worktree_id),
                    worktree.path.clone(),
                    specs,
                )
            }
            // §1/§2: `^s F` is the same-worktree terminal fallback for the thread on screen, so
            // the popup session it ensures is the worktree's own — one per worktree and agent,
            // running the configured agent command in the worktree path. Without the worktree
            // the request is the repository-level popup below, which lives in `repos_dir`.
            (Some(worktree_id), Some(agent)) => {
                let state = self.state.load().await?;
                let worktree = state
                    .worktrees
                    .iter()
                    .find(|entry| entry.id == worktree_id)
                    .ok_or_else(|| DaemonError::NotFound(worktree_id.to_string()))?;
                if worktree.host.is_some() {
                    return Err(remote_unsupported());
                }
                let session_id = worktree_agent_session_id(&worktree.session, agent)
                    .map_err(|error| DaemonError::Validation(error.to_string()))?;
                let name = match agent {
                    Agent::Claude => "claude",
                    Agent::Opencode => "opencode",
                };
                (
                    session_id,
                    SessionKind::Agent(agent),
                    worktree.path.clone(),
                    vec![fleet_core::sessions::TerminalSpec {
                        name: name.to_owned(),
                        command: config.agent_commands.command(agent).to_owned(),
                    }],
                )
            }
            (None, Some(agent)) => {
                let session_id = agent_session_id(agent)
                    .map_err(|error| DaemonError::Validation(error.to_string()))?;
                let repos_dir = config.repos_dir.clone();
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&repos_dir)
                        .map_err(|error| DaemonError::fs(&repos_dir, error))
                })
                .await
                .map_err(|error| DaemonError::Join(error.to_string()))??;
                let name = match agent {
                    Agent::Claude => "claude",
                    Agent::Opencode => "opencode",
                };
                (
                    session_id,
                    SessionKind::Agent(agent),
                    config.repos_dir.clone(),
                    vec![fleet_core::sessions::TerminalSpec {
                        name: name.to_owned(),
                        command: config.agent_commands.command(agent).to_owned(),
                    }],
                )
            }
            (None, None) => {
                return Err(DaemonError::Validation(
                    "a session requires a worktree, an agent, or both".to_owned(),
                ));
            }
        };
        if specs.is_empty() {
            return Err(DaemonError::Validation(
                "a session requires at least one configured terminal".to_owned(),
            ));
        }
        // Keep the entire existence-check/repair sequence atomic for this session. Different
        // sessions still ensure concurrently, while duplicate requests cannot both reserve and
        // append the same configured terminal name.
        let _ensure_claim = self.runtime.claim_ensure_lock(session_id.clone()).await;
        let configured_order = specs
            .iter()
            .enumerate()
            .map(|(index, spec)| (spec.name.clone(), index))
            .collect::<HashMap<_, _>>();

        let created = {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = registry.sessions.get(&session_id) {
                if existing.kind != kind {
                    return Err(DaemonError::Conflict(format!(
                        "session `{session_id}` belongs to a different workload"
                    )));
                }
                false
            } else {
                registry.sessions.insert(
                    session_id.clone(),
                    Session {
                        id: session_id.clone(),
                        kind: kind.clone(),
                        cwd: cwd.clone(),
                        terminals: Vec::new(),
                        active_terminal: None,
                        slept_at: None,
                        kept_terminals: Vec::new(),
                    },
                );
                true
            }
        };

        for spec in specs {
            let missing = self.runtime.session(&session_id).is_some_and(|session| {
                !session
                    .terminals
                    .iter()
                    .any(|entry| entry.name == spec.name)
            });
            if missing
                && let Err(error) = self
                    .new_terminal(session_id.clone(), spec.name, spec.command, cwd.clone())
                    .await
            {
                if created {
                    self.runtime.kill_if_present(&session_id);
                }
                return Err(error);
            }
        }
        {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(session) = registry.sessions.get_mut(&session_id) {
                session.terminals.sort_by_key(|terminal| {
                    configured_order
                        .get(&terminal.name)
                        .copied()
                        .unwrap_or(usize::MAX)
                });
            }
        }

        let previous = if matches!(kind, SessionKind::Worktree(_)) {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = registry.active_worktree.replace(session_id.clone());
            previous.filter(|previous| previous != &session_id)
        } else {
            None
        };
        if sleep_previous
            && let Some(previous) = previous
            && let Some(process) = self.runtime.process()
            && let Err(error) = crate::services::sleep::apply_session(
                &self.runtime,
                &self.config,
                process.as_ref(),
                &previous,
            )
            .await
        {
            tracing::warn!(%error, %previous, "failed to sleep previous session");
        }

        let session = self
            .runtime
            .session(&session_id)
            .ok_or_else(|| DaemonError::NotFound(session_id.to_string()))?;
        self.runtime.notify_session(&session_id);
        Ok(session)
    }

    /// Hard-kills a runtime session and all of its terminals.
    pub async fn kill(&self, session: SessionId) -> DaemonResult<()> {
        let _transition = self
            .runtime
            .claim_terminal_transition(session.clone())
            .await;
        if self.runtime.kill_if_present(&session) {
            Ok(())
        } else if self.is_remote_session(&session).await? {
            Err(remote_unsupported())
        } else {
            Err(DaemonError::NotFound(session.to_string()))
        }
    }

    /// Adds a login-shell terminal and types its configured command.
    ///
    /// A reserved `fleet://` command adds the tab without a PTY: see
    /// [`Sessions::new_native_terminal`].
    pub async fn new_terminal(
        &self,
        session: SessionId,
        name: String,
        command: String,
        cwd: String,
    ) -> DaemonResult<Terminal> {
        validate_terminal_input(&name, &command)?;
        let _transition = self
            .runtime
            .claim_terminal_transition(session.clone())
            .await;
        if is_native_command(&command) {
            return self.new_native_terminal(session, name, command, cwd);
        }
        let terminal_id = {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let parent = registry
                .sessions
                .get(&session)
                .ok_or_else(|| DaemonError::NotFound(session.to_string()))?;
            if parent.terminals.iter().any(|entry| entry.name == name) {
                return Err(DaemonError::Conflict(format!(
                    "terminal name `{name}` already exists in session `{session}`"
                )));
            }
            let id = TerminalId(registry.next_terminal);
            registry.next_terminal = registry.next_terminal.saturating_add(1);
            id
        };

        let (terminal, host) = spawn_terminal(
            terminal_id,
            session.clone(),
            name,
            command,
            cwd,
            self.config.load().await?.terminal.scrollback_bytes,
            1,
        )
        .await?;
        let host = Arc::new(host);
        let forwarder = host_bridge::prepare_host_events(
            Arc::clone(&self.runtime),
            terminal_id,
            Arc::clone(&host),
        )?;
        {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let parent = registry
                .sessions
                .get_mut(&session)
                .ok_or_else(|| DaemonError::NotFound(session.to_string()))?;
            parent.terminals.push(terminal.clone());
            parent.active_terminal.get_or_insert(terminal_id);
            registry
                .terminal_sessions
                .insert(terminal_id, session.clone());
            registry.hosts.insert(terminal_id, host);
            registry.observed_output_bytes.insert(terminal_id, 0);
        }
        forwarder.start()?;
        self.runtime.notify_session(&session);
        Ok(terminal)
    }

    /// Registers a client-drawn tab in the same ordering as PTY tabs, without a host.
    fn new_native_terminal(
        &self,
        session: SessionId,
        name: String,
        command: String,
        cwd: String,
    ) -> DaemonResult<Terminal> {
        validate_terminal_input(&name, &command)?;
        let terminal = {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if registry
                .sessions
                .get(&session)
                .ok_or_else(|| DaemonError::NotFound(session.to_string()))?
                .terminals
                .iter()
                .any(|entry| entry.name == name)
            {
                return Err(DaemonError::Conflict(format!(
                    "terminal name `{name}` already exists in session `{session}`"
                )));
            }
            let id = TerminalId(registry.next_terminal);
            registry.next_terminal = registry.next_terminal.saturating_add(1);
            let terminal = Terminal {
                id,
                name,
                command,
                cwd,
                shell_pid: None,
                foreground_command: None,
                status: TerminalStatus::Running,
                title: None,
                keep_alive: Vec::new(),
                has_unseen_output: false,
                kind: TerminalKind::Native,
            };
            let parent = registry
                .sessions
                .get_mut(&session)
                .ok_or_else(|| DaemonError::NotFound(session.to_string()))?;
            parent.terminals.push(terminal.clone());
            parent.active_terminal.get_or_insert(id);
            registry.terminal_sessions.insert(id, session.clone());
            terminal
        };
        self.runtime.notify_session(&session);
        Ok(terminal)
    }

    /// Closes one terminal without affecting siblings; closing the last removes the session.
    pub async fn close_terminal(&self, terminal: TerminalId) -> DaemonResult<()> {
        self.runtime
            .close_terminal_gated(terminal)
            .await
            .map(|_| ())
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))
    }

    /// Recreates an exited terminal from its retained command and working directory.
    pub async fn restart_terminal(&self, terminal: TerminalId) -> DaemonResult<Terminal> {
        let session_id = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal_sessions
            .get(&terminal)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        let _transition = self
            .runtime
            .claim_terminal_transition(session_id.clone())
            .await;
        let (session_id, old, starting_sequence) = {
            let registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session_id = registry
                .terminal_sessions
                .get(&terminal)
                .cloned()
                .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
            let entry = registry
                .sessions
                .get(&session_id)
                .and_then(|session| session.terminals.iter().find(|entry| entry.id == terminal))
                .cloned()
                .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
            if entry.is_native() {
                return Err(DaemonError::Conflict(format!(
                    "terminal `{terminal}` has no process to restart"
                )));
            }
            if !matches!(entry.status, TerminalStatus::Exited { .. }) {
                return Err(DaemonError::Conflict(format!(
                    "terminal `{terminal}` is still running"
                )));
            }
            (
                session_id,
                entry,
                registry.next_sequences.get(&terminal).copied().unwrap_or(1),
            )
        };
        let (replacement, host) = spawn_terminal(
            terminal,
            session_id.clone(),
            old.name,
            old.command,
            old.cwd,
            self.config.load().await?.terminal.scrollback_bytes,
            starting_sequence,
        )
        .await?;
        let host = Arc::new(host);
        let forwarder = host_bridge::prepare_host_events(
            Arc::clone(&self.runtime),
            terminal,
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
                .get_mut(&session_id)
                .ok_or_else(|| DaemonError::NotFound(session_id.to_string()))?;
            let entry = session
                .terminals
                .iter_mut()
                .find(|entry| entry.id == terminal)
                .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
            *entry = replacement.clone();
            registry.hosts.insert(terminal, host);
            registry.observed_output_bytes.insert(terminal, 0);
        }
        forwarder.start()?;
        self.runtime.notify_session(&session_id);
        Ok(replacement)
    }

    /// Renames one terminal while preserving its process.
    pub async fn rename_terminal(
        &self,
        terminal: TerminalId,
        name: String,
    ) -> DaemonResult<Terminal> {
        if name.trim().is_empty() {
            return Err(DaemonError::Validation(
                "terminal name must not be empty".to_owned(),
            ));
        }
        let owning_session = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal_sessions
            .get(&terminal)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        let _transition = self.runtime.claim_terminal_transition(owning_session).await;
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_id = registry
            .terminal_sessions
            .get(&terminal)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        let session = registry
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| DaemonError::NotFound(session_id.to_string()))?;
        if session
            .terminals
            .iter()
            .any(|entry| entry.id != terminal && entry.name == name)
        {
            return Err(DaemonError::Conflict(format!(
                "terminal name `{name}` already exists in session `{session_id}`"
            )));
        }
        let entry = session
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        entry.name = name;
        let terminal = entry.clone();
        drop(registry);
        self.runtime.notify_session(&session_id);
        Ok(terminal)
    }

    /// Selects the active terminal and clears its unseen-output flag.
    pub async fn select_terminal(
        &self,
        session: SessionId,
        terminal: TerminalId,
    ) -> DaemonResult<Session> {
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = registry
            .sessions
            .get_mut(&session)
            .ok_or_else(|| DaemonError::NotFound(session.to_string()))?;
        let entry = parent
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        entry.has_unseen_output = false;
        parent.active_terminal = Some(terminal);
        let parent = parent.clone();
        drop(registry);
        self.runtime.notify_session(&session);
        Ok(parent)
    }

    async fn is_remote_session(&self, session: &SessionId) -> DaemonResult<bool> {
        let config = self.config.load().await?;
        Ok(config
            .hosts
            .keys()
            .any(|host| session.as_str().starts_with(&format!("{host}/"))))
    }
}
