//! §3.8.6 Settings (`,`) — a 180 px section rail beside a 540 px pane.
//!
//! Editable fields carry a value in an input box; read-only facts are muted text **with no
//! input chrome**, because the absence of a box is how "you cannot edit this here" is said.
//!
//! **[D-13]**: everything a user changes more than once a year is editable here. `windows` and
//! `hosts` stay read-only in v1 (both are ordered/keyed structures whose editor is a whole
//! screen) and so do the keep-alive *patterns*; their escape hatch is `E`, one key, which opens
//! `config.json`. A rule's `enabled` flag **is** editable, because that is the switch people
//! actually flip, and it is shown with the live match count that explains what it does.

use fleet_core::{
    config::{Agent, CloneProtocol, Config},
    sleep::KeepAliveKind,
};
use fleet_proto::{
    request::RequestBody,
    response::{KeepAliveRuleMatch, ResponseBody},
};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, KeyDownEvent, Window, div, px};

use crate::{
    actions::{dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{TextInput, notify, root, step, typed_char, uptime_label, with_host},
    state::{AppState, Screen},
    views::workspace_tabs,
};

/// The section rail's width (§3.8.6).
const RAIL_WIDTH: f32 = 180.0;

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
    pub id: RowId,
    /// The row label.
    pub label: String,
    /// What it draws.
    pub kind: RowKind,
    /// The muted trailer, e.g. a live keep-alive match count.
    pub detail: Option<String>,
    /// A validation problem, e.g. an invalid keep-alive pattern.
    pub invalid: Option<String>,
}

impl SettingRow {
    /// Whether typing goes into this row rather than moving the cursor (§3.8.6: `j` / `k` are
    /// surrendered while a text input has focus).
    #[must_use]
    pub const fn owns_typing(&self) -> bool {
        matches!(self.kind, RowKind::Text(_) | RowKind::Number { .. })
    }
}

/// The Settings dialog's draft.
#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    /// The configuration as it will be saved.
    pub config: Option<Config>,
    /// The configuration as it was loaded, for the dirty check.
    pub original: Option<Config>,
    /// Which section the rail highlights.
    pub section: usize,
    /// Which row the pane's cursor is on.
    pub row: usize,
    /// The live keep-alive match counts.
    pub matches: Vec<KeepAliveRuleMatch>,
    /// The buffer backing the focused text or number row.
    pub editing: Option<TextInput>,
    /// The exact error from a refused write.
    pub error: Option<String>,
    /// Bumps on every seed; a late answer to a superseded dialog is dropped.
    pub seq: u64,
}

impl SettingsState {
    /// The section the rail highlights.
    #[must_use]
    pub fn current_section(&self) -> Section {
        Section::ALL
            .get(self.section)
            .copied()
            .unwrap_or(Section::General)
    }

    /// Whether the draft differs from what was loaded (§3.8.6 dirty state).
    #[must_use]
    pub fn dirty(&self) -> bool {
        match (&self.config, &self.original) {
            (Some(config), Some(original)) => config != original,
            _ => false,
        }
    }
}

