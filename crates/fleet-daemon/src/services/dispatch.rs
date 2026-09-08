use super::*;

impl Services {
    /// Dispatches one post-handshake protocol operation to its owning service.
    pub async fn dispatch(&self, body: RequestBody) -> DaemonResult<ResponseBody> {
        self.dispatch_owned(body, 0).await
    }

    pub(crate) async fn dispatch_owned(
        &self,
        body: RequestBody,
        owner: u64,
    ) -> DaemonResult<ResponseBody> {
        self.reject_remote_request(&body).await?;
        match body {
            RequestBody::AgentThreadList => self.agent_response(self.agents.list().await),
            RequestBody::AgentThreadCreate {
                worktree,
                provider,
                model,
                mode,
                resume_cursor,
                title,
            } => self.agent_response(
                self.agents
                    .create(worktree, provider, model, mode, resume_cursor, title)
                    .await,
            ),
            RequestBody::AgentThreadOpen { thread, from_seq } => {
                self.agent_response(self.agents.open(thread, from_seq).await)
            }
            RequestBody::AgentThreadClose { thread } => {
                self.agent_response(self.agents.close(thread).await)
            }
            RequestBody::AgentSend { thread, input } => {
                self.agent_response(self.agents.send(thread, input).await)
            }
            RequestBody::AgentInterrupt { thread } => {
                self.agent_response(self.agents.interrupt(thread).await)
            }
            RequestBody::AgentRespond {
                thread,
                gate,
                answer,
            } => self.agent_response(self.agents.respond(thread, gate, answer).await),
            RequestBody::AgentSetMode { thread, mode } => {
                self.agent_response(self.agents.set_mode(thread, mode).await)
            }
            RequestBody::AgentSetModel { thread, model } => {
                self.agent_response(self.agents.set_model(thread, model).await)
            }
            RequestBody::AgentMarkSeen { thread, seq } => {
                self.agent_response(self.agents.mark_seen(thread, seq).await)
            }
            RequestBody::AgentStop { thread } => {
                self.agent_response(self.agents.stop(thread).await)
            }
            RequestBody::ListBoards { context_id } => Ok(ResponseBody::Boards(
                self.boards.list(context_id.as_ref()).await?,
            )),
            RequestBody::GetBoard { board_id } => {
                Ok(ResponseBody::Board(self.boards.get(&board_id).await?))
            }
            RequestBody::EnsureBoard { context_id } => {
                Ok(ResponseBody::Board(self.boards.ensure(&context_id).await?))
            }
            RequestBody::CreateBoard {
                context_id,
                name,
                prefix,
                backend,
            } => Ok(ResponseBody::Board(
                self.boards
                    .create(&context_id, name, prefix, backend)
                    .await?,
            )),
            RequestBody::UpdateBoard { board_id, patch } => Ok(ResponseBody::Board(
                self.boards.update(&board_id, patch).await?,
            )),
            RequestBody::DeleteBoard { board_id } => {
                self.boards.delete(&board_id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::CreateCard { board_id, draft } => Ok(ResponseBody::Card(
                self.boards.create_card(&board_id, draft).await?,
            )),
            RequestBody::UpdateCard { card_id, patch } => Ok(ResponseBody::Card(
                self.boards.update_card(&card_id, patch).await?,
            )),
            RequestBody::MoveCard {
                card_id,
                status_id,
                index,
            } => Ok(ResponseBody::Card(
                self.boards.move_card(&card_id, &status_id, index).await?,
            )),
            RequestBody::DeleteCard { card_id } => {
                self.boards.delete_card(&card_id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::AddCardComment { card_id, body } => Ok(ResponseBody::Card(
                self.boards.add_comment(&card_id, body).await?,
            )),
            RequestBody::CreateWorktreeFromCard {
                card_id,
                repo_id,
                base,
                host,
            } => {
                let (card, worktree, created) = self
                    .boards
                    .create_worktree_from_card(&card_id, repo_id, base, host)
                    .await?;
                Ok(ResponseBody::CardWorktree {
                    card,
                    worktree,
                    created,
                })
            }
            RequestBody::SyncBoard { board_id, full } => {
                let job_id = self.boards.sync(&board_id, full).await?;
                let job = self
                    .jobs
                    .record(&job_id)
                    .ok_or_else(|| DaemonError::NotFound(format!("job {job_id}")))?;
                Ok(ResponseBody::Job(job))
            }
            RequestBody::ResolveCardConflict {
                card_id,
                resolution,
            } => Ok(ResponseBody::Card(
                self.boards.resolve_conflict(&card_id, resolution).await?,
            )),
            RequestBody::DescribeBoardBackend { board_id } => Ok(ResponseBody::BoardBackendSchema(
                self.boards.describe_backend(&board_id).await?,
            )),
            RequestBody::ListBoardBackends {} => {
                Ok(ResponseBody::BoardBackends(self.boards.list_backends()))
            }
            body @ RequestBody::StartWatch { .. } => Ok(ResponseBody::WatchStarted(
                self.sessions.start_watch(owner, body)?,
            )),
            RequestBody::AppendWatchOutput {
                watch,
                stream,
                text,
            } => {
                self.watches.append_owned(watch, owner, stream, text)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::FinishWatch {
                watch,
                code,
                signal,
            } => {
                self.watches.finish_owned(watch, owner, code, signal)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ListWatches { session } => {
                Ok(ResponseBody::Watches(self.watches.list(&session)))
            }
            RequestBody::TailWatch { watch, from_seq } => {
                Ok(ResponseBody::WatchTail(self.watches.tail(watch, from_seq)?))
            }
            RequestBody::DismissWatch { watch } => {
                self.watches.dismiss(watch)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::Hello { .. }
            | RequestBody::Subscribe { .. }
            | RequestBody::Unsubscribe => Err(DaemonError::Protocol(
                "connection-local request reached service facade".to_owned(),
            )),
            RequestBody::GetSnapshot => Ok(ResponseBody::Snapshot(self.snapshot().await?)),
            RequestBody::CreateContext { name, owners } => Ok(ResponseBody::Context(
                self.contexts.create(name, owners).await?,
            )),
            RequestBody::UpdateContext { id, name, owners } => Ok(ResponseBody::Context(
                self.contexts.update(id, name, owners).await?,
            )),
            RequestBody::DeleteContext { id } => {
                self.delete_context_cascade(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::SetActiveContext { id } => {
                self.contexts.set_active(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::CloneRepo {
                owner,
                name,
                url,
                context,
                default_branch,
            } => Ok(ResponseBody::CloneStarted(
                self.repos
                    .clone_repo(owner, name, url, context, default_branch)
                    .await?,
            )),
            RequestBody::DeleteRepo { repo } => {
                self.delete_repo_cascade(repo).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::MoveRepoToContext { repo, context } => Ok(ResponseBody::Repo(
                self.repos.move_to_context(repo, context).await?,
            )),
            RequestBody::SearchRemoteRepos { owner, query } => Ok(ResponseBody::RemoteRepos(
                self.repos.search_remote(owner, query).await?,
            )),
            RequestBody::ListRemoteRepos { owner, force } => Ok(ResponseBody::RemoteRepos(
                self.repos.list_remote(owner, force).await?,
            )),
            RequestBody::ListBaseRefs { repo, force } => Ok(ResponseBody::BaseRefs(
                self.repos.list_base_refs(repo, force).await?,
            )),
            RequestBody::SetRepoHooks { repo, hooks } => {
                Ok(ResponseBody::Repo(self.repos.set_hooks(repo, hooks).await?))
            }
            RequestBody::DismissClone { repo } => {
                self.repos.dismiss_clone(repo).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::CreateWorktree { host: Some(_), .. } => Err(remote_unsupported()),
            RequestBody::CreateWorktree {
                repo,
                slug,
                branch,
                base,
                host: None,
                hooks,
            } => {
                let (created, worktree, post_create_job) = self
                    .worktrees
                    .create(repo, slug, branch, base, hooks)
                    .await?;
                Ok(ResponseBody::Worktree {
                    created,
                    worktree,
                    post_create_job: post_create_job.map(Box::new),
                })
            }
            RequestBody::DeleteWorktrees { ids } => Ok(ResponseBody::WorktreesDeleted(
                self.worktrees.delete(ids).await?,
            )),
            RequestBody::InspectWorktrees { ids, repo, fetch } => Ok(ResponseBody::Inspections(
                self.inspect.worktrees(ids, repo, fetch).await?,
            )),
            RequestBody::PruneWorktrees {
                dry_run,
                fetch,
                kill_sessions,
                repo,
                ids,
            } => Ok(ResponseBody::Pruned(
                self.prune
                    .worktrees(dry_run, fetch, kill_sessions, repo, ids)
                    .await?,
            )),
            RequestBody::KillWorktree { id } => {
                self.worktrees.kill(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::SleepWorktree { id } => {
                Ok(ResponseBody::Slept(self.sleep.worktree(id).await?))
            }
            RequestBody::TouchWorktreeOpened { id } => {
                self.worktrees.touch_opened(id).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::WorktreePath { id } => {
                Ok(ResponseBody::Path(self.worktrees.path(id).await?))
            }
            RequestBody::RestoreTrash { entry } => {
                self.worktrees.restore_trash(entry).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::RefreshStatuses { repo } => {
                let statuses = self.sessions.refresh_statuses(repo).await?;
                Ok(ResponseBody::Statuses(statuses))
            }
            RequestBody::SetAgentActivity {
                session,
                terminal_id,
                activity,
                attention,
            } => {
                let transition = self.sessions.set_agent_activity(
                    &session,
                    terminal_id,
                    activity,
                    attention,
                    std::time::Instant::now(),
                )?;
                self.apply_agent_activity_transitions(transition.into_iter().collect())
                    .await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ListPullRequests {
                repo,
                context,
                tab,
                force,
            } => Ok(ResponseBody::PullRequests(
                self.github
                    .list_pull_requests(repo, context, tab, force)
                    .await?,
            )),
            RequestBody::CreateWorktreeFromPr { repo, number } => {
                let (created, worktree, post_create_job) =
                    self.worktrees.create_from_pr(repo, number).await?;
                Ok(ResponseBody::Worktree {
                    created,
                    worktree,
                    post_create_job: post_create_job.map(Box::new),
                })
            }
            RequestBody::EnsureSession {
                worktree,
                agent,
                sleep_previous,
            } => Ok(ResponseBody::Session(
                self.sessions
                    .ensure(worktree, agent, sleep_previous)
                    .await?,
            )),
            RequestBody::ListSessions => Ok(ResponseBody::Sessions(self.sessions.snapshot())),
            RequestBody::CurrentSession => {
                Ok(ResponseBody::CurrentSession(self.sessions.current()))
            }
            RequestBody::KillSession { session } => {
                self.sessions.kill(session).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::SleepSession { session } => {
                Ok(ResponseBody::Slept(self.sleep.session(session).await?))
            }
            RequestBody::NewTerminal {
                session,
                name,
                command,
                cwd,
            } => Ok(ResponseBody::Terminal(
                self.sessions
                    .new_terminal(session, name, command, cwd)
                    .await?,
            )),
            RequestBody::CloseTerminal { terminal } => {
                self.sessions.close_terminal(terminal).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::RestartTerminal { terminal } => Ok(ResponseBody::Terminal(
                self.sessions.restart_terminal(terminal).await?,
            )),
            RequestBody::RenameTerminal { terminal, name } => Ok(ResponseBody::Terminal(
                self.sessions.rename_terminal(terminal, name).await?,
            )),
            RequestBody::SelectTerminal { session, terminal } => Ok(ResponseBody::Session(
                self.sessions.select_terminal(session, terminal).await?,
            )),
            RequestBody::AttachTerminal {
                terminal,
                cols,
                rows,
            } => {
                self.sessions.attach(terminal, cols, rows).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::DetachTerminal { terminal } => {
                self.sessions.detach(terminal).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::TerminalInput { terminal, bytes } => {
                self.sessions.input(terminal, bytes).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::TerminalKey { terminal, key } => {
                self.sessions.key(terminal, key).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::TerminalMouse { terminal, mouse } => {
                self.sessions.mouse(terminal, mouse).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ResizeTerminal {
                terminal,
                cols,
                rows,
            } => {
                self.sessions.resize(terminal, cols, rows).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ScrollTerminal { terminal, scroll } => {
                self.sessions.scroll(terminal, scroll).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::WheelTerminal { terminal, wheel } => {
                self.sessions.wheel(terminal, wheel).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ScrollOrKeyTerminal {
                terminal,
                scroll,
                key,
            } => {
                self.sessions.scroll_or_key(terminal, scroll, key).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::RequestFullFrame { terminal } => {
                self.sessions.request_full_frame(terminal).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::PasteTerminal { terminal, text } => {
                self.sessions.paste(terminal, text).await?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::ListJobs => Ok(ResponseBody::Jobs(self.jobs.list())),
            RequestBody::CancelJob { job } => {
                self.jobs.cancel(&job)?;
                Ok(ResponseBody::JobCancelled(job))
            }
            RequestBody::RetryJob { job } => Ok(ResponseBody::Job(self.jobs.retry(&job)?)),
            RequestBody::TailJob { job, lines } => {
                Ok(ResponseBody::JobLog(self.jobs.tail(&job, lines).await?))
            }
            RequestBody::DismissJobs { jobs } => {
                self.jobs.dismiss(&jobs)?;
                Ok(ResponseBody::Ack)
            }
            RequestBody::GetConfig => Ok(ResponseBody::Config(self.config.load().await?)),
            RequestBody::SetConfig { patch } => {
                let config = self.config.update(patch).await?;
                self.reconcile_runtime_config(&config);
                Ok(ResponseBody::Config(config))
            }
            RequestBody::MatchKeepAliveRules => Ok(ResponseBody::KeepAliveRuleMatches(
                self.sleep.match_keep_alive_rules().await?,
            )),
            RequestBody::ImportFromSwarm => Ok(ResponseBody::Job(self.import.start().await?)),
            RequestBody::Doctor => Ok(ResponseBody::Doctor(self.doctor.check().await?)),
            RequestBody::ResetState => Ok(ResponseBody::Path(
                self.state.reset_quarantined().await?.display().to_string(),
            )),
            RequestBody::Update => Ok(ResponseBody::Job(self.update.start().await?)),
            RequestBody::DaemonPing => Ok(ResponseBody::Pong),
            RequestBody::DaemonVersion => Ok(ResponseBody::Version {
                version: Self::version(),
                protocol: fleet_proto::PROTOCOL_VERSION,
            }),
            RequestBody::DaemonShutdown { .. } => Ok(ResponseBody::ShuttingDown),
        }
    }

    fn agent_response(
        &self,
        result: Result<ResponseBody, fleet_proto::error::ProtoError>,
    ) -> DaemonResult<ResponseBody> {
        result.map_err(|error| match error.kind {
            fleet_proto::error::ErrorKind::NotFound => DaemonError::NotFound(error.message),
            fleet_proto::error::ErrorKind::Conflict => DaemonError::Conflict(error.message),
            fleet_proto::error::ErrorKind::Validation => DaemonError::Validation(error.message),
            fleet_proto::error::ErrorKind::Cancelled => DaemonError::Cancelled,
            fleet_proto::error::ErrorKind::Unsupported => DaemonError::Unsupported(error.message),
            fleet_proto::error::ErrorKind::Git => DaemonError::Git(error.message),
            fleet_proto::error::ErrorKind::Github => DaemonError::Github(error.message),
            fleet_proto::error::ErrorKind::Fs => DaemonError::fs(
                self.home.join("agents"),
                std::io::Error::other(error.message),
            ),
            fleet_proto::error::ErrorKind::Tmux
            | fleet_proto::error::ErrorKind::Remote
            | fleet_proto::error::ErrorKind::Unknown => DaemonError::Protocol(error.message),
        })
    }

    pub(super) async fn delete_repo_cascade(&self, repo: RepoId) -> DaemonResult<()> {
        let _deleting = self.jobs.begin_repo_deletion(&repo)?;
        self.jobs.quiesce_repo(&repo).await?;
        let ids = self
            .state
            .load()
            .await?
            .worktrees
            .into_iter()
            .filter(|worktree| worktree.repo_id == repo)
            .map(|worktree| worktree.id)
            .collect::<Vec<_>>();
        let failures = self
            .worktrees
            .delete_guarded(ids)
            .await?
            .into_iter()
            .filter(|result| !result.ok)
            .map(|result| {
                result
                    .reason
                    .unwrap_or_else(|| format!("could not delete {}", result.worktree_id))
            })
            .collect::<Vec<_>>();
        if !failures.is_empty() {
            return Err(DaemonError::Conflict(failures.join("; ")));
        }
        self.repos.delete_guarded(repo).await
    }

    async fn delete_context_cascade(
        &self,
        context: fleet_core::ids::ContextId,
    ) -> DaemonResult<()> {
        let context_lifecycle = self.repos.context_lifecycle();
        let _context_lifecycle = context_lifecycle.lock().await;
        loop {
            let state = self.state.load().await?;
            if !state.contexts.iter().any(|entry| entry.id == context) {
                return Err(DaemonError::NotFound(format!("context {context}")));
            }
            let mut repositories = state
                .repos
                .iter()
                .filter(|repo| repo.context_id == context)
                .map(|repo| repo.id.clone())
                .chain(
                    state
                        .clones
                        .iter()
                        .filter(|clone| clone.context_id == context)
                        .map(|clone| clone.id.clone()),
                )
                .collect::<Vec<_>>();
            repositories.sort();
            repositories.dedup();
            if repositories.is_empty() {
                // The board must go with its context: a stranded document would be adopted by
                // the next context whose name derives the same id, resurrecting deleted cards.
                self.boards.delete_for_context(&context).await?;
                match self.contexts.delete(context.clone()).await {
                    Ok(()) => return Ok(()),
                    Err(DaemonError::Conflict(message))
                        if message == format!("context {context} still owns repositories") => {}
                    Err(error) => return Err(error),
                }
            }
            for repository in repositories {
                let current = self.state.load().await?;
                let still_owned = current
                    .repos
                    .iter()
                    .any(|repo| repo.id == repository && repo.context_id == context)
                    || current
                        .clones
                        .iter()
                        .any(|clone| clone.id == repository && clone.context_id == context);
                if still_owned {
                    self.delete_repo_cascade(repository).await?;
                }
            }
        }
    }

    pub(super) async fn reject_remote_request(&self, body: &RequestBody) -> DaemonResult<()> {
        let needs_state = matches!(
            body,
            RequestBody::DeleteContext { .. }
                | RequestBody::DeleteRepo { .. }
                | RequestBody::DeleteWorktrees { .. }
                | RequestBody::InspectWorktrees { .. }
                | RequestBody::PruneWorktrees { .. }
                | RequestBody::KillWorktree { .. }
                | RequestBody::SleepWorktree { .. }
                | RequestBody::WorktreePath { .. }
                | RequestBody::RefreshStatuses { .. }
                | RequestBody::EnsureSession {
                    worktree: Some(_),
                    ..
                }
        );
        if !needs_state {
            return Ok(());
        }
        let state = self.state.load().await?;
        let proxies_remote = match body {
            RequestBody::DeleteContext { id } => state.worktrees.iter().any(|worktree| {
                worktree.host.is_some()
                    && state
                        .repos
                        .iter()
                        .any(|repo| repo.id == worktree.repo_id && &repo.context_id == id)
            }),
            RequestBody::DeleteRepo { repo } => state
                .worktrees
                .iter()
                .any(|worktree| worktree.host.is_some() && &worktree.repo_id == repo),
            RequestBody::DeleteWorktrees { ids } => state
                .worktrees
                .iter()
                .any(|worktree| worktree.host.is_some() && ids.contains(&worktree.id)),
            RequestBody::InspectWorktrees { ids, repo, .. } => {
                state.worktrees.iter().any(|worktree| {
                    worktree.host.is_some()
                        && (ids.is_empty() || ids.contains(&worktree.id))
                        && repo.as_ref().is_none_or(|repo| &worktree.repo_id == repo)
                })
            }
            RequestBody::PruneWorktrees { repo, .. } | RequestBody::RefreshStatuses { repo } => {
                state.worktrees.iter().any(|worktree| {
                    worktree.host.is_some()
                        && repo.as_ref().is_none_or(|repo| &worktree.repo_id == repo)
                })
            }
            RequestBody::KillWorktree { id }
            | RequestBody::SleepWorktree { id }
            | RequestBody::WorktreePath { id }
            | RequestBody::EnsureSession {
                worktree: Some(id), ..
            } => state
                .worktrees
                .iter()
                .any(|worktree| worktree.host.is_some() && &worktree.id == id),
            _ => false,
        };
        if proxies_remote {
            Err(remote_unsupported())
        } else {
            Ok(())
        }
    }
}
