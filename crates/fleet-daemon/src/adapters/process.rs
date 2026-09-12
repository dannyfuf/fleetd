//! Process discovery, liveness, and port inspection.

use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, OnceLock},
};

use async_trait::async_trait;

use crate::{
    DaemonError, DaemonResult,
    adapters::shell::{Shell, ShellCommand},
};

/// Where listening-port observations come from on this machine.
///
/// `lsof` ships with macOS but is not installed by default on Arch or a minimal Debian, and
/// the observation refresh runs on every status tick — so the answer is resolved once and
/// reused rather than rediscovered (and re-logged) a few dozen times a minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListeningPortSource {
    /// `lsof` is installed and reports the listeners of the requested pids.
    Lsof,
    /// Linux `/proc` is read directly because `lsof` is not installed.
    ProcFs,
    /// Neither is available, so listening ports are not observed at all.
    Unavailable,
}

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

/// Process observations from `ps`, and listening ports from `lsof` or Linux `/proc`.
#[derive(Clone)]
pub struct RealProcess {
    shell: Arc<dyn Shell>,
    listening_port_source: OnceLock<ListeningPortSource>,
}

impl RealProcess {
    /// Creates a process adapter backed by `shell`.
    #[must_use]
    pub fn new(shell: Arc<dyn Shell>) -> Self {
        Self {
            shell,
            listening_port_source: OnceLock::new(),
        }
    }

    /// Creates an adapter whose port source is fixed, so a test states which path it exercises.
    #[cfg(test)]
    fn with_listening_port_source(shell: Arc<dyn Shell>, source: ListeningPortSource) -> Self {
        Self {
            shell,
            listening_port_source: OnceLock::from(source),
        }
    }

    /// Resolves — once per adapter — how listening ports can be observed here.
    ///
    /// The probe stats `PATH` and `/proc`, so it runs on the blocking pool rather than
    /// stalling a tokio worker. A join failure is not cached: the next call retries instead
    /// of permanently disabling port observation over one panicked task.
    async fn listening_port_source(&self) -> ListeningPortSource {
        if let Some(source) = self.listening_port_source.get() {
            return *source;
        }
        let resolved = match tokio::task::spawn_blocking(resolve_listening_port_source).await {
            Ok(source) => source,
            Err(error) => {
                tracing::warn!(%error, "listening-port source probe failed");
                return ListeningPortSource::Unavailable;
            }
        };
        // Whoever stores the value reports it, so concurrent first calls log once between them.
        if self.listening_port_source.set(resolved).is_ok() {
            report_listening_port_source(resolved);
        }
        self.listening_port_source
            .get()
            .copied()
            .unwrap_or(resolved)
    }

    async fn lsof_listening_ports(&self, pids: &str) -> DaemonResult<Vec<ListeningPort>> {
        let result = self
            .shell
            .run(ShellCommand::new("lsof").args([
                "-nP",
                "-iTCP",
                "-sTCP:LISTEN",
                "-a",
                "-p",
                pids,
                "-F",
                "pn",
            ]))
            .await?;
        // `lsof` exits 1 when nothing matches, which is an empty answer and not a failure.
        if !result.success() && result.status != 1 {
            return Err(DaemonError::Process(format!(
                "lsof exited {}: {}",
                result.status,
                result.stderr.trim()
            )));
        }
        Ok(parse_lsof(&result.stdout))
    }
}

/// Picks the listening-port source. Blocking: stats `PATH` entries and `/proc`.
fn resolve_listening_port_source() -> ListeningPortSource {
    if binary_on_path("lsof") {
        return ListeningPortSource::Lsof;
    }
    if cfg!(target_os = "linux") && Path::new("/proc/net/tcp").exists() {
        return ListeningPortSource::ProcFs;
    }
    ListeningPortSource::Unavailable
}

/// States the consequence of the chosen source exactly once per adapter.
///
/// Reporting here, at the moment the answer is stored, is what keeps a missing `lsof` to one
/// line: letting the refresh fail and the caller log it is what filled a devbox log with the
/// same `lsof: No such file or directory` message on every tick, around the clock.
fn report_listening_port_source(source: ListeningPortSource) {
    match source {
        ListeningPortSource::Lsof => {}
        ListeningPortSource::ProcFs => {
            tracing::debug!("lsof is not installed; reading listening ports from /proc instead");
        }
        ListeningPortSource::Unavailable => tracing::warn!(
            "lsof is not installed and /proc is unavailable; listening-port keep-alive rules \
             are disabled while process rules keep working. Install lsof to restore them \
             (`pacman -S lsof`, `apt install lsof`)."
        ),
    }
}

/// Returns whether an executable of this name exists on `PATH`.
fn binary_on_path(binary: &str) -> bool {
    binary_in_search_path(binary, std::env::var_os("PATH").as_deref())
}

