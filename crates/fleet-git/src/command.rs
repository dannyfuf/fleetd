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

use tokio::{io::AsyncWriteExt, process::Command, sync::broadcast};

use crate::{
    command_log::{CommandLog, CommandOutcome, CommandRecord, preview, redact_arg},
    error::{GitError, Result},
    model::CommandKind,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_PREVIEW_LIMIT: usize = 16 * 1024;

/// Complete argv/env/cwd specification for one `git` child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommand {
    /// Ordered arguments after the `git` executable.
    pub argv: Vec<OsString>,
    /// Child working directory.
    pub cwd: PathBuf,
    /// Environment additions or replacements.
    pub env: BTreeMap<OsString, OsString>,
    /// Optional process deadline. `None` uses the runner default.
    pub timeout: Option<Duration>,
    /// Kill the child if its waiting future is dropped.
    pub kill_on_drop: bool,
    /// Optional bytes written to stdin.
    pub stdin: Option<Vec<u8>>,
    /// Read/mutation/network marker.
    pub kind: CommandKind,
    /// Apply literal pathspec semantics.
    pub literal_pathspecs: bool,
    /// Disable optional Git locks for background reads.
    pub background_read: bool,
    /// Extra accepted process exit codes.
    pub accepted_exit_codes: Vec<i32>,
    /// Whether a non-zero exit may mean "stopped with merge conflicts".
    pub may_conflict: bool,
}

impl GitCommand {
    /// Creates an empty command rooted at `cwd`.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>, kind: CommandKind) -> Self {
        Self {
            argv: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
            timeout: None,
            kill_on_drop: true,
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
    pub fn arg(mut self, value: impl Into<OsString>) -> Self {
        self.argv.push(value.into());
        self
    }

    /// Appends ordered arguments.
    #[must_use]
    pub fn args(mut self, values: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.argv.extend(values.into_iter().map(Into::into));
        self
    }

    /// Adds or replaces one environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Overrides the process deadline.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Supplies bytes on stdin.
    #[must_use]
    pub fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }

    /// Marks this command as containing pathspec arguments.
    #[must_use]
    pub fn literal_pathspecs(mut self) -> Self {
        self.literal_pathspecs = true;
        self
    }

    /// Allows Git to take optional locks for a foreground read.
    #[must_use]
    pub fn foreground_read(mut self) -> Self {
        self.background_read = false;
        self
    }

    /// Treats an additional exit code as successful.
    #[must_use]
    pub fn accept_exit_code(mut self, status: i32) -> Self {
        self.accepted_exit_codes.push(status);
        self
    }

    /// Marks the command as one whose failure can mean unresolved conflicts.
    ///
    /// [`Repository`](crate::Repository) only reports [`GitError::Conflict`] for
    /// commands carrying this marker.
    #[must_use]
    pub fn may_conflict(mut self) -> Self {
        self.may_conflict = true;
        self
    }
}

/// Byte-preserving output from an accepted Git invocation.
#[derive(Debug)]
pub struct GitOutput {
    /// Platform exit status.
    pub status: ExitStatus,
    /// Complete standard output.
    pub stdout: Vec<u8>,
    /// Complete standard error.
    pub stderr: Vec<u8>,
    /// Wall-clock elapsed time.
    pub elapsed: Duration,
    /// Final bounded command-log record.
    pub record: CommandRecord,
}

/// Cloneable process runner with live and recent command logs.
#[derive(Debug, Clone)]
pub struct Runner {
    inner: Arc<RunnerInner>,
}

#[derive(Debug)]
struct RunnerInner {
    next_id: AtomicU64,
    log: CommandLog,
    timeout: Duration,
    preview_limit: usize,
    git_program: OsString,
    env: BTreeMap<OsString, OsString>,
}

impl Default for Runner {
    fn default() -> Self {
        Self::new(256, 256)
    }
}

