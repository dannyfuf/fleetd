//! Cheap, read-only discovery and log tailing for agent subprocesses.

use crate::{
    DaemonResult,
    adapters::process::{Process, ProcessInfo},
    stores::config::ConfigStore,
};
use fleet_core::{
    config::{Config, DiscoveredWatchRule},
    ids::{SessionId, TerminalId},
    sessions::{Session, Terminal},
    watches::{Watch, WatchId, WatchSource, WatchStatus},
};
use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

use super::{sessions::Sessions, watches::Watches};

const LOG_POLL: Duration = Duration::from_millis(500);
const INITIAL_LOG_BYTES: u64 = 64 * 1024;
const HELPER_PATTERNS: &[&str] = &[
    r"app-server-broker\.mjs",
    r"codex app-server",
    r"codex-code-mode-host",
    r"unified-computer-use.*launch\.mjs",
    r"codex-companion\.mjs status",
];

#[derive(Default)]
struct DiscoveryState {
    environments: BTreeMap<u32, Vec<(String, String)>>,
    tracked: BTreeMap<u32, Tracked>,
}

struct Tracked {
    watch: WatchId,
    companion: Option<Companion>,
}

struct Companion {
    job_id: String,
    json_file: PathBuf,
    log_file: PathBuf,
    offset: u64,
    identity: Option<(u64, u64)>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompanionJob {
    #[serde(default)]
    kind_label: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    phase: String,
    log_file: Option<PathBuf>,
    started_at: Option<String>,
}

/// Observes candidate processes and mirrors their metadata into [`Watches`].
#[derive(Clone)]
pub(crate) struct WatchDiscovery {
    config: Arc<ConfigStore>,
    sessions: Sessions,
    process: Arc<dyn Process>,
    watches: Watches,
    temp_dir: PathBuf,
    state: Arc<Mutex<DiscoveryState>>,
}

impl WatchDiscovery {
    pub(crate) fn new(
        config: Arc<ConfigStore>,
        sessions: Sessions,
        process: Arc<dyn Process>,
        watches: Watches,
    ) -> Self {
        Self {
            config,
            sessions,
            process,
            watches,
            temp_dir: std::env::temp_dir(),
            state: Arc::new(Mutex::new(DiscoveryState::default())),
        }
    }

    #[cfg(test)]
    fn with_temp_dir(mut self, temp_dir: PathBuf) -> Self {
        self.temp_dir = temp_dir;
        self
    }

    pub(crate) async fn discover_once(&self) -> DaemonResult<()> {
        let config = self.config.load().await?;
        if !config.discovered_watches.enabled {
            return Ok(());
        }
        let snapshot = self.process.snapshot().await?;
        self.discover(&config, &self.sessions.snapshot(), &snapshot)
            .await
    }

