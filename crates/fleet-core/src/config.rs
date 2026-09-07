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

/// A configurable process pattern eligible for daemon-side watch discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredWatchRule {
    /// Stable settings identifier.
    pub id: String,
    /// Full-command regular expression.
    pub pattern: String,
    /// Whether this pattern participates in discovery.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// Daemon-side process watch discovery configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscoveredWatchesConfig {
    /// Whether process scanning is enabled.
    pub enabled: bool,
    /// Milliseconds between process-table scans.
    pub interval_ms: u64,
    /// Ordered candidate process patterns.
    pub processes: Vec<DiscoveredWatchRule>,
}

impl Default for DiscoveredWatchesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_ms: 2_000,
            processes: vec![
                discovered_rule("codex-companion", r"codex-companion\.mjs task-worker"),
                discovered_rule("codex", r"(^|/)codex( |$)"),
                discovered_rule("claude", r"(^|/)claude( |$)"),
                discovered_rule("opencode", r"(^|/)opencode( |$)"),
            ],
        }
    }
}

fn discovered_rule(id: &str, pattern: &str) -> DiscoveredWatchRule {
    DiscoveredWatchRule {
        id: id.to_owned(),
        pattern: pattern.to_owned(),
        enabled: true,
    }
}

const fn default_enabled() -> bool {
    true
}

/// The supported persisted configuration schema version.
pub const CONFIG_VERSION: u32 = 1;

/// The scheme that marks a `windows[].command` as a Fleet-provided surface rather than a
/// program to run in a PTY.
///
/// Reserved commands retain their position in the configured tab list. Unknown commands
/// using this scheme are rejected instead of being launched in a PTY.
pub const NATIVE_SCHEME: &str = "fleet://";

/// The reserved command of the native git pane (`crates/fleet-lazygit`).
pub const NATIVE_LAZYGIT: &str = "fleet://lazygit";

/// Every reserved command Fleet answers; any other `fleet://` command is rejected.
const NATIVE_COMMANDS: &[&str] = &[NATIVE_LAZYGIT];

/// Whether a `windows[].command` names a Fleet-provided surface instead of a program.
#[must_use]
pub fn is_native_command(command: &str) -> bool {
    command.starts_with(NATIVE_SCHEME)
}

/// The legacy commands a swarm import upgrades to [`NATIVE_LAZYGIT`].
///
/// Only the bare program and the program with arguments count: `lazygit --help` still means
/// "the user wants the git UI in this tab", while `my-lazygit-wrapper` does not.
#[must_use]
fn is_legacy_lazygit_command(command: &str) -> bool {
    let command = command.trim();
    command == "lazygit" || command.starts_with("lazygit ")
}

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

/// Agent-finished notification settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NotificationsConfig {
    /// Whether an agent-finished toast is shown.
    pub toast: bool,
    /// Whether an agent-finished sound is played.
    pub sound: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            toast: true,
            sound: true,
        }
    }
}

/// UI status refresh intervals and notification preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiConfig {
    /// Local status refresh interval in milliseconds.
    pub status_refresh_ms: i64,
    /// Remote status refresh interval in milliseconds.
    pub remote_status_refresh_ms: i64,
    /// Agent-finished notification channels.
    #[serde(default)]
    pub notifications: NotificationsConfig,
}

/// Recoverable-deletion settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashConfig {
    /// Time in milliseconds that deleted entries remain available for restoration.
    pub retention_ms: u64,
}

impl Default for TrashConfig {
    fn default() -> Self {
        Self {
            retention_ms: 600_000,
        }
    }
}

/// Background-job retention and quit-warning settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsConfig {
    /// Whether quitting the app should warn while daemon jobs are active.
    pub warn_before_quit: bool,
    /// Time in milliseconds to retain completed jobs in the client-visible registry.
    pub keep_finished_for: u64,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            warn_before_quit: true,
            keep_finished_for: 600_000,
        }
    }
}

