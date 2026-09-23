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
    /// The effort vocabulary the Agents section offers, refreshed with the rows.
    pub(crate) efforts: Efforts,
    /// The search the header's field holds, while it holds one.
    pub(crate) search: Option<SearchState>,
    /// Whether the search field owns the keyboard, which publishes `Dialog > SettingsSearch`.
    pub(crate) search_focused: bool,
    /// The search results' scroll position.
    pub(crate) hit_scroll: gpui::ScrollHandle,
}

/// What the search field holds and what it found.
#[derive(Debug, Default)]
pub struct SearchState {
    /// The typed query.
    pub(crate) query: String,
    /// The rows it matched, across every section.
    pub(crate) hits: Vec<SearchHit>,
    /// The hit `Enter` jumps to.
    pub(crate) cursor: usize,
}

impl SearchState {
    /// Replaces the hits and keeps the cursor on one of them.
    pub(super) fn set_hits(&mut self, hits: Vec<SearchHit>) {
        self.hits = hits;
        self.cursor = self.cursor.min(self.hits.len().saturating_sub(1));
    }

    /// Whether the pane shows hits rather than the section: a query is typed.
    #[must_use]
    pub fn active(&self) -> bool {
        !self.query.trim().is_empty()
    }
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

    /// The row under the cursor as the harness reads it: `<label> = <value>`, a switch's value
    /// `on` or `off`. Empty before the configuration has loaded.
    #[must_use]
    pub fn cursor_summary(&self) -> String {
        let Some(row) = self.prepared.get(self.row) else {
            return String::new();
        };
        let value = match &row.kind {
            RowKind::Toggle(true) => "on".to_owned(),
            RowKind::Toggle(false) => "off".to_owned(),
            RowKind::Choice { value, .. } | RowKind::Text(value) | RowKind::Fact(value) => {
                value.clone()
            }
            RowKind::Number { value, .. } => value.to_string(),
        };
        format!("{} = {value}", row.label)
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

/// Writes a complete, valid edit back into the draft.
pub fn commit_value(config: &mut Config, id: &RowId, raw: &str) -> bool {
    let number = |min: i64| raw.trim().parse::<i64>().ok().filter(|value| *value >= min);
    match id {
        RowId::ClaudeCommand => config.agent_commands.claude = raw.to_owned(),
        RowId::CodexCommand => config.agent_commands.codex = raw.to_owned(),
        RowId::ClaudeBinary => config.agent_binaries.claude = raw.to_owned(),
        RowId::CodexBinary => config.agent_binaries.codex = raw.to_owned(),
        RowId::ClaudeDefaultModel => config.native_agents.claude.model = optional(raw),
        RowId::CodexDefaultModel => config.native_agents.codex.model = optional(raw),
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
    pub(super) fn update_selected(&mut self) {
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

    pub(super) fn row_id(&self) -> Option<RowId> {
        use RowId::*;
        let ids: &[RowId] = match self.current_section() {
            Section::Agents => &[
                Agent,
                ClaudeCommand,
                ClaudeBinary,
                ClaudeDefaultMode,
                ClaudeDefaultModel,
                ClaudeDefaultEffort,
                CodexCommand,
                CodexBinary,
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
            Section::General | Section::Hosts | Section::About => return Some(ReadOnly),
        };
        ids.get(self.row).cloned()
    }

    pub(super) fn focused_row(&self) -> Option<FocusedSetting> {
        let config = self.config.as_ref()?;
        let id = self.row_id()?;
        let kind = kind_of(config, &id, &self.efforts)?;
        Some(FocusedSetting { id, kind })
    }
}

/// The kind an editable row draws with, straight from the draft, so the row under the cursor can
/// be refreshed without rebuilding the section. `None` for a keep-alive rule that is gone.
fn kind_of(config: &Config, id: &RowId, efforts: &Efforts) -> Option<RowKind> {
    let number = |value, min| RowKind::Number {
        value,
        min,
        unit: None,
    };
    let text = |value: &Option<String>| RowKind::Text(value.clone().unwrap_or_default());
    if let Some(choice) = choice_of(config, id, efforts) {
        return Some(choice.kind());
    }
    if let Some(on) = switch_of(config, id) {
        return Some(RowKind::Toggle(on));
    }
    Some(match id {
        RowId::ClaudeCommand => RowKind::Text(config.agent_commands.claude.clone()),
        RowId::CodexCommand => RowKind::Text(config.agent_commands.codex.clone()),
        RowId::ClaudeBinary => RowKind::Text(config.agent_binaries.claude.clone()),
        RowId::CodexBinary => RowKind::Text(config.agent_binaries.codex.clone()),
        RowId::ClaudeDefaultModel => text(&config.native_agents.claude.model),
        RowId::CodexDefaultModel => text(&config.native_agents.codex.model),
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
        RowId::ReadOnly => RowKind::Fact(String::new()),
        RowId::KeepAliveRule(_) => return None,
        RowId::Agent
        | RowId::CloneProtocol
        | RowId::KeepFinishedFor
        | RowId::TrashRetention
        | RowId::HotPoolSize
        | RowId::ClaudeDefaultMode
        | RowId::CodexDefaultMode
        | RowId::ClaudeDefaultEffort
        | RowId::CodexDefaultEffort
        | RowId::SleepOnSwitch
        | RowId::WarnBeforeQuit => return None,
    })
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
        let block = block_of(&host.settings.prepared, host.settings.row);
        host.settings.scroll.scroll_to_item(block);
    });
    end_editing(state, focus, window, cx);
    notify(state, cx);
}

/// The pane child that holds `row`: consecutive rows of one card are one child.
#[must_use]
pub fn block_of(rows: &[SettingRow], row: usize) -> usize {
    // Every row after the first starts a new child unless it continues the card above it.
    rows.windows(2)
        .take(row)
        .filter(|pair| pair[1].card.is_none() || pair[1].card != pair[0].card)
        .count()
}

/// `h` / `l` and `\u{2190}` / `\u{2192}` cycle a closed choice while no row owns the keyboard.
pub(super) fn cycle_row(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let Some(row) = focused_row(state, cx) else {
        return;
    };
    with_host(state, cx, |host| {
        let efforts = host.settings.efforts.clone();
        if let Some(config) = host.settings.config.as_mut() {
            cycle(config, &row.id, delta, &efforts);
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
    let seed = text.clone();
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_mono(true, cx);
        input.set_hide_status_line(true, cx);
        // The row draws the box (`ValueField`, `NumberField`) and keeps its label beside it, so
        // the editor is only the line of text and its caret.
        input.set_embedded(true, cx);
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
            Section::Agents => 11,
            Section::Jobs | Section::Github => 3,
            Section::Sleep => 2 + config.sleep.keep_alive.len(),
            Section::Pool => 4,
            Section::Status => 2,
            Section::General => config.windows.len(),
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
    let section = read_host(state, cx, |host, _| {
        step(host.settings.section, delta, Section::ALL.len())
    });
    goto_row(state, section, 0, focus, window, cx);
}

/// Shows `section` with the cursor on `row`: a click on the rail, `Tab`, or a search hit.
///
/// Whatever owned the keyboard — a row editor or the search field — gives it back to the dialog.
pub(super) fn goto_row(
    state: &Entity<AppState>,
    section: usize,
    row: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    end_editing(state, focus, window, cx);
    end_search(state, focus, window, cx);
    with_host(state, cx, |host| {
        host.settings.section = section.min(Section::ALL.len() - 1);
        host.settings.row = 0;
    });
    refresh_rows(state, cx);
    let len = focused_len(state, cx);
    with_host(state, cx, |host| {
        host.settings.row = row.min(len.saturating_sub(1));
        let block = block_of(&host.settings.prepared, host.settings.row);
        host.settings.scroll.scroll_to_item(block);
    });
    notify(state, cx);
}

pub(super) struct FocusedSetting {
    pub(super) id: RowId,
    pub(super) kind: RowKind,
}
