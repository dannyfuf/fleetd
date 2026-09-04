//! Entry point for the long-lived Fleet daemon process.

use std::{path::PathBuf, sync::Arc};

use clap::Parser;
use fleet_core::paths::FleetHome;
use fleet_daemon::{
    adapters::{
        clock::SystemClock,
        files::{Files, RealFiles},
        git::ShellGit,
        github::GhCli,
        process::RealProcess,
        shell::{RealShell, Shell},
    },
    jobs::JobManager,
    server::{BroadcastBus, Listener},
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::fmt::writer::MakeWriterExt;

#[derive(Debug, Parser)]
#[command(name = "fleetd", about = "Fleet background daemon")]
struct Args {
    /// Fleet's configuration and data directory.
    #[arg(long, env = "FLEET_HOME")]
    home: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let home = match args.home {
        Some(home) => home,
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".fleet"))
            .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass --home or FLEET_HOME"))?,
    };
    let layout = FleetHome::new(home.clone());
    std::fs::create_dir_all(layout.logs_dir())?;
    let file = tracing_appender::rolling::never(layout.logs_dir(), "fleetd.log");
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
    let state = Arc::new(StateStore::new(&home, files, Arc::new(SystemClock)));
    let jobs = Arc::new(JobManager::new(&home));

    // Construct every real command boundary now; service implementation agents can inject these
    // handles into their service structs without changing command semantics.
    let shell: Arc<dyn Shell> = Arc::new(RealShell);
    let _git = ShellGit::new(Arc::clone(&shell));
    let _github = GhCli::new(Arc::clone(&shell));
    let _process = RealProcess::new(shell);

    let services = Arc::new(Services::new(&home, config, state, jobs));
    let shutdown = CancellationToken::new();
    let listener =
        Listener::bind(&home, services, BroadcastBus::default(), shutdown.clone()).await?;

    tracing::info!(home = %home.display(), socket = %listener.socket_path().display(), "fleetd started");
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        if let Err(error) = wait_for_shutdown_signal().await {
            tracing::error!(%error, "failed to install shutdown signal handler");
        }
        signal_shutdown.cancel();
    });
    listener.run().await?;
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
