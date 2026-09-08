//! Client-to-daemon request messages.

use std::path::PathBuf;

use fleet_core::{
    agents::{
        AgentKind, GateAnswer, GateId, ModelSelection, PermissionMode, Seq, ThreadId, UserInput,
    },
    board::{BackendRef, BoardPatch, CardDraft, CardPatch, ConflictResolution},
    config::Agent,
    github::PrTab,
    ids::{
        BoardId, CardId, ContextId, HostId, JobId, RepoId, SessionId, StatusId, TerminalId,
        WorktreeId,
    },
    model::RepoHooks,
    sessions::AgentActivity,
    watches::{WatchId, WatchStream},
};
use serde::{Deserialize, Serialize};

use crate::{
    event::EventKind,
    terminal::{KeyEvent, MouseEvent, ScrollCommand, WheelEvent},
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
    /// List persisted and live native-agent threads.
    AgentThreadList,
    /// Create and start a native-agent thread in a published worktree.
    AgentThreadCreate {
        /// Owning published worktree; the daemon resolves its canonical path.
        worktree: WorktreeId,
        /// Provider implementation to start.
        provider: AgentKind,
        /// Optional initial model.
        model: Option<ModelSelection>,
        /// Initial permission policy.
        mode: PermissionMode,
        /// Optional provider-native cursor to resume.
        resume_cursor: Option<String>,
        /// Optional display title.
        title: Option<String>,
    },
    /// Open a thread and request its projection plus an event tail.
    AgentThreadOpen {
        /// Thread to open.
        thread: ThreadId,
        /// Return persisted events strictly after this sequence.
        from_seq: Option<Seq>,
    },
    /// Release one client's interest in a native-agent thread.
    AgentThreadClose {
        /// Thread to close.
        thread: ThreadId,
    },
    /// Send or steer user input.
    AgentSend {
        /// Target thread.
        thread: ThreadId,
        /// Text and attachments.
        input: UserInput,
    },
    /// Interrupt the active turn.
    AgentInterrupt {
        /// Target thread.
        thread: ThreadId,
    },
    /// Answer an open provider gate.
    AgentRespond {
        /// Target thread.
        thread: ThreadId,
        /// Gate being answered.
        gate: GateId,
        /// Provider-neutral answer.
        answer: GateAnswer,
    },
    /// Change a thread's permission or plan mode.
    AgentSetMode {
        /// Target thread.
        thread: ThreadId,
        /// New mode.
        mode: PermissionMode,
    },
    /// Change a thread's model selection.
    AgentSetModel {
        /// Target thread.
        thread: ThreadId,
        /// New model and optional effort/provider.
        model: ModelSelection,
    },
    /// Record a client's last viewed event cursor.
    AgentMarkSeen {
        /// Target thread.
        thread: ThreadId,
        /// Last event visible to the client.
        seq: Seq,
    },
    /// Stop a provider process while retaining its transcript.
    AgentStop {
        /// Target thread.
        thread: ThreadId,
    },
    /// List boards.
    ListBoards {
        /// Context id.
        context_id: Option<ContextId>,
    },
    /// Get board.
    GetBoard {
        /// Board id.
        board_id: BoardId,
    },
    /// Ensure board.
    EnsureBoard {
        /// Context id.
        context_id: ContextId,
    },
    /// Create board.
    CreateBoard {
        /// Context id.
        context_id: ContextId,
        /// Name.
        name: Option<String>,
        /// Prefix.
        prefix: Option<String>,
        /// Backend.
        backend: Option<BackendRef>,
    },
    /// Update board.
    UpdateBoard {
        /// Board id.
        board_id: BoardId,
        /// Patch.
        patch: BoardPatch,
    },
    /// Delete board.
    DeleteBoard {
        /// Board id.
        board_id: BoardId,
    },
    /// Create card.
    CreateCard {
        /// Board id.
        board_id: BoardId,
        /// Draft.
        draft: CardDraft,
    },
    /// Update card.
    UpdateCard {
        /// Card id.
        card_id: CardId,
        /// Patch.
        patch: CardPatch,
    },
    /// Move card.
    MoveCard {
        /// Card id.
        card_id: CardId,
        /// Status id.
        status_id: StatusId,
        /// Index.
        index: Option<usize>,
    },
    /// Delete card.
    DeleteCard {
        /// Card id.
        card_id: CardId,
    },
    /// Add card comment.
    AddCardComment {
        /// Card id.
        card_id: CardId,
        /// Body.
        body: String,
    },
    /// Create worktree from card.
    CreateWorktreeFromCard {
        /// Card id.
        card_id: CardId,
        /// Repo id.
        repo_id: Option<RepoId>,
        /// Base.
        base: Option<String>,
        /// Host.
        host: Option<HostId>,
    },
    /// Sync board.
    SyncBoard {
        /// Board id.
        board_id: BoardId,
        /// Ignore the stored incremental cursor and pull the complete remote set.
        #[serde(default)]
        full: bool,
    },
    /// Resolve card conflict.
    ResolveCardConflict {
        /// Card id.
        card_id: CardId,
        /// Resolution.
        resolution: ConflictResolution,
    },
    /// Describe board backend.
    DescribeBoardBackend {
        /// Board id.
        board_id: BoardId,
    },
    /// List the board backend kinds this daemon registers.
    ListBoardBackends {},

    /// Register a child watch under an existing terminal.
    StartWatch {
        /// Parent terminal.
        terminal: TerminalId,
        /// Display label.
        label: String,
        /// Child argv.
        command: Vec<String>,
        /// Child working directory.
        cwd: Option<PathBuf>,
        /// Child process id.
        pid: Option<u32>,
    },
    /// Append a lossy display copy of child output.
    AppendWatchOutput {
        /// Watch identifier.
        watch: WatchId,
        /// Original channel.
        stream: WatchStream,
        /// Output text.
        text: String,
    },
    /// Report child completion.
    FinishWatch {
        /// Watch identifier.
        watch: WatchId,
        /// Normal exit code.
        code: Option<i32>,
        /// Terminating signal.
        signal: Option<i32>,
    },
    /// List running and retained finished watches for a session.
    ListWatches {
        /// Owning session.
        session: SessionId,
    },
    /// Catch up from an inclusive sequence cursor.
    TailWatch {
        /// Watch identifier.
        watch: WatchId,
        /// Inclusive cursor; None returns all retained output.
        from_seq: Option<u64>,
    },
    /// Remove a finished watch. Running watches return Conflict; no process is killed.
    DismissWatch {
        /// Watch identifier.
        watch: WatchId,
    },

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
        /// Default branch reported by discovery, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_branch: Option<String>,
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
    /// List remote-tracking refs suitable as worktree bases.
    ListBaseRefs {
        /// Repository whose `origin/*` refs should be listed.
        repo: RepoId,
        /// Fetch origin before listing even when cached data is fresh.
        force: bool,
    },
    /// Replace one repository's prepare and post-create hooks.
    SetRepoHooks {
        /// Repository to update.
        repo: RepoId,
        /// Replacement hook configuration.
        hooks: RepoHooks,
    },
    /// Remove a failed clone row after the user has acknowledged it.
    DismissClone {
        /// Failed repository clone to remove.
        repo: RepoId,
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
        /// Exact worktrees approved by the caller; absent preserves legacy repo-wide pruning.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ids: Option<Vec<WorktreeId>>,
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
    /// Restore a recoverable repository or worktree trash entry.
    RestoreTrash {
        /// Trash directory entry name, never an arbitrary path.
        entry: String,
    },
    /// Refresh runtime worktree statuses immediately.
    RefreshStatuses {
        /// Optional repository scope; none refreshes every repository.
        repo: Option<RepoId>,
    },
    /// Set one terminal's coding-agent activity from an explicit lifecycle hook.
    SetAgentActivity {
        /// Owning session.
        session: SessionId,
        /// Target terminal within the session.
        terminal_id: TerminalId,
        /// Explicit working or idle activity.
        activity: AgentActivity,
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

    /// Ensure a worktree, agent, or worktree-scoped agent session and its terminals exist.
    ///
    /// The three shapes are `worktree` alone (the worktree's configured terminals), `agent`
    /// alone (the repository-level popup session in `repos_dir`), and both together — the
    /// `NATIVE-AGENTS.md` §1/§2 `^s F` fallback, which runs the agent command *inside* the
    /// named worktree. Neither is a validation error.
    EnsureSession {
        /// Worktree workload; with `agent`, the worktree the fallback runs in.
        worktree: Option<WorktreeId>,
        /// Agent workload; with `worktree`, the worktree-scoped terminal fallback.
        agent: Option<Agent>,
        /// Sleep the previously active worktree after switching.
        sleep_previous: bool,
    },
    /// List every daemon-owned session.
    ListSessions,
    /// Return the daemon's currently active worktree session, when any.
    CurrentSession,
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
    /// Recreate an exited terminal using its recorded command and working directory.
    RestartTerminal {
        /// Exited terminal to restart.
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
    /// Route wheel input using the terminal's live screen and mouse modes.
    WheelTerminal {
        /// Destination terminal.
        terminal: TerminalId,
        /// Whole-row wheel movement and pointer context.
        wheel: WheelEvent,
    },
    /// Scroll on the primary screen or forward a key on the alternate screen.
    ScrollOrKeyTerminal {
        /// Destination terminal.
        terminal: TerminalId,
        /// Primary-screen viewport movement.
        scroll: ScrollCommand,
        /// Alternate-screen input.
        key: KeyEvent,
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
    /// Restart a retained failed or cancelled job.
    RetryJob {
        /// Retained job to restart.
        job: JobId,
    },
    /// Read the trailing lines of a job log.
    TailJob {
        /// Job whose log should be read.
        job: JobId,
        /// Maximum trailing line count.
        lines: usize,
    },
    /// Remove acknowledged finished jobs from daemon retention.
    DismissJobs {
        /// Finished jobs to remove.
        jobs: Vec<JobId>,
    },

    /// Fetch the effective configuration.
    GetConfig,
    /// Deep-merge and persist a partial configuration patch.
    SetConfig {
        /// Arbitrary partial configuration object.
        patch: serde_json::Value,
    },
    /// Count live process matches for every configured keep-alive rule.
    MatchKeepAliveRules,
    /// Import compatible configuration and state from the default swarm home.
    ImportFromSwarm,
    /// Run dependency and environment diagnostics.
    Doctor,
    /// Replace quarantined state with a validated empty state, retaining the archived file.
    ResetState,
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
    use crate::{
        assert_round_trip,
        terminal::{Key, KeyAction, Modifiers},
    };

    #[test]
    fn request_bodies_round_trip() {
        let repo = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
        let job = JobId::try_from("job-1").unwrap_or_else(|error| panic!("{error}"));
        let thread = ThreadId::new();
        let model = ModelSelection {
            model: "claude-sonnet-5".to_owned(),
            effort: Some("high".to_owned()),
            provider: None,
        };
        let bodies = vec![
            RequestBody::AgentThreadList,
            RequestBody::AgentThreadCreate {
                worktree: WorktreeId::try_from("acme/api#native-agents")
                    .unwrap_or_else(|error| panic!("{error}")),
                provider: AgentKind::Claude,
                model: Some(model.clone()),
                mode: PermissionMode::Ask,
                resume_cursor: Some("session-1".to_owned()),
                title: Some("native agents".to_owned()),
            },
            RequestBody::AgentThreadOpen {
                thread,
                from_seq: Some(Seq(41)),
            },
            RequestBody::AgentThreadClose { thread },
            RequestBody::AgentSend {
                thread,
                input: UserInput {
                    text: "inspect the failing test".to_owned(),
                    attachments: Vec::new(),
                },
            },
            RequestBody::AgentInterrupt { thread },
            RequestBody::AgentRespond {
                thread,
                gate: GateId::new(),
                answer: GateAnswer::Question {
                    answers: vec![vec!["SQLite".to_owned()]],
                },
            },
            RequestBody::AgentSetMode {
                thread,
                mode: PermissionMode::Plan,
            },
            RequestBody::AgentSetModel { thread, model },
            RequestBody::AgentMarkSeen {
                thread,
                seq: Seq(42),
            },
            RequestBody::AgentStop { thread },
            RequestBody::AttachTerminal {
                terminal: TerminalId(4),
                cols: 120,
                rows: 40,
            },
            RequestBody::SetConfig {
                patch: serde_json::json!({"agent":"opencode"}),
            },
            RequestBody::ScrollOrKeyTerminal {
                terminal: TerminalId(8),
                scroll: ScrollCommand::Pages(-1),
                key: KeyEvent {
                    key: Key::PageUp,
                    mods: Modifiers::SHIFT,
                    text: None,
                    action: KeyAction::Press,
                },
            },
            RequestBody::WheelTerminal {
                terminal: TerminalId(8),
                wheel: WheelEvent {
                    steps: -3,
                    col: 12,
                    row: 8,
                    mods: Modifiers::SHIFT | Modifiers::SUPER,
                },
            },
            RequestBody::ListBaseRefs {
                repo: repo.clone(),
                force: true,
            },
            RequestBody::SetRepoHooks {
                repo: repo.clone(),
                hooks: RepoHooks::default(),
            },
            RequestBody::DismissClone { repo: repo.clone() },
            RequestBody::RestoreTrash {
                entry: "123-api".to_owned(),
            },
            RequestBody::RefreshStatuses { repo: Some(repo) },
            RequestBody::SetAgentActivity {
                session: SessionId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
                terminal_id: TerminalId(8),
                activity: AgentActivity::Idle,
            },
            RequestBody::RestartTerminal {
                terminal: TerminalId(8),
            },
            RequestBody::RetryJob { job },
            RequestBody::MatchKeepAliveRules,
            RequestBody::ImportFromSwarm,
        ];
        for body in bodies {
            assert_round_trip(body);
        }
    }

    #[test]
    fn every_board_request_uses_contracted_snake_case_names() {
        let requests = [
            serde_json::json!({"type":"list_boards","context_id":null}),
            serde_json::json!({"type":"get_board","board_id":"work"}),
            serde_json::json!({"type":"ensure_board","context_id":"work"}),
            serde_json::json!({"type":"create_board","context_id":"work","name":null,"prefix":null,"backend":null}),
            serde_json::json!({"type":"update_board","board_id":"work","patch":{}}),
            serde_json::json!({"type":"delete_board","board_id":"work"}),
            serde_json::json!({"type":"create_card","board_id":"work","draft":{"title":"Task"}}),
            serde_json::json!({"type":"update_card","card_id":"card-1","patch":{"assignee":null}}),
            serde_json::json!({"type":"move_card","card_id":"card-1","status_id":"todo","index":1}),
            serde_json::json!({"type":"delete_card","card_id":"card-1"}),
            serde_json::json!({"type":"add_card_comment","card_id":"card-1","body":"Hello"}),
            serde_json::json!({"type":"create_worktree_from_card","card_id":"card-1","repo_id":null,"base":null,"host":null}),
            serde_json::json!({"type":"sync_board","board_id":"work"}),
            serde_json::json!({"type":"resolve_card_conflict","card_id":"card-1","resolution":"take_remote"}),
            serde_json::json!({"type":"describe_board_backend","board_id":"work"}),
        ];
        for json in requests {
            let request: RequestBody =
                serde_json::from_value(json.clone()).unwrap_or_else(|error| panic!("{error}"));
            let encoded = serde_json::to_value(&request).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(encoded["type"], json["type"]);
            let fields = encoded.as_object().expect("tagged request object");
            for (key, value) in json.as_object().expect("fixture object") {
                // A patch and a draft carry their own camelCase contract; only the variant's
                // own fields are snake_case.
                if key != "patch" && key != "draft" {
                    assert_eq!(&encoded[key], value, "wire field {key}");
                }
            }
            assert!(!fields.keys().any(|key| key.contains(char::is_uppercase)));
            assert_round_trip(request);
        }
    }
}
