//! Launching, recording and reattaching the detached holder process behind every PTY terminal.
//!
//! The daemon never owns a terminal's child. It starts one `fleetd pty-hold` process per terminal,
//! in its own session so no signal aimed at the daemon reaches it, and records what it started in
//! `<fleet-home>/pty/<terminal>.json`. The next daemon reads those sidecars, reattaches to the
//! holders that are still alive, and deletes the records of the ones that are not.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use fleet_core::{
    ids::{SessionId, TerminalId},
    paths::{FleetHome, pty_socket_dir, pty_socket_path, pty_socket_prefix},
    sessions::{Session, SessionKind, Terminal, TerminalKind, TerminalStatus},
};
use serde::{Deserialize, Serialize};

use crate::{DaemonError, DaemonResult, adapters::process::pid_is_alive};

/// Sidecar schema version. New fields are additive and defaulted; a bump means a field's meaning
/// changed, and a record from an unknown version is treated as unreadable and removed.
pub(super) const SIDECAR_VERSION: u32 = 1;

/// How long the daemon waits for a freshly launched holder to bind its socket.
const HOLDER_START_TIMEOUT: Duration = Duration::from_secs(10);
/// Interval between checks for the holder's socket while it starts.
const HOLDER_START_POLL: Duration = Duration::from_millis(20);

/// What a restarted daemon needs to rebuild one terminal's registry record.
///
/// Everything here is already-persisted vocabulary (`Session`, `Terminal`) plus the two runtime
/// facts a restarted daemon cannot recompute: which process holds the PTY, and where it listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtySidecar {
    /// Schema version of this record.
    pub(crate) version: u32,
    /// Terminal identifier, reused verbatim on adoption so client tabs reconnect.
    pub(crate) terminal: TerminalId,
    /// Owning session identifier.
    pub(crate) session: SessionId,
    /// Owning session's workload, so the session record can be rebuilt.
    pub(crate) session_kind: SessionKind,
    /// Owning session's default working directory.
    pub(crate) session_cwd: String,
    /// Session-local terminal name, which is the `windows` entry that configured it.
    pub(crate) name: String,
    /// Command typed into the login shell when the terminal was created.
    pub(crate) command: String,
    /// Terminal working directory.
    pub(crate) cwd: String,
    /// Holder process identifier; a dead one makes this record stale.
    pub(crate) holder_pid: u32,
    /// Login-shell process identifier reported by the holder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) shell_pid: Option<u32>,
    /// Socket the holder bound, recorded rather than recomputed.
    ///
    /// The name carries a per-spawn nonce and the directory may be outside the Fleet home, so this
    /// is the only way back to a holder — recomputing it is not possible.
    pub(crate) socket: PathBuf,
    /// ISO-8601 creation time.
    pub(crate) created_at: String,
}

/// The grid a reattaching daemon falls back to when a holder reports no window size of its own.
///
/// The holder's greeting is authoritative — its child has been resized since the terminal was
/// created — so this is a last resort, not a record. It is deliberately not persisted: a value
/// that goes stale on every resize is worse than one derived from the same defaults as a new
/// terminal.
pub(crate) const FALLBACK_COLS: u16 = super::INITIAL_COLS;
/// Rows counterpart of [`FALLBACK_COLS`].
pub(crate) const FALLBACK_ROWS: u16 = super::INITIAL_ROWS;

impl PtySidecar {
    /// Rebuilds the terminal record this sidecar describes.
    pub(crate) fn terminal(&self) -> Terminal {
        Terminal {
            id: self.terminal,
            name: self.name.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            shell_pid: self.shell_pid,
            foreground_command: None,
            status: TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
            agent_attention: None,
            kind: TerminalKind::Pty,
        }
    }

    /// Rebuilds the empty session record this sidecar's terminal belongs to.
    pub(crate) fn session(&self) -> Session {
        Session {
            id: self.session.clone(),
            host: None,
            kind: self.session_kind.clone(),
            cwd: self.session_cwd.clone(),
            terminals: Vec::new(),
            active_terminal: None,
            slept_at: None,
            kept_terminals: Vec::new(),
        }
    }

    /// Whether this record can still describe a live holder.
    pub(crate) fn is_readable(&self) -> bool {
        self.version == SIDECAR_VERSION
    }
}

/// A holder the daemon started and is now responsible for recording.
pub(crate) struct StartedHolder {
    /// Socket the holder bound.
    pub(crate) socket: PathBuf,
    /// Holder process identifier.
    pub(crate) pid: u32,
}

