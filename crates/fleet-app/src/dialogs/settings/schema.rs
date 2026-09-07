use super::*;

/// The sections of the rail, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Agent and its commands.
    General,
    /// Sleep policy and keep-alive rules.
    Sleep,
    /// Job retention and the quit warning.
    Jobs,
    /// The prepared-copy pool.
    Pool,
    /// GitHub caches and the clone protocol.
    Github,
    /// Status refresh intervals.
    Status,
    /// The default terminal layout (read-only).
    Windows,
    /// Configured remote hosts (read-only).
    Hosts,
    /// Versions, the daemon and the escape hatches.
    About,
}

impl Section {
    /// Every section, in rail order.
    pub const ALL: &'static [Self] = &[
        Self::General,
        Self::Sleep,
        Self::Jobs,
        Self::Pool,
        Self::Github,
        Self::Status,
        Self::Windows,
        Self::Hosts,
        Self::About,
    ];

    /// The rail label.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Sleep => "Sleep",
            Self::Jobs => "Jobs & warnings",
            Self::Pool => "Pool",
            Self::Github => "GitHub",
            Self::Status => "Status",
            Self::Windows => "Windows",
            Self::Hosts => "Hosts",
            Self::About => "About",
        }
    }

    /// Whether the section holds anything editable, which decides the `edit in config.json`
    /// trailer.
    #[must_use]
    pub const fn editable(self) -> bool {
        !matches!(self, Self::Windows | Self::Hosts | Self::About)
    }
}

/// Which setting a row edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowId {
    /// `agent`.
    Agent,
    /// `agentCommands.claude`.
    ClaudeCommand,
    /// `agentCommands.opencode`.
    OpencodeCommand,
    /// `sleep.enabled`.
    SleepOnSwitch,
    /// `sleep.graceMs`.
    GraceMs,
    /// `sleep.keepAlive[n].enabled`.
    KeepAliveRule(usize),
    /// `jobs.warnBeforeQuit`.
    WarnBeforeQuit,
    /// `jobs.keepFinishedFor`.
    KeepFinishedFor,
    /// `trash.retentionMs`.
    TrashRetention,
    /// `hotPoolSize`.
    HotPoolSize,
    /// `hotFreshnessMs`.
    HotFreshnessMs,
    /// `hotRefreshIntervalMs`.
    HotRefreshIntervalMs,
    /// `github.cloneProtocol`.
    CloneProtocol,
    /// `github.cacheTtlSeconds`.
    RepoCacheSeconds,
    /// `github.prTtlSeconds`.
    PrCacheSeconds,
    /// `ui.statusRefreshMs`.
    StatusRefreshMs,
    /// `ui.remoteStatusRefreshMs`.
    RemoteStatusRefreshMs,
    /// A row that cannot be edited here.
    ReadOnly,
}

/// What a row draws and how it is edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// A checkbox: `Space` toggles it.
    Toggle(bool),
    /// A closed list: `←` / `→` cycle it.
    Choice {
        /// The value shown.
        value: String,
        /// Whether a previous value exists.
        has_prev: bool,
        /// Whether a next value exists.
        has_next: bool,
        /// Whether the persisted value is outside the configured steps.
        off_grid: bool,
    },
    /// An integer: digits type into it.
    Number {
        /// The value.
        value: i64,
        /// The inclusive minimum.
        min: i64,
        /// The unit suffix.
        unit: Option<String>,
    },
    /// Free text: printable keys type into it.
    Text(String),
    /// A fact with no input chrome.
    Fact(String),
}

/// One row of the settings pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    /// Which setting this row edits.
    pub(crate) id: RowId,
    /// The row label.
    pub(crate) label: String,
    /// What it draws.
    pub(crate) kind: RowKind,
    /// The muted trailer, e.g. a live keep-alive match count.
    pub(crate) detail: Option<String>,
    /// A validation problem, e.g. an invalid keep-alive pattern.
    pub(crate) invalid: Option<String>,
}

/// Builds the rows of one section from the draft.
#[must_use]
pub fn rows(state: &SettingsState, app: &AppState) -> Vec<SettingRow> {
    let Some(config) = state.config.as_ref() else {
        return Vec::new();
    };
    match state.current_section() {
        Section::General => general_rows(config),
        Section::Sleep => sleep_rows(config, &state.matches),
        Section::Jobs => jobs_rows(config),
        Section::Pool => pool_rows(config, app),
        Section::Github => github_rows(config),
        Section::Status => status_rows(config),
        Section::Windows => window_rows(config),
        Section::Hosts => host_rows(config),
        Section::About => about_rows(app),
    }
}

fn general_rows(config: &Config) -> Vec<SettingRow> {
    vec![
        SettingRow {
            id: RowId::Agent,
            label: "Agent".to_owned(),
            kind: choice(agent_name(config.agent), AGENTS, agent_name(config.agent)),
            detail: None,
            invalid: None,
        },
        text_row(
            RowId::ClaudeCommand,
            "Claude command",
            &config.agent_commands.claude,
        ),
        text_row(
            RowId::OpencodeCommand,
            "OpenCode command",
            &config.agent_commands.opencode,
        ),
    ]
}