/// Builds the rows of one section from the draft.
#[must_use]
pub fn rows(state: &SettingsState, app: &AppState) -> Vec<SettingRow> {
    let Some(config) = state.config.as_ref() else {
        return Vec::new();
    };
    let fact = |label: &str, value: String| SettingRow {
        id: RowId::ReadOnly,
        label: label.to_owned(),
        kind: RowKind::Fact(value),
        detail: None,
        invalid: None,
    };
    match state.current_section() {
        Section::General => vec![
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
        ],
        Section::Sleep => {
            let mut list = vec![
                SettingRow {
                    id: RowId::SleepOnSwitch,
                    label: "Sleep on switch".to_owned(),
                    kind: RowKind::Toggle(config.sleep.enabled),
                    detail: None,
                    invalid: None,
                },
                number_row(RowId::GraceMs, "Grace", config.sleep.grace_ms, 0, "ms"),
            ];
            for (index, rule) in config.sleep.keep_alive.iter().enumerate() {
                let live = state.matches.iter().find(|entry| entry.rule_id == rule.id);
                let kind = match rule.kind {
                    KeepAliveKind::Process => "process",
                    KeepAliveKind::ListeningPort => "listening-port",
                };
                list.push(SettingRow {
                    id: RowId::KeepAliveRule(index),
                    label: format!("{}  {kind}  {}", rule.label, rule.pattern),
                    kind: RowKind::Toggle(rule.enabled),
                    detail: live.map(|entry| {
                        format!(
                            "{} \u{2014} matching {} processes now",
                            rule.label, entry.count
                        )
                    }),
                    invalid: live.and_then(|entry| {
                        entry
                            .error
                            .as_ref()
                            .map(|_| "invalid pattern \u{2014} rule is skipped".to_owned())
                    }),
                });
            }
            list
        }
        Section::Jobs => vec![
            SettingRow {
                id: RowId::WarnBeforeQuit,
                label: "Warn before quitting with running jobs".to_owned(),
                kind: RowKind::Toggle(config.jobs.warn_before_quit),
                detail: None,
                invalid: None,
            },
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
        ],
        Section::Pool => {
            let pools = app.snapshot.as_ref().map_or((0, 0), |snapshot| {
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
                fact("prepared copies", format!("{}/{} ready", pools.0, pools.1)),
            ]
        }
        Section::Github => vec![
            SettingRow {
                id: RowId::CloneProtocol,
                label: "Clone protocol".to_owned(),
                kind: choice(
                    protocol_name(config.github.clone_protocol),
                    PROTOCOLS,
                    protocol_name(config.github.clone_protocol),
                ),
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
        ],
        Section::Status => vec![
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
        ],
        Section::Windows => config
            .windows
            .iter()
            .enumerate()
            .map(|(index, window)| {
                fact(
                    &format!("{}", index + 1),
                    format!("{} \u{2014} {}", window.name, window.command),
                )
            })
            .collect(),
        Section::Hosts => {
            if config.hosts.is_empty() {
                vec![fact("hosts", "none configured".to_owned())]
            } else {
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
        }
        Section::About => about_rows(app),
    }
}

/// The About section: versions, the daemon, and the two escape hatches.
fn about_rows(app: &AppState) -> Vec<SettingRow> {
    let fact = |label: &str, value: String| SettingRow {
        id: RowId::ReadOnly,
        label: label.to_owned(),
        kind: RowKind::Fact(value),
        detail: None,
        invalid: None,
    };
    let mut list = Vec::new();
    let now = crate::dialogs::now_epoch();
    if let Some(snapshot) = app.snapshot.as_ref() {
        list.push(fact(
            "Fleet",
            crate::shell::bare_version(&snapshot.daemon.version).to_owned(),
        ));
        let uptime = crate::dialogs::age_secs(&snapshot.daemon.started_at, now)
            .map_or_else(|| "\u{2013}".to_owned(), uptime_label);
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
    list.push(fact("protocol", super::help::protocol().to_string()));
    list.push(fact(
        "E",
        "open config.json in a new terminal tab".to_owned(),
    ));
    list.push(fact("D", "run doctor".to_owned()));
    list
}

const AGENTS: &[&str] = &["claude", "opencode"];
const PROTOCOLS: &[&str] = &["ssh", "https"];
/// The `Keep finished jobs for` and `Trash retention` steps, in milliseconds.
const DURATIONS: &[u64] = &[60_000, 300_000, 600_000, 1_800_000, 3_600_000];
/// The `Hot pool size` steps.
const POOL_SIZES: &[u64] = &[0, 1, 2, 3];

const fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Opencode => "opencode",
    }
}

const fn protocol_name(protocol: CloneProtocol) -> &'static str {
    match protocol {
        CloneProtocol::Ssh => "ssh",
        CloneProtocol::Https => "https",
    }
}

fn choice(value: &str, options: &[&str], current: &str) -> RowKind {
    let index = options.iter().position(|option| *option == current);
    RowKind::Choice {
        value: value.to_owned(),
        has_prev: index.is_some_and(|index| index > 0),
        has_next: index.is_some_and(|index| index + 1 < options.len()),
        off_grid: index.is_none(),
    }
}

fn duration_choice(value: u64) -> RowKind {
    let index = DURATIONS.iter().position(|step| *step == value);
    RowKind::Choice {
        value: format_duration(value),
        has_prev: index.is_some_and(|index| index > 0),
        has_next: index.is_some_and(|index| index + 1 < DURATIONS.len()),
        off_grid: index.is_none(),
    }
}

fn pool_choice(value: u64) -> RowKind {
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

fn text_row(id: RowId, label: &str, value: &str) -> SettingRow {
    SettingRow {
        id,
        label: label.to_owned(),
        kind: RowKind::Text(value.to_owned()),
        detail: None,
        invalid: None,
    }
}

fn number_row(id: RowId, label: &str, value: i64, min: i64, unit: &str) -> SettingRow {
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

// ---------------------------------------------------------------------------- mutation

/// `Space`: flips the toggle under the cursor.
pub fn toggle(config: &mut Config, id: &RowId) {
    match id {
        RowId::SleepOnSwitch => config.sleep.enabled = !config.sleep.enabled,
        RowId::WarnBeforeQuit => config.jobs.warn_before_quit = !config.jobs.warn_before_quit,
        RowId::KeepAliveRule(index) => {
            if let Some(rule) = config.sleep.keep_alive.get_mut(*index) {
                rule.enabled = !rule.enabled;
            }
        }
        _ => {}
    }
}

/// `←` / `→`: steps the choice under the cursor, never wrapping.
pub fn cycle(config: &mut Config, id: &RowId, delta: isize) {
    match id {
        RowId::Agent => {
            let index = AGENTS
                .iter()
                .position(|name| *name == agent_name(config.agent))
                .unwrap_or(0);
            config.agent = match step(index, delta, AGENTS.len()) {
                0 => Agent::Claude,
                _ => Agent::Opencode,
            };
        }
        RowId::CloneProtocol => {
            let index = PROTOCOLS
                .iter()
                .position(|name| *name == protocol_name(config.github.clone_protocol))
                .unwrap_or(0);
            config.github.clone_protocol = match step(index, delta, PROTOCOLS.len()) {
                0 => CloneProtocol::Ssh,
                _ => CloneProtocol::Https,
            };
        }
        RowId::KeepFinishedFor => {
            config.jobs.keep_finished_for = stepped_duration(config.jobs.keep_finished_for, delta);
        }
        RowId::TrashRetention => {
            config.trash.retention_ms = stepped_duration(config.trash.retention_ms, delta);
        }
        RowId::HotPoolSize => {
            let index = POOL_SIZES
                .iter()
                .position(|size| *size == config.hot_pool_size)
                .unwrap_or(0);
            config.hot_pool_size = POOL_SIZES
                .get(step(index, delta, POOL_SIZES.len()))
                .copied()
                .unwrap_or(config.hot_pool_size);
        }
        _ => {}
    }
}

fn stepped_duration(current: u64, delta: isize) -> u64 {
    let index = DURATIONS
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    DURATIONS
        .get(step(index, delta, DURATIONS.len()))
        .copied()
        .unwrap_or(current)
}

/// Writes an edited text or number value back into the draft, clamping to the row's minimum.
pub fn commit_value(config: &mut Config, id: &RowId, raw: &str) {
    let number = |min: i64| raw.trim().parse::<i64>().unwrap_or(min).max(min);
    match id {
        RowId::ClaudeCommand => config.agent_commands.claude = raw.to_owned(),
        RowId::OpencodeCommand => config.agent_commands.opencode = raw.to_owned(),
        RowId::GraceMs => config.sleep.grace_ms = number(0),
        RowId::HotFreshnessMs => {
            config.hot_freshness_ms = u64::try_from(number(0)).unwrap_or_default();
        }
        RowId::HotRefreshIntervalMs => {
            config.hot_refresh_interval_ms = u64::try_from(number(0)).unwrap_or_default();
        }
        RowId::RepoCacheSeconds => config.github.cache_ttl_seconds = number(0),
        RowId::PrCacheSeconds => config.github.pr_ttl_seconds = number(0),
        RowId::StatusRefreshMs => config.ui.status_refresh_ms = number(500),
        RowId::RemoteStatusRefreshMs => config.ui.remote_status_refresh_ms = number(500),
        _ => {}
    }
}

// ---------------------------------------------------------------------------- seeding

/// Loads the effective configuration and the live keep-alive match counts.
pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let seq = with_host(cx, |host| {
        let seq = host.settings.seq.wrapping_add(1);
        host.settings = SettingsState {
            seq,
            ..SettingsState::default()
        };
        seq
    });
    let config_reply = bridge.request(RequestBody::GetConfig);
    let matches_reply = bridge.request(RequestBody::MatchKeepAliveRules);
    let state = state.clone();
    cx.spawn(async move |cx| {
        if let Ok(Ok(ResponseBody::Config(config))) = config_reply.recv().await {
            cx.update(|cx| {
                let live = with_host(cx, |host| {
                    if host.settings.seq != seq {
                        return false;
                    }
                    host.settings.original = Some(config.clone());
                    host.settings.config = Some(config);
                    true
                });
                if live {
                    notify(&state, cx);
                }
            });
        }
        if let Ok(Ok(ResponseBody::KeepAliveRuleMatches(matches))) = matches_reply.recv().await {
            cx.update(|cx| {
                let live = with_host(cx, |host| {
                    if host.settings.seq != seq {
                        return false;
                    }
                    host.settings.matches = matches;
                    true
                });
                if live {
                    notify(&state, cx);
                }
            });
        }
    })
    .detach();
}

// ---------------------------------------------------------------------------- rendering

/// Renders the dialog (§3.8.6).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight) = {
        let theme = cx.theme();
        (theme.space.md, theme.space.xs)
    };
    let draft = with_host(cx, |host| host.settings.clone());
    let pane_rows = rows(&draft, state.read(cx));
    let section = draft.current_section();
    let dirty = draft.dirty();

    let rail = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(RAIL_WIDTH))
        .gap(tight)
        .children(Section::ALL.iter().enumerate().map(|(index, entry)| {
            let selected = index == draft.section;
            Row::new()
                .selected(selected)
                .cursor(selected)
                .column(RowColumn::flex(Text::ui(entry.title())))
        }));

    let editing = draft.editing.clone();
    let pane = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(tight)
        .children(pane_rows.iter().enumerate().map(|(index, row)| {
            let focused = index == draft.row;
            row_element(row, focused, focused.then_some(editing.as_ref()).flatten())
        }))
        .children(
            section
                .editable()
                .then(|| Text::hint("edit in config.json").tone(Tone::Muted)),
        );

    let body = div()
        .flex()
        .flex_row()
        .gap(gap)
        .size_full()
        .child(rail)
        .child(Divider::vertical())
        .child(pane);

    let mut card = Dialog::new("Settings")
        .icon(Icon::Settings2)
        .width(super::Dialogs::Settings.width())
        .height(px(560.0))
        .body(body)
        .hint_row(if dirty {
            KeyHintRow::new()
                .key("\u{23ce}", "save")
                .key("esc", "discard changes")
                .key("E", "config.json")
                .key("D", "doctor")
        } else {
            KeyHintRow::new()
                .key("\u{21e5}", "section")
                .key("j/k", "row")
                .key("E", "config.json")
                .key("D", "doctor")
        });
    if dirty {
        // §3.8.6: the dirty state marks the title in accent and renames the footer.
        card = card.tone(Tone::Accent).primary("\u{23ce} Save");
    }
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let save_state = state.clone();
    let save_bridge = bridge.clone();
    let doctor_bridge = bridge.clone();
    let doctor_state = state.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                if type_into_row(&state, event, cx) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::MoveDown, _window, cx| move_row(&state, 1, "j", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::MoveUp, _window, cx| move_row(&state, -1, "k", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| move_row(&state, 1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| move_row(&state, -1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| move_section(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| move_section(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::CyclePrev, _window, cx| cycle_row(&state, -1, "h", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::CycleNext, _window, cx| cycle_row(&state, 1, "l", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::Toggle, _window, cx| toggle_row(&state, cx)
        })
        // `Backspace` / `ctrl-u` / `ctrl-w` are unambiguous edit intents, so unlike `j` / `k`
        // they focus the row's input themselves.
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit_focused(&state, cx, TextInput::backspace);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                edit_focused(&state, cx, TextInput::clear);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit_focused(&state, cx, TextInput::delete_word);
            }
        })
        // §KEYMAP "Dialogs and text inputs": `ctrl-a` / `ctrl-e` and `←` / `→` move the caret.
        // `←` / `→` fall back to cycling a choice, which is what §3.8.6 gives them on a row
        // that has no input.
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                move_caret(&state, cx, TextInput::home);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                move_caret(&state, cx, TextInput::end);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                if !move_caret(&state, cx, TextInput::left) {
                    cycle_row(&state, -1, "", cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                if !move_caret(&state, cx, TextInput::right) {
                    cycle_row(&state, 1, "", cx);
                }
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            save(&save_state, &save_bridge, cx);
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &settings_actions::OpenConfigFile, _window, cx| {
                // §3.8.6 surrenders every bound printable key to a focused input, `E` and `D`
                // included: `Claude command` and `OpenCode command` are free text, and a key
                // that replaced the screen instead of typing dropped the draft silently.
                if insert_literal(&state, "E", cx) {
                    return;
                }
                open_config_file(&state, &bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::RunDoctor, _window, cx| {
                if insert_literal(&state, "D", cx) {
                    return;
                }
                run_doctor(&doctor_state, &doctor_bridge, cx);
            }
        })
        .child(card)
        .into_any_element()
}

/// Draws one row with the kit control its kind calls for.
fn row_element(row: &SettingRow, focused: bool, editing: Option<&TextInput>) -> AnyElement {
    match &row.kind {
        RowKind::Toggle(checked) => {
            let mut toggle = Toggle::labeled(row.label.clone(), *checked).focused(focused);
            if let Some(detail) = row.invalid.clone().or_else(|| row.detail.clone()) {
                toggle = toggle.detail(detail);
            }
            toggle.into_any_element()
        }
        RowKind::Choice {
            value,
            has_prev,
            has_next,
            off_grid,
        } => Cycler::labeled(row.label.clone(), value.clone())
            .has_prev(*has_prev)
            .has_next(*has_next)
            .off_grid(*off_grid)
            .focused(focused)
            .into_any_element(),
        RowKind::Number { value, min, unit } => {
            let shown = editing.map_or(*value, |input| input.value().parse().unwrap_or(*value));
            let mut field = NumberField::labeled(row.label.clone(), shown)
                .min(*min)
                .focused(focused);
            if let Some(unit) = unit {
                field = field.unit(unit.clone());
            }
            field.into_any_element()
        }
        RowKind::Text(value) => {
            TextField::new(editing.map_or_else(|| value.clone(), |input| input.value().to_owned()))
                .label(row.label.clone())
                .caret(editing.map_or(0, TextInput::caret))
                .focused(focused)
                .mono(true)
                .into_any_element()
        }
        RowKind::Fact(value) => KeyValueList::new()
            .row(row.label.clone(), FactValue::known(value.clone()))
            .into_any_element(),
    }
}

/// The row the cursor is on, if any.
fn focused_row(state: &Entity<AppState>, cx: &mut App) -> Option<SettingRow> {
    let draft = with_host(cx, |host| host.settings.clone());
    let index = draft.row;
    rows(&draft, state.read(cx)).into_iter().nth(index)
}

/// `j` / `k` move the cursor — unless a text input has it, where they type (§3.8.6).
///
/// Landing on a row deliberately does **not** focus its input: §3.8.6 surrenders `j`/`k` only
/// "while a text input has focus", and a row that grabbed the keyboard on arrival would make
/// the next `j` type into the value instead of moving on. Typing (or `Backspace` / `ctrl-u` /
/// `ctrl-w`) is what focuses an input; moving away drops it again.
fn move_row(state: &Entity<AppState>, delta: isize, literal: &str, cx: &mut App) {
    if !literal.is_empty() && insert_literal(state, literal, cx) {
        return;
    }
    let len = focused_len(state, cx);
    with_host(cx, |host| {
        host.settings.row = step(host.settings.row, delta, len);
        host.settings.editing = None;
    });
    notify(state, cx);
}

/// `←` / `→` cycle a choice — or type, for the same reason.
fn cycle_row(state: &Entity<AppState>, delta: isize, literal: &str, cx: &mut App) {
    if insert_literal(state, literal, cx) {
        return;
    }
    let Some(row) = focused_row(state, cx) else {
        return;
    };
    with_host(cx, |host| {
        if let Some(config) = host.settings.config.as_mut() {
            cycle(config, &row.id, delta);
        }
    });
    notify(state, cx);
}

/// `Space` toggles the switch under the cursor.
fn toggle_row(state: &Entity<AppState>, cx: &mut App) {
    if insert_literal(state, " ", cx) {
        return;
    }
    let Some(row) = focused_row(state, cx) else {
        return;
    };
    with_host(cx, |host| {
        if let Some(config) = host.settings.config.as_mut() {
            toggle(config, &row.id);
        }
    });
    notify(state, cx);
}

/// Applies an edit to the focused row's input, focusing it first if it is not focused yet.
fn edit_focused(state: &Entity<AppState>, cx: &mut App, edit: fn(&mut TextInput) -> bool) {
    if !begin_editing(state, cx) {
        return;
    }
    let changed = with_host(cx, |host| host.settings.editing.as_mut().is_some_and(edit));
    if changed {
        flush(state, cx);
    }
}

/// Moves the caret of an **already focused** input. Returns whether there was one.
fn move_caret(state: &Entity<AppState>, cx: &mut App, move_to: fn(&mut TextInput)) -> bool {
    let moved = with_host(cx, |host| match host.settings.editing.as_mut() {
        Some(input) => {
            move_to(input);
            true
        }
        None => false,
    });
    if moved {
        notify(state, cx);
    }
    moved
}

/// Whether the focused row's input would take this text at all.
///
/// A number row is not a free-text field. Letting a `j` from the `move down` binding into its
/// buffer makes `commit_value` fail to parse the result and clamp the setting to its minimum —
/// which is how `j` on `Sleep › Grace` used to wipe `2000` to `0`. Rejecting the character here
/// is also what lets the key fall through to row navigation.
fn accepts(kind: &RowKind, text: &str) -> bool {
    match kind {
        RowKind::Text(_) => true,
        RowKind::Number { .. } => !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()),
        RowKind::Toggle(_) | RowKind::Choice { .. } | RowKind::Fact(_) => false,
    }
}

/// Every **printable** key `Dialog > Settings` binds, and therefore every key a focused text
/// input has to be handed back as a character (§3.8.6).
///
/// It is a tripwire, not a dispatch table: each handler calls [`insert_literal`] with its own
/// literal, and the test below fails the moment a new printable binding is added here without
/// that call — which is how `E` and `D` came to replace the screen mid-word instead of typing.
pub const SURRENDERED_KEYS: &[&str] = &["space", "h", "l", "j", "k", "E", "D"];

/// Inserts a bound key's literal character when a text input owns the keyboard.
///
/// §3.8.6 surrenders `j`, `k`, `h`, `l` and `Space` to a **focused** input. gpui dispatches
/// those bindings before any key listener, so the surrender has to happen inside the action
/// handler: there is no other place that sees the key. Returning `false` hands the key back to
/// its normal meaning, which is why an unfocused row — or a number row facing a letter — still
/// navigates.
fn insert_literal(state: &Entity<AppState>, literal: &str, cx: &mut App) -> bool {
    if literal.is_empty() {
        return false;
    }
    if !with_host(cx, |host| host.settings.editing.is_some()) {
        return false;
    }
    let Some(row) = focused_row(state, cx) else {
        return false;
    };
    if !accepts(&row.kind, literal) {
        return false;
    }
    with_host(cx, |host| {
        if let Some(input) = host.settings.editing.as_mut() {
            input.insert(literal);
        }
    });
    flush(state, cx);
    true
}

/// Types a printable key into the focused text or number row.
fn type_into_row(state: &Entity<AppState>, event: &KeyDownEvent, cx: &mut App) -> bool {
    let Some(text) = typed_char(event) else {
        return false;
    };
    let Some(row) = focused_row(state, cx) else {
        return false;
    };
    if !accepts(&row.kind, &text) {
        return false;
    }
    if !begin_editing(state, cx) {
        return false;
    }
    with_host(cx, |host| {
        if let Some(input) = host.settings.editing.as_mut() {
            input.insert(&text);
        }
    });
    flush(state, cx);
    true
}

/// Seeds the edit buffer from the focused row. Returns whether that row accepts typing.
fn begin_editing(state: &Entity<AppState>, cx: &mut App) -> bool {
    let Some(row) = focused_row(state, cx) else {
        return false;
    };
    let seed = match &row.kind {
        RowKind::Text(value) => value.clone(),
        RowKind::Number { value, .. } => value.to_string(),
        _ => {
            with_host(cx, |host| host.settings.editing = None);
            return false;
        }
    };
    with_host(cx, |host| {
        if host.settings.editing.is_none() {
            host.settings.editing = Some(TextInput::new(seed));
        }
    });
    true
}

/// Writes the edit buffer into the draft configuration.
fn flush(state: &Entity<AppState>, cx: &mut App) {
    let Some(row) = focused_row(state, cx) else {
        return;
    };
    with_host(cx, |host| {
        let Some(raw) = host
            .settings
            .editing
            .as_ref()
            .map(|input| input.value().to_owned())
        else {
            return;
        };
        if let Some(config) = host.settings.config.as_mut() {
            commit_value(config, &row.id, &raw);
        }
    });
    notify(state, cx);
}

fn focused_len(state: &Entity<AppState>, cx: &mut App) -> usize {
    let draft = with_host(cx, |host| host.settings.clone());
    rows(&draft, state.read(cx)).len()
}

fn move_section(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(cx, |host| {
        host.settings.section = step(host.settings.section, delta, Section::ALL.len());
        host.settings.row = 0;
        host.settings.editing = None;
    });
    notify(state, cx);
}

/// `Enter`: save synchronously and silently; a refused write keeps the dialog open (§3.8.6).
fn save(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some(config) = with_host(cx, |host| host.settings.config.clone()) else {
        return;
    };
    let Ok(patch) = serde_json::to_value(&config) else {
        with_host(cx, |host| {
            host.settings.error = Some("the configuration could not be encoded".to_owned());
        });
        notify(state, cx);
        return;
    };
    let reply = bridge.request(RequestBody::SetConfig { patch });
    let warn_before_quit = config.jobs.warn_before_quit;
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(_) => {
                state.update(cx, |app, cx| {
                    app.warn_before_quit = warn_before_quit;
                    app.close_overlay();
                    cx.notify();
                });
            }
            Err(failure) => {
                with_host(cx, |host| host.settings.error = Some(failure.message));
                notify(&state, cx);
            }
        });
    })
    .detach();
}