/// Starts one detached holder and waits until it is listening.
///
/// The holder is `setsid`-ed before `exec`, so it leaves the daemon's session and process group:
/// `fleet daemon restart`'s SIGTERM, a `ctrl-c` in the daemon's terminal, and the daemon's own
/// exit all leave it running. Its stdio goes to `logs/pty-hold.log`; nothing is inherited.
pub(crate) async fn start_holder(
    home: &FleetHome,
    terminal: TerminalId,
    session: &SessionId,
    name: &str,
    cwd: &str,
    cols: u16,
    rows: u16,
) -> DaemonResult<StartedHolder> {
    // One socket name per spawn, never per terminal: identifiers are reused by `restart_terminal`.
    let socket = pty_socket_path(
        home,
        terminal,
        &uuid::Uuid::new_v4().simple().to_string()[..16],
    );
    let sidecar = home.pty_sidecar_path(terminal);
    let pty_dir = home.pty_dir();
    let logs_dir = home.logs_dir();
    let log_path = home.pty_log_path();
    let log = tokio::task::spawn_blocking({
        let log_path = log_path.clone();
        move || -> DaemonResult<std::fs::File> {
            // The records are as sensitive as the sockets: they name the control channel.
            create_private_dir(&pty_dir)?;
            std::fs::create_dir_all(&logs_dir)
                .map_err(|error| DaemonError::fs(&logs_dir, error))?;
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|error| DaemonError::fs(&log_path, error))?;
            Ok(log)
        }
    })
    .await
    .map_err(|error| DaemonError::Join(error.to_string()))??;
    let errors = log
        .try_clone()
        .map_err(|error| DaemonError::fs(&log_path, error))?;

    let program = holder_program()?;
    let mut command = tokio::process::Command::new(&program);
    command
        .arg("--home")
        .arg(home.root())
        .arg("pty-hold")
        .arg("--terminal")
        .arg(terminal.0.to_string())
        .arg("--session")
        .arg(session.as_str())
        .arg("--name")
        .arg(name)
        .arg("--cwd")
        .arg(cwd)
        .arg("--socket")
        .arg(&socket)
        .arg("--sidecar")
        .arg(&sidecar)
        .arg("--cols")
        .arg(cols.to_string())
        .arg("--rows")
        .arg(rows.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors));
    // SAFETY: the callback runs between fork and exec and calls only `setsid`, which is
    // async-signal-safe. This mirrors how `fleet` detaches `fleetd` itself.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|error| {
        DaemonError::Process(format!("failed to start {}: {error}", program.display()))
    })?;
    let pid = child.id().ok_or_else(|| {
        DaemonError::Process(format!(
            "pty holder for terminal {terminal} reported no pid"
        ))
    })?;

    let deadline = tokio::time::Instant::now() + HOLDER_START_TIMEOUT;
    loop {
        if tokio::fs::try_exists(&socket).await.unwrap_or(false) {
            break;
        }
        if let Some(status) = child.try_wait().map_err(|error| {
            DaemonError::Process(format!("pty holder for terminal {terminal}: {error}"))
        })? {
            return Err(DaemonError::Process(format!(
                "pty holder for terminal {terminal} exited with {status} before listening; see {}",
                log_path.display()
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            if let Err(error) = child.start_kill() {
                tracing::warn!(%error, %terminal, "failed to stop a pty holder that never listened");
            }
            return Err(DaemonError::Timeout(format!(
                "pty holder for terminal {terminal} did not bind {}",
                socket.display()
            )));
        }
        tokio::time::sleep(HOLDER_START_POLL).await;
    }

    // Reap the holder without waiting for it: the task ends exactly when the child does, so it
    // takes no cancellation token — aborting it would only leave a zombie for this daemon's life.
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => tracing::info!(%terminal, code = status.code(), "pty holder exited"),
            Err(error) => {
                tracing::warn!(%error, %terminal, "failed to observe a pty holder's exit")
            }
        }
    });
    tracing::info!(%terminal, pid, socket = %socket.display(), "pty holder spawned");
    Ok(StartedHolder { socket, pid })
}

