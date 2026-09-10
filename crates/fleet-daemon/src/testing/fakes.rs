//! Deterministic fake adapter implementations for service-level tests.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use super::lock;
use crate::{
    DaemonError, DaemonResult,
    adapters::{
        clock::Clock,
        git::ShellGit,
        github::GhCli,
        process::{ListeningPort, Process, ProcessInfo},
        shell::{DetachedProcess, LineCallback, Shell, ShellCommand, ShellResult},
    },
};

/// A captured fake shell invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeShellCall {
    /// A normal captured command.
    Run(ShellCommand),
    /// A detached command and its log destination.
    Detached {
        /// Captured command.
        command: ShellCommand,
        /// Captured log path.
        log_path: PathBuf,
    },
    /// A line-streaming command.
    Streaming(ShellCommand),
}

struct ShellRule {
    predicate: Box<dyn Fn(&ShellCommand) -> bool + Send + Sync>,
    result: ShellResult,
}

/// Rule-driven shell fake with an exact ordered call log and unmatched status 127.
#[derive(Default)]
pub struct FakeShell {
    rules: Mutex<Vec<ShellRule>>,
    calls: Mutex<Vec<FakeShellCall>>,
}

impl FakeShell {
    /// Creates an empty fake whose unmatched commands return status 127.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a predicate rule and reusable result.
    pub fn when<F>(&self, predicate: F, result: ShellResult)
    where
        F: Fn(&ShellCommand) -> bool + Send + Sync + 'static,
    {
        lock(&self.rules).push(ShellRule {
            predicate: Box::new(predicate),
            result,
        });
    }

    /// Returns the ordered captured calls.
    #[must_use]
    pub fn calls(&self) -> Vec<FakeShellCall> {
        lock(&self.calls).clone()
    }

    fn matching(&self, command: &ShellCommand) -> ShellResult {
        lock(&self.rules)
            .iter()
            .find(|rule| (rule.predicate)(command))
            .map(|rule| rule.result.clone())
            .unwrap_or_else(|| ShellResult {
                status: 127,
                stdout: String::new(),
                stderr: format!("unmatched fake command: {}", command.program),
            })
    }
}

#[async_trait]
impl Shell for FakeShell {
    async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult> {
        let result = self.matching(&command);
        lock(&self.calls).push(FakeShellCall::Run(command));
        Ok(result)
    }

    async fn run_detached(
        &self,
        command: ShellCommand,
        log_path: &Path,
    ) -> DaemonResult<DetachedProcess> {
        let result = self.matching(&command);
        lock(&self.calls).push(FakeShellCall::Detached {
            command,
            log_path: log_path.to_path_buf(),
        });
        if result.success() {
            Ok(DetachedProcess { pid: 10_001 })
        } else {
            Err(DaemonError::Shell(result.stderr))
        }
    }

