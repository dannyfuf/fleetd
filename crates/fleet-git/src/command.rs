//! Tokio-based, shell-free Git process runner.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{broadcast, oneshot},
};

use crate::{
    command_log::{CommandLog, CommandOutcome, CommandRecord, preview, redact_arg},
    error::{GitError, Result},
    model::CommandKind,
};

/// Deadline for local reads and mutations, which are CPU- and disk-bound.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// Deadline for [`CommandKind::Network`], which waits on a remote and a link fleetd does not
/// control. It stays bounded because a stalled TCP connection or an SSH askpass would otherwise
/// hold the per-repository mutation lock forever.
const DEFAULT_NETWORK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_PREVIEW_LIMIT: usize = 16 * 1024;
const DEFAULT_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

/// Complete argv/env/cwd specification for one `git` child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitCommand {
    /// Ordered arguments after the `git` executable.
    pub(crate) argv: Vec<OsString>,
    /// Child working directory.
    pub(crate) cwd: PathBuf,
    /// Environment additions or replacements.
    pub(crate) env: BTreeMap<OsString, OsString>,
    /// Optional bytes written to stdin.
    pub(crate) stdin: Option<Vec<u8>>,
    /// Read/mutation/network marker.
    pub(crate) kind: CommandKind,
    /// Apply literal pathspec semantics.
    pub(crate) literal_pathspecs: bool,
    /// Disable optional Git locks for background reads.
    pub(crate) background_read: bool,
    /// Extra accepted process exit codes.
    pub(crate) accepted_exit_codes: Vec<i32>,
    /// Whether a non-zero exit may mean "stopped with merge conflicts".
    pub(crate) may_conflict: bool,
}

impl GitCommand {
    /// Creates an empty command rooted at `cwd`.
    #[must_use]
    pub(crate) fn new(cwd: impl Into<PathBuf>, kind: CommandKind) -> Self {
        Self {
            argv: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
            stdin: None,
            kind,
            literal_pathspecs: false,
            background_read: kind == CommandKind::Read,
            accepted_exit_codes: Vec::new(),
            may_conflict: false,
        }
    }

    /// Appends one argument.
    #[must_use]
    pub(crate) fn arg(mut self, value: impl Into<OsString>) -> Self {
        self.argv.push(value.into());
        self
    }

    /// Appends ordered arguments.
    #[must_use]
    pub(crate) fn args(mut self, values: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.argv.extend(values.into_iter().map(Into::into));
        self
    }

    /// Appends `value` only when `condition` holds.
    #[must_use]
    pub(crate) fn arg_if(self, condition: bool, value: impl Into<OsString>) -> Self {
        if condition { self.arg(value) } else { self }
    }

    /// Appends `value` when it is present.
    #[must_use]
    pub(crate) fn arg_opt(self, value: Option<impl Into<OsString>>) -> Self {
        match value {
            Some(value) => self.arg(value),
            None => self,
        }
    }

    /// Appends `--` followed by literal repository paths.
    #[must_use]
    pub(crate) fn paths(mut self, paths: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Self {
        self.argv.push(OsString::from("--"));
        self.argv
            .extend(paths.into_iter().map(|path| path.as_ref().to_os_string()));
        self.literal_pathspecs = true;
        self
    }

    /// Adds or replaces one environment variable.
    #[must_use]
    pub(crate) fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Supplies bytes on stdin.
    #[must_use]
    pub(crate) fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }

    /// Allows Git to take optional locks for a foreground read.
    #[must_use]
    pub(crate) fn foreground_read(mut self) -> Self {
        self.background_read = false;
        self
    }

    /// Treats an additional exit code as successful.
    #[must_use]
    pub(crate) fn accept_exit_code(mut self, status: i32) -> Self {
        self.accepted_exit_codes.push(status);
        self
    }

    /// Marks the command as one whose failure can mean unresolved conflicts.
    ///
    /// [`Repository`](crate::Repository) only reports [`GitError::Conflict`] for
    /// commands carrying this marker.
    #[must_use]
    pub(crate) fn may_conflict(mut self) -> Self {
        self.may_conflict = true;
        self
    }
}

