use super::*;

use fleet_proto::snapshot::{HostStatus, LinkState};

/// The sections of the rail, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// The terminal tabs a new worktree opens with (read-only).
    General,
    /// The default agent and each harness's command, binary and thread defaults.
    Agents,
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
    /// Configured remote hosts (read-only).
    Hosts,
    /// Versions, the daemon and the escape hatches.
    About,
}

impl Section {
    /// Every section, in rail order.
    pub const ALL: &'static [Self] = &[
        Self::General,
        Self::Agents,
        Self::Sleep,
        Self::Jobs,
        Self::Pool,
        Self::Github,
        Self::Status,
        Self::Hosts,
        Self::About,
    ];

    /// The rail label.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Agents => "Agents",
            Self::Sleep => "Sleep",
            Self::Jobs => "Jobs & warnings",
            Self::Pool => "Pool",
            Self::Github => "GitHub",
            Self::Status => "Status",
            Self::Hosts => "Hosts",
            Self::About => "About",
        }
    }

    /// The rail glyph.
    #[must_use]
    pub const fn icon(self) -> Icon {
        match self {
            Self::General => Icon::Settings2,
            Self::Agents => Icon::Bot,
            Self::Sleep => Icon::Moon,
            Self::Jobs => Icon::LoaderCircle,
            Self::Pool => Icon::Boxes,
            Self::Github => Icon::GitPullRequest,
            Self::Status => Icon::Activity,
            Self::Hosts => Icon::Server,
            Self::About => Icon::Info,
        }
    }

    /// The one sentence the pane ends with, stated once under the section's cards.
    #[must_use]
    pub const fn caption(self) -> Option<&'static str> {
        match self {
            Self::General | Self::Hosts => Some("Change these in config.json."),
            Self::Sleep => Some(
                "Rules are defined in config.json. Open it from the footer to add or change one.",
            ),
            _ => None,
        }
    }

    /// The section's position in [`Self::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|section| *section == self)
            .unwrap_or(0)
    }
}

/// Which setting a row edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowId {
    /// `agent`.
    Agent,
    /// `agentCommands.claude`.
    ClaudeCommand,
    /// `agentCommands.codex`.
    CodexCommand,
    /// `agentBinaries.claude`.
    ClaudeBinary,
    /// `agentBinaries.codex`.
    CodexBinary,
    /// `nativeAgents.claude.mode`.
    ClaudeDefaultMode,
    /// `nativeAgents.claude.model`.
    ClaudeDefaultModel,
    /// `nativeAgents.claude.effort`.
    ClaudeDefaultEffort,
    /// `nativeAgents.codex.mode`.
    CodexDefaultMode,
    /// `nativeAgents.codex.model`.
    CodexDefaultModel,
    /// `nativeAgents.codex.effort`.
    CodexDefaultEffort,
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
        /// Every step in cycling order, worded as `value` is; the control draws them side by
        /// side when they are few enough.
        options: Vec<String>,
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
    /// A harness's default model: a dropdown over the models it reported, or a text box when it
    /// has reported none. `⏎` opens the text box either way, for a model id the list lacks.
    Model {
        /// The configured model id; empty leaves the choice to the harness.
        value: String,
        /// What the dropdown reads: the model's name, the raw id, or `Harness default`.
        shown: String,
        /// The models the harness reported, in its order. Empty draws the text box.
        options: Vec<ModelOption>,
        /// The harness, as the helper and the menu name it (`claude`).
        harness: &'static str,
    },
    /// A keep-alive rule: its switch leads the row, and the live match count ends it.
    Rule {
        /// Whether the rule takes part in matching.
        on: bool,
        /// What the rule inspects, as its badge reads (`command`, `port`).
        badge: &'static str,
        /// The command pattern; empty for a port rule, which ignores it.
        pattern: String,
        /// How many running processes match now; `None` while that is still loading.
        running: Option<u64>,
        /// The pattern failed to compile, so the daemon skips the rule.
        broken: bool,
    },
    /// A fact with no input chrome.
    Fact(String),
}

/// One model a harness reported, as the Default model dropdown lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// The id written to the configuration.
    pub id: String,
    /// The harness's own name for it; the id when the harness gave none.
    pub name: String,
}