/// `D`: run doctor and surface the first failing check in the sticky error slot.
/// §3.8.6 About: `E` opens `config.json` **in a new terminal tab**, not in the OS handler.
///
/// KEYMAP scopes `E` per context and gives this one the terminal tab, so that editing the file
/// happens inside Fleet, next to the daemon it configures. Without a live session there is no
/// tab strip to add to, and the system handler is the honest fallback rather than a dead key.
fn open_config_file(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let path = state.read(cx).home.join("config.json");
    let Some(session) = state.read(cx).active_session().cloned() else {
        cx.open_with_system(&path);
        return;
    };
    bridge.send(RequestBody::NewTerminal {
        session: session.id.clone(),
        name: workspace_tabs::unique_terminal_name(&session, "config"),
        command: format!("{} {}", editor_command(), path.display()),
        cwd: session.cwd.clone(),
    });
    state.update(cx, |app, cx| {
        app.close_overlay();
        app.screen = Screen::Workspace {
            session: session.id.clone(),
        };
        cx.notify();
    });
}

/// `$EDITOR`, or `vi` — the one editor POSIX guarantees.
fn editor_command() -> String {
    std::env::var("EDITOR")
        .ok()
        .map(|editor| editor.trim().to_owned())
        .filter(|editor| !editor.is_empty())
        .unwrap_or_else(|| "vi".to_owned())
}

