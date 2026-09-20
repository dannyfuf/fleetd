use super::*;

/// The field edited by the reusable card picker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum PickerKind {
    /// Status column.
    #[default]
    Status,
    /// Priority level.
    Priority,
    /// Assignee name.
    Assignee,
    /// Label set.
    Labels,
    /// Estimate points.
    Estimate,
    /// Due date.
    DueDate,
    /// Repository.
    Repo,
    /// Custom property schema key.
    Property(String),
}

impl PickerKind {
    /// The dialog subtitle: which field is being edited.
    #[must_use]
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Status => "Status".to_owned(),
            Self::Priority => "Priority".to_owned(),
            Self::Assignee => "Assignee".to_owned(),
            Self::Labels => "Labels".to_owned(),
            Self::Estimate => "Estimate".to_owned(),
            Self::DueDate => "Due date".to_owned(),
            Self::Repo => "Repository".to_owned(),
            Self::Property(key) => key.clone(),
        }
    }

    /// Whether `space` toggles rows instead of typing.
    #[must_use]
    pub(super) fn is_multi_select(&self, schema: Option<PropertyKind>) -> bool {
        matches!(self, Self::Labels) || schema == Some(PropertyKind::MultiSelect)
    }

    /// The standard card field this picker writes, in the vocabulary
    /// `SyncState::readonly_fields` uses.
    ///
    /// `None` for the two pickers that touch nothing a backend owns: the repository is Fleet's
    /// own link, and a custom property is declared by the schema that carries its own
    /// `editable` flag.
    #[must_use]
    pub(crate) const fn card_field(&self) -> Option<&'static str> {
        Some(match self {
            Self::Status => "status_id",
            Self::Priority => "priority",
            Self::Assignee => "assignee",
            Self::Labels => "labels",
            Self::Estimate => "estimate",
            Self::DueDate => "due_date",
            Self::Repo | Self::Property(_) => return None,
        })
    }
}

/// One offered value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PickerOption {
    /// The value written back to the card.
    pub(super) value: String,
    /// What the row reads.
    pub(super) label: String,
    /// The right-hand hint, when the value needs one.
    pub(super) detail: Option<String>,
}

impl PickerOption {
    pub(super) fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            detail: None,
        }
    }

    pub(super) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Target and selection for a card property picker.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardPickerState {
    /// Property being edited.
    pub(crate) kind: PickerKind,
    /// Target card.
    pub(crate) card_id: Option<CardId>,
    /// Selected option.
    pub(super) cursor: usize,
    /// The chosen values of a multi-select.
    pub(super) selected: Vec<String>,
    /// Whether applying this pick should go on to create the card's worktree.
    pub(crate) then_worktree: bool,
    /// Return to the existing card detail after applying or cancelling this picker.
    pub(crate) then_detail: bool,
    /// Why the current query cannot be applied.
    pub(super) error: Option<String>,
    /// The values [`candidates`] offered for the draft below, prepared once per change.
    pub(super) rows: std::rc::Rc<[PickerOption]>,
    /// Every input those rows were derived from, so a redraw does not derive them again.
    pub(super) prepared: Option<PreparedKey>,
}

impl CardPickerState {
    /// Toggles an option; the empty option clears the set.
    pub(super) fn toggle_value(&mut self, value: &str) {
        if value.is_empty() {
            self.selected.clear();
        } else if let Some(index) = self.selected.iter().position(|selected| selected == value) {
            self.selected.remove(index);
        } else {
            self.selected.push(value.to_owned());
        }
    }
    /// The surface revealed when this picker closes.
    #[must_use]
    pub(super) fn return_dialog(&self) -> Option<Dialogs> {
        self.then_detail.then_some(Dialogs::CardDetail)
    }
}

/// The offered rows already prepared by an input or backing-state change.
///
/// This accessor is intentionally read-only: on a board of any size `options` walks every card
/// and `candidates` folds every label, while render prepares nothing.
pub(super) fn prepared(state: &Entity<AppState>, cx: &mut App) -> std::rc::Rc<[PickerOption]> {
    read_host(state, cx, |host, _| host.card_picker.rows.clone())
}

/// `j` / `k`: move the highlight, clamped to the candidates the query left.
pub(super) fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let len = prepared(state, cx).len();
    with_host(state, cx, |host| {
        host.card_picker.cursor = step(host.card_picker.cursor, delta, len);
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// `space`: toggle the highlighted value of a multi-select.
pub(super) fn toggle(state: &Entity<AppState>, cx: &mut App) {
    let draft = read_host(state, cx, |host, _| host.card_picker.clone());
    if !draft
        .kind
        .is_multi_select(property_kind(state.read(cx), &draft.kind))
    {
        // The query deliberately rejects spaces so this documented exception can own the key.
        cx.stop_propagation();
        return;
    }
    let Some(option) = prepared(state, cx).get(draft.cursor).cloned() else {
        return;
    };
    with_host(state, cx, |host| {
        host.card_picker.toggle_value(&option.value);
    });
    notify(state, cx);
    cx.stop_propagation();
}
