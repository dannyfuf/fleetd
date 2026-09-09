//! Standard-input/output bridge to a daemon socket.

use std::{
    fs::File,
    io,
    os::fd::FromRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use fleet_core::paths::FleetHome;
use tokio::{net::UnixStream, time::Instant};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_INTERVAL: Duration = Duration::from_millis(50);

/// Ensures the selected daemon is running, then relays raw bytes between it and stdio.
pub async fn run_connect(home: Option<PathBuf>) -> anyhow::Result<()> {
    let home = resolve_home(home)?;
    let socket = connect_or_start(&home).await?;
    relay(socket).await?;
    Ok(())
}

fn resolve_home(home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    home.or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".fleet"))
    })
    .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass --home or FLEET_HOME"))
}

async fn connect_or_start(home: &Path) -> anyhow::Result<UnixStream> {
    let socket_path = FleetHome::new(home).socket_path();
    if let Ok(socket) = UnixStream::connect(&socket_path).await {
        return Ok(socket);
    }

    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--home")
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detached(&mut command);
    let mut child = command.spawn()?;
    let deadline = Instant::now() + START_TIMEOUT;
    let mut exit = None;
    loop {
        match UnixStream::connect(&socket_path).await {
            Ok(socket) => return Ok(socket),
            Err(error) if Instant::now() >= deadline => {
                let detail =
                    exit.map_or_else(String::new, |status| format!("; fleetd exited {status}"));
                return Err(anyhow::anyhow!(
                    "could not connect to Fleet daemon at {}: {error}{detail}",
                    socket_path.display()
                ));
            }
            Err(_) => {}
        }
        if exit.is_none() {
            exit = child.try_wait()?;
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

async fn relay(socket: UnixStream) -> io::Result<()> {
    let socket = socket.into_std()?;
    socket.set_nonblocking(false)?;
    let socket_reader = socket.try_clone()?;
    let input = duplicate_stdio(libc::STDIN_FILENO)?;
    let output = duplicate_stdio(libc::STDOUT_FILENO)?;
    tokio::task::spawn_blocking(move || {
        let (finished, completion) = std::sync::mpsc::channel();
        let to_daemon = finished.clone();
        let _to_daemon = std::thread::spawn(move || {
            let result = io::copy(&mut &input, &mut &socket);
            let _ = to_daemon.send(result);
        });
        let _from_daemon = std::thread::spawn(move || {
            let result = io::copy(&mut &socket_reader, &mut &output);
            let _ = finished.send(result);
        });
        completion
            .recv()
            .map_err(|_| io::Error::other("stdio relay workers stopped"))??;
        Ok(())
    })
    .await
    .map_err(|error| io::Error::other(format!("stdio relay task failed: {error}")))?
}

fn duplicate_stdio(fd: libc::c_int) -> io::Result<File> {
    // SAFETY: `dup` returns a new owned descriptor for the valid process stdio descriptor.
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor was just created by `dup` and ownership is transferred to `File`.
    let file = unsafe { File::from_raw_fd(duplicate) };
    Ok(file)
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
