use super::*;

/// Whether the typed query can be a value of its own for this kind.
#[must_use]
pub(super) fn accepts_free_text(kind: &PickerKind, schema: Option<PropertyKind>) -> bool {
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
pub(super) fn free_text_error(
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
pub(super) fn is_iso_date(value: &str) -> bool {
    fleet_core::board::valid_date(value)
}

/// Every value this picker offers, before the query narrows them.
#[must_use]
pub(super) fn options(state: &AppState, kind: &PickerKind) -> Vec<PickerOption> {
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

/// Every input the offered rows are derived from, as revisions and cheap values.
///
/// `options` walks the board's cards and `candidates` folds every label, so the rows are keyed
/// the way the Hub and the board key their projections rather than rebuilt per draw
/// (`docs/APP-CONTRACTS.md`, "render prepares nothing"). The snapshot is in the key because the
/// repository picker offers `snapshot.repos`; the board revision because every other kind reads
/// the board, and a card edit lands through `apply_card` and never through a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedKey {
    snapshot: u64,
    board: u64,
    kind: PickerKind,
    query: String,
}

/// The offered rows for `draft`, derived only when one of the inputs above changed.
pub(super) fn prepare(
    state: &AppState,
    draft: &mut CardPickerState,
) -> std::rc::Rc<[PickerOption]> {
    let key = PreparedKey {
        snapshot: state.snapshot_revision,
        board: state.board.revision,
        kind: draft.kind.clone(),
        query: draft.query.clone(),
    };
    if draft.prepared.as_ref() != Some(&key) {
        let rows = candidates(state, draft);
        draft.rows = rows.into();
        draft.prepared = Some(key);
    }
    draft.rows.clone()
}

/// The offered values the query keeps, plus the typed value when the kind accepts one.
#[must_use]
pub(super) fn candidates(state: &AppState, draft: &CardPickerState) -> Vec<PickerOption> {
    let schema = property_kind(state, &draft.kind);
    let query = draft.query.trim();
    // Folded once, not once per offered value: this runs on every keystroke and every draw.
    let needle = query.to_lowercase();
    let mut rows: Vec<PickerOption> = options(state, &draft.kind)
        .into_iter()
        .filter(|option| crate::presentation::contains_folded(&option.label, &needle))
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
pub(super) fn repo_options(
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

/// The picker's own word for the field, resolving a backend property to its schema name.
///
/// `PickerKind::Property` carries the wire key (`jira.issue_type`), and the dialog is the one
/// surface that must never show one: the row that opened it reads "Issue type".
#[must_use]
pub(super) fn picker_label(state: &AppState, kind: &PickerKind) -> String {
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

/// The declared kind of a custom property, when the picker edits one.
#[must_use]
pub(super) fn property_kind(state: &AppState, kind: &PickerKind) -> Option<PropertyKind> {
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
#[must_use]
pub(super) fn current_value(card: &fleet_core::board::Card, kind: &PickerKind) -> Option<String> {
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
