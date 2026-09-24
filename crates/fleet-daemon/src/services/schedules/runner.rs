//! The headless runner: launches Claude or Codex non-interactively in a schedule's work
//! directory, streams its output to the run's log, and maps the exit to an outcome
//! (`docs/BOARD.md` §12, "How a schedule runs").

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use fleet_core::{
    agents::{AgentKind, PermissionMode},
    paths::FleetHome,
    schedule::{
        SCHEDULE_SUMMARY_MAX_CHARS, Schedule, ScheduleAgent, ScheduleOutcome, summary_line,
    },
};
use tokio::{io::AsyncWriteExt, sync::mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    adapters::shell::{LineCallback, Shell, ShellCommand, ShellResult},
    agents::{
        claude::argv::permission_mode_to_wire,
        harness::{probe::strip_list, process},
    },
    stores::config::ConfigStore,
};

#[cfg(test)]
mod tests;

/// The summary a run cancelled through its job carries.
const CANCELED_SUMMARY: &str = "canceled";

/// What one headless run produced.
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    /// How the run ended.
    pub outcome: ScheduleOutcome,
    /// The agent's `SUMMARY:` line, a failure detail, or a timeout-duration fallback.
    pub summary: Option<String>,
    /// Reported cost, when the provider reports one.
    pub cost_usd: Option<f64>,
}

impl RunResult {
    fn failed(summary: impl Into<String>) -> Self {
        Self {
            outcome: ScheduleOutcome::Failed,
            summary: Some(cut(summary.into().trim())),
            cost_usd: None,
        }
    }
}

/// Runs one fire of a schedule; the seam the firing loop's tests fake.
#[async_trait]
pub trait ScheduleRunner: Send + Sync {
    /// Runs `prompt` for `schedule` until it exits, times out, or `cancel` fires.
    async fn run(&self, schedule: &Schedule, prompt: &str, cancel: CancellationToken) -> RunResult;

    /// The `fleet` binary a run's `PATH` starts with, which the prompt footer's `{fleet}` names.
    ///
    /// `None` when there is none; the footer then says `fleet` and the child relies on its
    /// login shell's `PATH`.
    fn fleet_program(&self) -> Option<&Path> {
        None
    }
}

/// Where the child's base environment comes from.
#[derive(Clone)]
enum EnvironmentSource {
    /// The user's login shell in the work directory, as native agent threads get.
    Login,
    /// A fixed map, so tests never spawn a login shell.
    #[cfg(test)]
    Fixed(HashMap<OsString, OsString>),
}

/// The production runner, launching provider CLIs through the `Shell` adapter.
#[derive(Clone)]
pub struct HeadlessRunner {
    shell: Arc<dyn Shell>,
    config: Arc<ConfigStore>,
    home: FleetHome,
    /// The `fleet` binary next to this `fleetd`, when there is one.
    fleet_program: Option<PathBuf>,
    environment: EnvironmentSource,
}

impl HeadlessRunner {
    /// Creates a runner over the shell adapter, the daemon config and the Fleet home.
    #[must_use]
    pub fn new(shell: Arc<dyn Shell>, config: Arc<ConfigStore>, home: FleetHome) -> Self {
        let daemon_exe = std::env::current_exe()
            .inspect_err(|error| {
                tracing::warn!(%error, "could not locate fleetd; schedules use `fleet` from PATH");
            })
            .ok();
        Self {
            shell,
            config,
            home,
            fleet_program: sibling_fleet(daemon_exe.as_deref()),
            environment: EnvironmentSource::Login,
        }
    }

    async fn base_environment(&self, cwd: &Path) -> HashMap<OsString, OsString> {
        match &self.environment {
            EnvironmentSource::Login => process::login_environment(cwd).await,
            #[cfg(test)]
            EnvironmentSource::Fixed(environment) => environment.clone(),
        }
    }

