//! Fleet home and Unix socket path resolution.

use std::path::{Path, PathBuf};

use fleet_core::paths::FleetHome;

/// Returns the daemon Unix socket path below a Fleet home directory.
#[must_use]
pub fn socket_path(home: impl AsRef<Path>) -> PathBuf {
    FleetHome::new(home.as_ref()).socket_path()
}

/// Returns the daemon PID file path below a Fleet home directory.
#[must_use]
pub fn pid_path(home: impl AsRef<Path>) -> PathBuf {
    FleetHome::new(home.as_ref()).pid_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegates_daemon_paths_to_core_layout() {
        assert_eq!(
            socket_path("/tmp/fleet"),
            PathBuf::from("/tmp/fleet/fleetd.sock")
        );
        assert_eq!(
            pid_path("/tmp/fleet"),
            PathBuf::from("/tmp/fleet/fleetd.pid")
        );
    }
}
