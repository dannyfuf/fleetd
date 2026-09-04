//! Process discovery, liveness, and port inspection.

use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;

use crate::{
    DaemonError, DaemonResult,
    adapters::shell::{Shell, ShellCommand},
};

/// One row from the system process table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    /// Process identifier.
    pub pid: u32,
    /// Parent process identifier.
    pub parent_pid: u32,
    /// Full command line.
    pub command: String,
}

/// A TCP listening socket owned by a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ListeningPort {
    /// Owning process identifier.
    pub pid: u32,
    /// TCP port number.
    pub port: u16,
}

/// Process and listening-port observation boundary.
#[async_trait]
pub trait Process: Send + Sync {
    /// Captures `ps -axo pid=,ppid=,command=`.
    async fn snapshot(&self) -> DaemonResult<Vec<ProcessInfo>>;
    /// Returns all recursive descendants of `pid`.
    async fn descendants(&self, pid: u32) -> DaemonResult<Vec<ProcessInfo>>;
    /// Returns TCP listeners owned by the supplied process identifiers.
    async fn listening_ports(&self, pids: &[u32]) -> DaemonResult<Vec<ListeningPort>>;
    /// Returns whether a process currently exists or is permission-protected.
    fn is_alive(&self, pid: u32) -> bool;
}

/// Real process adapter implemented with swarm's exact `ps` and `lsof` commands.
#[derive(Clone)]
pub struct RealProcess {
    shell: Arc<dyn Shell>,
}

impl RealProcess {
    /// Creates a process adapter backed by `shell`.
    #[must_use]
    pub fn new(shell: Arc<dyn Shell>) -> Self {
        Self { shell }
    }
}

#[async_trait]
impl Process for RealProcess {
    async fn snapshot(&self) -> DaemonResult<Vec<ProcessInfo>> {
        let result = self
            .shell
            .run(ShellCommand::new("ps").args(["-axo", "pid=,ppid=,command="]))
            .await?;
        if !result.success() {
            return Err(DaemonError::Process(format!(
                "ps exited {}: {}",
                result.status,
                result.stderr.trim()
            )));
        }
        result
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(parse_process)
            .collect()
    }

    async fn descendants(&self, pid: u32) -> DaemonResult<Vec<ProcessInfo>> {
        let snapshot = self.snapshot().await?;
        let mut parents = BTreeSet::from([pid]);
        let mut descendants = Vec::new();
        loop {
            let mut changed = false;
            for process in &snapshot {
                if parents.contains(&process.parent_pid) && !parents.contains(&process.pid) {
                    parents.insert(process.pid);
                    descendants.push(process.clone());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Ok(descendants)
    }

    async fn listening_ports(&self, pids: &[u32]) -> DaemonResult<Vec<ListeningPort>> {
        if pids.is_empty() {
            return Ok(Vec::new());
        }
        let joined = pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let result = self
            .shell
            .run(ShellCommand::new("lsof").args([
                "-nP",
                "-iTCP",
                "-sTCP:LISTEN",
                "-a",
                "-p",
                &joined,
                "-F",
                "pn",
            ]))
            .await?;
        if !result.success() && result.status != 1 {
            return Err(DaemonError::Process(format!(
                "lsof exited {}: {}",
                result.status,
                result.stderr.trim()
            )));
        }
        Ok(parse_lsof(&result.stdout))
    }

    fn is_alive(&self, pid: u32) -> bool {
        if pid == 0 || pid > i32::MAX as u32 {
            return false;
        }
        // SAFETY: signal zero performs no mutation and accepts any integer pid.
        let result = unsafe { libc::kill(pid.cast_signed(), 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

fn parse_process(line: &str) -> DaemonResult<ProcessInfo> {
    let line = line.trim_start();
    let pid_end = line.find(char::is_whitespace).unwrap_or(line.len());
    let (pid, remaining) = line.split_at(pid_end);
    let remaining = remaining.trim_start();
    let parent_end = remaining
        .find(char::is_whitespace)
        .unwrap_or(remaining.len());
    let (parent_pid, command) = remaining.split_at(parent_end);
    let pid = pid
        .parse()
        .ok()
        .ok_or_else(|| DaemonError::Process(format!("invalid ps row: {line}")))?;
    let parent_pid = parent_pid
        .parse()
        .ok()
        .ok_or_else(|| DaemonError::Process(format!("invalid ps row: {line}")))?;
    let command = command.trim_start().to_owned();
    Ok(ProcessInfo {
        pid,
        parent_pid,
        command,
    })
}

fn parse_lsof(output: &str) -> Vec<ListeningPort> {
    let mut pid = None;
    let mut ports = BTreeSet::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let Some(name) = line.strip_prefix('n')
            && let (Some(pid), Some(port)) = (
                pid,
                name.rsplit(':').next().and_then(|value| value.parse().ok()),
            )
        {
            ports.insert(ListeningPort { pid, port });
        }
    }
    ports.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use crate::{adapters::shell::ShellResult, testing::fakes::FakeShell};

    use super::*;

    #[tokio::test]
    async fn parses_recursive_descendants_and_lsof_ports() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "ps",
            ShellResult {
                status: 0,
                stdout: "   10     1 shell\n11 10 node server.js\n12 11 worker\n20 1 other\n"
                    .to_owned(),
                stderr: String::new(),
            },
        );
        shell.when(
            |command| command.program == "lsof",
            ShellResult {
                status: 0,
                stdout: "p11\nn*:3000\np12\nn127.0.0.1:4000\n".to_owned(),
                stderr: String::new(),
            },
        );
        let process = RealProcess::new(shell);
        assert_eq!(
            process
                .descendants(10)
                .await
                .unwrap_or_else(|error| panic!("{error}"))
                .iter()
                .map(|entry| entry.pid)
                .collect::<Vec<_>>(),
            vec![11, 12]
        );
        assert_eq!(
            process
                .listening_ports(&[11, 12])
                .await
                .unwrap_or_else(|error| panic!("{error}")),
            vec![
                ListeningPort {
                    pid: 11,
                    port: 3000
                },
                ListeningPort {
                    pid: 12,
                    port: 4000
                }
            ]
        );
    }
}