impl RowKind {
    /// Whether a Default model row draws its dropdown: the harness reported models and no id is
    /// being typed. Otherwise the row draws the text box every text row draws.
    #[must_use]
    pub fn draws_model_dropdown(&self, editing: bool) -> bool {
        matches!(self, Self::Model { options, .. } if !options.is_empty()) && !editing
    }

    /// Whether `⏎` (or a click on its box) opens an editor on the row.
    #[must_use]
    pub const fn opens_editor(&self) -> bool {
        matches!(
            self,
            Self::Text(_) | Self::Number { .. } | Self::Model { .. }
        )
    }
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
    /// The helper sentence under the label.
    pub(crate) detail: Option<String>,
    /// A validation problem. A keep-alive rule's is the sentence its card ends with.
    pub(crate) invalid: Option<String>,
    /// The card the row sits in, named by its heading (`Claude`); consecutive rows sharing one
    /// are drawn together.
    pub(crate) card: Option<&'static str>,
    /// What an empty text value means (`Harness default`).
    pub(crate) placeholder: Option<&'static str>,
    /// The text a copy button beside a read-only value puts on the clipboard.
    pub(crate) copy: Option<String>,
    /// A read-only value that is a path, an id or a version, drawn in the data face.
    pub(crate) mono: bool,
}

impl SettingRow {
    fn new(id: RowId, label: &str, kind: RowKind) -> Self {
        Self {
            id,
            label: label.to_owned(),
            kind,
            detail: None,
            invalid: None,
            card: None,
            placeholder: None,
            copy: None,
            mono: false,
        }
    }

    /// Attaches the helper sentence drawn under the row.
    fn detail(mut self, detail: &str) -> Self {
        self.detail = Some(detail.to_owned());
        self
    }

    fn card(mut self, card: &'static str) -> Self {
        self.card = Some(card);
        self
    }

    fn placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    fn copy(mut self, copy: String) -> Self {
        self.copy = Some(copy);
        self
    }

    fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

/// The muted note a card's header ends with, by the card's heading.
#[must_use]
pub fn card_note(card: &str) -> Option<&'static str> {
    (card == KEEP_AWAKE).then_some("matched against running processes now")
}

/// The sentences a card ends with: one per keep-alive rule in it whose pattern does not compile.
#[must_use]
pub fn card_captions(rows: &[SettingRow]) -> Vec<String> {
    rows.iter()
        .filter(|row| matches!(row.kind, RowKind::Rule { broken: true, .. }))
        .filter_map(|row| row.invalid.clone())
        .collect()
}

/// The heading of the Sleep section's keep-alive card.
pub(super) const KEEP_AWAKE: &str = "Keep awake while running";

/// Builds the rows of the section the rail highlights.
#[must_use]
pub fn rows(state: &SettingsState, app: &AppState) -> Vec<SettingRow> {
    section_rows(state.current_section(), state, app)
}

/// Builds the rows of one section from the draft.
#[must_use]
pub fn section_rows(section: Section, state: &SettingsState, app: &AppState) -> Vec<SettingRow> {
    let Some(config) = state.config.as_ref() else {
        return Vec::new();
    };
    match section {
        Section::General => window_rows(config),
        Section::Agents => agent_rows(config, &state.efforts, &state.models),
        Section::Sleep => sleep_rows(config, &state.matches, state.matches_loading),
        Section::Jobs => jobs_rows(config),
        Section::Pool => pool_rows(config, app),
        Section::Github => github_rows(config),
        Section::Status => status_rows(config),
        Section::Hosts => host_rows(config, app),
        Section::About => about_rows(app),
    }
}

/// A choice row, drawn from [`choice_of`].
fn choice_row(config: &Config, id: RowId, label: &str, efforts: &Efforts) -> SettingRow {
    let kind =
        choice_of(config, &id, efforts).map_or_else(|| RowKind::Fact(String::new()), Choice::kind);
    SettingRow::new(id, label, kind)
}

