//! Tokio-based, shell-free Git process runner.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::Stdio,
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

    /// Subscribes to started and finished command events.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<crate::CommandEvent> {
        self.inner.log.subscribe()
    }

    /// Returns a snapshot of the bounded command history.
    pub(crate) fn recent(&self) -> Vec<CommandRecord> {
        self.inner.log.recent()
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

        let mut child = match process.spawn() {
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

        let output = match tokio::time::timeout(DEFAULT_TIMEOUT, wait).await {
            Ok(Ok(output)) => output,
            Ok(Err(source)) => {
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
                    timeout: DEFAULT_TIMEOUT,
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
                stdout_preview: preview(&output.stdout, DEFAULT_PREVIEW_LIMIT),
                stderr_preview: preview(&output.stderr, DEFAULT_PREVIEW_LIMIT),
            }
        } else {
            CommandOutcome::Failed {
                status: output.status.code(),
                stdout_preview: preview(&output.stdout, DEFAULT_PREVIEW_LIMIT),
                stderr_preview: preview(&output.stderr, DEFAULT_PREVIEW_LIMIT),
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
            stdout: output.stdout,
            stderr: output.stderr,
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

fn display_argv(program: &OsStr, arguments: &[OsString]) -> Vec<String> {
    std::iter::once(program)
        .chain(arguments.iter().map(OsString::as_os_str))
        .map(|argument| redact_arg(&argument.to_string_lossy()))
        .collect()
}

fn concise_output(stderr: &[u8], stdout: &[u8]) -> String {
    let bytes = if stderr.is_empty() { stdout } else { stderr };
    String::from_utf8_lossy(bytes).trim().to_owned()
}

#[cfg(all(test, unix))]
mod tests {
    use super::{GitCommand, Runner};
    use crate::CommandKind;

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
}
