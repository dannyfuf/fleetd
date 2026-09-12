//! Shared RAII fixtures for daemon integration tests.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use fleet_core::{ids::HostId, model::HostConfigEntry};
use fleet_daemon::machines::CommandMachine;

const DEADLINE: Duration = Duration::from_secs(5);

pub struct DaemonProcess {
    child: Child,
    /// Fleet home, so teardown can also end the PTY holders this daemon started.
    home: PathBuf,
}

impl DaemonProcess {
    pub fn start(home: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_fleetd"))
            .arg("--home")
            .arg(home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start fleetd");
        Self {
            child,
            home: home.to_path_buf(),
        }
    }

    pub async fn wait(&mut self) {
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let Some(status) = self.child.try_wait().expect("wait for fleetd") {
                    assert!(status.success(), "fleetd exited with {status}");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("fleetd did not shut down within five seconds");
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        stop_holders(&self.home);
    }
}

/// Ends every PTY holder this Fleet home recorded.
///
/// Holders outlive their daemon on purpose, so killing the daemon process is not teardown: a test
/// that ensures a session would otherwise leave a login shell running for the life of the machine.
/// Closing the holder closes the PTY master, which hangs up the shell exactly as a lost daemon used
/// to. A test that means to exercise the shutdown path should still kill its sessions explicitly.
fn stop_holders(home: &Path) {
    let directory = fleet_core::paths::FleetHome::new(home).pty_dir();
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(pid) = record["holderPid"]
            .as_u64()
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        else {
            continue;
        };
        // SAFETY: `kill` takes scalar arguments; this pid came from a record this test's own
        // daemon wrote under its own temporary Fleet home.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

/// A second daemon plus a loopback command-machine configuration for two-daemon tests.
pub struct RemoteDaemon {
    pub daemon: DaemonProcess,
    pub machine: CommandMachine,
    pub config: HostConfigEntry,
}

impl RemoteDaemon {
    pub fn start(home: &Path) -> Self {
        // Both halves of a loopback link must be this crate's own binary: an ambient
        // `FLEET_DAEMON` from another checkout would mix builds and fail the link tests.
        let fleetd = env!("CARGO_BIN_EXE_fleetd").to_owned();
        let run = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "exec \"$@\"".to_owned(),
            "--".to_owned(),
        ];
        let fleet_home = Some(home.display().to_string());
        let config = HostConfigEntry::Command {
            run: run.clone(),
            fleetd: fleetd.clone(),
            fleet_home: fleet_home.clone(),
            display: Some("loopback".to_owned()),
        };
        Self {
            daemon: DaemonProcess::start(home),
            machine: CommandMachine::new(
                HostId::try_from("loopback").expect("loopback host id"),
                run,
                fleetd,
                fleet_home,
                Some("loopback".to_owned()),
            ),
            config,
        }
    }
}

pub fn assert_remote_contract(remote: &RemoteDaemon) {
    let _ = (&remote.daemon, &remote.machine, &remote.config);
}
