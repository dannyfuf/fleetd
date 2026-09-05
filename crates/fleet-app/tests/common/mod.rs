//! A real `fleetd`, in a temporary `FLEET_HOME`, with a fake `gh` on `PATH`.
//!
//! Every integration test in this crate talks to the daemon the app actually ships with —
//! no fakes, no in-process server — because the questions the Jobs panel and the §3.12 daemon
//! surfaces ask ("did the snapshot arrive?", "did a job event reach me?", "what does a failed
//! request look like?") are only meaningful across the socket.
//!
//! The harness never touches the user's `~/.fleet` or `~/.swarm`: the home is a fresh directory
//! under the system temp dir, `PATH` is rewritten for the child only, and the daemon is stopped
//! and reaped on drop.

#![allow(dead_code)]

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use fleet_client::Client;
use fleet_proto::{event::EventKind, paths::socket_path};

/// How long the harness waits for the daemon to answer its first ping.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// How often it probes while waiting.
const PROBE: Duration = Duration::from_millis(50);

static HOME_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// A running daemon plus the temporary home it owns.
pub struct Daemon {
    home: PathBuf,
    child: Child,
}

impl Daemon {
    /// Starts `fleetd --home <temp>` with a fake `gh` first on `PATH`.
    ///
    /// Returns `None` when the `fleetd` binary has not been built next to the test binary; the
    /// caller skips rather than failing, so `cargo test -p fleet-app` works before a full
    /// workspace build.
    pub fn start(label: &str) -> Option<Self> {
        let executable = fleetd_path()?;
        let home = temp_home(label);
        fs::create_dir_all(home.join("logs")).ok()?;
        let bin = fake_gh(&home)?;

        let path = match std::env::var_os("PATH") {
            Some(existing) => {
                let mut value = std::ffi::OsString::from(bin.as_os_str());
                value.push(":");
                value.push(existing);
                value
            }
            None => std::ffi::OsString::from(bin.as_os_str()),
        };

        let log = fs::File::create(home.join("logs").join("fleetd.out")).ok()?;
        let errors = log.try_clone().ok()?;
        let child = Command::new(executable)
            .arg("--home")
            .arg(&home)
            .env("PATH", path)
            // The daemon must never read the developer's real swarm state during a test.
            .env("HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errors))
            .spawn()
            .ok()?;

        Some(Self { home, child })
    }

    /// The temporary `FLEET_HOME`.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Connects, negotiates the handshake and subscribes to the families the app subscribes to.
    pub async fn connect(&self) -> Client {
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(client) = Client::connect(&self.home).await
                && client.daemon_ping().await.is_ok()
            {
                client
                    .subscribe(vec![
                        EventKind::SnapshotChanged,
                        EventKind::JobUpdated,
                        EventKind::Toast,
                        EventKind::DaemonShuttingDown,
                    ])
                    .await
                    .unwrap_or_else(|error| panic!("subscribe failed: {error}"));
                return client;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fleetd did not become ready within {READY_TIMEOUT:?}; log: {}",
                fs::read_to_string(self.home.join("logs").join("fleetd.out")).unwrap_or_default()
            );
            tokio::time::sleep(PROBE).await;
        }
    }

    /// The socket the app would connect to, for the §3.12 wording.
    pub fn socket(&self) -> PathBuf {
        socket_path(&self.home)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ignored = self.child.kill();
        let _ignored = self.child.wait();
        let _ignored = fs::remove_dir_all(&self.home);
    }
}

/// Where `fleetd` is: `FLEET_DAEMON`, then next to the test binary, then one directory up
/// (`target/debug/deps/<test>` → `target/debug/fleetd`).
pub fn fleetd_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FLEET_DAEMON") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let current = std::env::current_exe().ok()?;
    let mut directory = current.parent()?;
    for _ in 0..3 {
        let candidate = directory.join("fleetd");
        if candidate.is_file() {
            return Some(candidate);
        }
        directory = directory.parent()?;
    }
    None
}

/// A fresh temporary `FLEET_HOME`, unique per process and per call.
fn temp_home(label: &str) -> PathBuf {
    let sequence = HOME_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let home = std::env::temp_dir().join(format!(
        "fleet-app-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ignored = fs::remove_dir_all(&home);
    home
}

/// Writes a `gh` that answers every query with an empty JSON array, and returns its directory.
///
/// The daemon shells out to the real `gh` for discovery and pull requests; a test must never
/// reach GitHub, and must never depend on whether the developer happens to be logged in.
fn fake_gh(home: &Path) -> Option<PathBuf> {
    let bin = home.join("bin");
    fs::create_dir_all(&bin).ok()?;
    let script = bin.join("gh");
    let mut file = fs::File::create(&script).ok()?;
    file.write_all(
        b"#!/bin/sh\n\
          # Fake gh for fleet-app integration tests: never reaches the network.\n\
          case \"$1\" in\n\
            auth) echo 'github.com: logged in as fleet-test'; exit 0 ;;\n\
            api|pr|search|repo) echo '[]'; exit 0 ;;\n\
            --version) echo 'gh version 0.0.0-fleet-test'; exit 0 ;;\n\
            *) echo '[]'; exit 0 ;;\n\
          esac\n",
    )
    .ok()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some(bin)
}