/// Returns whether an executable of this name exists in an explicit search path.
fn binary_in_search_path(binary: &str, search_path: Option<&std::ffi::OsStr>) -> bool {
    let Some(search_path) = search_path else {
        return false;
    };
    std::env::split_paths(search_path).any(|directory| {
        std::fs::metadata(directory.join(binary))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
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
        match self.listening_port_source().await {
            ListeningPortSource::Lsof => {
                let joined = pids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                self.lsof_listening_ports(&joined).await
            }
            ListeningPortSource::ProcFs => {
                let pids = pids.to_vec();
                // Reading a few dozen `/proc/<pid>/fd` directories is blocking filesystem work.
                tokio::task::spawn_blocking(move || {
                    listening_ports_from_proc(Path::new("/proc"), &pids)
                })
                .await
                .map_err(|error| DaemonError::Process(format!("/proc scan failed: {error}")))
            }
            ListeningPortSource::Unavailable => Ok(Vec::new()),
        }
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

/// Resolves the TCP listeners of `pids` from a `/proc` tree, the way `lsof` would.
///
/// `proc_root` is a parameter so the whole walk is exercised against a fixture tree; tests
/// never read the machine's real `/proc`.
fn listening_ports_from_proc(proc_root: &Path, pids: &[u32]) -> Vec<ListeningPort> {
    let mut inode_ports = BTreeMap::new();
    for table in ["net/tcp", "net/tcp6"] {
        // A kernel without IPv6 has no `net/tcp6`; an unreadable table is simply no answer.
        if let Ok(contents) = std::fs::read_to_string(proc_root.join(table)) {
            parse_proc_net_tcp(&contents, &mut inode_ports);
        }
    }
    if inode_ports.is_empty() {
        return Vec::new();
    }
    let mut ports = BTreeSet::new();
    for pid in pids {
        // A process that exited between the snapshot and this walk has no directory left.
        let Ok(entries) = std::fs::read_dir(proc_root.join(pid.to_string()).join("fd")) else {
            continue;
        };
        let mut matched = BTreeSet::new();
        for entry in entries.flatten() {
            // One readlink per descriptor, and a busy process can hold thousands. A pid
            // cannot own more listeners than the machine has, so once this one accounts for
            // all of them the rest of its table cannot change the answer. The bound is
            // per-pid on purpose: a socket shared with another process must still be found
            // on that process too.
            if matched.len() == inode_ports.len() {
                break;
            }
            let Ok(target) = std::fs::read_link(entry.path()) else {
                continue;
            };
            if let Some(inode) = socket_inode(&target.to_string_lossy())
                && let Some(port) = inode_ports.get(&inode)
            {
                matched.insert(inode);
                ports.insert(ListeningPort {
                    pid: *pid,
                    port: *port,
                });
            }
        }
    }
    ports.into_iter().collect()
}

/// Collects the inode and local port of every listening socket in one `/proc/net/tcp` table.
///
/// Columns are `sl local_address rem_address st … inode`; `st` is `0A` for `TCP_LISTEN`, and
/// the local address is `<hex address>:<hex port>`.
fn parse_proc_net_tcp(contents: &str, ports: &mut BTreeMap<u64, u16>) {
    for line in contents.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let (Some(local), Some(state), Some(inode)) = (fields.get(1), fields.get(3), fields.get(9))
        else {
            continue;
        };
        if *state != "0A" {
            continue;
        }
        let (Some(port), Ok(inode)) = (
            local
                .rsplit(':')
                .next()
                .and_then(|port| u16::from_str_radix(port, 16).ok()),
            inode.parse::<u64>(),
        ) else {
            continue;
        };
        ports.insert(inode, port);
    }
}

/// Reads the inode out of a `socket:[12345]` file-descriptor link target.
fn socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
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
        let process = RealProcess::with_listening_port_source(shell, ListeningPortSource::Lsof);
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

    /// A `/proc/net/tcp` table as Linux prints it: a header, one listener, one established
    /// connection that must not be reported, and an IPv6-shaped row in the companion table.
    const PROC_NET_TCP: &str = concat!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4210 1 0000 100 0 0 10 0\n",
        "   1: 0100007F:8080 0100007F:C1B4 01 00000000:00000000 00:00000000 00000000  1000        0 4211 1 0000 100 0 0 10 0\n",
    );
    const PROC_NET_TCP6: &str = concat!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 00000000000000000000000000000000:0FA0 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4212 1 0000 100 0 0 10 0\n",
    );

    #[test]
    fn proc_net_tcp_yields_only_listening_sockets_with_their_inode() {
        let mut ports = BTreeMap::new();
        parse_proc_net_tcp(PROC_NET_TCP, &mut ports);
        parse_proc_net_tcp(PROC_NET_TCP6, &mut ports);

        assert_eq!(ports.get(&4210).copied(), Some(8080));
        assert_eq!(ports.get(&4212).copied(), Some(4000));
        // An established connection is not a listener, however inviting its port looks.
        assert_eq!(ports.get(&4211), None);
        // A truncated or header-only table contributes nothing rather than panicking.
        let mut empty = BTreeMap::new();
        parse_proc_net_tcp("sl local_address\n   0: 0100007F\n", &mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn socket_inodes_are_read_only_from_socket_links() {
        assert_eq!(socket_inode("socket:[4210]"), Some(4210));
        assert_eq!(socket_inode("/dev/pts/3"), None);
        assert_eq!(socket_inode("socket:[not-a-number]"), None);
    }

    #[test]
    fn proc_fallback_reports_the_same_listeners_lsof_would() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temp.path();
        std::fs::create_dir_all(root.join("net")).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(root.join("net/tcp"), PROC_NET_TCP)
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(root.join("net/tcp6"), PROC_NET_TCP6)
            .unwrap_or_else(|error| panic!("{error}"));
        for (pid, links) in [
            (11_u32, vec![("0", "/dev/pts/3"), ("3", "socket:[4210]")]),
            (12_u32, vec![("4", "socket:[4212]"), ("5", "socket:[4211]")]),
        ] {
            let fd = root.join(pid.to_string()).join("fd");
            std::fs::create_dir_all(&fd).unwrap_or_else(|error| panic!("{error}"));
            for (name, target) in links {
                std::os::unix::fs::symlink(target, fd.join(name))
                    .unwrap_or_else(|error| panic!("{error}"));
            }
        }

        assert_eq!(
            listening_ports_from_proc(root, &[11, 12, 99]),
            vec![
                ListeningPort {
                    pid: 11,
                    port: 8080
                },
                ListeningPort {
                    pid: 12,
                    port: 4000
                }
            ]
        );
    }

    /// The per-pid descriptor walk stops once a pid accounts for every listening inode. That
    /// bound must stay per-pid: a socket a server shares with its forked workers is held by
    /// all of them, and attributing it to whichever was scanned first would silently drop a
    /// keep-alive match for the others.
    #[test]
    fn a_socket_shared_between_processes_is_reported_for_each_of_them() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temp.path();
        std::fs::create_dir_all(root.join("net")).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(root.join("net/tcp"), PROC_NET_TCP)
            .unwrap_or_else(|error| panic!("{error}"));
        for pid in [21_u32, 22] {
            let fd = root.join(pid.to_string()).join("fd");
            std::fs::create_dir_all(&fd).unwrap_or_else(|error| panic!("{error}"));
            std::os::unix::fs::symlink("socket:[4210]", fd.join("3"))
                .unwrap_or_else(|error| panic!("{error}"));
            // Descriptors past the match exist only to be skipped by the short circuit.
            for extra in 4..64 {
                std::os::unix::fs::symlink("/dev/null", fd.join(extra.to_string()))
                    .unwrap_or_else(|error| panic!("{error}"));
            }
        }

        assert_eq!(
            listening_ports_from_proc(root, &[21, 22]),
            vec![
                ListeningPort {
                    pid: 21,
                    port: 8080
                },
                ListeningPort {
                    pid: 22,
                    port: 8080
                }
            ]
        );
    }

    #[tokio::test]
    async fn a_machine_without_lsof_reports_no_ports_instead_of_failing_every_tick() {
        let shell = Arc::new(FakeShell::new());
        let process = RealProcess::with_listening_port_source(
            shell.clone(),
            ListeningPortSource::Unavailable,
        );

        assert_eq!(
            process
                .listening_ports(&[11, 12])
                .await
                .unwrap_or_else(|error| panic!("{error}")),
            Vec::new()
        );
        // The missing binary is reported once, when the source is resolved, and never again
        // from the refresh path — so nothing is spawned here at all.
        assert_eq!(shell.calls().len(), 0);
        assert_eq!(
            process.listening_port_source().await,
            ListeningPortSource::Unavailable
        );
    }

    #[test]
    fn path_lookup_accepts_only_executable_files() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(temp.path().join("fleet-not-executable"), b"")
            .unwrap_or_else(|error| panic!("{error}"));
        let executable = temp.path().join("fleet-executable");
        std::fs::write(&executable, b"").unwrap_or_else(|error| panic!("{error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{error}"));
        let search_path = temp.path().as_os_str();

        assert!(binary_in_search_path("fleet-executable", Some(search_path)));
        // A same-named data file is not the tool; treating it as one would make the adapter
        // shell out to something that cannot run.
        assert!(!binary_in_search_path(
            "fleet-not-executable",
            Some(search_path)
        ));
        assert!(!binary_in_search_path("fleet-absent", Some(search_path)));
        assert!(!binary_in_search_path("fleet-executable", None));
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
