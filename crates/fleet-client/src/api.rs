//! Typed high-level request and response operations.

use fleet_core::{
    cache::RepoCache,
    config::{Agent, Config},
    github::PrTab,
    ids::{ContextId, HostId, JobId, RepoId, SessionId, TerminalId, WorktreeId},
    inspection::WorktreeInspection,
    model::{Context, Repo, RepoHooks, Worktree},
    sessions::{Session, Terminal, WorktreeStatus},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::EventKind,
    job::JobRecord,
    request::RequestBody,
    response::{
        BaseRefs, DoctorCheck, KeepAliveRuleMatch, PrSlice, PruneResult, ResponseBody, SleepResult,
        WorktreeDeleteResult,
    },
    snapshot::Snapshot,
    terminal::{KeyEvent, MouseEvent, ScrollCommand},
};
use serde::{Deserialize, Serialize};

use crate::connection::Client;

/// The result type returned by typed Fleet client operations.
pub type Result<T> = std::result::Result<T, ProtoError>;

/// Details returned by protocol negotiation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    /// Negotiated protocol version.
    pub protocol: u32,
    /// Daemon build identifier.
    pub server: String,
}

/// Result of creating or idempotently finding a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorktreeResult {
    /// Whether the request created a new worktree.
    pub created: bool,
    /// The resulting worktree.
    pub worktree: Worktree,
    /// Post-create hook job, when hooks were scheduled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_create_job: Option<JobRecord>,
}

/// Daemon version information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonVersion {
    /// Daemon build version.
    pub version: String,
    /// Supported wire protocol version.
    pub protocol: u32,
}

impl Client {
    /// Renegotiates the protocol and returns daemon identity information.
    pub async fn hello(&self, client: impl Into<String>) -> Result<HelloResult> {
        match self
            .request(RequestBody::Hello {
                protocol: 1,
                client: client.into(),
            })
            .await?
        {
            ResponseBody::Hello { protocol, server } => Ok(HelloResult { protocol, server }),
            response => Err(unexpected("hello", response)),
        }
    }

    /// Fetches a complete daemon snapshot.
    pub async fn get_snapshot(&self) -> Result<Snapshot> {
        match self.request(RequestBody::GetSnapshot).await? {
            ResponseBody::Snapshot(snapshot) => Ok(snapshot),
            response => Err(unexpected("get_snapshot", response)),
        }
    }

    /// Replaces this connection's event subscriptions.
    pub async fn subscribe(&self, events: Vec<EventKind>) -> Result<()> {
        expect_ack(
            "subscribe",
            self.request(RequestBody::Subscribe { events }).await?,
        )
    }

    /// Removes all event subscriptions from this connection.
    pub async fn unsubscribe(&self) -> Result<()> {
        expect_ack("unsubscribe", self.request(RequestBody::Unsubscribe).await?)
    }

    /// Creates a context.
    pub async fn create_context(
        &self,
        name: impl Into<String>,
        owners: Vec<String>,
    ) -> Result<Context> {
        match self
            .request(RequestBody::CreateContext {
                name: name.into(),
                owners,
            })
            .await?
        {
            ResponseBody::Context(context) => Ok(context),
            response => Err(unexpected("create_context", response)),
        }
    }

    /// Updates a context's display fields.
    pub async fn update_context(
        &self,
        id: ContextId,
        name: Option<String>,
        owners: Option<Vec<String>>,
    ) -> Result<Context> {
        match self
            .request(RequestBody::UpdateContext { id, name, owners })
            .await?
        {
            ResponseBody::Context(context) => Ok(context),
            response => Err(unexpected("update_context", response)),
        }
    }

    /// Deletes a context and its descendants.
    pub async fn delete_context(&self, id: ContextId) -> Result<()> {
        expect_ack(
            "delete_context",
            self.request(RequestBody::DeleteContext { id }).await?,
        )
    }

