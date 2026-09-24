//! Clap argument and subcommand definitions.

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Package version plus the build-time Git revision.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("FLEET_GIT_SHA"));
/// Full human-readable Fleet build version.
pub const VERSION_DISPLAY: &str = concat!(
    "fleet ",
    env!("CARGO_PKG_VERSION"),
    "+",
    env!("FLEET_GIT_SHA")
);

/// Fleet's command-line interface.
#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    name = "fleet",
    about = "Native worktree and terminal-session manager",
    version = VERSION,
    disable_version_flag = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Print Fleet's build version.
    #[arg(short = 'v', long = "version")]
    pub version: bool,
    /// Operation to perform.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// A Fleet operation.
// `board card edit` carries the widest flag set in the CLI, so `Board` is several hundred bytes
// wider than its siblings. One of these is parsed once per process and consumed immediately;
// boxing it would buy nothing and cost every match site a dereference.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Copy text through the terminal's OSC 52 clipboard integration.
    Clipboard(ClipboardArgs),
    /// Manage configured remote machines.
    Host(HostArgs),
    /// Manage context and worktree boards and their cards.
    Board(BoardArgs),
    /// Run a command, optionally teeing piped output to a read-only watch.
    Exec(ExecArgs),
    /// Inspect subagent watches and their retained output.
    Watch(WatchArgs),
    /// Private PID-preserving watch launcher.
    #[command(hide = true)]
    WatchChild(WatchChildArgs),
    /// Create or find a worktree.
    Create(CreateArgs),
    /// Ensure a worktree session exists.
    Open(OpenArgs),
    /// List registered repositories and worktrees.
    List(JsonArgs),
    /// Inspect worktree Git and runtime state.
    Inspect(InspectArgs),
    /// Unconditionally delete worktrees.
    Delete(DeleteArgs),
    /// Safely prune merged worktrees.
    Prune(PruneArgs),
    /// Hard-kill a worktree session.
    Kill(KillArgs),
    /// Refresh local worktree runtime status.
    Status(JsonArgs),
    /// Print a local worktree's absolute path.
    Path(PathArgs),
    /// Apply sleep policy to a session.
    Sleep(SleepArgs),
    /// Create and control native coding-agent threads.
    Agent(AgentArgs),
    /// Create and control delegated native-agent threads.
    Subagent(SubagentArgs),
    /// Report coding-agent lifecycle activity for the current Fleet terminal.
    AgentStatus(AgentStatusArgs),
    /// Run environment diagnostics.
    Doctor(DoctorArgs),
    /// Import compatible swarm configuration and state.
    Import(ImportArgs),
    /// Start a Fleet self-update job.
    Update,
    /// Manage the Fleet daemon.
    Daemon(DaemonArgs),
    /// Print Fleet's build version.
    Version,
}

/// Arguments accepted by `fleet host`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct HostArgs {
    /// Host operation.
    #[command(subcommand)]
    pub command: HostCommand,
}

/// Configured-host operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum HostCommand {
    /// List configured hosts and cached status.
    List {
        /// Emit a protocol-versioned JSON envelope.
        #[arg(long)]
        json: bool,
    },
    /// Run diagnostics for one configured host.
    Doctor {
        /// Configured host id.
        id: fleet_core::ids::HostId,
    },
    /// Build and install Fleet on one configured host.
    Bootstrap {
        /// Configured host id.
        id: fleet_core::ids::HostId,
        /// Source Git ref to install.
        #[arg(long = "ref")]
        git_ref: Option<String>,
    },
}

/// Arguments accepted by `fleet watch`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct WatchArgs {
    /// Watch query operation.
    #[command(subcommand)]
    pub command: WatchCommand,
}

/// Read-only watch operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum WatchCommand {
    /// List watches for --session or FLEET_SESSION.
    List(WatchListArgs),
    /// Print retained output, optionally following until the watch exits.
    Tail(WatchTailArgs),
}

/// Session and output format for a watch list.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct WatchListArgs {
    /// Session ID; defaults to FLEET_SESSION.
    #[arg(long)]
    pub session: Option<fleet_core::ids::SessionId>,
    /// Print a protocol-1 JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Output query for one watch.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct WatchTailArgs {
    /// Numeric daemon-local watch ID.
    pub id: fleet_core::watches::WatchId,
    /// Follow the watch until it exits, preserving stdout/stderr channels.
    #[arg(long)]
    pub follow: bool,
}

/// Arguments accepted by `fleet exec`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ExecArgs {
    /// Publish a watch when running inside a Fleet terminal.
    #[arg(long)]
    pub watch: bool,
    /// Display label (defaults to command basename).
    #[arg(long)]
    pub label: Option<String>,
    /// Command and unmodified arguments after --.
    #[arg(last = true, required = true, num_args = 1..)]
    pub command: Vec<std::ffi::OsString>,
}

/// Internal launcher arguments; not a user-facing command.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct WatchChildArgs {
    /// Private launch handshake socket.
    pub socket: std::path::PathBuf,
    /// Target argv.
    #[arg(last = true, required = true, num_args = 1..)]
    pub command: Vec<std::ffi::OsString>,
}

/// Arguments accepted by `fleet daemon`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct DaemonArgs {
    /// Daemon lifecycle operation.
    #[command(subcommand)]
    pub command: DaemonCommand,
}

/// Daemon lifecycle operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum DaemonCommand {
    /// Gracefully stop the running daemon and start the current fleetd binary.
    Restart,
}

/// Arguments accepted by `fleet clipboard`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ClipboardArgs {
    /// Clipboard operation.
    #[command(subcommand)]
    pub command: ClipboardCommand,
}

/// Terminal clipboard operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum ClipboardCommand {
    /// Copy a text argument, or standard input when no argument is supplied.
    Copy(ClipboardCopyArgs),
}

/// Text source for `fleet clipboard copy`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ClipboardCopyArgs {
    /// Text to copy after `--`; omit it to read standard input.
    #[arg(last = true, value_name = "TEXT")]
    pub text: Option<String>,
}

