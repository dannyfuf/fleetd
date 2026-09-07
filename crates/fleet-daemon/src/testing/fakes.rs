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
        Ok(result)
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
