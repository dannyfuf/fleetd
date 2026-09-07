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
    /// Search text, and — for the open-ended kinds — the value itself.
    pub(super) query: String,
    /// Selected option.
    pub(super) cursor: usize,
    /// Caret inside `query`, as a character offset.
    pub(super) caret: usize,
    /// The chosen values of a multi-select.
    pub(super) selected: Vec<String>,
    /// Whether applying this pick should go on to create the card's worktree.
    pub(crate) then_worktree: bool,
    /// Return to the existing card detail after applying or cancelling this picker.
    pub(crate) then_detail: bool,
    /// Why the current query cannot be applied.
    pub(super) error: Option<String>,
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

    /// The query as an editable buffer.
    #[must_use]
    pub(super) fn input(&self) -> TextFieldState {
        let mut input = TextFieldState::from_text(self.query.clone());
        for _ in self.caret..input.caret_chars() {
            input.move_left();
        }
        input
    }

    pub(super) fn set_input(&mut self, input: &TextFieldState) {
        self.query = input.text().to_owned();
        self.caret = input.caret_chars();
    }
}

/// `j` / `k`: move the highlight, clamped to the candidates the query left.
pub(super) fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let draft = read_host(state, cx, |host, _| host.card_picker.clone());
    let len = candidates(state.read(cx), &draft).len();
    with_host(state, cx, |host| {
        host.card_picker.cursor = step(host.card_picker.cursor, delta, len);
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// Runs a text edit against the query and re-aims the highlight at the first candidate.
///
/// The cursor resets because the row it was on is not the row that index names once the query
/// narrowed the list — keeping it would apply a value the user is no longer looking at.
pub(super) fn edit_query(
    state: &Entity<AppState>,
    cx: &mut App,
    edit: impl FnOnce(&mut TextFieldState),
) {
    with_host(state, cx, |host| {
        let mut input = host.card_picker.input();
        edit(&mut input);
        host.card_picker.set_input(&input);
        host.card_picker.cursor = 0;
        host.card_picker.error = None;
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
        // Everything else is a single choice, so `space` is just a space.
        edit_query(state, cx, |input| input.insert(" "));
        return;
    }
    let Some(option) = candidates(state.read(cx), &draft)
        .into_iter()
        .nth(draft.cursor)
    else {
        return;
    };
    with_host(state, cx, |host| {
        host.card_picker.toggle_value(&option.value);
    });
    notify(state, cx);
    cx.stop_propagation();
}
