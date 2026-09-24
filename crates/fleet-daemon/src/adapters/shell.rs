//! Shell command execution abstractions.

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
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
/// Maximum encoded size of one streamed line, including its truncation marker.
pub(crate) const STREAM_LINE_MAX_BYTES: usize = 64 * 1024;
pub(crate) const STREAM_LINE_TRUNCATION_MARKER: &str = " [fleet: line truncated]";

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
    /// Whether the child starts from an empty environment, so `env` is its whole environment
    /// rather than additions to the daemon's own: what a caller that has already filtered a
    /// complete environment (a scheduled agent run, BOARD §12.3) needs.
    pub clear_env: bool,
    /// Optional execution timeout.
    pub timeout: Option<Duration>,
    /// Whether a streaming child's whole process group is killed as soon as the child itself
    /// exits: what a scheduled agent run needs, whose descendants (MCP servers, tool shells)
    /// must not outlive it (BOARD §12.3).
    pub kill_group_on_exit: bool,
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
            clear_env: false,
            timeout: None,
            kill_group_on_exit: false,
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

    /// Starts the child from an empty environment: `env` becomes the whole of it.
    #[must_use]
    pub fn clear_env(mut self) -> Self {
        self.clear_env = true;
        self
    }

    /// Sets the process deadline.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Kills a streaming child's process group the moment the child exits.
    #[must_use]
    pub fn kill_group_on_exit(mut self) -> Self {
        self.kill_group_on_exit = true;
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

/// Future returned by a streaming line callback.
pub type LineCallbackFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Callback receiving complete, lossily decoded stdout and stderr lines from a streaming child.
///
/// The future lets a bounded consumer apply backpressure without blocking a Tokio worker. Stream
/// drain tasks remain abortable while they wait for the consumer.
pub type LineCallback = Arc<dyn Fn(String) -> LineCallbackFuture + Send + Sync>;

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

    /// Runs a child while streaming lossily decoded lines and honoring explicit cancellation.
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
                .map_err(|_| DaemonError::Timeout(subcommand(&command)))?
        } else {
            child.output().await
        }
        .map_err(|error| {
            // `subcommand`, for the reason a timeout uses it: a spawn failure that is not
            // ENOENT is classified as transient and its message is persisted in
            // `board.sync.last_error` and printed by the CLI and the app, so the full argv
            // would publish an issue summary and an assignee's address to all three. The argv
            // itself stays in the debug log.
            tracing::debug!(command = %description, %error, "shell command failed");
            DaemonError::Shell(format!("{}: {error}", subcommand(&command)))
        })?;
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
            .map_err(|error| {
                tracing::debug!(command = %description, %error, "streaming shell command failed to spawn");
                DaemonError::Shell(format!("{}: {error}", subcommand(&command)))
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DaemonError::Shell("streaming child stdout unavailable".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| DaemonError::Shell("streaming child stderr unavailable".to_owned()))?;
        let pid = child.id().ok_or_else(|| {
            tracing::debug!(command = %description, "streaming shell child has no pid");
            DaemonError::Shell(format!("{}: child has no pid", subcommand(&command)))
        })?;
        // `kill_on_drop` reaches the leader only; the group guard takes its descendants too when
        // this future is dropped mid-run, as it is when the daemon shuts down.
        let mut group = GroupGuard::new(pid);
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
        // From here the group is either reaped (the leader exited) or killed below, and its id
        // may be reused by an unrelated process once it is gone.
        group.disarm();

        // Errors name the program and its subcommands only: the arguments of an agent run carry
        // the whole prompt, which must not become a run's summary or a log line.
        let label = subcommand(&command);
        match completion {
            StreamingCompletion::Exited(status) => {
                let status =
                    status.map_err(|error| DaemonError::Shell(format!("{label}: {error}")))?;
                // The leader is reaped, but a group whose members remain keeps its id, so the
                // negative id still names only them.
                if command.kill_group_on_exit {
                    kill_group(pid);
                }
                match finish_streams(&mut drains, &label).await {
                    Ok(()) => {}
                    // The child's own output is complete; what holds the pipes open is a
                    // descendant it left behind, and that must not turn a finished command into
                    // a failed one, nor stay running.
                    Err(DaemonError::Timeout(_)) => {
                        tracing::warn!(command = %label, "a finished command's descendants kept its output open; killing them");
                        kill_group(pid);
                    }
                    Err(error) => return Err(error),
                }
                Ok(ShellResult {
                    status: status.code().unwrap_or(-1),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
            StreamingCompletion::Cancelled => {
                terminate_process_group(&mut child, pid, &label).await?;
                let _ignored = finish_streams(&mut drains, &label).await;
                Err(DaemonError::Cancelled)
            }
            StreamingCompletion::TimedOut => {
                terminate_process_group(&mut child, pid, &label).await?;
                let _ignored = finish_streams(&mut drains, &label).await;
                Err(DaemonError::Timeout(label))
            }
        }
    }
}

/// Kills a streaming child's whole process group when the future running it is dropped before
/// the child finished.
///
/// A scheduled agent (`claude -p`) starts MCP servers and tool shells in its group; without this
/// a daemon shutdown killed the leader through `kill_on_drop` and left the rest orphaned.
struct GroupGuard {
    group: Option<i32>,
}

impl GroupGuard {
    fn new(pid: u32) -> Self {
        Self {
            group: i32::try_from(pid).ok(),
        }
    }

    fn disarm(&mut self) {
        self.group = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(group) = self.group {
            // SAFETY: the child was placed in a process group whose id equals its pid and has not
            // been reaped (the guard is disarmed first), so the negative id names that group.
            let killed = unsafe { libc::kill(-group, libc::SIGKILL) };
            if killed != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(group, %error, "could not kill a dropped child's process group");
                }
            }
        }
    }
}

/// Kills what is left of an exited streaming child's process group; a group already empty is
/// not an error.
fn kill_group(pid: u32) {
    let Ok(group) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: the child was placed in a process group whose id equals its pid. Its leader is
    // reaped, but the id cannot be reused while any member of the group remains, so the negative
    // id names only those members, or nothing (`ESRCH`).
    let killed = unsafe { libc::kill(-group, libc::SIGKILL) };
    if killed != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(group, %error, "could not kill an exited child's process group");
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
            Err(DaemonError::Timeout(format!("{description}: stream drain")))
        }
    }
}