/// Byte-preserving output from an accepted Git invocation.
#[derive(Debug)]
pub(crate) struct GitOutput {
    /// Complete standard output.
    pub(crate) stdout: Vec<u8>,
    /// Complete standard error.
    pub(crate) stderr: Vec<u8>,
    /// Final bounded command-log record, including status and elapsed time.
    pub(crate) record: CommandRecord,
}

/// Cloneable process runner with live and recent command logs.
#[derive(Debug, Clone)]
pub struct Runner {
    inner: Arc<RunnerInner>,
    git_program: OsString,
    env: Arc<BTreeMap<OsString, OsString>>,
    output_limit: usize,
    read_timeout: Duration,
    network_timeout: Duration,
}

#[derive(Debug)]
struct RunnerInner {
    next_id: AtomicU64,
    log: CommandLog,
}

impl Default for Runner {
    fn default() -> Self {
        Self::new(256, 256)
    }
}

impl Runner {
    /// Creates a runner with bounded recent-record and broadcast capacities.
    fn new(recent_capacity: usize, event_capacity: usize) -> Self {
        Self {
            inner: Arc::new(RunnerInner {
                next_id: AtomicU64::new(1),
                log: CommandLog::new(recent_capacity, event_capacity),
            }),
            git_program: OsString::from("git"),
            env: Arc::new(BTreeMap::new()),
            output_limit: DEFAULT_OUTPUT_LIMIT,
            read_timeout: DEFAULT_TIMEOUT,
            network_timeout: DEFAULT_NETWORK_TIMEOUT,
        }
    }

