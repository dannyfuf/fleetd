//! Shell command execution abstractions.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use tokio::{
    fs::OpenOptions,
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use crate::{DaemonError, DaemonResult};

/// A fully specified process invocation without shell interpolation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    /// Executable name or absolute path.
    pub program: String,
    /// Ordered process arguments.
    pub args: Vec<String>,
    /// Optional working directory.
    pub cwd: Option<PathBuf>,
    /// Environment additions or replacements.
    pub env: BTreeMap<String, String>,
    /// Optional execution timeout.
    pub timeout: Option<Duration>,
}

impl ShellCommand {
    /// Creates an invocation for `program` with no arguments or overrides.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            timeout: None,
        }
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, argument: impl Into<String>) -> Self {
        self.args.push(argument.into());
        self
    }

    /// Appends ordered arguments.
    #[must_use]
    pub fn args(mut self, arguments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Sets the child working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Adds or replaces one environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Sets the process deadline.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Captured result of a completed process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellResult {
    /// Process exit status code, or `-1` when unavailable.
    pub status: i32,
    /// UTF-8-lossy standard output.
    pub stdout: String,
    /// UTF-8-lossy standard error.
    pub stderr: String,
}

impl ShellResult {
    /// Returns whether the process exited successfully.
    #[must_use]
    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// Converts a non-zero status into a contextual shell error.
    pub fn require_success(self, operation: &str) -> DaemonResult<Self> {
        if self.success() {
            Ok(self)
        } else {
            Err(DaemonError::Shell(format!(
                "{operation} exited {}: {}",
                self.status,
                concise_output(&self.stderr, &self.stdout)
            )))
        }
    }
}

/// Metadata for a successfully launched detached process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetachedProcess {
    /// Operating-system process identifier.
    pub pid: u32,
}

/// Callback receiving complete stdout and stderr lines from a streaming child.
pub type LineCallback = Arc<dyn Fn(String) + Send + Sync>;

/// External process boundary used by all command-line adapters.
#[async_trait]
pub trait Shell: Send + Sync {
    /// Runs a command to completion and captures output.
    async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult>;

    /// Starts a command independently with stdout and stderr appended to `log_path`.
    async fn run_detached(
        &self,
        command: ShellCommand,
        log_path: &Path,
    ) -> DaemonResult<DetachedProcess>;

    /// Runs a child while streaming lines and honoring explicit cancellation.
    async fn run_streaming(
        &self,
        command: ShellCommand,
        cancel: CancellationToken,
        on_line: LineCallback,
    ) -> DaemonResult<ShellResult>;
}

/// Tokio-backed real process runner.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealShell;

#[async_trait]
impl Shell for RealShell {
    async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult> {
        let description = describe(&command);
        let timeout = command.timeout;
        let mut child = build_command(&command);
        child.kill_on_drop(true);
        let output = if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, child.output())
                .await
                .map_err(|_| DaemonError::Timeout(description.clone()))?
        } else {
            child.output().await
        }
        .map_err(|error| DaemonError::Shell(format!("{description}: {error}")))?;
        Ok(ShellResult {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    async fn run_detached(
        &self,
        command: ShellCommand,
        log_path: &Path,
    ) -> DaemonResult<DetachedProcess> {
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| DaemonError::fs(parent, error))?;
        }
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .await
            .map_err(|error| DaemonError::fs(log_path, error))?
            .into_std()
            .await;
        let stderr = stdout
            .try_clone()
            .map_err(|error| DaemonError::fs(log_path, error))?;
        let description = describe(&command);
        let mut process = build_command(&command);
        process
            .process_group(0)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let child = process
            .spawn()
            .map_err(|error| DaemonError::Shell(format!("{description}: {error}")))?;
        let pid = child
            .id()
            .ok_or_else(|| DaemonError::Shell(format!("{description}: child has no pid")))?;
        drop(child);
        Ok(DetachedProcess { pid })
    }

    async fn run_streaming(
        &self,
        command: ShellCommand,
        cancel: CancellationToken,
        on_line: LineCallback,
    ) -> DaemonResult<ShellResult> {
        let description = describe(&command);
        let timeout = command.timeout;
        let mut process = build_command(&command);
        process
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = process
            .spawn()
            .map_err(|error| DaemonError::Shell(format!("{description}: {error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DaemonError::Shell("streaming child stdout unavailable".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| DaemonError::Shell("streaming child stderr unavailable".to_owned()))?;
        let stdout_task = stream_lines(stdout, Arc::clone(&on_line));
        let stderr_task = stream_lines(stderr, on_line);
        let wait = async {
            tokio::select! {
                () = cancel.cancelled() => {
                    child.kill().await.map_err(|error| DaemonError::Shell(format!("{description}: {error}")))?;
                    let _status = child.wait().await;
                    Err(DaemonError::Cancelled)
                }
                status = child.wait() => status
                    .map(|status| ShellResult {
                        status: status.code().unwrap_or(-1),
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                    .map_err(|error| DaemonError::Shell(format!("{description}: {error}"))),
            }
        };
        let result = if let Some(timeout) = timeout {
            tokio::time::timeout(timeout, wait)
                .await
                .map_err(|_| DaemonError::Timeout(description))?
        } else {
            wait.await
        };
        let (_stdout_result, _stderr_result) = tokio::join!(stdout_task, stderr_task);
        result
    }
}

fn build_command(command: &ShellCommand) -> Command {
    let mut process = Command::new(&command.program);
    process.args(&command.args).envs(&command.env);
    if let Some(cwd) = &command.cwd {
        process.current_dir(cwd);
    }
    process
}

fn describe(command: &ShellCommand) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn stream_lines<R>(reader: R, on_line: LineCallback)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        on_line(line);
    }
}

fn concise_output<'a>(stderr: &'a str, stdout: &'a str) -> &'a str {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        stdout.trim()
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_child_survives_runtime_shutdown() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let marker = temp.path().join("finished");
        let log = temp.path().join("detached.log");
        let runtime = tokio::runtime::Runtime::new().unwrap_or_else(|error| panic!("{error}"));
        runtime
            .block_on(RealShell.run_detached(
                ShellCommand::new("sh").args([
                    "-c".to_owned(),
                    "sleep 0.2; touch \"$1\"".to_owned(),
                    "fleet-detached-test".to_owned(),
                    marker.to_string_lossy().into_owned(),
                ]),
                &log,
            ))
            .unwrap_or_else(|error| panic!("{error}"));
        drop(runtime);

        std::thread::sleep(Duration::from_millis(400));
        assert!(
            marker.exists(),
            "detached child was killed with its runtime"
        );
    }
}
