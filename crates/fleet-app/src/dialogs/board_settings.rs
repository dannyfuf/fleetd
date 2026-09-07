//! Board settings (BOARD §8) — *the facts that change how the board behaves.*
//!
//! The rows follow §3.8.6's model exactly, which is why they answer the same keys: `j` / `k`
//! move, `space` toggles, `h` / `l` cycle a closed choice, and a text row simply types — a
//! bare `j` in the name field is the letter `j`, never a cursor move.
//!
//! # The backend rows are generic on purpose
//!
//! Nothing in this file knows what a Jira project key is. The **Backend** row cycles the kinds
//! the daemon registers (`ListBoardBackends`), and every row under it is one entry of that
//! descriptor's `settings_schema`: a `PropertyKind` decides whether the row is a text field, a
//! toggle, a number or a cycler, the schema's `key` is the JSON key written back into
//! `BackendRef.settings`, and the schema's `name` is what the row is called. A backend the
//! daemon adds tomorrow gets a complete settings form here with no change to `fleet-app` at
//! all — which is exactly the property `docs/BOARD-JIRA.md` §6 asks for.

use fleet_core::{
    board::{
        BackendRef, BoardPatch, BoardSettings, ConflictPolicy, PropertyKind, PropertyOption,
        PropertySchema,
    },
    ids::{BoardId, RepoId},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};

use crate::{
    actions::{dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{Dialogs, TextInput, notify, root, step, typed_char, with_host},
    state::AppState,
};

/// How wide the row labels are.
const LABEL_WIDTH: f32 = 150.0;
/// The longest identifier prefix the contract allows.
pub const MAX_PREFIX: usize = 8;
/// The suffix a schema name carries when the setting has no working default.
///
/// `PropertySchema` has no `required` flag, so this is the one signal a backend has to say
/// "this row must be filled in" — `docs/BOARD-JIRA.md` §5 names Jira's `project` row
/// `Project key (required)` for exactly that reason. The marker is stripped from the label the
/// row draws, because the row states the rule with its own `required` styling instead.
pub use fleet_core::board::REQUIRED_MARKER;
/// What a multi-select settings row is typed as, and split back on.
const MULTI_SEPARATOR: char = ',';

// ---------------------------------------------------------------------------- generic rows

/// One backend settings row: a schema entry plus the value being edited.
///
/// The value is always held as **text**, whatever the kind: that is what a caret, a partially
/// typed number and an empty-means-unset row all need. [`rows_to_settings`] is the one place
/// that turns it back into typed JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendRow {
    /// The JSON key inside `BackendRef.settings`.
    pub key: String,
    /// What the row is called, with [`REQUIRED_MARKER`] already stripped.
    pub name: String,
    /// Which control the row draws and which JSON type it writes.
    pub kind: PropertyKind,
    /// The closed set a `Select` row cycles.
    pub options: Vec<PropertyOption>,
    /// Whether an empty value stops the save.
    pub required: bool,
    /// The value, as typed.
    pub value: String,
    /// Whether the settings object carried this key when the dialog opened.
    ///
    /// Only a `Bool` needs it: `false` and "not set" are different things to a backend whose
    /// own default is `true`, and every other kind says "not set" by being empty.
    pub present: bool,
}

impl BackendRow {
    /// Whether typing lands in this row instead of moving the cursor.
    #[must_use]
    pub const fn is_text(&self) -> bool {
        !matches!(self.kind, PropertyKind::Bool | PropertyKind::Select)
    }

    /// Whether the row draws a checkbox.
    #[must_use]
    pub const fn is_flag(&self) -> bool {
        matches!(self.kind, PropertyKind::Bool)
    }

    /// The flag a `Bool` row holds.
    #[must_use]
    pub fn flag(&self) -> bool {
        self.value == "true"
    }

    /// Whether a character may be typed into this row.
    ///
    /// A number row takes digits only, which is what lets `h` and `l` keep their cycling
    /// meaning there: the letter is refused, so the key falls through to the cursor.
    #[must_use]
    pub fn accepts(&self, text: &str) -> bool {
        if !self.is_text() {
            return false;
        }
        self.kind != PropertyKind::Number
            || text.chars().all(|character| character.is_ascii_digit())
    }

    /// The failing rule of this row, in the wording the dialog shows it in.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        let value = self.value.trim();
        // Before the required rule, so a required number row spelled `abc` is told what is
        // wrong with it rather than that it is empty.
        if self.kind == PropertyKind::Number && !value.is_empty() && value.parse::<i64>().is_err() {
            return Some(format!("{} must be a whole number", self.name));
        }
        // Against what the row actually *writes*, never against its raw text: a required
        // multi-select spelled `","` trims to something non-empty and yet `json` returns
        // `None`, so `rows_to_settings` dropped the key and Save shipped a settings object
        // missing a key the backend declared it needs.
        if self.required && !self.is_flag() && self.json().is_none() {
            return Some(format!("{} is required", self.name));
        }
        None
    }

    /// The typed JSON this row writes, or `None` when the key is to be left out entirely.
    #[must_use]
    pub fn json(&self) -> Option<serde_json::Value> {
        let value = self.value.trim();
        match self.kind {
            // A flag the settings never carried and the user never turned on is not a setting
            // the board has an opinion about; writing `false` would override a backend default.
            PropertyKind::Bool => (self.present || self.flag()).then(|| self.flag().into()),
            PropertyKind::Number => value.parse::<i64>().ok().map(Into::into),
            PropertyKind::MultiSelect => {
                let values: Vec<serde_json::Value> = value
                    .split(MULTI_SEPARATOR)
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(Into::into)
                    .collect();
                (!values.is_empty()).then_some(serde_json::Value::Array(values))
            }
            _ => (!value.is_empty()).then(|| value.into()),
        }
    }
}

/// Whether a schema entry says it has no working default.
pub use fleet_core::board::is_required;

/// The row label of a schema entry: its name without the required marker.
pub use fleet_core::board::schema_row_name as row_name;

/// Turns a backend's settings schema and a settings object into editable rows.
///
/// Keys the schema does not mention are left alone — they are still in `settings` and
/// [`rows_to_settings`] carries them through, so a setting only `fleet board set` can write is
/// never silently dropped by opening this dialog.
#[must_use]
pub fn backend_rows(schema: &[PropertySchema], settings: &serde_json::Value) -> Vec<BackendRow> {
    let object = settings.as_object();
    schema
        .iter()
        .map(|entry| {
            let held = object.and_then(|map| map.get(&entry.key));
            BackendRow {
                key: entry.key.clone(),
                name: row_name(entry),
                kind: entry.kind,
                options: entry.options.clone(),
                required: is_required(entry),
                value: held.map_or_else(String::new, |value| text_of(value, entry.kind)),
                present: held.is_some_and(|value| !value.is_null()),
            }
        })
        .collect()
}