    /// Adds an environment variable applied to every child from this runner.
    ///
    /// The supported way to configure a runner: [`Repository::discover_with_runner`] takes the
    /// result, and Git configuration isolation (`GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM`) is
    /// expressed here rather than per command.
    ///
    /// [`Repository::discover_with_runner`]: crate::Repository::discover_with_runner
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        Arc::make_mut(&mut self.env).insert(key.into(), value.into());
        self
    }

    /// Replaces the `git` executable, for tests that script a fake `git`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_program(mut self, program: impl Into<OsString>) -> Self {
        self.git_program = program.into();
        self
    }

    /// Subscribes to started and finished command events.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<crate::CommandEvent> {
        self.inner.log.subscribe()
    }

    /// Returns a snapshot of the bounded command history.
    pub(crate) fn recent(&self) -> Vec<CommandRecord> {
        self.inner.log.recent()
    }

    /// Picks the deadline for one command.
    ///
    /// A fetch, pull, or push waits on a remote and on a link fleetd does not control, so it
    /// cannot share the deadline that bounds a local read or mutation.
    fn deadline_for(&self, kind: CommandKind) -> Duration {
        match kind {
            CommandKind::Read | CommandKind::Mutation => self.read_timeout,
            CommandKind::Network => self.network_timeout,
        }
    }

    /// Runs one command without shell interpolation.
    pub(crate) async fn run(&self, specification: GitCommand) -> Result<GitOutput> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let display_argv = display_argv(&self.git_program, &specification.argv);
        let started_at = SystemTime::now();
        let started = CommandRecord {
            id,
            kind: specification.kind,
            display_argv: display_argv.clone(),
            started_at,
            elapsed: None,
            outcome: CommandOutcome::Running,
        };
        self.inner.log.started(started);

        let start = Instant::now();
        let mut process = Command::new(&self.git_program);
        process
            .args(&specification.argv)
            .current_dir(&specification.cwd)
            .envs(self.env.as_ref())
            .envs(&specification.env)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if specification.literal_pathspecs {
            process.env("GIT_LITERAL_PATHSPECS", "1");
        }
        if specification.background_read {
            process.env("GIT_OPTIONAL_LOCKS", "0");
        }
        if specification.stdin.is_some() {
            process.stdin(Stdio::piped());
        } else {
            process.stdin(Stdio::null());
        }

        let child = match process.spawn() {
            Ok(child) => child,
            Err(source) => {
                let record = Self::spawn_error_record(
                    id,
                    specification.kind,
                    display_argv.clone(),
                    started_at,
                    start.elapsed(),
                    &source,
                );
                self.inner.log.finished(record);
                return Err(GitError::Spawn {
                    argv: display_argv,
                    source,
                });
            }
        };

        let cancellation_record = CommandRecord {
            id,
            kind: specification.kind,
            display_argv: display_argv.clone(),
            started_at,
            elapsed: None,
            outcome: CommandOutcome::Cancelled,
        };
        let (cancel_sender, cancel_receiver) = oneshot::channel();
        let mut cancellation = CancellationGuard::new(
            cancel_sender,
            self.inner.clone(),
            cancellation_record.clone(),
            start,
        );
        let mut supervisor = tokio::spawn(supervise_process(
            child,
            specification.stdin,
            self.output_limit,
            cancel_receiver,
            self.inner.clone(),
            cancellation_record,
            start,
        ));

        let deadline = self.deadline_for(specification.kind);
        let output = match tokio::time::timeout(deadline, &mut supervisor).await {
            Ok(Ok(Ok(output))) => {
                cancellation.disarm();
                output
            }
            Ok(Ok(Err(source))) => {
                cancellation.disarm();
                let record = Self::spawn_error_record(
                    id,
                    specification.kind,
                    display_argv.clone(),
                    started_at,
                    start.elapsed(),
                    &source,
                );
                self.inner.log.finished(record);
                return Err(GitError::Spawn {
                    argv: display_argv,
                    source,
                });
            }
            Ok(Err(source)) => {
                cancellation.disarm();
                let source =
                    std::io::Error::other(format!("Git process supervisor failed: {source}"));
                let record = Self::spawn_error_record(
                    id,
                    specification.kind,
                    display_argv.clone(),
                    started_at,
                    start.elapsed(),
                    &source,
                );
                self.inner.log.finished(record);
                return Err(GitError::Spawn {
                    argv: display_argv,
                    source,
                });
            }
            Err(_) => {
                cancellation.cancel(CancelReason::Timeout);
                let _ = supervisor.await;
                let record = CommandRecord {
                    id,
                    kind: specification.kind,
                    display_argv: display_argv.clone(),
                    started_at,
                    elapsed: Some(start.elapsed()),
                    outcome: CommandOutcome::TimedOut,
                };
                self.inner.log.finished(record);
                return Err(GitError::Timeout {
                    argv: display_argv,
                    timeout: deadline,
                });
            }
        };

        if output.stdout.exceeded || output.stderr.exceeded {
            let source = std::io::Error::other(format!(
                "Git output exceeded the {} byte capture limit",
                self.output_limit
            ));
            let record = CommandRecord {
                id,
                kind: specification.kind,
                display_argv: display_argv.clone(),
                started_at,
                elapsed: Some(start.elapsed()),
                outcome: CommandOutcome::OutputLimitExceeded,
            };
            self.inner.log.finished(record);
            return Err(GitError::Spawn {
                argv: display_argv,
                source,
            });
        }

        let elapsed = start.elapsed();
        let accepted = output.status.success()
            || output
                .status
                .code()
                .is_some_and(|code| specification.accepted_exit_codes.contains(&code));
        let outcome = if accepted {
            CommandOutcome::Success {
                status: output.status.code(),
                stdout_preview: preview(&output.stdout.bytes, DEFAULT_PREVIEW_LIMIT),
                stderr_preview: preview(&output.stderr.bytes, DEFAULT_PREVIEW_LIMIT),
            }
        } else {
            CommandOutcome::Failed {
                status: output.status.code(),
                stdout_preview: preview(&output.stdout.bytes, DEFAULT_PREVIEW_LIMIT),
                stderr_preview: preview(&output.stderr.bytes, DEFAULT_PREVIEW_LIMIT),
            }
        };
        let record = CommandRecord {
            id,
            kind: specification.kind,
            display_argv: display_argv.clone(),
            started_at,
            elapsed: Some(elapsed),
            outcome,
        };
        self.inner.log.finished(record.clone());
        if !accepted {
            let message = concise_output(&output.stderr.bytes, &output.stdout.bytes);
            return Err(GitError::Exit {
                status: output.status.code(),
                stdout: output.stdout.bytes,
                stderr: output.stderr.bytes,
                argv: display_argv,
                message,
            });
        }
        Ok(GitOutput {
            stdout: output.stdout.bytes,
            stderr: output.stderr.bytes,
            record,
        })
    }

    fn spawn_error_record(
        id: u64,
        kind: CommandKind,
        display_argv: Vec<String>,
        started_at: SystemTime,
        elapsed: Duration,
        source: &std::io::Error,
    ) -> CommandRecord {
        CommandRecord {
            id,
            kind,
            display_argv,
            started_at,
            elapsed: Some(elapsed),
            outcome: CommandOutcome::SpawnFailed(source.to_string()),
        }
    }
}

