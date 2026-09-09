//! Standard-input/output bridge to a daemon socket.

use std::{
    io,
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

/// Resolves the selected Fleet home, including a shell-quoted leading `~`.
pub fn resolve_home(home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    resolve_home_with(home, std::env::var_os("HOME").map(PathBuf::from))
}

fn resolve_home_with(
    home: Option<PathBuf>,
    environment_home: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    match home {
        Some(path) => match path.strip_prefix("~") {
            Ok(suffix) => environment_home
                .map(|home| home.join(suffix))
                .ok_or_else(|| {
                    anyhow::anyhow!("HOME is not set; cannot expand {}", path.display())
                }),
            Err(_) => Ok(path),
        },
        None => environment_home
            .map(|home| home.join(".fleet"))
            .ok_or_else(|| anyhow::anyhow!("HOME is not set; pass --home or FLEET_HOME")),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_default_and_shell_quoted_tilde_homes() {
        let environment_home = PathBuf::from("/home/df");

        assert_eq!(
            resolve_home_with(None, Some(environment_home.clone())).unwrap(),
            environment_home.join(".fleet")
        );
        assert_eq!(
            resolve_home_with(Some(PathBuf::from("~/.fleet")), Some(environment_home)).unwrap(),
            PathBuf::from("/home/df/.fleet")
        );
    }

    #[test]
    fn preserves_explicit_paths_and_requires_home_only_for_tilde() {
        assert_eq!(
            resolve_home_with(Some(PathBuf::from("/srv/fleet")), None).unwrap(),
            PathBuf::from("/srv/fleet")
        );
        assert!(resolve_home_with(Some(PathBuf::from("~/.fleet")), None).is_err());
    }
}
