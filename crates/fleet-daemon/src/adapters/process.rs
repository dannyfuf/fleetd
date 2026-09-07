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
    /// Returns TCP listeners owned by the supplied process identifiers.
    async fn listening_ports(&self, pids: &[u32]) -> DaemonResult<Vec<ListeningPort>>;
    /// Reads a same-user process environment without mutating it.
    async fn environment(&self, pid: u32) -> DaemonResult<Vec<(String, String)>>;
    /// Returns whether a process currently exists or is permission-protected.
    fn is_alive(&self, pid: u32) -> bool;
}

/// Process observations from `ps` and `lsof`.
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

    async fn environment(&self, pid: u32) -> DaemonResult<Vec<(String, String)>> {
        #[cfg(target_os = "linux")]
        {
            let path = format!("/proc/{pid}/environ");
            let bytes = std::fs::read(&path).map_err(|error| DaemonError::fs(&path, error))?;
            return Ok(bytes
                .split(|byte| *byte == 0)
                .filter_map(|entry| {
                    let entry = String::from_utf8_lossy(entry);
                    let (key, value) = entry.split_once('=')?;
                    Some((key.to_owned(), value.to_owned()))
                })
                .collect());
        }
        #[cfg(not(target_os = "linux"))]
        {
            let result = self
                .shell
                .run(ShellCommand::new("ps").args([
                    "-E",
                    "-ww",
                    "-o",
                    "command=",
                    "-p",
                    &pid.to_string(),
                ]))
                .await?;
            if !result.success() {
                return Err(DaemonError::Process(format!(
                    "ps environment for pid {pid} exited {}: {}",
                    result.status,
                    result.stderr.trim()
                )));
            }
            Ok(parse_environment_suffix(&result.stdout))
        }
    }

    fn is_alive(&self, pid: u32) -> bool {
        pid_is_alive(pid)
    }
}

pub(crate) fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal zero checks process existence without delivering a signal.
    let result = unsafe { libc::kill(pid.cast_signed(), 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn parse_environment_suffix(command: &str) -> Vec<(String, String)> {
    let mut environment = Vec::new();
    for token in command.split_whitespace().rev() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        if !valid_environment_key(key) {
            continue;
        }
        environment.push((key.to_owned(), value.to_owned()));
    }
    environment.reverse();
    environment
}

fn valid_environment_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('A'..='Z' | '_'))
        && chars.all(|character| matches!(character, 'A'..='Z' | '0'..='9' | '_'))
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
        .map_err(|_| DaemonError::Process(format!("invalid ps row: {line}")))?;
    let parent_pid = parent_pid
        .parse()
        .map_err(|_| DaemonError::Process(format!("invalid ps row: {line}")))?;
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
    async fn parses_process_table_and_lsof_ports() {
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
                .snapshot()
                .await
                .unwrap_or_else(|error| panic!("{error}"))
                .iter()
                .map(|entry| (entry.pid, entry.parent_pid))
                .collect::<Vec<_>>(),
            vec![(10, 1), (11, 10), (12, 11), (20, 1)]
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

    #[test]
    fn parses_only_environment_tokens_from_the_right() {
        assert_eq!(
            parse_environment_suffix(
                "/usr/bin/node worker --flag VALUE FLEET_SESSION=repo/main TMPDIR=/tmp/a"
            ),
            vec![
                ("FLEET_SESSION".to_owned(), "repo/main".to_owned()),
                ("TMPDIR".to_owned(), "/tmp/a".to_owned()),
            ]
        );
    }
}