/// Arguments accepted by `fleet create`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct CreateArgs {
    /// Repository identifier in owner/name form.
    pub repo: String,
    /// Canonical worktree slug.
    pub slug: String,
    /// Branch to check out or create (defaults to the slug).
    #[arg(long)]
    pub branch: Option<String>,
    /// Base ref (defaults to origin/<repository default branch>).
    #[arg(long)]
    pub base: Option<String>,
    /// Remote host identifier; omit for local/default placement.
    #[arg(long)]
    pub host: Option<String>,
    /// Clone URL required when the repository is not registered.
    #[arg(long)]
    pub url: Option<String>,
    /// Default branch hint used while creating an unregistered repository.
    #[arg(long = "default-branch")]
    pub default_branch: Option<String>,
    /// Repository hooks as a JSON object.
    #[arg(long)]
    pub hooks: Option<String>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet open`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct OpenArgs {
    /// Worktree id, stored session name, or repo/slug alias.
    pub target: String,
}

/// A command with an optional JSON output mode.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct JsonArgs {
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet doctor`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct DoctorArgs {
    /// Replace quarantined state with an empty state while retaining its archived file.
    #[arg(long)]
    pub reset_state: bool,
    /// Emit a protocol-versioned JSON envelope for a state reset.
    #[arg(long, requires = "reset_state")]
    pub json: bool,
}

