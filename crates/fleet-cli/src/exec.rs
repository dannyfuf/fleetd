//! Byte-exact command passthrough with a best-effort cooperative watch copy.

use crate::args::{ExecArgs, WatchChildArgs};
use fleet_client::Client;
use fleet_core::{
    ids::{SessionId, TerminalId},
    watches::{WatchId, WatchStream},
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use std::{
    ffi::OsString,
    io::{Read, Write},
    os::unix::{
        net::UnixStream,
        process::{CommandExt, ExitStatusExt},
    },
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};

const BATCH: Duration = Duration::from_millis(50);
const WATCH_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(args: ExecArgs) -> i32 {
    let terminal = match watch_terminal(args.watch, |key| std::env::var_os(key)) {
        Ok(terminal) => terminal,
        Err(reason) => {
            debug(reason);
            return passthrough(&args.command, None);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            debug(error);
            return passthrough(&args.command, None);
        }
    };
    match runtime.block_on(watched(&args, terminal)) {
        Ok(code) => code,
        Err(error) => {
            debug(error);
            passthrough(&args.command, None)
        }
    }
}

fn watch_terminal(
    watch: bool,
    mut env: impl FnMut(&str) -> Option<OsString>,
) -> Result<TerminalId, &'static str> {
    if !watch {
        return Err("--watch not requested");
    }
    let session = env("FLEET_SESSION").ok_or("FLEET_SESSION missing")?;
    session
        .to_str()
        .and_then(|id| id.parse::<SessionId>().ok())
        .ok_or("FLEET_SESSION invalid")?;
    let terminal = env("FLEET_TERMINAL_ID")
        .as_deref()
        .and_then(|id| id.to_str())
        .and_then(|id| id.parse::<TerminalId>().ok())
        .ok_or("FLEET_TERMINAL_ID missing or not numeric")?;
    if env("FLEET_WATCH").is_some() {
        return Err("FLEET_WATCH already set (nested watch)");
    }
    Ok(terminal)
}

fn debug(error: impl std::fmt::Display) {
    if std::env::var_os("FLEET_DEBUG").is_some() {
        let reason = error.to_string().replace(['\r', '\n'], " ");
        eprintln!("fleet exec: watch unavailable: {reason}");
    }
}
fn passthrough(argv: &[OsString], watch: Option<String>) -> i32 {
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(watch) = watch {
        command.env("FLEET_WATCH", watch);
    }
    let error = command.exec();
    eprintln!("fleet exec: {}: {error}", argv[0].to_string_lossy());
    if error.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

pub(crate) fn child(args: WatchChildArgs) -> i32 {
    // EOF/timeout releases the target transparently if the wrapper died during startup.
    let watch = (|| -> std::io::Result<String> {
        let mut socket = UnixStream::connect(&args.socket)?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut id = String::new();
        socket.read_to_string(&mut id)?;
        Ok(id)
    })()
    .ok()
    .filter(|id| id.parse::<WatchId>().is_ok());
    passthrough(&args.command, watch)
}

async fn watched(args: &ExecArgs, terminal: TerminalId) -> anyhow::Result<i32> {
    let home = crate::commands::fleet_home()?;
    let client = tokio::time::timeout(WATCH_TIMEOUT, Client::connect(home)).await??;
    // /tmp keeps the Unix socket below macOS's small sockaddr_un path limit.
    let gate_dir = tempfile::Builder::new()
        .prefix("fleet-watch-")
        .tempdir_in("/tmp")?;
    let gate_path = gate_dir.path().join("gate");
    let gate = tokio::net::UnixListener::bind(&gate_path)?;
    let executable = std::env::current_exe()?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let mut child = Command::new(executable)
        .arg("watch-child")
        .arg(&gate_path)
        .arg("--")
        .args(&args.command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // No fallible early return after spawn: the user's command must never run twice.
    let pid = Some(child.id());
    let forward = tokio::spawn(async move {
        loop {
            let sig = tokio::select! {
                _ = interrupt.recv() => Signal::SIGINT,
                _ = terminate.recv() => Signal::SIGTERM,
                _ = hangup.recv() => Signal::SIGHUP,
            };
            if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
                let _ = kill(Pid::from_raw(pid), sig);
            }
        }
    });
    let label = args.label.clone().unwrap_or_else(|| {
        Path::new(&args.command[0])
            .file_name()
            .unwrap_or(&args.command[0])
            .to_string_lossy()
            .into_owned()
    });
    let registration = tokio::time::timeout(
        WATCH_TIMEOUT,
        client.start_watch(
            terminal,
            label,
            args.command
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            std::env::current_dir().ok(),
            pid,
        ),
    )
    .await;
    let watch = match registration {
        Ok(Ok(id)) => Some(id),
        Ok(Err(error)) => {
            debug(error);
            None
        }
        Err(error) => {
            debug(error);
            None
        }
    };
    if let Ok(Ok((mut socket, _))) = tokio::time::timeout(WATCH_TIMEOUT, gate.accept()).await {
        if let Some(id) = watch {
            let _ = socket.write_all(id.to_string().as_bytes()).await;
        }
        let _ = socket.shutdown().await;
    }
    drop(gate);
    let (tx, rx) = mpsc::channel(128);
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            tee(stdout, WatchStream::Stdout, tx)
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            tee(stderr, WatchStream::Stderr, tx)
        }));
    }
    drop(tx);
    let reporter = tokio::spawn(report(client.clone(), watch, rx));
    let status = tokio::task::spawn_blocking(move || child.wait()).await;
    forward.abort(); // Do not forward to a reaped (and possibly reused) PID while draining.
    for reader in readers {
        let _ = tokio::task::spawn_blocking(move || reader.join()).await;
    }
    let _ = reporter.await;
    match status {
        Ok(Ok(status)) => {
            if let Some(id) = watch {
                let _ = tokio::time::timeout(
                    WATCH_TIMEOUT,
                    client.finish_watch(id, status.code(), status.signal()),
                )
                .await;
            }
            Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
        }
        other => {
            debug(format!("child wait failed: {other:?}"));
            Ok(1)
        }
    }
}

