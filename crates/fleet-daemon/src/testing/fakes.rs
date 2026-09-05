//! Deterministic fake adapter implementations for service-level tests.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fleet_core::{
    github::{InspectionPullRequest, PrTab, PullRequest, RemoteRepo},
    ids::RepoId,
};
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        clock::Clock,
        files::Files,
        git::{Git, ShellGit},
        github::{GhCli, Github},
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
    predicate: Arc<dyn Fn(&ShellCommand) -> bool + Send + Sync>,
    result: ShellResult,
    detached_pid: u32,
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
        self.rules
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ShellRule {
                predicate: Arc::new(predicate),
                result,
                detached_pid: 10_001,
            });
    }

    /// Returns the ordered captured calls.
    #[must_use]
    pub fn calls(&self) -> Vec<FakeShellCall> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn matching(&self, command: &ShellCommand) -> (ShellResult, u32) {
        self.rules
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|rule| (rule.predicate)(command))
            .map(|rule| (rule.result.clone(), rule.detached_pid))
            .unwrap_or_else(|| {
                (
                    ShellResult {
                        status: 127,
                        stdout: String::new(),
                        stderr: format!("unmatched fake command: {}", command.program),
                    },
                    10_001,
                )
            })
    }
}

#[async_trait]
impl Shell for FakeShell {
    async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeShellCall::Run(command.clone()));
        Ok(self.matching(&command).0)
    }

    async fn run_detached(
        &self,
        command: ShellCommand,
        log_path: &Path,
    ) -> DaemonResult<DetachedProcess> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeShellCall::Detached {
                command: command.clone(),
                log_path: log_path.to_path_buf(),
            });
        let (result, pid) = self.matching(&command);
        if result.success() {
            Ok(DetachedProcess { pid })
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
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeShellCall::Streaming(command.clone()));
        if cancel.is_cancelled() {
            return Err(DaemonError::Cancelled);
        }
        let result = self.matching(&command).0;
        for line in result.stdout.lines().chain(result.stderr.lines()) {
            on_line(line.to_owned());
        }
        Ok(result)
    }
}

/// Git fake backed by [`FakeShell`] rules and exact command capture.
#[derive(Clone)]
pub struct FakeGit {
    shell: Arc<FakeShell>,
    inner: ShellGit,
}

impl FakeGit {
    /// Creates a Git fake using `shell`.
    #[must_use]
    pub fn new(shell: Arc<FakeShell>) -> Self {
        Self {
            inner: ShellGit::new(shell.clone()),
            shell,
        }
    }

    /// Returns the underlying rule and call-log fake.
    #[must_use]
    pub fn shell(&self) -> &Arc<FakeShell> {
        &self.shell
    }
}

#[async_trait]
impl Git for FakeGit {
    async fn clone_repo(
        &self,
        url: &str,
        staging: &Path,
        log: &Path,
    ) -> DaemonResult<DetachedProcess> {
        self.inner.clone_repo(url, staging, log).await
    }
    async fn fetch(&self, cwd: &Path, prune: bool) -> DaemonResult<()> {
        self.inner.fetch(cwd, prune).await
    }
    async fn fetch_refs(&self, cwd: &Path, remote: &str, refs: &[String]) -> DaemonResult<()> {
        self.inner.fetch_refs(cwd, remote, refs).await
    }
    async fn origin_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.inner.origin_head(cwd).await
    }
    async fn repair_origin_head(&self, cwd: &Path) -> DaemonResult<()> {
        self.inner.repair_origin_head(cwd).await
    }
    async fn symbolic_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.inner.symbolic_head(cwd).await
    }
    async fn remote_branch_exists(&self, cwd: &Path, branch: &str) -> DaemonResult<bool> {
        self.inner.remote_branch_exists(cwd, branch).await
    }
    async fn remote_branches(&self, cwd: &Path) -> DaemonResult<Vec<String>> {
        self.inner.remote_branches(cwd).await
    }
    async fn checkout_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.inner.checkout_reset(cwd, branch).await
    }
    async fn hard_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.inner.hard_reset(cwd, branch).await
    }
    async fn clean(&self, cwd: &Path) -> DaemonResult<()> {
        self.inner.clean(cwd).await
    }
    async fn checkout_new_branch(&self, cwd: &Path, branch: &str, from: &str) -> DaemonResult<()> {
        self.inner.checkout_new_branch(cwd, branch, from).await
    }
    async fn checkout_force_branch(
        &self,
        cwd: &Path,
        branch: &str,
        from: &str,
    ) -> DaemonResult<()> {
        self.inner.checkout_force_branch(cwd, branch, from).await
    }
    async fn checkout_branch(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.inner.checkout_branch(cwd, branch).await
    }
    async fn fetch_pull_request(&self, cwd: &Path, number: u64) -> DaemonResult<()> {
        self.inner.fetch_pull_request(cwd, number).await
    }
    async fn revision(&self, cwd: &Path, revision: &str) -> DaemonResult<String> {
        self.inner.revision(cwd, revision).await
    }
    async fn revision_exists(&self, cwd: &Path, revision: &str) -> DaemonResult<bool> {
        self.inner.revision_exists(cwd, revision).await
    }
    async fn current_branch(&self, cwd: &Path) -> DaemonResult<String> {
        self.inner.current_branch(cwd).await
    }
    async fn upstream(&self, cwd: &Path, branch: &str) -> DaemonResult<String> {
        self.inner.upstream(cwd, branch).await
    }
    async fn divergence(&self, cwd: &Path, upstream: &str) -> DaemonResult<(u64, u64)> {
        self.inner.divergence(cwd, upstream).await
    }
    async fn unique_commits(&self, cwd: &Path, target: &str) -> DaemonResult<u64> {
        self.inner.unique_commits(cwd, target).await
    }
    async fn unique_commits_from(&self, cwd: &Path, target: &str, head: &str) -> DaemonResult<u64> {
        self.inner.unique_commits_from(cwd, target, head).await
    }
    async fn is_ancestor(
        &self,
        cwd: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> DaemonResult<bool> {
        self.inner.is_ancestor(cwd, ancestor, descendant).await
    }
    async fn status_porcelain(&self, cwd: &Path) -> DaemonResult<String> {
        self.inner.status_porcelain(cwd).await
    }
    async fn update_status(&self, cwd: &Path) -> DaemonResult<String> {
        self.inner.update_status(cwd).await
    }
    async fn is_inside_work_tree(&self, cwd: &Path) -> DaemonResult<bool> {
        self.inner.is_inside_work_tree(cwd).await
    }
    async fn pull_main(&self, cwd: &Path) -> DaemonResult<()> {
        self.inner.pull_main(cwd).await
    }
    async fn short_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.inner.short_head(cwd).await
    }
}

