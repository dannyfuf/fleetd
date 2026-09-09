//! Managed `opencode serve` process lifecycle.

use std::{
    ffi::OsString,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use fleet_core::agents::{AgentEvent, SessionState};
use fleet_core::ids::HostId;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

use super::super::{ProviderError, ProviderResult, ProviderSink, exit_code};

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const TERM_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(super) struct ManagedServer {
    pid: u32,
    expected_exit: Arc<AtomicBool>,
    /// The last stderr line the child printed, which is where a bind failure says so.
    last_error: Arc<std::sync::Mutex<Option<String>>>,
}

/// A local port held open until the server that will bind it is about to be launched.
///
/// Dropping the listener before the child is spawned leaves the port unbound for as long as the
/// launch takes — a version check, a process spawn — which is long enough for a second thread
/// to reserve the same number and for one of the two servers to fail to bind.
#[derive(Debug)]
pub(super) struct PortReservation {
    listener: std::net::TcpListener,
    port: u16,
}

impl PortReservation {
    /// The reserved port.
    pub(super) fn port(&self) -> u16 {
        self.port
    }

    /// Releases the port for the child that is about to bind it.
    fn release(self) {
        drop(self.listener);
    }
}

impl ManagedServer {
    pub(super) async fn spawn(
        command_line: &str,
        host: Option<&HostId>,
        worktree: &Path,
        port: PortReservation,
        events: ProviderSink,
    ) -> ProviderResult<Self> {
        let (program, base_args) = command_parts(command_line)?;
        check_version(&program, &base_args, host).await?;

        let port = {
            let number = port.port();
            // Released here and nowhere earlier: the version check above can take seconds.
            port.release();
            number
        };
        let mut command = Command::new(&program);
        // Each daemon owns and consumes its OpenCode HTTP/SSE server on its own machine.
        // Loopback binding keeps the port off both the LAN and the tailnet.
        command
            .args(&base_args)
            .args([
                "serve",
                "--hostname",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .current_dir(worktree)
            .process_group(0)
            .kill_on_drop(false)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| binary_io_error(&program, host, "launch `opencode serve`", error))?;
        let pid = child.id().ok_or_else(|| ProviderError::Unavailable {
            reason: "launched OpenCode server has no process id".to_owned(),
        })?;
        let last_error = Arc::new(std::sync::Mutex::new(None));
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(trace_lines(stdout, "stdout", pid, None));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(trace_lines(
                stderr,
                "stderr",
                pid,
                Some(Arc::clone(&last_error)),
            ));
        }

        let expected_exit = Arc::new(AtomicBool::new(false));
        let monitor_expected = Arc::clone(&expected_exit);
        tokio::spawn(async move {
            let status = child.wait().await;
            let expected = monitor_expected.load(Ordering::Acquire);
            let code = status.as_ref().ok().and_then(exit_code);
            let _ignored = events.send(AgentEvent::SessionExited { code, expected }.into());
            match status {
                Ok(status) if expected => {
                    tracing::debug!(pid, %status, "OpenCode server exited");
                }
                Ok(status) => {
                    let _ignored = events.send(
                        AgentEvent::RuntimeError {
                            fatal: true,
                            message: format!("OpenCode server exited unexpectedly: {status}"),
                        }
                        .into(),
                    );
                }
                Err(error) => {
                    let _ignored = events.send(
                        AgentEvent::RuntimeError {
                            fatal: true,
                            message: format!("failed to reap OpenCode server: {error}"),
                        }
                        .into(),
                    );
                }
            }
        });

        Ok(Self {
            pid,
            expected_exit,
            last_error,
        })
    }

    /// The last thing the child said on stderr, which names a bind failure the readiness
    /// timeout would otherwise report as a generic timeout.
    pub(super) fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) async fn stop(&self, events: &ProviderSink) -> ProviderResult<()> {
        self.expected_exit.store(true, Ordering::Release);
        signal_group(self.pid, libc::SIGTERM)?;
        let deadline = tokio::time::Instant::now() + TERM_GRACE;
        while process_group_is_alive(self.pid) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if process_group_is_alive(self.pid) {
            signal_group(self.pid, libc::SIGKILL)?;
        }
        let _ignored = events.send(AgentEvent::SessionStateChanged(SessionState::Stopped).into());
        Ok(())
    }
}

impl Drop for ManagedServer {
    fn drop(&mut self) {
        if !self.expected_exit.swap(true, Ordering::AcqRel) {
            let _ignored = signal_group(self.pid, libc::SIGTERM);
        }
    }
}

