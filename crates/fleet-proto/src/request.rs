//! Client-to-daemon request messages.

use std::path::PathBuf;

use fleet_core::{
    agents::{
        AgentKind, AttentionKind, GateAnswer, GateId, ItemId, ModelSelection, PermissionMode, Seq,
        StreamKind, ThreadId, UserInput,
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
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    agents::CheckpointId,
    event::EventKind,
    terminal::{KeyEvent, MouseEvent, ScrollCommand, WheelEvent},
};

/// Kind of peer opening a daemon protocol connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// Native graphical application (and compatibility default).
    #[default]
    App,
    /// Command-line client.
    Cli,
    /// Another daemon forwarding requests for its clients.
    Proxy,
}

/// Metadata about the peer opening a daemon protocol connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloClient {
    /// Peer category.
    #[serde(default)]
    pub kind: ClientKind,
    /// Identity of the forwarding daemon when `kind` is proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<HostId>,
    /// Optional behaviours this **client** can handle, by the same names the daemon advertises.
    ///
    /// Capability negotiation runs both ways for one reason: an adjacently tagged `Event` variant
    /// an older peer has no arm for kills its whole frame, not just that event. So an event
    /// family added after a client shipped — [`Event::AgentResync`](crate::event::Event::AgentResync)
    /// and its two siblings — is sent only to a connection that named it here. Absent means "the
    /// original set", which is what every client before this field is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

impl HelloClient {
    /// Whether this peer named `capability` in its handshake.
    #[must_use]
    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|held| held == capability)
    }
}

impl<'de> Deserialize<'de> for HelloClient {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Metadata {
            #[serde(default)]
            kind: ClientKind,
            #[serde(default)]
            host_id: Option<HostId>,
            #[serde(default)]
            capabilities: Vec<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireClient {
            Legacy(String),
            Metadata(Metadata),
        }

        Ok(match WireClient::deserialize(deserializer)? {
            WireClient::Legacy(_name) => Self::default(),
            WireClient::Metadata(metadata) => Self {
                kind: metadata.kind,
                host_id: metadata.host_id,
                capabilities: metadata.capabilities,
            },
        })
    }
}

impl From<&str> for HelloClient {
    fn from(_value: &str) -> Self {
        Self::default()
    }
}

