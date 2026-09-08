//! Session and terminal domain types independent of PTY implementation details.

use serde::{Deserialize, Serialize};

use crate::{
    config::{Agent, Config},
    ids::{IdError, SessionId, TerminalId, WorktreeId},
};

/// The workload represented by a daemon-owned terminal session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// A session rooted in a published worktree.
    Worktree(WorktreeId),
    /// A repository-level coding-agent session.
    Agent(Agent),
}

/// A terminal retained by the most recent sleep operation and its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptTerminal {
    /// Session-local terminal name.
    pub name: String,
    /// Human-readable keep-alive reason.
    pub reason: String,
}

/// A daemon-owned collection of terminals sharing a working context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Stable session name.
    pub id: SessionId,
    /// Session workload.
    pub kind: SessionKind,
    /// Default working directory.
    pub cwd: String,
    /// Ordered daemon-owned terminals.
    pub terminals: Vec<Terminal>,
    /// Selected terminal, when the session has any terminals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_terminal: Option<TerminalId>,
    /// ISO-8601 time of the most recent successful sleep operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slept_at: Option<String>,
    /// Terminals retained by the most recent sleep operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_terminals: Vec<KeptTerminal>,
}

/// Lifecycle state of a terminal's foreground process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    /// The PTY and login shell are being launched.
    Starting,
    /// The PTY is alive.
    Running,
    /// The PTY exited, optionally with a process exit code.
    Exited {
        /// Process exit code when one was available.
        code: Option<i32>,
    },
}

/// Where a terminal's content comes from.
///
/// Native tabs retain their configured names and positions, but have no PTY. Their content
/// is provided by the client for a reserved [`crate::config::NATIVE_SCHEME`] command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    /// A PTY running a login shell. The default, and what every field below describes.
    #[default]
    Pty,
    /// A surface the client draws. No PTY, no shell pid, nothing to attach to.
    Native,
}

/// Runtime metadata for one daemon-owned terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Terminal {
    /// Stable numeric terminal identifier.
    pub id: TerminalId,
    /// Session-local terminal name.
    pub name: String,
    /// Initial shell command.
    pub command: String,
    /// Terminal working directory.
    pub cwd: String,
    /// Login-shell PID, when launched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_pid: Option<u32>,
    /// Best-effort foreground command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground_command: Option<String>,
    /// PTY lifecycle state.
    pub status: TerminalStatus,
    /// Latest terminal title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Labels that currently prevent sleep.
    #[serde(default)]
    pub keep_alive: Vec<String>,
    /// Whether output arrived since the owning client last selected this terminal.
    #[serde(default)]
    pub has_unseen_output: bool,
    /// Whether a process or the client provides this terminal's content.
    ///
    /// Older snapshots without this field describe PTY terminals.
    #[serde(default)]
    pub kind: TerminalKind,
}

impl Terminal {
    /// Whether the Fleet app, rather than a PTY, draws this terminal.
    ///
    /// Everything that assumes a process — attach, keys, resize, scroll, the exit strip, the
    /// sleep observer — must ask this first: a native terminal has no `shell_pid` and no host.
    #[must_use]
    pub const fn is_native(&self) -> bool {
        matches!(self.kind, TerminalKind::Native)
    }
}

/// Observed attachment state for a worktree session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// No session exists.
    #[default]
    None,
    /// A session exists without an attached client.
    Detached,
    /// At least one client is attached.
    Attached,
    /// Observation failed, commonly because a remote host is offline.
    Unknown,
}

/// Whether a recognized coding agent is actively producing work or waiting for input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AgentActivity {
    /// No recognized agent process or no activity signal yet.
    #[default]
    Unknown,
    /// The agent is producing output or was explicitly marked as working.
    Working,
    /// The live agent is quiet and waiting for the user.
    Idle,
}

/// Per-terminal summary used by worktree status views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeWindowStatus {
    /// Zero-based terminal position.
    pub index: u32,
    /// Terminal name.
    pub name: String,
    /// Best-effort current command.
    pub command: String,
    /// Keep-alive labels for this terminal.
    pub keep_alive: Vec<String>,
    /// Recognized coding-agent executable in this terminal.
    pub agent: Option<String>,
    /// Current activity of the recognized agent.
    pub agent_activity: AgentActivity,
    /// ISO-8601 time when `agent_activity` last changed.
    pub agent_activity_changed_at: Option<String>,
}

/// Runtime status of a published worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeStatus {
    /// Worktree being summarized.
    pub worktree_id: WorktreeId,
    /// Session attachment state.
    pub session: SessionState,
    /// Ordered terminal summaries.
    pub windows: Vec<WorktreeWindowStatus>,
    /// Combined keep-alive labels.
    pub running: Vec<String>,
    /// Aggregate activity across the worktree's terminals.
    pub agent_activity: AgentActivity,
    /// Latest transition time among terminals determining the aggregate.
    pub agent_activity_changed_at: Option<String>,
}

/// Aggregates terminal activity with `Working` taking precedence over `Idle`.
#[must_use]
pub fn aggregate_agent_activity(
    windows: &[WorktreeWindowStatus],
) -> (AgentActivity, Option<String>) {
    let activity = if windows
        .iter()
        .any(|window| window.agent_activity == AgentActivity::Working)
    {
        AgentActivity::Working
    } else if windows
        .iter()
        .any(|window| window.agent_activity == AgentActivity::Idle)
    {
        AgentActivity::Idle
    } else {
        AgentActivity::Unknown
    };
    let changed_at = windows
        .iter()
        .filter(|window| window.agent_activity == activity)
        .filter_map(|window| window.agent_activity_changed_at.as_ref())
        .max()
        .cloned();
    (activity, changed_at)
}

