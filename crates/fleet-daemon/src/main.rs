//! Entry point for the long-lived Fleet daemon process.

use std::{path::PathBuf, sync::Arc};

use clap::{Args as ClapArgs, Parser, Subcommand};
use fleet_core::{ids::TerminalId, paths::FleetHome};
use fleet_daemon::{
    adapters::{
        Adapters,
        clock::SystemClock,
        files::{Files, RealFiles},
        logs::RotatingLog,
    },
    jobs::JobManager,
    server::{BroadcastBus, Listener, SingletonGuard},
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, fmt::writer::MakeWriterExt};

#[derive(Debug, Parser)]
#[command(name = "fleetd", about = "Fleet background daemon", version)]
struct Args {
    /// Fleet's configuration and data directory.
    #[arg(long, env = "FLEET_HOME", global = true)]
    home: Option<PathBuf>,
    /// Process mode; omitted starts the long-lived daemon.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Bridge standard input/output to this Fleet home's daemon socket.
    Connect,
    /// Hold one terminal's PTY in a detached process so it outlives this daemon.
    ///
    /// Started by `fleetd` itself, never by a user. See `docs/ARCHITECTURE.md`,
    /// "Detached PTY holders".
    #[command(hide = true)]
    PtyHold(PtyHoldArgs),
}

/// One held terminal, named exactly as the daemon's registry names it.
#[derive(Debug, ClapArgs)]
struct PtyHoldArgs {
    /// Terminal identifier, reused by the daemon that reattaches.
    #[arg(long)]
    terminal: u64,
    /// Owning session identifier, exported to the shell as `FLEET_SESSION`.
    #[arg(long)]
    session: String,
    /// Session-local terminal name, exported to the shell as `FLEET_TERMINAL`.
    #[arg(long)]
    name: String,
    /// Working directory the login shell starts in.
    #[arg(long)]
    cwd: PathBuf,
    /// Socket to bind.
    ///
    /// Chosen by the daemon, not derived here: the name carries a per-spawn nonce, and a home too
    /// deep for `sun_path` keeps its sockets elsewhere entirely.
    #[arg(long)]
    socket: PathBuf,
    /// Record to remove when this holder exits.
    #[arg(long)]
    sidecar: Option<PathBuf>,
    /// Initial grid width.
    #[arg(long, default_value_t = 120)]
    cols: u16,
    /// Initial grid height.
    #[arg(long, default_value_t = 36)]
    rows: u16,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    // A holder owns a PTY and a socket and nothing else. It runs before any runtime starts, so it
    // carries neither tokio's threads nor the daemon's services into a process meant to outlive
    // them both.
    if let Some(Command::PtyHold(hold)) = args.command {
        return run_pty_hold(args.home, hold);
    }
    run_daemon(args)
}

#[tokio::main]
async fn run_daemon(args: Args) -> anyhow::Result<()> {
    if matches!(args.command, Some(Command::Connect)) {
        if let Err(error) = fleet_daemon::server::bridge::run_connect(args.home).await {
            eprintln!("{error}");
            std::process::exit(2);
        }
        // Tokio's stdin reader uses a blocking helper that can outlive a closed daemon socket.
        // This bridge process has no state to flush once relay finishes, so exit immediately.
        std::process::exit(0);
    }
    let home = fleet_core::paths::resolve_home(args.home)?;
    let layout = FleetHome::new(home.clone());
    let singleton = SingletonGuard::acquire(&home).await?;
    std::fs::create_dir_all(layout.logs_dir())?;
    let file = RotatingLog::new(layout.logs_dir().join("fleetd.log"), 10 * 1024 * 1024, 4)?;
    let (file_writer, _log_guard) = tracing_appender::non_blocking(file);
    // `$RUST_LOG` selects the daemon's level the way it selects the app's; without a filter the
    // builder caps itself at INFO and every `debug!`/`trace!` site here is unreachable at runtime.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer((|| std::io::stderr()).and(file_writer))
        .init();

    let bootstrap_files: Arc<dyn Files> = Arc::new(RealFiles::new(
        layout.trash_dir(),
        [layout.repos_dir(), layout.worktrees_dir()],
    ));
    let bootstrap_config = ConfigStore::new(&home, bootstrap_files);
    let effective = bootstrap_config.load().await?;
    let files: Arc<dyn Files> = Arc::new(RealFiles::new(
        layout.trash_dir(),
        [
            PathBuf::from(&effective.repos_dir),
            PathBuf::from(&effective.worktrees_dir),
        ],
    ));
    let config = Arc::new(ConfigStore::new(&home, Arc::clone(&files)));
    let state = Arc::new(StateStore::new(
        &home,
        Arc::clone(&files),
        Arc::new(SystemClock),
    ));
    let jobs = Arc::new(JobManager::new(&home));
    let adapters = Adapters::system(files);
    let shutdown = CancellationToken::new();
    let events = BroadcastBus::default();
    let services = Services::new_with_events(&home, config, state, jobs, adapters, events.clone());
    let listener = Listener::bind_owned(
        singleton,
        Arc::clone(&services),
        events.clone(),
        shutdown.clone(),
    )
    .await?;
    // Bound but not yet serving: a client's connection waits in the backlog while the holders that
    // outlived the previous daemon come back, so the first snapshot anyone sees already lists them
    // and no client is kept waiting on a socket that does not exist yet.
    match services.sessions.adopt_holders().await {
        Ok(0) => {}
        Ok(adopted) => tracing::info!(adopted, "reattached surviving terminals"),
        Err(error) => tracing::warn!(%error, "failed to reattach surviving terminals"),
    }
    let periodic = services
        .start_periodic_tasks(events, shutdown.clone())
        .await?;

    tracing::info!(home = %home.display(), socket = %listener.socket_path().display(), "fleetd started");
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        if let Err(error) = wait_for_shutdown_signal().await {
            tracing::error!(%error, "failed to install shutdown signal handler");
        }
        signal_shutdown.cancel();
    });
    let result = listener.run().await;
    shutdown.cancel();
    periodic.join().await;
    result?;
    tracing::info!("fleetd stopped");
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() -> std::io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _signal = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}

/// Runs one detached PTY holder until its child exits.
fn run_pty_hold(home: Option<PathBuf>, args: PtyHoldArgs) -> anyhow::Result<()> {
    let home = fleet_core::paths::resolve_home(home)?;
    let _layout = FleetHome::new(home);
    let terminal = TerminalId(args.terminal);
    // The daemon redirects this process's stdio to `logs/pty-hold.log`, so logging to stderr is
    // what puts a holder's own diagnostics in that file.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let options = fleet_term::HolderOptions {
        socket: args.socket,
        sidecar: args.sidecar,
        pty: fleet_term::PtyOptions::shell(
            args.cwd,
            &args.session,
            &args.name,
            terminal,
            args.cols,
            args.rows,
        ),
        replay_bytes: fleet_term::REPLAY_BUFFER_BYTES,
    };
    match fleet_term::holder::run(options) {
        Ok(code) => {
            tracing::info!(%terminal, code, "pty holder finished");
            // `io::stderr` is unbuffered, so the line above has already reached the holder log.
            std::process::exit(code.unwrap_or(0));
        }
        Err(error) => {
            tracing::error!(%error, %terminal, "pty holder failed");
            Err(error.into())
        }
    }
}