    async fn discover(
        &self,
        config: &Config,
        sessions: &[Session],
        snapshot: &[ProcessInfo],
    ) -> DaemonResult<()> {
        let live_pids = snapshot
            .iter()
            .map(|process| process.pid)
            .collect::<BTreeSet<_>>();
        self.lock()
            .environments
            .retain(|pid, _| live_pids.contains(pid));
        let patterns = enabled_patterns(&config.discovered_watches.processes);
        let helpers = HELPER_PATTERNS
            .iter()
            .filter_map(|pattern| Regex::new(pattern).ok())
            .collect::<Vec<_>>();
        let candidates = snapshot
            .iter()
            .filter(|process| {
                patterns
                    .iter()
                    .any(|pattern| pattern.is_match(&process.command))
            })
            .filter(|process| {
                !helpers
                    .iter()
                    .any(|pattern| pattern.is_match(&process.command))
            })
            .filter(|process| !is_wrapper_shell(&process.command))
            .filter(|process| !is_primary_agent(process, sessions))
            .cloned()
            .collect::<Vec<_>>();
        let candidate_pids = candidates
            .iter()
            .map(|process| process.pid)
            .collect::<BTreeSet<_>>();

        for candidate in candidates {
            if has_matching_ancestor(candidate.pid, &candidate_pids, snapshot) {
                continue;
            }
            if self.watches.watch_for_pid(candidate.pid).is_some() {
                continue;
            }
            let environment = self.environment(candidate.pid).await;
            let Some((session, terminal)) =
                resolve_ownership(&candidate, &environment, sessions, snapshot, config)
            else {
                continue;
            };
            let companion =
                companion_from_command(&candidate.command, &environment, &self.temp_dir);
            let job = companion
                .as_ref()
                .and_then(|companion| read_job(&companion.json_file));
            let label = companion.as_ref().map_or_else(
                || command_basename(&candidate.command),
                |companion| companion_label(&companion.job_id, job.as_ref()),
            );
            let log_file = companion.as_ref().map(|companion| {
                job.as_ref()
                    .and_then(|job| job.log_file.clone())
                    .unwrap_or_else(|| companion.log_file.clone())
            });
            let watch = Watch {
                id: WatchId(0),
                session: session.id.clone(),
                terminal: terminal.id,
                label,
                command: command_tokens(&candidate.command),
                cwd: companion
                    .as_ref()
                    .and_then(|companion| companion_workspace(&candidate.command, companion))
                    .or_else(|| Some(PathBuf::from(&terminal.cwd))),
                pid: Some(candidate.pid),
                started_at: job
                    .as_ref()
                    .and_then(|job| job.started_at.clone())
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                status: WatchStatus::Running,
                source: WatchSource::Discovered,
                log_file,
            };
            let initial_output = companion.is_none().then(|| {
                format!(
                    "discovered pid {} ({}); output is not captured for this process\n",
                    candidate.pid, candidate.command
                )
            });
            let Some(watch_id) = self.watches.start_discovered(watch, initial_output) else {
                continue;
            };
            let companion = companion.map(|mut companion| {
                if let Some(job) = &job
                    && let Some(log_file) = &job.log_file
                {
                    companion.log_file = log_file.clone();
                }
                companion.offset = initial_offset(&companion.log_file);
                companion.identity = file_identity(&companion.log_file);
                companion
            });
            self.lock().tracked.insert(
                candidate.pid,
                Tracked {
                    watch: watch_id,
                    companion,
                },
            );
        }
        Ok(())
    }

    async fn environment(&self, pid: u32) -> Vec<(String, String)> {
        if let Some(environment) = self.lock().environments.get(&pid).cloned() {
            return environment;
        }
        let environment = self.process.environment(pid).await.unwrap_or_default();
        self.lock().environments.insert(pid, environment.clone());
        environment
    }

    pub(crate) fn poll_once(&self) {
        let pids = self.lock().tracked.keys().copied().collect::<Vec<_>>();
        for pid in pids {
            let mut tracked = match self.lock().tracked.remove(&pid) {
                Some(tracked) => tracked,
                None => continue,
            };
            let mut terminal_code = None;
            if let Some(companion) = &mut tracked.companion {
                let job = read_job(&companion.json_file);
                if let Some(job) = &job {
                    if let Some(log_file) = &job.log_file
                        && *log_file != companion.log_file
                    {
                        companion.log_file = log_file.clone();
                        companion.offset = initial_offset(log_file);
                        companion.identity = file_identity(log_file);
                    }
                    let _ = self.watches.update_discovered(
                        tracked.watch,
                        companion_label(&companion.job_id, Some(job)),
                        Some(companion.log_file.clone()),
                    );
                    terminal_code = terminal_job_code(job);
                }
                let identity = file_identity(&companion.log_file);
                if companion.identity.is_some() && identity != companion.identity {
                    companion.offset = 0;
                }
                companion.identity = identity;
                if let Some(text) = read_appended(&companion.log_file, &mut companion.offset) {
                    let _ = self.watches.append_discovered(tracked.watch, text);
                }
            }
            let alive = self.process.is_alive(pid);
            if let Some(code) = terminal_code {
                let _ = self.watches.finish_discovered(tracked.watch, Some(code));
                self.lock().environments.remove(&pid);
            } else if !alive {
                let _ = self.watches.finish_discovered(tracked.watch, None);
                self.lock().environments.remove(&pid);
            } else {
                self.lock().tracked.insert(pid, tracked);
            }
        }
    }

