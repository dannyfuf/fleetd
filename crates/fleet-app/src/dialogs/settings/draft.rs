use super::*;

/// The Settings dialog's draft.
#[derive(Debug, Default)]
pub struct SettingsState {
    /// The configuration as it will be saved.
    pub(crate) config: Option<Config>,
    /// The configuration as it was loaded, for the dirty check.
    pub(crate) original: Option<Config>,
    /// Which section the rail highlights.
    pub(crate) section: usize,
    /// Which row the pane's cursor is on.
    pub(crate) row: usize,
    /// The live keep-alive match counts.
    pub(crate) matches: Vec<KeepAliveRuleMatch>,
    /// The raw text of the row being edited, mirrored from `DialogHost.settings_input`.
    ///
    /// It is `Some` exactly while that editor exists, which is what publishes
    /// `Dialog > SettingsEditing`.
    pub(crate) editing: Option<String>,
    /// The exact error from a refused write.
    pub(crate) error: Option<String>,
    /// Config load failure; `Enter` retries it.
    pub(crate) config_error: Option<String>,
    /// A config load is awaiting its reply.
    pub(crate) config_loading: bool,
    /// Keep-alive diagnostics load failure; `Enter` retries it.
    pub(crate) matches_error: Option<String>,
    /// A diagnostics load is awaiting its reply.
    pub(crate) matches_loading: bool,
    /// Prevents duplicate config writes for one opening.
    pub(crate) save_in_flight: bool,
    /// Prevents duplicate doctor requests for one opening.
    pub(crate) doctor_in_flight: bool,
    /// Bumps on every seed; a late answer to a superseded dialog is dropped.
    pub(crate) seq: u64,
    pub(crate) scroll: gpui::ScrollHandle,
    pub(crate) prepared: std::rc::Rc<[SettingRow]>,
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

    pub(super) fn load_error(&self) -> Option<String> {
        match (&self.config_error, &self.matches_error) {
            (Some(config), Some(matches)) => Some(format!(
                "{config}; {matches} — Enter to retry configuration"
            )),
            (Some(error), None) => Some(format!("{error} — Enter to retry")),
            (None, Some(error)) => Some(format!("{error} — diagnostics will retry when you save")),
            (None, None) => None,
        }
    }

    pub(super) fn take_failed_loads(&mut self) -> (bool, bool) {
        let config_failed = self.config_error.take().is_some();
        let config = config_failed || (self.config.is_none() && !self.config_loading);
        let matches = self.matches_error.take().is_some();
        (config, matches)
    }

    pub(super) fn editing_is_valid(&self) -> bool {
        let Some(input) = self.editing.as_ref() else {
            return true;
        };
        match self.focused_row().map(|row| row.kind) {
            Some(RowKind::Text(_)) => true,
            Some(RowKind::Number { min, .. }) => {
                input.trim().parse::<i64>().is_ok_and(|value| value >= min)
            }
            _ => false,
        }
    }

    pub(super) fn begin_save(&mut self) -> Option<u64> {
        if self.save_in_flight {
            return None;
        }
        self.save_in_flight = true;
        self.error = None;
        Some(self.seq)
    }

    pub(super) fn finish_save(&mut self, seq: u64) -> bool {
        if self.seq != seq || !self.save_in_flight {
            return false;
        }
        self.save_in_flight = false;
        true
    }

    pub(super) fn begin_doctor(&mut self) -> Option<u64> {
        if self.doctor_in_flight {
            return None;
        }
        self.doctor_in_flight = true;
        self.error = None;
        Some(self.seq)
    }

    pub(super) fn finish_doctor(&mut self, seq: u64) -> bool {
        if self.seq != seq || !self.doctor_in_flight {
            return false;
        }
        self.doctor_in_flight = false;
        true
    }
}

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
                _ => Agent::Codex,
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
        RowId::ClaudeDefaultMode => cycle_mode(config, AgentKind::Claude, delta),
        RowId::CodexDefaultMode => cycle_mode(config, AgentKind::Codex, delta),
        _ => {}
    }
}

fn cycle_mode(config: &mut Config, kind: AgentKind, delta: isize) {
    let modes = kind.supported_modes();
    let defaults = match kind {
        AgentKind::Claude => &mut config.native_agents.claude,
        AgentKind::Codex => &mut config.native_agents.codex,
    };
    let index = modes
        .iter()
        .position(|mode| *mode == defaults.mode)
        .unwrap_or(0);
    defaults.mode = modes
        .get(step(index, delta, modes.len()))
        .copied()
        .unwrap_or(defaults.mode);
}

pub(super) fn stepped_duration(current: u64, delta: isize) -> u64 {
    let index = DURATIONS
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    DURATIONS
        .get(step(index, delta, DURATIONS.len()))
        .copied()
        .unwrap_or(current)
}

