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
    /// The models each harness reported, refreshed with the rows.
    pub(crate) models: Models,
    /// The first `Esc` on a dirty draft asked; the next one discards (§1 Esc ladder).
    ///
    /// Any edit, a click on a row or a move to another section disarms it, so the question is
    /// always about the draft as it now stands.
    pub(crate) discard_armed: bool,
    /// The row's value as the editor opened on it, which `Esc` puts back.
    pub(crate) edit_seed: Option<String>,
    /// The rule the open number editor's text breaks (`Must be at least 500 ms.`); Save waits.
    pub(crate) edit_rule: Option<String>,
}

/// What one `Esc` did, from the deepest thing it could leave outwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeStep {
    /// Reverted the open editor's row and closed the editor; the dialog stays.
    Revert,
    /// Asked about the unsaved draft, which is the one question the ladder allows.
    Ask,
    /// Nothing left to leave: the shell closes the dialog.
    Close,
}

/// The amber strip the first `Esc` on a dirty draft paints.
pub(super) const DISCARD_WARNING: &str = "Unsaved changes. Press Esc again to discard them.";

/// What the search field holds and what it found.
#[derive(Debug, Default)]
pub struct SearchState {
    /// The typed query.
    pub(crate) query: String,
    /// The rows it matched, across every section.
    pub(crate) hits: Vec<SearchHit>,
    /// How many rows every section holds together: the field's `shown/total` count.
    pub(crate) total: usize,
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
            RowKind::Toggle(true) | RowKind::Rule { on: true, .. } => "on".to_owned(),
            RowKind::Toggle(false) | RowKind::Rule { on: false, .. } => "off".to_owned(),
            RowKind::Choice { value, .. }
            | RowKind::Text(value)
            | RowKind::Model { value, .. }
            | RowKind::Fact(value) => value.clone(),
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
            Some(RowKind::Text(_) | RowKind::Model { .. }) => true,
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

    /// What `Esc` (or Cancel, ✕, the scrim) does next: revert an open editor, then ask once
    /// about a dirty draft, then let the shell close the dialog.
    ///
    /// Split out from [`cancel`] so the whole ladder can be proven without a window. The
    /// caller drops the editor entity on [`EscapeStep::Revert`].
    pub fn escape(&mut self) -> EscapeStep {
        if self.editing.is_some() {
            self.revert_edit();
            return EscapeStep::Revert;
        }
        if self.dirty() && !self.discard_armed {
            self.discard_armed = true;
            return EscapeStep::Ask;
        }
        EscapeStep::Close
    }

    /// Puts back the value the editor opened on and forgets the edit.
    fn revert_edit(&mut self) {
        if let (Some(seed), Some(id)) = (self.edit_seed.take(), self.row_id())
            && let Some(config) = self.config.as_mut()
        {
            // The seed is the committed value the editor started from, so it always parses.
            let _reverted = commit_value(config, &id, &seed);
        }
        self.close_editor();
        self.update_selected();
    }

    /// Forgets the open edit, keeping whatever it already wrote into the draft.
    pub(super) fn close_editor(&mut self) {
        self.editing = None;
        self.edit_seed = None;
        self.edit_rule = None;
    }

    /// The amber strip's sentence, while the first `Esc` on a dirty draft is waiting.
    #[must_use]
    pub fn discard_warning(&self) -> Option<&'static str> {
        (self.discard_armed && self.dirty()).then_some(DISCARD_WARNING)
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
    /// Re-reads the row under the cursor from the draft after an edit to it.
    ///
    /// Every edit passes through here, so it is also what disarms the discard question.
    pub(super) fn update_selected(&mut self) {
        self.discard_armed = false;
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
                // A rule row keeps its live count and badge; only its switch moved.
                (RowKind::Rule { on, .. }, RowKind::Toggle(edited)) => *on = edited,
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
        let kind = kind_of(config, &id, &self.efforts, &self.models)?;
        Some(FocusedSetting { id, kind })
    }
}

/// The kind an editable row draws with, straight from the draft, so the row under the cursor can
/// be refreshed without rebuilding the section. `None` for a keep-alive rule that is gone.
fn kind_of(config: &Config, id: &RowId, efforts: &Efforts, models: &Models) -> Option<RowKind> {
    let number = |value, min| RowKind::Number {
        value,
        min,
        unit: None,
    };
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
        RowId::ClaudeDefaultModel => model_of(config, AgentKind::Claude, models),
        RowId::CodexDefaultModel => model_of(config, AgentKind::Codex, models),
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
        host.settings.discard_armed = false;
        let block = block_of(&host.settings.prepared, host.settings.row);
        host.settings.scroll.scroll_to_item(block);
    });
    end_editing(state, focus, window, cx);
    notify(state, cx);
}

