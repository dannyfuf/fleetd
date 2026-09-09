//! Entry point for the long-lived Fleet daemon process.

use std::{path::PathBuf, sync::Arc};

use clap::{Parser, Subcommand};
use fleet_core::paths::FleetHome;
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
use tracing_subscriber::fmt::writer::MakeWriterExt;

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if matches!(args.command, Some(Command::Connect)) {
        if let Err(error) = fleet_daemon::server::bridge::run_connect(args.home).await {
            eprintln!("{error}");
            std::process::exit(2);
        }
        // Tokio's stdin reader uses a blocking helper that can outlive a closed daemon socket.
        // This bridge process has no state to flush once relay finishes, so exit immediately.
        std::process::exit(0);
    }
    let home = fleet_daemon::server::bridge::resolve_home(args.home)?;
    let layout = FleetHome::new(home.clone());
    let singleton = SingletonGuard::acquire(&home).await?;
    std::fs::create_dir_all(layout.logs_dir())?;
    let file = RotatingLog::new(layout.logs_dir().join("fleetd.log"), 10 * 1024 * 1024, 4)?;
    let (file_writer, _log_guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
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