/// §3.8.6 About: `D` runs doctor and shows the §3.12 `Daemon > Doctor` surface.
///
/// The same key means the same thing on the daemon-down splash, so both write the answer to
/// [`AppState::doctor`] and the shell renders it; a toast would have been a second, weaker
/// spelling of a screen that already exists.
fn run_doctor(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let reply = bridge.request(RequestBody::Doctor);
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| match answer {
            Ok(Ok(ResponseBody::Doctor(checks))) => {
                state.update(cx, |app, cx| {
                    app.doctor = Some(checks);
                    // The doctor table is a full surface; it replaces the dialog it was
                    // raised from, and `Esc` there comes back to the app.
                    app.close_overlay();
                    cx.notify();
                });
            }
            // A refused request keeps the dialog open with the exact error (§3.8.6 States).
            Ok(Err(error)) => {
                with_host(cx, |host| host.settings.error = Some(error.message.clone()));
                notify(&state, cx);
            }
            Ok(Ok(_)) | Err(_) => {
                with_host(cx, |host| {
                    host.settings.error = Some("doctor: the daemon did not answer".to_owned());
                });
                notify(&state, cx);
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use fleet_core::config::default_config;

    use super::*;

    fn draft() -> SettingsState {
        let config = default_config("/tmp/fleet");
        SettingsState {
            original: Some(config.clone()),
            config: Some(config),
            ..SettingsState::default()
        }
    }

    #[test]
    fn every_printable_settings_binding_is_surrendered_to_a_focused_input() {
        // §3.8.6: a bound printable key must reach a focused text input as a character.
        // gpui dispatches bindings before any key listener, so each handler has to call
        // `insert_literal` first; this catches a new binding that forgot to.
        for spec in crate::keymap::table() {
            if spec.context != "Dialog > Settings" {
                continue;
            }
            // A chord or a named non-printable key types nothing.
            let printable = spec.keys == "space"
                || (!spec.keys.contains(' ')
                    && !spec.keys.contains('-')
                    && spec.keys.chars().count() == 1);
            if !printable {
                continue;
            }
            assert!(
                SURRENDERED_KEYS.contains(&spec.keys),
                "`{}` types into `Claude command`; route it through `insert_literal`",
                spec.keys
            );
        }
    }

    #[test]
    fn a_text_row_takes_every_surrendered_key_as_a_character() {
        for key in ["E", "D", "j", "k", "h", "l", " "] {
            assert!(
                accepts(&RowKind::Text(String::new()), key),
                "a free-text row takes `{key}`"
            );
            assert!(
                !accepts(&RowKind::Toggle(false), key),
                "a toggle takes nothing, so `{key}` keeps its binding"
            );
        }
        let number = RowKind::Number {
            value: 1,
            min: 0,
            unit: None,
        };
        assert!(!accepts(&number, "E"), "a number row is not a text field");
        assert!(accepts(&number, "7"));
    }

    #[test]
    fn a_fresh_draft_is_not_dirty() {
        assert!(!draft().dirty());
    }

    #[test]
    fn toggling_a_switch_makes_the_draft_dirty() {
        let mut state = draft();
        let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
        let before = config.jobs.warn_before_quit;
        toggle(config, &RowId::WarnBeforeQuit);
        assert_ne!(config.jobs.warn_before_quit, before);
        assert!(state.dirty());
    }

    #[test]
    fn cycling_never_wraps_past_either_end() {
        let mut state = draft();
        let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
        cycle(config, &RowId::Agent, -1);
        assert_eq!(config.agent, Agent::Claude);
        cycle(config, &RowId::Agent, 1);
        assert_eq!(config.agent, Agent::Opencode);
        cycle(config, &RowId::Agent, 1);
        assert_eq!(config.agent, Agent::Opencode);
    }

    #[test]
    fn numbers_clamp_to_their_minimum() {
        let mut state = draft();
        let config = state.config.as_mut().unwrap_or_else(|| panic!("no config"));
        commit_value(config, &RowId::StatusRefreshMs, "10");
        assert_eq!(config.ui.status_refresh_ms, 500);
        commit_value(config, &RowId::StatusRefreshMs, "4000");
        assert_eq!(config.ui.status_refresh_ms, 4_000);
        commit_value(config, &RowId::GraceMs, "not a number");
        assert_eq!(config.sleep.grace_ms, 0);
    }

    #[test]
    fn text_rows_own_typing_and_choice_rows_do_not() {
        let text = text_row(RowId::ClaudeCommand, "Claude command", "claude");
        assert!(text.owns_typing());
        let toggle = SettingRow {
            id: RowId::WarnBeforeQuit,
            label: "Warn".to_owned(),
            kind: RowKind::Toggle(true),
            detail: None,
            invalid: None,
        };
        assert!(!toggle.owns_typing());
    }

    #[test]
    fn a_number_row_refuses_every_non_digit() {
        let number = number_row(RowId::GraceMs, "Grace", 2_000, 0, "ms");
        // `j` / `k` / `h` / `l` / space are bound to navigation: none may reach the buffer,
        // where `commit_value` would clamp `2000j` down to the minimum.
        for literal in ["j", "k", "h", "l", " ", "-", "x"] {
            assert!(
                !accepts(&number.kind, literal),
                "`{literal}` must never type into a number row"
            );
        }
        assert!(accepts(&number.kind, "7"));
        assert!(!accepts(&number.kind, ""));

        // A free-text row still takes every printable key, including those letters.
        let text = text_row(RowId::ClaudeCommand, "Claude command", "claude");
        for literal in ["j", "k", "h", "l", " ", "7"] {
            assert!(accepts(&text.kind, literal));
        }

        // Rows with no input never take typing at all.
        let toggle = SettingRow {
            id: RowId::WarnBeforeQuit,
            label: "Warn".to_owned(),
            kind: RowKind::Toggle(true),
            detail: None,
            invalid: None,
        };
        assert!(!accepts(&toggle.kind, "j"));
    }

    #[test]
    fn durations_read_as_one_unit() {
        assert_eq!(format_duration(600_000), "10 min");
        assert_eq!(format_duration(3_600_000), "1 h");
        assert_eq!(format_duration(30_000), "30 s");
    }

    #[test]
    fn every_section_has_a_title_and_only_data_sections_are_editable() {
        for section in Section::ALL {
            assert!(!section.title().is_empty());
        }
        assert!(Section::General.editable());
        assert!(!Section::Windows.editable());
        assert!(!Section::About.editable());
    }

    #[test]
    fn the_sleep_section_lists_one_row_per_keep_alive_rule() {
        let state = draft();
        let mut probe = state;
        probe.section = Section::ALL
            .iter()
            .position(|section| *section == Section::Sleep)
            .unwrap_or(0);
        let app = AppState::new("/tmp/fleet", std::time::Instant::now());
        let list = rows(&probe, &app);
        let rules = probe
            .config
            .as_ref()
            .map_or(0, |config| config.sleep.keep_alive.len());
        assert_eq!(list.len(), 2 + rules);
    }
}