    pub(crate) async fn run(self, shutdown: CancellationToken, scan_every: Duration) {
        let mut poll = tokio::time::interval(LOG_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut elapsed = Duration::MAX;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = poll.tick() => {
                    if elapsed >= scan_every {
                        if let Err(error) = self.discover_once().await {
                            tracing::warn!(%error, "discovered watch scan failed");
                        }
                        elapsed = Duration::ZERO;
                    }
                    self.poll_once();
                    elapsed = elapsed.saturating_add(LOG_POLL);
                }
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DiscoveryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn enabled_patterns(rules: &[DiscoveredWatchRule]) -> Vec<Regex> {
    rules
        .iter()
        .filter(|rule| rule.enabled)
        .filter_map(|rule| Regex::new(&rule.pattern).ok())
        .collect()
}

fn resolve_ownership<'a>(
    candidate: &ProcessInfo,
    environment: &[(String, String)],
    sessions: &'a [Session],
    snapshot: &[ProcessInfo],
    config: &Config,
) -> Option<(&'a Session, &'a Terminal)> {
    if let Some(session_id) =
        env_value(environment, "FLEET_SESSION").and_then(|value| value.parse::<SessionId>().ok())
        && let Some(session) = sessions.iter().find(|session| session.id == session_id)
    {
        if let Some(terminal) = env_value(environment, "FLEET_TERMINAL_ID")
            .and_then(|value| value.parse::<TerminalId>().ok())
            .and_then(|terminal| session.terminals.iter().find(|entry| entry.id == terminal))
        {
            return Some((session, terminal));
        }
        return select_terminal(session, config).map(|terminal| (session, terminal));
    }
    terminal_ancestor(candidate, sessions, snapshot)
}

fn select_terminal<'a>(session: &'a Session, config: &Config) -> Option<&'a Terminal> {
    let agent = config.agent_commands.command(config.agent);
    session
        .terminals
        .iter()
        .find(|terminal| same_program(&terminal.command, agent))
        .or_else(|| session.terminals.first())
}

fn terminal_ancestor<'a>(
    candidate: &ProcessInfo,
    sessions: &'a [Session],
    snapshot: &[ProcessInfo],
) -> Option<(&'a Session, &'a Terminal)> {
    let parents = snapshot
        .iter()
        .map(|process| (process.pid, process.parent_pid))
        .collect::<BTreeMap<_, _>>();
    let mut pid = candidate.parent_pid;
    while pid != 0 {
        for session in sessions {
            if let Some(terminal) = session
                .terminals
                .iter()
                .find(|terminal| terminal.shell_pid == Some(pid))
            {
                return Some((session, terminal));
            }
        }
        let Some(parent) = parents.get(&pid) else {
            break;
        };
        if *parent == pid {
            break;
        }
        pid = *parent;
    }
    None
}

fn has_matching_ancestor(pid: u32, candidates: &BTreeSet<u32>, snapshot: &[ProcessInfo]) -> bool {
    let parents = snapshot
        .iter()
        .map(|process| (process.pid, process.parent_pid))
        .collect::<BTreeMap<_, _>>();
    let mut current = pid;
    while let Some(parent) = parents.get(&current) {
        if candidates.contains(parent) {
            return true;
        }
        if *parent == 0 || *parent == current {
            break;
        }
        current = *parent;
    }
    false
}