/// GitHub fake backed by [`FakeShell`] rules and exact command capture.
#[derive(Clone)]
pub struct FakeGithub {
    shell: Arc<FakeShell>,
    inner: GhCli,
}

impl FakeGithub {
    /// Creates a GitHub fake using `shell`.
    #[must_use]
    pub fn new(shell: Arc<FakeShell>) -> Self {
        Self {
            inner: GhCli::new(shell.clone()),
            shell,
        }
    }

    /// Returns the underlying rule and call-log fake.
    #[must_use]
    pub fn shell(&self) -> &Arc<FakeShell> {
        &self.shell
    }
}

#[async_trait]
impl Github for FakeGithub {
    async fn viewer_login(&self) -> DaemonResult<String> {
        self.inner.viewer_login().await
    }
    async fn list_repositories(&self, owner: &str) -> DaemonResult<Vec<RemoteRepo>> {
        self.inner.list_repositories(owner).await
    }
    async fn open_pull_request(
        &self,
        repo: &RepoId,
        branch: &str,
    ) -> DaemonResult<Option<PullRequest>> {
        self.inner.open_pull_request(repo, branch).await
    }
    async fn latest_inspection_pull_request(
        &self,
        repo: &RepoId,
        branch: &str,
    ) -> DaemonResult<Option<InspectionPullRequest>> {
        self.inner
            .latest_inspection_pull_request(repo, branch)
            .await
    }
    async fn list_pull_requests(
        &self,
        repo: &RepoId,
        tab: PrTab,
    ) -> DaemonResult<Vec<PullRequest>> {
        self.inner.list_pull_requests(repo, tab).await
    }
    async fn auth_status(&self) -> DaemonResult<()> {
        self.inner.auth_status().await
    }
}

/// An in-memory filesystem call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeFilesCall {
    /// Text was read.
    Read(PathBuf),
    /// Text was atomically written.
    Write(PathBuf, String),
    /// A prefix tree was cloned.
    Clone(PathBuf, PathBuf),
    /// A prefix tree was renamed.
    Rename(PathBuf, PathBuf),
    /// A prefix tree was removed.
    Remove(PathBuf),
}

/// Absolute-path in-memory filesystem with prefix clone, move, and removal semantics.
#[derive(Default)]
pub struct FakeFiles {
    files: Mutex<BTreeMap<PathBuf, String>>,
    directories: Mutex<BTreeSet<PathBuf>>,
    calls: Mutex<Vec<FakeFilesCall>>,
    trash_root: PathBuf,
    removable_roots: Vec<PathBuf>,
}

impl FakeFiles {
    /// Creates an empty fake with explicit trash and deletion roots.
    #[must_use]
    pub fn new(trash_root: PathBuf, removable_roots: Vec<PathBuf>) -> Self {
        Self {
            trash_root,
            removable_roots,
            ..Self::default()
        }
    }