/// Terminal history storage and wheel sensitivity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TerminalConfig {
    /// Maximum retained history bytes per terminal (allocated on demand).
    pub scrollback_bytes: usize,
    /// Rows per line-based mouse wheel step.
    pub scroll_lines_per_step: u32,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_bytes: 1_073_741_824,
            scroll_lines_per_step: 3,
        }
    }
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
    /// Read-only daemon discovery of agent subprocesses.
    #[serde(default)]
    pub discovered_watches: DiscoveredWatchesConfig,
    /// GitHub settings.
    pub github: GithubConfig,
    /// UI polling settings.
    pub ui: UiConfig,
    /// Recoverable-deletion settings.
    #[serde(default)]
    pub trash: TrashConfig,
    /// Background-job retention and warning settings.
    #[serde(default)]
    pub jobs: JobsConfig,
    /// Terminal history and wheel settings.
    #[serde(default)]
    pub terminal: TerminalConfig,
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
                command: NATIVE_LAZYGIT.to_owned(),
            },
        ],
        sleep: SleepConfig {
            enabled: true,
            keep_alive: default_keep_alive_rules(),
            grace_ms: 2_000,
        },
        discovered_watches: DiscoveredWatchesConfig::default(),
        github: GithubConfig {
            cache_ttl_seconds: 3_600,
            pr_ttl_seconds: 90,
            clone_protocol: CloneProtocol::Ssh,
        },
        ui: UiConfig {
            status_refresh_ms: 2_000,
            remote_status_refresh_ms: 10_000,
            notifications: NotificationsConfig::default(),
        },
        trash: TrashConfig::default(),
        jobs: JobsConfig::default(),
        terminal: TerminalConfig::default(),
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
fn apply_runtime_clamps(config: &mut Config) {
    config.sleep.grace_ms = config.sleep.grace_ms.max(0);
    config.ui.status_refresh_ms = config.ui.status_refresh_ms.max(500);
    config.ui.remote_status_refresh_ms = config.ui.remote_status_refresh_ms.max(500);
}