#[derive(Debug)]
struct ProcessOutput {
    status: ExitStatus,
    stdout: BoundedOutput,
    stderr: BoundedOutput,
}

#[derive(Debug)]
struct BoundedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

#[derive(Debug, Clone, Copy)]
enum CancelReason {
    CallerDropped,
    Timeout,
}

struct CancellationGuard {
    sender: Option<oneshot::Sender<CancelReason>>,
    inner: Arc<RunnerInner>,
    record: CommandRecord,
    start: Instant,
}

impl CancellationGuard {
    fn new(
        sender: oneshot::Sender<CancelReason>,
        inner: Arc<RunnerInner>,
        record: CommandRecord,
        start: Instant,
    ) -> Self {
        Self {
            sender: Some(sender),
            inner,
            record,
            start,
        }
    }

    fn cancel(&mut self, reason: CancelReason) {
        let Some(sender) = self.sender.take() else {
            return;
        };
        if matches!(sender.send(reason), Err(CancelReason::CallerDropped)) {
            self.record.elapsed = Some(self.start.elapsed());
            self.inner.log.finished(self.record.clone());
        }
    }

    fn disarm(&mut self) {
        self.sender = None;
    }
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        self.cancel(CancelReason::CallerDropped);
    }
}

async fn supervise_process(
    mut child: Child,
    stdin: Option<Vec<u8>>,
    output_limit: usize,
    mut cancel_receiver: oneshot::Receiver<CancelReason>,
    inner: Arc<RunnerInner>,
    mut cancellation_record: CommandRecord,
    start: Instant,
) -> std::io::Result<ProcessOutput> {
    let stdin_pipe = child.stdin.take();
    let stdout_pipe = child.stdout.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Git stdout was not piped")
    })?;
    let stderr_pipe = child.stderr.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Git stderr was not piped")
    })?;
    enum Supervision {
        Completed(std::io::Result<ProcessOutput>),
        Cancelled(std::result::Result<CancelReason, oneshot::error::RecvError>),
    }

    let supervision = {
        let communication = async {
            let write_stdin = async move {
                if let Some(bytes) = stdin {
                    let mut pipe = stdin_pipe.ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "Git stdin was not piped",
                        )
                    })?;
                    pipe.write_all(&bytes).await?;
                }
                Ok::<(), std::io::Error>(())
            };
            let (stdin_result, stdout, stderr, status) = tokio::join!(
                write_stdin,
                read_bounded(stdout_pipe, output_limit),
                read_bounded(stderr_pipe, output_limit),
                child.wait(),
            );
            stdin_result?;
            Ok(ProcessOutput {
                status: status?,
                stdout: stdout?,
                stderr: stderr?,
            })
        };
        tokio::pin!(communication);
        tokio::select! {
            result = &mut communication => Supervision::Completed(result),
            reason = &mut cancel_receiver => Supervision::Cancelled(reason),
        }
    };

    match supervision {
        Supervision::Completed(result) => {
            if matches!(cancel_receiver.try_recv(), Ok(CancelReason::CallerDropped)) {
                cancellation_record.elapsed = Some(start.elapsed());
                inner.log.finished(cancellation_record);
            }
            result
        }
        Supervision::Cancelled(reason) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            if matches!(reason, Ok(CancelReason::CallerDropped)) {
                cancellation_record.elapsed = Some(start.elapsed());
                inner.log.finished(cancellation_record);
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Git command cancelled",
            ))
        }
    }
}

