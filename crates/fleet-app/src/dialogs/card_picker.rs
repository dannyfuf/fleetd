//! The one dialog every card property is edited through (BOARD §8).
//!
//! *Every property is a list you can type into.* Status, priority, assignee, labels, estimate,
//! due date, repository and every backend-declared custom property share one surface: a query
//! field over a [`FuzzyList`] of the values the field can take. The kinds that are not closed
//! sets — an assignee nobody has used yet, an estimate that is not on the Fibonacci ladder, a
//! date — accept the typed query itself as the value, which is why the field is an input and
//! not a menu.
//!
//! `Labels` and custom `MultiSelect` properties use multiple selection: `space` toggles the highlighted label, `Enter` applies
//! the whole set at once, because a picker that closes after one label makes tagging a card
//! four keystrokes per label.

use fleet_core::{
    board::{CardPatch, Priority, PropertyKind, PropertyValue},
    ids::{CardId, LabelId, RepoId, StatusId},
};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{DialogHost, Dialogs, notify, root, step, type_into, with_host},
    screens::board,
    state::AppState,
};

/// The estimate ladder §8 offers before the typed value.
pub const ESTIMATES: [u32; 7] = [0, 1, 2, 3, 5, 8, 13];
/// How many rows the list shows at once.
pub const PICKER_ROWS: usize = 8;