impl From<String> for HelloClient {
    fn from(_value: String) -> Self {
        Self::default()
    }
}

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
    /// Open a thread and request a bounded window of its transcript plus an event tail.
    ///
    /// The four window fields are additive on protocol 7 and are sent **only** to a daemon that
    /// advertised [`AGENT_WINDOW_CAPABILITY`](crate::AGENT_WINDOW_CAPABILITY). An older daemon
    /// ignores them and answers the version-6
    /// [`ResponseBody::AgentThreadSnapshot`](crate::response::ResponseBody::AgentThreadSnapshot);
    /// a windowing daemon answers
    /// [`ResponseBody::AgentThreadWindow`](crate::response::ResponseBody::AgentThreadWindow)
    /// exactly when [`RequestBody::wants_window`] holds, so a peer never receives a payload
    /// shape it cannot decode.
    AgentThreadOpen {
        /// Thread to open.
        thread: ThreadId,
        /// Return persisted events strictly after this sequence.
        ///
        /// The version-6 name for the resume cursor. It is neither renamed nor repurposed —
        /// `rust-ipc-protocol` Rule 7 — so a version-6 peer keeps working unchanged;
        /// [`RequestBody::resume_seq`] resolves the pair for the daemon in one place.
        from_seq: Option<Seq>,
        /// Resume cursor for a windowed open.
        ///
        /// When set the daemon replays events after this sequence instead of reading a window.
        /// Overlap with live delivery is expected and deduped by sequence.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_seq: Option<Seq>,
        /// Turns in the returned window; absent means the daemon's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_limit: Option<u32>,
        /// Opaque exclusive keyset cursor from a previous window's `page.before_cursor`.
        ///
        /// Never parsed by a client: a malformed, foreign, or unknown-version token degrades to
        /// the newest page rather than erroring, because the client that sends a stale one is
        /// usually one that just reconnected.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_cursor: Option<String>,
        /// Ask for an explicit [`Event::AgentSynchronized`](crate::event::Event::AgentSynchronized)
        /// between catch-up and live delivery.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        request_sync_marker: bool,
    },
    /// Read a stored item body in offset ranges, for an output a window elided.
    ///
    /// Sent only to daemons advertising
    /// [`AGENT_ITEM_BODY_CAPABILITY`](crate::AGENT_ITEM_BODY_CAPABILITY). `limit` is clamped
    /// server-side by [`clamp_item_body_limit`](crate::agents::clamp_item_body_limit), so a
    /// client asking for a whole 40 MiB build log gets a 256 KiB page and a `total` to walk.
    AgentItemBody {
        /// Owning thread.
        thread: ThreadId,
        /// Item whose body is being read.
        item: ItemId,
        /// Which of the item's append-only streams to read.
        stream: StreamKind,
        /// Byte offset into that stream.
        offset: u64,
        /// Requested byte count, clamped to
        /// [`ITEM_BODY_MAX_CHUNK_BYTES`](crate::agents::ITEM_BODY_MAX_CHUNK_BYTES).
        limit: u32,
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
    /// List the Fleet-owned checkpoints a thread's worktree can be reverted to.
    ///
    /// Sent only to daemons advertising
    /// [`AGENT_CHECKPOINTS_CAPABILITY`](crate::AGENT_CHECKPOINTS_CAPABILITY). The answer is
    /// what `[u]` is drawn from, so a daemon without the service refuses it with
    /// [`checkpoints_capability_error`](crate::agents::checkpoints_capability_error) rather than
    /// answering an empty list.
    AgentCheckpoints {
        /// Target thread.
        thread: ThreadId,
    },
    /// Restore a thread's worktree from one of its checkpoints.
    ///
    /// **Files only.** The harness's conversation is untouched: reverting a working tree and
    /// rewinding a model's context are different operations, and conflating them is how a user
    /// loses work they meant to keep (`docs/NATIVE-AGENTS.md` §5). Nothing here maps to Codex's
    /// deprecated `thread/rollback`, which rewrites history and leaves the files alone —
    /// precisely the opposite trade.
    AgentRevert {
        /// Target thread.
        thread: ThreadId,
        /// Checkpoint to restore, from a previous
        /// [`ResponseBody::AgentCheckpoints`](crate::response::ResponseBody::AgentCheckpoints).
        checkpoint: CheckpointId,
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
        /// Client kind and proxy identity.
        #[serde(default)]
        client: HelloClient,
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
        /// Explicit activity paired with optional semantic attention.
        activity: AgentActivity,
        /// Why the terminal needs the user; absent for working and legacy status-only signals.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attention: Option<AttentionKind>,
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
        /// Remote host; absent means local/default placement.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<HostId>,
    },

    /// Start a job that builds and installs Fleet on a configured host.
    BootstrapHost {
        /// Target configured host.
        host: HostId,
        /// Git ref to install, or the local build commit when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
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
    /// Run diagnostics for one configured remote host.
    DoctorHost {
        /// Configured host to diagnose.
        host: HostId,
    },
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

impl RequestBody {
    /// Whether this open asks for a bounded window rather than the unbounded snapshot.
    ///
    /// The daemon uses exactly this predicate to choose the response shape, which is what makes
    /// adding the window additive: a peer that sent no window field cannot be handed a variant
    /// it has no arm for. A client only ever sets one of these fields after checking
    /// [`AGENT_WINDOW_CAPABILITY`](crate::AGENT_WINDOW_CAPABILITY).
    #[must_use]
    pub const fn wants_window(&self) -> bool {
        match self {
            Self::AgentThreadOpen {
                after_seq,
                turn_limit,
                before_cursor,
                request_sync_marker,
                ..
            } => {
                after_seq.is_some()
                    || turn_limit.is_some()
                    || before_cursor.is_some()
                    || *request_sync_marker
            }
            _ => false,
        }
    }

    /// The resume cursor of an open, whichever field carried it.
    ///
    /// `after_seq` wins when both are set: a peer that speaks the window shape means the newer
    /// field, and resolving the pair here keeps the precedence out of the daemon's dispatch.
    #[must_use]
    pub const fn resume_seq(&self) -> Option<Seq> {
        match self {
            Self::AgentThreadOpen {
                from_seq,
                after_seq,
                ..
            } => match after_seq {
                Some(seq) => Some(*seq),
                None => *from_seq,
            },
            _ => None,
        }
    }
}

/// The thread whose agent mutations must not interleave, for a request that is one.
///
/// The seven agent mutations for one thread are stream-ordered exactly as PTY input is: a mode
/// change overtaking the send it was meant to precede is the terminal resize-overtakes-input bug
/// in another costume. The daemon serializes on the returned thread — **per thread, never
/// globally**, so two threads still run concurrently — and everything else joins the concurrent
/// pool. Reads (`AgentThreadOpen`, `AgentItemBody`, `AgentThreadList`, `AgentCheckpoints`) are
/// deliberately absent: they mutate nothing, and holding a slow remote open ahead of a keystroke
/// is the stall this carve-out exists to prevent.
///
/// `AgentRevert` is in the list for a stronger reason than ordering taste: it rewrites the very
/// worktree the harness is editing, so a revert that interleaved with a send would restore files
/// underneath a running turn.
#[must_use]
pub const fn agent_request_is_serialized(body: &RequestBody) -> Option<ThreadId> {
    match body {
        RequestBody::AgentSend { thread, .. }
        | RequestBody::AgentRespond { thread, .. }
        | RequestBody::AgentInterrupt { thread }
        | RequestBody::AgentSetMode { thread, .. }
        | RequestBody::AgentSetModel { thread, .. }
        | RequestBody::AgentStop { thread }
        | RequestBody::AgentRevert { thread, .. } => Some(*thread),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
