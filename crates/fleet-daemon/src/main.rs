//! Entry point for the long-lived Fleet daemon process.

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "fleetd", about = "Fleet background daemon")]
struct Args {
    /// Fleet's configuration and data directory.
    #[arg(long, env = "FLEET_HOME")]
    home: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let args = Args::parse();

    tracing::debug!(home = ?args.home, "resolved Fleet home");
    println!("fleetd starting");
    Ok(())
}
