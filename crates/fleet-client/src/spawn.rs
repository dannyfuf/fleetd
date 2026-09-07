//! Daemon discovery, health checks, and automatic spawning.

use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Read},
    os::fd::AsRawFd,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::Duration,
};

use fleet_core::paths::FleetHome;
use fleet_proto::error::ProtoError;
use thiserror::Error;
use tokio::time::{Instant, timeout};

use crate::{Client, ConnectError};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_GRACE: Duration = Duration::from_secs(2);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    executable: Option<PathBuf>,
    started_at: Option<(u64, u64)>,
}

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
    /// The daemon ignored both its shutdown request and SIGTERM.
    #[error("Fleet daemon process {pid} did not stop within 5 seconds")]
    StopTimeout {
        /// Live process identifier found before restart.
        pid: u32,
    },
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

    if let Some(process) = live_process(home)? {
        return Err(SpawnError::ProcessAlive { pid: process.pid });
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
    // A regular thread keeps reaping ownership after startup, cancellation, and runtime shutdown.
    let (exited_tx, mut exited) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("fleetd-reaper".into())
        .spawn(move || {
            let status = command.spawn().and_then(|mut child| child.wait());
            if let Err(error) = &status {
                tracing::warn!(%error, "Fleet daemon child could not be spawned or reaped");
            }
            let _ = exited_tx.send(status);
        })?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match exited.try_recv() {
            Ok(status) => return Err(SpawnError::Exited { status: status? }),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                return Err(io::Error::other("Fleet daemon reaper stopped unexpectedly").into());
            }
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

/// Gracefully stops the current daemon, falls back to SIGTERM, then starts the resolved binary.
pub async fn restart_daemon(
    home: impl AsRef<Path>,
    fleetd_path: Option<PathBuf>,
) -> Result<Client, SpawnError> {
    let home = home.as_ref();
    let daemon_path = match fleetd_path {
        Some(path) => path,
        None => resolve_daemon_path()?,
    };
    let process = live_process(home)?;
    let graceful = match Client::connect(home).await {
        Ok(client) => timeout(STOP_GRACE, client.daemon_shutdown(false))
            .await
            .is_ok_and(|result| result.is_ok()),
        Err(_) => false,
    };
    if graceful && wait_until_stopped(home, process.as_ref(), STOP_GRACE).await {
        return ensure_daemon(home, Some(daemon_path)).await;
    }

    if let Some(recorded) = process.as_ref()
        && let Some(current) = live_process(home)?
        && verified_signal_target(recorded, &current)
    {
        if let Some(actual) = current.executable.as_deref()
            && !executable_matches(actual, &daemon_path)
        {
            tracing::warn!(
                running = %actual.display(),
                replacement = %daemon_path.display(),
                "restarting Fleet daemon from a different executable"
            );
        }
        terminate(recorded.pid)?;
    }
    if !wait_until_stopped(home, process.as_ref(), STOP_TIMEOUT).await {
        return Err(SpawnError::StopTimeout {
            pid: process.as_ref().map_or(0, |process| process.pid),
        });
    }
    ensure_daemon(home, Some(daemon_path)).await
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

async fn wait_until_stopped(
    home: &Path,
    process: Option<&ProcessIdentity>,
    timeout_after: Duration,
) -> bool {
    let deadline = Instant::now() + timeout_after;
    loop {
        let process_stopped = process.is_none_or(|expected| !same_process_is_alive(expected));
        if process_stopped {
            let socket = FleetHome::new(home).socket_path();
            if !socket.exists() || remove_confirmed_stale_socket(&socket) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

fn live_process(home: &Path) -> Result<Option<ProcessIdentity>, io::Error> {
    let pid_path = FleetHome::new(home).pid_path();
    let mut pid_file = match OpenOptions::new().read(true).write(true).open(&pid_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // The daemon holds this lock for its whole lifetime. An unlocked file is stale, so clear its
    // contents while holding the lock rather than trusting a PID that may have been reused.
    // SAFETY: flock operates on this function's valid open descriptor.
    if unsafe { libc::flock(pid_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        pid_file.set_len(0)?;
        return Ok(None);
    }
    let lock_error = io::Error::last_os_error();
    if !lock_error
        .raw_os_error()
        .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN)
    {
        return Err(lock_error);
    }
    let mut contents = String::new();
    pid_file.read_to_string(&mut contents)?;
    let Ok(pid) = contents.trim().parse::<u32>() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "locked Fleet daemon PID file did not contain a PID",
        ));
    };
    if pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "locked Fleet daemon PID file contained PID zero",
        ));
    }
    if process_is_alive(pid) {
        Ok(Some(process_identity(pid)))
    } else {
        Ok(None)
    }
}

fn verified_signal_target(recorded: &ProcessIdentity, current: &ProcessIdentity) -> bool {
    recorded.pid == current.pid
        && recorded.started_at.is_some()
        && recorded.started_at == current.started_at
}

fn executable_matches(actual: &Path, expected: &Path) -> bool {
    if expected.components().count() == 1 {
        return actual.file_name() == expected.file_name();
    }
    let actual = fs::canonicalize(actual).unwrap_or_else(|_| actual.to_path_buf());
    let expected = fs::canonicalize(expected).unwrap_or_else(|_| expected.to_path_buf());
    actual == expected
}

fn same_process_is_alive(expected: &ProcessIdentity) -> bool {
    if !process_is_alive(expected.pid) {
        return false;
    }
    let current = process_identity(expected.pid);
    match (expected.started_at, current.started_at) {
        (Some(expected), Some(current)) => expected == current,
        _ => true,
    }
}

