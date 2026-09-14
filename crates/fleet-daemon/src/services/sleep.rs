//! Session sleep policy, process observation, and editor shutdown handling.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::Arc,
    time::Duration,
};

use fleet_core::{
    agents::recognized_agent,
    ids::{SessionId, TerminalId, WorktreeId},
    sessions::{KeptTerminal, Session},
    sleep::{CompiledSleepPolicy, KeepAliveKind, KeepAliveRule},
};
use fleet_proto::response::{KeepAliveRuleMatch, SleepKept, SleepResult};

use crate::{
    DaemonError, DaemonResult,
    adapters::process::{ListeningPort, Process, ProcessInfo},
    error::remote_unsupported,
    services::maintenance::RepeatedFailure,
    services::sessions::{SessionRuntime, Sessions},
    stores::{config::ConfigStore, state::StateStore},
};

/// Session sleep-policy service.
#[derive(Clone)]
pub struct Sleep {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    process: Arc<dyn Process>,
    runtime: Arc<SessionRuntime>,
}

impl Sleep {
    /// Creates the sleep service over the terminal registry it observes.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        process: Arc<dyn Process>,
        sessions: &Sessions,
    ) -> Self {
        let runtime = Arc::clone(&sessions.runtime);
        runtime.register_process(Arc::clone(&process));
        Self {
            config,
            state,
            process,
            runtime,
        }
    }

    pub(super) async fn refresh_observations(&self) -> DaemonResult<()> {
        refresh_observations(&self.runtime, &self.config, self.process.as_ref()).await
    }

    /// Applies keep-alive rules and editor `:qa` grace handling to a session.
    pub async fn session(&self, session: SessionId) -> DaemonResult<SleepResult> {
        if self.runtime.session(&session).is_none() {
            let config = self.config.load().await?;
            if config
                .hosts
                .keys()
                .any(|host| session.as_str().starts_with(&format!("{host}/")))
            {
                return Err(remote_unsupported());
            }
        }
        apply_session(&self.runtime, &self.config, self.process.as_ref(), &session).await
    }

    /// Resolves a worktree session and applies the same sleep policy.
    pub async fn worktree(&self, worktree: WorktreeId) -> DaemonResult<SleepResult> {
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .iter()
            .find(|entry| entry.id == worktree)
            .ok_or_else(|| DaemonError::NotFound(worktree.to_string()))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        let session = SessionId::try_from(worktree.session.as_str())
            .map_err(|error| DaemonError::Validation(error.to_string()))?;
        self.session(session).await
    }

    /// Counts current process matches for configured keep-alive rules.
    pub async fn match_keep_alive_rules(&self) -> DaemonResult<Vec<KeepAliveRuleMatch>> {
        let (config, policy) = self.config.load_with_sleep_policy().await?;
        let snapshot = match self.process.snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Ok(config
                    .sleep
                    .keep_alive
                    .iter()
                    .map(|rule| KeepAliveRuleMatch {
                        rule_id: rule.id.clone(),
                        count: 0,
                        error: Some(error.to_string()),
                    })
                    .collect());
            }
        };
        let all_pids = snapshot.iter().map(|entry| entry.pid).collect::<Vec<_>>();
        let ports = self.process.listening_ports(&all_pids).await;
        Ok(config
            .sleep
            .keep_alive
            .iter()
            .enumerate()
            .map(|(index, rule)| match_rule_count(&policy, index, rule, &snapshot, ports.as_ref()))
            .collect())
    }
}