/// Writes a holder's sidecar, replacing any predecessor atomically.
pub(crate) async fn write_sidecar(home: &FleetHome, sidecar: &PtySidecar) -> DaemonResult<()> {
    let path = home.pty_sidecar_path(sidecar.terminal);
    let temporary = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(sidecar).map_err(|error| {
        DaemonError::Validation(format!("failed to encode a pty sidecar: {error}"))
    })?;
    tokio::fs::write(&temporary, &encoded)
        .await
        .map_err(|error| DaemonError::fs(&temporary, error))?;
    tokio::fs::rename(&temporary, &path)
        .await
        .map_err(|error| DaemonError::fs(&path, error))
}

/// One sidecar found on disk, paired with the record it holds when it could be read.
pub(crate) struct DiscoveredSidecar {
    /// Sidecar file path.
    pub(crate) path: PathBuf,
    /// Parsed record, or `None` when the file is unreadable or from an unknown version.
    pub(crate) sidecar: Option<PtySidecar>,
}

/// Reads every sidecar in `<fleet-home>/pty`, in terminal order.
///
/// A missing directory is the ordinary first-run case and yields no records.
pub(crate) async fn discover_sidecars(home: &FleetHome) -> DaemonResult<Vec<DiscoveredSidecar>> {
    let directory = home.pty_dir();
    let mut entries = match tokio::fs::read_dir(&directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(DaemonError::fs(&directory, error)),
    };
    let mut discovered = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| DaemonError::fs(&directory, error))?
    {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let sidecar = match tokio::fs::read(&path).await {
            Ok(bytes) => parse_sidecar(&path, &bytes),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "failed to read a pty sidecar");
                None
            }
        };
        discovered.push(DiscoveredSidecar { path, sidecar });
    }
    discovered.sort_by_key(|entry| {
        entry
            .sidecar
            .as_ref()
            .map_or(u64::MAX, |sidecar| sidecar.terminal.0)
    });
    Ok(discovered)
}

/// Whether the holder a sidecar names is still running.
pub(crate) fn holder_is_alive(sidecar: &PtySidecar) -> bool {
    pid_is_alive(sidecar.holder_pid)
}

/// Replaces the mutable half of a holder's record after the terminal was renamed.
///
/// The record is what a later daemon rebuilds the registry entry from, so a name it never hears
/// about is a name the user loses at the next restart. Grid size is deliberately absent from the
/// record — the holder reports its own — so resizing needs no write at all.
pub(crate) async fn rename_in_sidecar(home: &FleetHome, terminal: TerminalId, name: &str) {
    let path = home.pty_sidecar_path(terminal);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        // A terminal with no record — a native tab, or one whose write failed — has nothing to fix.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(%error, %terminal, "failed to read a pty holder record to rename it");
            return;
        }
    };
    let Some(mut sidecar) = parse_sidecar(&path, &bytes) else {
        return;
    };
    if sidecar.name == name {
        return;
    }
    sidecar.name = name.to_owned();
    if let Err(error) = write_sidecar(home, &sidecar).await {
        tracing::warn!(%error, %terminal, "failed to record a renamed terminal for the next daemon");
    }
}

/// Ends a holder deliberately and then removes its record.
///
/// The one safe way to retire a live holder this daemon cannot drive: unlinking the socket first
/// would leave its shell running with no way back to it, forever.
pub(crate) async fn stop_and_remove(path: &Path, socket: &Path) {
    if let Err(error) = fleet_term::HolderPty::stop(socket) {
        tracing::warn!(%error, socket = %socket.display(), "failed to stop a pty holder before dropping its record");
    }
    remove_record(path, Some(socket)).await;
}

/// Ends every holder whose socket still names `terminal`, for a record that cannot be read.
///
/// An unreadable record names neither a pid nor a socket, but the socket file still carries the
/// terminal identifier, which is enough to reach the holder and stop it rather than orphan it.
pub(crate) async fn stop_unreadable_record(home: &FleetHome, path: &Path) {
    let Some(terminal) = terminal_of_record(path) else {
        tracing::warn!(path = %path.display(), "removing a pty record whose name carries no terminal");
        remove_record(path, None).await;
        return;
    };
    for socket in sockets_of(home, terminal).await {
        tracing::warn!(
            %terminal,
            socket = %socket.display(),
            "stopping a pty holder whose record this daemon cannot read"
        );
        if let Err(error) = fleet_term::HolderPty::stop(&socket) {
            tracing::warn!(%error, %terminal, "failed to stop that holder");
        }
        remove_quietly(&socket).await;
    }
    remove_record(path, None).await;
}

