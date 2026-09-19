use super::*;

/// Editable board configuration, including its backend and that backend's own settings.
#[derive(Debug, Clone, Default)]
pub(crate) struct BoardSettingsState {
    /// Identity of this opening, used to ignore replies to discarded drafts.
    pub(super) generation: u64,
    /// A request is in flight; retain and freeze the draft until it answers.
    pub(super) saving: bool,
    /// Board being configured.
    pub(super) board_id: Option<BoardId>,
    /// Display name.
    pub(super) name: String,
    /// Card identifier prefix.
    pub(super) prefix: String,
    /// Default repository for card worktrees.
    pub(super) default_repo_id: Option<RepoId>,
    /// Move unstarted cards to Started when creating a worktree.
    pub(super) start_on_worktree: bool,
    /// File a card made here as a new issue on the board's backend.
    ///
    /// It defaults to `false`, so a GUI-only session on a linked board used to create card
    /// after card that never became an issue, with nothing anywhere saying why. The row is
    /// drawn disabled on a local board, where there is no backend to push anything to.
    pub(super) push_new_cards: bool,
    /// Conflict resolution policy.
    pub(super) conflict_policy: ConflictPolicy,
    /// Backend registry key the draft currently selects.
    pub(super) backend_kind: String,
    /// The kind the board was opened with, so returning to it restores its settings.
    pub(super) original_kind: String,
    /// The settings the board was opened with.
    pub(super) original_settings: serde_json::Value,
    /// The selected backend's schema rows, with their values.
    pub(super) rows: Vec<BackendRow>,
    /// Which row carries the cursor.
    pub(super) row: usize,
    /// Scroll position of the row list.
    ///
    /// A backend with nine settings makes this form taller than the card ever gets, and a
    /// dialog whose last rows are clipped away is a dialog whose Save the user cannot reach.
    pub(super) scroll: gpui::ScrollHandle,
    /// The row the scroller was last asked to reveal.
    ///
    /// gpui's `FirstVisible` strategy re-anchors on the row it is given, so calling it on
    /// every render pinned the list to the focused row: the wheel scrolled and snapped
    /// straight back, and a nine-row backend form could not be read past the cursor.
    pub(super) revealed: Option<usize>,
    /// Why the draft cannot be saved.
    pub(super) error: Option<String>,
}

impl BoardSettingsState {
    /// Completes only the request belonging to this opening.
    pub(super) fn finish_save(&mut self, generation: u64, error: Option<String>) -> bool {
        if self.generation != generation || !self.saving {
            return false;
        }
        self.saving = false;
        self.error = error;
        self.error.is_none()
    }

    /// Every row of this draft, in the order they are drawn.
    #[must_use]
    pub(super) fn rows(&self) -> Vec<SettingRow> {
        let mut rows = FIXED_ROWS.to_vec();
        rows.extend((0..self.rows.len()).map(SettingRow::BackendSetting));
        rows
    }

    /// The row the cursor is on.
    #[must_use]
    pub(super) fn focused(&self) -> SettingRow {
        self.rows()
            .get(self.row)
            .copied()
            .unwrap_or(SettingRow::Name)
    }

    /// Whether the draft still points at the kind the board is stored with.
    #[must_use]
    pub(super) fn keeps_kind(&self) -> bool {
        self.backend_kind == self.original_kind
    }

    /// Points the draft at `kind`, rebuilding its rows from that backend's schema.
    ///
    /// Returning to the board's own kind restores the settings it was opened with; every other
    /// kind starts from nothing, which is the same rule the daemon applies (`BOARD-JIRA` §4:
    /// on a kind change the settings come from the patch and are never carried over).
    pub(super) fn select_backend(&mut self, kind: &str, schema: &[PropertySchema]) {
        self.backend_kind = kind.to_owned();
        let base = if self.keeps_kind() {
            self.original_settings.clone()
        } else {
            serde_json::Value::Null
        };
        self.rows = backend_rows(schema, &base);
        self.row = self.row.min(self.rows().len().saturating_sub(1));
        self.error = None;
    }

    /// The settings object this draft would send.
    ///
    /// A backend that configures nothing sends `null`, not `{}`: that is what a board with no
    /// settings stores, and a patch that swapped one for the other would differ from the stored
    /// backend on every save — writing the document, resetting the cursor and announcing a
    /// change nobody made.
    #[must_use]
    pub(super) fn settings_json(&self) -> serde_json::Value {
        let base = if self.keeps_kind() {
            self.original_settings.clone()
        } else {
            serde_json::Value::Null
        };
        let settings = rows_to_settings(&base, &self.rows);
        match &settings {
            serde_json::Value::Object(object) if object.is_empty() => serde_json::Value::Null,
            _ => settings,
        }
    }