/// Converts the first legacy agent window command when no `{agent}` placeholder exists.
fn normalize_legacy_agent_window(windows: &mut [WindowConfig]) {
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

/// Upgrades an *imported* window list to Fleet's own surfaces.
///
/// Import preserves tab names and positions. Ordinary configuration loading does not call
/// this: an explicit `lazygit` command in Fleet's config remains a PTY command.
pub fn normalize_imported_windows(windows: &mut [WindowConfig]) {
    for window in windows {
        if is_legacy_lazygit_command(&window.command) {
            window.command = NATIVE_LAZYGIT.to_owned();
        }
    }
}

/// Validates cross-field and non-empty constraints in a complete configuration.
pub fn validate_config(config: &Config) -> Result<(), ConfigError> {
    if config.terminal.scrollback_bytes == 0
        || !(1..=50).contains(&config.terminal.scroll_lines_per_step)
    {
        return Err(ConfigError::Validation(
            "terminal scrollbackBytes must be positive and scrollLinesPerStep must be in 1..=50"
                .to_owned(),
        ));
    }
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
    for window in &config.windows {
        if window.name.is_empty() || window.command.is_empty() {
            return Err(ConfigError::Validation(
                "window names and commands must be non-empty".to_owned(),
            ));
        }
        if is_native_command(&window.command) && !NATIVE_COMMANDS.contains(&window.command.as_str())
        {
            return Err(ConfigError::Validation(format!(
                "window `{}` uses unknown reserved command `{}`; the only one Fleet provides is `{NATIVE_LAZYGIT}`",
                window.name, window.command
            )));
        }
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
    fn terminal_defaults_and_positive_byte_budget() {
        let home = "/tmp/fleet";
        let defaults = merge_config(home, json!({})).unwrap();
        assert_eq!(defaults.terminal.scrollback_bytes, 1_073_741_824);
        assert_eq!(defaults.terminal.scroll_lines_per_step, 3);
        for value in [1, 50] {
            assert!(merge_config(home, json!({"terminal": {"scrollLinesPerStep": value}})).is_ok());
        }
        for value in [51, u32::MAX] {
            assert!(
                merge_config(home, json!({"terminal": {"scrollLinesPerStep": value}})).is_err()
            );
        }
        let custom = merge_config(
            home,
            json!({"terminal": {"scrollbackBytes": 65536, "scrollLinesPerStep": 5}}),
        )
        .unwrap();
        assert_eq!(custom.terminal.scrollback_bytes, 65536);
        assert_eq!(custom.terminal.scroll_lines_per_step, 5);
        assert!(merge_config(home, json!({"terminal": {"scrollbackBytes": 0}})).is_err());
        assert!(merge_config(home, json!({"terminal": {"scrollLinesPerStep": 0}})).is_err());
    }

    #[test]
    fn defaults_match_swarm_json_with_fleet_terminal_settings() {
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
                {"name": "lg", "command": "fleet://lazygit"}
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
            "discoveredWatches": {
                "enabled": true,
                "intervalMs": 2000,
                "processes": [
                    {"id":"codex-companion","pattern":"codex-companion\\.mjs task-worker","enabled":true},
                    {"id":"codex","pattern":"(^|/)codex( |$)","enabled":true},
                    {"id":"claude","pattern":"(^|/)claude( |$)","enabled":true},
                    {"id":"opencode","pattern":"(^|/)opencode( |$)","enabled":true}
                ]
            },
            "github": {"cacheTtlSeconds":3600,"prTtlSeconds":90,"cloneProtocol":"ssh"},
            "ui": {
                "statusRefreshMs":2000,
                "remoteStatusRefreshMs":10000,
                "notifications":{"toast":true,"sound":true}
            },
            "trash": {"retentionMs":600000},
            "jobs": {"warnBeforeQuit":true,"keepFinishedFor":600000},
            "terminal": {"scrollbackBytes":1073741824,"scrollLinesPerStep":3}
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_reserved_scheme_accepts_only_commands_fleet_provides() {
        let mut config = default_config("/home/me/.fleet");
        assert!(validate_config(&config).is_ok());

        config.windows[2].command = "fleet://gitui".to_owned();
        let error = validate_config(&config)
            .err()
            .unwrap_or_else(|| panic!("an unknown reserved command must be rejected"))
            .to_string();
        assert!(error.contains("fleet://gitui"), "{error}");
        assert!(error.contains("fleet://lazygit"), "{error}");

        // A plain program is never a reserved command, whatever it is called.
        config.windows[2].command = "lazygit".to_owned();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn an_imported_lazygit_window_becomes_the_native_pane() {
        let mut windows = vec![
            WindowConfig {
                name: "nvim".to_owned(),
                command: "nvim .".to_owned(),
            },
            WindowConfig {
                name: "lg".to_owned(),
                command: "lazygit".to_owned(),
            },
            WindowConfig {
                name: "lg2".to_owned(),
                command: "lazygit --use-config-file ~/.lg.yml".to_owned(),
            },
            WindowConfig {
                name: "wrapper".to_owned(),
                command: "my-lazygit".to_owned(),
            },
        ];
        normalize_imported_windows(&mut windows);
        assert_eq!(windows[0].command, "nvim .");
        assert_eq!(windows[1].command, NATIVE_LAZYGIT);
        assert_eq!(windows[2].command, NATIVE_LAZYGIT);
        assert_eq!(windows[3].command, "my-lazygit", "only `lazygit` upgrades");
        // The names and the order — which `ctrl-s <n>` counts — never move.
        assert_eq!(
            windows
                .iter()
                .map(|window| window.name.as_str())
                .collect::<Vec<_>>(),
            vec!["nvim", "lg", "lg2", "wrapper"]
        );
    }

    #[test]
    fn fleet_own_config_keeps_an_explicit_lazygit_binary() {
        // The opt-out: writing `lazygit` into Fleet's own config.json must survive a load.
        let config = merge_config_with_user_home(
            "/home/me/.fleet",
            "/home/me",
            json!({"windows": [{"name": "lg", "command": "lazygit"}]}),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.windows.len(), 1);
        assert_eq!(config.windows[0].command, "lazygit");
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
                "ui": {"statusRefreshMs": 1, "remoteStatusRefreshMs": 2,
                    "notifications": {"sound": false}}
            }),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.repos_dir, "/home/me/src");
        assert_eq!(config.agent_commands.opencode, "opencode");
        assert_eq!(config.sleep.grace_ms, 0);
        assert_eq!(config.ui.status_refresh_ms, 500);
        assert_eq!(config.ui.remote_status_refresh_ms, 500);
        assert!(config.ui.notifications.toast);
        assert!(!config.ui.notifications.sound);
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
    fn ui_notifications_default_when_older_config_omits_them() {
        let ui: UiConfig = serde_json::from_value(json!({
            "statusRefreshMs": 2_000,
            "remoteStatusRefreshMs": 10_000
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(ui.notifications, NotificationsConfig::default());
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