/// Returns the sockets in this home's holder directory that belong to `terminal`.
pub(crate) async fn sockets_of(home: &FleetHome, terminal: TerminalId) -> Vec<PathBuf> {
    let directory = pty_socket_dir(home);
    let prefix = pty_socket_prefix(terminal);
    let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
        return Vec::new();
    };
    let mut sockets = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) && name.ends_with(".sock") {
            sockets.push(path);
        }
    }
    sockets
}

/// Removes a sidecar and the socket it names, tolerating either already being gone.
pub(crate) async fn remove_record(path: &Path, socket: Option<&Path>) {
    remove_quietly(path).await;
    if let Some(socket) = socket {
        remove_quietly(socket).await;
    }
}

/// Lists every socket and record under this home's holder directories.
///
/// `fleet doctor` uses it to name the holders no session claims, which is the only way an orphaned
/// shell becomes visible rather than merely invisible.
pub(crate) async fn runtime_files(home: &FleetHome) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut directories = vec![home.pty_dir()];
    let sockets = pty_socket_dir(home);
    if sockets != home.pty_dir() {
        directories.push(sockets);
    }
    for directory in directories {
        let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            found.push(entry.path());
        }
    }
    found.sort();
    found
}

/// Returns the terminal a record's file name names.
fn terminal_of_record(path: &Path) -> Option<TerminalId> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.parse().ok())
        .map(TerminalId)
}

/// Creates a directory only this user can enter.
fn create_private_dir(directory: &Path) -> DaemonResult<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(directory).map_err(|error| DaemonError::fs(directory, error))?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| DaemonError::fs(directory, error))
}

fn parse_sidecar(path: &Path, bytes: &[u8]) -> Option<PtySidecar> {
    match serde_json::from_slice::<PtySidecar>(bytes) {
        Ok(sidecar) if sidecar.is_readable() => Some(sidecar),
        Ok(sidecar) => {
            tracing::warn!(
                path = %path.display(),
                version = sidecar.version,
                "ignoring a pty sidecar written by an unknown version"
            );
            None
        }
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "failed to parse a pty sidecar");
            None
        }
    }
}

async fn remove_quietly(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "failed to remove a pty holder record");
        }
    }
}