/// Writes a complete, valid edit back into the draft.
pub fn commit_value(config: &mut Config, id: &RowId, raw: &str) -> bool {
    let number = |min: i64| raw.trim().parse::<i64>().ok().filter(|value| *value >= min);
    match id {
        RowId::ClaudeCommand => config.agent_commands.claude = raw.to_owned(),
        RowId::CodexCommand => config.agent_commands.codex = raw.to_owned(),
        RowId::ClaudeBinary => config.agent_binaries.claude = raw.to_owned(),
        RowId::CodexBinary => config.agent_binaries.codex = raw.to_owned(),
        RowId::ClaudeDefaultModel => config.native_agents.claude.model = optional(raw),
        RowId::ClaudeDefaultEffort => config.native_agents.claude.effort = optional(raw),
        RowId::CodexDefaultModel => config.native_agents.codex.model = optional(raw),
        RowId::CodexDefaultEffort => config.native_agents.codex.effort = optional(raw),
        RowId::GraceMs => {
            let Some(value) = number(0) else { return false };
            config.sleep.grace_ms = value;
        }
        RowId::HotFreshnessMs => {
            let Some(value) = number(0).and_then(|value| u64::try_from(value).ok()) else {
                return false;
            };
            config.hot_freshness_ms = value;
        }
        RowId::HotRefreshIntervalMs => {
            let Some(value) = number(0).and_then(|value| u64::try_from(value).ok()) else {
                return false;
            };
            config.hot_refresh_interval_ms = value;
        }
        RowId::RepoCacheSeconds => {
            let Some(value) = number(0) else { return false };
            config.github.cache_ttl_seconds = value;
        }
        RowId::PrCacheSeconds => {
            let Some(value) = number(0) else { return false };
            config.github.pr_ttl_seconds = value;
        }
        RowId::StatusRefreshMs => {
            let Some(value) = number(500) else {
                return false;
            };
            config.ui.status_refresh_ms = value;
        }
        RowId::RemoteStatusRefreshMs => {
            let Some(value) = number(500) else {
                return false;
            };
            config.ui.remote_status_refresh_ms = value;
        }
        _ => return false,
    }
    true
}