/// How a stored JSON value reads in its row.
fn text_of(value: &serde_json::Value, kind: PropertyKind) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Array(values) if kind == PropertyKind::MultiSelect => values
            .iter()
            .map(|entry| match entry {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        // A shape no row can draw — an object, or an array where a string was declared. It is
        // shown verbatim rather than blanked, because blanking it would be a silent deletion.
        other => other.to_string(),
    }
}

/// Merges the rows back into a settings object, keeping every key the schema does not name.
#[must_use]
pub fn rows_to_settings(base: &serde_json::Value, rows: &[BackendRow]) -> serde_json::Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    for row in rows {
        // A row the user never touched writes back exactly what it read. The `,` a
        // multi-select joins and splits on has no escape, so a stored column named
        // `Blocked, waiting` came back as two — a value this dialog cannot even express,
        // silently rewritten by a Save that was about some other row entirely.
        if let Some(held) = object.get(&row.key)
            && text_of(held, row.kind) == row.value
        {
            continue;
        }
        match row.json() {
            Some(value) => {
                object.insert(row.key.clone(), value);
            }
            // An emptied row removes the key instead of writing `""`: a backend deserializes
            // an absent optional key as `None`, and an empty string as a setting.
            None => {
                object.remove(&row.key);
            }
        }
    }
    serde_json::Value::Object(object)
}

/// The first row that cannot be saved, in row order.
#[must_use]
pub fn rows_error(rows: &[BackendRow]) -> Option<String> {
    rows.iter().find_map(BackendRow::error)
}

// ---------------------------------------------------------------------------- the dialog rows

/// The rows of the dialog, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingRow {
    /// Display name.
    Name,
    /// Identifier prefix.
    Prefix,
    /// Repository a card's worktree defaults to.
    DefaultRepo,
    /// Whether a worktree starts its card.
    StartOnWorktree,
    /// Whether a card made here is filed as a new remote issue.
    PushNewCards,
    /// How sync conflicts are resolved.
    ConflictPolicy,
    /// Which backend mirrors this board.
    Backend,
    /// One entry of the selected backend's settings schema.
    BackendSetting(usize),
}

/// The rows every board has, before the backend's own.
pub const FIXED_ROWS: [SettingRow; 7] = [
    SettingRow::Name,
    SettingRow::Prefix,
    SettingRow::DefaultRepo,
    SettingRow::StartOnWorktree,
    SettingRow::PushNewCards,
    SettingRow::ConflictPolicy,
    SettingRow::Backend,
];

impl SettingRow {
    /// The row's label; a backend row is named by its own schema.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Prefix => "Prefix",
            Self::DefaultRepo => "Default repository",
            Self::StartOnWorktree => "Start card on worktree",
            Self::PushNewCards => "Push new cards",
            Self::ConflictPolicy => "Conflict policy",
            Self::Backend => "Backend",
            Self::BackendSetting(_) => "",
        }
    }
}

/// The three policies, in cycle order.
pub const POLICIES: [ConflictPolicy; 3] = [
    ConflictPolicy::Manual,
    ConflictPolicy::RemoteWins,
    ConflictPolicy::LocalWins,
];

/// The word a policy reads as.
#[must_use]
pub const fn policy_label(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Manual => "manual",
        ConflictPolicy::RemoteWins => "remote wins",
        ConflictPolicy::LocalWins => "local wins",
    }
}

/// Editable board configuration, including its backend and that backend's own settings.
#[derive(Debug, Clone, Default)]
pub struct BoardSettingsState {
    /// Identity of this opening, used to ignore replies to discarded drafts.
    pub generation: u64,
    /// A request is in flight; retain and freeze the draft until it answers.
    pub saving: bool,
    /// Board being configured.
    pub board_id: Option<BoardId>,
    /// Display name.
    pub name: String,
    /// Card identifier prefix.
    pub prefix: String,
    /// Default repository for card worktrees.
    pub default_repo_id: Option<RepoId>,
    /// Move unstarted cards to Started when creating a worktree.
    pub start_on_worktree: bool,
    /// File a card made here as a new issue on the board's backend.
    ///
    /// It defaults to `false`, so a GUI-only session on a linked board used to create card
    /// after card that never became an issue, with nothing anywhere saying why. The row is
    /// drawn disabled on a local board, where there is no backend to push anything to.
    pub push_new_cards: bool,
    /// Conflict resolution policy.
    pub conflict_policy: ConflictPolicy,
    /// Backend registry key the draft currently selects.
    pub backend_kind: String,
    /// The kind the board was opened with, so returning to it restores its settings.
    pub original_kind: String,
    /// The settings the board was opened with.
    pub original_settings: serde_json::Value,
    /// The selected backend's schema rows, with their values.
    pub rows: Vec<BackendRow>,
    /// Which row carries the cursor.
    pub row: usize,
    /// Caret in the focused text row, as a character offset.
    pub caret: usize,
    /// Scroll position of the row list.
    ///
    /// A backend with nine settings makes this form taller than the card ever gets, and a
    /// dialog whose last rows are clipped away is a dialog whose Save the user cannot reach.
    pub scroll: gpui::ScrollHandle,
    /// The row the scroller was last asked to reveal.
    ///
    /// gpui's `FirstVisible` strategy re-anchors on the row it is given, so calling it on
    /// every render pinned the list to the focused row: the wheel scrolled and snapped
    /// straight back, and a nine-row backend form could not be read past the cursor.
    pub revealed: Option<usize>,
    /// Why the draft cannot be saved.
    pub error: Option<String>,
}

impl BoardSettingsState {
    /// Completes only the request belonging to this opening.
    pub fn finish_save(&mut self, generation: u64, error: Option<String>) -> bool {
        if self.generation != generation || !self.saving {
            return false;
        }
        self.saving = false;
        self.error = error;
        self.error.is_none()
    }

    /// Every row of this draft, in the order they are drawn.
    #[must_use]
    pub fn rows(&self) -> Vec<SettingRow> {
        let mut rows = FIXED_ROWS.to_vec();
        rows.extend((0..self.rows.len()).map(SettingRow::BackendSetting));
        rows
    }