pub(crate) async fn apply_session(
    runtime: &Arc<SessionRuntime>,
    config_store: &ConfigStore,
    process: &dyn Process,
    session_id: &SessionId,
) -> DaemonResult<SleepResult> {
    let Some(session) = runtime.session(session_id) else {
        return Ok(empty_result());
    };
    let generation = SleepGeneration::capture(runtime, session.clone());
    let (config, policy) = config_store.load_with_sleep_policy().await?;
    if !config.sleep.enabled {
        let kept = session
            .terminals
            .iter()
            .map(|terminal| SleepKept {
                window: terminal.name.clone(),
                reason: "sleep disabled".to_owned(),
            })
            .collect::<Vec<_>>();
        runtime.record_sleep(session_id, core_kept(&kept));
        return Ok(SleepResult {
            kept,
            closed: Vec::new(),
            session_killed: false,
        });
    }

    let sessions = std::slice::from_ref(&session);
    let observations = observe_sessions(sessions, process, policy.ports_enabled()).await?;
    let keep_alive = publish_observations(runtime, &policy, sessions, &observations);

    let mut kept = Vec::new();
    let mut closable = Vec::new();
    for terminal in session.terminals.iter().rev() {
        if terminal.is_native() {
            // Client-drawn tabs have no process that can keep a session awake.
            closable.push((terminal.id, terminal.name.clone()));
            continue;
        }
        let observation = observations.get(&terminal.id);
        let labels = keep_alive.get(&terminal.id).map_or(&[][..], Vec::as_slice);
        if !labels.is_empty() {
            kept.push(SleepKept {
                window: terminal.name.clone(),
                reason: labels.join(", "),
            });
            continue;
        }

        if let Some(editor_pid) = observation.and_then(Observation::editor_pid) {
            if observation
                .and_then(Observation::foreground_editor_pid)
                .is_some()
                && let Some(host) = runtime.host(terminal.id)
            {
                host.write(vec![0x1b])
                    .map_err(|error| terminal_io_error(terminal.id, error))?;
                host.write(b":qa".to_vec())
                    .map_err(|error| terminal_io_error(terminal.id, error))?;
                host.write(vec![b'\r'])
                    .map_err(|error| terminal_io_error(terminal.id, error))?;
            }
            wait_for_exit(process, editor_pid, config.sleep.grace_ms).await;
            if process.is_alive(editor_pid) {
                kept.push(SleepKept {
                    window: terminal.name.clone(),
                    reason: "unsaved changes".to_owned(),
                });
                continue;
            }
        }
        closable.push((terminal.id, terminal.name.clone()));
    }

    let closable = generation.revalidate(runtime, &closable, &mut kept);
    let mut closed = Vec::new();
    for (terminal, _) in closable {
        if generation.terminal_is_unchanged(runtime, terminal)
            && let Some(name) = runtime.close_terminal_gated(terminal).await
        {
            closed.push(name);
        }
    }
    let session_killed = runtime.session(session_id).is_none();
    if !session_killed {
        runtime.record_sleep(session_id, core_kept(&kept));
    }
    Ok(SleepResult {
        kept,
        closed,
        session_killed,
    })
}

struct SleepGeneration {
    session: Session,
    output_bytes: HashMap<TerminalId, Option<u64>>,
}

impl SleepGeneration {
    fn capture(runtime: &SessionRuntime, session: Session) -> Self {
        let output_bytes = session
            .terminals
            .iter()
            .map(|terminal| {
                (
                    terminal.id,
                    runtime
                        .host(terminal.id)
                        .map(|host| host.activity().output_bytes_total),
                )
            })
            .collect();
        Self {
            session,
            output_bytes,
        }
    }

    fn revalidate(
        &self,
        runtime: &SessionRuntime,
        closable: &[(TerminalId, String)],
        kept: &mut Vec<SleepKept>,
    ) -> Vec<(TerminalId, String)> {
        let Some(current) = runtime.session(&self.session.id) else {
            return Vec::new();
        };
        let active_unchanged = current.active_terminal == self.session.active_terminal;
        let mut approved = Vec::new();
        for terminal in &current.terminals {
            let candidate = closable
                .iter()
                .find(|(candidate, _)| *candidate == terminal.id);
            let existed = self
                .session
                .terminals
                .iter()
                .any(|original| original.id == terminal.id);
            if let Some((id, name)) = candidate
                && active_unchanged
                && self.terminal_matches(runtime, terminal)
            {
                approved.push((*id, name.clone()));
            } else if (!existed || candidate.is_some())
                && !kept.iter().any(|entry| entry.window == terminal.name)
            {
                kept.push(SleepKept {
                    window: terminal.name.clone(),
                    reason: "activity changed during sleep".to_owned(),
                });
            }
        }
        approved
    }

    fn terminal_is_unchanged(&self, runtime: &SessionRuntime, terminal: TerminalId) -> bool {
        runtime
            .session(&self.session.id)
            .and_then(|session| {
                session
                    .terminals
                    .into_iter()
                    .find(|current| current.id == terminal)
            })
            .is_some_and(|current| self.terminal_matches(runtime, &current))
    }