/// The field edited by the reusable card picker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PickerKind {
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
    pub fn label(&self) -> String {
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
    pub fn is_multi_select(&self, schema: Option<PropertyKind>) -> bool {
        matches!(self, Self::Labels) || schema == Some(PropertyKind::MultiSelect)
    }

    /// The standard card field this picker writes, in the vocabulary
    /// `SyncState::readonly_fields` uses.
    ///
    /// `None` for the two pickers that touch nothing a backend owns: the repository is Fleet's
    /// own link, and a custom property is declared by the schema that carries its own
    /// `editable` flag.
    #[must_use]
    pub const fn card_field(&self) -> Option<&'static str> {
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
pub struct PickerOption {
    /// The value written back to the card.
    pub value: String,
    /// What the row reads.
    pub label: String,
    /// The right-hand hint, when the value needs one.
    pub detail: Option<String>,
}

impl PickerOption {
    fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            detail: None,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Target and selection for a card property picker.
#[derive(Debug, Clone, Default)]
pub struct CardPickerState {
    /// Property being edited.
    pub kind: PickerKind,
    /// Target card.
    pub card_id: Option<CardId>,
    /// Search text, and — for the open-ended kinds — the value itself.
    pub query: String,
    /// Selected option.
    pub cursor: usize,
    /// Caret inside `query`, as a character offset.
    pub caret: usize,
    /// The chosen values of a multi-select.
    pub selected: Vec<String>,
    /// Whether applying this pick should go on to create the card's worktree.
    pub then_worktree: bool,
    /// Return to the existing card detail after applying or cancelling this picker.
    pub then_detail: bool,
    /// Why the current query cannot be applied.
    pub error: Option<String>,
}

impl CardPickerState {
    /// Toggles an option; the empty option clears the set.
    pub fn toggle_value(&mut self, value: &str) {
        if value.is_empty() {
            self.selected.clear();
        } else if let Some(index) = self.selected.iter().position(|selected| selected == value) {
            self.selected.remove(index);
        } else {
            self.selected.push(value.to_owned());
        }
    }
    /// The surface revealed when this picker closes.
    pub fn return_dialog(&self) -> Option<Dialogs> {
        self.then_detail.then_some(Dialogs::CardDetail)
    }

    /// The query as an editable buffer.
    fn input(&self) -> TextFieldState {
        let mut input = TextFieldState::from_text(self.query.clone());
        for _ in self.caret..input.caret_chars() {
            input.move_left();
        }
        input
    }

    fn set_input(&mut self, input: &TextFieldState) {
        self.query = input.text().to_owned();
        self.caret = input.caret_chars();
    }
}

/// Whether the typed query can be a value of its own for this kind.
#[must_use]
pub fn accepts_free_text(kind: &PickerKind, schema: Option<PropertyKind>) -> bool {
    match kind {
        PickerKind::Assignee | PickerKind::Estimate | PickerKind::DueDate => true,
        PickerKind::Property(_) => matches!(
            schema,
            Some(
                PropertyKind::Text | PropertyKind::Number | PropertyKind::Url | PropertyKind::User
            )
        ),
        _ => false,
    }
}

/// The reason a typed value is refused, or `None` when it is fine.
#[must_use]
pub fn free_text_error(
    kind: &PickerKind,
    value: &str,
    schema: Option<PropertyKind>,
) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    match (kind, schema) {
        // The request builder sends an estimate as `u32`; accepting `2.5` here would show no
        // refusal until Enter failed with a message that names no reason.
        (PickerKind::Estimate, _) => value
            .parse::<u32>()
            .is_err()
            .then(|| format!("`{value}` is not a whole number of points")),
        (PickerKind::Property(_), Some(PropertyKind::Number)) => value
            .parse::<f64>()
            .is_err()
            .then(|| format!("`{value}` is not a number")),
        (PickerKind::DueDate, _) | (PickerKind::Property(_), Some(PropertyKind::Date)) => {
            (!is_iso_date(value)).then(|| format!("`{value}` is not a YYYY-MM-DD date"))
        }
        _ => None,
    }
}

/// Whether `value` is a real `YYYY-MM-DD` calendar date.
///
/// The domain's own calendar, not `epoch_seconds`: that one range-checks the month and the day
/// so it can date a daemon timestamp, and would take `2026-02-31` for a date the daemon then
/// refuses.
#[must_use]
pub fn is_iso_date(value: &str) -> bool {
    fleet_core::board::valid_date(value)
}

/// Every value this picker offers, before the query narrows them.
#[must_use]
pub fn options(state: &AppState, kind: &PickerKind) -> Vec<PickerOption> {
    let Some(view) = state.board() else {
        return Vec::new();
    };
    match kind {
        PickerKind::Status => view
            .board
            .statuses
            .iter()
            .map(|status| {
                PickerOption::new(status.id.as_str(), status.name.clone())
                    .detail(format!("{:?}", status.category).to_lowercase())
            })
            .collect(),
        PickerKind::Priority => Priority::ALL
            .iter()
            .map(|priority| {
                let option =
                    PickerOption::new(format!("{priority:?}").to_lowercase(), priority.label());
                if priority.glyph().is_empty() {
                    option
                } else {
                    option.detail(priority.glyph())
                }
            })
            .collect(),
        PickerKind::Assignee => {
            let mut names: Vec<String> = view
                .cards
                .iter()
                .filter_map(|card| card.assignee.clone())
                .collect();
            names.sort_unstable();
            names.dedup();
            let mut options = vec![PickerOption::new("", "Unassigned").detail("clear")];
            options.extend(
                names
                    .into_iter()
                    .map(|name| PickerOption::new(name.clone(), name)),
            );
            options
        }
        PickerKind::Labels => {
            // `toggle_value("")` exists to empty the set; without a row to reach it, dropping
            // five labels costs five `space`s. Every other multi-value picker offers one.
            let mut options = vec![PickerOption::new("", "No labels").detail("clear")];
            options.extend(
                view.board
                    .labels
                    .iter()
                    .map(|label| PickerOption::new(label.id.as_str(), label.name.clone())),
            );
            options
        }
        PickerKind::Estimate => {
            let mut options = vec![PickerOption::new("", "No estimate").detail("clear")];
            options.extend(
                ESTIMATES
                    .iter()
                    .map(|points| PickerOption::new(points.to_string(), format!("{points} pt"))),
            );
            options
        }
        PickerKind::DueDate => vec![PickerOption::new("", "No due date").detail("clear")],
        PickerKind::Repo => repo_options(
            state
                .snapshot
                .as_ref()
                .map_or(&[][..], |snapshot| &snapshot.repos),
            &view.board.context_id,
        ),
        PickerKind::Property(key) => {
            let Some(schema) = view.board.properties.iter().find(|entry| &entry.key == key) else {
                return Vec::new();
            };
            match schema.kind {
                PropertyKind::Bool => vec![
                    PickerOption::new("true", "Yes"),
                    PickerOption::new("false", "No"),
                ],
                PropertyKind::Select | PropertyKind::MultiSelect => {
                    let mut options = vec![PickerOption::new("", "None").detail("clear")];
                    options.extend(schema.options.iter().map(|option| {
                        PickerOption::new(option.value.clone(), option.label.clone())
                    }));
                    options
                }
                PropertyKind::User => {
                    let mut names: Vec<String> = view
                        .cards
                        .iter()
                        .filter_map(|card| card.assignee.clone())
                        .collect();
                    names.sort_unstable();
                    names.dedup();
                    names
                        .into_iter()
                        .map(|name| PickerOption::new(name.clone(), name))
                        .collect()
                }
                _ => vec![PickerOption::new("", "Clear").detail("clear")],
            }
        }
    }
}

/// The offered values the query keeps, plus the typed value when the kind accepts one.
#[must_use]
pub fn candidates(state: &AppState, draft: &CardPickerState) -> Vec<PickerOption> {
    let schema = property_kind(state, &draft.kind);
    let query = draft.query.trim();
    let mut rows: Vec<PickerOption> = options(state, &draft.kind)
        .into_iter()
        .filter(|option| crate::presentation::contains_folded(&option.label, &query.to_lowercase()))
        .collect();
    if accepts_free_text(&draft.kind, schema)
        && !query.is_empty()
        && !rows.iter().any(|option| option.value == query)
    {
        rows.insert(
            0,
            PickerOption::new(query, query.to_owned()).detail("use this"),
        );
    }
    rows
}

/// The repositories a card may point at: the ones in its own board's context.
///
/// The daemon refuses a repository from another context, so offering one here would offer a
/// refusal — and, on the `w` path, a refusal instead of the worktree the user asked for.
/// `board_settings::repo_choices` scopes the board's default repository the same way.
fn repo_options(
    repos: &[fleet_core::model::Repo],
    context: &fleet_core::ids::ContextId,
) -> Vec<PickerOption> {
    let mut options = vec![PickerOption::new("", "No repository").detail("clear")];
    options.extend(
        repos
            .iter()
            .filter(|repo| repo.context_id == *context)
            .map(|repo| {
                PickerOption::new(repo.id.as_str(), repo.id.as_str())
                    .detail(repo.default_branch.clone())
            }),
    );
    options
}

/// The declared kind of a custom property, when the picker edits one.
/// The picker's own word for the field, resolving a backend property to its schema name.
///
/// `PickerKind::Property` carries the wire key (`jira.issue_type`), and the dialog is the one
/// surface that must never show one: the row that opened it reads "Issue type".
fn picker_label(state: &AppState, kind: &PickerKind) -> String {
    let PickerKind::Property(key) = kind else {
        return kind.label();
    };
    state
        .board()
        .and_then(|view| {
            view.board
                .properties
                .iter()
                .find(|schema| &schema.key == key)
                .map(|schema| schema.name.clone())
        })
        .unwrap_or_else(|| kind.label())
}

fn property_kind(state: &AppState, kind: &PickerKind) -> Option<PropertyKind> {
    let PickerKind::Property(key) = kind else {
        return None;
    };
    state.board().and_then(|view| {
        view.board
            .properties
            .iter()
            .find(|schema| &schema.key == key)
            .map(|schema| schema.kind)
    })
}

/// The value a card currently holds for a single-valued picker, as that picker spells it.
///
/// Multi-value kinds are not here: they open on `selected`, which already carries every value.
fn current_value(card: &fleet_core::board::Card, kind: &PickerKind) -> Option<String> {
    match kind {
        PickerKind::Status => Some(card.status_id.as_str().to_owned()),
        PickerKind::Priority => Some(format!("{:?}", card.priority).to_lowercase()),
        PickerKind::Assignee => Some(card.assignee.clone().unwrap_or_default()),
        PickerKind::Estimate => Some(card.estimate.map(|points| points.to_string())?),
        PickerKind::DueDate => card.due_date.clone(),
        PickerKind::Repo => Some(
            card.repo_id
                .as_ref()
                .map_or_else(String::new, |id| id.as_str().to_owned()),
        ),
        PickerKind::Property(key) => match card.properties.get(key)? {
            PropertyValue::Select(value) | PropertyValue::User(value) => Some(value.clone()),
            PropertyValue::Bool(flag) => Some(flag.to_string()),
            _ => None,
        },
        PickerKind::Labels => None,
    }
}

pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let target = with_host(state, cx, |host| host.card_picker.card_id.clone());
    let card = state
        .read(cx)
        .board()
        .and_then(|view| {
            target
                .as_ref()
                .and_then(|id| view.cards.iter().find(|card| &card.id == id))
        })
        .cloned();
    let kind_now = with_host(state, cx, |host| host.card_picker.kind.clone());
    // The cursor opens on the value the card already holds. Leaving it at `0` makes `Enter` on a
    // picker the user opened only to look at a write: `s` would move the card to the board's
    // first column — a transition pushed to the real issue on a linked board — and `p` would set
    // `Urgent`, whatever the card held.
    // A kind whose ladder does not contain the card's value — every due date, an estimate off
    // the 0/1/2/3/5/8/13 rungs — found no row and left the cursor on the clear row, so `Enter`
    // on a picker opened only to look wiped the field: exactly the failure this seed exists to
    // prevent. Its own value goes into the query instead, where `candidates` offers it back as
    // the "use this" row at index 0.
    let current = card
        .as_ref()
        .and_then(|card| current_value(card, &kind_now))
        .filter(|value| !value.is_empty());
    let at = current.as_ref().and_then(|current| {
        options(state.read(cx), &kind_now)
            .iter()
            .position(|option| option.value == *current)
    });
    let schema = property_kind(state.read(cx), &kind_now);
    let query = match (&at, &current) {
        (None, Some(current)) if accepts_free_text(&kind_now, schema) => current.clone(),
        _ => String::new(),
    };
    with_host(state, cx, |host| {
        let kind = host.card_picker.kind.clone();
        let then_worktree = host.card_picker.then_worktree;
        let then_detail = host.card_picker.then_detail;
        let selected = match (&kind, card.as_ref()) {
            (PickerKind::Labels, Some(card)) => card
                .labels
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            (PickerKind::Property(key), Some(card)) => match card.properties.get(key) {
                Some(PropertyValue::MultiSelect(values)) => values.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        host.card_picker = CardPickerState {
            kind,
            card_id: card.map(|card| card.id),
            selected,
            then_worktree,
            then_detail,
            cursor: at.unwrap_or(0),
            // The caret sits at the end of the seeded value, so `ctrl-u` clears it and a typed
            // character extends it, rather than editing in front of it.
            caret: query.chars().count(),
            query,
            ..Default::default()
        };
    });
}

/// Renders the query field over the offered values.
pub fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let draft = with_host(state, cx, |host| host.card_picker.clone());
    let rows = candidates(state.read(cx), &draft);
    let schema = property_kind(state.read(cx), &draft.kind);
    let invalid = free_text_error(&draft.kind, draft.query.trim(), schema);
    let multi = draft.kind.is_multi_select(schema);

    let label = picker_label(state.read(cx), &draft.kind);
    let mut field = TextField::new(draft.query.clone())
        .label(label.clone())
        .placeholder(if multi {
            "filter values"
        } else {
            "type to filter or set"
        })
        .caret(draft.caret)
        .focused(true);
    if let Some(message) = invalid.clone() {
        field = field.invalid(message);
    }

    let selected = draft.selected.clone();
    let accent = Tone::Accent.color(cx.theme());
    let list = FuzzyList::new(rows.iter().map(|option| {
        let mut item = FuzzyItem::new(option.label.clone());
        if let Some(detail) = option.detail.clone() {
            item = item.trailing(detail);
        }
        if multi && selected.contains(&option.value) {
            item = item.leading(Icon::Check.el().size(IconSize::Small).color(accent));
        }
        item
    }))
    .cursor(draft.cursor)
    .cap(PICKER_ROWS)
    .under_text_field(true)
    // Telling a fixed-list kind to type one is telling it to do the thing `apply` answers with
    // "No match — pick a value": only a kind that takes free text can be typed into.
    .empty(
        Text::ui(if accepts_free_text(&draft.kind, schema) {
            "No values \u{2014} type one."
        } else {
            "No values to pick."
        })
        .muted(),
    );

    // `ctrl-n` / `ctrl-p` are the only way to move the highlight on either branch — `j` and
    // `k` type into the query — so the multi-select row names them too. Without it a Labels
    // picker offered no discoverable way to reach its second row.
    let hints = if multi {
        KeyHintRow::new()
            .key("\u{2303}n/\u{2303}p", "move")
            .key("space", "toggle")
            .key("\u{23ce}", "apply")
            .key("esc", "cancel")
    } else {
        KeyHintRow::new()
            .key("\u{2303}n/\u{2303}p", "move")
            .key("\u{23ce}", "set")
            .key("esc", "cancel")
    };

    let mut card = Dialog::new("Card property")
        .icon(Icon::ArrowRightLeft)
        .width(Dialogs::CardPicker.width(cx))
        .subtitle(format!("\u{00b7} {label}"))
        .body(div().flex().flex_col().child(field).child(list))
        .hint_row(hints)
        .primary("\u{23ce} Apply");
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let apply_state = state.clone();
    let apply_bridge = bridge.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let typed = with_host(&state, cx, |host| {
                    let mut input = host.card_picker.input();
                    if !type_into(&mut input, event) {
                        return false;
                    }
                    host.card_picker.set_input(&input);
                    host.card_picker.cursor = 0;
                    host.card_picker.error = None;
                    true
                });
                if typed {
                    notify(&state, cx);
                    cx.stop_propagation();
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| move_cursor(&state, -1, cx)
        })
        // `tab` moves the highlight here too, as KEYMAP §Board says the generic Dialog context
        // binds it to: bare `j` and `k` type into the query, so a dead `tab` left `ctrl-n` as
        // the only way to move.
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| move_cursor(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_left();
                })
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_right();
                })
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit_query(&state, cx, |input| {
                    input.backspace();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit_query(&state, cx, |input| {
                    input.delete_word_before();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                edit_query(&state, cx, |input| {
                    input.clear();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_to_start();
                })
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_to_end();
                })
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::Toggle, _window, cx| toggle(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Cancel, _window, cx| {
                close(&state, cx);
                cx.stop_propagation();
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            apply(&apply_state, &apply_bridge, cx);
            cx.stop_propagation();
        })
        .child(card)
        .into_any_element()
}

fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let draft = with_host(state, cx, |host| host.card_picker.clone());
    let len = candidates(state.read(cx), &draft).len();
    with_host(state, cx, |host| {
        host.card_picker.cursor = step(host.card_picker.cursor, delta, len);
    });
    notify(state, cx);
    cx.stop_propagation();
}

