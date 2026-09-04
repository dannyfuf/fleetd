//! Client-to-daemon request messages.

use fleet_core::{
    config::Agent,
    github::PrTab,
    ids::{ContextId, HostId, JobId, RepoId, SessionId, TerminalId, WorktreeId},
    model::RepoHooks,
};
use serde::{Deserialize, Serialize};

use crate::{
    event::EventKind,
    terminal::{KeyEvent, MouseEvent, ScrollCommand},
};

/// A correlated client request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// Client-generated correlation identifier.
    pub id: u64,
    /// Requested operation and its arguments.
    pub body: RequestBody,
}

/// Every operation supported by the CLI and native app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestBody {
    /// Negotiate the protocol immediately after connecting.
    Hello {
        /// Client protocol version.
        protocol: u32,
        /// Client name and version.
        client: String,
    },
    /// Fetch a complete daemon snapshot.
    GetSnapshot,
    /// Subscribe this connection to event families.
    Subscribe {
        /// Event families to forward.
        events: Vec<EventKind>,
    },
    /// Remove all event subscriptions for this connection.
    Unsubscribe,

    /// Create a context.
    CreateContext {
        /// Display name used to derive its identifier.
        name: String,
        /// GitHub owners grouped by the context.
        owners: Vec<String>,
    },
    /// Update a context's display fields.
    UpdateContext {
        /// Context to update.
        id: ContextId,
        /// Replacement display name when supplied.
        name: Option<String>,
        /// Replacement owner list when supplied.
        owners: Option<Vec<String>>,
    },
    /// Delete a context and its descendants.
    DeleteContext {
        /// Context to delete.
        id: ContextId,
    },
    /// Change the active context.
    SetActiveContext {
        /// New active context, or none to clear selection.
        id: Option<ContextId>,
    },

    /// Start cloning and registering a repository.
    CloneRepo {
        /// GitHub owner.
        owner: String,
        /// Repository name.
        name: String,
        /// Clone URL.
        url: String,
        /// Destination context.
        context: ContextId,
    },
    /// Delete a repository and all of its worktrees.
    DeleteRepo {
        /// Repository to delete.
        repo: RepoId,
    },
    /// Assign a repository to another context.
    MoveRepoToContext {
        /// Repository to move.
        repo: RepoId,
        /// Destination context.
        context: ContextId,
    },
    /// Search one owner's remote repositories.
    SearchRemoteRepos {
        /// GitHub owner.
        owner: String,
        /// Fuzzy search query.
        query: String,
    },
    /// List one owner's remote repositories.
    ListRemoteRepos {
        /// GitHub owner.
        owner: String,
        /// Bypass a fresh cache entry.
        force: bool,
    },

    /// Create or idempotently return a worktree.
    CreateWorktree {
        /// Parent repository.
        repo: RepoId,
        /// Canonical worktree slug.
        slug: String,
        /// Explicit branch, or the slug when absent.
        branch: Option<String>,
        /// Explicit base ref, or the repository default when absent.
        base: Option<String>,
        /// Remote host; absent means local.
        host: Option<HostId>,
        /// Repository hook definitions to persist.
        hooks: RepoHooks,
    },
    /// Delete one or more worktrees, continuing after individual failures.
    DeleteWorktrees {
        /// Worktrees to delete.
        ids: Vec<WorktreeId>,
    },
    /// Inspect selected or filtered worktrees.
    InspectWorktrees {
        /// Explicit worktrees; empty means all matching worktrees.
        ids: Vec<WorktreeId>,
        /// Optional repository filter.
        repo: Option<RepoId>,
        /// Fetch remotes before inspecting.
        fetch: bool,
    },
    /// Safely prune merged worktrees.
    PruneWorktrees {
        /// Report eligible worktrees without deleting them.
        dry_run: bool,
        /// Fetch remotes before inspecting.
        fetch: bool,
        /// Kill otherwise eligible running sessions.
        kill_sessions: bool,
        /// Optional repository filter.
        repo: Option<RepoId>,
    },
    /// Hard-kill a worktree session.
    KillWorktree {
        /// Worktree whose session should be killed.
        id: WorktreeId,
    },
    /// Apply sleep policy to a worktree session.
    SleepWorktree {
        /// Worktree whose session should sleep.
        id: WorktreeId,
    },
    /// Update a worktree's last-opened timestamp.
    TouchWorktreeOpened {
        /// Worktree that was opened.
        id: WorktreeId,
    },
    /// Resolve a local worktree's absolute path.
    WorktreePath {
        /// Worktree to resolve.
        id: WorktreeId,
    },

    /// List pull requests scoped to a repository or context.
    ListPullRequests {
        /// Repository scope, mutually exclusive with `context`.
        repo: Option<RepoId>,
        /// Context scope, mutually exclusive with `repo`.
        context: Option<ContextId>,
        /// Authored or review-requested tab.
        tab: PrTab,
        /// Bypass fresh cache entries.
        force: bool,
    },
    /// Create a worktree at a pull request head.
    CreateWorktreeFromPr {
        /// Target repository.
        repo: RepoId,
        /// Pull request number.
        number: u64,
    },

    /// Ensure a worktree or agent session and its configured terminals exist.
    EnsureSession {
        /// Worktree workload, mutually exclusive with `agent`.
        worktree: Option<WorktreeId>,
        /// Agent workload, mutually exclusive with `worktree`.
        agent: Option<Agent>,
        /// Sleep the previously active worktree after switching.
        sleep_previous: bool,
    },
    /// List every daemon-owned session.
    ListSessions,
    /// Hard-kill a session.
    KillSession {
        /// Session to kill.
        session: SessionId,
    },
    /// Apply sleep policy to a session.
    SleepSession {
        /// Session to sleep.
        session: SessionId,
    },
    /// Add a terminal to a session.
    NewTerminal {
        /// Parent session.
        session: SessionId,
        /// Session-local terminal name.
        name: String,
        /// Initial shell command.
        command: String,
        /// Working directory.
        cwd: String,
    },
    /// Close a terminal.
    CloseTerminal {
        /// Terminal to close.
        terminal: TerminalId,
    },
    /// Rename a terminal.
    RenameTerminal {
        /// Terminal to rename.
        terminal: TerminalId,
        /// Replacement name.
        name: String,
    },
    /// Select a session's active terminal.
    SelectTerminal {
        /// Parent session.
        session: SessionId,
        /// Terminal to select.
        terminal: TerminalId,
    },

    /// Attach this connection to a terminal and set its grid size.
    AttachTerminal {
        /// Terminal to attach.
        terminal: TerminalId,
        /// Grid columns.
        cols: u16,
        /// Grid rows.
        rows: u16,
    },
    /// Detach this connection from a terminal.
    DetachTerminal {
        /// Terminal to detach.
        terminal: TerminalId,
    },
    /// Write already encoded bytes to a terminal PTY.
    TerminalInput {
        /// Destination terminal.
        terminal: TerminalId,
        /// Raw bytes.
        bytes: Vec<u8>,
    },
    /// Encode a semantic key event using current terminal modes.
    TerminalKey {
        /// Destination terminal.
        terminal: TerminalId,
        /// Semantic key event.
        key: KeyEvent,
    },
    /// Encode a mouse event when terminal mouse reporting is active.
    TerminalMouse {
        /// Destination terminal.
        terminal: TerminalId,
        /// Semantic mouse event.
        mouse: MouseEvent,
    },
    /// Resize a terminal PTY.
    ResizeTerminal {
        /// Destination terminal.
        terminal: TerminalId,
        /// Grid columns.
        cols: u16,
        /// Grid rows.
        rows: u16,
    },
    /// Move a terminal's server-side scrollback viewport.
    ScrollTerminal {
        /// Destination terminal.
        terminal: TerminalId,
        /// Viewport movement.
        scroll: ScrollCommand,
    },
    /// Request a complete terminal frame after loss or attachment.
    RequestFullFrame {
        /// Destination terminal.
        terminal: TerminalId,
    },
    /// Paste text with mode-aware bracketed-paste handling.
    PasteTerminal {
        /// Destination terminal.
        terminal: TerminalId,
        /// Text to paste.
        text: String,
    },

    /// List current and recently completed jobs.
    ListJobs,
    /// Cancel a cancellable job.
    CancelJob {
        /// Job to cancel.
        job: JobId,
    },
    /// Read the trailing lines of a job log.
    TailJob {
        /// Job whose log should be read.
        job: JobId,
        /// Maximum trailing line count.
        lines: usize,
    },

    /// Fetch the effective configuration.
    GetConfig,
    /// Deep-merge and persist a partial configuration patch.
    SetConfig {
        /// Arbitrary partial configuration object.
        patch: serde_json::Value,
    },
    /// Run dependency and environment diagnostics.
    Doctor,
    /// Start a Fleet self-update job.
    Update,
    /// Check daemon liveness.
    DaemonPing,
    /// Fetch daemon build and protocol versions.
    DaemonVersion,
    /// Shut down the daemon explicitly.
    DaemonShutdown {
        /// Also terminate every daemon-owned session.
        stop_sessions: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_round_trips() {
        let body = RequestBody::AttachTerminal {
            terminal: TerminalId(4),
            cols: 120,
            rows: 40,
        };
        let json = serde_json::to_string(&body).unwrap_or_else(|error| panic!("{error}"));
        let decoded: RequestBody =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, body);

        let body = RequestBody::SetConfig {
            patch: serde_json::json!({"agent":"opencode"}),
        };
        let json = serde_json::to_string(&body).unwrap_or_else(|error| panic!("{error}"));
        let decoded: RequestBody =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, body);
    }
}
