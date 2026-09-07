//! Isolated real-daemon fixtures with child-only HOME/PATH and RAII cleanup.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result};
use fleet_client::Client;
use fleet_proto::event::EventKind;

/// How long the harness waits for the daemon to answer its first ping.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// How often it probes while waiting.
const PROBE: Duration = Duration::from_millis(50);

/// A running daemon plus the temporary home it owns.
pub struct Daemon {
    home: tempfile::TempDir,
    child: Child,
}

impl Daemon {
    /// Requires a built fleetd binary, or an explicit FLEET_DAEMON path.
    pub fn start(label: &str) -> Result<Self> {
        let executable = fleetd_path()
            .context("fleetd is missing; run cargo build -p fleet-daemon or set FLEET_DAEMON")?;
        let home = tempfile::Builder::new()
            .prefix(&format!("fleet-app-{label}-"))
            .tempdir()
            .context("create isolated Fleet home")?;
        fs::create_dir_all(home.path().join("logs")).context("create daemon log directory")?;
        let bin = fake_gh(home.path())?;
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(paths).context("build child PATH")?;
        let log = fs::File::create(home.path().join("logs/fleetd.out"))
            .context("create daemon stdout log")?;
        let errors = log.try_clone().context("clone daemon log handle")?;
        let child = Command::new(&executable)
            .arg("--home")
            .arg(home.path())
            .env("PATH", path)
            .env("HOME", home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errors))
            .spawn()
            .with_context(|| format!("start {}", executable.display()))?;
        Ok(Self { home, child })
    }

    /// The temporary `FLEET_HOME`.
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// Connects, negotiates the handshake and subscribes to the families the app subscribes to.
    pub async fn connect(&self) -> Client {
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(client) = Client::connect(self.home.path()).await
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
                fs::read_to_string(self.home.path().join("logs").join("fleetd.out"))
                    .unwrap_or_default()
            );
            tokio::time::sleep(PROBE).await;
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ignored = self.child.kill();
        let _ignored = self.child.wait();
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

/// Writes a `gh` that answers every query with an empty JSON array, and returns its directory.
///
/// The daemon shells out to the real `gh` for discovery and pull requests; a test must never
/// reach GitHub, and must never depend on whether the developer happens to be logged in.
fn fake_gh(home: &Path) -> Result<PathBuf> {
    let bin = home.join("bin");
    fs::create_dir_all(&bin).context("create fake gh directory")?;
    let script = bin.join("gh");
    fs::write(
        &script,
        b"#!/bin/sh\n\
          # Fake gh for fleet-app integration tests: never reaches the network.\n\
          case \"$1\" in\n\
            auth) echo 'github.com: logged in as fleet-test'; exit 0 ;;\n\
            api|pr|search|repo) echo '[]'; exit 0 ;;\n\
            --version) echo 'gh version 0.0.0-fleet-test'; exit 0 ;;\n\
            *) echo '[]'; exit 0 ;;\n\
          esac\n",
    )
    .context("write fake gh script")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .context("make fake gh executable")?;
    }
    Ok(bin)
}
