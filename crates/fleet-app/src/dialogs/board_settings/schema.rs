use super::*;

/// One backend settings row: a schema entry plus the value being edited.
///
/// The value is always held as **text**, whatever the kind: that is what a caret, a partially
/// typed number and an empty-means-unset row all need. [`rows_to_settings`] is the one place
/// that turns it back into typed JSON.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BackendRow {
    /// The JSON key inside `BackendRef.settings`.
    pub(super) key: String,
    /// What the row is called, with [`fleet_core::board::REQUIRED_MARKER`] already stripped.
    pub(super) name: String,
    /// Which control the row draws and which JSON type it writes.
    pub(super) kind: PropertyKind,
    /// The closed set a `Select` row cycles.
    pub(super) options: Vec<PropertyOption>,
    /// Whether an empty value stops the save.
    pub(super) required: bool,
    /// The value, as typed.
    pub(super) value: String,
    /// Whether the settings object carried this key when the dialog opened.
    ///
    /// Only a `Bool` needs it: `false` and "not set" are different things to a backend whose
    /// own default is `true`, and every other kind says "not set" by being empty.
    pub(super) present: bool,
}

impl BackendRow {
    /// Whether typing lands in this row instead of moving the cursor.
    #[must_use]
    pub(super) const fn is_text(&self) -> bool {
        !matches!(self.kind, PropertyKind::Bool | PropertyKind::Select)
    }

    /// Whether the row draws a checkbox.
    #[must_use]
    pub(super) const fn is_flag(&self) -> bool {
        matches!(self.kind, PropertyKind::Bool)
    }

    /// The flag a `Bool` row holds.
    #[must_use]
    pub(super) fn flag(&self) -> bool {
        self.value == "true"
    }

    /// Whether a character may be typed into this row.
    ///
    /// A number row takes digits only, which is what lets `h` and `l` keep their cycling
    /// meaning there: the letter is refused, so the key falls through to the cursor.
    #[must_use]
    pub(super) fn accepts(&self, text: &str) -> bool {
        if !self.is_text() {
            return false;
        }
        self.kind != PropertyKind::Number
            || text.chars().all(|character| character.is_ascii_digit())
    }

    /// The failing rule of this row, in the wording the dialog shows it in.
    #[must_use]
    pub(super) fn error(&self) -> Option<String> {
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
    pub(super) fn json(&self) -> Option<serde_json::Value> {
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
///
/// `PropertySchema` has no `required` flag, so the name's suffix is the one signal a backend
/// has to say "this row must be filled in" — `docs/BOARD-JIRA.md` §5 names Jira's `project`
/// row `Project key (required)` for exactly that reason.
pub(super) use fleet_core::board::is_required;

/// The row label of a schema entry: its name without the required marker.
///
/// The marker is stripped because the row states the rule with its own `required` styling
/// instead, and showing both would say it twice.
pub(super) use fleet_core::board::schema_row_name as row_name;

/// Turns a backend's settings schema and a settings object into editable rows.
///
/// Keys the schema does not mention are left alone — they are still in `settings` and
/// [`rows_to_settings`] carries them through, so a setting only `fleet board set` can write is
/// never silently dropped by opening this dialog.
#[must_use]
pub(super) fn backend_rows(
    schema: &[PropertySchema],
    settings: &serde_json::Value,
) -> Vec<BackendRow> {
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
#[must_use]
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
pub(super) fn rows_to_settings(base: &serde_json::Value, rows: &[BackendRow]) -> serde_json::Value {
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
pub(super) fn rows_error(rows: &[BackendRow]) -> Option<String> {
    rows.iter().find_map(BackendRow::error)
}

/// The rows of the dialog, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingRow {
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
pub(super) const FIXED_ROWS: [SettingRow; 7] = [
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
    pub(super) const fn label(self) -> &'static str {
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
pub(super) const POLICIES: [ConflictPolicy; 3] = [
    ConflictPolicy::Manual,
    ConflictPolicy::RemoteWins,
    ConflictPolicy::LocalWins,
];

/// The word a policy reads as.
#[must_use]
pub(super) const fn policy_label(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Manual => "manual",
        ConflictPolicy::RemoteWins => "remote wins",
        ConflictPolicy::LocalWins => "local wins",
    }
}

/// The repositories a board may default to: the ones in its own context.
#[must_use]
pub(super) fn repo_choices(state: &AppState) -> Vec<RepoId> {
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
pub(super) fn backend_kinds(state: &AppState, current: &str) -> Vec<String> {
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
pub(super) fn schema_for(state: &AppState, kind: &str) -> Vec<PropertySchema> {
    state
        .backend_descriptor(kind)
        .map(|descriptor| descriptor.settings_schema.clone())
        .unwrap_or_default()
}