fn tee(mut pipe: impl Read, stream: WatchStream, tx: mpsc::Sender<(WatchStream, Vec<u8>)>) {
    let mut buffer = [0_u8; 8192];
    let mut copying = true;
    loop {
        let n = match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let bytes = &buffer[..n];
        let forwarded = match stream {
            WatchStream::Stdout => {
                let mut out = std::io::stdout().lock();
                out.write_all(bytes).and_then(|()| out.flush())
            }
            WatchStream::Stderr => {
                let mut out = std::io::stderr().lock();
                out.write_all(bytes).and_then(|()| out.flush())
            }
        };
        if forwarded.is_err() {
            // Closing our read end propagates a broken consumer pipe to the child.
            break;
        }
        // Flush original bytes before enqueueing. Bounded backpressure preserves burst output;
        // the reporter closes this queue on RPC timeout so daemon failure cannot strand readers.
        if copying && tx.blocking_send((stream, bytes.to_vec())).is_err() {
            copying = false;
        }
    }
}

async fn report(
    client: Client,
    watch: Option<WatchId>,
    mut rx: mpsc::Receiver<(WatchStream, Vec<u8>)>,
) -> bool {
    let Some(watch) = watch else {
        return false;
    };
    let mut interval = tokio::time::interval(BATCH);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut chunks: Vec<(WatchStream, Vec<u8>)> = Vec::new();
    let mut bytes = 0;
    loop {
        let done = tokio::select! {
            item = rx.recv(), if bytes < 256 * 1024 => {
                if let Some((stream, data)) = item {
                    bytes += data.len();
                    if let Some((_, text)) = chunks.iter_mut().find(|(previous, _)| *previous == stream) { text.extend(data); }
                    else { chunks.push((stream, data)); }
                    continue;
                }
                true
            }
            _ = interval.tick() => false,
        };
        for (stream, data) in chunks.drain(..) {
            if !matches!(
                tokio::time::timeout(
                    WATCH_TIMEOUT,
                    client.append_watch_output(
                        watch,
                        stream,
                        String::from_utf8_lossy(&data).into_owned()
                    )
                )
                .await,
                Ok(Ok(()))
            ) {
                return false;
            }
        }
        bytes = 0;
        if done {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eligibility(watch: bool, env: &[(&str, &str)]) -> Result<TerminalId, &'static str> {
        watch_terminal(watch, |key| {
            env.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn watch_eligibility_uses_terminal_id_and_preserves_human_name() {
        assert_eq!(
            eligibility(
                true,
                &[
                    ("FLEET_SESSION", "repo/feature"),
                    ("FLEET_TERMINAL", "editor"),
                    ("FLEET_TERMINAL_ID", "42"),
                ]
            ),
            Ok(TerminalId(42))
        );
        for id in [None, Some("editor"), Some(""), Some("18446744073709551616")] {
            let mut env = vec![("FLEET_SESSION", "repo/feature"), ("FLEET_TERMINAL", "42")];
            if let Some(id) = id {
                env.push(("FLEET_TERMINAL_ID", id));
            }
            assert_eq!(
                eligibility(true, &env),
                Err("FLEET_TERMINAL_ID missing or not numeric")
            );
        }
    }

    #[test]
    fn every_ineligible_watch_has_a_passthrough_reason() {
        assert_eq!(eligibility(false, &[]), Err("--watch not requested"));
        assert_eq!(
            eligibility(true, &[("FLEET_TERMINAL_ID", "42")]),
            Err("FLEET_SESSION missing")
        );
        assert_eq!(
            eligibility(true, &[("FLEET_SESSION", ""), ("FLEET_TERMINAL_ID", "42")]),
            Err("FLEET_SESSION invalid")
        );
        assert_eq!(
            eligibility(
                true,
                &[
                    ("FLEET_SESSION", "repo/feature"),
                    ("FLEET_TERMINAL_ID", "42"),
                    ("FLEET_WATCH", ""),
                ]
            ),
            Err("FLEET_WATCH already set (nested watch)")
        );
    }
}