/// Resolves the `fleetd` executable a holder must be started from.
///
/// The running daemon is by definition the right holder binary, so `current_exe` answers first and
/// a stale `FLEET_DAEMON` from another checkout cannot mix builds. `FLEET_DAEMON` is the fallback
/// for a suite whose own executable is a test harness, which has no other way to say where `fleetd`
/// is. Anything else is refused rather than executed: launching a test harness as a holder would
/// run the suite again.
fn holder_program() -> DaemonResult<PathBuf> {
    let current = std::env::current_exe().map_err(|error| {
        DaemonError::Process(format!(
            "failed to resolve the fleetd executable for a pty holder: {error}"
        ))
    })?;
    if current
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.starts_with("fleetd"))
    {
        return Ok(current);
    }
    if let Some(path) = std::env::var_os("FLEET_DAEMON") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(DaemonError::Process(format!(
        "cannot start a pty holder: {} is not fleetd and $FLEET_DAEMON names no fleetd binary \
         (run the suite through `make test`)",
        current.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PtySidecar {
        PtySidecar {
            version: SIDECAR_VERSION,
            terminal: TerminalId(12),
            session: SessionId::try_from("api/feature").expect("session id"),
            session_kind: SessionKind::Worktree("acme/api#feature".parse().expect("worktree id")),
            session_cwd: "/w/acme/api/feature".to_owned(),
            name: "cc".to_owned(),
            command: "claude".to_owned(),
            cwd: "/w/acme/api/feature".to_owned(),
            holder_pid: 4_242,
            shell_pid: Some(4_243),
            socket: PathBuf::from("/home/df/.fleet/pty/12-0123456789abcdef.sock"),
            created_at: "2026-09-12T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn a_sidecar_round_trips_through_json() {
        let sidecar = sample();
        let encoded = serde_json::to_vec(&sidecar).expect("encode");
        let decoded: PtySidecar = serde_json::from_slice(&encoded).expect("decode");
        assert_eq!(decoded, sidecar);
        assert!(decoded.is_readable());
    }

    #[test]
    fn a_sidecar_rebuilds_its_terminal_and_session_records() {
        let sidecar = sample();
        let terminal = sidecar.terminal();
        assert_eq!(terminal.id, TerminalId(12));
        assert_eq!(terminal.name, "cc");
        assert_eq!(terminal.shell_pid, Some(4_243));
        assert_eq!(terminal.kind, TerminalKind::Pty);
        assert_eq!(terminal.status, TerminalStatus::Running);

        let session = sidecar.session();
        assert_eq!(session.id, sidecar.session);
        assert_eq!(session.kind, sidecar.session_kind);
        assert_eq!(session.cwd, "/w/acme/api/feature");
        assert!(session.terminals.is_empty());
    }

    #[test]
    fn an_absent_shell_pid_is_omitted_and_defaults_back() {
        let mut sidecar = sample();
        sidecar.shell_pid = None;
        let encoded = serde_json::to_string(&sidecar).expect("encode");
        assert!(!encoded.contains("shellPid"));
        let decoded: PtySidecar = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded.shell_pid, None);
    }

    #[test]
    fn a_record_from_an_unknown_version_is_not_readable() {
        let mut sidecar = sample();
        sidecar.version = SIDECAR_VERSION + 1;
        let encoded = serde_json::to_vec(&sidecar).expect("encode");
        assert!(parse_sidecar(Path::new("/tmp/9.json"), &encoded).is_none());
        assert!(parse_sidecar(Path::new("/tmp/9.json"), b"{").is_none());
    }

    #[tokio::test]
    async fn discovery_returns_nothing_before_the_first_terminal() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = FleetHome::new(temp.path().join("fleet"));
        assert!(
            discover_sidecars(&home)
                .await
                .unwrap_or_else(|error| panic!("{error}"))
                .is_empty()
        );
    }

    #[tokio::test]
    async fn discovery_reads_records_in_terminal_order_and_reports_unreadable_ones() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = FleetHome::new(temp.path().join("fleet"));
        std::fs::create_dir_all(home.pty_dir()).unwrap_or_else(|error| panic!("{error}"));
        let mut second = sample();
        second.terminal = TerminalId(2);
        let mut first = sample();
        first.terminal = TerminalId(1);
        write_sidecar(&home, &second)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        write_sidecar(&home, &first)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(home.pty_dir().join("99.json"), b"not json")
            .unwrap_or_else(|error| panic!("{error}"));
        // A socket in the same directory must not be mistaken for a record.
        std::fs::write(home.pty_dir().join("1-0123456789abcdef.sock"), b"")
            .unwrap_or_else(|error| panic!("{error}"));

        let discovered = discover_sidecars(&home)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let terminals = discovered
            .iter()
            .map(|entry| entry.sidecar.as_ref().map(|sidecar| sidecar.terminal))
            .collect::<Vec<_>>();
        assert_eq!(
            terminals,
            vec![Some(TerminalId(1)), Some(TerminalId(2)), None]
        );
    }

    #[tokio::test]
    async fn renaming_a_terminal_is_recorded_for_the_next_daemon() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = FleetHome::new(temp.path().join("fleet"));
        std::fs::create_dir_all(home.pty_dir()).unwrap_or_else(|error| panic!("{error}"));
        let record = sample();
        write_sidecar(&home, &record)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        rename_in_sidecar(&home, record.terminal, "agent").await;
        let reread = discover_sidecars(&home)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            reread
                .first()
                .and_then(|entry| entry.sidecar.as_ref())
                .map(|sidecar| sidecar.name.as_str()),
            Some("agent"),
            "an adopted terminal must come back under the name the user gave it"
        );

        // A terminal with no record — a native tab — is not an error to rename.
        rename_in_sidecar(&home, TerminalId(999), "anything").await;
    }

    #[tokio::test]
    async fn removing_a_record_clears_the_sidecar_and_its_socket_once() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let sidecar = temp.path().join("7.json");
        let socket = temp.path().join("7.sock");
        std::fs::write(&sidecar, b"{}").unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(&socket, b"").unwrap_or_else(|error| panic!("{error}"));

        remove_record(&sidecar, Some(&socket)).await;
        assert!(!sidecar.exists());
        assert!(!socket.exists());
        // Idempotent: adoption and the holder itself both clean up.
        remove_record(&sidecar, Some(&socket)).await;
    }

    #[test]
    fn a_dead_holder_pid_makes_a_record_stale() {
        let mut sidecar = sample();
        sidecar.holder_pid = std::process::id();
        assert!(holder_is_alive(&sidecar));
        // PID 0 is never a live process from `kill(2)`'s point of view here.
        sidecar.holder_pid = 0;
        assert!(!holder_is_alive(&sidecar));
    }
}
