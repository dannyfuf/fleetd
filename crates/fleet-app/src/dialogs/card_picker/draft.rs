use super::*;

/// The field edited by the reusable card picker.
///
/// The five workflow kinds answer here in full — their rows, their current value and the
/// requests they apply — and the card detail's property rows (P7-T02) are what open them; the
/// board's `b` and `m` (P9-T01) are the second door onto the same picker. The `dead_code`
/// expectation that held the enum honest while nothing constructed the five went unfulfilled
/// the moment those rows landed, which is exactly the signal its author asked for.
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
    /// The cards this one waits for.
    BlockedBy,
    /// The cards that wait for this one.
    Blocks,
    /// Which agent runs this card, or the column's default.
    Provider,
    /// Which model runs this card, or the column's default.
    Model,
    /// Which reasoning effort runs this card, or the column's default.
    Effort,
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
            Self::BlockedBy => "Blocked by".to_owned(),
            Self::Blocks => "Blocks".to_owned(),
            Self::Provider => "Provider".to_owned(),
            Self::Model => "Model".to_owned(),
            Self::Effort => "Effort".to_owned(),
            Self::Property(key) => key.clone(),
        }
    }

    /// Whether `space` toggles rows instead of typing.
    #[must_use]
    pub(super) fn is_multi_select(&self, schema: Option<PropertyKind>) -> bool {
        matches!(self, Self::Labels | Self::BlockedBy | Self::Blocks)
            || schema == Some(PropertyKind::MultiSelect)
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
            // Both link pickers write the same field, on either end of the link: `Blocks`
            // patches `blocked_by` on the dependants rather than on this card.
            Self::BlockedBy | Self::Blocks => "blocked_by",
            Self::Provider | Self::Model | Self::Effort => "agent",
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
    /// Whether the row is listed but cannot be taken.
    ///
    /// A cycle candidate is shown rather than hidden — a row that vanishes explains nothing —
    /// and the reason rides in [`Self::detail`] (contracts §5.3).
    pub(super) disabled: bool,
}

impl PickerOption {
    pub(super) fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            detail: None,
            disabled: false,
        }
    }

    pub(super) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Lists the row without letting it be taken.
    pub(super) fn disabled(mut self) -> Self {
        self.disabled = true;
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
    /// Scroll position of the offered rows, so the cursor can be kept in view past the fold.
    pub(super) scroll: gpui::ScrollHandle,
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
        FuzzyList::reveal(&host.card_picker.scroll, host.card_picker.cursor);
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// A click on row `index`: the same as moving the cursor there and pressing the row's key —
/// `space` on a multi-select, which keeps the dialog open for the next label, `⏎` otherwise.
pub(super) fn click_row(state: &Entity<AppState>, bridge: &Bridge, index: usize, cx: &mut App) {
    let len = prepared(state, cx).len();
    if index >= len {
        return;
    }
    let multi = with_host(state, cx, |host| {
        host.card_picker.cursor = index;
        host.card_picker.kind.clone()
    });
    let multi = multi.is_multi_select(property_kind(state.read(cx), &multi));
    if multi {
        toggle(state, cx);
    } else {
        super::lifecycle::apply(state, bridge, cx);
    }
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
    // A listed row that cannot be taken still owns the key: consuming `space` without moving
    // the set is what keeps the dialog open with its reason on the row (contracts §5.3).
    if option.disabled {
        cx.stop_propagation();
        return;
    }
    with_host(state, cx, |host| {
        host.card_picker.toggle_value(&option.value);
    });
    notify(state, cx);
    cx.stop_propagation();
}
