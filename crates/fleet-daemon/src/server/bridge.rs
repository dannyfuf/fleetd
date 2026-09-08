//! Standard-input/output bridge to a daemon socket.

use std::path::PathBuf;

/// Runs the remote stdio bridge. The link stage supplies the transport body.
pub async fn run_connect(_home: Option<PathBuf>) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("fleetd connect: not implemented"))
}
