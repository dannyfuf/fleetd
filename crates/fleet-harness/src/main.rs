use anyhow::Context as _;
use clap::{Parser, Subcommand, ValueEnum};
use fleet_harness::{
    agent,
    lane::Lane,
    scenario::{self, RunOptions},
};
use std::path::PathBuf;

/// Drives the Fleet GUI end to end and leaves a run directory of evidence behind.
#[derive(Debug, Parser)]
#[command(name = "fleet-harness")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run one scenario file, or every scenario under a directory in lexical order.
    Run {
        /// The scenario file or directory of scenarios to execute.
        scenario_or_directory: PathBuf,
        /// Where the window lives: no pixels, an isolated output, or this session.
        #[arg(long, value_enum, default_value_t = LaneArg::Virtual)]
        lane: LaneArg,
        /// Keep the hermetic FLEET_HOME after a clean run; a failed run always keeps it.
        ///
        /// The run directory itself is always kept: it is the evidence. Teardown stops
        /// processes and display lanes unconditionally either way.
        #[arg(long)]
        keep: bool,
        /// Write artifacts here instead of under /tmp/fleet-harness/.
        #[arg(long)]
        run_dir: Option<PathBuf>,
        /// Keep going after a failed line instead of stopping the scenario.
        #[arg(long)]
        continue_on_failure: bool,
        /// Replace the stored screenshot baselines with this run's captures.
        #[arg(long)]
        update_baselines: bool,
    },
    /// Speak a provider's native wire protocol from a scripted transcript.
    Agent {
        /// Which native agent protocol to imitate.
        #[arg(long, value_enum)]
        provider: ProviderArg,
        /// The transcript document to replay.
        #[arg(long)]
        transcript: PathBuf,
        /// Print the version the provider's real binary answers with, and exit.
        ///
        /// `fleetd` probes a configured agent command with `<command> --version` before it
        /// speaks the protocol, and refuses a Claude older than 2.1.
        #[arg(long)]
        version: bool,
        /// The vendor launch line the adapter appends — `app-server`, the stream-json flags —
        /// accepted and ignored, so a direct `fleetd` configuration does not fail on parsing.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        vendor_args: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LaneArg {
    Headless,
    Virtual,
    Attach,
}
impl From<LaneArg> for Lane {
    fn from(value: LaneArg) -> Self {
        match value {
            LaneArg::Headless => Self::Headless,
            LaneArg::Virtual => Self::Virtual,
            LaneArg::Attach => Self::Attach,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProviderArg {
    Claude,
    Codex,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Commands::Run {
            scenario_or_directory,
            lane,
            keep,
            run_dir,
            continue_on_failure,
            update_baselines,
        } => {
            scenario::run_path(
                &scenario_or_directory,
                RunOptions {
                    lane: lane.into(),
                    keep,
                    run_dir,
                    continue_on_failure,
                    update_baselines,
                },
            )
            .await
        }
        Commands::Agent {
            provider,
            transcript,
            version,
            vendor_args,
        } => {
            let provider = match provider {
                ProviderArg::Claude => agent::Provider::Claude,
                ProviderArg::Codex => agent::Provider::Codex,
            };
            if version {
                println!("{}", provider.version_line());
                return Ok(());
            }
            if !vendor_args.is_empty() {
                eprintln!("fleet-harness agent: ignoring the vendor launch line {vendor_args:?}");
            }
            agent::run_transcript(provider, &transcript)
                .await
                .context("run scripted agent")
        }
    }
}