    /// Builds the child invocation; everything but the process itself.
    async fn command(
        &self,
        schedule: &Schedule,
        prompt: &str,
        last_message: &Path,
    ) -> DaemonResult<ShellCommand> {
        let work_dir = self.home.schedule_work_dir(&schedule.id);
        tokio::fs::create_dir_all(&work_dir)
            .await
            .map_err(|error| DaemonError::fs(&work_dir, error))?;
        let config = self.config.load().await?;
        let provider = schedule.agent.provider;
        let (program, base_args) = process::command_parts(config.agent_binaries.binary(provider))
            .map_err(|error| {
            DaemonError::Shell(format!(
                "the configured {} executable is invalid: {error}",
                provider.display_name()
            ))
        })?;

        let overrides = BTreeMap::from([
            ("FLEET_BOARD".to_owned(), schedule.board_id.to_string()),
            (
                "FLEET_HOME".to_owned(),
                self.home.root().to_string_lossy().into_owned(),
            ),
            ("FLEET_SCHEDULE".to_owned(), schedule.id.to_string()),
        ]);
        let mut environment = process::filter_environment(
            self.base_environment(&work_dir).await,
            strip_list(provider),
            &overrides,
        );
        if let Some(directory) = self.fleet_program.as_deref().and_then(Path::parent) {
            process::prepend_path(&mut environment, directory);
        }
        let program = process::resolve_program(program, &environment);

        let args = match provider {
            AgentKind::Claude => claude_argv(&schedule.agent, prompt),
            AgentKind::Codex => codex_argv(&schedule.agent, prompt, last_message),
        };
        let mut command = ShellCommand::new(program.to_string_lossy())
            .args(base_args.iter().map(|argument| argument.to_string_lossy()))
            .args(args)
            .cwd(work_dir)
            // `environment` is the whole filtered login environment, so a variable the strip
            // list removes must not come back from the daemon's own environment.
            .clear_env()
            // B6: the agent's MCP servers and tool shells end with it.
            .kill_group_on_exit()
            .timeout(Duration::from_secs(
                u64::from(schedule.timeout_minutes).saturating_mul(60),
            ));
        for (key, value) in environment {
            // A variable that is not UTF-8 cannot cross `ShellCommand`, and the child starts
            // without it.
            if let (Ok(key), Ok(value)) = (key.into_string(), value.into_string()) {
                command = command.env(key, value);
            }
        }
        Ok(command)
    }
}

#[async_trait]
impl ScheduleRunner for HeadlessRunner {
    async fn run(&self, schedule: &Schedule, prompt: &str, cancel: CancellationToken) -> RunResult {
        let log_path = run_log_path(&self.home, schedule);
        let last_message = log_path.with_extension("last-message");
        let command = match self.command(schedule, prompt, &last_message).await {
            Ok(command) => command,
            Err(error) => {
                tracing::warn!(schedule = %schedule.id, %error, "could not prepare a scheduled run");
                return RunResult::failed(error.to_string());
            }
        };
        if cancel.is_cancelled() {
            return RunResult::failed(CANCELED_SUMMARY);
        }
        let log = match RunLog::open(&log_path).await {
            Ok(log) => log,
            Err(error) => {
                tracing::warn!(schedule = %schedule.id, %error, "could not open a scheduled run's log");
                return RunResult::failed(error.to_string());
            }
        };
        if schedule.agent.provider == AgentKind::Codex {
            // A reply left by an earlier run must not pass for this one's.
            match tokio::fs::remove_file(&last_message).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(path = %last_message.display(), %error, "could not clear a stale Codex reply");
                }
            }
        }
        let stream = Arc::new(Mutex::new(StreamState::default()));
        let on_line = log.callback(Arc::clone(&stream));
        let exit = self.shell.run_streaming(command, cancel, on_line).await;
        log.finish(&schedule.id).await;

        let stream = std::mem::take(&mut *lock(&stream));
        let final_message = match schedule.agent.provider {
            AgentKind::Claude => stream.claude_final_message(),
            AgentKind::Codex => read_last_message(&last_message).await,
        };
        let result = interpret(
            schedule.agent.provider,
            schedule.timeout_minutes,
            exit,
            &stream,
            final_message.as_deref(),
        );
        tracing::info!(
            schedule = %schedule.id,
            outcome = ?result.outcome,
            log = %log_path.display(),
            "scheduled run finished"
        );
        result
    }

    fn fleet_program(&self) -> Option<&Path> {
        self.fleet_program.as_deref()
    }
}