    /// Inserts or replaces one text file.
    pub fn insert_text(&self, path: impl Into<PathBuf>, text: impl Into<String>) {
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.into(), text.into());
    }

    /// Returns the ordered operation log.
    #[must_use]
    pub fn calls(&self) -> Vec<FakeFilesCall> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Returns a cloned text value for assertions.
    #[must_use]
    pub fn text(&self, path: &Path) -> Option<String> {
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path)
            .cloned()
    }
}

impl Files for FakeFiles {
    fn read_text(&self, path: &Path) -> DaemonResult<String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeFilesCall::Read(path.to_path_buf()));
        self.text(path)
            .ok_or_else(|| DaemonError::NotFound(path.display().to_string()))
    }

    fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
        self.directories
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf());
        Ok(())
    }

    fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        if self.exists(destination) {
            return Err(DaemonError::Conflict(format!(
                "{} exists",
                destination.display()
            )));
        }
        let copies = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|(path, text)| {
                path.strip_prefix(source)
                    .ok()
                    .map(|suffix| (destination.join(suffix), text.clone()))
            })
            .collect::<Vec<_>>();
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(copies);
        self.directories
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(destination.to_path_buf());
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeFilesCall::Clone(
                source.to_path_buf(),
                destination.to_path_buf(),
            ));
        Ok(())
    }

    fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf(), text.to_owned());
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeFilesCall::Write(path.to_path_buf(), text.to_owned()));
        Ok(())
    }

    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        let moved = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|(path, text)| {
                path.strip_prefix(source)
                    .ok()
                    .map(|suffix| (path.clone(), destination.join(suffix), text.clone()))
            })
            .collect::<Vec<_>>();
        if moved.is_empty()
            && !self
                .directories
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(source)
        {
            return Err(DaemonError::NotFound(source.display().to_string()));
        }
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (old, new, text) in moved {
            files.remove(&old);
            files.insert(new, text);
        }
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeFilesCall::Rename(
                source.to_path_buf(),
                destination.to_path_buf(),
            ));
        Ok(())
    }

    fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("item");
        let destination = self.trash_root.join(format!("0-{name}"));
        self.rename(path, &destination)?;
        Ok(destination)
    }

    fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
        self.guard_strict_descendant(path)?;
        let keys = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .filter(|candidate| candidate.starts_with(path))
            .cloned()
            .collect::<Vec<_>>();
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for key in keys {
            files.remove(&key);
        }
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(FakeFilesCall::Remove(path.to_path_buf()));
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> DaemonResult<()> {
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(path);
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .any(|candidate| candidate == path || candidate.starts_with(path))
            || self
                .directories
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(path)
    }

    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
        let mut values = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .chain(
                self.directories
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter(),
            )
            .filter(|candidate| candidate.parent() == Some(path))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        values.sort();
        Ok(values)
    }

    fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
        if self
            .removable_roots
            .iter()
            .chain(std::iter::once(&self.trash_root))
            .any(|root| path != root && path.starts_with(root))
        {
            Ok(())
        } else {
            Err(DaemonError::Validation(format!(
                "unsafe removal: {}",
                path.display()
            )))
        }
    }
}

/// Mutable deterministic process fake.
#[derive(Default)]
pub struct FakeProcess {
    snapshot: Mutex<Vec<ProcessInfo>>,
    ports: Mutex<Vec<ListeningPort>>,
    alive: Mutex<BTreeSet<u32>>,
}

impl FakeProcess {
    /// Replaces the process table and derives liveness from it.
    pub fn set_snapshot(&self, snapshot: Vec<ProcessInfo>) {
        *self
            .alive
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            snapshot.iter().map(|process| process.pid).collect();
        *self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
    }

    /// Replaces listening-port observations.
    pub fn set_ports(&self, ports: Vec<ListeningPort>) {
        *self
            .ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ports;
    }
}

#[async_trait]
impl Process for FakeProcess {
    async fn snapshot(&self) -> DaemonResult<Vec<ProcessInfo>> {
        Ok(self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
    async fn descendants(&self, pid: u32) -> DaemonResult<Vec<ProcessInfo>> {
        let snapshot = self.snapshot().await?;
        let mut parents = BTreeSet::from([pid]);
        let mut result = Vec::new();
        loop {
            let found = snapshot
                .iter()
                .filter(|process| {
                    parents.contains(&process.parent_pid) && !parents.contains(&process.pid)
                })
                .cloned()
                .collect::<Vec<_>>();
            if found.is_empty() {
                break;
            }
            for process in found {
                parents.insert(process.pid);
                result.push(process);
            }
        }
        Ok(result)
    }
    async fn listening_ports(&self, pids: &[u32]) -> DaemonResult<Vec<ListeningPort>> {
        Ok(self
            .ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|port| pids.contains(&port.pid))
            .copied()
            .collect())
    }
    fn is_alive(&self, pid: u32) -> bool {
        self.alive
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&pid)
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
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = now;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
