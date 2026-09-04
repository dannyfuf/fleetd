//! Configuration schemas, defaults, and pure merge behavior.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    ids::{HostId, LOCAL_HOST},
    model::{HostConfigEntry, Hosts},
    sleep::{KeepAliveRule, default_keep_alive_rules},
};

/// The supported persisted configuration schema version.
pub const CONFIG_VERSION: u32 = 1;

/// Supported interactive coding agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// Anthropic Claude Code.
    Claude,
    /// OpenCode.
    Opencode,
}

/// Shell commands used to launch each supported agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommands {
    /// Command used for Claude Code.
    pub claude: String,
    /// Command used for OpenCode.
    pub opencode: String,
}

impl AgentCommands {
    /// Returns the configured shell command for an agent.
    #[must_use]
    pub fn command(&self, agent: Agent) -> &str {
        match agent {
            Agent::Claude => &self.claude,
            Agent::Opencode => &self.opencode,
        }
    }
}

/// A terminal in the default session layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowConfig {
    /// Unique display name within a session.
    pub name: String,
    /// Shell command typed into the terminal.
    pub command: String,
}

/// Sleep policy configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepConfig {
    /// Whether sleeping may close idle terminals.
    pub enabled: bool,
    /// Ordered rules that preserve terminals.
    pub keep_alive: Vec<KeepAliveRule>,
    /// Grace period after asking editors to quit.
    pub grace_ms: i64,
}

/// GitHub cache and clone settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubConfig {
    /// Repository discovery cache lifetime.
    pub cache_ttl_seconds: i64,
    /// Pull-request cache lifetime.
    pub pr_ttl_seconds: i64,
    /// Clone URL family to select.
    pub clone_protocol: CloneProtocol,
}

/// Clone URL protocol used for discovered GitHub repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloneProtocol {
    /// Use SSH clone URLs.
    Ssh,
    /// Use HTTPS clone URLs.
    Https,
}

/// UI status refresh intervals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiConfig {
    /// Local status refresh interval in milliseconds.
    pub status_refresh_ms: i64,
    /// Remote status refresh interval in milliseconds.
    pub remote_status_refresh_ms: i64,
}

/// Fleet's complete version-one configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Persisted schema version, always one.
    pub version: u32,
    /// Absolute directory containing pristine clones.
    pub repos_dir: String,
    /// Absolute directory containing published worktrees and pool slots.
    pub worktrees_dir: String,
    /// Configured remote hosts.
    pub hosts: Hosts,
    /// Default placement, either `local` or a configured host identifier.
    pub default_host: String,
    /// Desired prepared-copy count per repository.
    pub hot_pool_size: u64,
    /// Maximum fresh marker age in milliseconds.
    pub hot_freshness_ms: u64,
    /// Prepared-copy refresh interval in milliseconds; zero disables it.
    pub hot_refresh_interval_ms: u64,
    /// Selected coding agent.
    pub agent: Agent,
    /// Agent launch commands.
    pub agent_commands: AgentCommands,
    /// Ordered terminal layout.
    pub windows: Vec<WindowConfig>,
    /// Terminal sleep policy.
    pub sleep: SleepConfig,
    /// GitHub settings.
    pub github: GithubConfig,
    /// UI polling settings.
    pub ui: UiConfig,
}

/// A malformed partial or complete Fleet configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// JSON could not be converted to the configuration schema.
    #[error("invalid configuration: {0}")]
    Json(#[from] serde_json::Error),
    /// A cross-field or non-empty-string constraint failed.
    #[error("invalid configuration: {0}")]
    Validation(String),
}

/// Creates the exact default configuration for a Fleet home directory.
#[must_use]
pub fn default_config(home: impl AsRef<Path>) -> Config {
    let home = home.as_ref();
    Config {
        version: CONFIG_VERSION,
        repos_dir: home.join("repos").to_string_lossy().into_owned(),
        worktrees_dir: home.join("worktrees").to_string_lossy().into_owned(),
        hosts: BTreeMap::new(),
        default_host: LOCAL_HOST.to_owned(),
        hot_pool_size: 1,
        hot_freshness_ms: 60_000,
        hot_refresh_interval_ms: 300_000,
        agent: Agent::Claude,
        agent_commands: AgentCommands {
            claude: "claude".to_owned(),
            opencode: "opencode".to_owned(),
        },
        windows: vec![
            WindowConfig {
                name: "nvim".to_owned(),
                command: "nvim .".to_owned(),
            },
            WindowConfig {
                name: "cc".to_owned(),
                command: "{agent}".to_owned(),
            },
            WindowConfig {
                name: "lg".to_owned(),
                command: "lazygit".to_owned(),
            },
        ],
        sleep: SleepConfig {
            enabled: true,
            keep_alive: default_keep_alive_rules(),
            grace_ms: 2_000,
        },
        github: GithubConfig {
            cache_ttl_seconds: 3_600,
            pr_ttl_seconds: 90,
            clone_protocol: CloneProtocol::Ssh,
        },
        ui: UiConfig {
            status_refresh_ms: 2_000,
            remote_status_refresh_ms: 10_000,
        },
    }
}