fn build_command(command: &ShellCommand) -> Command {
    let mut process = Command::new(&command.program);
    if command.clear_env {
        process.env_clear();
    }
    process.args(&command.args).envs(&command.env);
    if let Some(cwd) = &command.cwd {
        process.current_dir(cwd);
    }
    process
}

/// What a timed-out call is named as: the program and its leading subcommands, never its
/// values.
///
/// A timeout is the one failure whose text is persisted (`board.sync.last_error`) and printed
/// by the job log, the CLI and the app. The full argv carries what the call was *about* — an
/// issue summary, an assignee's email address, a branch name — into all three.
fn subcommand(command: &ShellCommand) -> String {
    std::iter::once(command.program.as_str())
        .chain(
            command
                .args
                .iter()
                .map(String::as_str)
                .take_while(|argument| !argument.starts_with('-'))
                .take(3),
        )
        .collect::<Vec<_>>()
        .join(" ")
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
    let mut reader = BufReader::new(reader);
    let mut line = Vec::with_capacity(STREAM_LINE_MAX_BYTES);
    let mut truncated = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if !line.is_empty() || truncated {
                on_line(finish_stream_line(&mut line, truncated)).await;
            }
            break;
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        let content = &available[..newline.unwrap_or(available.len())];
        if !truncated {
            let remaining = STREAM_LINE_MAX_BYTES.saturating_sub(line.len());
            line.extend_from_slice(&content[..content.len().min(remaining)]);
            truncated = content.len() > remaining;
        }
        reader.consume(consumed);

        if newline.is_some() {
            on_line(finish_stream_line(&mut line, truncated)).await;
            truncated = false;
        }
    }
    Ok(())
}