/// The Agents section: the default agent, then one card per harness.
fn agent_rows(config: &Config, efforts: &Efforts, models: &Models) -> Vec<SettingRow> {
    let mut rows = vec![
        choice_row(config, RowId::Agent, "Default agent", efforts)
            .detail("Used by the Agent buttons and by a new thread."),
    ];
    for kind in [AgentKind::Claude, AgentKind::Codex] {
        let (card, command, binary, mode, model, effort) = match kind {
            AgentKind::Claude => (
                "Claude",
                (RowId::ClaudeCommand, &config.agent_commands.claude),
                (RowId::ClaudeBinary, &config.agent_binaries.claude),
                RowId::ClaudeDefaultMode,
                RowId::ClaudeDefaultModel,
                RowId::ClaudeDefaultEffort,
            ),
            AgentKind::Codex => (
                "Codex",
                (RowId::CodexCommand, &config.agent_commands.codex),
                (RowId::CodexBinary, &config.agent_binaries.codex),
                RowId::CodexDefaultMode,
                RowId::CodexDefaultModel,
                RowId::CodexDefaultEffort,
            ),
        };
        let model_kind = model_of(config, kind, models);
        let model_helper = match &model_kind {
            RowKind::Model {
                options, harness, ..
            } if !options.is_empty() => {
                format!("The models {harness} reported. Harness default lets {harness} pick.")
            }
            _ => "Empty means the harness picks.".to_owned(),
        };
        let mut model_row = SettingRow::new(model, "Default model", model_kind)
            .placeholder("Harness default")
            .card(card);
        model_row.detail = Some(model_helper);
        rows.extend([
            text_row(command.0, "Terminal command", command.1)
                .detail("Typed into a terminal tab. Aliases work.")
                .card(card),
            text_row(binary.0, "Binary for threads", binary.1)
                .detail("Run directly for an agent thread, without a shell.")
                .card(card),
            choice_row(config, mode, "Default access", efforts)
                .detail("New threads start with it. Each thread can change it.")
                .card(card),
            model_row,
            choice_row(config, effort, "Effort", efforts)
                .detail("Used with the default model.")
                .card(card),
        ]);
    }
    rows
}

fn sleep_rows(config: &Config, matches: &[KeepAliveRuleMatch], loading: bool) -> Vec<SettingRow> {
    let mut list = vec![
        toggle_row(
            RowId::SleepOnSwitch,
            "Sleep on switch",
            config.sleep.enabled,
        )
        .detail(
            "Opening another worktree sleeps the one you leave. Agents, servers and unsaved \
             editors stay.",
        ),
        number_row(RowId::GraceMs, "Grace", config.sleep.grace_ms, 0, "ms")
            .detail("How long a terminal gets to finish before sleep closes it."),
    ];
    list.extend(
        config
            .sleep
            .keep_alive
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                let live = matches.iter().find(|entry| entry.rule_id == rule.id);
                let broken = live.is_some_and(|entry| entry.error.is_some());
                let (badge, pattern) = match rule.kind {
                    KeepAliveKind::Process => ("command", rule.pattern.clone()),
                    // A port rule ignores its pattern (`KeepAliveRule::pattern`).
                    KeepAliveKind::ListeningPort => ("port", String::new()),
                };
                let mut row = SettingRow::new(
                    RowId::KeepAliveRule(index),
                    &rule.label,
                    RowKind::Rule {
                        on: rule.enabled,
                        badge,
                        pattern,
                        running: live
                            .filter(|entry| entry.error.is_none() && !loading)
                            .map(|entry| entry.count),
                        broken,
                    },
                );
                if rule.kind == KeepAliveKind::ListeningPort {
                    row.detail = Some("Any process listening on a port.".to_owned());
                }
                row.invalid = broken.then(|| {
                    format!(
                        "{}: the pattern does not compile, so the rule is skipped.",
                        rule.label
                    )
                });
                row.card(KEEP_AWAKE)
            }),
    );
    list
}

fn jobs_rows(config: &Config) -> Vec<SettingRow> {
    let efforts = Efforts::default();
    vec![
        toggle_row(
            RowId::WarnBeforeQuit,
            "Warn before quitting with running jobs",
            config.jobs.warn_before_quit,
        )
        .detail("They keep running in fleetd either way."),
        choice_row(
            config,
            RowId::KeepFinishedFor,
            "Keep finished jobs for",
            &efforts,
        )
        .detail("How long a finished job stays in the jobs list."),
        choice_row(config, RowId::TrashRetention, "Trash retention", &efforts)
            .detail("How long something deleted can still be restored."),
    ]
}