/// The pane child that holds `row`: consecutive rows of one card are one child, and so are
/// consecutive rows with no heading, which share one untitled card.
#[must_use]
pub fn block_of(rows: &[SettingRow], row: usize) -> usize {
    // Every row after the first starts a new card when its heading differs from the row above.
    rows.windows(2)
        .take(row)
        .filter(|pair| pair[1].card != pair[0].card)
        .count()
}

/// `h` / `l` and `\u{2190}` / `\u{2192}` cycle a closed choice while no row owns the keyboard.
pub(super) fn cycle_row(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let Some(row) = focused_row(state, cx) else {
        return;
    };
    with_host(state, cx, |host| {
        let efforts = host.settings.efforts.clone();
        let models = host.settings.models.clone();
        if let Some(config) = host.settings.config.as_mut() {
            match model_harness(&row.id) {
                Some(kind) => cycle_model(config, kind, delta, &models),
                None => cycle(config, &row.id, delta, &efforts),
            }
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
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let (editing, broken) = read_host(state, cx, |host, _| {
        (
            host.settings_input.is_some(),
            host.settings.edit_rule.is_some(),
        )
    });
    if editing {
        // `⏎` in an open editor keeps what was typed and closes the box in place; a value that
        // breaks its rule keeps the box open, because closing it would silently drop the edit.
        if !broken {
            end_editing(state, focus, window, cx);
            notify(state, cx);
        }
        return true;
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
        RowKind::Text(value) | RowKind::Model { value, .. } => (value.clone(), None),
        RowKind::Number { value, min, .. } => (value.to_string(), Some(*min)),
        RowKind::Toggle(_) | RowKind::Choice { .. } | RowKind::Rule { .. } | RowKind::Fact(_) => {
            return false;
        }
    };
    let placeholder = matches!(focused.kind, RowKind::Model { .. }).then_some(MODEL_DEFAULT);
    // The unit the rule names (`Must be at least 500 ms.`) is the prepared row's.
    let unit = read_host(state, cx, |host, _| {
        host.settings
            .prepared
            .get(host.settings.row)
            .and_then(|row| match &row.kind {
                RowKind::Number { unit, .. } => unit.clone(),
                _ => None,
            })
    });
    let seed = text.clone();
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_mono(true, cx);
        input.set_hide_status_line(true, cx);
        // The row's `ValueBox` is the chrome: the editor is only the line of text and its
        // caret, drawn inside the same box the value rested in.
        input.set_embedded(true, cx);
        if let Some(placeholder) = placeholder {
            input.set_placeholder(placeholder, cx);
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
        host.settings.edit_seed = Some(text.clone());
        host.settings.edit_rule = None;
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
        // The rule the typed number breaks replaces the row's helper; Save waits for it.
        let rule = min.and_then(|min| {
            let value = typed.trim().parse::<i64>().unwrap_or(i64::MIN);
            number_rule(value, Some(min), None, unit.as_deref()).map(|rule| rule.to_string())
        });
        with_host(&state, cx, |host| {
            host.settings.editing = Some(typed);
            host.settings.edit_rule = rule;
        });
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
        host.settings.close_editor();
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
        host.settings.discard_armed = false;
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

/// `Esc`, Cancel, ✕ or the scrim: one step down the ladder.
///
/// Returns whether the dialog keeps the key. Only [`EscapeStep::Close`] lets it through to the
/// shell's own `dialog::Cancel`, which is the single path that closes an overlay.
pub(super) fn cancel(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let step = with_host(state, cx, |host| host.settings.escape());
    match step {
        EscapeStep::Revert => {
            // The draft is already reverted; this drops the editor and hands the keys back.
            end_editing(state, focus, window, cx);
            notify(state, cx);
            true
        }
        EscapeStep::Ask => {
            notify(state, cx);
            true
        }
        EscapeStep::Close => false,
    }
}

pub(super) struct FocusedSetting {
    pub(super) id: RowId,
    pub(super) kind: RowKind,
}