fn is_primary_agent(process: &ProcessInfo, sessions: &[Session]) -> bool {
    // The foreground program launched directly by a terminal's PTY shell is that
    // terminal itself (for example `cc` resolving to `claude`), never a subagent.
    sessions.iter().any(|session| {
        session
            .terminals
            .iter()
            .any(|terminal| terminal.shell_pid == Some(process.parent_pid))
    })
}

fn is_wrapper_shell(command: &str) -> bool {
    matches!(
        command_basename(command).as_str(),
        "sh" | "bash" | "zsh" | "fish"
    )
}

fn same_program(left: &str, right: &str) -> bool {
    command_basename(left) == command_basename(right)
}

fn command_basename(command: &str) -> String {
    command_tokens(command)
        .first()
        .and_then(|program| Path::new(program).file_name())
        .and_then(|program| program.to_str())
        .unwrap_or("agent")
        .to_owned()
}

fn env_value<'a>(environment: &'a [(String, String)], key: &str) -> Option<&'a str> {
    environment
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn companion_from_command(
    command: &str,
    environment: &[(String, String)],
    daemon_temp: &Path,
) -> Option<Companion> {
    if !command.contains("codex-companion.mjs task-worker") {
        return None;
    }
    let workspace = argument(command, "--cwd")?;
    let job_id = argument(command, "--job-id")?;
    let state_root = env_value(environment, "CLAUDE_PLUGIN_DATA")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("state"))
        .unwrap_or_else(|| {
            env_value(environment, "TMPDIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| daemon_temp.to_path_buf())
                .join("codex-companion")
        });
    let state_dir = state_root.join(workspace_state_name(Path::new(&workspace)));
    let jobs = state_dir.join("jobs");
    Some(Companion {
        json_file: jobs.join(format!("{job_id}.json")),
        log_file: jobs.join(format!("{job_id}.log")),
        job_id,
        offset: 0,
        identity: None,
    })
}

fn companion_workspace(command: &str, _companion: &Companion) -> Option<PathBuf> {
    argument(command, "--cwd").map(PathBuf::from)
}

fn argument(command: &str, flag: &str) -> Option<String> {
    let tokens = command_tokens(command);
    let mut tokens = tokens.iter().map(String::as_str);
    while let Some(token) = tokens.next() {
        if token == flag {
            return tokens.next().map(ToOwned::to_owned);
        }
        if let Some(value) = token.strip_prefix(&format!("{flag}=")) {
            return Some(value.to_owned());
        }
    }
    None
}

fn command_tokens(command: &str) -> Vec<String> {
    shell_words::split(command)
        .unwrap_or_else(|_| command.split_whitespace().map(ToOwned::to_owned).collect())
}

fn workspace_state_name(workspace: &Path) -> String {
    let canonical = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let slug_source = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    let mut slug = String::new();
    let mut replacing = false;
    for character in slug_source.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            slug.push(character);
            replacing = false;
        } else if !replacing {
            slug.push('-');
            replacing = true;
        }
    }
    let slug = slug.trim_matches('-').to_owned();
    let slug = if slug.is_empty() { "workspace" } else { &slug };
    let hash = Sha256::digest(canonical.to_string_lossy().as_bytes());
    format!("{slug}-{}", hex_prefix(&hash, 16))
}

fn hex_prefix(bytes: &[u8], digits: usize) -> String {
    bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .take(digits)
        .map(|nibble| char::from_digit(u32::from(nibble), 16).unwrap_or('0'))
        .collect()
}

fn read_job(path: &Path) -> Option<CompanionJob> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn companion_label(job_id: &str, job: Option<&CompanionJob>) -> String {
    let short_id = job_id.rsplit_once('-').map_or(job_id, |(prefix, _)| prefix);
    if let Some(kind) = job
        .map(|job| job.kind_label.trim())
        .filter(|value| !value.is_empty())
    {
        format!("codex {kind} {short_id}")
    } else if let Some(title) = job
        .map(|job| job.title.trim())
        .filter(|value| !value.is_empty())
    {
        title.to_owned()
    } else {
        format!("codex {job_id}")
    }
}