    /// The failing rule, in the wording the contract states it in.
    #[must_use]
    pub(super) fn validate(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("name must not be empty".to_owned());
        }
        let prefix = self.prefix.trim();
        if !prefix
            .chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
        {
            return Some("prefix must be A\u{2013}Z and 0\u{2013}9 only".to_owned());
        }
        if prefix.is_empty() || prefix.len() > MAX_PREFIX {
            return Some(format!("prefix must be 1\u{2013}{MAX_PREFIX} characters"));
        }
        rows_error(&self.rows)
    }

    /// The focused row's seed text, when that row owns a live input.
    #[must_use]
    pub(super) fn focused_text(&self) -> Option<String> {
        match self.focused() {
            SettingRow::Name => Some(self.name.clone()),
            SettingRow::Prefix => Some(self.prefix.clone()),
            SettingRow::BackendSetting(index) => {
                let row = self.rows.get(index)?;
                if !row.is_text() {
                    return None;
                }
                Some(row.value.clone())
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn focused_backend_row(&self) -> Option<&BackendRow> {
        match self.focused() {
            SettingRow::BackendSetting(index) => self.rows.get(index),
            _ => None,
        }
    }

    /// Mirror the focused entity after every `Changed` event; the entity remains the editing
    /// source of truth and this copy is only the serializable board draft used by Save.
    pub(super) fn set_focused_text(&mut self, text: &str) {
        if self.saving {
            return;
        }
        match self.focused() {
            SettingRow::Name => self.name = text.to_owned(),
            // The contract stores prefixes uppercase, so the field shows what it will store.
            SettingRow::Prefix => self.prefix = text.to_uppercase(),
            SettingRow::BackendSetting(index) => match self.rows.get_mut(index) {
                Some(row) => row.value = text.to_owned(),
                None => return,
            },
            _ => return,
        }
        self.error = None;
    }
}

/// Where a repository sits in the cycler: `0` is `none`, the rest follow the list.
#[must_use]
pub(super) fn repo_position(repos: &[RepoId], current: Option<&RepoId>) -> usize {
    current
        .and_then(|repo| repos.iter().position(|entry| entry == repo))
        .map_or(0, |index| index + 1)
}

/// Whether the cycler has a position for this value at all.
///
/// A board can hold a `defaultRepoId` this context does not list — the repository moved, or the
/// snapshot has not arrived yet — and that value belongs to no step of the cycle.
#[must_use]
pub(super) fn repo_listed(repos: &[RepoId], current: Option<&RepoId>) -> bool {
    current.is_none_or(|repo| repos.iter().any(|entry| entry == repo))
}

/// `j` / `k`: move the cursor while no text row owns the keyboard.
pub(super) fn move_row(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
        let len = host.board_settings.rows().len();
        host.board_settings.row = step(host.board_settings.row, delta, len);
    });
    cx.stop_propagation();
}