fn finish_stream_line(buffer: &mut Vec<u8>, truncated: bool) -> String {
    while buffer.last() == Some(&b'\r') {
        buffer.pop();
    }
    let mut line = String::from_utf8_lossy(buffer).into_owned();
    buffer.clear();
    cap_stream_line(&mut line, truncated);
    line
}

pub(crate) fn cap_stream_line(line: &mut String, force_marker: bool) {
    if force_marker || line.len() > STREAM_LINE_MAX_BYTES {
        let limit = STREAM_LINE_MAX_BYTES - STREAM_LINE_TRUNCATION_MARKER.len();
        let mut boundary = limit.min(line.len());
        while !line.is_char_boundary(boundary) {
            boundary -= 1;
        }
        line.truncate(boundary);
        line.push_str(STREAM_LINE_TRUNCATION_MARKER);
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
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use tokio::io::{AsyncRead, ReadBuf};

    use super::*;

    /// A failure whose text is persisted (`board.sync.last_error`) and printed by the job log,
    /// the CLI and the app: the argv it used to carry held issue summaries and assignee email
    /// addresses. Both the timeout and the spawn failure are named by the subcommand alone —
    /// a spawn failure that is not ENOENT is classified transient and reported the same way.
    #[test]
    fn a_failed_call_is_named_by_its_subcommand_and_not_by_its_values() {
        let command = ShellCommand::new("acli").args([
            "jira".to_owned(),
            "workitem".to_owned(),
            "edit".to_owned(),
            "--key".to_owned(),
            "SP-1".to_owned(),
            "--summary".to_owned(),
            "Pay the December bonus to ana@example.com".to_owned(),
        ]);
        assert_eq!(subcommand(&command), "acli jira workitem edit");
        // The full argv survives for the debug log, which is the one place it belongs.
        assert!(describe(&command).contains("ana@example.com"));
        // A program that cannot be spawned at all reports the same trimmed name.
        let runtime = tokio::runtime::Runtime::new().unwrap_or_else(|error| panic!("{error}"));
        let error = runtime
            .block_on(
                RealShell.run(
                    ShellCommand::new("fleet-no-such-program")
                        .args(["--summary".to_owned(), "ana@example.com".to_owned()]),
                ),
            )
            .expect_err("a program that is not on PATH cannot run");
        let message = error.to_string();
        assert!(!message.contains("ana@example.com"), "{message}");
        assert!(message.contains("fleet-no-such-program"), "{message}");
    }

    #[tokio::test]
    async fn a_streaming_spawn_failure_keeps_the_os_error_and_hides_long_arguments() {
        let secret = "private prompt ".repeat(80);
        let error = RealShell
            .run_streaming(
                ShellCommand::new("fleet-no-such-streaming-program").args([
                    "-p".to_owned(),
                    "--".to_owned(),
                    secret.clone(),
                ]),
                CancellationToken::new(),
                Arc::new(|_| Box::pin(async {})),
            )
            .await
            .expect_err("a program that is not on PATH cannot stream");
        let message = error.to_string();
        assert!(message.contains("No such file or directory"), "{message}");
        assert!(!message.contains(&secret), "{message}");
    }

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
                    Box::pin(async {})
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
    async fn streaming_replaces_invalid_utf8_and_delivers_later_lines() {
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = Arc::clone(&lines);
        let result = RealShell
            .run_streaming(
                ShellCommand::new("sh").args(["-c", "printf 'ok\\n\\377\\nafter\\n'"]),
                CancellationToken::new(),
                Arc::new(move |line| {
                    observed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(line);
                    Box::pin(async {})
                }),
            )
            .await
            .expect("invalid UTF-8 is lossy, not fatal");

        assert!(result.success());
        assert_eq!(
            *lines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["ok", "�", "after"]
        );
    }

    #[tokio::test]
    async fn a_newline_free_stream_is_capped_without_buffering_the_remainder() {
        let input = vec![b'x'; STREAM_LINE_MAX_BYTES * 3];
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = Arc::clone(&lines);

        stream_lines(
            input.as_slice(),
            Arc::new(move |line| {
                observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(line);
                Box::pin(async {})
            }),
        )
        .await
        .expect("the oversized line is drained");

        let lines = lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), STREAM_LINE_MAX_BYTES);
        assert!(lines[0].ends_with(STREAM_LINE_TRUNCATION_MARKER));
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
                        Box::pin(async {})
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

    /// A daemon shutdown drops the run's future rather than cancelling it; the child's whole
    /// group must still go, not only its leader.
    #[tokio::test]
    async fn dropping_a_streaming_run_kills_its_descendants() {
        let (pid_sender, mut pid_receiver) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            RealShell
                .run_streaming(
                    ShellCommand::new("sh").args(["-c", "sleep 30 & echo $!; wait"]),
                    CancellationToken::new(),
                    Arc::new(move |line| {
                        if let Ok(pid) = line.parse() {
                            let _ignored = pid_sender.send(pid);
                        }
                        Box::pin(async {})
                    }),
                )
                .await
        });
        let descendant = tokio::time::timeout(Duration::from_secs(2), pid_receiver.recv())
            .await
            .expect("descendant pid is reported")
            .expect("pid channel remains open");

        task.abort();
        let aborted = task.await.expect_err("the run was dropped mid-flight");
        assert!(aborted.is_cancelled());

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while crate::adapters::process::pid_is_alive(descendant)
            && tokio::time::Instant::now() < deadline
        {
            tokio::task::yield_now().await;
        }
        assert!(
            !crate::adapters::process::pid_is_alive(descendant),
            "descendant survived its dropped run"
        );
    }

    /// A descendant still holding the inherited output does not fail a command that finished,
    /// and does not outlive it.
    #[tokio::test]
    async fn a_descendant_holding_the_output_does_not_fail_a_finished_command() {
        for kill_on_exit in [false, true] {
            let (pid_sender, mut pid_receiver) = tokio::sync::mpsc::unbounded_channel();
            let mut command = ShellCommand::new("sh").args(["-c", "sleep 30 & echo $!"]);
            if kill_on_exit {
                command = command.kill_group_on_exit();
            }
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                RealShell.run_streaming(
                    command,
                    CancellationToken::new(),
                    Arc::new(move |line| {
                        if let Ok(pid) = line.parse::<u32>() {
                            let _ignored = pid_sender.send(pid);
                        }
                        Box::pin(async {})
                    }),
                ),
            )
            .await
            .expect("the drain is bounded")
            .expect("a finished command succeeds");
            assert!(result.success());
            let descendant = pid_receiver.try_recv().expect("descendant pid is reported");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while crate::adapters::process::pid_is_alive(descendant)
                && tokio::time::Instant::now() < deadline
            {
                tokio::task::yield_now().await;
            }
            assert!(
                !crate::adapters::process::pid_is_alive(descendant),
                "descendant survived its finished command (kill on exit: {kill_on_exit})"
            );
        }
    }

    #[tokio::test]
    async fn stream_read_failure_fails_command() {
        let error = stream_lines(BrokenReader::default(), Arc::new(|_| Box::pin(async {})))
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

    #[tokio::test]
    async fn a_cleared_environment_is_exactly_the_one_given() {
        let command = ShellCommand::new("/bin/sh")
            .args([
                "-c",
                "printf '%s|%s' \"${HOME:-unset}\" \"${FLEET_ONLY:-unset}\"",
            ])
            .env("FLEET_ONLY", "kept")
            .clear_env();
        let result = RealShell
            .run(command)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(result.stdout, "unset|kept");
    }
}