fn terminal_job_code(job: &CompanionJob) -> Option<i32> {
    match job.status.as_str() {
        "done" | "completed" | "succeeded" => Some(0),
        "failed" | "error" => Some(1),
        _ => match job.phase.as_str() {
            "done" => Some(0),
            "failed" => Some(1),
            _ => None,
        },
    }
}

fn initial_offset(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len().saturating_sub(INITIAL_LOG_BYTES))
        .unwrap_or(0)
}

#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn read_appended(path: &Path, offset: &mut u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length < *offset {
        *offset = 0;
    }
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    *offset = length;
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapters::{clock::SystemClock, files::RealFiles},
        stores::state::StateStore,
        testing::fakes::FakeProcess,
    };
    use fleet_core::{
        config::default_config,
        sessions::{SessionKind, TerminalKind, TerminalStatus},
        watches::WatchStream,
    };
    use std::{fs::OpenOptions, io::Write};
    use tempfile::TempDir;

    struct Harness {
        _temp: TempDir,
        discovery: WatchDiscovery,
        process: Arc<FakeProcess>,
        watches: Watches,
        config: Config,
        session: Session,
    }

    async fn harness() -> Harness {
        let temp = tempfile::tempdir().unwrap();
        let files = Arc::new(RealFiles::new(
            temp.path().join("trash"),
            [temp.path().join("repos"), temp.path().join("worktrees")],
        ));
        let config_store = Arc::new(ConfigStore::new(temp.path(), files.clone()));
        let state = Arc::new(StateStore::new(temp.path(), files, Arc::new(SystemClock)));
        let sessions = Sessions::new(config_store.clone(), state);
        let process = Arc::new(FakeProcess::default());
        let watches = Watches::default();
        let discovery =
            WatchDiscovery::new(config_store, sessions, process.clone(), watches.clone())
                .with_temp_dir(temp.path().to_path_buf());
        let session = session(temp.path());
        Harness {
            config: default_config(temp.path()),
            _temp: temp,
            discovery,
            process,
            watches,
            session,
        }
    }

    fn session(root: &Path) -> Session {
        Session {
            id: "repo/main".parse().unwrap(),
            kind: SessionKind::Worktree("owner/repo#main".parse().unwrap()),
            cwd: root.to_string_lossy().into_owned(),
            terminals: vec![
                terminal(1, "nvim", "nvim .", 100, root),
                terminal(2, "cc", "claude", 200, root),
            ],
            active_terminal: Some(TerminalId(2)),
            slept_at: None,
            kept_terminals: Vec::new(),
        }
    }

    fn terminal(id: u64, name: &str, command: &str, shell_pid: u32, root: &Path) -> Terminal {
        Terminal {
            id: TerminalId(id),
            name: name.into(),
            command: command.into(),
            cwd: root.to_string_lossy().into_owned(),
            shell_pid: Some(shell_pid),
            foreground_command: None,
            status: TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
            kind: TerminalKind::Pty,
        }
    }

    fn process(pid: u32, parent_pid: u32, command: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid,
            command: command.into(),
        }
    }

    fn fleet_environment(terminal: Option<u64>) -> Vec<(String, String)> {
        let mut environment = vec![("FLEET_SESSION".into(), "repo/main".into())];
        if let Some(terminal) = terminal {
            environment.push(("FLEET_TERMINAL_ID".into(), terminal.to_string()));
        }
        environment
    }

    #[tokio::test]
    async fn env_tags_select_terminal_and_candidates_are_filtered_and_collapsed() {
        let h = harness().await;
        h.process.set_snapshot(vec![
            process(201, 200, "claude"),
            process(300, 1, "node codex-companion.mjs status --json"),
            process(301, 1, "node app-server-broker.mjs serve codex"),
            process(302, 1, "/bin/zsh -c claude"),
            process(400, 1, "/usr/bin/codex exec parent"),
            process(401, 400, "/usr/bin/codex exec child"),
        ]);
        h.process.set_environment(400, fleet_environment(Some(1)));
        h.process.set_environment(401, fleet_environment(Some(1)));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();

        let watches = h.watches.list(&h.session.id);
        assert_eq!(watches.len(), 1);
        assert_eq!(watches[0].pid, Some(400));
        assert_eq!(watches[0].terminal, TerminalId(1));
        assert_eq!(watches[0].source, WatchSource::Discovered);
        assert!(
            h.watches.tail(watches[0].id, None).unwrap().chunks[0]
                .text
                .contains("output is not captured")
        );
        assert_eq!(h.process.environment_calls(), vec![400]);
    }

    #[tokio::test]
    async fn descendant_fallback_and_session_only_tags_choose_the_right_terminals() {
        let h = harness().await;
        h.process.set_snapshot(vec![
            process(150, 100, "/bin/zsh -c codex"),
            process(500, 150, "/usr/bin/codex exec fallback"),
            process(501, 1, "/usr/bin/opencode run detached"),
        ]);
        h.process.set_environment(501, fleet_environment(None));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();

        let watches = h.watches.list(&h.session.id);
        assert_eq!(watches.len(), 2);
        assert_eq!(watches[0].terminal, TerminalId(1));
        assert_eq!(watches[1].terminal, TerminalId(2));
    }

    #[tokio::test]
    async fn cooperative_pid_dedupes_and_discovered_watches_reject_client_mutation() {
        let h = harness().await;
        let owner = h.watches.owner();
        h.watches.start(
            owner.id,
            Watch {
                id: WatchId(0),
                session: h.session.id.clone(),
                terminal: TerminalId(2),
                label: "cooperative".into(),
                command: vec!["codex".into()],
                cwd: None,
                pid: Some(600),
                started_at: chrono::Utc::now().to_rfc3339(),
                status: WatchStatus::Running,
                source: WatchSource::Cooperative,
                log_file: None,
            },
        );
        h.process
            .set_snapshot(vec![process(600, 1, "/usr/bin/codex exec duplicate")]);
        h.process.set_environment(600, fleet_environment(Some(2)));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(h.watches.list(&h.session.id).len(), 1);

        h.process
            .set_snapshot(vec![process(601, 1, "/usr/bin/codex exec discovered")]);
        h.process.set_environment(601, fleet_environment(Some(2)));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();
        let discovered = h.watches.watch_for_pid(601).unwrap();
        assert!(
            h.watches
                .append_owned(
                    discovered.id,
                    owner.id,
                    WatchStream::Stdout,
                    "client".into()
                )
                .is_err()
        );
        assert!(
            h.watches
                .finish_owned(discovered.id, owner.id, Some(0), None)
                .is_err()
        );
        drop(owner);
        assert_eq!(
            h.watches.watch_for_pid(601).unwrap().status,
            WatchStatus::Running
        );
    }

    #[tokio::test]
    async fn process_liveness_marks_a_discovered_watch_exited_without_a_signal() {
        let h = harness().await;
        h.process
            .set_snapshot(vec![process(700, 1, "/usr/bin/codex exec alive")]);
        h.process.set_environment(700, fleet_environment(Some(2)));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();
        h.process.set_alive(700, false);
        h.discovery.poll_once();
        assert_eq!(
            h.watches.watch_for_pid(700).unwrap().status,
            WatchStatus::Exited {
                code: None,
                signal: None
            }
        );
    }

    #[tokio::test]
    async fn companion_tails_recent_log_appends_and_truncation_then_maps_job_status() {
        let h = harness().await;
        let workspace = h._temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let job_id = "task-mtp0uppa-8bbzg9";
        let jobs = h
            ._temp
            .path()
            .join("codex-companion")
            .join(workspace_state_name(&workspace))
            .join("jobs");
        std::fs::create_dir_all(&jobs).unwrap();
        let json_file = jobs.join(format!("{job_id}.json"));
        let log_file = jobs.join(format!("{job_id}.log"));
        let prefix = "x".repeat(70 * 1024);
        std::fs::write(&log_file, format!("{prefix}\nrecent\n")).unwrap();
        write_job(&json_file, &log_file, "running", "rescue");
        let command = format!(
            "node /plugin/codex-companion.mjs task-worker --cwd {} --job-id {job_id}",
            workspace.display()
        );
        h.process.set_snapshot(vec![process(800, 1, &command)]);
        let mut environment = fleet_environment(Some(2));
        environment.push((
            "TMPDIR".into(),
            h._temp.path().to_string_lossy().into_owned(),
        ));
        h.process.set_environment(800, environment);
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();
        h.discovery.poll_once();
        let watch = h.watches.watch_for_pid(800).unwrap();
        assert_eq!(watch.label, "codex rescue task-mtp0uppa");
        assert_eq!(watch.log_file.as_deref(), Some(log_file.as_path()));
        let first = h.watches.tail(watch.id, None).unwrap();
        assert!(first.chunks[0].text.len() <= INITIAL_LOG_BYTES as usize);
        assert!(first.chunks[0].text.ends_with("recent\n"));

        OpenOptions::new()
            .append(true)
            .open(&log_file)
            .unwrap()
            .write_all(b"append\n")
            .unwrap();
        h.discovery.poll_once();
        assert!(
            h.watches
                .tail(watch.id, None)
                .unwrap()
                .chunks
                .iter()
                .any(|chunk| chunk.text == "append\n")
        );

        std::fs::write(&log_file, "after truncate\n").unwrap();
        h.discovery.poll_once();
        assert!(
            h.watches
                .tail(watch.id, None)
                .unwrap()
                .chunks
                .iter()
                .any(|chunk| chunk.text == "after truncate\n")
        );

        std::fs::rename(&log_file, jobs.join("rotated.log")).unwrap();
        std::fs::write(&log_file, "after rotation\n").unwrap();
        h.discovery.poll_once();
        assert!(
            h.watches
                .tail(watch.id, None)
                .unwrap()
                .chunks
                .iter()
                .any(|chunk| chunk.text == "after rotation\n")
        );

        write_job(&json_file, &log_file, "failed", "rescue");
        h.discovery.poll_once();
        assert_eq!(
            h.watches.watch_for_pid(800).unwrap().status,
            WatchStatus::Exited {
                code: Some(1),
                signal: None
            }
        );
    }

    fn write_job(path: &Path, log: &Path, status: &str, kind_label: &str) {
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "id": "task-mtp0uppa-8bbzg9",
                "kindLabel": kind_label,
                "title": "Codex Task",
                "status": status,
                "phase": status,
                "logFile": log,
                "startedAt": "2026-09-05T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn done_and_failed_job_statuses_map_to_exit_codes() {
        for (status, code) in [("done", 0), ("failed", 1)] {
            assert_eq!(
                terminal_job_code(&CompanionJob {
                    status: status.into(),
                    ..CompanionJob::default()
                }),
                Some(code)
            );
        }
    }

    #[test]
    fn companion_prefers_plugin_data_and_parses_a_quoted_workspace() {
        let companion = companion_from_command(
            "node codex-companion.mjs task-worker --cwd '/tmp/with space' --job-id task-1-x",
            &[("CLAUDE_PLUGIN_DATA".into(), "/plugin/data".into())],
            Path::new("/daemon/tmp"),
        )
        .unwrap();
        assert!(companion.json_file.starts_with("/plugin/data/state"));
        assert!(companion.json_file.ends_with("jobs/task-1-x.json"));
    }
}
