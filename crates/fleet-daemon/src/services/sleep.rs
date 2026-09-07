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
    runtime: &SessionRuntime,
    config_store: &ConfigStore,
    process: &dyn Process,
    session_id: &SessionId,
) -> DaemonResult<SleepResult> {
    let Some(session) = runtime.session(session_id) else {
        return Ok(empty_result());
    };
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
    let observations = observe_sessions(sessions, process).await?;
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
            if let Some(host) = runtime.host(terminal.id) {
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

    let closed = closable
        .iter()
        .map(|(_, name)| name.clone())
        .collect::<Vec<_>>();
    if kept.is_empty() {
        runtime.kill_if_present(session_id);
        return Ok(SleepResult {
            kept,
            closed,
            session_killed: true,
        });
    }
    for (terminal, _) in closable {
        runtime.close_terminal_if_present(terminal);
    }
    runtime.record_sleep(session_id, core_kept(&kept));
    Ok(SleepResult {
        kept,
        closed,
        session_killed: false,
    })
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
}

async fn observe_sessions(
    sessions: &[Session],
    process: &dyn Process,
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
    let pids = all_pids.into_iter().collect::<Vec<_>>();
    let ports = process.listening_ports(&pids).await?;
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
    let observations = observe_sessions(&sessions, process).await?;
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
        testing::fakes::{FakeFiles, FakeShell},
    };

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
}