    async fn run_streaming(
        &self,
        command: ShellCommand,
        cancel: CancellationToken,
        on_line: LineCallback,
    ) -> DaemonResult<ShellResult> {
        let result = self.matching(&command);
        lock(&self.calls).push(FakeShellCall::Streaming(command));
        if cancel.is_cancelled() {
            return Err(DaemonError::Cancelled);
        }
        for line in result.stdout.lines().chain(result.stderr.lines()) {
            on_line(line.to_owned());
        }
        // `RealShell` streams every line through `on_line` and keeps the captured buffers
        // empty; a fake that returned them would let tests assert output production never has.
        Ok(ShellResult {
            status: result.status,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// The production Git adapter over a deterministic shell.
pub type FakeGit = ShellGit<FakeShell>;

/// The production GitHub adapter over a deterministic shell.
pub type FakeGithub = GhCli<FakeShell>;

pub use super::files::{FakeFiles, FakeFilesCall};

/// Mutable deterministic process fake.
#[derive(Default)]
pub struct FakeProcess {
    snapshot: Mutex<Vec<ProcessInfo>>,
    ports: Mutex<Vec<ListeningPort>>,
    alive: Mutex<BTreeSet<u32>>,
    environments: Mutex<BTreeMap<u32, Vec<(String, String)>>>,
    environment_calls: Mutex<Vec<u32>>,
}

impl FakeProcess {
    /// Replaces the process table and derives liveness from it.
    pub fn set_snapshot(&self, snapshot: Vec<ProcessInfo>) {
        *lock(&self.alive) = snapshot.iter().map(|process| process.pid).collect();
        *lock(&self.snapshot) = snapshot;
    }

    /// Replaces listening-port observations.
    pub fn set_ports(&self, ports: Vec<ListeningPort>) {
        *lock(&self.ports) = ports;
    }

    /// Replaces one process's environment.
    pub fn set_environment(&self, pid: u32, environment: Vec<(String, String)>) {
        lock(&self.environments).insert(pid, environment);
    }

    /// Explicitly changes process liveness without changing the snapshot.
    pub fn set_alive(&self, pid: u32, alive: bool) {
        let mut pids = lock(&self.alive);
        if alive {
            pids.insert(pid);
        } else {
            pids.remove(&pid);
        }
    }

    /// Returns environment-read calls in order.
    #[must_use]
    pub fn environment_calls(&self) -> Vec<u32> {
        lock(&self.environment_calls).clone()
    }
}

#[async_trait]
impl Process for FakeProcess {
    async fn snapshot(&self) -> DaemonResult<Vec<ProcessInfo>> {
        Ok(lock(&self.snapshot).clone())
    }
    async fn listening_ports(&self, pids: &[u32]) -> DaemonResult<Vec<ListeningPort>> {
        Ok(lock(&self.ports)
            .iter()
            .filter(|port| pids.contains(&port.pid))
            .copied()
            .collect())
    }
    async fn environment(&self, pid: u32) -> DaemonResult<Vec<(String, String)>> {
        lock(&self.environment_calls).push(pid);
        Ok(lock(&self.environments)
            .get(&pid)
            .cloned()
            .unwrap_or_default())
    }
    fn is_alive(&self, pid: u32) -> bool {
        lock(&self.alive).contains(&pid)
    }
}

/// Thread-safe deterministic wall clock.
pub struct FixedClock {
    now: Mutex<DateTime<Utc>>,
}

impl FixedClock {
    /// Creates a clock fixed at `now`.
    #[must_use]
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(now),
        }
    }
    /// Replaces the current instant.
    pub fn set(&self, now: DateTime<Utc>) {
        *lock(&self.now) = now;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *lock(&self.now)
    }
}

/// Captured invocation of a scripted board backend.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeBackendCall {
    /// Validated backend settings.
    Validate(serde_json::Value),
    /// Requested schema for a board.
    Describe(fleet_core::board::Board),
    /// Pulled a board with the supplied cursor.
    Pull(fleet_core::board::Board, Option<String>),
    /// Pushed operations and their current local cards.
    Push(
        fleet_core::board::Board,
        Vec<fleet_core::board::Card>,
        Vec<fleet_core::board::PushOp>,
    ),
}

/// FIFO-scripted backend; exhausted queues return empty successful results.
pub struct FakeBackend {
    /// Registry key used by test boards.
    pub kind: &'static str,
    /// Advertised remote operations.
    pub capabilities: fleet_core::board::BackendCapabilities,
    /// Scripted validation results, consumed in order.
    pub validate_responses:
        Mutex<std::collections::VecDeque<Result<(), fleet_core::board::BoardError>>>,
    /// Scripted schema descriptions, consumed in order.
    pub describe_responses: Mutex<
        std::collections::VecDeque<
            Result<fleet_core::board::BackendSchema, fleet_core::board::BoardError>,
        >,
    >,
    /// Scripted pull results, consumed in order.
    pub pull_responses: Mutex<
        std::collections::VecDeque<
            Result<fleet_core::board::PullResult, fleet_core::board::BoardError>,
        >,
    >,
    /// Scripted push results, consumed in order.
    pub push_responses: Mutex<
        std::collections::VecDeque<
            Result<fleet_core::board::PushResult, fleet_core::board::BoardError>,
        >,
    >,
    /// Ordered calls including all supplied arguments.
    pub calls: Mutex<Vec<FakeBackendCall>>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            kind: "fake",
            capabilities: fleet_core::board::BackendCapabilities::default(),
            validate_responses: Mutex::default(),
            describe_responses: Mutex::default(),
            pull_responses: Mutex::default(),
            push_responses: Mutex::default(),
            calls: Mutex::default(),
        }
    }
}