fn pool_rows(config: &Config, app: &AppState) -> Vec<SettingRow> {
    let (ready, size) = app.snapshot.as_ref().map_or((0, 0), |snapshot| {
        snapshot.pools.iter().fold((0, 0), |(ready, size), pool| {
            (ready + pool.ready, size + pool.size)
        })
    });
    vec![
        choice_row(
            config,
            RowId::HotPoolSize,
            "Hot pool size",
            &Efforts::default(),
        )
        .detail("How many prepared copies each repository keeps ready."),
        number_row(
            RowId::HotFreshnessMs,
            "Freshness",
            i64::try_from(config.hot_freshness_ms).unwrap_or(i64::MAX),
            0,
            "ms",
        )
        .detail("How old a prepared copy may be and still count as fresh."),
        number_row(
            RowId::HotRefreshIntervalMs,
            "Refresh interval",
            i64::try_from(config.hot_refresh_interval_ms).unwrap_or(i64::MAX),
            0,
            "ms",
        )
        .detail("How often a prepared copy is brought up to date with its base branch."),
        fact("prepared copies", format!("{ready}/{size} ready")),
    ]
}

fn github_rows(config: &Config) -> Vec<SettingRow> {
    vec![
        choice_row(
            config,
            RowId::CloneProtocol,
            "Clone protocol",
            &Efforts::default(),
        )
        .detail("Which URL a clone is made from."),
        number_row(
            RowId::RepoCacheSeconds,
            "Repo cache",
            config.github.cache_ttl_seconds,
            0,
            "s",
        )
        .detail("How long the list of discovered repositories is kept."),
        number_row(
            RowId::PrCacheSeconds,
            "PR cache",
            config.github.pr_ttl_seconds,
            0,
            "s",
        )
        .detail("How long pull requests are kept before they are fetched again."),
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
        )
        .detail("How often local sessions and worktrees are polled."),
        number_row(
            RowId::RemoteStatusRefreshMs,
            "Remote status refresh",
            config.ui.remote_status_refresh_ms,
            500,
            "ms",
        )
        .detail("How often hosts on other machines are polled."),
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
            .mono()
            .card("Terminal tabs a new worktree opens")
        })
        .collect()
}

/// The Hosts section: `defaultHost`, then one read-only row per configured machine.
///
/// The row is the configured shape (provider, node, user) joined to what the daemon last saw
/// of that machine, because a host entry that looks right and answers nothing is the whole
/// reason this section exists. A legacy entry is named as such: it is probe-only and cannot
/// carry worktrees, so it needs migrating rather than debugging.
pub(super) fn host_rows(config: &Config, app: &AppState) -> Vec<SettingRow> {
    let mut rows = vec![fact("default", config.default_host.clone()).mono()];
    if config.hosts.is_empty() {
        rows.push(fact("hosts", "none configured".to_owned()));
        return rows;
    }
    let statuses = app
        .snapshot
        .as_ref()
        .map_or::<&[HostStatus], _>(&[], |snapshot| snapshot.hosts.as_slice());
    rows.extend(config.hosts.iter().map(|(id, host)| {
        let status = statuses.iter().find(|status| &status.id == id);
        fact(
            id.as_str(),
            format!(
                "{} \u{2014} {}",
                host_shape(id, host),
                host_link(host, status)
            ),
        )
    }));
    rows
}

/// The configured shape of one host, in the config's own vocabulary.
fn host_shape(id: &fleet_core::ids::HostId, host: &fleet_core::model::HostConfigEntry) -> String {
    match host {
        fleet_core::model::HostConfigEntry::Tailscale { node, user, .. } => {
            let mut line = format!("tailscale \u{00b7} node {node}");
            if let Some(user) = user {
                line.push_str(&format!(" \u{00b7} user {user}"));
            }
            line
        }
        fleet_core::model::HostConfigEntry::Command { display, .. } => format!(
            "command \u{00b7} {}",
            display.as_deref().unwrap_or(id.as_str())
        ),
        fleet_core::model::HostConfigEntry::Legacy { ssh, swarm_command } => {
            format!("legacy \u{00b7} ssh {ssh} \u{00b7} {swarm_command}")
        }
    }
}