pub(super) fn free_port() -> ProviderResult<PortReservation> {
    let listener =
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|error| {
            ProviderError::Unavailable {
                reason: format!("reserve local port for OpenCode: {error}"),
            }
        })?;
    let port = listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| ProviderError::Unavailable {
            reason: format!("inspect reserved OpenCode port: {error}"),
        })?;
    Ok(PortReservation { listener, port })
}

/// Splits the configured `agentCommands.opencode` line into a program and its leading
/// arguments, so `opencode --print-logs` keeps working and `serve …` is appended after it.
fn command_parts(command_line: &str) -> ProviderResult<(OsString, Vec<OsString>)> {
    let parts = shell_words::split(command_line).map_err(|error| ProviderError::Unavailable {
        reason: format!("invalid configured OpenCode command: {error}"),
    })?;
    let mut parts = parts.into_iter();
    let program = parts.next().ok_or_else(|| ProviderError::Unavailable {
        reason: "configured OpenCode command is empty".to_owned(),
    })?;
    Ok((OsString::from(program), parts.map(OsString::from).collect()))
}

async fn check_version(
    program: &OsString,
    base_args: &[OsString],
    host: Option<&HostId>,
) -> ProviderResult<()> {
    let output = tokio::time::timeout(
        VERSION_TIMEOUT,
        Command::new(program)
            .args(base_args)
            .arg("--version")
            .output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        what: "`opencode --version`".to_owned(),
    })?
    .map_err(|error| binary_io_error(program, host, "run `opencode --version`", error))?;
    if !output.status.success() {
        return Err(ProviderError::Unavailable {
            reason: format!(
                "`opencode --version` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let token = version
        .split_whitespace()
        .find(|token| {
            token
                .trim_start_matches('v')
                .starts_with(|ch: char| ch.is_ascii_digit())
        })
        .ok_or_else(|| ProviderError::Unavailable {
            reason: format!("cannot parse OpenCode version `{}`", version.trim()),
        })?;
    let major = token
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|major| major.parse::<u64>().ok())
        .ok_or_else(|| ProviderError::Unavailable {
            reason: format!("cannot parse OpenCode version `{}`", version.trim()),
        })?;
    if major < 1 {
        return Err(ProviderError::Unavailable {
            reason: format!("OpenCode {token} is unsupported; version 1 or newer is required"),
        });
    }
    Ok(())
}

fn machine_label(host: Option<&HostId>) -> String {
    host.map_or_else(|| "this machine".to_owned(), |host| format!("host {host}"))
}

fn binary_io_error(
    program: &OsString,
    host: Option<&HostId>,
    operation: &str,
    error: std::io::Error,
) -> ProviderError {
    ProviderError::Unavailable {
        reason: if error.kind() == std::io::ErrorKind::NotFound {
            format!(
                "OpenCode binary `{}` was not found on {}",
                program.to_string_lossy(),
                machine_label(host)
            )
        } else {
            format!("{operation} on {}: {error}", machine_label(host))
        },
    }
}

fn signal_group(pid: u32, signal: i32) -> ProviderResult<()> {
    let pid = i32::try_from(pid).map_err(|_| ProviderError::Protocol {
        message: format!("invalid OpenCode process id {pid}"),
    })?;
    // SAFETY: Fleet placed this exact child in a process group whose id equals its pid.
    let result = unsafe { libc::kill(-pid, signal) };
    let error = std::io::Error::last_os_error();
    if result == 0 || error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(ProviderError::Protocol {
            message: format!("signal OpenCode process group {pid}: {error}"),
        })
    }
}

fn process_group_is_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks whether the process group Fleet created still exists.
    let result = unsafe { libc::kill(-pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

async fn trace_lines<R>(
    reader: R,
    stream: &'static str,
    pid: u32,
    last_error: Option<Arc<std::sync::Mutex<Option<String>>>>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(last_error) = &last_error
                    && !line.trim().is_empty()
                {
                    *last_error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(line.trim().to_owned());
                }
                tracing::debug!(pid, stream, %line, "OpenCode server output");
            }
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(pid, stream, %error, "failed reading OpenCode server output");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing_program() -> OsString {
        OsString::from("/fleet-tests/no-such-opencode-binary")
    }

    #[tokio::test]
    async fn missing_binary_names_remote_host() {
        let host = HostId::try_from("dev-box").unwrap_or_else(|error| panic!("{error}"));
        let error = check_version(&missing_program(), &[], Some(&host))
            .await
            .expect_err("missing binary should fail");

        assert!(error.to_string().contains("not found on host dev-box"));
    }

    #[tokio::test]
    async fn missing_binary_names_this_machine_for_local_provider() {
        let error = check_version(&missing_program(), &[], None)
            .await
            .expect_err("missing binary should fail");

        assert!(error.to_string().contains("not found on this machine"));
    }
}
