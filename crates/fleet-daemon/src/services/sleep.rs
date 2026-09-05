//! Session sleep policy, process observation, and editor shutdown handling.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::{Arc, Weak},
    time::Duration,
};

use fleet_core::{
    ids::{SessionId, TerminalId, WorktreeId},
    sessions::{KeptTerminal, Session},
    sleep::{KeepAliveKind, KeepAliveRule, match_keep_alive},
};
use fleet_proto::response::{KeepAliveRuleMatch, SleepKept, SleepResult};

use crate::{
    DaemonError, DaemonResult,
    adapters::process::{ListeningPort, Process, ProcessInfo},
    services::sessions::{SessionRuntime, shared_runtime},
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
    /// Creates the sleep service and starts best-effort foreground-command polling.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        process: Arc<dyn Process>,
    ) -> Self {
        let runtime = shared_runtime(&state);
        runtime.register_process(Arc::clone(&process));
        start_status_poller(
            Arc::downgrade(&runtime),
            Arc::clone(&config),
            Arc::clone(&process),
        );
        Self {
            config,
            state,
            process,
            runtime,
        }
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
                return Err(DaemonError::Unsupported(
                    "remote hosts are not supported yet".to_owned(),
                ));
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
            return Err(DaemonError::Unsupported(
                "remote hosts are not supported yet".to_owned(),
            ));
        }
        let session = SessionId::try_from(worktree.session.as_str())
            .map_err(|error| DaemonError::Validation(error.to_string()))?;
        self.session(session).await
    }

    /// Counts current process matches for configured keep-alive rules.
    pub async fn match_keep_alive_rules(&self) -> DaemonResult<Vec<KeepAliveRuleMatch>> {
        let config = self.config.load().await?;
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
            .map(|rule| match_rule_count(rule, &snapshot, ports.as_ref()))
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
    let config = config_store.load().await?;
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

    let observations = observe_session(&session, process).await?;
    for terminal in &session.terminals {
        if terminal.is_native() {
            // Nothing to observe: the client draws this tab and no process belongs to it.
            continue;
        }
        let observation = observations.get(&terminal.id);
        let labels = observation.map_or_else(Vec::new, |observation| {
            match_keep_alive(
                &config.sleep.keep_alive,
                &observation.commands,
                &observation.ports,
            )
        });
        let foreground = observation.and_then(Observation::foreground_command);
        runtime.update_observation(terminal.id, foreground, labels);
    }

    let mut kept = Vec::new();
    let mut closable = Vec::new();
    for terminal in session.terminals.iter().rev() {
        if terminal.is_native() {
            // Idle by construction: no keep-alive rule can match a tab with no process, and
            // there is no editor to ask to save. It closes with the rest, and `ensure` puts it
            // back — exactly what a `lazygit` PTY did before it was native.
            closable.push((terminal.id, terminal.name.clone()));
            continue;
        }
        let observation = observations.get(&terminal.id);
        let labels = observation.map_or_else(Vec::new, |observation| {
            match_keep_alive(
                &config.sleep.keep_alive,
                &observation.commands,
                &observation.ports,
            )
        });
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

async fn observe_session(
    session: &Session,
    process: &dyn Process,
) -> DaemonResult<HashMap<TerminalId, Observation>> {
    observe_sessions(std::slice::from_ref(session), process).await
}

async fn observe_sessions(
    sessions: &[Session],
    process: &dyn Process,
) -> DaemonResult<HashMap<TerminalId, Observation>> {
    let snapshot = process.snapshot().await?;
    let mut observations = HashMap::new();
    let mut all_pids = BTreeSet::new();
    for terminal in sessions.iter().flat_map(|session| &session.terminals) {
        let mut observation = Observation {
            shell_pid: terminal.shell_pid,
            ..Observation::default()
        };
        if let Some(shell_pid) = terminal.shell_pid {
            all_pids.insert(shell_pid);
            observation.processes = descendants_from_snapshot(&snapshot, shell_pid);
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

fn descendants_from_snapshot(snapshot: &[ProcessInfo], root: u32) -> Vec<ProcessInfo> {
    let mut parents = BTreeSet::from([root]);
    let mut descendants = Vec::new();
    loop {
        let mut changed = false;
        for entry in snapshot {
            if parents.contains(&entry.parent_pid) && !parents.contains(&entry.pid) {
                parents.insert(entry.pid);
                descendants.push(entry.clone());
                changed = true;
            }
        }
        if !changed {
            break;
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
            if has_obvious_regex_error(&rule.pattern) {
                return KeepAliveRuleMatch {
                    rule_id: rule.id.clone(),
                    count: 0,
                    error: Some(format!("invalid process pattern `{}`", rule.pattern)),
                };
            }
            let count = snapshot
                .iter()
                .filter(|entry| {
                    !match_keep_alive(
                        std::slice::from_ref(rule),
                        std::slice::from_ref(&entry.command),
                        &[],
                    )
                    .is_empty()
                })
                .count();
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

fn has_obvious_regex_error(pattern: &str) -> bool {
    let mut square = 0_i64;
    let mut round = 0_i64;
    let mut escaped = false;
    for character in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' => square += 1,
            ']' => {
                square -= 1;
                if square < 0 {
                    return true;
                }
            }
            '(' if square == 0 => round += 1,
            ')' if square == 0 => {
                round -= 1;
                if round < 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    escaped || square != 0 || round != 0
}

fn terminal_io_error(terminal: TerminalId, error: impl std::fmt::Display) -> DaemonError {
    DaemonError::Process(format!("terminal `{terminal}`: {error}"))
}

fn start_status_poller(
    runtime: Weak<SessionRuntime>,
    config: Arc<ConfigStore>,
    process: Arc<dyn Process>,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        loop {
            let delay = config
                .load()
                .await
                .map(|config| config.ui.status_refresh_ms.max(500))
                .ok()
                .and_then(|millis| u64::try_from(millis).ok())
                .map(Duration::from_millis)
                .unwrap_or_else(|| Duration::from_secs(2));
            tokio::time::sleep(delay).await;
            let Some(runtime) = runtime.upgrade() else {
                break;
            };
            if let Err(error) = refresh_observations(&runtime, &config, process.as_ref()).await {
                tracing::warn!(%error, "failed to refresh terminal process observations");
            }
        }
    });
}

async fn refresh_observations(
    runtime: &SessionRuntime,
    config_store: &ConfigStore,
    process: &dyn Process,
) -> DaemonResult<()> {
    let config = config_store.load().await?;
    let sessions = runtime.sessions();
    let observations = observe_sessions(&sessions, process).await?;
    for session in sessions {
        for terminal in session.terminals {
            let observation = observations.get(&terminal.id);
            let labels = observation.map_or_else(Vec::new, |observation| {
                match_keep_alive(
                    &config.sleep.keep_alive,
                    &observation.commands,
                    &observation.ports,
                )
            });
            runtime.update_observation(
                terminal.id,
                observation.and_then(Observation::foreground_command),
                labels,
            );
        }
    }
    Ok(())
}