    fn terminal_matches(
        &self,
        runtime: &SessionRuntime,
        current: &fleet_core::sessions::Terminal,
    ) -> bool {
        let Some(original) = self
            .session
            .terminals
            .iter()
            .find(|terminal| terminal.id == current.id)
        else {
            return false;
        };
        let mut expected = original.clone();
        expected
            .foreground_command
            .clone_from(&current.foreground_command);
        expected.keep_alive.clone_from(&current.keep_alive);
        let current_output = runtime
            .host(current.id)
            .map(|host| host.activity().output_bytes_total);
        expected == *current && self.output_bytes.get(&current.id).copied() == Some(current_output)
    }
}

#[derive(Default)]
struct Observation {
    processes: Vec<ProcessInfo>,
    commands: Vec<String>,
    ports: Vec<u16>,
    shell_pid: Option<u32>,
}

impl Observation {
    fn foreground_command(&self) -> Option<String> {
        let shell_pid = self.shell_pid?;
        self.processes
            .iter()
            .find(|entry| entry.parent_pid == shell_pid)
            .map(|entry| entry.command.clone())
    }

    fn editor_pid(&self) -> Option<u32> {
        self.processes
            .iter()
            .find(|entry| is_editor_command(&entry.command))
            .map(|entry| entry.pid)
    }

    fn foreground_editor_pid(&self) -> Option<u32> {
        self.processes
            .iter()
            .find(|entry| {
                is_editor_command(&entry.command)
                    && entry.terminal_foreground_process_group_id == Some(entry.process_group_id)
            })
            .map(|entry| entry.pid)
    }
}

/// Builds one observation per terminal: the shell's descendants, their command lines, and —
/// only when a keep-alive rule can consume them — the ports those processes listen on.
async fn observe_sessions(
    sessions: &[Session],
    process: &dyn Process,
    observe_ports: bool,
) -> DaemonResult<HashMap<TerminalId, Observation>> {
    let snapshot = process.snapshot().await?;
    let mut children = HashMap::<u32, Vec<usize>>::new();
    for (index, entry) in snapshot.iter().enumerate() {
        children.entry(entry.parent_pid).or_default().push(index);
    }
    let mut observations = HashMap::new();
    let mut all_pids = BTreeSet::new();
    for terminal in sessions.iter().flat_map(|session| &session.terminals) {
        let mut observation = Observation {
            shell_pid: terminal.shell_pid,
            ..Observation::default()
        };
        if let Some(shell_pid) = terminal.shell_pid {
            all_pids.insert(shell_pid);
            observation.processes = descendants_from_snapshot(&snapshot, &children, shell_pid);
            observation.commands = observation
                .processes
                .iter()
                .map(|entry| entry.command.clone())
                .collect();
            all_pids.extend(observation.processes.iter().map(|entry| entry.pid));
        }
        observations.insert(terminal.id, observation);
    }
    if !observe_ports {
        // No enabled rule matches on ports, so nothing can consume the answer and the whole
        // query — an `lsof` process, or a readlink over every descendant's file descriptors —
        // would be work done on every status tick for a result no one reads.
        return Ok(observations);
    }
    let pids = all_pids.into_iter().collect::<Vec<_>>();
    // Ports are the optional half of an observation: without them the port keep-alive rules
    // go unmatched, but the process rules, the foreground command and the editor handling all
    // still work. Failing the whole observation instead is what made session sleep unusable
    // on a host with no `lsof`.
    let ports = match process.listening_ports(&pids).await {
        Ok(ports) => ports,
        Err(error) => {
            warn_ports_unavailable(&error);
            Vec::new()
        }
    };
    for port in ports {
        for observation in observations.values_mut() {
            if observation
                .processes
                .iter()
                .any(|entry| entry.pid == port.pid)
                || observation.shell_pid == Some(port.pid)
            {
                observation.ports.push(port.port);
            }
        }
    }
    Ok(observations)
}

/// Warns that listening ports could not be observed, without repeating itself.
///
/// Reuses the maintenance loop's keyed limiter rather than suppressing on time alone: a
/// *different* failure arriving inside the hour is news and must not be demoted to debug
/// just because an earlier one was noisy.
///
/// The state is module-level because the two call paths cannot share an owner —
/// [`apply_session`] is a free function that `services/sessions/lifecycle.rs` calls without
/// a [`Sleep`], while [`refresh_observations`] goes through one. A daemon has exactly one
/// observation pipeline, so one limiter is the right granularity in production; the cost is
/// that a test asserting on this warning would see another test's state, and none do.
fn warn_ports_unavailable(error: &DaemonError) {
    static PORTS_WARNING: std::sync::LazyLock<std::sync::Mutex<RepeatedFailure>> =
        std::sync::LazyLock::new(|| {
            std::sync::Mutex::new(RepeatedFailure::new(
                "listening-port observation unavailable; port keep-alive rules cannot match \
                 until it recovers",
            ))
        });
    PORTS_WARNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .report(error);
}