/// The `claude -p` argv, program excluded.
///
/// `--permission-mode` reuses the interactive adapter's table, so a schedule and a thread in the
/// same mode get the same value. The prompt is last and one argument, after `--`: the shell
/// adapter passes argv without a shell, and the `--` keeps a prompt that opens with `-` (a
/// Markdown list) from being read as an option, so the prompt may contain anything.
#[must_use]
pub fn claude_argv(agent: &ScheduleAgent, prompt: &str) -> Vec<String> {
    let mut argv: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        permission_mode_to_wire(agent.mode),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if let Some(model) = &agent.model {
        argv.extend(["--model".to_owned(), model.clone()]);
    }
    if let Some(effort) = &agent.effort {
        argv.extend(["--effort".to_owned(), effort.clone()]);
    }
    argv.extend(["--".to_owned(), prompt.to_owned()]);
    argv
}

/// The `codex exec` argv, program excluded; the final reply is written to `last_message`.
///
/// `codex exec` has no channel to ask anyone anything, so `Ask` cannot prompt: it and `Plan`
/// run in the `read-only` sandbox, where anything needing approval is refused. `FullAccess`
/// bypasses approvals and the sandbox; `AcceptEdits` (and `Auto`) may write in the work
/// directory; `DontAsk` refuses like `Ask`.
#[must_use]
pub fn codex_argv(agent: &ScheduleAgent, prompt: &str, last_message: &Path) -> Vec<String> {
    let mut argv: Vec<String> = ["exec", "--json", "--skip-git-repo-check"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    match agent.mode {
        PermissionMode::FullAccess => {
            argv.push("--dangerously-bypass-approvals-and-sandbox".to_owned());
        }
        PermissionMode::AcceptEdits | PermissionMode::Auto => {
            argv.extend(["-s".to_owned(), "workspace-write".to_owned()]);
        }
        PermissionMode::Ask | PermissionMode::Plan | PermissionMode::DontAsk => {
            argv.extend(["-s".to_owned(), "read-only".to_owned()]);
        }
    }
    if let Some(model) = &agent.model {
        argv.extend(["-m".to_owned(), model.clone()]);
    }
    if let Some(effort) = &agent.effort {
        argv.extend([
            "-c".to_owned(),
            format!("model_reasoning_effort=\"{effort}\""),
        ]);
    }
    // `--` for the reason `claude_argv` gives: a prompt opening with `-` is still the prompt.
    argv.extend([
        "-o".to_owned(),
        last_message.to_string_lossy().into_owned(),
        "--".to_owned(),
        prompt.to_owned(),
    ]);
    argv
}

/// The log of the run being executed: the one the firing loop recorded last, else a fresh one.
fn run_log_path(home: &FleetHome, schedule: &Schedule) -> PathBuf {
    match schedule.runs.last() {
        Some(run) => run.log_path.as_ref().map_or_else(
            || home.schedule_log_path(&schedule.id, &run.started_at),
            PathBuf::from,
        ),
        None => home.schedule_log_path(&schedule.id, &chrono::Utc::now().to_rfc3339()),
    }
}

/// A `fleet` sitting next to this daemon's own `fleetd`.
fn sibling_fleet(daemon_exe: Option<&Path>) -> Option<PathBuf> {
    let sibling = daemon_exe?.parent()?.join("fleet");
    sibling.is_file().then_some(sibling)
}

/// What the runner keeps from the output stream.
#[derive(Debug, Default)]
struct StreamState {
    /// The last Claude `{"type":"result"}` object.
    result: Option<serde_json::Value>,
    /// The last non-empty line that is not a JSON object.
    last_line: Option<String>,
}

impl StreamState {
    fn observe(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) if value.is_object() => {
                if value.get("type").and_then(serde_json::Value::as_str) == Some("result") {
                    self.result = Some(value);
                }
            }
            _ => self.last_line = Some(trimmed.to_owned()),
        }
    }

    fn claude_final_message(&self) -> Option<String> {
        self.result
            .as_ref()?
            .get("result")?
            .as_str()
            .map(str::to_owned)
    }

    fn claude_is_error(&self) -> bool {
        self.result
            .as_ref()
            .and_then(|result| result.get("is_error"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    fn claude_cost(&self) -> Option<f64> {
        self.result.as_ref()?.get("total_cost_usd")?.as_f64()
    }
}

/// Maps the child's exit and what it printed onto a [`RunResult`].
fn interpret(
    provider: AgentKind,
    timeout_minutes: u32,
    exit: DaemonResult<ShellResult>,
    stream: &StreamState,
    final_message: Option<&str>,
) -> RunResult {
    let cost_usd = match provider {
        AgentKind::Claude => stream.claude_cost(),
        // `codex exec --json` reports tokens, never a price.
        AgentKind::Codex => None,
    };
    let summary = final_message.and_then(summary_line);
    let outcome = match &exit {
        Ok(result) if result.success() => {
            let finished = match provider {
                AgentKind::Claude => stream.result.is_some() && !stream.claude_is_error(),
                AgentKind::Codex => true,
            };
            if finished {
                ScheduleOutcome::Succeeded
            } else {
                ScheduleOutcome::Failed
            }
        }
        Ok(_) => ScheduleOutcome::Failed,
        Err(DaemonError::Timeout(_)) => ScheduleOutcome::TimedOut,
        Err(DaemonError::Cancelled) => {
            return RunResult {
                outcome: ScheduleOutcome::Failed,
                summary: Some(CANCELED_SUMMARY.to_owned()),
                cost_usd,
            };
        }
        Err(_) => ScheduleOutcome::Failed,
    };
    let summary = if summary.is_some() || outcome == ScheduleOutcome::Succeeded {
        summary
    } else {
        final_message
            .and_then(last_non_empty_line)
            .or_else(|| stream.last_line.clone())
            .or_else(|| match &exit {
                Err(DaemonError::Timeout(_)) => {
                    Some(format!("stopped after {timeout_minutes} min"))
                }
                Err(error) => Some(error.to_string()),
                Ok(result) => Some(format!("exited with status {}", result.status)),
            })
            .map(|line| cut(&line))
    };
    RunResult {
        outcome,
        summary,
        cost_usd,
    }
}

fn last_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

fn cut(text: &str) -> String {
    text.chars().take(SCHEDULE_SUMMARY_MAX_CHARS).collect()
}

/// Reads and removes Codex's `-o` file; a missing file means Codex wrote no final reply.
///
/// Read lossily, like the agent's streamed output: one invalid byte must not cost the run its
/// `SUMMARY:` line.
async fn read_last_message(path: &Path) -> Option<String> {
    let text = match tokio::fs::read(path).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "could not read Codex's final reply");
            return None;
        }
    };
    if let Err(error) = tokio::fs::remove_file(path).await {
        tracing::warn!(path = %path.display(), %error, "could not remove Codex's final reply");
    }
    Some(text)
}

