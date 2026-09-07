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
    /// The buffer backing the focused text or number row.
    pub(crate) editing: Option<TextFieldState>,
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
            Some(RowKind::Number { min, .. }) => input
                .text()
                .trim()
                .parse::<i64>()
                .is_ok_and(|value| value >= min),
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
        RowId::OpencodeCommand => config.agent_commands.opencode = raw.to_owned(),
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
            Section::General => &[Agent, ClaudeCommand, OpencodeCommand],
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
            RowId::OpencodeCommand => RowKind::Text(config.agent_commands.opencode.clone()),
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

/// `j` / `k` move the cursor — unless a text input has it, where they type (§3.8.6).
///
/// Landing on a row deliberately does **not** focus its input: §3.8.6 surrenders `j`/`k` only
/// "while a text input has focus", and a row that grabbed the keyboard on arrival would make
/// the next `j` type into the value instead of moving on. Typing (or `Backspace` / `ctrl-u` /
/// `ctrl-w`) is what focuses an input; moving away drops it again.
pub(super) fn move_row(state: &Entity<AppState>, delta: isize, literal: &str, cx: &mut App) {
    if !literal.is_empty() && insert_literal(state, literal, cx) {
        return;
    }
    let len = focused_len(state, cx);
    with_host(state, cx, |host| {
        host.settings.row = step(host.settings.row, delta, len);
        host.settings.editing = None;
        host.settings.scroll.scroll_to_item(host.settings.row);
    });
    notify(state, cx);
}

/// `←` / `→` cycle a choice — or type, for the same reason.
pub(super) fn cycle_row(state: &Entity<AppState>, delta: isize, literal: &str, cx: &mut App) {
    if insert_literal(state, literal, cx) {
        return;
    }
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
    if insert_literal(state, " ", cx) {
        return;
    }
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

/// Applies an edit to the focused row's input, focusing it first if it is not focused yet.
pub(super) fn edit_focused(
    state: &Entity<AppState>,
    cx: &mut App,
    edit: fn(&mut TextFieldState) -> bool,
) {
    if !begin_editing(state, cx) {
        return;
    }
    let changed = with_host(state, cx, |host| {
        host.settings.editing.as_mut().is_some_and(edit)
    });
    if changed {
        flush(state, cx);
    }
}

/// Moves the caret of an **already focused** input. Returns whether there was one.
pub(super) fn move_caret(
    state: &Entity<AppState>,
    cx: &mut App,
    move_to: fn(&mut TextFieldState) -> bool,
) -> bool {
    let moved = with_host(state, cx, |host| match host.settings.editing.as_mut() {
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
pub(super) fn accepts(kind: &RowKind, text: &str) -> bool {
    match kind {
        RowKind::Text(_) => true,
        RowKind::Number { .. } => !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()),
        RowKind::Toggle(_) | RowKind::Choice { .. } | RowKind::Fact(_) => false,
    }
}

/// Inserts a bound key's literal character when a text input owns the keyboard.
///
/// §3.8.6 surrenders `j`, `k`, `h`, `l` and `Space` to a **focused** input. gpui dispatches
/// those bindings before any key listener, so the surrender has to happen inside the action
/// handler: there is no other place that sees the key. Returning `false` hands the key back to
/// its normal meaning, which is why an unfocused row — or a number row facing a letter — still
/// navigates.
pub(super) fn insert_literal(state: &Entity<AppState>, literal: &str, cx: &mut App) -> bool {
    if literal.is_empty() {
        return false;
    }
    if !with_host(state, cx, |host| host.settings.editing.is_some()) {
        return false;
    }
    let Some(row) = focused_row(state, cx) else {
        return false;
    };
    if !accepts(&row.kind, literal) {
        return false;
    }
    with_host(state, cx, |host| {
        if let Some(input) = host.settings.editing.as_mut() {
            input.insert(literal);
        }
    });
    flush(state, cx);
    true
}

/// Types a printable key into the focused text or number row.
pub(super) fn type_into_row(state: &Entity<AppState>, event: &KeyDownEvent, cx: &mut App) -> bool {
    let Some(text) = typed_char(event) else {
        return false;
    };
    let Some(row) = focused_row(state, cx) else {
        return false;
    };
    if !accepts(&row.kind, text) {
        return false;
    }
    if !begin_editing(state, cx) {
        return false;
    }
    with_host(state, cx, |host| {
        if let Some(input) = host.settings.editing.as_mut() {
            input.insert(text);
        }
    });
    flush(state, cx);
    true
}

/// Seeds the edit buffer from the focused row. Returns whether that row accepts typing.
pub(super) fn begin_editing(state: &Entity<AppState>, cx: &mut App) -> bool {
    if with_host(state, cx, |host| host.settings.editing.is_some()) {
        return true;
    }
    let Some(row) = focused_row(state, cx) else {
        return false;
    };
    let seed = match row.kind {
        RowKind::Text(value) => value,
        RowKind::Number { value, .. } => value.to_string(),
        _ => {
            with_host(state, cx, |host| host.settings.editing = None);
            return false;
        }
    };
    with_host(state, cx, |host| {
        if host.settings.editing.is_none() {
            host.settings.editing = Some(TextFieldState::from_text(seed));
        }
    });
    true
}

/// Writes the edit buffer into the draft configuration.
pub(super) fn flush(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        let draft = &mut host.settings;
        let Some(id) = draft.row_id() else {
            return;
        };
        if let (Some(config), Some(input)) = (&mut draft.config, &draft.editing) {
            let _valid = commit_value(config, &id, input.text());
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
            Section::Hosts => config.hosts.len().max(1),
            Section::About => about_rows(state.read(cx)).len(),
        }
    })
}

pub(super) fn move_section(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
        host.settings.section = step(host.settings.section, delta, Section::ALL.len());
        host.settings.row = 0;
        host.settings.editing = None;
        host.settings.scroll.scroll_to_item(host.settings.row);
    });
    refresh_rows(state, cx);
    notify(state, cx);
}

pub(super) struct FocusedSetting {
    pub(super) id: RowId,
    pub(super) kind: RowKind,
}
