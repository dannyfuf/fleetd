//! Standard-input/output bridge to a daemon socket.

use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use fleet_core::paths::{FleetHome, resolve_home};
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

async fn connect_or_start(home: &Path) -> anyhow::Result<UnixStream> {
    let layout = FleetHome::new(home);
    let socket_path = layout.socket_path();
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
    let (mut socket_reader, mut socket_writer) = socket.into_split();
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    tokio::select! {
        result = tokio::io::copy(&mut input, &mut socket_writer) => result.map(|_| ()),
        result = tokio::io::copy(&mut socket_reader, &mut output) => result.map(|_| ()),
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
