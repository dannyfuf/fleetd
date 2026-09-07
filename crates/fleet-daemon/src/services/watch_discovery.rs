//! Cheap, read-only discovery and log tailing for agent subprocesses.

use crate::{
    DaemonError, DaemonResult,
    adapters::process::{Process, ProcessIdentity, ProcessInfo},
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

use super::{maintenance, sessions::Sessions, watches::Watches};

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
    environments: BTreeMap<ProcessIdentity, Vec<(String, String)>>,
    rules: Vec<DiscoveredWatchRule>,
    patterns: Arc<[Regex]>,
    tracked: BTreeMap<ProcessIdentity, Tracked>,
    retained: BTreeMap<ProcessIdentity, WatchId>,
}

struct Tracked {
    watch: WatchId,
    session: SessionId,
    terminal: TerminalId,
    companion: Option<Companion>,
    environment_loaded: bool,
}

struct Companion {
    job_id: String,
    json_file: PathBuf,
    log_file: PathBuf,
    offset: u64,
    pending_utf8: Vec<u8>,
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
    sessions: Sessions,
    process: Arc<dyn Process>,
    watches: Watches,
    runtime_config: tokio::sync::watch::Receiver<Option<Arc<Config>>>,
    temp_dir: PathBuf,
    state: Arc<Mutex<DiscoveryState>>,
    helpers: Arc<[Regex]>,
}

impl WatchDiscovery {
    pub(crate) fn new(
        config: Arc<ConfigStore>,
        sessions: Sessions,
        process: Arc<dyn Process>,
        watches: Watches,
    ) -> Self {
        let runtime_config = maintenance::runtime_config_receiver(&config);
        Self {
            sessions,
            process,
            watches,
            runtime_config,
            temp_dir: std::env::temp_dir(),
            state: Arc::new(Mutex::new(DiscoveryState::default())),
            helpers: HELPER_PATTERNS
                .iter()
                .filter_map(|pattern| Regex::new(pattern).ok())
                .collect(),
        }
    }

    #[cfg(test)]
    fn with_temp_dir(mut self, temp_dir: PathBuf) -> Self {
        self.temp_dir = temp_dir;
        self
    }

    async fn discover_config(&self, config: &Config) -> DaemonResult<()> {
        if !config.discovered_watches.enabled {
            return Ok(());
        }
        let sessions = self.sessions.snapshot();
        if sessions.is_empty() {
            return Ok(());
        }
        let snapshot = self.process.snapshot().await?;
        self.discover(config, &sessions, &snapshot).await
    }