    /// Changes or clears the active context.
    pub async fn set_active_context(&self, id: Option<ContextId>) -> Result<()> {
        expect_ack(
            "set_active_context",
            self.request(RequestBody::SetActiveContext { id }).await?,
        )
    }

    /// Starts cloning and registering a repository.
    pub async fn clone_repo(
        &self,
        owner: impl Into<String>,
        name: impl Into<String>,
        url: impl Into<String>,
        context: ContextId,
        default_branch: Option<String>,
    ) -> Result<JobRecord> {
        match self
            .request(RequestBody::CloneRepo {
                owner: owner.into(),
                name: name.into(),
                url: url.into(),
                context,
                default_branch,
            })
            .await?
        {
            ResponseBody::CloneStarted(job) => Ok(job),
            response => Err(unexpected("clone_repo", response)),
        }
    }

    /// Deletes a repository and all of its worktrees.
    pub async fn delete_repo(&self, repo: RepoId) -> Result<()> {
        expect_ack(
            "delete_repo",
            self.request(RequestBody::DeleteRepo { repo }).await?,
        )
    }

    /// Assigns a repository to another context.
    pub async fn move_repo_to_context(&self, repo: RepoId, context: ContextId) -> Result<Repo> {
        match self
            .request(RequestBody::MoveRepoToContext { repo, context })
            .await?
        {
            ResponseBody::Repo(repo) => Ok(repo),
            response => Err(unexpected("move_repo_to_context", response)),
        }
    }

    /// Searches one owner's remote repositories.
    pub async fn search_remote_repos(
        &self,
        owner: impl Into<String>,
        query: impl Into<String>,
    ) -> Result<RepoCache> {
        match self
            .request(RequestBody::SearchRemoteRepos {
                owner: owner.into(),
                query: query.into(),
            })
            .await?
        {
            ResponseBody::RemoteRepos(repos) => Ok(repos),
            response => Err(unexpected("search_remote_repos", response)),
        }
    }

    /// Lists one owner's remote repositories.
    pub async fn list_remote_repos(
        &self,
        owner: impl Into<String>,
        force: bool,
    ) -> Result<RepoCache> {
        match self
            .request(RequestBody::ListRemoteRepos {
                owner: owner.into(),
                force,
            })
            .await?
        {
            ResponseBody::RemoteRepos(repos) => Ok(repos),
            response => Err(unexpected("list_remote_repos", response)),
        }
    }

    /// Lists cached or freshly fetched base refs for worktree creation.
    pub async fn list_base_refs(&self, repo: RepoId, force: bool) -> Result<BaseRefs> {
        match self
            .request(RequestBody::ListBaseRefs { repo, force })
            .await?
        {
            ResponseBody::BaseRefs(refs) => Ok(refs),
            response => Err(unexpected("list_base_refs", response)),
        }
    }

    /// Replaces a repository's prepare and post-create hooks.
    pub async fn set_repo_hooks(&self, repo: RepoId, hooks: RepoHooks) -> Result<Repo> {
        match self
            .request(RequestBody::SetRepoHooks { repo, hooks })
            .await?
        {
            ResponseBody::Repo(repo) => Ok(repo),
            response => Err(unexpected("set_repo_hooks", response)),
        }
    }

    /// Dismisses a retained failed clone row.
    pub async fn dismiss_clone(&self, repo: RepoId) -> Result<()> {
        expect_ack(
            "dismiss_clone",
            self.request(RequestBody::DismissClone { repo }).await?,
        )
    }