async fn read_bounded(
    mut pipe: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<BoundedOutput> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    let mut exceeded = false;
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        exceeded |= count > remaining;
    }
    Ok(BoundedOutput { bytes, exceeded })
}

fn display_argv(program: &OsStr, arguments: &[OsString]) -> Vec<String> {
    std::iter::once(program)
        .chain(arguments.iter().map(OsString::as_os_str))
        .map(|argument| redact_arg(&argument.to_string_lossy()))
        .collect()
}

fn concise_output(stderr: &[u8], stdout: &[u8]) -> String {
    let bytes = if stderr.is_empty() { stdout } else { stderr };
    redact_arg(String::from_utf8_lossy(bytes).trim())
}

#[cfg(all(test, unix))]
mod tests {
    use super::{CancelReason, GitCommand, Runner, supervise_process};
    use crate::{CommandEvent, CommandKind, CommandOutcome, CommandRecord, GitError};
    use std::{
        process::Stdio,
        time::{Duration, Instant, SystemTime},
    };
    use tokio::{process::Command, sync::oneshot};

    /// Runs `/bin/sh` in place of `git` so the test can echo its environment.
    fn shell_runner() -> Runner {
        Runner {
            git_program: "/bin/sh".into(),
            ..Runner::default()
        }
    }

    #[tokio::test]
    async fn cloned_runners_share_history_and_keep_independent_environment() {
        let original = shell_runner().env("FLEET_RUNNER_TEST_CONFIG", "original");
        let changed = original.clone().env("FLEET_RUNNER_TEST_CONFIG", "changed");
        let command = || {
            GitCommand::new(std::env::temp_dir(), CommandKind::Read)
                .args(["-c", "printf '%s' \"$FLEET_RUNNER_TEST_CONFIG\""])
        };
        let first = original.run(command()).await.unwrap();
        let second = changed.run(command()).await.unwrap();
        assert_eq!(first.stdout, b"original");
        assert_eq!(second.stdout, b"changed");
        assert_eq!(original.recent(), changed.recent());
        assert_ne!(first.record.id, second.record.id);
    }

    #[tokio::test]
    async fn chatty_child_large_stdin_completes() {
        let runner = shell_runner();
        let stdin = vec![b'x'; 512 * 1024];
        let command = GitCommand::new(std::env::temp_dir(), CommandKind::Read)
            .args(["-c", "dd if=/dev/zero bs=65536 count=4 2>/dev/null; wc -c"])
            .stdin(stdin);

        let output = tokio::time::timeout(Duration::from_secs(10), runner.run(command))
            .await
            .expect("concurrent pipe pumping should not deadlock")
            .expect("shell command should succeed");

        assert!(output.stdout.len() >= 256 * 1024);
        assert!(output.stdout.ends_with(b"524288\n"));
    }

    #[tokio::test]
    async fn error_message_redacts_without_mutating_output() {
        let runner = shell_runner();
        let stderr = b"fatal: https://user:secret@example.test/org/repo\n";
        let command = GitCommand::new(std::env::temp_dir(), CommandKind::Read).args([
            "-c",
            "printf 'fatal: https://user:secret@example.test/org/repo\\n' >&2; exit 1",
        ]);

        let error = runner.run(command).await.expect_err("command should fail");

        let GitError::Exit {
            message,
            stderr: raw_stderr,
            ..
        } = error
        else {
            panic!("expected exit error");
        };
        assert_eq!(message, "fatal: https://[REDACTED]@example.test/org/repo");
        assert_eq!(raw_stderr, stderr);
    }