fn optional(raw: &str) -> Option<String> {
    let value = raw.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The row the cursor is on, if any.
pub(super) fn focused_row(state: &Entity<AppState>, cx: &mut App) -> Option<FocusedSetting> {
    read_host(state, cx, |host, _| host.settings.focused_row())
}

impl SettingsState {
    fn update_selected(&mut self) {
        let Some(focused) = self.focused_row() else {
            return;
        };
        if focused.id == RowId::ReadOnly {
            return;
        }
        if let Some(row) = std::rc::Rc::make_mut(&mut self.prepared).get_mut(self.row)
            && row.id == focused.id
        {
            match (&mut row.kind, focused.kind) {
                (RowKind::Number { value, .. }, RowKind::Number { value: edited, .. }) => {
                    *value = edited
                }
                (_, kind) => row.kind = kind,
            }
        }
    }

    fn row_id(&self) -> Option<RowId> {
        use RowId::*;
        let ids: &[RowId] = match self.current_section() {
            Section::General => &[
                Agent,
                ClaudeCommand,
                CodexCommand,
                ClaudeBinary,
                CodexBinary,
                ClaudeDefaultMode,
                ClaudeDefaultModel,
                ClaudeDefaultEffort,
                CodexDefaultMode,
                CodexDefaultModel,
                CodexDefaultEffort,
            ],
            Section::Sleep => {
                return match self.row {
                    0 => Some(SleepOnSwitch),
                    1 => Some(GraceMs),
                    index => self
                        .config
                        .as_ref()?
                        .sleep
                        .keep_alive
                        .get(index - 2)
                        .map(|_| KeepAliveRule(index - 2)),
                };
            }
            Section::Jobs => &[WarnBeforeQuit, KeepFinishedFor, TrashRetention],
            Section::Pool => &[HotPoolSize, HotFreshnessMs, HotRefreshIntervalMs, ReadOnly],
            Section::Github => &[CloneProtocol, RepoCacheSeconds, PrCacheSeconds],
            Section::Status => &[StatusRefreshMs, RemoteStatusRefreshMs],
            Section::Windows | Section::Hosts | Section::About => return Some(ReadOnly),
        };
        ids.get(self.row).cloned()
    }

    pub(super) fn focused_row(&self) -> Option<FocusedSetting> {
        let config = self.config.as_ref()?;
        let id = self.row_id()?;
        let number = |value, min| RowKind::Number {
            value,
            min,
            unit: None,
        };
        let kind = match id {
            RowId::ClaudeCommand => RowKind::Text(config.agent_commands.claude.clone()),
            RowId::CodexCommand => RowKind::Text(config.agent_commands.codex.clone()),
            RowId::ClaudeBinary => RowKind::Text(config.agent_binaries.claude.clone()),
            RowId::CodexBinary => RowKind::Text(config.agent_binaries.codex.clone()),
            RowId::ClaudeDefaultModel => RowKind::Text(
                config
                    .native_agents
                    .claude
                    .model
                    .clone()
                    .unwrap_or_default(),
            ),
            RowId::ClaudeDefaultEffort => RowKind::Text(
                config
                    .native_agents
                    .claude
                    .effort
                    .clone()
                    .unwrap_or_default(),
            ),
            RowId::CodexDefaultModel => {
                RowKind::Text(config.native_agents.codex.model.clone().unwrap_or_default())
            }
            RowId::CodexDefaultEffort => RowKind::Text(
                config
                    .native_agents
                    .codex
                    .effort
                    .clone()
                    .unwrap_or_default(),
            ),
            RowId::GraceMs => number(config.sleep.grace_ms, 0),
            RowId::HotFreshnessMs => number(
                i64::try_from(config.hot_freshness_ms).unwrap_or(i64::MAX),
                0,
            ),
            RowId::HotRefreshIntervalMs => number(
                i64::try_from(config.hot_refresh_interval_ms).unwrap_or(i64::MAX),
                0,
            ),
            RowId::RepoCacheSeconds => number(config.github.cache_ttl_seconds, 0),
            RowId::PrCacheSeconds => number(config.github.pr_ttl_seconds, 0),
            RowId::StatusRefreshMs => number(config.ui.status_refresh_ms, 500),
            RowId::RemoteStatusRefreshMs => number(config.ui.remote_status_refresh_ms, 500),
            RowId::Agent => choice(agent_name(config.agent), AGENTS, agent_name(config.agent)),
            RowId::ClaudeDefaultMode => {
                mode_choice(AgentKind::Claude, config.native_agents.claude.mode)
            }
            RowId::CodexDefaultMode => {
                mode_choice(AgentKind::Codex, config.native_agents.codex.mode)
            }
            RowId::CloneProtocol => choice(
                protocol_name(config.github.clone_protocol),
                PROTOCOLS,
                protocol_name(config.github.clone_protocol),
            ),
            RowId::SleepOnSwitch => RowKind::Toggle(config.sleep.enabled),
            RowId::WarnBeforeQuit => RowKind::Toggle(config.jobs.warn_before_quit),
            RowId::KeepAliveRule(index) => {
                RowKind::Toggle(config.sleep.keep_alive.get(index)?.enabled)
            }
            RowId::KeepFinishedFor => duration_choice(config.jobs.keep_finished_for),
            RowId::TrashRetention => duration_choice(config.trash.retention_ms),
            RowId::HotPoolSize => pool_choice(config.hot_pool_size),
            RowId::ReadOnly => RowKind::Fact(String::new()),
        };
        Some(FocusedSetting { id, kind })
    }
}

fn mode_choice(kind: AgentKind, mode: PermissionMode) -> RowKind {
    let labels = kind
        .supported_modes()
        .iter()
        .map(|mode| crate::screens::agent_thread::presentation::mode_label(*mode))
        .collect::<Vec<_>>();
    let current = crate::screens::agent_thread::presentation::mode_label(mode);
    choice(current, &labels, current)
}

/// `j` / `k` move the cursor (§3.8.6).
///
/// Landing on a row deliberately does **not** focus its input: §3.8.6 surrenders `j` / `k`
/// only while a text input has focus, and a row that grabbed the keyboard on arrival would
/// make the next `j` type into the value instead of moving on. `Enter` is what opens a row for
/// editing; moving away closes it again.
pub(super) fn move_row(
    state: &Entity<AppState>,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let len = focused_len(state, cx);
    with_host(state, cx, |host| {
        host.settings.row = step(host.settings.row, delta, len);
        host.settings.scroll.scroll_to_item(host.settings.row);
    });
    end_editing(state, focus, window, cx);
    notify(state, cx);
}

/// `h` / `l` and `\u{2190}` / `\u{2192}` cycle a closed choice while no row owns the keyboard.
pub(super) fn cycle_row(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let Some(row) = focused_row(state, cx) else {
        return;
    };
    with_host(state, cx, |host| {
        if let Some(config) = host.settings.config.as_mut() {
            cycle(config, &row.id, delta);
        }
        host.settings.update_selected();
    });
    notify(state, cx);
}

/// `Space` toggles the switch under the cursor.
pub(super) fn toggle_row(state: &Entity<AppState>, cx: &mut App) {
    let Some(row) = focused_row(state, cx) else {
        return;
    };
    with_host(state, cx, |host| {
        if let Some(config) = host.settings.config.as_mut() {
            toggle(config, &row.id);
        }
        host.settings.update_selected();
    });
    notify(state, cx);
}

/// `Enter` while browsing: open the focused text or number row for editing.
///
/// Returns whether it did. Every other row — a toggle, a cycler, a read-only fact — leaves
/// `Enter` its ordinary meaning, which in this dialog is Save.
pub(super) fn confirm_opens_editing(
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if read_host(state, cx, |host, _| host.settings_input.is_some()) {
        return false;
    }
    begin_editing(state, window, cx)
}

/// Materializes the focused row's editor and hands it the keyboard.
///
/// Returns whether the row takes typing at all; a toggle, a cycler and a read-only fact do
/// not, which is what lets `Enter` keep its ordinary meaning on them.
pub(super) fn begin_editing(state: &Entity<AppState>, window: &mut Window, cx: &mut App) -> bool {
    if read_host(state, cx, |host, _| host.settings_input.is_some()) {
        return true;
    }
    let Some(focused) = focused_row(state, cx) else {
        return false;
    };
    let (text, min) = match &focused.kind {
        RowKind::Text(value) => (value.clone(), None),
        RowKind::Number { value, min, .. } => (value.to_string(), Some(*min)),
        RowKind::Toggle(_) | RowKind::Choice { .. } | RowKind::Fact(_) => return false,
    };
    // A number row is drawn by `NumberField`, which keeps the label and the unit around the
    // editor; a text row's editor carries its own label.
    let label = min.is_none().then(|| {
        read_host(state, cx, |host, _| {
            host.settings
                .prepared
                .get(host.settings.row)
                .map(|row| row.label.clone())
        })
    });
    let seed = text.clone();
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_mono(true, cx);
        input.set_hide_status_line(true, cx);
        if let Some(label) = label.flatten() {
            input.set_label(Some(label.into()), cx);
        }
        if min.is_some() {
            // A number row is not a free-text field: a letter that reached the buffer would
            // make `commit_value` fail to parse it and clamp the setting to its minimum.
            input.set_filter(Some(|character: char| character.is_ascii_digit()), cx);
        }
        input.set_text(seed, cx);
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, _| {
        host.settings.editing = Some(text);
        host.settings_input = Some(input.clone());
    });
    let weak_state = state.downgrade();
    let subscription = cx.subscribe(&input, move |input, event, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        let typed = input.read(cx).text().to_owned();
        with_host(&state, cx, |host| host.settings.editing = Some(typed));
        if let Some(min) = min {
            let invalid = (!read_host(&state, cx, |host, _| host.settings.editing_is_valid()))
                .then(|| format!("must be an integer of at least {min}"));
            let input = input.clone();
            cx.defer(move |cx| {
                input.update(cx, |input, cx| {
                    input.set_invalid(invalid.map(Into::into), cx)
                })
            });
        }
        flush(&state, cx);
    });
    host.update(cx, |host, _| {
        host.settings_input_subscription = Some(subscription)
    });
    input.update(cx, |input, cx| input.focus(window, cx));
    notify(state, cx);
    true
}