/// The run's log file, written by a task fed from the synchronous line callback.
struct RunLog {
    /// Taken by `finish`, so a callback the shell kept alive cannot hold the writer open.
    sender: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
    writer: tokio::task::JoinHandle<()>,
}

impl RunLog {
    async fn open(path: &Path) -> DaemonResult<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| DaemonError::fs(parent, error))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|error| DaemonError::fs(path, error))?;
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let log_name = path.display().to_string();
        let writer = tokio::spawn(async move {
            let mut failed = false;
            while let Some(mut line) = receiver.recv().await {
                if failed {
                    continue;
                }
                line.push('\n');
                if let Err(error) = file.write_all(line.as_bytes()).await {
                    tracing::warn!(path = %log_name, %error, "could not append to a scheduled run's log");
                    failed = true;
                }
            }
            if let Err(error) = file.flush().await {
                tracing::warn!(path = %log_name, %error, "could not flush a scheduled run's log");
            }
        });
        Ok(Self {
            sender: Arc::new(Mutex::new(Some(sender))),
            writer,
        })
    }

    /// The line callback: remembers what the result mapping needs and queues the line.
    fn callback(&self, stream: Arc<Mutex<StreamState>>) -> LineCallback {
        let sender = Arc::clone(&self.sender);
        Arc::new(move |line: String| {
            lock(&stream).observe(&line);
            let queued = lock(&sender)
                .as_ref()
                .is_some_and(|sender| sender.send(line).is_ok());
            if !queued {
                tracing::debug!("a scheduled run printed a line after its log closed");
            }
        })
    }

    /// Closes the queue and waits for every queued line to reach the file.
    async fn finish(self, schedule: &fleet_core::ids::ScheduleId) {
        drop(lock(&self.sender).take());
        if let Err(error) = self.writer.await {
            tracing::warn!(%schedule, %error, "a scheduled run's log writer failed");
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