/// Deep-merges a partial JSON object over defaults and applies compatibility normalization.
///
/// Tildes are expanded relative to the parent of `fleet_home`, matching the normal
/// `~/.fleet` layout while keeping this helper deterministic and free of environment access.
pub fn merge_config(fleet_home: impl AsRef<Path>, patch: Value) -> Result<Config, ConfigError> {
    let fleet_home = fleet_home.as_ref();
    let user_home = fleet_home.parent().unwrap_or(fleet_home);
    merge_config_with_user_home(fleet_home, user_home, patch)
}

/// Deep-merges configuration with an explicit user home for deterministic tilde expansion.
pub fn merge_config_with_user_home(
    fleet_home: impl AsRef<Path>,
    user_home: impl AsRef<Path>,
    patch: Value,
) -> Result<Config, ConfigError> {
    let fleet_home = fleet_home.as_ref();
    let user_home = user_home.as_ref();
    let mut merged = serde_json::to_value(default_config(fleet_home))?;
    deep_merge_json(&mut merged, patch);
    let mut config: Config = serde_json::from_value(merged)?;
    normalize_legacy_agent_window(&mut config.windows);
    config.repos_dir = normalize_directory(&config.repos_dir, user_home);
    config.worktrees_dir = normalize_directory(&config.worktrees_dir, user_home);
    validate_config(&config)?;
    apply_runtime_clamps(&mut config);
    Ok(config)
}

