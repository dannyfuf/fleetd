//! Typed high-level request and response operations.

use fleet_core::{
    board::{
        BackendDescriptor, BackendRef, BackendSchema, BoardPatch, BoardSummary, BoardView, Card,
        CardDraft, CardPatch, ConflictResolution,
    },
    cache::RepoCache,
    config::{Agent, Config},
    github::PrTab,
    ids::{
        BoardId, CardId, ContextId, HostId, JobId, RepoId, SessionId, StatusId, TerminalId,
        WorktreeId,
    },
    inspection::WorktreeInspection,
    model::{Context, Repo, RepoHooks, Worktree},
    sessions::{AgentActivity, Session, Terminal, WorktreeStatus},
};
use fleet_proto::{
    PROTOCOL_VERSION,
    error::{ErrorKind, ProtoError},
    event::EventKind,
    job::JobRecord,
    request::RequestBody,
    response::{
        BaseRefs, DoctorCheck, KeepAliveRuleMatch, PrSlice, PruneResult, ResponseBody, SleepResult,
        WorktreeDeleteResult,
    },
    snapshot::Snapshot,
    terminal::{KeyEvent, MouseEvent, ScrollCommand, WheelEvent},
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
    /// List boards through the daemon.
    pub async fn list_boards(&self, context_id: Option<ContextId>) -> Result<Vec<BoardSummary>> {
        match self.request(RequestBody::ListBoards { context_id }).await? {
            ResponseBody::Boards(value) => Ok(value),
            response => Err(unexpected("list_boards", response)),
        }
    }
    /// Get board through the daemon.
    pub async fn get_board(&self, board_id: BoardId) -> Result<BoardView> {
        match self.request(RequestBody::GetBoard { board_id }).await? {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("get_board", response)),
        }
    }
    /// Ensure board through the daemon.
    pub async fn ensure_board(&self, context_id: ContextId) -> Result<BoardView> {
        match self
            .request(RequestBody::EnsureBoard { context_id })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("ensure_board", response)),
        }
    }
    /// Create board through the daemon.
    pub async fn create_board(
        &self,
        context_id: ContextId,
        name: Option<String>,
        prefix: Option<String>,
        backend: Option<BackendRef>,
    ) -> Result<BoardView> {
        match self
            .request(RequestBody::CreateBoard {
                context_id,
                name,
                prefix,
                backend,
            })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("create_board", response)),
        }
    }
    /// Update board through the daemon.
    pub async fn update_board(&self, board_id: BoardId, patch: BoardPatch) -> Result<BoardView> {
        match self
            .request(RequestBody::UpdateBoard { board_id, patch })
            .await?
        {
            ResponseBody::Board(value) => Ok(value),
            response => Err(unexpected("update_board", response)),
        }
    }
    /// Delete board through the daemon.
    pub async fn delete_board(&self, board_id: BoardId) -> Result<()> {
        match self.request(RequestBody::DeleteBoard { board_id }).await? {
            ResponseBody::Ack => Ok(()),
            response => Err(unexpected("delete_board", response)),
        }
    }
    /// Create card through the daemon.
    pub async fn create_card(&self, board_id: BoardId, draft: CardDraft) -> Result<Card> {
        match self
            .request(RequestBody::CreateCard { board_id, draft })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("create_card", response)),
        }
    }
    /// Update card through the daemon.
    pub async fn update_card(&self, card_id: CardId, patch: CardPatch) -> Result<Card> {
        match self
            .request(RequestBody::UpdateCard { card_id, patch })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("update_card", response)),
        }
    }
    /// Move card through the daemon.
    pub async fn move_card(
        &self,
        card_id: CardId,
        status_id: StatusId,
        index: Option<usize>,
    ) -> Result<Card> {
        match self
            .request(RequestBody::MoveCard {
                card_id,
                status_id,
                index,
            })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("move_card", response)),
        }
    }
    /// Delete card through the daemon.
    pub async fn delete_card(&self, card_id: CardId) -> Result<()> {
        match self.request(RequestBody::DeleteCard { card_id }).await? {
            ResponseBody::Ack => Ok(()),
            response => Err(unexpected("delete_card", response)),
        }
    }
    /// Add card comment through the daemon.
    pub async fn add_card_comment(&self, card_id: CardId, body: String) -> Result<Card> {
        match self
            .request(RequestBody::AddCardComment { card_id, body })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("add_card_comment", response)),
        }
    }
    /// Create worktree from card through the daemon; the flag says whether it was created.
    pub async fn create_worktree_from_card(
        &self,
        card_id: CardId,
        repo_id: Option<RepoId>,
        base: Option<String>,
        host: Option<HostId>,
    ) -> Result<(Card, Worktree, bool)> {
        match self
            .request(RequestBody::CreateWorktreeFromCard {
                card_id,
                repo_id,
                base,
                host,
            })
            .await?
        {
            ResponseBody::CardWorktree {
                card,
                worktree,
                created,
            } => Ok((card, worktree, created)),
            response => Err(unexpected("create_worktree_from_card", response)),
        }
    }
    /// Sync board through the daemon; `full` ignores the stored incremental cursor.
    pub async fn sync_board(&self, board_id: BoardId, full: bool) -> Result<JobId> {
        match self
            .request(RequestBody::SyncBoard { board_id, full })
            .await?
        {
            ResponseBody::Job(job) => Ok(job.id),
            response => Err(unexpected("sync_board", response)),
        }
    }
    /// Resolve card conflict through the daemon.
    pub async fn resolve_card_conflict(
        &self,
        card_id: CardId,
        resolution: ConflictResolution,
    ) -> Result<Card> {
        match self
            .request(RequestBody::ResolveCardConflict {
                card_id,
                resolution,
            })
            .await?
        {
            ResponseBody::Card(value) => Ok(value),
            response => Err(unexpected("resolve_card_conflict", response)),
        }
    }
    /// Describe board backend through the daemon.
    pub async fn describe_board_backend(&self, board_id: BoardId) -> Result<BackendSchema> {
        match self
            .request(RequestBody::DescribeBoardBackend { board_id })
            .await?
        {
            ResponseBody::BoardBackendSchema(value) => Ok(value),
            response => Err(unexpected("describe_board_backend", response)),
        }
    }

    /// List the board backend kinds this daemon registers.
    pub async fn list_board_backends(&self) -> Result<Vec<BackendDescriptor>> {
        match self.request(RequestBody::ListBoardBackends {}).await? {
            ResponseBody::BoardBackends(value) => Ok(value),
            response => Err(unexpected("list_board_backends", response)),
        }
    }

    /// Renegotiates the protocol and returns daemon identity information.
    pub async fn hello(&self, client: impl Into<String>) -> Result<HelloResult> {
        match self
            .request(RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
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

    /// Sets one terminal's coding-agent activity from an explicit lifecycle hook.
    pub async fn set_agent_activity(
        &self,
        session: SessionId,
        terminal_id: TerminalId,
        activity: AgentActivity,
    ) -> Result<()> {
        expect_ack(
            "set_agent_activity",
            self.request(RequestBody::SetAgentActivity {
                session,
                terminal_id,
                activity,
            })
            .await?,
        )
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

    /// Enqueues wheel input in order without waiting for the daemon's response.
    pub async fn wheel_terminal(&self, terminal: TerminalId, wheel: WheelEvent) -> Result<()> {
        self.request_background(RequestBody::WheelTerminal { terminal, wheel })
            .await
    }

    /// Enqueues a viewport shortcut in order without waiting for acknowledgement.
    pub async fn scroll_or_key_terminal(
        &self,
        terminal: TerminalId,
        scroll: ScrollCommand,
        key: KeyEvent,
    ) -> Result<()> {
        self.request_background(RequestBody::ScrollOrKeyTerminal {
            terminal,
            scroll,
            key,
        })
        .await
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

pub(crate) fn expect_ack(operation: &str, response: ResponseBody) -> Result<()> {
    match response {
        ResponseBody::Ack => Ok(()),
        response => Err(unexpected(operation, response)),
    }
}

pub(crate) fn unexpected(operation: &str, response: ResponseBody) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: format!("unexpected response to {operation}: {response:?}"),
    }
}

#[cfg(test)]
mod board_tests {
    use super::*;
    use fleet_core::board::{new_board, summarize};
    use fleet_proto::{
        codec::FleetCodec,
        job::{JobKind, JobStatus},
        request::Request,
        response::Response,
    };
    use futures_util::{SinkExt, StreamExt};
    use std::{future::Future, time::Duration};
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::Framed;

    type Transport = Framed<UnixStream, FleetCodec<Response, Request>>;

    async fn exchange<T>(
        transport: &mut Transport,
        expected: RequestBody,
        result: Result<ResponseBody>,
        operation: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let server = async {
            let request = transport.next().await.unwrap().unwrap();
            assert_eq!(request.body, expected);
            transport
                .send(Response {
                    id: request.id,
                    result,
                })
                .await
                .unwrap();
        };
        let (_, result) = tokio::join!(server, operation);
        result
    }

    #[tokio::test]
    async fn board_api_round_trips_over_the_unix_socket() {
        tokio::time::timeout(Duration::from_secs(10), board_api_round_trips())
            .await
            .unwrap();
    }

    async fn board_api_round_trips() {
        let home = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(fleet_proto::paths::socket_path(home.path())).unwrap();
        let server = async {
            let (socket, _) = listener.accept().await.unwrap();
            let mut transport = Transport::new(socket, FleetCodec::new());
            let hello = transport.next().await.unwrap().unwrap();
            assert!(matches!(
                hello.body,
                RequestBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    ..
                }
            ));
            transport
                .send(Response {
                    id: hello.id,
                    result: Ok(ResponseBody::Hello {
                        protocol: PROTOCOL_VERSION,
                        server: "board-test".into(),
                    }),
                })
                .await
                .unwrap();
            let subscribe = transport.next().await.unwrap().unwrap();
            assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
            transport
                .send(Response {
                    id: subscribe.id,
                    result: Ok(ResponseBody::Ack),
                })
                .await
                .unwrap();
            transport
        };
        let (client, mut transport) = tokio::join!(Client::connect(home.path()), server);
        let client = client.unwrap();
        let context = Context {
            id: "work".parse().unwrap(),
            name: "Work".into(),
            owners: vec![],
            created_at: "now".into(),
        };
        let board = new_board(&context, "now");
        let card: Card = serde_json::from_value(serde_json::json!({
            "id":"card-1", "boardId":"work", "number":1, "title":"Task", "statusId":"todo",
            "createdAt":"now", "updatedAt":"now"
        }))
        .unwrap();
        let view = BoardView {
            board: board.clone(),
            cards: vec![card.clone()],
        };
        let summary = summarize(&board, &view.cards);
        let worktree: Worktree = serde_json::from_value(serde_json::json!({
            "id":"acme/api#task", "repoId":"acme/api", "slug":"task", "branch":"task",
            "baseRef":"main", "path":"/tmp/task", "session":"task", "createdAt":"now"
        }))
        .unwrap();
        let job = JobRecord {
            id: "sync-job".parse().unwrap(),
            kind: JobKind::Custom("board.sync".into()),
            target: board.id.to_string(),
            title: "Sync board".into(),
            status: JobStatus::Queued,
            progress: None,
            log_path: "/tmp/sync.log".into(),
            started_at: "now".into(),
            finished_at: None,
            cancellable: true,
            retryable: false,
        };
        // Each operation is checked at the socket boundary, including every argument and typed result.
        macro_rules! check {
            ($request:expr, $response:expr, $operation:expr, $expected:expr) => {
                assert_eq!(
                    exchange(&mut transport, $request, Ok($response), $operation)
                        .await
                        .unwrap(),
                    $expected
                );
            };
        }
        check!(
            RequestBody::ListBoards {
                context_id: Some(context.id.clone())
            },
            ResponseBody::Boards(vec![summary.clone()]),
            client.list_boards(Some(context.id.clone())),
            vec![summary]
        );
        check!(
            RequestBody::GetBoard {
                board_id: board.id.clone()
            },
            ResponseBody::Board(view.clone()),
            client.get_board(board.id.clone()),
            view
        );
        check!(
            RequestBody::EnsureBoard {
                context_id: context.id.clone()
            },
            ResponseBody::Board(view.clone()),
            client.ensure_board(context.id.clone()),
            view
        );
        check!(
            RequestBody::CreateBoard {
                context_id: context.id.clone(),
                name: Some("Team".into()),
                prefix: Some("TM".into()),
                backend: Some(BackendRef::default())
            },
            ResponseBody::Board(view.clone()),
            client.create_board(
                context.id.clone(),
                Some("Team".into()),
                Some("TM".into()),
                Some(BackendRef::default())
            ),
            view
        );
        let board_patch = BoardPatch {
            default_repo_id: Some(None),
            ..Default::default()
        };
        check!(
            RequestBody::UpdateBoard {
                board_id: board.id.clone(),
                patch: board_patch.clone()
            },
            ResponseBody::Board(view.clone()),
            client.update_board(board.id.clone(), board_patch),
            view
        );
        let draft = CardDraft {
            title: "Task".into(),
            ..Default::default()
        };
        check!(
            RequestBody::CreateCard {
                board_id: board.id.clone(),
                draft: draft.clone()
            },
            ResponseBody::Card(card.clone()),
            client.create_card(board.id.clone(), draft),
            card
        );
        let patch = CardPatch {
            assignee: Some(None),
            estimate: Some(Some(3)),
            ..Default::default()
        };
        check!(
            RequestBody::UpdateCard {
                card_id: card.id.clone(),
                patch: patch.clone()
            },
            ResponseBody::Card(card.clone()),
            client.update_card(card.id.clone(), patch),
            card
        );
        check!(
            RequestBody::MoveCard {
                card_id: card.id.clone(),
                status_id: card.status_id.clone(),
                index: Some(2)
            },
            ResponseBody::Card(card.clone()),
            client.move_card(card.id.clone(), card.status_id.clone(), Some(2)),
            card
        );
        check!(
            RequestBody::AddCardComment {
                card_id: card.id.clone(),
                body: "Hello".into()
            },
            ResponseBody::Card(card.clone()),
            client.add_card_comment(card.id.clone(), "Hello".into()),
            card
        );
        check!(
            RequestBody::CreateWorktreeFromCard {
                card_id: card.id.clone(),
                repo_id: Some(worktree.repo_id.clone()),
                base: Some("main".into()),
                host: Some("devbox".parse().unwrap())
            },
            ResponseBody::CardWorktree {
                card: card.clone(),
                worktree: worktree.clone(),
                created: true
            },
            client.create_worktree_from_card(
                card.id.clone(),
                Some(worktree.repo_id.clone()),
                Some("main".into()),
                Some("devbox".parse().unwrap())
            ),
            (card.clone(), worktree, true)
        );
        check!(
            RequestBody::SyncBoard {
                board_id: board.id.clone(),
                full: false
            },
            ResponseBody::Job(job.clone()),
            client.sync_board(board.id.clone(), false),
            job.id
        );
        check!(
            RequestBody::SyncBoard {
                board_id: board.id.clone(),
                full: true
            },
            ResponseBody::Job(job.clone()),
            client.sync_board(board.id.clone(), true),
            job.id
        );
        let descriptors = vec![BackendDescriptor {
            kind: "local".into(),
            label: "Local".into(),
            capabilities: Default::default(),
            settings_schema: vec![],
        }];
        check!(
            RequestBody::ListBoardBackends {},
            ResponseBody::BoardBackends(descriptors.clone()),
            client.list_board_backends(),
            descriptors
        );
        check!(
            RequestBody::ResolveCardConflict {
                card_id: card.id.clone(),
                resolution: ConflictResolution::TakeRemote
            },
            ResponseBody::Card(card.clone()),
            client.resolve_card_conflict(card.id.clone(), ConflictResolution::TakeRemote),
            card
        );
        let schema = BackendSchema {
            key_prefix: Some("EXT".into()),
            ..Default::default()
        };
        check!(
            RequestBody::DescribeBoardBackend {
                board_id: board.id.clone()
            },
            ResponseBody::BoardBackendSchema(schema.clone()),
            client.describe_board_backend(board.id.clone()),
            schema
        );
        check!(
            RequestBody::DeleteCard {
                card_id: card.id.clone()
            },
            ResponseBody::Ack,
            client.delete_card(card.id.clone()),
            ()
        );
        check!(
            RequestBody::DeleteBoard {
                board_id: board.id.clone()
            },
            ResponseBody::Ack,
            client.delete_board(board.id.clone()),
            ()
        );

        let error = ProtoError {
            kind: ErrorKind::NotFound,
            message: "board missing".into(),
        };
        assert_eq!(
            exchange(
                &mut transport,
                RequestBody::GetBoard {
                    board_id: board.id.clone()
                },
                Err(error.clone()),
                client.get_board(board.id.clone())
            )
            .await
            .unwrap_err(),
            error
        );
        let unexpected = exchange(
            &mut transport,
            RequestBody::SyncBoard {
                board_id: board.id.clone(),
                full: false,
            },
            Ok(ResponseBody::Ack),
            client.sync_board(board.id, false),
        )
        .await
        .unwrap_err();
        assert_eq!(unexpected.kind, ErrorKind::Unknown);
        assert!(unexpected.message.contains("sync_board"));
    }
}