/// `h` / `l`: cycle a closed choice while no text row owns the keyboard.
pub(super) fn cycle(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let app = state.read(cx);
    let repos = repo_choices(app);
    let selected = read_host(state, cx, |host, _| {
        host.board_settings.backend_kind.clone()
    });
    let kinds = backend_kinds(state.read(cx), &selected);
    let next_kind = kinds
        .iter()
        .position(|kind| *kind == selected)
        .map(|index| kinds[step(index, delta, kinds.len())].clone())
        .unwrap_or(selected);
    let schema = schema_for(state.read(cx), &next_kind);
    with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if draft.saving {
            return;
        }
        match draft.focused() {
            SettingRow::DefaultRepo => {
                // A value the list does not carry has no position to step from: wrapping out
                // of an invented `0` sent `h` to the *last* repository. From off the grid the
                // step lands on the neighbour it names — `l` on the first repository, `h` on
                // "none" — and never anywhere the arrows did not point.
                if !repo_listed(&repos, draft.default_repo_id.as_ref()) {
                    draft.default_repo_id = (delta > 0).then(|| repos.first().cloned()).flatten();
                } else {
                    let index = repo_position(&repos, draft.default_repo_id.as_ref());
                    let next = step(index, delta, repos.len() + 1);
                    draft.default_repo_id =
                        next.checked_sub(1).and_then(|at| repos.get(at).cloned());
                }
            }
            SettingRow::ConflictPolicy => {
                if draft.backend_kind == BackendRef::LOCAL {
                    return;
                }
                let index = POLICIES
                    .iter()
                    .position(|policy| *policy == draft.conflict_policy)
                    .unwrap_or(0);
                draft.conflict_policy = POLICIES[step(index, delta, POLICIES.len())];
                return;
            }
            // `h` is off and `l` is on, never a flip: every other cycler row on this dialog
            // steps by `delta`, and a row that flipped on both made `h l` land somewhere other
            // than where it started.
            SettingRow::StartOnWorktree => draft.start_on_worktree = delta > 0,
            SettingRow::PushNewCards => {
                if draft.backend_kind == BackendRef::LOCAL {
                    return;
                }
                draft.push_new_cards = delta > 0;
            }
            SettingRow::Backend => {
                draft.select_backend(&next_kind, &schema);
                return;
            }
            SettingRow::BackendSetting(index) => {
                cycle_backend_row(draft, index, delta);
                return;
            }
            SettingRow::Name | SettingRow::Prefix => {}
        }
        draft.error = None;
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// `h` / `l` inside a backend row: step a number, cycle a select, flip a flag.
pub(super) fn cycle_backend_row(draft: &mut BoardSettingsState, index: usize, delta: isize) {
    let Some(row) = draft.rows.get_mut(index) else {
        return;
    };
    match row.kind {
        // `h` is off and `l` is on, for the reason the fixed toggle rows are: a flip on both
        // makes `h l` land on the opposite of where it started. `space` is the toggle.
        PropertyKind::Bool => {
            row.value = (delta > 0).to_string();
            row.present = true;
        }
        PropertyKind::Number => {
            // Stepping *down* from an unset row would write `0` — a value the daemon is not
            // using, that `backend_element` refuses to draw, that `ctrl-u` is the only way out
            // of, and that on two of Jira's three number rows either fails the save or turns
            // every pull into a full one. An empty row has nothing below it.
            if row.value.trim().is_empty() && delta < 0 {
                return;
            }
            let current: i64 = row.value.trim().parse().unwrap_or(0);
            row.value = current
                .saturating_add(i64::try_from(delta).unwrap_or(0))
                .max(0)
                .to_string();
        }
        PropertyKind::Select if !row.options.is_empty() => {
            let at = row
                .options
                .iter()
                .position(|option| option.value == row.value)
                .unwrap_or(0);
            row.value = row.options[step(at, delta, row.options.len())]
                .value
                .clone();
        }
        _ => return,
    }
    draft.error = None;
}

/// `space`: toggle the focused flag row; anywhere else it is a space.
pub(super) fn toggle(state: &Entity<AppState>, cx: &mut App) {
    let changed = with_host(state, cx, |host| {
        if host.board_settings.saving {
            return false;
        }
        match host.board_settings.focused() {
            SettingRow::StartOnWorktree => {
                host.board_settings.start_on_worktree = !host.board_settings.start_on_worktree;
                true
            }
            SettingRow::PushNewCards => {
                if host.board_settings.backend_kind == BackendRef::LOCAL {
                    return false;
                }
                host.board_settings.push_new_cards = !host.board_settings.push_new_cards;
                true
            }
            SettingRow::BackendSetting(index) => {
                let Some(row) = host.board_settings.rows.get_mut(index) else {
                    return false;
                };
                if !row.is_flag() {
                    return false;
                }
                row.value = (!row.flag()).to_string();
                row.present = true;
                true
            }
            _ => false,
        }
    });
    if changed {
        notify(state, cx);
    }
    cx.stop_propagation();
}

/// Rebuild the one row-scoped entity after focus moves, dropping it on non-text rows.
pub(super) fn materialize_input(
    state: &Entity<AppState>,
    window: Option<&mut Window>,
    dialog_focus: Option<&FocusHandle>,
    cx: &mut App,
) {
    let spec = read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        let row = draft.focused();
        let text = draft.focused_text()?;
        let (label, placeholder, mono, digits) = match row {
            SettingRow::Name => (row.label().to_owned(), "Fleet", false, false),
            SettingRow::Prefix => (row.label().to_owned(), "FLT", true, false),
            SettingRow::BackendSetting(index) => {
                let backend = draft.rows.get(index)?;
                let label = if backend.required {
                    format!("{} \u{2217}", backend.name)
                } else {
                    backend.name.clone()
                };
                (
                    label,
                    input_placeholder(backend),
                    backend.kind == PropertyKind::Number,
                    backend.kind == PropertyKind::Number,
                )
            }
            _ => return None,
        };
        Some((row, text, label, placeholder, mono, digits))
    });
    let Some((row, text, label, placeholder, mono, digits)) = spec else {
        with_host(state, cx, |host| {
            host.board_settings_input = None;
            host.board_settings_input_subscription = None;
        });
        if let (Some(window), Some(focus)) = (window, dialog_focus) {
            window.focus(focus, cx);
        }
        notify(state, cx);
        return;
    };
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_text(text, cx);
        input.set_label(Some(label.into()), cx);
        input.set_placeholder(placeholder, cx);
        input.set_mono(mono, cx);
        if digits {
            input.set_filter(Some(|character| character.is_ascii_digit()), cx);
        }
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    let weak_host = host.downgrade();
    let subscription = cx.subscribe(&input, move |input, event, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(host) = weak_host.upgrade() else {
            return;
        };
        let raw = input.read(cx).text().to_owned();
        let normalized = if row == SettingRow::Prefix {
            raw.to_uppercase()
        } else {
            raw
        };
        // Mirror prefixes in their stored uppercase form, but leave the live editor untouched so
        // selection and undo history remain user edits. Moving away and back materializes that
        // normalized draft value.
        host.update(cx, |host, cx| {
            if host.board_settings.focused() == row {
                host.board_settings.set_focused_text(&normalized);
                cx.notify();
            }
        });
    });
    host.update(cx, |host, _| {
        host.board_settings_input = Some(input.clone());
        host.board_settings_input_subscription = Some(subscription);
    });
    if let Some(window) = window {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    notify(state, cx);
}