/// What the daemon last saw of one host.
fn host_link(host: &fleet_core::model::HostConfigEntry, status: Option<&HostStatus>) -> String {
    if matches!(host, fleet_core::model::HostConfigEntry::Legacy { .. }) {
        return "legacy entry \u{2014} migrate it to a tailscale host".to_owned();
    }
    let Some(status) = status else {
        return "no status yet".to_owned();
    };
    let detail = status
        .error
        .as_deref()
        .filter(|error| !error.is_empty())
        .map(|error| format!(" \u{00b7} {error}"))
        .unwrap_or_default();
    match status.link {
        LinkState::Ready => match status.version.as_deref() {
            Some(version) => format!("ready \u{00b7} fleetd {version}"),
            None => "ready".to_owned(),
        },
        LinkState::Connecting => format!("connecting{detail}"),
        LinkState::Down => format!("down{detail}"),
        LinkState::Legacy => "legacy entry \u{2014} migrate it to a tailscale host".to_owned(),
    }
}

/// The About section: versions and the daemon. The two escape hatches, `config.json` and doctor,
/// are the footer's buttons.
pub(super) fn about_rows(app: &AppState) -> Vec<SettingRow> {
    let mut list = vec![
        fact(
            "Fleet",
            crate::presentation::bare_version(env!("CARGO_PKG_VERSION")).to_owned(),
        )
        .mono(),
    ];
    let now = now_unix();
    if let Some(snapshot) = app.snapshot.as_ref().filter(|_| app.daemon.is_connected()) {
        let uptime = age_secs(&snapshot.daemon.started_at, now)
            .map_or_else(|| "\u{2013}".to_owned(), fleet_ui_kit::format_age);
        list.push(
            fact(
                "fleetd",
                format!(
                    "running \u{00b7} pid {} \u{00b7} up {uptime}",
                    snapshot.daemon.pid
                ),
            )
            .mono()
            .copy(snapshot.daemon.pid.to_string()),
        );
    } else {
        list.push(fact("fleetd", "not connected".to_owned()));
    }
    let home = app.home.display().to_string();
    list.push(fact("FLEET_HOME", home.clone()).mono().copy(home));
    if let Some(version) = app.update_version.as_ref() {
        list.push(fact(
            "Update",
            format!("Fleet {version} available \u{00b7} U"),
        ));
    }
    list.push(fact("protocol", crate::dialogs::help::protocol().to_string()).mono());
    list
}

pub(super) const AGENTS: &[&str] = &["claude", "codex"];
pub(super) const PROTOCOLS: &[&str] = &["ssh", "https"];
/// The `Keep finished jobs for` and `Trash retention` steps, in milliseconds.
pub(super) const DURATIONS: &[u64] = &[60_000, 300_000, 600_000, 1_800_000, 3_600_000];
/// The `Hot pool size` steps.
pub(super) const POOL_SIZES: &[u64] = &[0, 1, 2, 3];

pub(super) const fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        // ADR 0014 keeps the legacy value decodable for one release. The row names what the
        // config actually says rather than the harness that replaced it, because cycling the
        // row is what moves it forward and a relabelled value would hide that it needs to.
        Agent::Opencode => "opencode",
    }
}

pub(super) const fn protocol_name(protocol: CloneProtocol) -> &'static str {
    match protocol {
        CloneProtocol::Ssh => "ssh",
        CloneProtocol::Https => "https",
    }
}

/// A millisecond duration as the settings cycler words it (`10 min`).
///
/// Deliberately not [`fleet_ui_kit::format_duration`], which words the same number the way an
/// agent turn footer does (`48s`): one states a configured interval a user picks from a list,
/// the other states elapsed work.
#[must_use]
pub fn format_cycler_duration(millis: u64) -> String {
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
    SettingRow::new(RowId::ReadOnly, label, RowKind::Fact(value))
}

fn toggle_row(id: RowId, label: &str, checked: bool) -> SettingRow {
    SettingRow::new(id, label, RowKind::Toggle(checked))
}

pub(super) fn text_row(id: RowId, label: &str, value: &str) -> SettingRow {
    SettingRow::new(id, label, RowKind::Text(value.to_owned()))
}

pub(super) fn number_row(id: RowId, label: &str, value: i64, min: i64, unit: &str) -> SettingRow {
    SettingRow::new(
        id,
        label,
        RowKind::Number {
            value,
            min,
            unit: Some(unit.to_owned()),
        },
    )
}

