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

/// A daemon-backed Fleet operation.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Manage configured remote machines.
    Host(HostArgs),
    /// Manage context boards and their cards.
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
    /// OpenCode.
    Opencode,
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
    Tail(AgentTailArgs),
    /// Open the legacy PTY agent session for this repository (the §10 fallback).
    Terminal(AgentTerminalArgs),
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
    /// Provider-native model, optionally `provider/model` for OpenCode.
    #[arg(long)]
    pub model: Option<String>,
    /// Initial permission or plan mode.
    #[arg(long, value_enum, default_value_t = AgentModeChoice::Ask)]
    pub mode: AgentModeChoice,
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
                "opencode",
                "--model",
                "anthropic/claude-sonnet-4",
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
            vec!["fleet", "agent", "terminal", "opencode"],
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
        ] {
            let error = Cli::try_parse_from(&arguments).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        }
        Cli::command().debug_assert();
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
    /// Explicit board ID; mutually exclusive with --context.
    #[arg(long, global = true, conflicts_with = "context")]
    pub board: Option<fleet_core::ids::BoardId>,
    /// Context ID; defaults to the daemon's active context.
    #[arg(long, global = true, conflicts_with = "board")]
    pub context: Option<fleet_core::ids::ContextId>,
    /// Emit a protocol-one JSON envelope.
    #[arg(long, global = true)]
    pub json: bool,
    /// Board operation.
    #[command(subcommand)]
    pub command: BoardCommand,
}

/// Operations accepted by `fleet board`.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum BoardCommand {
    /// Show columns and their cards.
    Show,
    /// List board summaries across contexts.
    List,
    /// List registered backend kinds, their capabilities, and their setting keys.
    Backends,
    /// Print what the board's backend reports: statuses, labels, properties, read-only fields.
    Describe,
    /// Create a board for a context.
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
    /// Create, inspect, and update cards.
    Card(BoardCardArgs),
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
    /// Board name; defaults to the context name.
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
            () => None,
        }
    }
}
