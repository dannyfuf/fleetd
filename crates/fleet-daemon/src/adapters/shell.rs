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
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{DaemonError, DaemonResult};

const STREAM_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

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
        let mut child = process
            .spawn()
            .map_err(|error| DaemonError::Shell(format!("{description}: {error}")))?;
        let pid = child
            .id()
            .ok_or_else(|| DaemonError::Shell(format!("{description}: child has no pid")))?;
        // A detached child survives runtime shutdown; while running, the daemon reaps it.
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) if status.success() => {}
                Ok(status) => tracing::warn!(pid, %status, %description, "detached command failed"),
                Err(error) => {
                    tracing::warn!(pid, %error, %description, "failed to reap detached command")
                }
            }
        });
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
            .process_group(0)
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
        let pid = child
            .id()
            .ok_or_else(|| DaemonError::Shell(format!("{description}: child has no pid")))?;
        let mut drains = JoinSet::new();
        let stdout_callback = Arc::clone(&on_line);
        drains.spawn(async move { ("stdout", stream_lines(stdout, stdout_callback).await) });
        drains.spawn(async move { ("stderr", stream_lines(stderr, on_line).await) });

        let deadline = async {
            match timeout {
                Some(timeout) => tokio::time::sleep(timeout).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(deadline);
        let completion = tokio::select! {
            () = cancel.cancelled() => StreamingCompletion::Cancelled,
            status = child.wait() => StreamingCompletion::Exited(status),
            () = &mut deadline => StreamingCompletion::TimedOut,
        };

        match completion {
            StreamingCompletion::Exited(status) => {
                let status = status
                    .map_err(|error| DaemonError::Shell(format!("{description}: {error}")))?;
                finish_streams(&mut drains, &description).await?;
                Ok(ShellResult {
                    status: status.code().unwrap_or(-1),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            StreamingCompletion::Cancelled => {
                terminate_process_group(&mut child, pid, &description).await?;
                let _ignored = finish_streams(&mut drains, &description).await;
                Err(DaemonError::Cancelled)
            }
            StreamingCompletion::TimedOut => {
                terminate_process_group(&mut child, pid, &description).await?;
                let _ignored = finish_streams(&mut drains, &description).await;
                Err(DaemonError::Timeout(description))
            }
        }
    }
}

enum StreamingCompletion {
    Exited(std::io::Result<std::process::ExitStatus>),
    Cancelled,
    TimedOut,
}

async fn terminate_process_group(
    child: &mut tokio::process::Child,
    pid: u32,
    description: &str,
) -> DaemonResult<()> {
    let group_pid = i32::try_from(pid)
        .map_err(|_| DaemonError::Shell(format!("{description}: invalid child pid {pid}")))?;
    // SAFETY: the child was placed in a process group whose id equals its pid; a negative id
    // targets that group and SIGKILL cannot be caught by descendants holding inherited pipes.
    let killed = unsafe { libc::kill(-group_pid, libc::SIGKILL) };
    let kill_error = std::io::Error::last_os_error();
    if killed != 0 && kill_error.raw_os_error() != Some(libc::ESRCH) {
        let _ignored = child.kill().await;
        return Err(DaemonError::Shell(format!(
            "{description}: kill process group {pid}: {kill_error}"
        )));
    }
    tokio::time::timeout(STREAM_DRAIN_TIMEOUT, child.wait())
        .await
        .map_err(|_| DaemonError::Shell(format!("{description}: child did not exit after kill")))?
        .map_err(|error| DaemonError::Shell(format!("{description}: reap child: {error}")))?;
    Ok(())
}

async fn finish_streams(
    drains: &mut JoinSet<(&'static str, std::io::Result<()>)>,
    description: &str,
) -> DaemonResult<()> {
    let result = tokio::time::timeout(STREAM_DRAIN_TIMEOUT, async {
        while let Some(joined) = drains.join_next().await {
            let (name, result) = joined.map_err(|error| {
                DaemonError::Shell(format!("{description}: stream task failed: {error}"))
            })?;
            result.map_err(|error| {
                DaemonError::Shell(format!("{description}: read {name}: {error}"))
            })?;
        }
        Ok(())
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            drains.abort_all();
            Err(DaemonError::Shell(format!(
                "{description}: stream drain timed out"
            )))
        }
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

async fn stream_lines<R>(reader: R, on_line: LineCallback) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        on_line(line);
    }
    Ok(())
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
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use tokio::io::{AsyncRead, ReadBuf};

    use super::*;

    #[tokio::test]
    async fn verbose_child_cannot_deadlock() {
        let lines = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&lines);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            RealShell.run_streaming(
                ShellCommand::new("sh").args(["-c", "yes fleet | head -n 20000"]),
                CancellationToken::new(),
                Arc::new(move |_| {
                    observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }),
            ),
        )
        .await
        .expect("verbose command completes before deadline")
        .expect("verbose command succeeds");
        assert!(result.success());
        assert_eq!(lines.load(std::sync::atomic::Ordering::Relaxed), 20_000);
    }

    #[tokio::test]
    async fn cancel_kills_descendants_and_bounds_drain() {
        let cancel = CancellationToken::new();
        let (pid_sender, mut pid_receiver) = tokio::sync::mpsc::unbounded_channel();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            RealShell
                .run_streaming(
                    ShellCommand::new("sh").args(["-c", "sleep 30 & echo $!; wait"]),
                    task_cancel,
                    Arc::new(move |line| {
                        if let Ok(pid) = line.parse() {
                            let _ignored = pid_sender.send(pid);
                        }
                    }),
                )
                .await
        });
        let descendant = tokio::time::timeout(Duration::from_secs(2), pid_receiver.recv())
            .await
            .expect("descendant pid is reported")
            .expect("pid channel remains open");

        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("cancellation bounds child and pipe cleanup")
            .expect("streaming task does not panic");
        assert!(matches!(result, Err(DaemonError::Cancelled)));

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while crate::adapters::process::pid_is_alive(descendant)
            && tokio::time::Instant::now() < deadline
        {
            tokio::task::yield_now().await;
        }
        assert!(
            !crate::adapters::process::pid_is_alive(descendant),
            "descendant survived cancellation"
        );
    }

    #[tokio::test]
    async fn stream_read_failure_fails_command() {
        let error = stream_lines(BrokenReader::default(), Arc::new(|_| {}))
            .await
            .expect_err("read failure must propagate");
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
    }

    #[derive(Default)]
    struct BrokenReader {
        emitted_line: bool,
    }

    impl AsyncRead for BrokenReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.emitted_line {
                Poll::Ready(Err(std::io::Error::other("injected read failure")))
            } else {
                self.emitted_line = true;
                buffer.put_slice(b"partial output\n");
                Poll::Ready(Ok(()))
            }
        }
    }

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