    /// Creates or idempotently returns a worktree.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_worktree(
        &self,
        repo: RepoId,
        slug: impl Into<String>,
        branch: Option<String>,
        base: Option<String>,
        host: Option<HostId>,
        hooks: RepoHooks,
    ) -> Result<CreateWorktreeResult> {
        let response = self
            .request(RequestBody::CreateWorktree {
                repo,
                slug: slug.into(),
                branch,
                base,
                host,
                hooks,
            })
            .await?;
        worktree_result("create_worktree", response)
    }

    /// Deletes worktrees while preserving individual outcomes.
    pub async fn delete_worktrees(
        &self,
        ids: Vec<WorktreeId>,
    ) -> Result<Vec<WorktreeDeleteResult>> {
        match self.request(RequestBody::DeleteWorktrees { ids }).await? {
            ResponseBody::WorktreesDeleted(results) => Ok(results),
            response => Err(unexpected("delete_worktrees", response)),
        }
    }

    /// Inspects selected or repository-filtered worktrees.
    pub async fn inspect_worktrees(
        &self,
        ids: Vec<WorktreeId>,
        repo: Option<RepoId>,
        fetch: bool,
    ) -> Result<Vec<WorktreeInspection>> {
        match self
            .request(RequestBody::InspectWorktrees { ids, repo, fetch })
            .await?
        {
            ResponseBody::Inspections(inspections) => Ok(inspections),
            response => Err(unexpected("inspect_worktrees", response)),
        }
    }

    /// Safely prunes merged worktrees.
    pub async fn prune_worktrees(
        &self,
        dry_run: bool,
        fetch: bool,
        kill_sessions: bool,
        repo: Option<RepoId>,
    ) -> Result<PruneResult> {
        match self
            .request(RequestBody::PruneWorktrees {
                dry_run,
                fetch,
                kill_sessions,
                repo,
            })
            .await?
        {
            ResponseBody::Pruned(result) => Ok(result),
            response => Err(unexpected("prune_worktrees", response)),
        }
    }

    /// Hard-kills a worktree session.
    pub async fn kill_worktree(&self, id: WorktreeId) -> Result<()> {
        expect_ack(
            "kill_worktree",
            self.request(RequestBody::KillWorktree { id }).await?,
        )
    }

    /// Applies sleep policy to a worktree session.
    pub async fn sleep_worktree(&self, id: WorktreeId) -> Result<SleepResult> {
        match self.request(RequestBody::SleepWorktree { id }).await? {
            ResponseBody::Slept(result) => Ok(result),
            response => Err(unexpected("sleep_worktree", response)),
        }
    }

    /// Records that a worktree was opened.
    pub async fn touch_worktree_opened(&self, id: WorktreeId) -> Result<()> {
        expect_ack(
            "touch_worktree_opened",
            self.request(RequestBody::TouchWorktreeOpened { id })
                .await?,
        )
    }

    /// Resolves a local worktree's absolute path.
    pub async fn worktree_path(&self, id: WorktreeId) -> Result<String> {
        match self.request(RequestBody::WorktreePath { id }).await? {
            ResponseBody::Path(path) => Ok(path),
            response => Err(unexpected("worktree_path", response)),
        }
    }

    /// Restores one recoverable trash entry.
    pub async fn restore_trash(&self, entry: impl Into<String>) -> Result<()> {
        expect_ack(
            "restore_trash",
            self.request(RequestBody::RestoreTrash {
                entry: entry.into(),
            })
            .await?,
        )
    }

    /// Refreshes runtime status for one repository or the complete fleet.
    pub async fn refresh_statuses(&self, repo: Option<RepoId>) -> Result<Vec<WorktreeStatus>> {
        match self.request(RequestBody::RefreshStatuses { repo }).await? {
            ResponseBody::Statuses(statuses) => Ok(statuses),
            response => Err(unexpected("refresh_statuses", response)),
        }
    }

    /// Lists pull requests for a repository or context.
    pub async fn list_pull_requests(
        &self,
        repo: Option<RepoId>,
        context: Option<ContextId>,
        tab: PrTab,
        force: bool,
    ) -> Result<Vec<PrSlice>> {
        match self
            .request(RequestBody::ListPullRequests {
                repo,
                context,
                tab,
                force,
            })
            .await?
        {
            ResponseBody::PullRequests(requests) => Ok(requests),
            response => Err(unexpected("list_pull_requests", response)),
        }
    }

    /// Creates a worktree at a pull request head.
    pub async fn create_worktree_from_pr(
        &self,
        repo: RepoId,
        number: u64,
    ) -> Result<CreateWorktreeResult> {
        let response = self
            .request(RequestBody::CreateWorktreeFromPr { repo, number })
            .await?;
        worktree_result("create_worktree_from_pr", response)
    }

    /// Ensures a worktree or agent session exists.
    pub async fn ensure_session(
        &self,
        worktree: Option<WorktreeId>,
        agent: Option<Agent>,
        sleep_previous: bool,
    ) -> Result<Session> {
        match self
            .request(RequestBody::EnsureSession {
                worktree,
                agent,
                sleep_previous,
            })
            .await?
        {
            ResponseBody::Session(session) => Ok(session),
            response => Err(unexpected("ensure_session", response)),
        }
    }

    /// Lists every daemon-owned session.
    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        match self.request(RequestBody::ListSessions).await? {
            ResponseBody::Sessions(sessions) => Ok(sessions),
            response => Err(unexpected("list_sessions", response)),
        }
    }

    /// Returns the daemon's active worktree session, when one is selected.
    pub async fn current_session(&self) -> Result<Option<SessionId>> {
        match self.request(RequestBody::CurrentSession).await? {
            ResponseBody::CurrentSession(session) => Ok(session),
            response => Err(unexpected("current_session", response)),
        }
    }

    /// Hard-kills a session.
    pub async fn kill_session(&self, session: SessionId) -> Result<()> {
        expect_ack(
            "kill_session",
            self.request(RequestBody::KillSession { session }).await?,
        )
    }

    /// Applies sleep policy to a session.
    pub async fn sleep_session(&self, session: SessionId) -> Result<SleepResult> {
        match self.request(RequestBody::SleepSession { session }).await? {
            ResponseBody::Slept(result) => Ok(result),
            response => Err(unexpected("sleep_session", response)),
        }
    }

    /// Adds a terminal to a session.
    pub async fn new_terminal(
        &self,
        session: SessionId,
        name: impl Into<String>,
        command: impl Into<String>,
        cwd: impl Into<String>,
    ) -> Result<Terminal> {
        match self
            .request(RequestBody::NewTerminal {
                session,
                name: name.into(),
                command: command.into(),
                cwd: cwd.into(),
            })
            .await?
        {
            ResponseBody::Terminal(terminal) => Ok(terminal),
            response => Err(unexpected("new_terminal", response)),
        }
    }

    /// Closes a terminal.
    pub async fn close_terminal(&self, terminal: TerminalId) -> Result<()> {
        expect_ack(
            "close_terminal",
            self.request(RequestBody::CloseTerminal { terminal })
                .await?,
        )
    }

    /// Restarts an exited terminal and returns its updated metadata.
    pub async fn restart_terminal(&self, terminal: TerminalId) -> Result<Terminal> {
        match self
            .request(RequestBody::RestartTerminal { terminal })
            .await?
        {
            ResponseBody::Terminal(terminal) => Ok(terminal),
            response => Err(unexpected("restart_terminal", response)),
        }
    }

    /// Renames a terminal and returns its updated metadata.
    pub async fn rename_terminal(
        &self,
        terminal: TerminalId,
        name: impl Into<String>,
    ) -> Result<Terminal> {
        match self
            .request(RequestBody::RenameTerminal {
                terminal,
                name: name.into(),
            })
            .await?
        {
            ResponseBody::Terminal(terminal) => Ok(terminal),
            response => Err(unexpected("rename_terminal", response)),
        }
    }

    /// Selects a session's active terminal and returns the updated session.
    pub async fn select_terminal(
        &self,
        session: SessionId,
        terminal: TerminalId,
    ) -> Result<Session> {
        match self
            .request(RequestBody::SelectTerminal { session, terminal })
            .await?
        {
            ResponseBody::Session(session) => Ok(session),
            response => Err(unexpected("select_terminal", response)),
        }
    }

    /// Attaches this connection to a terminal.
    pub async fn attach_terminal(&self, terminal: TerminalId, cols: u16, rows: u16) -> Result<()> {
        expect_ack(
            "attach_terminal",
            self.request(RequestBody::AttachTerminal {
                terminal,
                cols,
                rows,
            })
            .await?,
        )
    }

    /// Detaches this connection from a terminal.
    pub async fn detach_terminal(&self, terminal: TerminalId) -> Result<()> {
        expect_ack(
            "detach_terminal",
            self.request(RequestBody::DetachTerminal { terminal })
                .await?,
        )
    }

    /// Writes already encoded bytes to a terminal PTY.
    pub async fn terminal_input(&self, terminal: TerminalId, bytes: Vec<u8>) -> Result<()> {
        expect_ack(
            "terminal_input",
            self.request(RequestBody::TerminalInput { terminal, bytes })
                .await?,
        )
    }

    /// Sends a semantic key event to a terminal.
    pub async fn terminal_key(&self, terminal: TerminalId, key: KeyEvent) -> Result<()> {
        expect_ack(
            "terminal_key",
            self.request(RequestBody::TerminalKey { terminal, key })
                .await?,
        )
    }

    /// Sends a semantic mouse event to a terminal.
    pub async fn terminal_mouse(&self, terminal: TerminalId, mouse: MouseEvent) -> Result<()> {
        expect_ack(
            "terminal_mouse",
            self.request(RequestBody::TerminalMouse { terminal, mouse })
                .await?,
        )
    }

    /// Resizes a terminal PTY.
    pub async fn resize_terminal(&self, terminal: TerminalId, cols: u16, rows: u16) -> Result<()> {
        expect_ack(
            "resize_terminal",
            self.request(RequestBody::ResizeTerminal {
                terminal,
                cols,
                rows,
            })
            .await?,
        )
    }

    /// Moves a terminal's server-side scrollback viewport.
    pub async fn scroll_terminal(&self, terminal: TerminalId, scroll: ScrollCommand) -> Result<()> {
        expect_ack(
            "scroll_terminal",
            self.request(RequestBody::ScrollTerminal { terminal, scroll })
                .await?,
        )
    }

    /// Requests a complete frame for a terminal.
    pub async fn request_full_frame(&self, terminal: TerminalId) -> Result<()> {
        expect_ack(
            "request_full_frame",
            self.request(RequestBody::RequestFullFrame { terminal })
                .await?,
        )
    }

    /// Pastes text with mode-aware bracketed-paste handling.
    pub async fn paste_terminal(
        &self,
        terminal: TerminalId,
        text: impl Into<String>,
    ) -> Result<()> {
        expect_ack(
            "paste_terminal",
            self.request(RequestBody::PasteTerminal {
                terminal,
                text: text.into(),
            })
            .await?,
        )
    }

    /// Lists current and recently completed jobs.
    pub async fn list_jobs(&self) -> Result<Vec<JobRecord>> {
        match self.request(RequestBody::ListJobs).await? {
            ResponseBody::Jobs(jobs) => Ok(jobs),
            response => Err(unexpected("list_jobs", response)),
        }
    }

    /// Cancels a cancellable job.
    pub async fn cancel_job(&self, job: JobId) -> Result<JobId> {
        match self.request(RequestBody::CancelJob { job }).await? {
            ResponseBody::JobCancelled(job) => Ok(job),
            response => Err(unexpected("cancel_job", response)),
        }
    }

    /// Retries a retained restartable job.
    pub async fn retry_job(&self, job: JobId) -> Result<JobRecord> {
        match self.request(RequestBody::RetryJob { job }).await? {
            ResponseBody::Job(job) => Ok(job),
            response => Err(unexpected("retry_job", response)),
        }
    }

    /// Reads trailing lines from a job log.
    pub async fn tail_job(&self, job: JobId, lines: usize) -> Result<Vec<String>> {
        match self.request(RequestBody::TailJob { job, lines }).await? {
            ResponseBody::JobLog(lines) => Ok(lines),
            response => Err(unexpected("tail_job", response)),
        }
    }

    /// Removes acknowledged finished jobs from daemon retention.
    pub async fn dismiss_jobs(&self, jobs: Vec<JobId>) -> Result<()> {
        expect_ack(
            "dismiss_jobs",
            self.request(RequestBody::DismissJobs { jobs }).await?,
        )
    }

    /// Fetches the effective configuration.
    pub async fn get_config(&self) -> Result<Config> {
        match self.request(RequestBody::GetConfig).await? {
            ResponseBody::Config(config) => Ok(config),
            response => Err(unexpected("get_config", response)),
        }
    }

    /// Deep-merges a partial configuration patch.
    pub async fn set_config(&self, patch: serde_json::Value) -> Result<Config> {
        match self.request(RequestBody::SetConfig { patch }).await? {
            ResponseBody::Config(config) => Ok(config),
            response => Err(unexpected("set_config", response)),
        }
    }

    /// Counts current process matches for configured keep-alive rules.
    pub async fn match_keep_alive_rules(&self) -> Result<Vec<KeepAliveRuleMatch>> {
        match self.request(RequestBody::MatchKeepAliveRules).await? {
            ResponseBody::KeepAliveRuleMatches(matches) => Ok(matches),
            response => Err(unexpected("match_keep_alive_rules", response)),
        }
    }

    /// Starts a non-destructive import from the default swarm home.
    pub async fn import_from_swarm(&self) -> Result<JobRecord> {
        match self.request(RequestBody::ImportFromSwarm).await? {
            ResponseBody::Job(job) => Ok(job),
            response => Err(unexpected("import_from_swarm", response)),
        }
    }

    /// Runs daemon environment diagnostics.
    pub async fn doctor(&self) -> Result<Vec<DoctorCheck>> {
        match self.request(RequestBody::Doctor).await? {
            ResponseBody::Doctor(checks) => Ok(checks),
            response => Err(unexpected("doctor", response)),
        }
    }

    /// Starts a Fleet self-update job.
    pub async fn update(&self) -> Result<JobRecord> {
        match self.request(RequestBody::Update).await? {
            ResponseBody::Job(job) => Ok(job),
            response => Err(unexpected("update", response)),
        }
    }

    /// Checks daemon liveness.
    pub async fn daemon_ping(&self) -> Result<()> {
        match self.request(RequestBody::DaemonPing).await? {
            ResponseBody::Pong => Ok(()),
            response => Err(unexpected("daemon_ping", response)),
        }
    }

    /// Fetches daemon build and protocol versions.
    pub async fn daemon_version(&self) -> Result<DaemonVersion> {
        match self.request(RequestBody::DaemonVersion).await? {
            ResponseBody::Version { version, protocol } => Ok(DaemonVersion { version, protocol }),
            response => Err(unexpected("daemon_version", response)),
        }
    }

    /// Asks the daemon to shut down explicitly.
    pub async fn daemon_shutdown(&self, stop_sessions: bool) -> Result<()> {
        match self
            .request(RequestBody::DaemonShutdown { stop_sessions })
            .await?
        {
            ResponseBody::ShuttingDown => Ok(()),
            response => Err(unexpected("daemon_shutdown", response)),
        }
    }
}

fn worktree_result(operation: &str, response: ResponseBody) -> Result<CreateWorktreeResult> {
    match response {
        ResponseBody::Worktree {
            created,
            worktree,
            post_create_job,
        } => Ok(CreateWorktreeResult {
            created,
            worktree,
            post_create_job: post_create_job.map(|job| *job),
        }),
        response => Err(unexpected(operation, response)),
    }
}

fn expect_ack(operation: &str, response: ResponseBody) -> Result<()> {
    match response {
        ResponseBody::Ack => Ok(()),
        response => Err(unexpected(operation, response)),
    }
}

fn unexpected(operation: &str, response: ResponseBody) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: format!("unexpected response to {operation}: {response:?}"),
    }
}