/// Rebuilds the pane's rows, and the search hits when a search is typed.
pub(crate) fn refresh_rows(state: &Entity<AppState>, cx: &mut App) {
    let efforts = Efforts::declared(state.read(cx));
    let models = Models::declared(state.read(cx));
    with_host(state, cx, |host| {
        host.settings.efforts = efforts;
        host.settings.models = models;
    });
    let (prepared, hits) = read_host(state, cx, |host, cx| {
        let app = state.read(cx);
        let prepared: std::rc::Rc<[SettingRow]> = rows(&host.settings, app).into();
        let hits = host
            .settings
            .search
            .as_ref()
            .map(|typed| search(&typed.query, &host.settings, app));
        (prepared, hits)
    });
    with_host(state, cx, |host| {
        host.settings.prepared = prepared;
        if let (Some(search), Some((hits, total))) = (host.settings.search.as_mut(), hits) {
            search.set_hits(hits);
            search.total = total;
        }
    });
}

/// Every row of every section whose label or helper sentence contains each word of `query`,
/// ignoring case, in rail order. An empty query matches nothing: the pane shows the section.
#[cfg(test)]
#[must_use]
pub fn search_hits(query: &str, state: &SettingsState, app: &AppState) -> Vec<SearchHit> {
    search(query, state, app).0
}

/// [`search_hits`], and how many rows every section holds, which the field's count divides by.
#[must_use]
pub fn search(query: &str, state: &SettingsState, app: &AppState) -> (Vec<SearchHit>, usize) {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut hits = Vec::new();
    let mut total = 0;
    for section in Section::ALL {
        let rows = section_rows(*section, state, app);
        total += rows.len();
        if words.is_empty() {
            continue;
        }
        for (row, entry) in rows.into_iter().enumerate() {
            let haystack = format!(
                "{} {} {} {}",
                section.title(),
                entry.card.unwrap_or(""),
                entry.label,
                entry.detail.as_deref().unwrap_or("")
            )
            .to_lowercase();
            if words.iter().all(|word| haystack.contains(word.as_str())) {
                let label = match entry.card {
                    Some(card) => format!("{card} \u{203a} {}", entry.label),
                    None => entry.label,
                };
                hits.push(SearchHit {
                    section: *section,
                    row,
                    spans: label_spans(&label, &words),
                    label,
                    detail: entry.detail,
                });
            }
        }
    }
    (hits, total)
}

/// One row a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// The section it lives in.
    pub section: Section,
    /// Its position in that section.
    pub row: usize,
    /// Its label, led by its card's heading when it sits in one (`Claude › Effort`).
    pub label: String,
    /// The label cut into spans, `true` marking a typed word's occurrence, which the row draws in
    /// the strong face. Joined, they are `label`.
    pub spans: Vec<(String, bool)>,
    /// Its helper sentence.
    pub detail: Option<String>,
}

/// `label` cut where the typed `words` (already lowercased) occur in it, case-insensitively:
/// `true` spans are matches, merged where they touch or overlap. The spans concatenate back to
/// exactly `label`; a label nothing matches is one plain span.
#[must_use]
pub fn label_spans(label: &str, words: &[String]) -> Vec<(String, bool)> {
    // Lowercasing can change a character's byte length, so the search runs over a lowercase
    // copy and maps every match back through each original character's offset in it.
    let chars: Vec<char> = label.chars().collect();
    let mut lowered = String::with_capacity(label.len());
    let mut starts = Vec::with_capacity(chars.len());
    for c in &chars {
        starts.push(lowered.len());
        lowered.extend(c.to_lowercase());
    }
    starts.push(lowered.len());
    let mut strong = vec![false; chars.len()];
    for word in words.iter().filter(|word| !word.is_empty()) {
        for (at, found) in lowered.match_indices(word.as_str()) {
            let end = at + found.len();
            for (index, flag) in strong.iter_mut().enumerate() {
                if starts[index] < end && starts[index + 1] > at {
                    *flag = true;
                }
            }
        }
    }
    let mut spans: Vec<(String, bool)> = Vec::new();
    for (c, is_strong) in chars.iter().zip(strong) {
        match spans.last_mut() {
            Some((text, last)) if *last == is_strong => text.push(*c),
            _ => spans.push((c.to_string(), is_strong)),
        }
    }
    if spans.is_empty() {
        spans.push((String::new(), false));
    }
    spans
}
