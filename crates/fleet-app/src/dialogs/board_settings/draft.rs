use std::cell::RefCell;

use super::*;

/// Where each cursor row was painted last frame, by row index, shared between the rows'
/// prepaint listeners and the reveal in `view.rs`.
pub(super) type RowBounds = Rc<RefCell<Vec<Option<gpui::Bounds<gpui::Pixels>>>>>;

/// What a context board with no worktree "runs in" (BOARD §11.10).
pub(super) const CONTEXT_LABEL: &str = "the context";

/// What one `esc` did, from the deepest thing it could leave outwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EscapeStep {
    /// Closed the open editor, putting back the value it opened with.
    Editor,
    /// Cancelled a delete that was waiting for a target.
    Delete,
    /// Left one column's form for the list.
    Column,
    /// Left one schedule's form for the Schedules list.
    Schedule,
    /// Asked about the unsaved draft, which is the one question §5.4 allows.
    Ask,
    /// Nothing left to leave: the shell closes the dialog.
    Close,
}

/// Editable board configuration, including its backend and that backend's own settings.
#[derive(Debug, Clone, Default)]
pub(crate) struct BoardSettingsState {
    /// Which section of the rail is open (contracts §5.4).
    ///
    /// Defaults to the one this app session last used, so `,` reopens where the user left off.
    pub(super) section: BoardSection,
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
    /// The board the draft was seeded from.
    ///
    /// The column rules live in `fleet-core` and take a whole `Board`; this is the one the
    /// dialog is editing, kept so the rules can be asked about a candidate column vector
    /// without the dialog inventing a board of its own.
    ///
    /// Behind an `Rc` because the render reads the draft by cloning it, once per frame: what
    /// the form costs a frame has to stay the size of the form, not the size of the board
    /// (`docs/APP-CONTRACTS.md`, "render prepares nothing").
    pub(super) board: Option<Rc<Board>>,
    /// Live runs this board may have at once, as the model stores it: `None` is one.
    pub(super) max_live_runs: Option<u32>,
    /// The column vector being edited (contracts §5.4). Nothing here is sent until `^s`.
    pub(super) columns: Vec<ColumnDraft>,
    /// The column vector the board was opened with, for the dirty check.
    pub(super) original_columns: Vec<ColumnDraft>,
    /// Which column the Columns pane has drilled into, if any.
    pub(super) opened_column: Option<usize>,
    /// Whether an editor is open over the cursor row.
    ///
    /// Every text and number row opens its editor on `\u{23ce}` (or a click on its box), never
    /// on arrival (§3.8.6): that is what leaves `j` / `k`, `n`, `d`, `J`/`K` and `P` meaning
    /// themselves wherever the cursor lands.
    pub(super) editing: bool,
    /// The text the open editor started from, which `esc` puts back.
    ///
    /// The editor mirrors every keystroke into the draft, so reverting is writing this back;
    /// `\u{23ce}` keeps what was typed and simply closes the box.
    pub(super) editing_seed: Option<String>,
    /// `max_live_runs` as it was when its editor opened, which `esc` puts back as it was.
    ///
    /// The seed text of an unset row is `1`, and writing that back would store `Some(1)` over a
    /// `None` that means the same thing — a reverted row that reads as unsaved.
    pub(super) editing_seed_runs: Option<u32>,
    /// Where each cursor row was painted last frame, by row, for the reveal (`view.rs`).
    ///
    /// Shared with the rows' prepaint listeners; a row's card is not enough to scroll to when
    /// the card is taller than the pane.
    pub(super) row_bounds: RowBounds,
    /// The models and efforts the column and schedule forms offer, read at seed time.
    pub(super) catalogue: Rc<Catalogue>,
    /// Whether this board may carry automation at all.
    ///
    /// A context board has no checkout to run in and a Jira board's columns are the backend's,
    /// so a column's form folds its automation rows away under one callout (§5.4): the order
    /// and the names are still this board's to change.
    pub(super) automation_locked: bool,
    /// Where the board's runs execute, stated read-only in General (BOARD §11.10).
    ///
    /// Not editable in v1: changing it on a board with live runs would strand them.
    pub(super) run_location: RunLocation,
    /// The cards each column holds, in board order, for the delete rule.
    ///
    /// Behind an `Rc` for the reason the board above is: this one is the whole board's cards,
    /// and cloning a card id per card per frame is the one part of this draft that grows
    /// without bound.
    pub(super) cards_by_column: Rc<Vec<(StatusId, Vec<CardId>)>>,
    /// The column `d` was pressed on, waiting for the user to point at where its cards go.
    ///
    /// A column holding cards cannot simply vanish: §5.4 requires a target, and the cards are
    /// moved one `MoveCard` at a time before the column leaves the draft.
    pub(super) pending_delete: Option<usize>,
    /// Whether `esc` has already asked about the unsaved draft.
    pub(super) discard_armed: bool,
    /// What the last list key did, when it did something worth saying.
    pub(super) notice: Option<String>,
    /// The Columns pane's rows, prepared whenever the draft changes.
    pub(super) prepared: Vec<ColumnRow>,
    /// Why the draft cannot be saved.
    pub(super) error: Option<String>,
    /// Whether the board can have schedules, read at seed time: the daemon serves `schedules`
    /// and stores the board (`AppState::board_takes_schedules`). The rail shows Schedules
    /// only then.
    pub(super) schedules_supported: bool,
    /// Why it cannot, when it cannot: what `T` says instead of opening the section.
    pub(super) schedules_refusal: Option<String>,
    /// The Schedules section's list and form.
    pub(super) schedules: SchedulesPane,
}