fn edit_query(state: &Entity<AppState>, cx: &mut App, edit: impl FnOnce(&mut TextFieldState)) {
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
fn toggle(state: &Entity<AppState>, cx: &mut App) {
    let draft = with_host(state, cx, |host| host.card_picker.clone());
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

/// `Enter`: turn the selection into a request, send it, and close.
fn apply(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let draft = with_host(state, cx, |host| host.card_picker.clone());
    let Some(card_id) = draft.card_id.clone() else {
        return;
    };
    if !state
        .read(cx)
        .board()
        .is_some_and(|view| view.cards.iter().any(|card| card.id == card_id))
    {
        // A reload can drop the card out from under an open picker. Every other refusal in this
        // dialog says so; returning silently makes `Enter` a dead key with nothing to read.
        with_host(state, cx, |host| {
            host.card_picker.error = Some("That card is no longer on this board".into());
        });
        notify(state, cx);
        return;
    }
    let schema = property_kind(state.read(cx), &draft.kind);
    if let Some(message) = free_text_error(&draft.kind, draft.query.trim(), schema) {
        with_host(state, cx, |host| host.card_picker.error = Some(message));
        notify(state, cx);
        return;
    }
    let chosen = candidates(state.read(cx), &draft)
        .into_iter()
        .nth(draft.cursor)
        .map(|option| option.value);
    // A multi-select applies `draft.selected`, not the row under the cursor: filtering the list
    // down to nothing after toggling still leaves a perfectly good set to send, and a board with
    // no labels at all would otherwise make `t` a picker that can never apply anything.
    let multi = draft.kind.is_multi_select(schema);
    // A kind whose values are a fixed list takes no typed one: falling back to the query would
    // send a status or label the board does not have and wait for the daemon to say so.
    if chosen.is_none() && !multi && !accepts_free_text(&draft.kind, schema) {
        with_host(state, cx, |host| {
            host.card_picker.error = Some("No match \u{2014} pick a value".into());
        });
        notify(state, cx);
        return;
    }
    let value = chosen.unwrap_or_else(|| draft.query.trim().to_owned());

    if draft.then_worktree && value.is_empty() {
        with_host(state, cx, |host| {
            host.card_picker.error = Some("Pick a repository".into())
        });
        notify(state, cx);
        return;
    }
    let Some(request) = request_for(&draft, &card_id, &value, schema) else {
        with_host(state, cx, |host| {
            host.card_picker.error = Some(format!("`{value}` is not a valid value"));
        });
        notify(state, cx);
        return;
    };

    // The picker returns to the card detail when it came from there, and that dialog's scrim
    // covers the status bar: the refusal has to follow the surface the user is left looking at.
    let refusal = board::Refusal::for_detail(draft.then_detail, &card_id);
    if draft.then_worktree {
        // §8's `w` asked for a repository first: one request links the repo and creates the
        // worktree, so the daemon can never see the create before the repo it needs.
        board::request_worktree_reporting(
            card_id,
            RepoId::try_from(value.as_str()).ok(),
            state,
            bridge,
            refusal,
            cx,
        );
    } else {
        board::send_card_reporting(state, bridge, request, refusal, cx);
    }
    close(state, cx);
}

fn close(state: &Entity<AppState>, cx: &mut App) {
    let then_detail = with_host(state, cx, |host| {
        let dialog = host.card_picker.return_dialog();
        if dialog.is_some() {
            host.open = dialog.clone();
        }
        dialog
    });
    state.update(cx, |app, cx| {
        if let Some(dialog) = then_detail {
            app.open_overlay(crate::state::Overlay::Dialog(dialog));
        } else {
            app.close_overlay();
        }
        cx.notify();
    });
}

/// The request one applied pick turns into.
fn request_for(
    draft: &CardPickerState,
    card_id: &CardId,
    value: &str,
    schema: Option<PropertyKind>,
) -> Option<RequestBody> {
    let empty = value.is_empty();
    let patch = |patch: CardPatch| {
        Some(RequestBody::UpdateCard {
            card_id: card_id.clone(),
            patch,
        })
    };
    match &draft.kind {
        PickerKind::Status => Some(RequestBody::MoveCard {
            card_id: card_id.clone(),
            status_id: StatusId::try_from(value).ok()?,
            index: None,
        }),
        PickerKind::Priority => patch(CardPatch {
            priority: Some(parse_priority(value)?),
            ..CardPatch::default()
        }),
        PickerKind::Assignee => patch(CardPatch {
            assignee: Some((!empty).then(|| value.to_owned())),
            ..CardPatch::default()
        }),
        PickerKind::Labels => patch(CardPatch {
            labels: Some(
                draft
                    .selected
                    .iter()
                    .filter_map(|id| LabelId::try_from(id.as_str()).ok())
                    .collect(),
            ),
            ..CardPatch::default()
        }),
        PickerKind::Estimate => patch(CardPatch {
            estimate: Some(if empty {
                None
            } else {
                Some(value.parse::<u32>().ok()?)
            }),
            ..CardPatch::default()
        }),
        PickerKind::DueDate => patch(CardPatch {
            due_date: Some(if empty {
                None
            } else {
                Some(is_iso_date(value).then(|| value.to_owned())?)
            }),
            ..CardPatch::default()
        }),
        PickerKind::Repo => patch(CardPatch {
            repo_id: Some(if empty {
                None
            } else {
                Some(RepoId::try_from(value).ok()?)
            }),
            ..CardPatch::default()
        }),
        PickerKind::Property(key) => {
            let value = property_value(schema?, &draft.selected, value)?;
            patch(CardPatch {
                properties: Some([(key.clone(), value)].into_iter().collect()),
                ..CardPatch::default()
            })
        }
    }
}

/// The typed value a custom property takes, or `None` when the text does not fit its kind.
fn property_value(kind: PropertyKind, selected: &[String], value: &str) -> Option<PropertyValue> {
    if value.is_empty() && kind != PropertyKind::MultiSelect {
        return Some(PropertyValue::Null);
    }
    Some(match kind {
        PropertyKind::Text => PropertyValue::Text(value.to_owned()),
        PropertyKind::Number => PropertyValue::Number(value.parse().ok()?),
        PropertyKind::Bool => PropertyValue::Bool(value == "true"),
        PropertyKind::Date => PropertyValue::Date(is_iso_date(value).then(|| value.to_owned())?),
        PropertyKind::Select => PropertyValue::Select(value.to_owned()),
        PropertyKind::MultiSelect => PropertyValue::MultiSelect(selected.to_vec()),
        PropertyKind::User => PropertyValue::User(value.to_owned()),
        PropertyKind::Url => PropertyValue::Url(value.to_owned()),
    })
}

/// The priority a picker row's value names.
fn parse_priority(value: &str) -> Option<Priority> {
    Priority::ALL
        .into_iter()
        .find(|priority| format!("{priority:?}").to_lowercase() == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_detail_picker_returns_to_detail_without_reseeding() {
        let draft = CardPickerState {
            then_detail: true,
            ..Default::default()
        };
        assert_eq!(draft.return_dialog(), Some(Dialogs::CardDetail));
        assert_eq!(CardPickerState::default().return_dialog(), None);
    }

    #[test]
    fn the_repository_picker_offers_only_this_context_s_repositories() {
        let repo = |id: &str, context: &str| fleet_core::model::Repo {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            owner: "acme".into(),
            name: id.into(),
            url: "unused".into(),
            context_id: context.parse().unwrap_or_else(|error| panic!("{error}")),
            default_branch: "main".into(),
            path: "/tmp".into(),
            cloned_at: "2026-09-06T12:00:00Z".into(),
            hooks: fleet_core::model::RepoHooks::default(),
        };
        let repos = [repo("acme/api", "work"), repo("acme/site", "home")];
        let context: fleet_core::ids::ContextId =
            "work".parse().unwrap_or_else(|error| panic!("{error}"));
        let values: Vec<_> = repo_options(&repos, &context)
            .into_iter()
            .map(|option| option.value)
            .collect();
        assert_eq!(values, ["", "acme/api"]);
    }

    #[test]
    fn only_the_open_ended_kinds_accept_a_typed_value() {
        assert!(accepts_free_text(&PickerKind::Assignee, None));
        assert!(accepts_free_text(&PickerKind::Estimate, None));
        assert!(accepts_free_text(&PickerKind::DueDate, None));
        assert!(!accepts_free_text(&PickerKind::Status, None));
        assert!(!accepts_free_text(&PickerKind::Labels, None));
        assert!(accepts_free_text(
            &PickerKind::Property("x".into()),
            Some(PropertyKind::Text)
        ));
        assert!(!accepts_free_text(
            &PickerKind::Property("x".into()),
            Some(PropertyKind::Select)
        ));
    }

    #[test]
    fn every_picker_names_the_card_field_it_writes() {
        // These are the words `SyncState::readonly_fields` uses; a mismatch would silently
        // stop the read-only guard from ever recognising a field.
        assert_eq!(PickerKind::Status.card_field(), Some("status_id"));
        assert_eq!(PickerKind::Priority.card_field(), Some("priority"));
        assert_eq!(PickerKind::Assignee.card_field(), Some("assignee"));
        assert_eq!(PickerKind::Labels.card_field(), Some("labels"));
        assert_eq!(PickerKind::Estimate.card_field(), Some("estimate"));
        assert_eq!(PickerKind::DueDate.card_field(), Some("due_date"));
        // The repository is Fleet's own link, and a custom property carries `editable` itself.
        assert_eq!(PickerKind::Repo.card_field(), None);
        assert_eq!(PickerKind::Property("teams".into()).card_field(), None);
    }

    #[test]
    fn dates_are_validated_as_real_calendar_days() {
        assert!(is_iso_date("2026-09-06"));
        assert!(!is_iso_date("2026-13-06"));
        assert!(!is_iso_date("2026-9-6"));
        assert!(!is_iso_date("tomorrow"));
        // Days per month and leap years, exactly as the daemon counts them: a date the picker
        // accepts and the daemon refuses is a refusal the user could have been spared.
        assert!(!is_iso_date("2026-02-31"));
        assert!(!is_iso_date("2026-04-31"));
        assert!(!is_iso_date("2025-02-29"));
        assert!(is_iso_date("2024-02-29"));
        assert_eq!(free_text_error(&PickerKind::DueDate, "", None), None);
        assert!(free_text_error(&PickerKind::DueDate, "soon", None).is_some());
    }

    #[test]
    fn an_estimate_must_be_a_number() {
        assert_eq!(free_text_error(&PickerKind::Estimate, "8", None), None);
        assert!(free_text_error(&PickerKind::Estimate, "big", None).is_some());
    }

    #[test]
    fn a_status_pick_moves_the_card_and_a_priority_pick_patches_it() {
        let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
        let draft = CardPickerState {
            kind: PickerKind::Status,
            ..CardPickerState::default()
        };
        assert!(matches!(
            request_for(&draft, &card, "in-progress", None),
            Some(RequestBody::MoveCard { .. })
        ));
        let draft = CardPickerState {
            kind: PickerKind::Priority,
            ..CardPickerState::default()
        };
        let Some(RequestBody::UpdateCard { patch, .. }) =
            request_for(&draft, &card, "urgent", None)
        else {
            panic!("priority must patch the card");
        };
        assert_eq!(patch.priority, Some(Priority::Urgent));
    }

    #[test]
    fn clearing_a_value_sends_some_none_not_nothing() {
        let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
        let draft = CardPickerState {
            kind: PickerKind::Assignee,
            ..CardPickerState::default()
        };
        let Some(RequestBody::UpdateCard { patch, .. }) = request_for(&draft, &card, "", None)
        else {
            panic!("assignee must patch the card");
        };
        assert_eq!(patch.assignee, Some(None), "`Some(None)` clears the field");
    }

    #[test]
    fn a_multi_select_property_writes_the_whole_toggled_set() {
        let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
        let mut draft = CardPickerState {
            kind: PickerKind::Property("teams".into()),
            ..CardPickerState::default()
        };
        assert!(draft.kind.is_multi_select(Some(PropertyKind::MultiSelect)));
        draft.toggle_value("core");
        draft.toggle_value("ui");
        draft.toggle_value("core");
        draft.toggle_value("");
        assert!(draft.selected.is_empty());
        draft.toggle_value("core");
        draft.toggle_value("ui");
        let Some(RequestBody::UpdateCard { patch, .. }) =
            request_for(&draft, &card, "ui", Some(PropertyKind::MultiSelect))
        else {
            panic!("a property must patch the card");
        };
        let properties = patch.properties.unwrap_or_else(|| panic!("no properties"));
        assert_eq!(
            properties.get("teams"),
            Some(&PropertyValue::MultiSelect(vec![
                "core".to_owned(),
                "ui".to_owned()
            ]))
        );
    }

    #[test]
    fn an_unparsable_value_produces_no_request_at_all() {
        let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
        let draft = CardPickerState {
            kind: PickerKind::Estimate,
            ..CardPickerState::default()
        };
        assert!(request_for(&draft, &card, "many", None).is_none());
        let draft = CardPickerState {
            kind: PickerKind::DueDate,
            ..CardPickerState::default()
        };
        assert!(request_for(&draft, &card, "someday", None).is_none());
    }

    /// A card with nothing set, so a test can say what it is about.
    fn bare_card() -> fleet_core::board::Card {
        let mut board = fleet_core::board::new_board(
            &fleet_core::model::Context {
                id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
                name: "Work".into(),
                owners: vec![],
                created_at: "2026-09-06T12:00:00Z".into(),
            },
            "2026-09-06T12:00:00Z",
        );
        fleet_core::board::create_card(
            &mut board,
            &[],
            "11111111-1111-4111-8111-111111111111"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            fleet_core::board::CardDraft {
                title: "Task".into(),
                ..fleet_core::board::CardDraft::default()
            },
            "2026-09-06T12:00:00Z",
        )
        .unwrap_or_else(|error| panic!("{error}"))
    }

    /// The cursor opens on the value the card holds. At `0` a picker opened only to look at is
    /// a write: `Enter` moves the card to the first column, or sets `Urgent`.
    #[test]
    fn a_picker_opens_on_the_value_its_card_already_holds() {
        let mut card = bare_card();
        assert_eq!(
            current_value(&card, &PickerKind::Status).as_deref(),
            Some(card.status_id.as_str())
        );
        card.priority = fleet_core::board::Priority::Low;
        assert_eq!(
            current_value(&card, &PickerKind::Priority).as_deref(),
            Some("low"),
            "the priority picker spells its values the way `Priority::ALL` does"
        );
        // An unset optional value is the picker's own clear row, which is always first.
        assert_eq!(
            current_value(&card, &PickerKind::Assignee).as_deref(),
            Some("")
        );
        card.assignee = Some("Ana Rojas".into());
        assert_eq!(
            current_value(&card, &PickerKind::Assignee).as_deref(),
            Some("Ana Rojas")
        );
        card.estimate = Some(3);
        assert_eq!(
            current_value(&card, &PickerKind::Estimate).as_deref(),
            Some("3")
        );
        // Labels are multi-valued and open on `selected` instead.
        assert_eq!(current_value(&card, &PickerKind::Labels), None);
    }

    /// The priority values the picker offers must be the ones `current_value` answers with, or
    /// the cursor lands on row zero and `Enter` writes `Urgent`.
    #[test]
    fn every_priority_the_picker_offers_is_one_a_card_can_report() {
        let spelled: Vec<String> = fleet_core::board::Priority::ALL
            .iter()
            .map(|priority| format!("{priority:?}").to_lowercase())
            .collect();
        for priority in fleet_core::board::Priority::ALL {
            let mut card = bare_card();
            card.priority = priority;
            let value = current_value(&card, &PickerKind::Priority).unwrap_or_default();
            assert!(spelled.contains(&value), "{value} is not an offered row");
        }
    }
}