/// Recursively merges JSON objects, replacing scalars and arrays at their containing key.
pub fn deep_merge_json(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                if let Some(base_value) = base.get_mut(&key) {
                    deep_merge_json(base_value, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

/// Applies runtime minimums that are intentionally looser in the persisted schema.
pub fn apply_runtime_clamps(config: &mut Config) {
    config.sleep.grace_ms = config.sleep.grace_ms.max(0);
    config.ui.status_refresh_ms = config.ui.status_refresh_ms.max(500);
    config.ui.remote_status_refresh_ms = config.ui.remote_status_refresh_ms.max(500);
}

/// Converts the first legacy agent window command when no `{agent}` placeholder exists.
pub fn normalize_legacy_agent_window(windows: &mut [WindowConfig]) {
    if windows
        .iter()
        .any(|window| window.command.contains("{agent}"))
    {
        return;
    }
    if let Some(window) = windows
        .iter_mut()
        .find(|window| matches!(window.command.as_str(), "cc" | "claude" | "opencode"))
    {
        window.command = "{agent}".to_owned();
    }
}

/// Validates cross-field and non-empty constraints in a complete configuration.
pub fn validate_config(config: &Config) -> Result<(), ConfigError> {
    if config.version != CONFIG_VERSION {
        return Err(ConfigError::Validation("version must be 1".to_owned()));
    }
    if config.repos_dir.is_empty() || config.worktrees_dir.is_empty() {
        return Err(ConfigError::Validation(
            "reposDir and worktreesDir must be non-empty".to_owned(),
        ));
    }
    if config.default_host != LOCAL_HOST {
        let host = HostId::try_from(config.default_host.as_str())
            .map_err(|error| ConfigError::Validation(error.to_string()))?;
        if !config.hosts.contains_key(&host) {
            return Err(ConfigError::Validation(format!(
                "defaultHost `{}` is not configured",
                config.default_host
            )));
        }
    }
    for HostConfigEntry { ssh, swarm_command } in config.hosts.values() {
        if ssh.is_empty() || swarm_command.is_empty() {
            return Err(ConfigError::Validation(
                "host ssh and swarmCommand must be non-empty".to_owned(),
            ));
        }
    }
    if config.agent_commands.claude.is_empty() || config.agent_commands.opencode.is_empty() {
        return Err(ConfigError::Validation(
            "agent commands must be non-empty".to_owned(),
        ));
    }
    if config
        .windows
        .iter()
        .any(|window| window.name.is_empty() || window.command.is_empty())
    {
        return Err(ConfigError::Validation(
            "window names and commands must be non-empty".to_owned(),
        ));
    }
    if config.ui.remote_status_refresh_ms <= 0 {
        return Err(ConfigError::Validation(
            "ui.remoteStatusRefreshMs must be positive".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_directory(value: &str, user_home: &Path) -> String {
    let path = if value == "~" {
        user_home.to_path_buf()
    } else if let Some(relative) = value.strip_prefix("~/") {
        user_home.join(relative)
    } else {
        Path::new(value).to_path_buf()
    };
    let absolute = if path.is_absolute() {
        path
    } else {
        user_home.join(path)
    };
    lexical_normalize(&absolute).to_string_lossy().into_owned()
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_match_exact_swarm_json() {
        let actual = serde_json::to_value(default_config("/home/me/.fleet"))
            .unwrap_or_else(|error| panic!("{error}"));
        let expected = json!({
            "version": 1,
            "reposDir": "/home/me/.fleet/repos",
            "worktreesDir": "/home/me/.fleet/worktrees",
            "hosts": {},
            "defaultHost": "local",
            "hotPoolSize": 1,
            "hotFreshnessMs": 60000,
            "hotRefreshIntervalMs": 300000,
            "agent": "claude",
            "agentCommands": {"claude": "claude", "opencode": "opencode"},
            "windows": [
                {"name": "nvim", "command": "nvim ."},
                {"name": "cc", "command": "{agent}"},
                {"name": "lg", "command": "lazygit"}
            ],
            "sleep": {
                "enabled": true,
                "keepAlive": [
                    {"id":"claude","label":"claude","kind":"process","pattern":"(^|/)claude( |$)","enabled":true},
                    {"id":"opencode","label":"opencode","kind":"process","pattern":"(^|/)opencode( |$)","enabled":true},
                    {"id":"codex","label":"codex","kind":"process","pattern":"(^|/)codex( |$)","enabled":true},
                    {"id":"servers","label":"server","kind":"listening-port","pattern":"","enabled":true}
                ],
                "graceMs": 2000
            },
            "github": {"cacheTtlSeconds":3600,"prTtlSeconds":90,"cloneProtocol":"ssh"},
            "ui": {"statusRefreshMs":2000,"remoteStatusRefreshMs":10000}
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn deep_merges_strips_unknowns_expands_tildes_and_clamps() {
        let config = merge_config_with_user_home(
            "/home/me/.fleet",
            "/home/me",
            json!({
                "reposDir": "~/src",
                "unknown": true,
                "agentCommands": {"claude": "claude --model opus"},
                "sleep": {"graceMs": -5},
                "ui": {"statusRefreshMs": 1, "remoteStatusRefreshMs": 2}
            }),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.repos_dir, "/home/me/src");
        assert_eq!(config.agent_commands.opencode, "opencode");
        assert_eq!(config.sleep.grace_ms, 0);
        assert_eq!(config.ui.status_refresh_ms, 500);
        assert_eq!(config.ui.remote_status_refresh_ms, 500);
        let json = serde_json::to_value(config).unwrap_or_else(|error| panic!("{error}"));
        assert!(json.get("unknown").is_none());
    }

    #[test]
    fn resolves_dot_segments_and_rejects_non_positive_remote_interval() {
        let config = merge_config_with_user_home(
            "/home/me/.fleet",
            "/home/me",
            json!({"worktreesDir":"~/src/../worktrees"}),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.worktrees_dir, "/home/me/worktrees");
        assert!(
            merge_config("/home/me/.fleet", json!({"ui":{"remoteStatusRefreshMs":0}})).is_err()
        );
    }

    #[test]
    fn normalizes_legacy_agent_window_round_trip() {
        let config = merge_config(
            "/home/me/.fleet",
            json!({"windows":[{"name":"old","command":"opencode"}]}),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.windows[0].command, "{agent}");
        let value = serde_json::to_value(&config).unwrap_or_else(|error| panic!("{error}"));
        let round_trip =
            merge_config("/home/me/.fleet", value).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(round_trip, config);
    }
}
