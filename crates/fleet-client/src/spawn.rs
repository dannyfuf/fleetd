//! Daemon discovery, health checks, and automatic spawning.

use std::{
    env,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::Duration,
};

use fleet_proto::{error::ProtoError, paths::pid_path};
use thiserror::Error;
use tokio::time::{Instant, timeout};

use crate::{Client, ConnectError};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_INTERVAL: Duration = Duration::from_millis(50);

/// A failure to discover, start, or observe the Fleet daemon.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// A filesystem or process operation failed.
    #[error("could not start Fleet daemon: {0}")]
    Io(#[from] io::Error),
    /// A process recorded in the PID file is alive while its socket is unavailable.
    #[error("Fleet daemon process {pid} is alive but its socket is unavailable")]
    ProcessAlive {
        /// Live process identifier found in `fleetd.pid`.
        pid: u32,
    },
    /// The newly started daemon exited before becoming ready.
    #[error("Fleet daemon exited before becoming ready: {status}")]
    Exited {
        /// Child exit status.
        status: ExitStatus,
    },
    /// The daemon did not answer a ping before the startup deadline.
    #[error("Fleet daemon did not become ready within 10 seconds")]
    Timeout,
    /// The daemon connected but rejected or violated the protocol handshake.
    #[error(transparent)]
    Connect(#[from] ConnectError),
    /// The daemon was reachable but rejected the liveness request.
    #[error("Fleet daemon rejected the liveness request: {0}")]
    Protocol(#[from] ProtoError),
}

/// Ensures a healthy daemon exists, spawning a detached one when the socket and PID are stale.
pub async fn ensure_daemon(
    home: impl AsRef<Path>,
    fleetd_path: Option<PathBuf>,
) -> Result<Client, SpawnError> {
    let home = home.as_ref();
    match Client::connect(home).await {
        Ok(client) => {
            client.daemon_ping().await?;
            return Ok(client);
        }
        Err(ConnectError::Io(_)) => {}
        Err(error) => return Err(error.into()),
    }

    if let Some(pid) = live_pid(home)? {
        return Err(SpawnError::ProcessAlive { pid });
    }

    let logs = home.join("logs");
    fs::create_dir_all(&logs)?;
    let log_path = logs.join("fleetd.out");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = stdout.try_clone()?;
    let executable = match fleetd_path {
        Some(path) => path,
        None => resolve_daemon_path()?,
    };
    let mut command = Command::new(executable);
    command
        .arg("--home")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    configure_detached(&mut command);
    let mut child = command.spawn()?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(SpawnError::Exited { status });
        }
        if let Some(client) = probe_before(home, deadline).await {
            return Ok(client);
        }
        if Instant::now() >= deadline {
            return Err(SpawnError::Timeout);
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

/// Resolves `fleetd` from `FLEET_DAEMON`, the current executable's directory, then `PATH`.
pub fn resolve_daemon_path() -> Result<PathBuf, io::Error> {
    if let Some(path) = env::var_os("FLEET_DAEMON") {
        return Ok(PathBuf::from(path));
    }
    let current = env::current_exe()?;
    if let Some(parent) = current.parent() {
        let sibling = parent.join("fleetd");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from("fleetd"))
}

async fn probe(home: &Path) -> Option<Client> {
    let client = Client::connect(home).await.ok()?;
    client.daemon_ping().await.ok()?;
    Some(client)
}

async fn probe_before(home: &Path, deadline: Instant) -> Option<Client> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return None;
    }
    timeout(remaining, probe(home)).await.ok().flatten()
}

fn live_pid(home: &Path) -> Result<Option<u32>, io::Error> {
    let contents = match fs::read_to_string(pid_path(home)) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Ok(pid) = contents.trim().parse::<u32>() else {
        return Ok(None);
    };
    if pid == 0 {
        return Ok(None);
    }
    if process_is_alive(pid) {
        Ok(Some(pid))
    } else {
        Ok(None)
    }
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: `kill` with signal zero does not deliver a signal and accepts any process ID.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().kind() == io::ErrorKind::PermissionDenied
}

#[cfg(unix)]
fn configure_detached(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: this callback only invokes async-signal-safe `setsid` between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}
