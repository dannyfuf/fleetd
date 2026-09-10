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
    /// Kernel process-group identifier.
    pub process_group_id: u32,
    /// Foreground process group for the controlling terminal, when present.
    pub terminal_foreground_process_group_id: Option<u32>,
    /// Process start marker used to distinguish reused process identifiers.
    pub start_identity: String,
    /// Full command line.
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    start_identity: String,
}

impl ProcessInfo {
    pub(crate) fn identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            start_identity: self.start_identity.clone(),
        }
    }
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
    /// Captures process identity, hierarchy, terminal groups, and command lines.
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
            .run(ShellCommand::new("ps").args(["-axo", "pid=,ppid=,pgid=,tpgid=,lstart=,command="]))
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
            return Ok(parse_environment_entries(&bytes));
        }
        #[cfg(target_os = "macos")]
        {
            read_macos_environment(pid)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(DaemonError::Unsupported(
                "structured process environment inspection is unavailable on this platform"
                    .to_owned(),
            ))
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

fn parse_environment_entries(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('=')?;
            valid_environment_key(key).then(|| (key.to_owned(), value.to_owned()))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn read_macos_environment(pid: u32) -> DaemonResult<Vec<(String, String)>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid.cast_signed()];
    let mut size = 0_usize;
    // SAFETY: sysctl receives a valid three-element MIB and an output-size pointer.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(DaemonError::Process(format!(
            "sysctl environment size for pid {pid}: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut bytes = vec![0_u8; size];
    // SAFETY: `bytes` owns `size` writable bytes and sysctl updates the initialized length.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(DaemonError::Process(format!(
            "sysctl environment for pid {pid}: {}",
            std::io::Error::last_os_error()
        )));
    }
    bytes.truncate(size);
    parse_macos_procargs(&bytes)
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_procargs(bytes: &[u8]) -> DaemonResult<Vec<(String, String)>> {
    let argc_bytes: [u8; std::mem::size_of::<i32>()] = bytes
        .get(..std::mem::size_of::<i32>())
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| DaemonError::Process("truncated KERN_PROCARGS2 payload".to_owned()))?;
    let argc = i32::from_ne_bytes(argc_bytes);
    let argc = usize::try_from(argc)
        .map_err(|_| DaemonError::Process("negative KERN_PROCARGS2 argc".to_owned()))?;
    let mut cursor = std::mem::size_of::<i32>();
    cursor = skip_nul_terminated(bytes, cursor, "executable path")?;
    while bytes.get(cursor) == Some(&0) {
        cursor += 1;
    }
    for _ in 0..argc {
        cursor = skip_nul_terminated(bytes, cursor, "process argument")?;
    }
    Ok(parse_environment_entries(&bytes[cursor..]))
}

#[cfg(any(target_os = "macos", test))]
fn skip_nul_terminated(bytes: &[u8], cursor: usize, field: &str) -> DaemonResult<usize> {
    let end = bytes[cursor..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|offset| cursor + offset)
        .ok_or_else(|| DaemonError::Process(format!("unterminated KERN_PROCARGS2 {field}")))?;
    Ok(end + 1)
}

fn valid_environment_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('A'..='Z' | '_'))
        && chars.all(|character| matches!(character, 'A'..='Z' | '0'..='9' | '_'))
}

fn parse_process(line: &str) -> DaemonResult<ProcessInfo> {
    let invalid = || DaemonError::Process(format!("invalid ps row: {line}"));
    let mut remaining = line;
    let pid = take_process_field(&mut remaining)
        .ok_or_else(&invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let parent_pid = take_process_field(&mut remaining)
        .ok_or_else(&invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let process_group_id = take_process_field(&mut remaining)
        .ok_or_else(&invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let terminal_foreground_process_group_id = take_process_field(&mut remaining)
        .ok_or_else(&invalid)?
        .parse::<i64>()
        .map_err(|_| invalid())?
        .try_into()
        .ok()
        .filter(|group: &u32| *group != 0);
    let start_identity = (0..5)
        .map(|_| take_process_field(&mut remaining).ok_or_else(&invalid))
        .collect::<DaemonResult<Vec<_>>>()?
        .join(" ");
    let command = remaining.trim_start().to_owned();
    Ok(ProcessInfo {
        pid,
        parent_pid,
        process_group_id,
        terminal_foreground_process_group_id,
        start_identity,
        command,
    })
}

fn take_process_field<'a>(remaining: &mut &'a str) -> Option<&'a str> {
    *remaining = remaining.trim_start();
    if remaining.is_empty() {
        return None;
    }
    let end = remaining
        .find(char::is_whitespace)
        .unwrap_or(remaining.len());
    let (field, rest) = remaining.split_at(end);
    *remaining = rest;
    Some(field)
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
                stdout: concat!(
                    "  10   1  10  12 Sun Sep  6 09:00:00 2026 shell\n",
                    "  11  10  12  12 Sun Sep  6 09:00:01 2026 node server.js\n",
                    "  12  11  12  12 Sun Sep  6 09:00:02 2026 worker\n",
                    "  20   1  20  -1 Sun Sep  6 09:00:03 2026 other\n",
                )
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
        let snapshot = process
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(snapshot[1].process_group_id, 12);
        assert_eq!(snapshot[1].terminal_foreground_process_group_id, Some(12));
        assert_eq!(snapshot[1].start_identity, "Sun Sep 6 09:00:01 2026");
        assert_eq!(snapshot[1].command, "node server.js");
        assert_eq!(snapshot[3].terminal_foreground_process_group_id, None);
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
    fn environment_ignores_argv_equals() {
        let mut procargs = 2_i32.to_ne_bytes().to_vec();
        procargs.extend_from_slice(
            b"/bin/worker\0\0worker\0ARGUMENT=not-environment\0FLEET_SESSION=repo/main\0TMPDIR=/tmp/a b\0",
        );
        assert_eq!(
            parse_macos_procargs(&procargs).expect("parse structured process data"),
            vec![
                ("FLEET_SESSION".to_owned(), "repo/main".to_owned()),
                ("TMPDIR".to_owned(), "/tmp/a b".to_owned()),
            ]
        );
    }
}