/// Drops the row editor and hands the keyboard back to the dialog.
pub(super) fn end_editing(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let had_input = with_host(state, cx, |host| {
        host.settings.editing = None;
        host.settings_input_subscription = None;
        host.settings_input.take().is_some()
    });
    if had_input {
        window.focus(focus, cx);
    }
}

/// Writes the edit buffer into the draft configuration.
pub(super) fn flush(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        let draft = &mut host.settings;
        let Some(id) = draft.row_id() else {
            return;
        };
        if let (Some(config), Some(input)) = (&mut draft.config, &draft.editing) {
            let _valid = commit_value(config, &id, input);
        }
        draft.update_selected();
    });
    notify(state, cx);
}

pub(super) fn focused_len(state: &Entity<AppState>, cx: &mut App) -> usize {
    read_host(state, cx, |host, cx| {
        let draft = &host.settings;
        let Some(config) = draft.config.as_ref() else {
            return 0;
        };
        match draft.current_section() {
            Section::General | Section::Jobs | Section::Github => 3,
            Section::Sleep => 2 + config.sleep.keep_alive.len(),
            Section::Pool => 4,
            Section::Status => 2,
            Section::Windows => config.windows.len(),
            Section::Hosts => host_rows(config, state.read(cx)).len(),
            Section::About => about_rows(state.read(cx)).len(),
        }
    })
}

pub(super) fn move_section(
    state: &Entity<AppState>,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    with_host(state, cx, |host| {
        host.settings.section = step(host.settings.section, delta, Section::ALL.len());
        host.settings.row = 0;
        host.settings.scroll.scroll_to_item(host.settings.row);
    });
    end_editing(state, focus, window, cx);
    refresh_rows(state, cx);
    notify(state, cx);
}

pub(super) struct FocusedSetting {
    pub(super) id: RowId,
    pub(super) kind: RowKind,
}