    async fn discover(
        &self,
        config: &Config,
        sessions: &[Session],
        snapshot: &[ProcessInfo],
    ) -> DaemonResult<()> {
        let live_identities = snapshot
            .iter()
            .map(ProcessInfo::identity)
            .collect::<BTreeSet<_>>();
        self.lock()
            .environments
            .retain(|identity, _| live_identities.contains(identity));
        self.reconcile_tracked(&live_identities);
        let patterns = {
            let mut state = self.lock();
            if state.rules != config.discovered_watches.processes {
                state.patterns = enabled_patterns(&config.discovered_watches.processes).into();
                state.rules.clone_from(&config.discovered_watches.processes);
            }
            Arc::clone(&state.patterns)
        };
        let parents = snapshot
            .iter()
            .map(|process| (process.pid, process.parent_pid))
            .collect::<BTreeMap<_, _>>();
        let candidates = snapshot
            .iter()
            .filter(|process| {
                patterns
                    .iter()
                    .any(|pattern| pattern.is_match(&process.command))
            })
            .filter(|process| {
                !self
                    .helpers
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
            if has_matching_ancestor(candidate.pid, &candidate_pids, &parents) {
                continue;
            }
            let identity = candidate.identity();
            self.release_reused_retained(&identity);
            if self.retained_watch_is_current(&identity) {
                continue;
            }
            let mut retry = self.lock().tracked.get(&identity).map(|tracked| {
                (
                    tracked.watch,
                    tracked.session.clone(),
                    tracked.terminal,
                    tracked.environment_loaded,
                )
            });
            if retry.as_ref().is_some_and(|(_, _, _, loaded)| *loaded) {
                continue;
            }
            if retry.is_none() && self.watches.watch_for_pid(candidate.pid).is_some() {
                continue;
            }
            let (environment, environment_loaded) = self.environment(&candidate).await;
            let ownership = resolve_ownership(&candidate, &environment, sessions, &parents, config);
            let Some((session, terminal)) = ownership else {
                if environment_loaded && let Some((watch, _, _, _)) = retry {
                    self.watches.replace_discovered(watch)?;
                    self.lock().tracked.remove(&identity);
                }
                continue;
            };
            if let Some((watch, tracked_session, tracked_terminal, _)) = &retry
                && (*tracked_session != session.id || *tracked_terminal != terminal.id)
            {
                self.watches.replace_discovered(*watch)?;
                self.lock().tracked.remove(&identity);
                retry = None;
            }
            let command = candidate.command.clone();
            let temp_dir = self.temp_dir.clone();
            let (companion, job) = tokio::task::spawn_blocking(move || {
                let mut companion = companion_from_command(&command, &environment, &temp_dir);
                let job = companion
                    .as_ref()
                    .and_then(|companion| read_job(&companion.json_file));
                if let Some(companion) = &mut companion {
                    if let Some(log_file) = job.as_ref().and_then(|job| job.log_file.as_ref()) {
                        companion.log_file = log_file.clone();
                    }
                    companion.offset = initial_offset(&companion.log_file);
                    companion.identity = file_identity(&companion.log_file);
                }
                (companion, job)
            })
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?;
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
            let owner_is_live = self.sessions.snapshot().iter().any(|live_session| {
                live_session.id == session.id
                    && live_session
                        .terminals
                        .iter()
                        .any(|live_terminal| live_terminal.id == terminal.id)
            });
            if !owner_is_live {
                continue;
            }
            if let Some((watch_id, _, _, _)) = retry {
                self.watches
                    .update_discovered(watch_id, watch.label, watch.log_file)?;
                if let Some(tracked) = self.lock().tracked.get_mut(&identity) {
                    tracked.companion = companion;
                    tracked.environment_loaded = environment_loaded;
                }
                continue;
            }
            let Some(watch_id) = self.watches.start_discovered(watch, initial_output) else {
                continue;
            };
            self.lock().tracked.insert(
                identity,
                Tracked {
                    watch: watch_id,
                    session: session.id.clone(),
                    terminal: terminal.id,
                    companion,
                    environment_loaded,
                },
            );
        }
        Ok(())
    }

    fn reconcile_tracked(&self, live_identities: &BTreeSet<ProcessIdentity>) {
        let live_pids = live_identities
            .iter()
            .map(|identity| identity.pid)
            .collect::<BTreeSet<_>>();
        let retired = {
            let mut state = self.lock();
            let stale = state
                .tracked
                .keys()
                .filter(|identity| !live_identities.contains(*identity))
                .cloned()
                .collect::<Vec<_>>();
            stale
                .into_iter()
                .filter_map(|identity| {
                    state
                        .tracked
                        .remove(&identity)
                        .map(|tracked| (identity, tracked))
                })
                .collect::<Vec<_>>()
        };
        for (identity, tracked) in retired {
            if live_pids.contains(&identity.pid) {
                let _ = self.watches.replace_discovered(tracked.watch);
                continue;
            }
            if self.watches.finish_discovered(tracked.watch, None).is_err() {
                continue;
            }
            self.lock().retained.insert(identity, tracked.watch);
        }
    }

    fn release_reused_retained(&self, current: &ProcessIdentity) {
        let stale = {
            let mut state = self.lock();
            let identities = state
                .retained
                .keys()
                .filter(|identity| identity.pid == current.pid && *identity != current)
                .cloned()
                .collect::<Vec<_>>();
            identities
                .into_iter()
                .filter_map(|identity| state.retained.remove(&identity))
                .collect::<Vec<_>>()
        };
        for watch in stale {
            let _ = self.watches.dismiss(watch);
        }
    }

    fn retained_watch_is_current(&self, identity: &ProcessIdentity) -> bool {
        let Some(watch) = self.lock().retained.get(identity).copied() else {
            return false;
        };
        if self
            .watches
            .watch_for_pid(identity.pid)
            .is_some_and(|current| current.id == watch)
        {
            return true;
        }
        self.lock().retained.remove(identity);
        false
    }

    async fn environment(&self, process: &ProcessInfo) -> (Vec<(String, String)>, bool) {
        let identity = process.identity();
        if let Some(environment) = self.lock().environments.get(&identity).cloned() {
            return (environment, true);
        }
        let Ok(environment) = self.process.environment(process.pid).await else {
            return (Vec::new(), false);
        };
        self.lock()
            .environments
            .insert(identity, environment.clone());
        (environment, true)
    }

    pub(crate) async fn poll_once(&self) -> DaemonResult<()> {
        let snapshot = self.process.snapshot().await?;
        let discovery = self.clone();
        tokio::task::spawn_blocking(move || discovery.poll_snapshot(&snapshot))
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?;
        Ok(())
    }

    fn poll_snapshot(&self, snapshot: &[ProcessInfo]) {
        let live_identities = snapshot
            .iter()
            .map(ProcessInfo::identity)
            .collect::<BTreeSet<_>>();
        let identities = self.lock().tracked.keys().cloned().collect::<Vec<_>>();
        for identity in identities {
            let mut tracked = match self.lock().tracked.remove(&identity) {
                Some(tracked) => tracked,
                None => continue,
            };
            let mut terminal_code = None;
            let alive = live_identities.contains(&identity);
            let mut has_more = false;
            if let Some(companion) = &mut tracked.companion {
                let job = read_job(&companion.json_file);
                if let Some(job) = &job {
                    if let Some(log_file) = &job.log_file
                        && *log_file != companion.log_file
                    {
                        companion.log_file = log_file.clone();
                        companion.offset = initial_offset(log_file);
                        companion.pending_utf8.clear();
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
                    companion.pending_utf8.clear();
                }
                companion.identity = identity;
                if let Some((text, remaining)) = read_appended(
                    &companion.log_file,
                    &mut companion.offset,
                    &mut companion.pending_utf8,
                    terminal_code.is_some() || !alive,
                ) {
                    has_more = remaining;
                    if !text.is_empty() {
                        let _ = self.watches.append_discovered(tracked.watch, text);
                    }
                }
            }
            if has_more {
                self.lock().tracked.insert(identity, tracked);
                continue;
            }
            if let Some(code) = terminal_code {
                if self
                    .watches
                    .finish_discovered(tracked.watch, Some(code))
                    .is_ok()
                {
                    self.lock().retained.insert(identity.clone(), tracked.watch);
                }
                self.lock().environments.remove(&identity);
            } else if !alive {
                if self.watches.finish_discovered(tracked.watch, None).is_ok() {
                    self.lock().retained.insert(identity.clone(), tracked.watch);
                }
                self.lock().environments.remove(&identity);
            } else {
                self.lock().tracked.insert(identity, tracked);
            }
        }
    }

    pub(crate) async fn run(self, shutdown: CancellationToken) {
        let mut poll = tokio::time::interval(LOG_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut policy = self.runtime_config.clone();
        let mut next_scan = tokio::time::Instant::now();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                changed = policy.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    next_scan = tokio::time::Instant::now();
                }
                _ = poll.tick() => {
                    let config = policy.borrow().clone();
                    if let Some(config) = config
                        && tokio::time::Instant::now() >= next_scan
                    {
                        let scan_every = Duration::from_millis(
                            config.discovered_watches.interval_ms.max(500),
                        );
                        if let Err(error) = self.discover_config(&config).await {
                            tracing::warn!(%error, "discovered watch scan failed");
                        }
                        next_scan = tokio::time::Instant::now() + scan_every;
                    }
                    if let Err(error) = self.poll_once().await {
                        tracing::warn!(%error, "discovered watch log polling failed");
                    }
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
    parents: &BTreeMap<u32, u32>,
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
    terminal_ancestor(candidate, sessions, parents)
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
    parents: &BTreeMap<u32, u32>,
) -> Option<(&'a Session, &'a Terminal)> {
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

fn has_matching_ancestor(
    pid: u32,
    candidates: &BTreeSet<u32>,
    parents: &BTreeMap<u32, u32>,
) -> bool {
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
        pending_utf8: Vec::new(),
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
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(INITIAL_LOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > INITIAL_LOG_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn companion_label(job_id: &str, job: Option<&CompanionJob>) -> String {
    let short_id = job_id.rsplit_once('-').map_or(job_id, |(prefix, _)| prefix);
    if !short_id.contains('-')
        && let Some(job) = job
        && !job.kind_label.trim().is_empty()
        && !job.title.trim().is_empty()
    {
        return format!("{} · {}", job.kind_label.trim(), job.title.trim());
    }
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

fn read_appended(
    path: &Path,
    offset: &mut u64,
    pending: &mut Vec<u8>,
    finished: bool,
) -> Option<(String, bool)> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length < *offset {
        *offset = 0;
        pending.clear();
    }
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut bytes = Vec::new();
    file.take(INITIAL_LOG_BYTES).read_to_end(&mut bytes).ok()?;
    *offset = offset.saturating_add(bytes.len() as u64);
    let has_more = *offset < length;
    pending.extend_from_slice(&bytes);
    let mut consumed = pending.len();
    let mut rest = pending.as_slice();
    while let Err(error) = std::str::from_utf8(rest) {
        match error.error_len() {
            Some(invalid) => rest = &rest[error.valid_up_to() + invalid..],
            None => {
                if !finished || has_more {
                    consumed -= rest.len() - error.valid_up_to();
                }
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&pending[..consumed]).into_owned();
    pending.drain(..consumed);
    Some((text, has_more))
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
        config::{NATIVE_LAZYGIT, WindowConfig, default_config},
        ids::{ContextId, RepoId, WorktreeId},
        model::{Context, Repo, RepoHooks, Worktree},
        state::default_state,
        watches::WatchStream,
    };
    use std::{fs::OpenOptions, io::Write};
    use tempfile::TempDir;

    #[test]
    fn log_reads_are_bounded_and_keep_split_utf8_until_completed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("output.log");
        let prefix = "x".repeat(INITIAL_LOG_BYTES as usize - 1);
        std::fs::write(&path, format!("{prefix}€tail")).unwrap();
        let mut offset = 0;
        let mut pending = Vec::new();
        let (first, more) = read_appended(&path, &mut offset, &mut pending, true).unwrap();
        assert_eq!(first, prefix);
        assert!(more);
        assert_eq!(offset, INITIAL_LOG_BYTES);
        let (second, more) = read_appended(&path, &mut offset, &mut pending, true).unwrap();
        assert_eq!(second, "€tail");
        assert!(!more);
        assert!(pending.is_empty());
        let (empty, _) = read_appended(&path, &mut offset, &mut pending, false).unwrap();
        assert!(empty.is_empty());
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0xe2])
            .unwrap();
        assert!(
            read_appended(&path, &mut offset, &mut pending, false)
                .unwrap()
                .0
                .is_empty()
        );
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[0x82, 0xac])
            .unwrap();
        assert_eq!(
            read_appended(&path, &mut offset, &mut pending, false)
                .unwrap()
                .0,
            "€"
        );
    }

    struct Harness {
        _temp: TempDir,
        config_store: Arc<ConfigStore>,
        discovery: WatchDiscovery,
        process: Arc<FakeProcess>,
        watches: Watches,
        config: Config,
        sessions: Sessions,
        session: Session,
    }

    async fn harness() -> Harness {
        let temp = tempfile::tempdir().unwrap();
        let files = Arc::new(RealFiles::new(
            temp.path().join("trash"),
            [temp.path().join("repos"), temp.path().join("worktrees")],
        ));
        let config_store = Arc::new(ConfigStore::new(temp.path(), files.clone()));
        let mut config = default_config(temp.path());
        config.windows = vec![
            WindowConfig {
                name: "nvim".into(),
                command: NATIVE_LAZYGIT.into(),
            },
            WindowConfig {
                name: "cc".into(),
                command: NATIVE_LAZYGIT.into(),
            },
        ];
        config_store.save(config.clone()).await.unwrap();
        let state = Arc::new(StateStore::new(temp.path(), files, Arc::new(SystemClock)));
        let context = ContextId::try_from("team").unwrap();
        let repo = RepoId::try_from("owner/repo").unwrap();
        let worktree = WorktreeId::try_from("owner/repo#main").unwrap();
        let mut persisted = default_state();
        persisted.contexts.push(Context {
            id: context.clone(),
            name: "Team".into(),
            owners: vec!["owner".into()],
            created_at: "2026-09-06T00:00:00Z".into(),
        });
        persisted.repos.push(Repo {
            id: repo.clone(),
            owner: "owner".into(),
            name: "repo".into(),
            url: "https://example.invalid/owner/repo".into(),
            context_id: context,
            default_branch: "main".into(),
            path: temp.path().join("repos/owner/repo").display().to_string(),
            cloned_at: "2026-09-06T00:00:00Z".into(),
            hooks: RepoHooks::default(),
        });
        persisted.worktrees.push(Worktree {
            id: worktree.clone(),
            repo_id: repo,
            slug: "main".into(),
            branch: "main".into(),
            base_ref: "origin/main".into(),
            path: temp
                .path()
                .join("worktrees/owner/repo/main")
                .display()
                .to_string(),
            session: "repo/main".into(),
            host: None,
            created_at: "2026-09-06T00:00:00Z".into(),
            last_opened_at: None,
            degraded: None,
        });
        state.save(persisted).await.unwrap();
        let sessions = Sessions::new(config_store.clone(), state);
        let mut session = sessions.ensure(Some(worktree), None, false).await.unwrap();
        session.terminals[0].command = "nvim .".into();
        session.terminals[0].shell_pid = Some(100);
        session.terminals[1].command = "claude".into();
        session.terminals[1].shell_pid = Some(200);
        let process = Arc::new(FakeProcess::default());
        let watches = Watches::default();
        let discovery = WatchDiscovery::new(
            config_store.clone(),
            sessions.clone(),
            process.clone(),
            watches.clone(),
        )
        .with_temp_dir(temp.path().to_path_buf());
        Harness {
            config,
            _temp: temp,
            config_store,
            discovery,
            process,
            watches,
            sessions,
            session,
        }
    }

    struct FailingEnvironmentProcess {
        inner: Arc<FakeProcess>,
        failures: Mutex<BTreeSet<u32>>,
        calls: Mutex<Vec<u32>>,
    }

    impl FailingEnvironmentProcess {
        fn new(inner: Arc<FakeProcess>) -> Self {
            Self {
                inner,
                failures: Mutex::new(BTreeSet::new()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn fail_once(&self, pid: u32) {
            self.failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(pid);
        }

        fn environment_calls(&self) -> Vec<u32> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl Process for FailingEnvironmentProcess {
        async fn snapshot(&self) -> DaemonResult<Vec<ProcessInfo>> {
            self.inner.snapshot().await
        }

        async fn listening_ports(
            &self,
            pids: &[u32],
        ) -> DaemonResult<Vec<crate::adapters::process::ListeningPort>> {
            self.inner.listening_ports(pids).await
        }

        async fn environment(&self, pid: u32) -> DaemonResult<Vec<(String, String)>> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(pid);
            if self
                .failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&pid)
            {
                return Err(DaemonError::Process(
                    "temporary environment read failure".to_owned(),
                ));
            }
            self.inner.environment(pid).await
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.inner.is_alive(pid)
        }
    }

    fn process(pid: u32, parent_pid: u32, command: &str) -> ProcessInfo {
        process_at(pid, parent_pid, command, &format!("start-{pid}"))
    }

    fn process_at(pid: u32, parent_pid: u32, command: &str, start_identity: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid,
            process_group_id: pid,
            terminal_foreground_process_group_id: Some(pid),
            start_identity: start_identity.to_owned(),
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
    async fn closed_terminal_cannot_gain_watch() {
        let h = harness().await;
        h.process
            .set_snapshot(vec![process(450, 1, "/usr/bin/codex exec stale")]);
        h.process.set_environment(450, fleet_environment(Some(2)));
        let process_snapshot = h.process.snapshot().await.unwrap();
        h.sessions.close_terminal(TerminalId(2)).await.unwrap();

        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &process_snapshot,
            )
            .await
            .unwrap();

        assert!(h.watches.watch_for_pid(450).is_none());
    }

    #[tokio::test]
    async fn pid_reuse_does_not_reuse_environment() {
        let h = harness().await;
        h.process.set_snapshot(vec![process_at(
            451,
            1,
            "/usr/bin/codex exec reused",
            "first-start",
        )]);
        h.process
            .set_environment(451, vec![("FLEET_SESSION".into(), "repo/missing".into())]);
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();
        assert!(h.watches.watch_for_pid(451).is_none());

        h.process.set_snapshot(vec![process_at(
            451,
            1,
            "/usr/bin/codex exec reused",
            "second-start",
        )]);
        h.process.set_environment(451, fleet_environment(Some(2)));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            h.watches.watch_for_pid(451).unwrap().terminal,
            TerminalId(2)
        );
        assert_eq!(h.process.environment_calls(), vec![451, 451]);
    }

    #[tokio::test]
    async fn pid_reuse_finishes_old_watch_and_creates_new_watch() {
        let h = harness().await;
        let events = crate::server::BroadcastBus::default();
        let mut receiver = events.subscribe();
        h.watches.with_events(events);
        h.process.set_snapshot(vec![process_at(
            452,
            1,
            "/usr/bin/codex exec first",
            "first-start",
        )]);
        h.process.set_environment(452, fleet_environment(Some(1)));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();
        let old = h.watches.watch_for_pid(452).unwrap();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            fleet_proto::event::Event::WatchStarted(watch) if watch.id == old.id
        ));

        h.process.set_snapshot(vec![process_at(
            452,
            1,
            "/usr/bin/codex exec second",
            "second-start",
        )]);
        h.process.set_environment(452, fleet_environment(Some(2)));
        h.discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();

        let new = h.watches.watch_for_pid(452).unwrap();
        assert_ne!(new.id, old.id);
        assert_eq!(new.terminal, TerminalId(2));
        let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| matches!(
            event,
            fleet_proto::event::Event::WatchExited(watch)
                if watch.id == old.id
                    && watch.status == (WatchStatus::Exited { code: None, signal: None })
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            fleet_proto::event::Event::WatchDismissed(id) if *id == old.id
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            fleet_proto::event::Event::WatchStarted(watch) if watch.id == new.id
        )));
    }

    #[tokio::test]
    async fn environment_failure_is_retried_and_repairs_companion() {
        let h = harness().await;
        let workspace = h._temp.path().join("retry-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let plugin_data = h._temp.path().join("plugin-data");
        let job_id = "task-retry";
        let jobs = plugin_data
            .join("state")
            .join(workspace_state_name(&workspace))
            .join("jobs");
        std::fs::create_dir_all(&jobs).unwrap();
        let json_file = jobs.join(format!("{job_id}.json"));
        let log_file = jobs.join(format!("{job_id}.log"));
        std::fs::write(&log_file, "correct log\n").unwrap();
        std::fs::write(
            &json_file,
            serde_json::to_vec(&serde_json::json!({
                "id": job_id,
                "kindLabel": "corrected",
                "title": "Retried Task",
                "status": "running",
                "phase": "running",
                "logFile": log_file,
                "startedAt": "2026-09-06T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
        let command = format!(
            "node /plugin/codex-companion.mjs task-worker --cwd {} --job-id {job_id}",
            workspace.display()
        );
        h.process.set_snapshot(vec![
            process(250, 200, "runner"),
            process_at(453, 250, &command, "retry-start"),
        ]);
        let mut environment = fleet_environment(Some(2));
        environment.push((
            "CLAUDE_PLUGIN_DATA".to_owned(),
            plugin_data.display().to_string(),
        ));
        h.process.set_environment(453, environment);
        let process = Arc::new(FailingEnvironmentProcess::new(Arc::clone(&h.process)));
        process.fail_once(453);
        let discovery = WatchDiscovery::new(
            Arc::clone(&h.config_store),
            h.sessions.clone(),
            process.clone(),
            h.watches.clone(),
        )
        .with_temp_dir(h._temp.path().to_path_buf());

        discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();
        let provisional = h.watches.watch_for_pid(453).unwrap();
        assert_ne!(provisional.log_file.as_deref(), Some(log_file.as_path()));

        discovery
            .discover(
                &h.config,
                std::slice::from_ref(&h.session),
                &h.process.snapshot().await.unwrap(),
            )
            .await
            .unwrap();

        let repaired = h.watches.watch_for_pid(453).unwrap();
        assert_eq!(repaired.id, provisional.id);
        assert_eq!(repaired.label, "corrected · Retried Task");
        assert_eq!(repaired.log_file.as_deref(), Some(log_file.as_path()));
        assert_eq!(process.environment_calls(), vec![453, 453]);
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
        h.process.set_snapshot(Vec::new());
        h.discovery.poll_once().await.unwrap();
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
        h.discovery.poll_once().await.unwrap();
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
        h.discovery.poll_once().await.unwrap();
        assert!(
            h.watches
                .tail(watch.id, None)
                .unwrap()
                .chunks
                .iter()
                .any(|chunk| chunk.text == "append\n")
        );

        std::fs::write(&log_file, "after truncate\n").unwrap();
        h.discovery.poll_once().await.unwrap();
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
        h.discovery.poll_once().await.unwrap();
        assert!(
            h.watches
                .tail(watch.id, None)
                .unwrap()
                .chunks
                .iter()
                .any(|chunk| chunk.text == "after rotation\n")
        );

        OpenOptions::new()
            .append(true)
            .open(&log_file)
            .unwrap()
            .write_all(&vec![b'x'; INITIAL_LOG_BYTES as usize + 10])
            .unwrap();
        write_job(&json_file, &log_file, "failed", "rescue");
        h.discovery.poll_once().await.unwrap();
        assert_eq!(
            h.watches.watch_for_pid(800).unwrap().status,
            WatchStatus::Running
        );
        h.discovery.poll_once().await.unwrap();
        assert!(
            h.watches
                .tail(watch.id, None)
                .unwrap()
                .chunks
                .iter()
                .any(|chunk| chunk.text == "x".repeat(10))
        );
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