    /// The row the cursor is on.
    #[must_use]
    pub fn focused(&self) -> SettingRow {
        self.rows()
            .get(self.row)
            .copied()
            .unwrap_or(SettingRow::Name)
    }

    /// The backend row under the cursor, when one is.
    #[must_use]
    pub fn focused_backend_row(&self) -> Option<&BackendRow> {
        match self.focused() {
            SettingRow::BackendSetting(index) => self.rows.get(index),
            _ => None,
        }
    }

    /// Whether the draft still points at the kind the board is stored with.
    #[must_use]
    pub fn keeps_kind(&self) -> bool {
        self.backend_kind == self.original_kind
    }

    /// Points the draft at `kind`, rebuilding its rows from that backend's schema.
    ///
    /// Returning to the board's own kind restores the settings it was opened with; every other
    /// kind starts from nothing, which is the same rule the daemon applies (`BOARD-JIRA` §4:
    /// on a kind change the settings come from the patch and are never carried over).
    pub fn select_backend(&mut self, kind: &str, schema: &[PropertySchema]) {
        self.backend_kind = kind.to_owned();
        let base = if self.keeps_kind() {
            self.original_settings.clone()
        } else {
            serde_json::Value::Null
        };
        self.rows = backend_rows(schema, &base);
        self.row = self.row.min(self.rows().len().saturating_sub(1));
        self.caret = 0;
        self.error = None;
    }

    /// The settings object this draft would send.
    ///
    /// A backend that configures nothing sends `null`, not `{}`: that is what a board with no
    /// settings stores, and a patch that swapped one for the other would differ from the stored
    /// backend on every save — writing the document, resetting the cursor and announcing a
    /// change nobody made.
    #[must_use]
    pub fn settings_json(&self) -> serde_json::Value {
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
    pub fn validate(&self) -> Option<String> {
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

    /// The text buffer of the focused row, when it has one.
    fn input(&self) -> Option<TextInput> {
        let value = match self.focused() {
            SettingRow::Name => self.name.clone(),
            SettingRow::Prefix => self.prefix.clone(),
            SettingRow::BackendSetting(index) => {
                let row = self.rows.get(index)?;
                if !row.is_text() {
                    return None;
                }
                row.value.clone()
            }
            _ => return None,
        };
        let mut input = TextInput::new(value);
        for _ in self.caret..input.caret() {
            input.left();
        }
        Some(input)
    }

    fn set_input(&mut self, input: &TextInput) {
        if self.saving {
            return;
        }
        match self.focused() {
            SettingRow::Name => self.name = input.value().to_owned(),
            // The contract stores prefixes uppercase, so the field shows what it will store.
            SettingRow::Prefix => self.prefix = input.value().to_uppercase(),
            SettingRow::BackendSetting(index) => match self.rows.get_mut(index) {
                Some(row) => row.value = input.value().to_owned(),
                None => return,
            },
            _ => return,
        }
        self.caret = input.caret();
        self.error = None;
    }

    /// The character count of the focused row's text, for a caret parked at its end.
    fn text_len(&self) -> usize {
        match self.focused() {
            SettingRow::Name => self.name.chars().count(),
            SettingRow::Prefix => self.prefix.chars().count(),
            SettingRow::BackendSetting(index) => self
                .rows
                .get(index)
                .filter(|row| row.is_text())
                .map_or(0, |row| row.value.chars().count()),
            _ => 0,
        }
    }
}

/// The repositories a board may default to: the ones in its own context.
#[must_use]
pub fn repo_choices(state: &AppState) -> Vec<RepoId> {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return Vec::new();
    };
    let context = state.board().map(|view| view.board.context_id.clone());
    snapshot
        .repos
        .iter()
        .filter(|repo| context.as_ref().is_none_or(|id| &repo.context_id == id))
        .map(|repo| repo.id.clone())
        .collect()
}

/// The backend kinds the cycler offers: the registry's, plus the board's own if it is unknown.
///
/// The board's kind is always in the list even when the daemon does not register it, because a
/// cycler that cannot show the value it is holding is a control that lies about the board.
#[must_use]
pub fn backend_kinds(state: &AppState, current: &str) -> Vec<String> {
    let mut kinds: Vec<String> = state
        .board_backends
        .iter()
        .map(|descriptor| descriptor.kind.clone())
        .collect();
    if !kinds.iter().any(|kind| kind == current) {
        kinds.insert(0, current.to_owned());
    }
    kinds
}

/// The settings schema of one backend kind; empty when the registry has not answered.
#[must_use]
pub fn schema_for(state: &AppState, kind: &str) -> Vec<PropertySchema> {
    state
        .backend_descriptor(kind)
        .map(|descriptor| descriptor.settings_schema.clone())
        .unwrap_or_default()
}

pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let app = state.read(cx);
    let draft = app
        .board()
        .map(|view| {
            let kind = view.board.backend.kind.clone();
            let settings = view.board.backend.settings.clone();
            let mut draft = BoardSettingsState {
                board_id: Some(view.board.id.clone()),
                name: view.board.name.clone(),
                prefix: view.board.prefix.clone(),
                default_repo_id: view.board.default_repo_id.clone(),
                start_on_worktree: view.board.settings.start_on_worktree,
                push_new_cards: view.board.settings.push_new_cards,
                conflict_policy: view.board.settings.conflict_policy,
                backend_kind: kind.clone(),
                original_kind: kind.clone(),
                original_settings: settings,
                ..BoardSettingsState::default()
            };
            draft.rows = backend_rows(&schema_for(app, &kind), &draft.original_settings);
            draft
        })
        .unwrap_or_default();
    with_host(cx, |host| {
        let generation = host.board_settings.generation.wrapping_add(1);
        host.board_settings = BoardSettingsState {
            generation,
            ..draft
        };
    });
}