fn sleep_rows(config: &Config, matches: &[KeepAliveRuleMatch]) -> Vec<SettingRow> {
    let mut list = vec![
        toggle_row(
            RowId::SleepOnSwitch,
            "Sleep on switch",
            config.sleep.enabled,
        ),
        number_row(RowId::GraceMs, "Grace", config.sleep.grace_ms, 0, "ms"),
    ];
    list.extend(
        config
            .sleep
            .keep_alive
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                let live = matches.iter().find(|entry| entry.rule_id == rule.id);
                let kind = match rule.kind {
                    KeepAliveKind::Process => "process",
                    KeepAliveKind::ListeningPort => "listening-port",
                };
                SettingRow {
                    id: RowId::KeepAliveRule(index),
                    label: format!("{}  {kind}  {}", rule.label, rule.pattern),
                    kind: RowKind::Toggle(rule.enabled),
                    detail: live.map(|entry| {
                        format!(
                            "{} \u{2014} matching {} processes now",
                            rule.label, entry.count
                        )
                    }),
                    invalid: live
                        .and_then(|entry| entry.error.as_ref())
                        .map(|_| "invalid pattern \u{2014} rule is skipped".to_owned()),
                }
            }),
    );
    list
}

fn jobs_rows(config: &Config) -> Vec<SettingRow> {
    vec![
        toggle_row(
            RowId::WarnBeforeQuit,
            "Warn before quitting with running jobs",
            config.jobs.warn_before_quit,
        ),
        SettingRow {
            id: RowId::KeepFinishedFor,
            label: "Keep finished jobs for".to_owned(),
            kind: duration_choice(config.jobs.keep_finished_for),
            detail: None,
            invalid: None,
        },
        SettingRow {
            id: RowId::TrashRetention,
            label: "Trash retention".to_owned(),
            kind: duration_choice(config.trash.retention_ms),
            detail: None,
            invalid: None,
        },
    ]
}

fn pool_rows(config: &Config, app: &AppState) -> Vec<SettingRow> {
    let (ready, size) = app.snapshot.as_ref().map_or((0, 0), |snapshot| {
        snapshot.pools.iter().fold((0, 0), |(ready, size), pool| {
            (ready + pool.ready, size + pool.size)
        })
    });
    vec![
        SettingRow {
            id: RowId::HotPoolSize,
            label: "Hot pool size".to_owned(),
            kind: pool_choice(config.hot_pool_size),
            detail: None,
            invalid: None,
        },
        number_row(
            RowId::HotFreshnessMs,
            "Freshness",
            i64::try_from(config.hot_freshness_ms).unwrap_or(i64::MAX),
            0,
            "ms",
        ),
        number_row(
            RowId::HotRefreshIntervalMs,
            "Refresh interval",
            i64::try_from(config.hot_refresh_interval_ms).unwrap_or(i64::MAX),
            0,
            "ms",
        ),
        fact("prepared copies", format!("{ready}/{size} ready")),
    ]
}

fn github_rows(config: &Config) -> Vec<SettingRow> {
    let protocol = protocol_name(config.github.clone_protocol);
    vec![
        SettingRow {
            id: RowId::CloneProtocol,
            label: "Clone protocol".to_owned(),
            kind: choice(protocol, PROTOCOLS, protocol),
            detail: None,
            invalid: None,
        },
        number_row(
            RowId::RepoCacheSeconds,
            "Repo cache",
            config.github.cache_ttl_seconds,
            0,
            "s",
        ),
        number_row(
            RowId::PrCacheSeconds,
            "PR cache",
            config.github.pr_ttl_seconds,
            0,
            "s",
        ),
    ]
}

fn status_rows(config: &Config) -> Vec<SettingRow> {
    vec![
        number_row(
            RowId::StatusRefreshMs,
            "Local status refresh",
            config.ui.status_refresh_ms,
            500,
            "ms",
        ),
        number_row(
            RowId::RemoteStatusRefreshMs,
            "Remote status refresh",
            config.ui.remote_status_refresh_ms,
            500,
            "ms",
        ),
    ]
}

fn window_rows(config: &Config) -> Vec<SettingRow> {
    config
        .windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            // A reserved `fleet://` command is not a program the user could run, so the row
            // says what it *is* instead of repeating a URL nobody can type into a shell. The
            // raw value is still one `E` away in `config.json`.
            let command = if fleet_core::config::is_native_command(&window.command) {
                format!("{} (built in)", window.command)
            } else {
                window.command.clone()
            };
            fact(
                &format!("{}", index + 1),
                format!("{} \u{2014} {command}", window.name),
            )
        })
        .collect()
}

fn host_rows(config: &Config) -> Vec<SettingRow> {
    if config.hosts.is_empty() {
        return vec![fact("hosts", "none configured".to_owned())];
    }
    config
        .hosts
        .iter()
        .map(|(id, host)| {
            fact(
                id.as_str(),
                format!("{} \u{2014} {}", host.ssh, host.swarm_command),
            )
        })
        .collect()
}