fn descendants_from_snapshot(
    snapshot: &[ProcessInfo],
    children: &HashMap<u32, Vec<usize>>,
    root: u32,
) -> Vec<ProcessInfo> {
    let mut visited = BTreeSet::from([root]);
    let mut pending = children
        .get(&root)
        .into_iter()
        .flatten()
        .map(|index| (0_usize, *index))
        .collect::<BTreeSet<_>>();
    let mut descendants = Vec::new();
    while let Some((pass, index)) = pending.pop_first() {
        let entry = &snapshot[index];
        if !visited.insert(entry.pid) {
            continue;
        }
        descendants.push(entry.clone());
        // Preserve the original repeated-scan order, including children before their parents.
        for child in children.get(&entry.pid).into_iter().flatten() {
            pending.insert((pass + usize::from(*child <= index), *child));
        }
    }
    descendants
}

async fn wait_for_exit(process: &dyn Process, pid: u32, grace_ms: i64) {
    let grace = Duration::from_millis(u64::try_from(grace_ms.max(0)).unwrap_or(0));
    let started = tokio::time::Instant::now();
    while process.is_alive(pid) && started.elapsed() < grace {
        let remaining = grace.saturating_sub(started.elapsed());
        tokio::time::sleep(remaining.min(Duration::from_millis(100))).await;
    }
}

fn is_editor_command(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|program| Path::new(program).file_name())
        .and_then(|program| program.to_str())
        .is_some_and(|program| matches!(program, "vim" | "nvim"))
}

fn core_kept(kept: &[SleepKept]) -> Vec<KeptTerminal> {
    kept.iter()
        .map(|entry| KeptTerminal {
            name: entry.window.clone(),
            reason: entry.reason.clone(),
        })
        .collect()
}

fn empty_result() -> SleepResult {
    SleepResult {
        kept: Vec::new(),
        closed: Vec::new(),
        session_killed: false,
    }
}

fn match_rule_count(
    policy: &CompiledSleepPolicy,
    rule_index: usize,
    rule: &KeepAliveRule,
    snapshot: &[ProcessInfo],
    ports: Result<&Vec<ListeningPort>, &DaemonError>,
) -> KeepAliveRuleMatch {
    if !rule.enabled {
        return KeepAliveRuleMatch {
            rule_id: rule.id.clone(),
            count: 0,
            error: None,
        };
    }
    match rule.kind {
        KeepAliveKind::Process => {
            if let Some(diagnostic) = policy
                .diagnostics()
                .iter()
                .find(|diagnostic| diagnostic.rule_index == rule_index)
            {
                return KeepAliveRuleMatch {
                    rule_id: rule.id.clone(),
                    count: 0,
                    error: Some(diagnostic.error.to_string()),
                };
            }
            let count = policy.count_process_matches(
                rule_index,
                snapshot.iter().map(|entry| entry.command.as_str()),
            );
            KeepAliveRuleMatch {
                rule_id: rule.id.clone(),
                count: u64::try_from(count).unwrap_or(u64::MAX),
                error: None,
            }
        }
        KeepAliveKind::ListeningPort => match ports {
            Ok(ports) => KeepAliveRuleMatch {
                rule_id: rule.id.clone(),
                count: u64::try_from(ports.len()).unwrap_or(u64::MAX),
                error: None,
            },
            Err(error) => KeepAliveRuleMatch {
                rule_id: rule.id.clone(),
                count: 0,
                error: Some(error.to_string()),
            },
        },
    }
}

fn terminal_io_error(terminal: TerminalId, error: impl std::fmt::Display) -> DaemonError {
    DaemonError::Process(format!("terminal `{terminal}`: {error}"))
}