impl Runner {
    /// Creates a runner with bounded recent-record and broadcast capacities.
    #[must_use]
    pub fn new(recent_capacity: usize, event_capacity: usize) -> Self {
        Self {
            inner: Arc::new(RunnerInner {
                next_id: AtomicU64::new(1),
                log: CommandLog::new(recent_capacity, event_capacity),
                timeout: DEFAULT_TIMEOUT,
                preview_limit: DEFAULT_PREVIEW_LIMIT,
                git_program: OsString::from("git"),
                env: BTreeMap::new(),
            }),
        }
    }

    /// Creates a runner using a specific Git executable (primarily for tests).
    #[must_use]
    pub fn with_git_program(program: impl Into<OsString>) -> Self {
        let mut runner = Self::default();
        Arc::get_mut(&mut runner.inner)
            .expect("new runner is uniquely owned")
            .git_program = program.into();
        runner
    }

    /// Adds an environment variable applied to every child from this runner.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("runner builder must be used before cloning")
            .env
            .insert(key.into(), value.into());
        self
    }

    /// Subscribes to started and finished command events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<crate::CommandEvent> {
        self.inner.log.subscribe()
    }

    /// Returns a snapshot of the bounded command history.
    #[must_use]
    pub fn recent(&self) -> Vec<CommandRecord> {
        self.inner.log.recent()
    }

    /// Runs one command without shell interpolation.
    pub async fn run(&self, specification: GitCommand) -> Result<GitOutput> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let display_argv = display_argv(&self.inner.git_program, &specification.argv);
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
        let timeout_duration = specification.timeout.unwrap_or(self.inner.timeout);
        let mut process = Command::new(&self.inner.git_program);
        process
            .args(&specification.argv)
            .current_dir(&specification.cwd)
            .envs(&self.inner.env)
            .envs(&specification.env)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(specification.kill_on_drop);
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

        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(source) => {
                let record = self.finish_spawn_error(
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

        let stdin = specification.stdin;
        let wait = async move {
            if let Some(bytes) = stdin {
                let mut pipe = child.stdin.take().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Git stdin was not piped")
                })?;
                pipe.write_all(&bytes).await?;
                drop(pipe);
            }
            child.wait_with_output().await
        };

        let output = match tokio::time::timeout(timeout_duration, wait).await {
            Ok(Ok(output)) => output,
            Ok(Err(source)) => {
                let record = self.finish_spawn_error(
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
                    timeout: timeout_duration,
                });
            }
        };

        let elapsed = start.elapsed();
        let accepted = output.status.success()
            || output
                .status
                .code()
                .is_some_and(|code| specification.accepted_exit_codes.contains(&code));
        let outcome = if accepted {
            CommandOutcome::Success {
                status: output.status.code(),
                stdout_preview: preview(&output.stdout, self.inner.preview_limit),
                stderr_preview: preview(&output.stderr, self.inner.preview_limit),
            }
        } else {
            CommandOutcome::Failed {
                status: output.status.code(),
                stdout_preview: preview(&output.stdout, self.inner.preview_limit),
                stderr_preview: preview(&output.stderr, self.inner.preview_limit),
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
            let message = concise_output(&output.stderr, &output.stdout);
            return Err(GitError::Exit {
                status: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
                argv: display_argv,
                message,
            });
        }
        Ok(GitOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
            elapsed,
            record,
        })
    }

    fn finish_spawn_error(
        &self,
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

fn display_argv(program: &OsStr, arguments: &[OsString]) -> Vec<String> {
    std::iter::once(program)
        .chain(arguments.iter().map(OsString::as_os_str))
        .map(|argument| redact_arg(&argument.to_string_lossy()))
        .collect()
}

pub(crate) fn concise_output(stderr: &[u8], stdout: &[u8]) -> String {
    let bytes = if stderr.is_empty() { stdout } else { stderr };
    String::from_utf8_lossy(bytes).trim().to_owned()
}