impl BoardSettingsState {
    /// The open section's own title, for the harness `dialog.section` field.
    ///
    /// The section is draft state, so the projection cannot read it from `AppState`; this is the
    /// one accessor the dialog host uses to state it (`docs/TESTING-HARNESS.md` §3).
    pub(crate) const fn section_title(&self) -> &'static str {
        self.section.title()
    }

    /// What to call the one row-scoped editor, for the harness `dialog.fields` entry beside it.
    ///
    /// Every other row of this dialog is a cycler whose value the projection already carries —
    /// a column's name and its `\u{26a1}` mark are `settings.columns`, and the board's own rows
    /// are `AppState` — so the live editor is the only place a value *typed* into Board settings
    /// can be read back at all. Naming it after the row it belongs to is what lets a scenario
    /// say which row it read (`docs/TESTING-HARNESS.md` §11).
    ///
    /// `None` while no row owns an editor, which is the same condition
    /// [`Self::focused_text`] answers: a cycler, or a locked automation row.
    pub(crate) fn editor_row_name(&self) -> Option<String> {
        self.focused_text()?;
        Some(match self.focused() {
            SettingRow::BackendSetting(index) => self.rows.get(index)?.name.to_lowercase(),
            SettingRow::ColumnField(field) => field.label().to_lowercase(),
            SettingRow::ScheduleField(field) => field.label().to_lowercase(),
            row => row.label().to_lowercase(),
        })
    }

    /// Completes only the request belonging to this opening.
    pub(super) fn finish_save(&mut self, generation: u64, error: Option<String>) -> bool {
        if self.generation != generation || !self.saving {
            return false;
        }
        self.saving = false;
        self.error = error;
        self.error.is_none()
    }

    /// The rows the open section draws, in order.
    #[must_use]
    pub(super) fn rows(&self) -> Vec<SettingRow> {
        match self.section {
            BoardSection::General => GENERAL_ROWS.to_vec(),
            BoardSection::Backend => {
                let mut rows = vec![SettingRow::Backend];
                rows.extend((0..self.rows.len()).map(SettingRow::BackendSetting));
                rows
            }
            BoardSection::Columns => self.prepared.iter().map(|row| row.row).collect(),
            BoardSection::Schedules => self.schedules.rows(),
        }
    }

    /// Recomputes the Columns pane's rows.
    ///
    /// Called from every path that changes the column draft, so `render` never formats a value
    /// or decides a disabled flag (`docs/APP-CONTRACTS.md`).
    pub(super) fn prepare(&mut self) {
        self.prepared = prepare(
            &self.columns,
            self.opened_column,
            self.automation_locked,
            &self.catalogue,
        );
        let len = self.rows().len();
        if self.row >= len {
            self.row = len.saturating_sub(1);
        }
    }

    /// Live runs this board allows at once; one when it says nothing.
    #[must_use]
    pub(super) fn live_run_limit(&self) -> u32 {
        self.max_live_runs.unwrap_or(1)
    }

    /// `h` / `l` on the throttle row, clamped rather than wrapped.
    ///
    /// Wrapping would put `h` on one run at eight — the difference between one agent in the
    /// checkout and eight of them editing the same files.
    ///
    /// A step that lands where it started writes nothing: `h` on one run would otherwise turn
    /// the stored `None` into `Some(1)`, a draft that reads as unsaved and saves a no-op.
    pub(super) fn step_live_runs(&mut self, delta: isize) -> bool {
        let current = self.live_run_limit();
        let next = current
            .saturating_add_signed(i32::try_from(delta).unwrap_or(0))
            .clamp(1, MAX_LIVE_RUNS_PER_BOARD);
        if next == current {
            return false;
        }
        self.max_live_runs = Some(next);
        true
    }

    /// What `esc` does next, from the deepest thing it can leave outwards.
    ///
    /// Split out from [`cancel`] so the whole ladder — and the one question §5.4 allows — can
    /// be proven without a window. The editor step reverts the row and stays where it is: the
    /// box closes in place over the value it opened with (§3.8.6).
    pub(super) fn escape(&mut self) -> EscapeStep {
        if self.editing {
            if let Some(seed) = self.editing_seed.take() {
                self.set_focused_text(&seed);
            }
            // The seed text says `1` for a row stored as `None`; the value goes back as stored.
            if self.focused() == SettingRow::MaxLiveRuns {
                self.max_live_runs = self.editing_seed_runs;
            }
            self.editing = false;
            return EscapeStep::Editor;
        }
        if self.pending_delete.take().is_some() {
            self.notice = None;
            return EscapeStep::Delete;
        }
        if let Some(step) = self.escape_schedules() {
            return step;
        }
        if let Some(index) = self.opened_column.take() {
            self.row = index.min(self.columns.len().saturating_sub(1));
            self.prepare();
            return EscapeStep::Column;
        }
        // §5.4: one question, once. The second `esc` discards, which is what the amber strip
        // says ([`Self::footer_strip`]).
        if self.dirty() && !self.discard_armed {
            self.discard_armed = true;
            return EscapeStep::Ask;
        }
        EscapeStep::Close
    }

    /// Whether anything the dialog would save differs from what it opened with.
    ///
    /// `esc` asks once on a dirty draft and closes a clean one, so this is what stands between
    /// a user and four minutes of column edits. It compares against the board itself rather
    /// than against a second copy of every row: the board is already kept for the column
    /// rules, and one source of "what was stored" cannot drift from another.
    #[must_use]
    pub(super) fn dirty(&self) -> bool {
        if self.columns != self.original_columns {
            return true;
        }
        let Some(board) = self.board.as_ref() else {
            return false;
        };
        // An empty settings object and no settings at all are the same board; `settings_json`
        // already normalises its own side, and this normalises the stored one, so a dialog
        // opened and closed on a backend that configures nothing is not "unsaved".
        let stored = match &self.original_settings {
            serde_json::Value::Object(object) if object.is_empty() => &serde_json::Value::Null,
            other => other,
        };
        self.name != board.name
            || self.prefix != board.prefix
            || self.default_repo_id != board.default_repo_id
            || self.start_on_worktree != board.settings.start_on_worktree
            || self.push_new_cards != board.settings.push_new_cards
            || self.conflict_policy != board.settings.conflict_policy
            || self.max_live_runs != board.settings.max_live_runs
            || !self.keeps_kind()
            || self.settings_json() != *stored
    }

    /// The column the Columns pane has drilled into.
    #[must_use]
    pub(super) fn opened(&self) -> Option<&ColumnDraft> {
        self.columns.get(self.opened_column?)
    }

    /// Whether the Columns pane is showing its list rather than one column's form.
    #[must_use]
    pub(super) fn in_column_list(&self) -> bool {
        self.section == BoardSection::Columns && self.opened_column.is_none()
    }

    /// The cards a column holds, by its position in the draft.
    #[must_use]
    pub(super) fn cards_in(&self, index: usize) -> &[CardId] {
        self.columns
            .get(index)
            .and_then(|column| {
                self.cards_by_column
                    .iter()
                    .find(|(id, _)| *id == column.status.id)
            })
            .map_or(&[][..], |(_, cards)| cards.as_slice())
    }

    /// The row the cursor is on.
    #[must_use]
    pub(super) fn focused(&self) -> SettingRow {
        self.rows()
            .get(self.row)
            .copied()
            .unwrap_or(SettingRow::NoRow)
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
        if let Some(message) = self.name_rule().or_else(|| self.prefix_rule()) {
            return Some(message);
        }
        if let Some(message) = rows_error(&self.rows) {
            return Some(message);
        }
        // The column rules are `fleet-core`'s, so the dialog refuses a routing loop, a nameless
        // skill and a reserved env key in the daemon's own words before anything is sent.
        validate(self.board.as_deref()?, &self.columns, self.live_run_limit())
    }

    /// The rule the board's name breaks, if it breaks one.
    #[must_use]
    pub(super) fn name_rule(&self) -> Option<String> {
        self.name
            .trim()
            .is_empty()
            .then(|| "name must not be empty".to_owned())
    }

    /// The rule the board's prefix breaks, charset before length.
    #[must_use]
    pub(super) fn prefix_rule(&self) -> Option<String> {
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
        None
    }

    /// The rule `Max live runs` breaks, as its row states it: `Must be between 1 and 8.`
    #[must_use]
    pub(super) fn live_runs_rule(&self) -> Option<SharedString> {
        number_rule(
            i64::from(self.live_run_limit()),
            Some(1),
            Some(i64::from(MAX_LIVE_RUNS_PER_BOARD)),
            None,
        )
    }

    /// Where this board's runs execute, as the subtitle and the `Runs in` fact state it.
    ///
    /// A context board with no worktree "runs in" nothing a checkout could name: `this
    /// worktree` above a callout saying the board runs in the context contradicts itself, so
    /// that board says `the context` (BOARD §11.10).
    #[must_use]
    pub(super) fn runs_in(&self) -> &'static str {
        let no_worktree = self
            .board
            .as_ref()
            .is_some_and(|board| board.worktree_id.is_none());
        if no_worktree && self.run_location.is_board_worktree() {
            CONTEXT_LABEL
        } else {
            run_location_label(self.run_location)
        }
    }

    /// The dialog's subtitle: `Fleet board · FLT · this worktree` (§3.8.6).
    ///
    /// Read from the board as stored, not from the draft: it names the board being configured,
    /// and a name half-typed into the Name row is not that board's name yet.
    #[must_use]
    pub(super) fn subtitle(&self) -> String {
        let (name, prefix) = self
            .board
            .as_ref()
            .map_or((self.name.as_str(), self.prefix.as_str()), |board| {
                (board.name.as_str(), board.prefix.as_str())
            });
        format!("{name} \u{b7} {prefix} \u{b7} {}", self.runs_in())
    }

    /// What the footer strip says, if it has anything to say (§3.8.6).
    ///
    /// A refusal or a broken rule first, red and verbatim; then an armed delete's question; then
    /// the one question `esc` asks about unsaved work, amber. A draft that is merely dirty paints
    /// nothing: the footer's `Unsaved` says that much.
    #[must_use]
    pub(super) fn footer_strip(&self) -> Option<FooterStrip> {
        if let Some(message) = self.error.clone().or_else(|| self.validate()) {
            return Some(FooterStrip::Error(message));
        }
        if (self.pending_delete.is_some() || self.schedules.pending_delete.is_some())
            && let Some(question) = self.notice.clone()
        {
            return Some(FooterStrip::Warning(question));
        }
        if self.section == BoardSection::Schedules
            && self.schedules.discard_armed
            && self
                .schedules
                .form
                .as_ref()
                .is_some_and(ScheduleForm::dirty)
        {
            return Some(FooterStrip::Warning(UNSAVED_SCHEDULE.to_owned()));
        }
        (self.discard_armed && self.dirty()).then(|| FooterStrip::Warning(UNSAVED_DRAFT.to_owned()))
    }

    /// The pane's last caption while it reports what a list key did: `P`'s result, a saved or
    /// started schedule. An armed delete's question is the footer strip's instead.
    #[must_use]
    pub(super) fn pane_notice(&self) -> Option<&str> {
        if self.pending_delete.is_some() || self.schedules.pending_delete.is_some() {
            return None;
        }
        self.notice.as_deref()
    }

    /// Whether the footer says `Unsaved`: the board draft, or the open schedule form, differs
    /// from what it opened with.
    #[must_use]
    pub(super) fn unsaved(&self) -> bool {
        self.dirty()
            || (self.section == BoardSection::Schedules
                && self
                    .schedules
                    .form
                    .as_ref()
                    .is_some_and(ScheduleForm::dirty))
    }

    /// Whether the cursor row is one `\u{23ce}` opens an editor over.
    #[must_use]
    pub(super) fn focused_is_typed(&self) -> bool {
        match self.focused() {
            SettingRow::Name | SettingRow::Prefix | SettingRow::MaxLiveRuns => true,
            SettingRow::BackendSetting(index) => {
                self.rows.get(index).is_some_and(BackendRow::is_text)
            }
            SettingRow::ColumnField(field) => {
                field.is_text() && !(self.automation_locked && field.is_automation())
            }
            SettingRow::ScheduleField(field) => field.is_text(),
            _ => false,
        }
    }

    /// Opens the editor over the cursor row, remembering what `esc` reverts to.
    pub(super) fn open_editor(&mut self) {
        self.editing = true;
        self.editing_seed = self.focused_text();
        self.editing_seed_runs = self.max_live_runs;
        if self.editing_seed.is_none() {
            self.editing = false;
        }
    }

    /// The focused row's seed text, while that row's editor is open.
    ///
    /// No row materializes an editor on arrival: `\u{23ce}` (or a click on the row's box) opens
    /// it, which is what leaves the list keys and the arrows meaning themselves on every row.
    #[must_use]
    pub(super) fn focused_text(&self) -> Option<String> {
        if !self.editing {
            return None;
        }
        match self.focused() {
            SettingRow::Name => Some(self.name.clone()),
            SettingRow::Prefix => Some(self.prefix.clone()),
            SettingRow::MaxLiveRuns => Some(self.live_run_limit().to_string()),
            SettingRow::BackendSetting(index) => {
                let row = self.rows.get(index)?;
                if !row.is_text() {
                    return None;
                }
                Some(row.value.clone())
            }
            SettingRow::ColumnField(field) => {
                if self.automation_locked && field.is_automation() {
                    return None;
                }
                field_text(self.opened()?, field)
            }
            SettingRow::ScheduleField(field) => self.schedule_text(field),
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
            // Digits only reach here; an emptied box keeps the last number until one is typed,
            // and a number outside 1–8 is kept so the row can state the rule it breaks.
            SettingRow::MaxLiveRuns => {
                if let Ok(runs) = text.trim().parse::<u32>() {
                    self.max_live_runs = Some(runs);
                }
            }
            SettingRow::BackendSetting(index) => match self.rows.get_mut(index) {
                Some(row) => row.value = text.to_owned(),
                None => return,
            },
            SettingRow::ColumnField(field) => {
                let Some(index) = self.opened_column else {
                    return;
                };
                let locked = self.automation_locked;
                set_field_text(&mut self.columns, index, field, text, locked);
                self.prepare();
            }
            SettingRow::ScheduleField(field) => self.set_schedule_text(field, text),
            _ => return,
        }
        self.error = None;
        self.notice = None;
        self.discard_armed = false;
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

/// The amber question the first `esc` on a dirty board draft asks (§3.8.6).
pub(super) const UNSAVED_DRAFT: &str = "Unsaved changes. Press Esc again to discard them.";
/// The amber question the first `esc` (or a section change) on a dirty schedule form asks.
pub(super) const UNSAVED_SCHEDULE: &str = "Unsaved schedule. Press Esc again to discard it.";

/// What the footer strip above the buttons says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FooterStrip {
    /// A refusal or a broken rule, red, verbatim.
    Error(String),
    /// A question the dialog is waiting on an answer to, amber.
    Warning(String),
}