fn remove_confirmed_stale_socket(socket: &Path) -> bool {
    let Ok(before) = fs::symlink_metadata(socket) else {
        return !socket.exists();
    };
    if !before.file_type().is_socket() {
        return false;
    }
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(_) => false,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            let Ok(after) = fs::symlink_metadata(socket) else {
                return true;
            };
            if before.dev() != after.dev() || before.ino() != after.ino() {
                return false;
            }
            fs::remove_file(socket).is_ok() || !socket.exists()
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "macos")]
fn process_identity(pid: u32) -> ProcessIdentity {
    use std::{ffi::OsString, mem::MaybeUninit, os::unix::ffi::OsStringExt};

    let Ok(raw_pid) = libc::pid_t::try_from(pid) else {
        return ProcessIdentity {
            pid,
            executable: None,
            started_at: None,
        };
    };
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let info_size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).unwrap_or(i32::MAX);
    // SAFETY: proc_pidinfo receives a correctly sized writable proc_bsdinfo buffer.
    let read = unsafe {
        libc::proc_pidinfo(
            raw_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            info_size,
        )
    };
    let started_at = (read == info_size).then(|| {
        // SAFETY: a full proc_bsdinfo was initialized when proc_pidinfo returned its size.
        let info = unsafe { info.assume_init() };
        (info.pbi_start_tvsec, info.pbi_start_tvusec)
    });
    let mut path = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: proc_pidpath receives the buffer's valid pointer and exact capacity.
    let path_len = unsafe {
        libc::proc_pidpath(
            raw_pid,
            path.as_mut_ptr().cast(),
            libc::PROC_PIDPATHINFO_MAXSIZE as u32,
        )
    };
    let executable = usize::try_from(path_len)
        .ok()
        .filter(|length| *length > 0 && *length <= path.len())
        .map(|length| PathBuf::from(OsString::from_vec(path[..length].to_vec())));
    ProcessIdentity {
        pid,
        executable,
        started_at,
    }
}

#[cfg(target_os = "linux")]
fn process_identity(pid: u32) -> ProcessIdentity {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok();
    let started_at = stat
        .as_deref()
        .and_then(|stat| stat.rsplit_once(") "))
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse::<u64>().ok())
        .map(|ticks| (ticks, 0));
    ProcessIdentity {
        pid,
        executable: fs::read_link(format!("/proc/{pid}/exe")).ok(),
        started_at,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        executable: None,
        started_at: None,
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
fn terminate(pid: u32) -> Result<(), io::Error> {
    let pid = libc::pid_t::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "daemon PID is out of range"))?;
    // SAFETY: the PID was read and revalidated from this Fleet home's own pid file immediately
    // before the signal. SIGTERM is the daemon's normal graceful-shutdown signal.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[tokio::test]
    async fn daemon_reaper_survives_startup_cancellation() {
        let home = TempDir::new().unwrap();
        let executable = home.path().join("fake-fleetd");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$2/started\"\nsleep 0.3\nexit 7\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let daemon_home = home.path().to_path_buf();
        let startup =
            tokio::spawn(async move { ensure_daemon(daemon_home, Some(executable)).await });
        let pid = timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(pid) = fs::read_to_string(home.path().join("started"))
                    && let Ok(pid) = pid.parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        startup.abort();
        assert!(startup.await.unwrap_err().is_cancelled());
        timeout(Duration::from_secs(2), async {
            while process_is_alive(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled startup left an unreaped daemon child");
    }

    #[tokio::test]
    async fn daemon_startup_reports_the_reaped_exit_status() {
        let home = TempDir::new().unwrap();
        let error = ensure_daemon(home.path(), Some(PathBuf::from("/usr/bin/false")))
            .await
            .unwrap_err();
        let SpawnError::Exited { status } = error else {
            panic!("expected child exit status, got {error}");
        };
        assert_eq!(status.code(), Some(1));
    }

    #[test]
    fn never_signals_reused_pid() {
        let recorded = ProcessIdentity {
            pid: 42,
            executable: Some(PathBuf::from("/opt/fleetd")),
            started_at: Some((100, 5)),
        };
        let reused = ProcessIdentity {
            pid: 42,
            executable: Some(PathBuf::from("/opt/fleetd")),
            started_at: Some((101, 9)),
        };
        assert!(!verified_signal_target(&recorded, &reused));
    }

    #[test]
    fn signals_same_process_from_different_build_path() {
        let recorded = ProcessIdentity {
            pid: 42,
            executable: Some(PathBuf::from("/worktree-a/target/debug/fleetd")),
            started_at: Some((100, 5)),
        };
        let current = ProcessIdentity {
            pid: 42,
            executable: Some(PathBuf::from("/worktree-a/target/debug/fleetd")),
            started_at: Some((100, 5)),
        };
        assert!(verified_signal_target(&recorded, &current));
        assert!(!executable_matches(
            current.executable.as_deref().unwrap(),
            Path::new("/worktree-b/target/release/fleetd")
        ));
    }

    #[tokio::test]
    async fn dead_daemon_stale_socket_recovers() {
        let home = TempDir::new().unwrap();
        let socket = FleetHome::new(home.path()).socket_path();
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        drop(listener);
        assert!(wait_until_stopped(home.path(), None, Duration::ZERO).await);
        assert!(!socket.exists());
    }
}