/// Arguments accepted by `fleet inspect`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct InspectArgs {
    /// Exact worktree identifiers; omit to inspect all worktrees.
    pub ids: Vec<String>,
    /// Fetch remotes before inspection.
    #[arg(long)]
    pub fetch: bool,
    /// Restrict inspection to one owner/name repository.
    #[arg(long)]
    pub repo: Option<String>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet delete`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct DeleteArgs {
    /// Exact worktree identifiers.
    #[arg(required = true, num_args = 1..)]
    pub ids: Vec<String>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet prune`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct PruneArgs {
    /// Report eligible worktrees without deleting them.
    #[arg(long)]
    pub dry_run: bool,
    /// Do not fetch remotes before inspection.
    #[arg(long)]
    pub no_fetch: bool,
    /// Permit killing detached sessions with running commands.
    #[arg(long)]
    pub kill_sessions: bool,
    /// Restrict pruning to one owner/name repository.
    #[arg(long)]
    pub repo: Option<String>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet kill`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct KillArgs {
    /// Exact worktree identifier.
    pub id: String,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet path`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct PathArgs {
    /// Exact local worktree identifier.
    pub id: String,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet sleep`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SleepArgs {
    /// Session or worktree identifier. A sole running session is inferred when omitted.
    pub session: Option<String>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Supported coding-agent choices.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum AgentChoice {
    /// Claude Code.
    Claude,
    /// OpenAI Codex.
    Codex,
}

/// Arguments accepted by `fleet agent`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentArgs {
    /// Native-agent thread operation.
    #[command(subcommand)]
    pub command: AgentCommand,
}

/// Native-agent thread operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum AgentCommand {
    /// List persisted and live native-agent threads.
    List,
    /// Create a native-agent thread in a published worktree.
    New(AgentNewArgs),
    /// Send or steer a message.
    Send(AgentSendArgs),
    /// Answer an open permission, question, or plan gate.
    Respond(AgentRespondArgs),
    /// Interrupt the active turn.
    Interrupt(AgentThreadArgs),
    /// Stop the provider while retaining its transcript.
    Stop(AgentThreadArgs),
    /// Print sequenced events until the provider exits.
    ///
    /// The raw event stream of one thread. To read what a delegated child *reported*, use
    /// `fleet subagent status <delegation>`, which prints the report body whole; tailing a
    /// child thread to reconstruct it from events is neither necessary nor reliable.
    Tail(AgentTailArgs),
    /// Open the legacy PTY agent session for this repository (the §10 fallback).
    Terminal(AgentTerminalArgs),
}

/// Arguments accepted by `fleet subagent`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SubagentArgs {
    /// Delegated-agent operation.
    #[command(subcommand)]
    pub command: SubagentCommand,
}

/// Delegated-agent operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum SubagentCommand {
    /// Start a delegated child thread.
    ///
    /// `--json` answers an envelope whose `delegation.brief` is cut to a 200-character preview,
    /// and which then carries `briefElided: true`; the caller wrote the brief, so echoing it back
    /// whole only costs it context. A brief already that short is carried whole and the key is
    /// absent. `fleet subagent status --json` always returns it whole.
    Run(SubagentRunArgs),
    /// Report the current delegated child's result.
    Complete(SubagentCompleteArgs),
    /// Wait for a delegation to finish or for a timeout.
    ///
    /// On success the child's own report body is printed: in human output under the delivered
    /// message, and in `--json` as the envelope's `delegation.result.text`. There is no second
    /// verb to run for it, and `fleet subagent status` prints the same body again afterwards.
    ///
    /// A wait issued by the delegation's own caller — `--caller`, else FLEET_SESSION — also
    /// consumes the result, so the same report is not injected into the caller's transcript a
    /// second time. A wait from anywhere else reads without consuming.
    ///
    /// Like `run`, `--json` cuts a brief over 200 characters and then sets `briefElided: true`.
    Wait(SubagentWaitArgs),
    /// Show one delegation, its brief, its usage, and the child's report.
    ///
    /// The whole record, in order: the fixed-field line, the brief in full, the child's spend
    /// when the daemon knows it, and — once the delegation is terminal — the child's own report
    /// rendered exactly as `fleet subagent wait` renders it, byte for byte. Unlike `run`, `wait`
    /// and `list`, the `--json` envelope keeps the brief whole. Reading a report never consumes
    /// it.
    Status(SubagentIdArgs),
    /// List delegations, optionally for one caller.
    ///
    /// One fixed-field line each: id, status, provider, child thread, duration, total tokens,
    /// cost, delivery. An unknown token count or cost prints `-`. `--json` cuts every brief over
    /// 200 characters and then sets `briefElided: true`.
    List(SubagentListArgs),
    /// Cancel one live delegation.
    Cancel(SubagentIdArgs),
}

/// Options for starting a delegated child thread.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SubagentRunArgs {
    /// Structured provider to launch.
    #[arg(long, value_enum)]
    pub provider: AgentChoice,
    /// Read the work brief from this file instead of stdin.
    #[arg(long)]
    pub brief_file: Option<std::path::PathBuf>,
    /// Completion criteria for the child.
    #[arg(long = "expect")]
    pub expectation: String,
    /// Published worktree override.
    #[arg(long)]
    pub worktree: Option<fleet_core::ids::WorktreeId>,
    /// Permission-mode override.
    #[arg(long, value_enum)]
    pub mode: Option<AgentModeChoice>,
    /// Provider-native model override.
    #[arg(long)]
    pub model: Option<String>,
    /// Provider-native reasoning effort, with or without `--model`.
    ///
    /// Free text, never an enum: the legal ladder is per provider and per model and is published
    /// by the harness, so Fleet passes whatever is given straight through and lets the provider
    /// reject a value it does not know. Passed alone, the child keeps the model configured as
    /// that provider's default and runs at this effort.
    #[arg(long)]
    pub effort: Option<String>,
    /// Child-thread title override.
    #[arg(long)]
    pub title: Option<String>,
    /// Environment variable for the child, repeatable: `--env KEY=VALUE`.
    ///
    /// Each entry is a whole-value override in the child's process environment, so it is for a
    /// variable the child would otherwise inherit nothing for — `--env CARGO_TARGET_DIR=…` to
    /// keep two concurrent children off one build directory is the case this exists for.
    ///
    /// Five refusals, all validation errors naming what they rejected: an entry with no `=`, an
    /// empty key, the same key twice, any `FLEET_*` name — the delegation's own identity, which
    /// a caller must not be able to forge — and `PATH`, because an entry here replaces the value
    /// outright rather than extending the login shell's, and Fleet already prepends the
    /// directory holding this `fleet` so the child can run `fleet subagent complete`.
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// Deliver the result as soon as it is ready.
    #[arg(long)]
    pub eager: bool,
    /// Calling native-agent thread; defaults to FLEET_SESSION.
    #[arg(long)]
    pub caller: Option<fleet_core::agents::ThreadId>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Options for reporting a delegated child result.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SubagentCompleteArgs {
    /// Delegation id; defaults to FLEET_DELEGATION.
    pub id: Option<fleet_core::agents::DelegationId>,
    /// Read the result from this file instead of stdin.
    #[arg(long)]
    pub result_file: Option<std::path::PathBuf>,
    /// Report that the child is blocked rather than finished.
    #[arg(long)]
    pub blocked: bool,
    /// Require the result to be valid JSON.
    #[arg(long)]
    pub json_result: bool,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Options for waiting on one delegation.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SubagentWaitArgs {
    /// Delegation id.
    pub id: fleet_core::agents::DelegationId,
    /// Maximum wait in seconds; defaults to 540.
    ///
    /// 540 is a default, not a ceiling: it sits under the Claude Code shell-tool timeout so the
    /// common caller outlives its own wait. Larger values are accepted and Fleet imposes no
    /// upper bound of its own, but the caller's tool timeout may still kill the wait before this
    /// one elapses. A timeout exits 2 and says the child is still running; a terminal record
    /// exits 0 and returns the child's report.
    #[arg(long, default_value_t = 540)]
    pub timeout: u64,
    /// Thread this wait is issued for; defaults to FLEET_SESSION.
    ///
    /// Naming the delegation's own caller is what marks the result read, so the report this wait
    /// returns is not also injected into that thread's transcript. Optional on purpose: a wait
    /// from a plain shell with no FLEET_SESSION works exactly as before and simply consumes
    /// nothing. A malformed value is still refused rather than silently ignored.
    #[arg(long)]
    pub caller: Option<fleet_core::agents::ThreadId>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// One delegation id and output mode.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SubagentIdArgs {
    /// Delegation id.
    pub id: fleet_core::agents::DelegationId,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Delegation-list filters and output mode.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SubagentListArgs {
    /// Restrict results to one calling thread.
    #[arg(long)]
    pub caller: Option<fleet_core::agents::ThreadId>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Provider selection for the legacy PTY agent session.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentTerminalArgs {
    /// Agent to launch; defaults to `config.agent`.
    #[arg(value_enum)]
    pub agent: Option<AgentChoice>,
}

/// Creation options for one native-agent thread.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentNewArgs {
    /// Exact published worktree identifier.
    pub worktree: fleet_core::ids::WorktreeId,
    /// Structured provider to launch.
    #[arg(long, value_enum)]
    pub provider: AgentChoice,
    /// Provider-native model, optionally qualified as `provider/model` when supported.
    #[arg(long)]
    pub model: Option<String>,
    /// Initial permission or plan mode.
    #[arg(long, value_enum)]
    pub mode: Option<AgentModeChoice>,
}

/// Permission modes accepted by `fleet agent new`.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum AgentModeChoice {
    /// Ask before protected operations.
    Ask,
    /// Apply edits without asking.
    AcceptEdits,
    /// Request a plan before execution.
    Plan,
    /// Let Claude approve actions it classifies as safe.
    Auto,
    /// Let Claude deny unlisted tools instead of asking.
    DontAsk,
    /// Auto-allow supported operations.
    FullAccess,
}

/// A native-agent thread and message.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentSendArgs {
    /// Thread UUID.
    pub thread: fleet_core::agents::ThreadId,
    /// Message text.
    pub text: String,
}

/// A native-agent gate and provider-neutral answer words.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentRespondArgs {
    /// Thread UUID.
    pub thread: fleet_core::agents::ThreadId,
    /// Gate UUID.
    pub gate: fleet_core::agents::GateId,
    /// Answer words; permission/plan answers begin with their action.
    #[arg(required = true, num_args = 1..)]
    pub answer: Vec<String>,
}

/// A native-agent thread target.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentThreadArgs {
    /// Thread UUID.
    pub thread: fleet_core::agents::ThreadId,
}

/// Event-tail options for one native-agent thread.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentTailArgs {
    /// Thread UUID.
    pub thread: fleet_core::agents::ThreadId,
    /// Print retained history before following live events.
    #[arg(long)]
    pub replay: bool,
    /// Print the retained history and exit instead of following; implies `--replay`.
    ///
    /// Without it a tail that is killed by its caller's timeout before the thread says anything
    /// new prints nothing at all, because the retained events are only folded into the
    /// projection. This is the flag for taking a snapshot from a script.
    #[arg(long)]
    pub no_follow: bool,
    /// Print only the last N retained events; implies `--replay`.
    ///
    /// A client-side trim of the history this tail already received, not a paginated request.
    /// It bounds the replay only: when the tail goes on to follow, every live event still
    /// prints.
    #[arg(long, value_name = "N")]
    pub last: Option<usize>,
}

/// Activity values accepted by `fleet agent-status`.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum AgentStatusChoice {
    /// The agent started or resumed work.
    Working,
    /// The agent finished its turn and is waiting for the user.
    Finished,
    /// The agent is waiting for tool permission.
    Permission,
    /// The agent asked the user a question.
    Question,
    /// The agent proposed a plan for approval.
    Plan,
}

/// Arguments accepted by `fleet agent-status`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct AgentStatusArgs {
    /// Activity reported by the agent hook.
    #[arg(value_enum)]
    pub activity: AgentStatusChoice,
    /// Owning session; defaults to FLEET_SESSION.
    #[arg(long)]
    pub session: Option<String>,
    /// Numeric terminal id; defaults to FLEET_TERMINAL_ID.
    #[arg(long)]
    pub terminal_id: Option<u64>,
    /// Emit a protocol-versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

/// Arguments accepted by `fleet import`.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ImportArgs {
    /// Import ~/.swarm/config.json and state.json without modifying the source.
    #[arg(long, required = true)]
    pub from_swarm: bool,
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser, error::ErrorKind};

    use super::*;

    fn parses(arguments: &[&str]) -> Command {
        Cli::try_parse_from(arguments)
            .unwrap_or_else(|error| panic!("failed to parse {arguments:?}: {error}"))
            .command
            .unwrap_or_else(|| panic!("missing command for {arguments:?}"))
    }

    #[test]
    fn parses_every_command_and_flag() {
        let cases = [
            vec!["fleet", "clipboard", "copy"],
            vec!["fleet", "clipboard", "copy", "--", "a b"],
            vec![
                "fleet",
                "create",
                "acme/api",
                "feature",
                "--branch",
                "topic",
                "--base",
                "origin/main",
                "--host",
                "devbox",
                "--url",
                "git@example/acme/api.git",
                "--default-branch",
                "trunk",
                "--hooks",
                r#"{"prepare":[],"postCreate":[]}"#,
                "--json",
            ],
            vec!["fleet", "open", "acme/api#feature"],
            vec!["fleet", "list", "--json"],
            vec![
                "fleet",
                "inspect",
                "acme/api#one",
                "acme/api#two",
                "--fetch",
                "--repo",
                "acme/api",
                "--json",
            ],
            vec!["fleet", "delete", "acme/api#one", "acme/api#two", "--json"],
            vec![
                "fleet",
                "prune",
                "--dry-run",
                "--no-fetch",
                "--kill-sessions",
                "--repo",
                "acme/api",
                "--json",
            ],
            vec!["fleet", "kill", "acme/api#feature", "--json"],
            vec!["fleet", "status", "--json"],
            vec!["fleet", "path", "acme/api#feature"],
            vec!["fleet", "sleep", "api/feature", "--json"],
            vec!["fleet", "agent", "list"],
            vec![
                "fleet",
                "agent",
                "new",
                "acme/api#feature",
                "--provider",
                "codex",
                "--model",
                "gpt-5.1-codex-max",
                "--mode",
                "plan",
            ],
            vec![
                "fleet",
                "agent",
                "send",
                "00000000-0000-4000-8000-000000000001",
                "hello",
            ],
            vec![
                "fleet",
                "agent",
                "respond",
                "00000000-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000002",
                "once",
            ],
            vec![
                "fleet",
                "agent",
                "interrupt",
                "00000000-0000-4000-8000-000000000001",
            ],
            vec![
                "fleet",
                "agent",
                "stop",
                "00000000-0000-4000-8000-000000000001",
            ],
            vec![
                "fleet",
                "agent",
                "tail",
                "00000000-0000-4000-8000-000000000001",
                "--replay",
            ],
            vec!["fleet", "agent", "terminal"],
            vec!["fleet", "agent", "terminal", "codex"],
            vec![
                "fleet",
                "subagent",
                "run",
                "--provider",
                "codex",
                "--brief-file",
                "/tmp/brief.md",
                "--expect",
                "tests pass",
                "--worktree",
                "acme/api#feature",
                "--mode",
                "full-access",
                "--model",
                "gpt-5",
                "--title",
                "worker",
                "--eager",
                "--caller",
                "00000000-0000-4000-8000-000000000001",
                "--json",
            ],
            vec![
                "fleet",
                "subagent",
                "complete",
                "00000000-0000-4000-8000-000000000003",
                "--result-file",
                "/tmp/result.md",
                "--blocked",
                "--json-result",
                "--json",
            ],
            vec![
                "fleet",
                "subagent",
                "wait",
                "00000000-0000-4000-8000-000000000003",
                "--timeout",
                "12",
                "--json",
            ],
            vec![
                "fleet",
                "subagent",
                "status",
                "00000000-0000-4000-8000-000000000003",
                "--json",
            ],
            vec![
                "fleet",
                "subagent",
                "list",
                "--caller",
                "00000000-0000-4000-8000-000000000001",
                "--json",
            ],
            vec![
                "fleet",
                "subagent",
                "cancel",
                "00000000-0000-4000-8000-000000000003",
                "--json",
            ],
            vec!["fleet", "agent-status", "finished", "--json"],
            vec!["fleet", "agent-status", "permission"],
            vec!["fleet", "agent-status", "question"],
            vec!["fleet", "agent-status", "plan"],
            vec!["fleet", "doctor"],
            vec!["fleet", "doctor", "--reset-state", "--json"],
            vec!["fleet", "import", "--from-swarm"],
            vec!["fleet", "update"],
            vec!["fleet", "daemon", "restart"],
            vec!["fleet", "version"],
        ];

        for arguments in cases {
            let _ = parses(&arguments);
        }
    }

    #[test]
    fn agent_new_omits_mode_so_the_daemon_can_apply_its_default() {
        let command = parses(&[
            "fleet",
            "agent",
            "new",
            "acme/api#feature",
            "--provider",
            "claude",
        ]);
        let Command::Agent(AgentArgs {
            command: AgentCommand::New(arguments),
        }) = command
        else {
            panic!("expected agent new");
        };
        assert!(arguments.mode.is_none());
    }

    #[test]
    fn supports_help_and_version_flags() {
        for arguments in [vec!["fleet", "--version"], vec!["fleet", "-v"]] {
            let parsed = Cli::try_parse_from(&arguments)
                .unwrap_or_else(|error| panic!("failed to parse {arguments:?}: {error}"));
            assert!(parsed.version);
        }
        for arguments in [
            vec!["fleet", "--help"],
            vec!["fleet", "-h"],
            vec!["fleet", "create", "--help"],
            vec!["fleet", "create", "-h"],
            vec!["fleet", "clipboard", "copy", "--help"],
        ] {
            let error = Cli::try_parse_from(&arguments).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        }
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("clipboard"));
        Cli::command().debug_assert();
    }

    #[test]
    fn clipboard_copy_parses_stdin_and_argument_forms() {
        assert_eq!(
            parses(&["fleet", "clipboard", "copy"]),
            Command::Clipboard(ClipboardArgs {
                command: ClipboardCommand::Copy(ClipboardCopyArgs { text: None }),
            })
        );
        assert_eq!(
            parses(&["fleet", "clipboard", "copy", "--", " \ttext\n"]),
            Command::Clipboard(ClipboardArgs {
                command: ClipboardCommand::Copy(ClipboardCopyArgs {
                    text: Some(" \ttext\n".to_owned()),
                }),
            })
        );
        assert!(Cli::try_parse_from(["fleet", "clipboard", "copy", "text"]).is_err());
    }

    #[test]
    fn rejects_duplicate_flags_unknown_options_and_missing_values() {
        let invalid = [
            vec!["fleet", "list", "--json", "--json"],
            vec![
                "fleet", "create", "acme/api", "slug", "--branch", "one", "--branch", "two",
            ],
            vec!["fleet", "prune", "--repo"],
            vec!["fleet", "status", "--wat"],
            vec!["fleet", "path", "acme/api#slug", "extra"],
            vec!["fleet", "delete"],
            vec!["fleet", "import"],
            // `--timeout` no longer has a ceiling, but it is still a number.
            vec![
                "fleet",
                "subagent",
                "wait",
                "00000000-0000-4000-8000-000000000003",
                "--timeout",
                "forever",
            ],
            // `--effort` takes a value; a bare flag is still a parse error.
            vec![
                "fleet",
                "subagent",
                "run",
                "--provider",
                "claude",
                "--expect",
                "tests pass",
                "--effort",
            ],
        ];

        for arguments in invalid {
            let error = Cli::try_parse_from(&arguments).unwrap_err();
            assert!(
                !matches!(
                    error.kind(),
                    ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
                ),
                "unexpected display result for {arguments:?}"
            );
        }
    }

    #[test]
    fn watch_commands_parse_options_and_reject_invalid_ids() {
        assert_eq!(
            Cli::try_parse_from(["fleet", "watch", "list"])
                .unwrap()
                .command,
            Some(Command::Watch(WatchArgs {
                command: WatchCommand::List(WatchListArgs {
                    session: None,
                    json: false
                })
            }))
        );
        assert_eq!(
            Cli::try_parse_from(["fleet", "watch", "list", "--session", "repo/main", "--json"])
                .unwrap()
                .command,
            Some(Command::Watch(WatchArgs {
                command: WatchCommand::List(WatchListArgs {
                    session: Some("repo/main".parse().unwrap()),
                    json: true
                })
            }))
        );
        for follow in [false, true] {
            let mut arguments = vec!["fleet", "watch", "tail", "42"];
            if follow {
                arguments.push("--follow");
            }
            assert_eq!(
                Cli::try_parse_from(arguments).unwrap().command,
                Some(Command::Watch(WatchArgs {
                    command: WatchCommand::Tail(WatchTailArgs {
                        id: fleet_core::watches::WatchId(42),
                        follow
                    })
                }))
            );
        }
        for arguments in [
            vec!["fleet", "watch"],
            vec!["fleet", "watch", "list", "--session"],
            vec!["fleet", "watch", "list", "--session", "invalid"],
            vec!["fleet", "watch", "tail"],
            vec!["fleet", "watch", "tail", "invalid"],
            vec!["fleet", "watch", "tail", "-1"],
            vec!["fleet", "watch", "tail", "42", "--json"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn exec_keeps_child_flags_and_delimiters() {
        let cli = Cli::try_parse_from([
            "fleet", "exec", "--watch", "--label", "review", "--", "codex", "exec", "--json", "--",
            "a b",
        ])
        .unwrap();
        let Some(Command::Exec(args)) = cli.command else {
            panic!("expected exec");
        };
        assert!(args.watch);
        assert_eq!(args.label.as_deref(), Some("review"));
        assert_eq!(
            args.command,
            ["codex", "exec", "--json", "--", "a b"].map(std::ffi::OsString::from)
        );
        assert!(Cli::try_parse_from(["fleet", "exec", "--", "true"]).is_ok());
        assert!(Cli::try_parse_from(["fleet", "exec", "--watch"]).is_err());
        assert!(Cli::try_parse_from(["fleet", "exec", "sh"]).is_err());
    }
}

/// Board selection and nested operations.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct BoardArgs {
    /// Explicit board ID; mutually exclusive with --context and --worktree.
    #[arg(long, global = true, conflicts_with_all = ["context", "worktree"])]
    pub board: Option<fleet_core::ids::BoardId>,
    /// Worktree ID, or the current terminal's worktree when no ID is given.
    #[arg(
        long,
        global = true,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "@session",
        conflicts_with_all = ["board", "context"]
    )]
    pub worktree: Option<BoardWorktreeSelector>,
    /// Context ID; defaults to the daemon's active context.
    #[arg(long, global = true, conflicts_with_all = ["board", "worktree"])]
    pub context: Option<fleet_core::ids::ContextId>,
    /// Emit a protocol-one JSON envelope.
    #[arg(long, global = true)]
    pub json: bool,
    /// Board operation.
    #[command(subcommand)]
    pub command: BoardCommand,
}

/// Worktree selected explicitly or inferred from the current Fleet session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardWorktreeSelector {
    /// A worktree named directly on the command line.
    Explicit(fleet_core::ids::WorktreeId),
    /// The worktree owning the session named by `FLEET_SESSION`.
    FromSession,
}

impl std::str::FromStr for BoardWorktreeSelector {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "@session" {
            return Ok(Self::FromSession);
        }
        fleet_core::ids::WorktreeId::try_from(value)
            .map(Self::Explicit)
            .map_err(|error| error.to_string())
    }
}

/// Operations accepted by `fleet board`.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum BoardCommand {
    /// Show columns and their cards.
    Show,
    /// List board summaries across context and worktree scopes.
    List,
    /// List registered backend kinds, their capabilities, and their setting keys.
    Backends,
    /// Print what the board's backend reports: statuses, labels, properties, read-only fields.
    Describe,
    /// Create a board for a context or worktree.
    Create(BoardCreateArgs),
    /// Update board properties and settings.
    Set(BoardSetArgs),
    /// Synchronize the board with its backend.
    Sync {
        /// Wait for completion and print the summary or last error.
        #[arg(long)]
        wait: bool,
        /// Ignore the incremental cursor and pull the backend's complete set.
        #[arg(long)]
        full: bool,
    },
    /// Inspect and edit the board's columns and the automation they carry.
    Columns(BoardColumnsArgs),
    /// Create, inspect, and update cards.
    Card(BoardCardArgs),
}

/// Nested column operations.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct BoardColumnsArgs {
    /// Column operation; without one the columns are listed.
    #[command(subcommand)]
    pub command: Option<BoardColumnsCommand>,
}

/// Operations accepted by `fleet board columns`.
///
/// Every verb is a read-modify-write of the whole column vector and sends one `UpdateBoard`:
/// a concurrent editor loses, exactly as `board set` already behaves.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum BoardColumnsCommand {
    /// Add a column.
    Add {
        /// Column name.
        name: String,
        /// Column ID; defaults to a slug of the name.
        #[arg(long)]
        id: Option<String>,
        /// Category the backend sorts and reports the column under.
        #[arg(long, value_enum)]
        category: Option<BoardStatusCategory>,
        /// Insert after this column ID or name.
        #[arg(long, conflicts_with = "before")]
        after: Option<String>,
        /// Insert before this column ID or name.
        #[arg(long)]
        before: Option<String>,
    },
    /// Edit a column and the automation it carries.
    Edit {
        /// Column ID or name (case-insensitive).
        id: String,
        /// Replacement name.
        #[arg(long)]
        name: Option<String>,
        /// Replacement category.
        #[arg(long, value_enum)]
        category: Option<BoardStatusCategory>,
        /// Replacement color, as the backend spells it.
        #[arg(long)]
        color: Option<String>,
        /// What entering the column starts: `none`, `prompt`, or `skill:<name>[:<args>]`.
        #[arg(long = "on-enter", value_name = "ACTION")]
        on_enter: Option<String>,
        /// Provider the column's runs launch.
        #[arg(long, value_enum)]
        provider: Option<AgentChoice>,
        /// Provider-native model for the column's runs.
        #[arg(long)]
        model: Option<String>,
        /// Provider-native reasoning effort for the column's runs.
        ///
        /// Free text, never an enum: the legal ladder is per provider and per model, so Fleet
        /// passes whatever is given straight through.
        #[arg(long)]
        effort: Option<String>,
        /// Permission mode the column's runs start in.
        #[arg(long, value_enum)]
        mode: Option<AgentModeChoice>,
        /// Markdown prepended to every brief this column starts; `{key}` and `{title}` are
        /// substituted.
        #[arg(long, conflicts_with = "instructions_file")]
        instructions: Option<String>,
        /// Read the instructions from this file instead.
        #[arg(long)]
        instructions_file: Option<std::path::PathBuf>,
        /// Completion criteria printed in the brief's footer.
        #[arg(long = "expect")]
        expect: Option<String>,
        /// Column a succeeding run moves the card into.
        #[arg(long = "on-success", conflicts_with = "no_on_success")]
        on_success: Option<String>,
        /// Clear the success route.
        #[arg(long = "no-on-success")]
        no_on_success: bool,
        /// Column a card here moves into once nothing blocks it.
        #[arg(long = "when-unblocked", conflicts_with = "no_when_unblocked")]
        when_unblocked: Option<String>,
        /// Clear the unblocked route.
        #[arg(long = "no-when-unblocked")]
        no_when_unblocked: bool,
        /// Environment variable for the column's runs, repeatable: `--env KEY=VALUE`.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Clear the column's environment before applying any `--env`.
        #[arg(long)]
        clear_env: bool,
    },
    /// Move a column; one of `--after` and `--before` is required.
    Move {
        /// Column ID or name (case-insensitive).
        id: String,
        /// Place after this column ID or name.
        #[arg(long, conflicts_with = "before", required_unless_present = "before")]
        after: Option<String>,
        /// Place before this column ID or name.
        #[arg(long)]
        before: Option<String>,
    },
    /// Remove a column.
    Remove {
        /// Column ID or name (case-insensitive).
        id: String,
        /// Move the column's cards into this column first; without it the daemon refuses to
        /// remove a column a card still uses.
        #[arg(long, value_name = "ID")]
        move_cards_to: Option<String>,
    },
    /// Add the columns a named preset expects, leaving existing ones untouched.
    Preset {
        /// Preset to apply.
        #[arg(value_enum)]
        which: BoardColumnsPreset,
    },
}

/// Column categories with the contract's snake_case CLI spelling.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum BoardStatusCategory {
    /// Not yet planned.
    Backlog,
    /// Planned, not started.
    Unstarted,
    /// In progress.
    Started,
    /// Finished.
    Completed,
    /// Abandoned.
    Canceled,
}

/// Column presets `fleet board columns preset` applies.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum BoardColumnsPreset {
    /// The workflow columns: the automation lane a card travels from Ready to Done.
    Workflow,
}

/// Splits one `--setting key=value` pair, leaving the value untouched for
/// [`fleet_core::board::merge_settings`] to parse as JSON or keep as a string.
fn parse_setting(value: &str) -> Result<(String, String), String> {
    let (key, value) = value
        .split_once('=')
        .ok_or_else(|| "a setting must be written key=value".to_owned())?;
    let key = key.trim();
    if key.is_empty() {
        return Err("a setting key must not be empty".to_owned());
    }
    Ok((key.to_owned(), value.to_owned()))
}

/// New board properties.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct BoardCreateArgs {
    /// Board name; defaults to the context name or worktree slug.
    #[arg(long)]
    pub name: Option<String>,
    /// Uppercase identifier prefix.
    #[arg(long)]
    pub prefix: Option<String>,
    /// Registered backend kind; defaults to local.
    #[arg(long)]
    pub backend: Option<String>,
    /// Backend setting as `key=value`; repeat for more. Values parse as JSON when they are
    /// valid JSON, else as strings. Requires `--backend`.
    #[arg(long = "setting", value_name = "KEY=VALUE", value_parser = parse_setting, requires = "backend")]
    pub settings: Vec<(String, String)>,
}

/// Editable board properties.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct BoardSetArgs {
    /// New board name.
    #[arg(long)]
    pub name: Option<String>,
    /// New uppercase identifier prefix.
    #[arg(long)]
    pub prefix: Option<String>,
    /// Default repository in owner/name form.
    #[arg(long)]
    pub default_repo: Option<fleet_core::ids::RepoId>,
    /// Clear the default repository.
    #[arg(long, conflicts_with = "default_repo")]
    pub clear_default_repo: bool,
    /// Move unstarted cards into progress when creating a worktree
    /// (`--start-on-worktree`, `--start-on-worktree false`).
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub start_on_worktree: Option<bool>,
    /// Policy for conflicting local and remote changes.
    #[arg(long, value_enum)]
    pub conflict_policy: Option<BoardConflictPolicy>,
    /// Whether cards created here are pushed to the backend as new remote issues
    /// (`--push-new-cards`, `--push-new-cards false`).
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub push_new_cards: Option<bool>,
    /// Branch name template for worktrees started from a card, e.g. `{key}-{slug}`.
    #[arg(long)]
    pub branch_template: Option<String>,
    /// Live card runs allowed at once across this board; defaults to 1.
    #[arg(long)]
    pub max_live_runs: Option<u32>,
    /// Label to add to the board; repeat for multiple labels.
    #[arg(long = "add-label")]
    pub add_labels: Vec<String>,
    /// Label ID or name to remove from the board; repeat for multiple labels.
    #[arg(long = "remove-label")]
    pub remove_labels: Vec<String>,
    /// Registered backend kind. Changing it starts the backend settings from empty, so every
    /// setting the new kind needs must be given in the same command.
    #[arg(long)]
    pub backend: Option<String>,
    /// Backend setting as `key=value`; repeat for more. Values parse as JSON when they are
    /// valid JSON, else as strings. Without `--backend` they merge into the current settings.
    #[arg(long = "setting", value_name = "KEY=VALUE", value_parser = parse_setting)]
    pub settings: Vec<(String, String)>,
}

/// Conflict policies with the contract's snake_case CLI spelling.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
#[value(rename_all = "snake_case")]
pub enum BoardConflictPolicy {
    /// Require an explicit resolution.
    Manual,
    /// Prefer remote changes.
    RemoteWins,
    /// Prefer local changes.
    LocalWins,
}

/// Card priority choices, from highest to lowest.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum BoardPriority {
    /// Urgent work.
    Urgent,
    /// High priority.
    High,
    /// Medium priority.
    Medium,
    /// Low priority.
    Low,
    /// No priority assigned.
    None,
}

/// Explicit resolution of a card conflict.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum BoardResolution {
    /// Keep the local version.
    KeepLocal,
    /// Accept the remote version.
    TakeRemote,
}

/// Nested card operations.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct BoardCardArgs {
    /// Card operation.
    #[command(subcommand)]
    pub command: BoardCardCommand,
}

/// Operations accepted by `fleet board card`.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum BoardCardCommand {
    /// Create a card.
    New {
        /// Card title.
        title: String,
        /// Initial card properties.
        #[command(flatten)]
        fields: BoardCardFields,
        /// Card that must finish before this one may advance; repeat for more.
        #[arg(long = "blocked-by", value_name = "KEY")]
        blocked_by: Vec<String>,
        /// Card this one blocks; repeat for more. Sugar for editing that card's `blocked-by`.
        #[arg(long = "blocks", value_name = "KEY")]
        blocks: Vec<String>,
    },
    /// Show card properties, description, and comments.
    Show {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
    },
    /// Edit selected properties of a card.
    Edit {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
        /// Replacement title.
        #[arg(long)]
        title: Option<String>,
        /// Properties to replace; omitted properties remain unchanged.
        #[command(flatten)]
        fields: BoardCardFields,
        /// Archive (`--archive`, `--archive true`) or restore (`--archive false`) the card.
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        archive: Option<bool>,
        /// Card to add to this card's blockers; repeat for more.
        #[arg(long = "add-blocked-by", value_name = "KEY")]
        add_blocked_by: Vec<String>,
        /// Card to remove from this card's blockers; repeat for more.
        #[arg(long = "remove-blocked-by", value_name = "KEY")]
        remove_blocked_by: Vec<String>,
        /// Clear this card's blockers.
        #[arg(long, conflicts_with_all = ["add_blocked_by", "remove_blocked_by"])]
        clear_blocked_by: bool,
        /// Card this one blocks; repeat for more. Sugar for editing that card's `blocked-by`.
        #[arg(long = "add-blocks", value_name = "KEY")]
        add_blocks: Vec<String>,
        /// Card this one stops blocking; repeat for more.
        #[arg(long = "remove-blocks", value_name = "KEY")]
        remove_blocks: Vec<String>,
    },
    /// Move a card into a status column.
    Move {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
        /// Status ID or name.
        status: String,
        /// Position in the target column; defaults to the end.
        #[arg(long)]
        index: Option<usize>,
        /// Cancel the card's live run instead of refusing the move.
        #[arg(long)]
        cancel_run: bool,
    },
    /// Start a run for a card sitting in an action column.
    Run {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
    },
    /// Cancel the card's live run.
    Cancel {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
    },
    /// List the card's runs, newest last.
    Runs {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
    },
    /// Print the thread the card's run is talking in.
    ///
    /// The live run's thread, or the newest run's when none is live.
    Attach {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
    },
    /// Wait for the card's newest run to finish.
    ///
    /// Exits 0 once that run is terminal, and 2 when it is still live or none started within
    /// the timeout — the pair an orchestrator scripts against.
    Wait {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
        /// Maximum wait in seconds; defaults to 540.
        ///
        /// 540 is a default, not a ceiling: it sits under the Claude Code shell-tool timeout so
        /// the common caller outlives its own wait.
        #[arg(long, default_value_t = 540)]
        timeout: u64,
    },
    /// Add a comment.
    Comment {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
        /// Comment text.
        body: String,
    },
    /// Delete a card.
    Delete {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
    },
    /// Create a worktree linked to a card.
    Worktree {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
        /// Repository in owner/name form; overrides the card and board defaults.
        #[arg(long)]
        repo: Option<fleet_core::ids::RepoId>,
        /// Base Git ref.
        #[arg(long)]
        base: Option<String>,
        /// Host ID; local uses the local host.
        #[arg(long)]
        host: Option<String>,
    },
    /// Resolve a synchronization conflict.
    Resolve {
        /// Display key, local key, or card ID (case-insensitive).
        key: String,
        /// Which version to keep.
        #[arg(value_enum)]
        resolution: BoardResolution,
    },
}

/// Common fields for card creation and editing.
#[derive(Debug, Default, Args, PartialEq, Eq)]
pub struct BoardCardFields {
    /// Markdown description.
    #[arg(long)]
    pub desc: Option<String>,
    /// Read the description from this file instead.
    #[arg(long, conflicts_with = "desc")]
    pub desc_file: Option<std::path::PathBuf>,
    /// Status ID or name.
    #[arg(long)]
    pub status: Option<String>,
    /// Card priority.
    #[arg(long, value_enum)]
    pub priority: Option<BoardPriority>,
    /// Label ID or name; repeat for multiple labels.
    #[arg(long = "label")]
    pub labels: Vec<String>,
    /// Clear labels.
    #[arg(long, conflicts_with = "labels")]
    pub clear_labels: bool,
    /// Assignee name.
    #[arg(long)]
    pub assignee: Option<String>,
    /// Clear assignee.
    #[arg(long, conflicts_with = "assignee")]
    pub clear_assignee: bool,
    /// Estimate in points.
    #[arg(long)]
    pub estimate: Option<u32>,
    /// Clear estimate.
    #[arg(long, conflicts_with = "estimate")]
    pub clear_estimate: bool,
    /// Due date in YYYY-MM-DD form.
    #[arg(long)]
    pub due: Option<String>,
    /// Clear due.
    #[arg(long, conflicts_with = "due")]
    pub clear_due: bool,
    /// Repository in owner/name form.
    #[arg(long)]
    pub repo: Option<fleet_core::ids::RepoId>,
    /// Clear repo.
    #[arg(long, conflicts_with = "repo")]
    pub clear_repo: bool,
    /// Provider this card's runs launch; without it the column's provider is used.
    #[arg(long, value_enum)]
    pub provider: Option<AgentChoice>,
    /// Provider-native model for this card's runs.
    #[arg(long)]
    pub model: Option<String>,
    /// Provider-native reasoning effort for this card's runs.
    ///
    /// Free text, never an enum: the legal ladder is per provider and per model, so Fleet
    /// passes whatever is given straight through.
    #[arg(long)]
    pub effort: Option<String>,
    /// Clear the card's agent preferences, leaving the column's.
    #[arg(long, conflicts_with_all = ["provider", "model", "effort"])]
    pub clear_agent: bool,
}

impl BoardCardFields {
    /// The first `--clear-*` flag that was supplied, if any.
    #[must_use]
    pub const fn clear_flag(&self) -> Option<&'static str> {
        match () {
            () if self.clear_labels => Some("--clear-labels"),
            () if self.clear_assignee => Some("--clear-assignee"),
            () if self.clear_estimate => Some("--clear-estimate"),
            () if self.clear_due => Some("--clear-due"),
            () if self.clear_repo => Some("--clear-repo"),
            () if self.clear_agent => Some("--clear-agent"),
            () => None,
        }
    }
}