impl FakeBackend {
    /// Creates a named scripted backend with explicit capabilities.
    pub fn new(kind: &'static str, capabilities: fleet_core::board::BackendCapabilities) -> Self {
        Self {
            kind,
            capabilities,
            ..Self::default()
        }
    }
    /// Copies the complete ordered invocation log.
    pub fn calls(&self) -> Vec<FakeBackendCall> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn record(&self, call: FakeBackendCall) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }
}

#[async_trait]
impl crate::adapters::board::BoardBackend for FakeBackend {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn label(&self) -> &'static str {
        "Fake"
    }
    fn capabilities(&self) -> fleet_core::board::BackendCapabilities {
        self.capabilities
    }
    fn settings_schema(&self) -> Vec<fleet_core::board::PropertySchema> {
        // One required row, as every remote backend has: it is what marks the setting that
        // names the remote, and the service refuses to change that one under linked cards.
        ["project", "filter"]
            .into_iter()
            .map(|key| fleet_core::board::PropertySchema {
                key: key.into(),
                name: if key == "project" {
                    format!("Project {}", fleet_core::board::REQUIRED_MARKER)
                } else {
                    "Filter".into()
                },
                kind: fleet_core::board::PropertyKind::Text,
                options: Vec::new(),
                editable: true,
                source: fleet_core::board::PropertySource::Backend,
                show_on_card: false,
            })
            .collect()
    }
    async fn validate(
        &self,
        settings: &serde_json::Value,
    ) -> Result<(), fleet_core::board::BoardError> {
        self.record(FakeBackendCall::Validate(settings.clone()));
        self.validate_responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Ok(()))
    }
    async fn describe(
        &self,
        board: &fleet_core::board::Board,
    ) -> Result<fleet_core::board::BackendSchema, fleet_core::board::BoardError> {
        self.record(FakeBackendCall::Describe(board.clone()));
        self.describe_responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Ok(Default::default()))
    }
    async fn pull(
        &self,
        board: &fleet_core::board::Board,
        cursor: Option<&str>,
    ) -> Result<fleet_core::board::PullResult, fleet_core::board::BoardError> {
        self.record(FakeBackendCall::Pull(
            board.clone(),
            cursor.map(str::to_owned),
        ));
        self.pull_responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Ok(Default::default()))
    }
    async fn push(
        &self,
        board: &fleet_core::board::Board,
        cards: &[fleet_core::board::Card],
        ops: &[fleet_core::board::PushOp],
    ) -> Result<fleet_core::board::PushResult, fleet_core::board::BoardError> {
        self.record(FakeBackendCall::Push(
            board.clone(),
            cards.to_vec(),
            ops.to_vec(),
        ));
        self.push_responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Ok(Default::default()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::shell::RealShell;

    async fn stream(shell: &dyn Shell, command: ShellCommand) -> (ShellResult, Vec<String>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let result = shell
            .run_streaming(
                command,
                CancellationToken::new(),
                Arc::new(move |line| lock(&sink).push(line)),
            )
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let mut lines = lock(&lines).clone();
        // The real adapter drains stdout and stderr concurrently, so only the set is defined.
        lines.sort();
        (result, lines)
    }

    #[tokio::test]
    async fn fake_and_real_shells_agree_on_streaming_results() {
        let command = ShellCommand::new("sh").args(["-c", "echo out; echo err 1>&2; exit 3"]);
        let fake = FakeShell::new();
        fake.when(
            |command| command.program == "sh",
            ShellResult {
                status: 3,
                stdout: "out\n".to_owned(),
                stderr: "err\n".to_owned(),
            },
        );

        let (faked, faked_lines) = stream(&fake, command.clone()).await;
        let (real, real_lines) = stream(&RealShell, command).await;

        assert_eq!(faked_lines, vec!["err".to_owned(), "out".to_owned()]);
        assert_eq!(faked_lines, real_lines);
        assert_eq!(faked, real);
    }
}