/// Renders the board's own rows, the backend cycler, and one row per backend setting.
pub fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.sm;
    adopt_late_schema(state, cx);
    let draft = with_host(cx, |host| host.board_settings.clone());
    let repos = repo_choices(state.read(cx));
    let repo_index = repo_position(&repos, draft.default_repo_id.as_ref());
    let repo_off_grid = !repo_listed(&repos, draft.default_repo_id.as_ref());
    let policy_index = POLICIES
        .iter()
        .position(|policy| *policy == draft.conflict_policy)
        .unwrap_or(0);
    let kinds = backend_kinds(state.read(cx), &draft.backend_kind);
    let kind_index = kinds
        .iter()
        .position(|kind| *kind == draft.backend_kind)
        .unwrap_or(0);
    let backend_label = state.read(cx).backend_label(&draft.backend_kind);
    let focused = draft.focused();

    // The focused row is brought into view when it *changes*: `j` past the fold has to land
    // somewhere the user can see, and the card tops out at 90 % of the window however many
    // rows there are. Only when it changes, for the reason the board screen gates the same
    // call: an unconditional reveal re-anchors the list on every render and takes the wheel
    // away from the user.
    if let Some(row) = with_host(cx, |host| {
        (host.board_settings.revealed != Some(host.board_settings.row)).then(|| {
            host.board_settings.revealed = Some(host.board_settings.row);
            host.board_settings.row
        })
    }) {
        draft.scroll.scroll_to_item(row);
    }
    let rows = div()
        .id("board-settings-rows")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap(gap)
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .child(
            TextField::new(draft.name.clone())
                .label(SettingRow::Name.label())
                .placeholder("Fleet")
                .caret(if focused == SettingRow::Name {
                    draft.caret
                } else {
                    draft.name.chars().count()
                })
                .focused(focused == SettingRow::Name),
        )
        .child(
            TextField::new(draft.prefix.clone())
                .label(SettingRow::Prefix.label())
                .placeholder("FLT")
                .mono(true)
                .caret(if focused == SettingRow::Prefix {
                    draft.caret
                } else {
                    draft.prefix.chars().count()
                })
                .focused(focused == SettingRow::Prefix),
        )
        .child(
            Cycler::labeled(
                SettingRow::DefaultRepo.label(),
                draft
                    .default_repo_id
                    .as_ref()
                    .map_or_else(|| "none".to_owned(), |repo| repo.as_str().to_owned()),
            )
            .label_width(px(LABEL_WIDTH))
            .has_prev(repo_off_grid || repo_index > 0)
            .has_next(repo_off_grid || repo_index < repos.len())
            // A stored repository this context no longer lists sits on no position of the
            // cycler: drawn as if it were "none" the row said `h` would do nothing and `l`
            // would move one step, while the value on screen was the repository id itself.
            .off_grid(repo_off_grid)
            .focused(focused == SettingRow::DefaultRepo),
        )
        .child(
            Toggle::labeled(SettingRow::StartOnWorktree.label(), draft.start_on_worktree)
                .label_width(px(LABEL_WIDTH))
                .detail("moves a backlog card to the first started column")
                .focused(focused == SettingRow::StartOnWorktree),
        )
        .child(
            Toggle::labeled(SettingRow::PushNewCards.label(), draft.push_new_cards)
                .label_width(px(LABEL_WIDTH))
                .detail("files a card made here as a new issue on the backend")
                .disabled(draft.backend_kind == BackendRef::LOCAL)
                .focused(focused == SettingRow::PushNewCards),
        )
        .child(
            Cycler::labeled(
                SettingRow::ConflictPolicy.label(),
                policy_label(draft.conflict_policy),
            )
            .label_width(px(LABEL_WIDTH))
            .has_prev(policy_index > 0)
            .has_next(policy_index + 1 < POLICIES.len())
            .disabled(draft.backend_kind == BackendRef::LOCAL)
            .focused(focused == SettingRow::ConflictPolicy),
        )
        .child(
            Cycler::labeled(SettingRow::Backend.label(), backend_label)
                .label_width(px(LABEL_WIDTH))
                .has_prev(kind_index > 0)
                .has_next(kind_index + 1 < kinds.len())
                .focused(focused == SettingRow::Backend),
        )
        .children(
            draft
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| backend_element(row, &draft, index)),
        );

    let mut card = Dialog::new(Dialogs::BoardSettings.title())
        .icon(Dialogs::BoardSettings.icon())
        .width(Dialogs::BoardSettings.width())
        .body(rows)
        .hint_row(
            KeyHintRow::new()
                .key("j/k", "row")
                .key("space", "toggle")
                .key("h/l", "cycle")
                .key("esc", "cancel"),
        )
        .primary("\u{23ce} Save");
    if let Some(message) = draft.error.clone().or_else(|| draft.validate()) {
        card = card.error(message);
    }

    let save_state = state.clone();
    let save_bridge = bridge.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let Some(text) = typed_char(event) else {
                    return;
                };
                if insert(&state, &text, cx) {
                    cx.stop_propagation();
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
        // KEYMAP §Board says the board dialog rows are "in addition to everything the generic
        // Dialog context binds", and `tab` is one of them: without these two it was a dead key
        // here while every other multi-row dialog moved on it.
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| move_row(&state, 1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| move_row(&state, -1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| move_row(&state, -1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::CycleNext, _window, cx| cycle(&state, 1, "l", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::CyclePrev, _window, cx| cycle(&state, -1, "h", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                if !caret(&state, cx, TextInput::right) {
                    cycle(&state, 1, "", cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                if !caret(&state, cx, TextInput::left) {
                    cycle(&state, -1, "", cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::Toggle, _window, cx| toggle(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit(&state, cx, |input| {
                    input.backspace();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit(&state, cx, |input| {
                    input.delete_word();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                edit(&state, cx, |input| {
                    input.clear();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                caret(&state, cx, TextInput::home);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                caret(&state, cx, TextInput::end);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            save(&save_state, &save_bridge, cx);
            cx.stop_propagation();
        })
        .child(card)
        .into_any_element()
}

/// Fills in the backend rows when the registry answers after the dialog was seeded.
///
/// `,` can be pressed on the first frame after a reconnect, before `ListBoardBackends` has
/// come back; without this the dialog would show a `Backend` row and no settings under it for
/// as long as it stayed open. Rows are only ever built when there are none, so nothing the
/// user has typed can be overwritten — and the cursor stays exactly where it was.
fn adopt_late_schema(state: &Entity<AppState>, cx: &mut App) {
    let (kind, empty) = with_host(cx, |host| {
        (
            host.board_settings.backend_kind.clone(),
            host.board_settings.rows.is_empty(),
        )
    });
    if !empty {
        return;
    }
    let schema = schema_for(state.read(cx), &kind);
    if schema.is_empty() {
        return;
    }
    with_host(cx, |host| {
        let (row, caret) = (host.board_settings.row, host.board_settings.caret);
        host.board_settings.select_backend(&kind, &schema);
        host.board_settings.row = row;
        host.board_settings.caret = caret;
    });
}

/// Draws one backend settings row as the control its `PropertyKind` names.
fn backend_element(row: &BackendRow, draft: &BoardSettingsState, index: usize) -> AnyElement {
    let focused = draft.focused() == SettingRow::BackendSetting(index);
    let label = if row.required {
        format!("{} \u{2217}", row.name)
    } else {
        row.name.clone()
    };
    match row.kind {
        PropertyKind::Bool => Toggle::labeled(label, row.flag())
            .label_width(px(LABEL_WIDTH))
            .focused(focused)
            .into_any_element(),
        PropertyKind::Select => {
            let position = row
                .options
                .iter()
                .position(|option| option.value == row.value);
            let shown =
                position.map_or_else(|| row.value.clone(), |at| row.options[at].label.clone());
            Cycler::labeled(
                label,
                if shown.is_empty() {
                    "none".to_owned()
                } else {
                    shown
                },
            )
            .label_width(px(LABEL_WIDTH))
            .has_prev(position.is_some_and(|at| at > 0) || position.is_none())
            .has_next(position.is_none_or(|at| at + 1 < row.options.len()))
            .off_grid(position.is_none() && !row.value.is_empty())
            .focused(focused)
            .into_any_element()
        }
        // An unset optional number is drawn as an empty field with its placeholder, never as
        // `0`: every backend number row here has a non-zero default, and a dialog that shows
        // `0` states a value the daemon is not using.
        // …and only while it is not the row being edited: `BackendRow::is_text` is true for a
        // number, so `left`/`right`/`ctrl-a`/`ctrl-e` move a caret `NumberField` does not draw.
        // On "10" that turned a `left` and a typed `5` into "150" with nothing on screen to
        // explain it. The focused row is drawn as the text field it is actually edited as.
        PropertyKind::Number if !row.value.trim().is_empty() && !focused => {
            let mut field = NumberField::labeled(label, row.value.trim().parse().unwrap_or(0))
                .min(0)
                .focused(false);
            if let Some(message) = row.error() {
                field = field.invalid(message);
            }
            field.into_any_element()
        }
        _ => {
            let mut field = TextField::new(row.value.clone())
                .label(label)
                .placeholder(placeholder(row))
                .mono(row.kind == PropertyKind::Number)
                .caret(if focused {
                    draft.caret
                } else {
                    row.value.chars().count()
                })
                .focused(focused);
            if let Some(message) = row.error() {
                field = field.invalid(message);
            }
            field.into_any_element()
        }
    }
}

/// What an empty backend text row suggests.
fn placeholder(row: &BackendRow) -> &'static str {
    match row.kind {
        PropertyKind::MultiSelect => "comma, separated, values",
        PropertyKind::Date => "YYYY-MM-DD",
        _ if row.required => "required",
        _ => "optional",
    }
}

/// Where a repository sits in the cycler: `0` is `none`, the rest follow the list.
fn repo_position(repos: &[RepoId], current: Option<&RepoId>) -> usize {
    current
        .and_then(|repo| repos.iter().position(|entry| entry == repo))
        .map_or(0, |index| index + 1)
}

/// Whether the cycler has a position for this value at all.
///
/// A board can hold a `defaultRepoId` this context does not list — the repository moved, or the
/// snapshot has not arrived yet — and that value belongs to no step of the cycle.
fn repo_listed(repos: &[RepoId], current: Option<&RepoId>) -> bool {
    current.is_none_or(|repo| repos.iter().any(|entry| entry == repo))
}

/// Types `text` into the focused row. Returns whether it landed anywhere.
fn insert(state: &Entity<AppState>, text: &str, cx: &mut App) -> bool {
    let typed = with_host(cx, |host| {
        // A number row refuses a letter, which is what leaves `h` and `l` their cycling
        // meaning there instead of typing an `h` no backend can parse.
        if host
            .board_settings
            .focused_backend_row()
            .is_some_and(|row| !row.accepts(text))
        {
            return false;
        }
        let Some(mut input) = host.board_settings.input() else {
            return false;
        };
        input.insert(text);
        host.board_settings.set_input(&input);
        true
    });
    if typed {
        notify(state, cx);
    }
    typed
}

/// `j` / `k`: move the cursor — or type the letter, when a text row owns the keyboard.
fn move_row(state: &Entity<AppState>, delta: isize, literal: &str, cx: &mut App) {
    if !literal.is_empty() && insert(state, literal, cx) {
        cx.stop_propagation();
        return;
    }
    with_host(cx, |host| {
        let len = host.board_settings.rows().len();
        host.board_settings.row = step(host.board_settings.row, delta, len);
        host.board_settings.caret = host.board_settings.text_len();
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// `h` / `l`: cycle a closed choice — or type the letter, for the same reason.
fn cycle(state: &Entity<AppState>, delta: isize, literal: &str, cx: &mut App) {
    if !literal.is_empty() && insert(state, literal, cx) {
        cx.stop_propagation();
        return;
    }
    let app = state.read(cx);
    let repos = repo_choices(app);
    let selected = with_host(cx, |host| host.board_settings.backend_kind.clone());
    let kinds = backend_kinds(state.read(cx), &selected);
    let next_kind = kinds
        .iter()
        .position(|kind| *kind == selected)
        .map(|index| kinds[step(index, delta, kinds.len())].clone())
        .unwrap_or(selected);
    let schema = schema_for(state.read(cx), &next_kind);
    with_host(cx, |host| {
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
fn cycle_backend_row(draft: &mut BoardSettingsState, index: usize, delta: isize) {
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
fn toggle(state: &Entity<AppState>, cx: &mut App) {
    let toggled = with_host(cx, |host| {
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
    if !toggled {
        insert(state, " ", cx);
    }
    notify(state, cx);
    cx.stop_propagation();
}

/// Moves the caret of the focused text row. Returns whether there was one.
fn caret(state: &Entity<AppState>, cx: &mut App, move_to: fn(&mut TextInput)) -> bool {
    let moved = with_host(cx, |host| {
        let Some(mut input) = host.board_settings.input() else {
            return false;
        };
        move_to(&mut input);
        host.board_settings.caret = input.caret();
        true
    });
    if moved {
        notify(state, cx);
    }
    moved
}

/// Runs a text edit against the focused row.
fn edit(state: &Entity<AppState>, cx: &mut App, edit: impl FnOnce(&mut TextInput)) {
    let edited = with_host(cx, |host| {
        let Some(mut input) = host.board_settings.input() else {
            return false;
        };
        edit(&mut input);
        host.board_settings.set_input(&input);
        true
    });
    if edited {
        notify(state, cx);
        cx.stop_propagation();
    }
}

/// `Enter`: send one `UpdateBoard` with everything the dialog changed.
fn save(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let draft = with_host(cx, |host| host.board_settings.clone());
    // A save already in flight is answered by the reply it is waiting for; a board that changed
    // under the dialog is not answered by anything, and returning silently makes `Enter` a dead
    // key with nothing on the error line to read. `card_picker::apply` says so for the same case.
    if draft.saving {
        return;
    }
    if !state
        .read(cx)
        .board()
        .is_some_and(|view| Some(&view.board.id) == draft.board_id.as_ref())
    {
        with_host(cx, |host| {
            host.board_settings.error = Some("That board is no longer loaded".into());
        });
        notify(state, cx);
        return;
    }
    if let Some(message) = draft.validate() {
        with_host(cx, |host| host.board_settings.error = Some(message));
        notify(state, cx);
        return;
    }
    let Some(board_id) = draft.board_id.clone() else {
        return;
    };
    // The one setting this dialog does not show — the branch template — is carried through
    // unchanged, because `BoardPatch::settings` replaces the whole struct.
    let settings = state
        .read(cx)
        .board()
        .map_or_else(BoardSettings::default, |view| view.board.settings.clone());
    let patch = BoardPatch {
        name: Some(draft.name.trim().to_owned()),
        prefix: Some(draft.prefix.trim().to_owned()),
        default_repo_id: Some(draft.default_repo_id.clone()),
        settings: Some(BoardSettings {
            start_on_worktree: draft.start_on_worktree,
            push_new_cards: draft.push_new_cards,
            conflict_policy: draft.conflict_policy,
            ..settings
        }),
        // A backend the dialog did not change is left out of the patch entirely: `update`
        // resets the sync cursor for any backend it is handed, and the board's own settings
        // rows must not cost a re-pull.
        backend: Some(BackendRef {
            kind: draft.backend_kind.clone(),
            settings: draft.settings_json(),
        })
        .filter(|backend| {
            state
                .read(cx)
                .board()
                .is_none_or(|view| *backend != view.board.backend)
        }),
        ..BoardPatch::default()
    };

    with_host(cx, |host| {
        host.board_settings.saving = true;
        host.board_settings.error = None;
    });
    let generation = draft.generation;
    let reply = bridge.request(RequestBody::UpdateBoard {
        board_id: board_id.clone(),
        patch,
    });
    notify(state, cx);
    let handle = state.clone();
    cx.spawn(async move |cx| {
        let answer = match reply.recv().await {
            Ok(Ok(ResponseBody::Board(view))) => Ok(view),
            // The daemon's own words, verbatim: refusing a kind change on a linked board, or a
            // setting the backend cannot parse, is a sentence only the backend can write.
            Ok(Err(error)) => Err(error.message),
            Ok(Ok(_)) => Err("Unexpected settings response".into()),
            Err(_) => Err("Daemon disconnected before replying".into()),
        };
        cx.update(|cx| {
            if handle.read(cx).overlay
                != Some(crate::state::Overlay::Dialog(Dialogs::BoardSettings))
                || !handle
                    .read(cx)
                    .board()
                    .is_some_and(|view| view.board.id == board_id)
            {
                return;
            }
            let completed = with_host(cx, |host| {
                host.board_settings
                    .finish_save(generation, answer.as_ref().err().cloned())
            });
            if completed && let Ok(view) = answer {
                handle.update(cx, |app, cx| {
                    app.apply_board_view(view);
                    app.close_overlay();
                    cx.notify();
                });
            }
            notify(&handle, cx);
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::board::PropertySource;

    fn draft() -> BoardSettingsState {
        BoardSettingsState {
            board_id: Some("work".parse().unwrap_or_else(|error| panic!("{error}"))),
            name: "Fleet".to_owned(),
            prefix: "FLT".to_owned(),
            backend_kind: "local".to_owned(),
            original_kind: "local".to_owned(),
            ..BoardSettingsState::default()
        }
    }

    fn entry(key: &str, name: &str, kind: PropertyKind) -> PropertySchema {
        PropertySchema {
            key: key.to_owned(),
            name: name.to_owned(),
            kind,
            options: Vec::new(),
            editable: true,
            source: PropertySource::Backend,
            show_on_card: false,
        }
    }

    /// A schema shaped like a real backend's, without naming one: every kind the dialog draws.
    fn schema() -> Vec<PropertySchema> {
        vec![
            entry("project", "Project key (required)", PropertyKind::Text),
            entry("jql", "Extra filter", PropertyKind::Text),
            entry("statuses", "Columns", PropertyKind::MultiSelect),
            entry("overlapMinutes", "Overlap", PropertyKind::Number),
            entry("archived", "Include archived", PropertyKind::Bool),
        ]
    }

    #[test]
    fn a_schema_becomes_one_row_per_key_with_the_stored_value_in_it() {
        let settings = serde_json::json!({
            "project": "SP",
            "statuses": ["To Do", "Done"],
            "overlapMinutes": 5,
            "extraFields": [{"id": "customfield_1"}]
        });
        let rows = backend_rows(&schema(), &settings);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].value, "SP");
        assert!(rows[0].required, "the marker is the only `required` signal");
        assert_eq!(
            rows[0].name, "Project key",
            "the marker is not part of the label"
        );
        assert_eq!(rows[1].value, "", "an absent optional key reads as empty");
        assert!(!rows[1].required);
        assert_eq!(rows[2].value, "To Do, Done");
        assert_eq!(rows[3].value, "5");
        assert!(!rows[4].present, "an absent flag is not `false`");
    }

    #[test]
    fn rows_write_typed_json_and_keep_the_keys_the_schema_never_names() {
        let settings = serde_json::json!({
            "project": "SP",
            "overlapMinutes": 5,
            "extraFields": [{"id": "customfield_1"}]
        });
        let mut rows = backend_rows(&schema(), &settings);
        rows[0].value = "  PAY  ".into();
        rows[2].value = "To Do , , Done ".into();
        rows[3].value = "8".into();
        let written = rows_to_settings(&settings, &rows);
        assert_eq!(
            written,
            serde_json::json!({
                "project": "PAY",
                "statuses": ["To Do", "Done"],
                // A number is written as a number: a backend deserializing `usize` refuses
                // `"8"` and refuses `8.0` just as hard.
                "overlapMinutes": 8,
                // A setting only the CLI can write survives a pass through this dialog.
                "extraFields": [{"id": "customfield_1"}]
            })
        );
    }

    #[test]
    fn an_emptied_row_removes_its_key_rather_than_writing_an_empty_string() {
        let settings = serde_json::json!({"project": "SP", "jql": "sprint in openSprints()"});
        let mut rows = backend_rows(&schema(), &settings);
        assert_eq!(rows[1].value, "sprint in openSprints()");
        rows[1].value = "   ".into();
        let written = rows_to_settings(&settings, &rows);
        assert_eq!(written, serde_json::json!({"project": "SP"}));
        // An absent optional key is `None` to the backend; `""` is a setting it has to honour.
        assert!(written.get("jql").is_none());
    }

    #[test]
    fn a_flag_is_written_only_once_it_means_something() {
        let mut rows = backend_rows(&schema(), &serde_json::Value::Null);
        assert_eq!(
            rows[4].json(),
            None,
            "a flag nobody touched overrides no default"
        );
        rows[4].value = "true".into();
        rows[4].present = true;
        assert_eq!(rows[4].json(), Some(serde_json::json!(true)));
        rows[4].value = "false".into();
        assert_eq!(
            rows[4].json(),
            Some(serde_json::json!(false)),
            "a flag turned off is an opinion, not an absence"
        );
    }

    #[test]
    fn an_empty_required_row_and_a_non_numeric_one_stop_the_save() {
        let mut rows = backend_rows(&schema(), &serde_json::Value::Null);
        assert_eq!(
            rows_error(&rows).as_deref(),
            Some("Project key is required"),
            "the message names the row, not the JSON key"
        );
        rows[0].value = "SP".into();
        assert_eq!(rows_error(&rows), None);
        rows[3].value = "many".into();
        assert_eq!(
            rows_error(&rows).as_deref(),
            Some("Overlap must be a whole number")
        );
        // A number row refuses the letter, which is what leaves `h`/`l` their cycling meaning.
        assert!(!rows[3].accepts("h"));
        assert!(rows[3].accepts("7"));
        assert!(rows[1].accepts("h"));
        assert!(!rows[4].accepts("h"), "a flag is not typed into");
    }

    /// `h` is off and `l` is on: a row that flipped on both made `h l` invert the setting.
    #[test]
    fn hl_on_a_flag_row_sets_a_side_rather_than_flipping() {
        let mut state = draft();
        state.rows = backend_rows(&schema(), &serde_json::Value::Null);
        let flag = state.rows.len() - 1;
        assert_eq!(state.rows[flag].kind, PropertyKind::Bool);
        cycle_backend_row(&mut state, flag, 1);
        assert!(state.rows[flag].flag());
        cycle_backend_row(&mut state, flag, 1);
        assert!(state.rows[flag].flag(), "`l` twice is still on");
        cycle_backend_row(&mut state, flag, -1);
        assert!(!state.rows[flag].flag());
        cycle_backend_row(&mut state, flag, -1);
        assert!(!state.rows[flag].flag(), "`h` twice is still off");
        assert!(
            state.rows[flag].present,
            "a flag the user touched is a setting the board has an opinion about"
        );
    }

    /// `pushNewCards` used to be reachable from no app surface at all, and it defaults to off:
    /// every `c` on a linked board made a card that never became an issue, silently.
    #[test]
    fn the_push_new_cards_row_is_drawn_and_carried_into_the_saved_settings() {
        assert!(FIXED_ROWS.contains(&SettingRow::PushNewCards));
        assert_eq!(SettingRow::PushNewCards.label(), "Push new cards");
        let mut state = draft();
        state.row = FIXED_ROWS
            .iter()
            .position(|row| *row == SettingRow::PushNewCards)
            .unwrap_or_else(|| panic!("the row is drawn"));
        assert_eq!(state.focused(), SettingRow::PushNewCards);
        assert!(!state.push_new_cards, "the board default is off");
        // It sits with the other board-level flag, ahead of the backend's own rows.
        assert_eq!(state.row, 4);
        assert!(state.focused_backend_row().is_none());
    }

    /// A row the user never touched writes back exactly what it read. The `,` a multi-select
    /// joins and splits on has no escape, so a stored column named `Blocked, waiting` came back
    /// as two — silently rewritten by a Save that was about some other row entirely.
    #[test]
    fn an_untouched_row_is_written_back_unchanged_however_it_is_spelled() {
        let settings = serde_json::json!({
            "project": "SP",
            "statuses": ["To Do", "Blocked, waiting"],
        });
        let rows = backend_rows(&schema(), &settings);
        assert_eq!(rows_to_settings(&settings, &rows), settings);
        // A row the user *did* touch is written as the rows spell it.
        let mut edited = rows.clone();
        edited[2].value = "To Do, Done".to_owned();
        assert_eq!(
            rows_to_settings(&settings, &edited)["statuses"],
            serde_json::json!(["To Do", "Done"])
        );
        // A shape no row can draw is preserved rather than flattened into its own debug text.
        let odd = serde_json::json!({"project": "SP", "jql": {"saved": 42}});
        assert_eq!(rows_to_settings(&odd, &backend_rows(&schema(), &odd)), odd);
    }

    /// A stored repository this context no longer lists sits on no position of the cycler.
    /// Drawn as if it were "none", the row said `h` would do nothing and `l` would move one
    /// step, while the value on screen was the repository id itself — and `h` wrapped to the
    /// *last* repository rather than clearing it.
    #[test]
    fn a_repository_the_context_no_longer_lists_is_off_the_cycler_s_grid() {
        let repos: Vec<RepoId> = vec!["acme/api".parse().unwrap(), "acme/web".parse().unwrap()];
        let gone: RepoId = "acme/gone".parse().unwrap();
        assert!(repo_listed(&repos, None));
        assert!(repo_listed(&repos, Some(&repos[1])));
        assert!(!repo_listed(&repos, Some(&gone)));
        // On the grid, the positions are "none" then the list in order.
        assert_eq!(repo_position(&repos, None), 0);
        assert_eq!(repo_position(&repos, Some(&repos[1])), 2);
    }

    /// The required rule reads what the row *writes*: separators alone write nothing.
    #[test]
    fn a_required_multi_select_of_separators_alone_is_refused_rather_than_dropped() {
        let mut schema = schema();
        schema[2].name = "Columns (required)".to_owned();
        let mut rows = backend_rows(&schema, &serde_json::json!({"project": "SP"}));
        rows[2].value = format!("{MULTI_SEPARATOR} {MULTI_SEPARATOR}");
        assert_eq!(rows[2].json(), None, "the row writes no value");
        assert_eq!(
            rows_error(&rows).as_deref(),
            Some("Columns is required"),
            "a save that shipped settings missing a required key would have gone out"
        );
        rows[2].value = format!("To Do{MULTI_SEPARATOR}Done");
        assert_eq!(rows_error(&rows), None);
    }

    #[test]
    fn changing_the_kind_starts_from_empty_settings_and_coming_back_restores_them() {
        let mut state = draft();
        state.original_settings = serde_json::json!({"project": "SP"});
        state.rows = backend_rows(&schema(), &state.original_settings);
        state.select_backend("other", &schema());
        assert!(!state.keeps_kind());
        assert_eq!(
            state.rows[0].value, "",
            "settings never cross a kind change"
        );
        // Nothing configured is `null`, which is what an unconfigured board stores: an empty
        // object would differ from it and make every save a write.
        assert_eq!(state.settings_json(), serde_json::Value::Null);
        state.select_backend("local", &schema());
        assert!(state.keeps_kind());
        assert_eq!(state.rows[0].value, "SP");
        assert_eq!(state.settings_json(), serde_json::json!({"project": "SP"}));
    }

    #[test]
    fn the_backend_rows_follow_the_fixed_ones() {
        let mut state = draft();
        assert_eq!(state.rows().len(), FIXED_ROWS.len());
        state.select_backend("other", &schema());
        assert_eq!(state.rows().len(), FIXED_ROWS.len() + 5);
        state.row = FIXED_ROWS.len();
        assert_eq!(state.focused(), SettingRow::BackendSetting(0));
        assert_eq!(
            state.focused_backend_row().map(|row| row.key.as_str()),
            Some("project")
        );
        // A shorter schema cannot leave the cursor pointing past the end.
        state.select_backend("smaller", &[entry("a", "A", PropertyKind::Text)]);
        assert!(state.row < state.rows().len());
    }

    #[test]
    fn only_the_typing_rows_take_a_caret() {
        let mut state = draft();
        state.select_backend("other", &schema());
        assert!(state.input().is_some(), "name");
        state.row = 2;
        assert!(state.input().is_none(), "the repo cycler");
        state.row = FIXED_ROWS.len();
        assert!(state.input().is_some(), "a text setting");
        state.row = FIXED_ROWS.len() + 4;
        assert!(state.input().is_none(), "a flag");
        assert!(state.rows[3].is_text(), "a number is typed into");
        assert!(!state.rows[4].is_text());
    }

    #[test]
    fn a_backend_row_stores_what_is_typed_into_it() {
        let mut state = draft();
        state.select_backend("other", &schema());
        state.row = FIXED_ROWS.len();
        let mut input = state.input().unwrap_or_else(|| panic!("a text row"));
        input.insert("SP");
        state.set_input(&input);
        assert_eq!(state.rows[0].value, "SP");
        assert_eq!(state.settings_json(), serde_json::json!({"project": "SP"}));
    }

    #[test]
    fn a_schema_that_arrives_late_fills_the_rows_without_moving_the_cursor() {
        // `,` on the first frame after a reconnect seeds the dialog before the registry has
        // answered. What arrives late has to land, or the dialog shows a backend with no
        // settings for as long as it stays open.
        let mut state = draft();
        state.original_settings = serde_json::json!({"project": "SP"});
        state.rows = backend_rows(&[], &state.original_settings);
        assert!(state.rows.is_empty());
        state.row = 1;
        state.caret = 2;
        let (row, caret) = (state.row, state.caret);
        state.select_backend(&state.backend_kind.clone(), &schema());
        state.row = row;
        state.caret = caret;
        assert_eq!(
            state.rows[0].value, "SP",
            "the board's own settings, not blanks"
        );
        assert_eq!((state.row, state.caret), (1, 2));
    }

    #[test]
    fn the_backend_cycler_always_offers_the_board_s_own_kind() {
        let state = AppState::new("/tmp/fleet-board-settings", std::time::Instant::now());
        assert_eq!(backend_kinds(&state, "remote"), vec!["remote".to_owned()]);
        assert!(schema_for(&state, "remote").is_empty());
    }

    #[test]
    fn non_ascii_prefix_reports_charset_before_length() {
        let draft = BoardSettingsState {
            name: "Board".into(),
            prefix: "界界界界".into(),
            ..Default::default()
        };
        assert!(draft.validate().unwrap().contains("only"));
    }

    #[test]
    fn the_prefix_rule_is_stated_exactly() {
        let mut state = draft();
        assert_eq!(state.validate(), None);
        state.prefix = "fl t".to_owned();
        assert_eq!(
            state.validate().as_deref(),
            Some("prefix must be A\u{2013}Z and 0\u{2013}9 only")
        );
        state.prefix = "TOOOOOLONG".to_owned();
        assert!(state.validate().is_some());
        state.prefix = String::new();
        assert!(state.validate().is_some());
        state.prefix = "FLT".to_owned();
        state.name = "  ".to_owned();
        assert_eq!(state.validate().as_deref(), Some("name must not be empty"));
    }

    #[test]
    fn a_prefix_is_stored_the_way_it_is_shown() {
        let mut state = draft();
        state.row = 1;
        let mut input = state
            .input()
            .unwrap_or_else(|| panic!("prefix is a text row"));
        input.clear();
        input.insert("pay");
        state.set_input(&input);
        assert_eq!(state.prefix, "PAY");
    }

    #[test]
    fn the_repo_cycler_puts_none_first() {
        let repos: Vec<RepoId> = ["a/one", "a/two"]
            .iter()
            .map(|id| RepoId::try_from(*id).unwrap_or_else(|error| panic!("{error}")))
            .collect();
        assert_eq!(repo_position(&repos, None), 0);
        assert_eq!(repo_position(&repos, Some(&repos[1])), 2);
    }

    /// `backend_element` refuses to draw an unset number as `0`, so `h` must not write one.
    #[test]
    fn stepping_down_an_unset_number_row_leaves_it_unset() {
        let mut draft = BoardSettingsState {
            rows: vec![BackendRow {
                key: "fullSyncEvery".into(),
                name: "Full sync every".into(),
                kind: PropertyKind::Number,
                options: Vec::new(),
                required: false,
                value: String::new(),
                present: false,
            }],
            ..Default::default()
        };
        cycle_backend_row(&mut draft, 0, -1);
        assert_eq!(draft.rows[0].value, "");
        assert_eq!(
            draft.rows[0].json(),
            None,
            "`0` here makes every pull a full one, and nothing validates it"
        );
        // Stepping up from empty is a value the user asked for, and comes back down again.
        cycle_backend_row(&mut draft, 0, 1);
        assert_eq!(draft.rows[0].value, "1");
        cycle_backend_row(&mut draft, 0, -1);
        assert_eq!(draft.rows[0].value, "0");
    }
}
