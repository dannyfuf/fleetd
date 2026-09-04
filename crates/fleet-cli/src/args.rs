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
    /// Ensure a repository-level coding-agent session exists.
    Agent(AgentArgs),
    /// Run environment diagnostics.
    Doctor,
    /// Import compatible swarm configuration and state.
    Import(ImportArgs),
    /// Start a Fleet self-update job.
    Update,
    /// Print Fleet's build version.
    Version,
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
    /// Agent to launch; defaults to config.agent.
    #[arg(value_enum)]
    pub agent: Option<AgentChoice>,
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
            vec!["fleet", "agent", "opencode"],
            vec!["fleet", "doctor"],
            vec!["fleet", "import", "--from-swarm"],
            vec!["fleet", "update"],
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
}
