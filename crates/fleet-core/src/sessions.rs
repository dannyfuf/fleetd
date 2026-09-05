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
    let name = match agent {
        Agent::Claude => "swarm-agent-claude",
        Agent::Opencode => "swarm-agent-opencode",
    };
    SessionId::try_from(name)
}

#[cfg(test)]
mod tests {
    use crate::config::default_config;

    use super::*;

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
                    command: "lazygit".to_owned()
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
}