    #[tokio::test]
    async fn network_commands_get_their_own_deadline() {
        let mut runner = shell_runner();
        runner.read_timeout = Duration::from_millis(100);
        runner.network_timeout = Duration::from_secs(30);
        assert!(runner.network_timeout > runner.read_timeout);
        let sleeper =
            |kind| GitCommand::new(std::env::temp_dir(), kind).args(["-c", "exec sleep 0.4"]);

        runner
            .run(sleeper(CommandKind::Network))
            .await
            .expect("a network command outlives the read deadline");

        let error = runner
            .run(sleeper(CommandKind::Read))
            .await
            .expect_err("a read is still bounded by the read deadline");
        let GitError::Timeout { timeout, .. } = error else {
            panic!("expected a timeout error");
        };
        assert_eq!(timeout, Duration::from_millis(100));
    }

    #[tokio::test]
    async fn oversized_output_is_bounded() {
        let mut runner = shell_runner();
        runner.output_limit = 1024;
        let command = GitCommand::new(std::env::temp_dir(), CommandKind::Read)
            .args(["-c", "dd if=/dev/zero bs=4096 count=1 2>/dev/null"]);

        let error = runner
            .run(command)
            .await
            .expect_err("output must be capped");

        assert!(error.to_string().contains("capture limit"));
        assert_eq!(
            runner.recent()[0].outcome,
            CommandOutcome::OutputLimitExceeded
        );
    }

    #[tokio::test]
    async fn cancelled_run_finishes_record() {
        let runner = shell_runner();
        let mut events = runner.subscribe();
        let task_runner = runner.clone();
        let task: tokio::task::JoinHandle<_> = tokio::spawn(async move {
            task_runner
                .run(
                    GitCommand::new(std::env::temp_dir(), CommandKind::Read)
                        .args(["-c", "exec sleep 30"]),
                )
                .await
        });
        let started_id = match events.recv().await.expect("started event") {
            CommandEvent::Started(record) => record.id,
            CommandEvent::Finished(_) => panic!("finished before started"),
        };

        task.abort();
        let _ = task.await;
        let finished = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("cancelled supervisor should reap promptly")
            .expect("finished event");

        let CommandEvent::Finished(record) = finished else {
            panic!("expected finished event");
        };
        assert_eq!(record.id, started_id);
        assert_eq!(record.outcome, CommandOutcome::Cancelled);
        assert_eq!(runner.recent(), vec![record]);
    }

    #[tokio::test]
    async fn caller_drop_after_child_exit_finishes_record() {
        let runner = shell_runner();
        let mut events = runner.subscribe();

        for id in 1..=32 {
            let mut child = Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn child");
            while child.try_wait().expect("inspect child").is_none() {
                tokio::task::yield_now().await;
            }

            let start = Instant::now();
            let running_record = CommandRecord {
                id,
                kind: CommandKind::Read,
                display_argv: vec!["/bin/sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()],
                started_at: SystemTime::now(),
                elapsed: None,
                outcome: CommandOutcome::Running,
            };
            runner.inner.log.started(running_record.clone());
            let CommandEvent::Started(started) = events.try_recv().expect("started event") else {
                panic!("finished before started");
            };
            assert_eq!(started.id, id);
            let cancellation_record = CommandRecord {
                outcome: CommandOutcome::Cancelled,
                ..running_record
            };
            let (cancel_sender, cancel_receiver) = oneshot::channel();
            cancel_sender
                .send(CancelReason::CallerDropped)
                .expect("supervisor should receive cancellation");

            let _ = supervise_process(
                child,
                None,
                1024,
                cancel_receiver,
                runner.inner.clone(),
                cancellation_record,
                start,
            )
            .await;

            let CommandEvent::Finished(finished) =
                events.try_recv().expect("caller drop should finish record")
            else {
                panic!("expected finished event");
            };
            assert_eq!(finished.id, id);
            assert_eq!(finished.outcome, CommandOutcome::Cancelled);
            assert_eq!(
                runner.recent().last().map(|record| &record.outcome),
                Some(&CommandOutcome::Cancelled)
            );
        }
    }
}