/// A resolved default-terminal definition ready for session creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSpec {
    /// Session-local terminal name.
    pub name: String,
    /// Shell command after agent placeholder substitution.
    pub command: String,
}

/// Resolves the configured terminal layout and substitutes the selected agent command.
#[must_use]
pub fn default_terminals(config: &Config, agent: Agent) -> Vec<TerminalSpec> {
    let agent_command = config.agent_commands.command(agent);
    config
        .windows
        .iter()
        .map(|window| TerminalSpec {
            name: window.name.clone(),
            command: window.command.replace("{agent}", agent_command),
        })
        .collect()
}

/// Returns the runtime-only repository-level session name for an agent.
pub fn agent_session_id(agent: Agent) -> Result<SessionId, IdError> {
    SessionId::try_from(match agent {
        Agent::Claude => "swarm-agent-claude",
        Agent::Opencode => "swarm-agent-opencode",
    })
}

/// Returns the agent-popup session name scoped to one worktree.
///
/// `NATIVE-AGENTS.md` §1/§2 make `^s F` the *same-worktree* terminal fallback for the thread
/// on screen, so it cannot land in the one repository-level session
/// [`agent_session_id`] names: that one lives in `repos_dir` and would open `claude` beside the
/// repositories rather than inside the worktree the thread is editing.
pub fn worktree_agent_session_id(
    worktree_session: &str,
    agent: Agent,
) -> Result<SessionId, IdError> {
    let name = match agent {
        Agent::Claude => "claude",
        Agent::Opencode => "opencode",
    };
    SessionId::try_from(format!("{worktree_session}/agent-{name}"))
}

#[cfg(test)]
mod tests {
    use crate::config::default_config;

    use super::*;

    fn window(activity: AgentActivity, changed_at: Option<&str>) -> WorktreeWindowStatus {
        WorktreeWindowStatus {
            index: 0,
            name: "agent".to_owned(),
            command: "claude".to_owned(),
            keep_alive: Vec::new(),
            agent: Some("claude".to_owned()),
            agent_activity: activity,
            agent_activity_changed_at: changed_at.map(str::to_owned),
        }
    }

    #[test]
    fn agent_activity_aggregate_uses_priority_and_determining_timestamp() {
        let windows = vec![
            window(AgentActivity::Idle, Some("2026-09-05T12:00:00Z")),
            window(AgentActivity::Working, Some("2026-09-05T11:00:00Z")),
            window(AgentActivity::Working, Some("2026-09-05T13:00:00Z")),
        ];
        assert_eq!(
            aggregate_agent_activity(&windows),
            (
                AgentActivity::Working,
                Some("2026-09-05T13:00:00Z".to_owned())
            )
        );
        assert_eq!(
            aggregate_agent_activity(&[window(AgentActivity::Idle, Some("2026-09-05T14:00:00Z"))]),
            (AgentActivity::Idle, Some("2026-09-05T14:00:00Z".to_owned()))
        );
        assert_eq!(
            aggregate_agent_activity(&[]),
            (AgentActivity::Unknown, None)
        );
    }

    #[test]
    fn worktree_activity_serializes_with_camel_case_status_fields() {
        let value = serde_json::to_value(WorktreeStatus {
            worktree_id: WorktreeId::try_from("acme/api#feature")
                .unwrap_or_else(|error| panic!("{error}")),
            session: SessionState::Detached,
            windows: vec![window(AgentActivity::Idle, Some("2026-09-05T14:00:00Z"))],
            running: vec!["claude".to_owned()],
            agent_activity: AgentActivity::Idle,
            agent_activity_changed_at: Some("2026-09-05T14:00:00Z".to_owned()),
        })
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(value["agentActivity"], "idle");
        assert_eq!(value["agentActivityChangedAt"], "2026-09-05T14:00:00Z");
        assert_eq!(value["windows"][0]["agent"], "claude");
        assert_eq!(value["windows"][0]["agentActivity"], "idle");
    }

    /// The names and their order are what `ctrl-s <n>` counts; they never move.
    #[test]
    fn resolves_default_layout() {
        let config = default_config("/tmp/.fleet");
        let terminals = default_terminals(&config, Agent::Claude);
        assert_eq!(
            terminals,
            vec![
                TerminalSpec {
                    name: "nvim".to_owned(),
                    command: "nvim .".to_owned()
                },
                TerminalSpec {
                    name: "cc".to_owned(),
                    command: "claude".to_owned()
                },
                TerminalSpec {
                    name: "lg".to_owned(),
                    command: crate::config::NATIVE_LAZYGIT.to_owned()
                },
            ]
        );
    }

    #[test]
    fn constructs_agent_session_names() {
        assert_eq!(
            agent_session_id(Agent::Opencode).map(|id| id.to_string()),
            Ok("swarm-agent-opencode".to_owned())
        );
    }

    #[test]
    fn a_worktree_fallback_session_is_named_after_its_worktree() {
        // §1/§2: `^s F` is the same-worktree fallback, so its session is per worktree *and*
        // per agent — never the one repository-level session in `repos_dir`.
        assert_eq!(
            worktree_agent_session_id("smoke/repo/work", Agent::Claude).map(|id| id.to_string()),
            Ok("smoke/repo/work/agent-claude".to_owned())
        );
        assert_ne!(
            worktree_agent_session_id("smoke/repo/work", Agent::Claude),
            worktree_agent_session_id("smoke/repo/other", Agent::Claude)
        );
        assert_ne!(
            worktree_agent_session_id("smoke/repo/work", Agent::Claude),
            agent_session_id(Agent::Claude)
        );
    }
}