/// The About section: versions, the daemon, and the two escape hatches.
pub(super) fn about_rows(app: &AppState) -> Vec<SettingRow> {
    let mut list = Vec::new();
    let now = now_unix();
    if let Some(snapshot) = app.snapshot.as_ref() {
        list.push(fact(
            "Fleet",
            crate::presentation::bare_version(&snapshot.daemon.version).to_owned(),
        ));
        let uptime = age_secs(&snapshot.daemon.started_at, now)
            .map_or_else(|| "\u{2013}".to_owned(), fleet_ui_kit::format_age);
        list.push(fact(
            "fleetd",
            format!(
                "running \u{00b7} pid {} \u{00b7} up {uptime}",
                snapshot.daemon.pid
            ),
        ));
        list.push(fact("FLEET_HOME", snapshot.daemon.home.clone()));
    } else {
        list.push(fact("fleetd", "not connected".to_owned()));
        list.push(fact("FLEET_HOME", app.home.display().to_string()));
    }
    if let Some(version) = app.update_version.as_ref() {
        list.push(fact(
            "Update",
            format!("Fleet {version} available \u{00b7} U"),
        ));
    }
    list.push(fact(
        "protocol",
        crate::dialogs::help::protocol().to_string(),
    ));
    list.push(fact(
        "E",
        "open config.json in a new terminal tab".to_owned(),
    ));
    list.push(fact("D", "run doctor".to_owned()));
    list
}

pub(super) const AGENTS: &[&str] = &["claude", "opencode"];
pub(super) const PROTOCOLS: &[&str] = &["ssh", "https"];
/// The `Keep finished jobs for` and `Trash retention` steps, in milliseconds.
pub(super) const DURATIONS: &[u64] = &[60_000, 300_000, 600_000, 1_800_000, 3_600_000];
/// The `Hot pool size` steps.
pub(super) const POOL_SIZES: &[u64] = &[0, 1, 2, 3];

pub(super) const fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Opencode => "opencode",
    }
}

pub(super) const fn protocol_name(protocol: CloneProtocol) -> &'static str {
    match protocol {
        CloneProtocol::Ssh => "ssh",
        CloneProtocol::Https => "https",
    }
}

pub(super) fn choice(value: &str, options: &[&str], current: &str) -> RowKind {
    let index = options.iter().position(|option| *option == current);
    RowKind::Choice {
        value: value.to_owned(),
        has_prev: index.is_some_and(|index| index > 0),
        has_next: index.is_some_and(|index| index + 1 < options.len()),
        off_grid: index.is_none(),
    }
}

pub(super) fn duration_choice(value: u64) -> RowKind {
    let index = DURATIONS.iter().position(|step| *step == value);
    RowKind::Choice {
        value: format_duration(value),
        has_prev: index.is_some_and(|index| index > 0),
        has_next: index.is_some_and(|index| index + 1 < DURATIONS.len()),
        off_grid: index.is_none(),
    }
}

pub(super) fn pool_choice(value: u64) -> RowKind {
    let index = POOL_SIZES.iter().position(|step| *step == value);
    RowKind::Choice {
        value: value.to_string(),
        has_prev: index.is_some_and(|index| index > 0),
        has_next: index.is_some_and(|index| index + 1 < POOL_SIZES.len()),
        off_grid: index.is_none(),
    }
}

/// A millisecond duration as the cycler words it (`10 min`).
#[must_use]
pub fn format_duration(millis: u64) -> String {
    let seconds = millis / 1_000;
    if seconds >= 3_600 && seconds.is_multiple_of(3_600) {
        format!("{} h", seconds / 3_600)
    } else if seconds >= 60 && seconds.is_multiple_of(60) {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

/// A read-only row: a label and the value it states.
fn fact(label: &str, value: String) -> SettingRow {
    SettingRow {
        id: RowId::ReadOnly,
        label: label.to_owned(),
        kind: RowKind::Fact(value),
        detail: None,
        invalid: None,
    }
}

fn toggle_row(id: RowId, label: &str, checked: bool) -> SettingRow {
    SettingRow {
        id,
        label: label.to_owned(),
        kind: RowKind::Toggle(checked),
        detail: None,
        invalid: None,
    }
}

pub(super) fn text_row(id: RowId, label: &str, value: &str) -> SettingRow {
    SettingRow {
        id,
        label: label.to_owned(),
        kind: RowKind::Text(value.to_owned()),
        detail: None,
        invalid: None,
    }
}

pub(super) fn number_row(id: RowId, label: &str, value: i64, min: i64, unit: &str) -> SettingRow {
    SettingRow {
        id,
        label: label.to_owned(),
        kind: RowKind::Number {
            value,
            min,
            unit: Some(unit.to_owned()),
        },
        detail: None,
        invalid: None,
    }
}

pub(crate) fn refresh_rows(state: &Entity<AppState>, cx: &mut App) {
    let prepared = read_host(state, cx, |host, cx| {
        rows(&host.settings, state.read(cx)).into()
    });
    with_host(state, cx, |host| host.settings.prepared = prepared);
}