async fn refresh_observations(
    runtime: &SessionRuntime,
    config_store: &ConfigStore,
    process: &dyn Process,
) -> DaemonResult<()> {
    let sessions = runtime.sessions();
    if sessions.is_empty() {
        return Ok(());
    }
    let (_, policy) = config_store.load_with_sleep_policy().await?;
    let observations = observe_sessions(&sessions, process, policy.ports_enabled()).await?;
    publish_observations(runtime, &policy, &sessions, &observations);
    Ok(())
}

/// Publishes the foreground command, keep-alive labels, and recognized agent of every
/// terminal, and returns the labels so a caller deciding closability does not re-match
/// the policy it just evaluated.
fn publish_observations(
    runtime: &SessionRuntime,
    policy: &CompiledSleepPolicy,
    sessions: &[Session],
    observations: &HashMap<TerminalId, Observation>,
) -> HashMap<TerminalId, Vec<String>> {
    let mut keep_alive = HashMap::new();
    for terminal in sessions.iter().flat_map(|session| &session.terminals) {
        let observation = observations.get(&terminal.id);
        let labels = observation.map_or_else(Vec::new, |observation| {
            policy.match_keep_alive(&observation.commands, &observation.ports)
        });
        let agent = observation
            .and_then(|observation| {
                observation
                    .commands
                    .iter()
                    .find_map(|command| recognized_agent(command))
            })
            .map(str::to_owned);
        runtime.update_observation(
            terminal.id,
            observation.and_then(Observation::foreground_command),
            labels.clone(),
            agent,
        );
        keep_alive.insert(terminal.id, labels);
    }
    keep_alive
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapters::{clock::SystemClock, process::RealProcess},
        testing::fakes::{FakeFiles, FakeProcess, FakeShell},
    };

    #[tokio::test]
    async fn terminal_close_waits_for_the_session_transition_gate() {
        let temp = tempfile::tempdir().unwrap();
        let files = Arc::new(FakeFiles::new(
            temp.path().join("trash"),
            vec![temp.path().join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(temp.path(), files.clone()));
        let mut effective = config.load().await.unwrap();
        effective.windows = vec![fleet_core::config::WindowConfig {
            name: "lazygit".to_owned(),
            command: fleet_core::config::NATIVE_LAZYGIT.to_owned(),
        }];
        config.save(effective).await.unwrap();
        let state = Arc::new(StateStore::new(temp.path(), files, Arc::new(SystemClock)));
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
        let session = sessions
            .ensure(None, Some(fleet_core::config::Agent::Claude), false)
            .await
            .unwrap();
        let session_id = session.id.clone();
        let transition = sessions
            .runtime
            .claim_terminal_transition(session_id.clone())
            .await;
        let sleep = Sleep::new(config, state, Arc::new(FakeProcess::default()), &sessions);
        let mut sleeping = tokio::spawn(async move { sleep.session(session_id).await });

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut sleeping)
                .await
                .is_err(),
            "sleep bypassed the terminal transition gate"
        );
        drop(transition);
        let result = tokio::time::timeout(Duration::from_secs(2), sleeping)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.closed.len(), 1);
        assert!(result.session_killed);
    }

    /// Regression: on a host without `lsof` every observation failed with
    /// `failed to sleep previous session error=… lsof …` and
    /// `failed to refresh terminal process observations`, so the session-sleep policy never
    /// worked there and the log filled at the status-tick rate.
    #[tokio::test]
    async fn observation_degrades_to_no_ports_when_they_cannot_be_read() {
        let process = FakeProcess::default();
        process.set_snapshot(vec![ProcessInfo {
            pid: 200,
            parent_pid: 100,
            process_group_id: 200,
            terminal_foreground_process_group_id: Some(200),
            start_identity: "shell-start".to_owned(),
            command: "node server.js".to_owned(),
        }]);
        process.fail_ports("command failed: lsof: No such file or directory");
        let sessions = [session_with_shell(100)];

        let observations = observe_sessions(&sessions, &process, true)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let observation = observations
            .get(&TerminalId(1))
            .unwrap_or_else(|| panic!("the terminal is still observed"));
        // The `ps` half of the observation is intact; only the ports are missing.
        assert_eq!(observation.commands, vec!["node server.js".to_owned()]);
        assert_eq!(
            observation.foreground_command().as_deref(),
            Some("node server.js")
        );
        assert!(observation.ports.is_empty());
    }

    /// Observing ports means an `lsof` process or a readlink over every descendant's file
    /// descriptors. On a machine with no port keep-alive rule — the default configuration —
    /// that is pure cost on every status tick, for an answer nothing reads.
    #[tokio::test]
    async fn ports_are_not_observed_when_no_enabled_rule_matches_on_them() {
        let process = FakeProcess::default();
        process.set_snapshot(vec![ProcessInfo {
            pid: 200,
            parent_pid: 100,
            process_group_id: 200,
            terminal_foreground_process_group_id: Some(200),
            start_identity: "shell-start".to_owned(),
            command: "node server.js".to_owned(),
        }]);
        let sessions = [session_with_shell(100)];

        let observations = observe_sessions(&sessions, &process, false)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(process.port_calls(), 0);
        // The process half of the observation is unaffected by the gate.
        let observation = observations
            .get(&TerminalId(1))
            .unwrap_or_else(|| panic!("the terminal is still observed"));
        assert_eq!(observation.commands, vec!["node server.js".to_owned()]);

        // A configuration that does have a port rule still gets its query.
        observe_sessions(&sessions, &process, true)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(process.port_calls(), 1);
    }

    fn session_with_shell(shell_pid: u32) -> Session {
        Session {
            id: SessionId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}")),
            host: None,
            kind: fleet_core::sessions::SessionKind::Agent(fleet_core::config::Agent::Claude),
            cwd: "/tmp".to_owned(),
            terminals: vec![fleet_core::sessions::Terminal {
                id: TerminalId(1),
                name: "shell".to_owned(),
                command: "zsh".to_owned(),
                cwd: "/tmp".to_owned(),
                shell_pid: Some(shell_pid),
                foreground_command: None,
                status: fleet_core::sessions::TerminalStatus::Running,
                title: None,
                keep_alive: Vec::new(),
                has_unseen_output: false,
                agent_attention: None,
                kind: fleet_core::sessions::TerminalKind::default(),
            }],
            active_terminal: Some(TerminalId(1)),
            slept_at: None,
            kept_terminals: Vec::new(),
        }
    }

    #[tokio::test]
    async fn idle_observation_does_not_run_process_queries() {
        let temp = tempfile::tempdir().unwrap();
        let files = Arc::new(FakeFiles::new(
            temp.path().join("trash"),
            vec![temp.path().join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(temp.path(), files.clone()));
        let state = Arc::new(StateStore::new(temp.path(), files, Arc::new(SystemClock)));
        let shell = Arc::new(FakeShell::new());
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
        let sleep = Sleep::new(
            config,
            state,
            Arc::new(RealProcess::new(shell.clone())),
            &sessions,
        );
        sleep.refresh_observations().await.unwrap();
        sleep.refresh_observations().await.unwrap();
        assert_eq!(shell.calls().len(), 0);
    }

    #[test]
    fn indexed_descendants_keep_scan_order_for_unordered_process_tables() {
        let snapshot = [(4, 3), (2, 1), (5, 2), (3, 2), (6, 3), (99, 90)]
            .into_iter()
            .map(|(pid, parent_pid)| ProcessInfo {
                pid,
                parent_pid,
                process_group_id: pid,
                terminal_foreground_process_group_id: Some(pid),
                start_identity: format!("start-{pid}"),
                command: pid.to_string(),
            })
            .collect::<Vec<_>>();
        let mut children = HashMap::<u32, Vec<usize>>::new();
        for (index, entry) in snapshot.iter().enumerate() {
            children.entry(entry.parent_pid).or_default().push(index);
        }
        let descendants = descendants_from_snapshot(&snapshot, &children, 1);
        assert_eq!(
            descendants
                .iter()
                .map(|entry| entry.pid)
                .collect::<Vec<_>>(),
            [2, 5, 3, 6, 4]
        );
    }

    #[test]
    fn background_editor_gets_no_shutdown_input() {
        let mut observation = Observation {
            processes: vec![ProcessInfo {
                pid: 42,
                parent_pid: 10,
                process_group_id: 42,
                terminal_foreground_process_group_id: Some(41),
                start_identity: "editor-start".to_owned(),
                command: "nvim notes.md".to_owned(),
            }],
            ..Observation::default()
        };

        assert_eq!(observation.editor_pid(), Some(42));
        assert_eq!(observation.foreground_editor_pid(), None);
        observation.processes[0].terminal_foreground_process_group_id = Some(42);
        assert_eq!(observation.editor_pid(), Some(42));
        assert_eq!(observation.foreground_editor_pid(), Some(42));
    }
}
