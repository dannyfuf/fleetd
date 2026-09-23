use super::*;

/// Whether the typed query can be a value of its own for this kind.
#[must_use]
pub(super) fn accepts_free_text(kind: &PickerKind, schema: Option<PropertyKind>) -> bool {
    match kind {
        PickerKind::Assignee | PickerKind::Estimate | PickerKind::DueDate => true,
        // The catalogue is only what this client has seen a harness declare, never the whole
        // truth: a model or an effort the app has not met yet still has to be settable, so the
        // typed query is a row of its own and nothing validates it (contracts §5.3). The
        // provider is a closed set of two and is not typed into.
        PickerKind::Model | PickerKind::Effort => true,
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
///
/// `card` is the card being edited: the link and agent kinds answer from it — which cards may
/// block it without closing a cycle, and which models the harness that would run it declared —
/// so they cannot be derived from the board alone.
#[must_use]
pub(super) fn options(
    state: &AppState,
    kind: &PickerKind,
    card: Option<&CardId>,
) -> Vec<PickerOption> {
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
        PickerKind::BlockedBy | PickerKind::Blocks => {
            let Some(card) = card.and_then(|id| view.cards.iter().find(|card| &card.id == id))
            else {
                return Vec::new();
            };
            link_options(view, kind, card)
        }
        PickerKind::Provider => {
            let mut options = vec![inherited_row()];
            options.extend(
                [AgentKind::Claude, AgentKind::Codex]
                    .into_iter()
                    .map(|provider| {
                        PickerOption::new(provider.executable(), provider.executable())
                            .detail(provider.display_name())
                    }),
            );
            options
        }
        PickerKind::Model | PickerKind::Effort => {
            let (provider, model) = card
                .and_then(|id| view.cards.iter().find(|card| &card.id == id))
                .map_or((None, None), |card| resolved_prefs(view, card));
            let mut options = vec![inherited_row()];
            if matches!(kind, PickerKind::Model) {
                options.extend(
                    declared_models(state, provider)
                        .into_iter()
                        .map(|model| described_row(model.id, model.display_name)),
                );
            } else {
                options.extend(
                    declared_efforts(state, provider, model.as_deref())
                        .into_iter()
                        .map(|effort| described_row(effort.id, effort.description)),
                );
            }
            options
        }
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

/// The row that hands a card's agent choice back to the column it sits in.
///
/// The empty value is how every other picker spells "clear", and clearing one of the three
/// agent fields is exactly what inheriting the column's again means (contracts §5.3).
fn inherited_row() -> PickerOption {
    PickerOption::new("", "column default").detail("clear")
}

/// A row whose value is its own label, with the harness's description beside it.
fn described_row(value: String, description: String) -> PickerOption {
    let option = PickerOption::new(value.clone(), value);
    if description.trim().is_empty() || description == option.label {
        option
    } else {
        option.detail(description)
    }
}

/// The cards either link picker offers: every other card the board still shows.
///
/// A candidate that would close a dependency loop is listed and disabled rather than hidden,
/// because a row that vanishes explains nothing. The rule is the daemon's own
/// `validate_links`, run against the link this row would add, so the picker can never disable
/// a row the daemon would have taken or offer one it will refuse.
fn link_options(
    view: &fleet_core::board::BoardView,
    kind: &PickerKind,
    card: &fleet_core::board::Card,
) -> Vec<PickerOption> {
    let mut options = vec![
        PickerOption::new(
            "",
            if matches!(kind, PickerKind::Blocks) {
                "Blocks nothing"
            } else {
                "No blockers"
            },
        )
        .detail("clear"),
    ];
    // One probe, rewritten per candidate: `validate_links` reads only the id and the blocker
    // set, and cloning a card with its comments and activity per row is a walk of the board.
    let mut probe = card.clone();
    for candidate in view
        .cards
        .iter()
        .filter(|other| !other.archived && other.id != card.id)
    {
        // `BlockedBy` adds the candidate to this card's blockers; `Blocks` adds this card to
        // the candidate's, which is the same edge seen from the other end.
        if matches!(kind, PickerKind::Blocks) {
            probe.id = candidate.id.clone();
            probe.blocked_by = vec![card.id.clone()];
        } else {
            probe.id = card.id.clone();
            probe.blocked_by = vec![candidate.id.clone()];
        }
        let option = PickerOption::new(
            candidate.id.as_str(),
            format!(
                "{}  {}",
                candidate.display_key(&view.board),
                candidate.title
            ),
        );
        // Every listed candidate is on this board and none of them is this card, so the only
        // refusal `validate_links` has left to give is the cycle one.
        options.push(
            if fleet_core::board::validate_links(&view.board, &view.cards, &probe).is_err() {
                option.detail("would cycle").disabled()
            } else {
                option.detail(column_name(view, candidate))
            },
        );
    }
    options
}

/// The name of the column a card sits in, for the right-hand hint of a link row.
fn column_name(view: &fleet_core::board::BoardView, card: &fleet_core::board::Card) -> String {
    view.board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .map_or_else(String::new, |status| status.name.clone())
}

/// The provider and model a run of this card would use: the card's own choice, else the one
/// its column asks for.
///
/// `automation::resolve_prefs` is the daemon's own resolution, so the catalogue the Model row
/// offers is the catalogue of the harness that would really run the card.
fn resolved_prefs(
    view: &fleet_core::board::BoardView,
    card: &fleet_core::board::Card,
) -> (Option<AgentKind>, Option<String>) {
    let action = view
        .board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .and_then(|status| status.automation.as_ref())
        .and_then(|automation| automation.on_enter.as_ref());
    let Some(action) = action else {
        let prefs = card.agent.as_ref();
        return (
            prefs.and_then(|prefs| prefs.provider),
            prefs.and_then(|prefs| prefs.model.clone()),
        );
    };
    let resolved = fleet_core::board::resolve_prefs(card, action);
    (resolved.provider, resolved.model)
}

/// Every model this client has seen `provider` declare, first thread first.
///
/// This is the composer's model picker's own source — a harness declares its vocabulary on the
/// thread it runs in, and there is no board-wide catalogue — read across the threads this
/// client has opened instead of the one thread the composer sits in. A provider the app has
/// never opened a thread for offers nothing, which is why the typed row exists.
fn declared_models(
    state: &AppState,
    provider: Option<AgentKind>,
) -> Vec<fleet_core::agents::ModelDescriptor> {
    let mut models: Vec<fleet_core::agents::ModelDescriptor> = Vec::new();
    for summary in state.agents.summaries() {
        if provider.is_some_and(|wanted| wanted != summary.provider) {
            continue;
        }
        let Some(projection) = state.agents.projection(summary.thread) else {
            continue;
        };
        for model in &projection.models {
            if !models.iter().any(|seen| seen.id == model.id) {
                models.push(model.clone());
            }
        }
    }
    models
}

/// Every reasoning effort `provider` has declared on this client's threads, over all of its
/// models, in harness order: the Settings Agents section's effort vocabulary beyond the base.
pub(crate) fn declared_effort_ids(state: &AppState, provider: AgentKind) -> Vec<String> {
    declared_efforts(state, Some(provider), None)
        .into_iter()
        .map(|effort| effort.id)
        .collect()
}

/// The efforts declared for the model this card would run under, in harness order.
///
/// `^s e` offers the selected model's efforts and no others. A model nobody has declared —
/// typed here, or belonging to a harness this client has not opened — would narrow that to an
/// empty list, so the provider's whole vocabulary stands in rather than no rows at all.
fn declared_efforts(
    state: &AppState,
    provider: Option<AgentKind>,
    model: Option<&str>,
) -> Vec<fleet_core::agents::ReasoningEffortDescriptor> {
    let models = declared_models(state, provider);
    let matching: Vec<&fleet_core::agents::ModelDescriptor> = models
        .iter()
        .filter(|descriptor| model.is_none_or(|id| descriptor.id == id))
        .collect();
    let source = if matching.is_empty() {
        models.iter().collect()
    } else {
        matching
    };
    let mut efforts: Vec<fleet_core::agents::ReasoningEffortDescriptor> = Vec::new();
    for descriptor in source {
        for effort in &descriptor.efforts {
            if !efforts.iter().any(|seen| seen.id == effort.id) {
                efforts.push(effort.clone());
            }
        }
    }
    efforts
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
    /// The threads whose harnesses have declared a model vocabulary: the Model and Effort rows
    /// are read from them, and a thread opened under the picker adds rows the board never sees.
    agents: u64,
    /// The vocabulary itself, which lands on a projection rather than on a summary: without it
    /// a picker opened before a harness declared its models keeps the empty list it built.
    vocabulary: u64,
    kind: PickerKind,
    /// The link and agent kinds answer from one card, so two cards of the same kind are two
    /// different lists.
    card: Option<CardId>,
    query: String,
}

/// The offered rows for `draft`, derived only when one of the inputs above changed.
pub(super) fn prepare(
    state: &AppState,
    draft: &mut CardPickerState,
    query: &str,
) -> std::rc::Rc<[PickerOption]> {
    let key = PreparedKey {
        snapshot: state.snapshot_revision,
        board: state.board.revision,
        agents: state.agents.summaries_revision(),
        vocabulary: state.agents.vocabulary_revision(),
        kind: draft.kind.clone(),
        card: draft.card_id.clone(),
        query: query.to_owned(),
    };
    if draft.prepared.as_ref() != Some(&key) {
        let rows = candidates(state, draft, query);
        draft.rows = rows.into();
        draft.prepared = Some(key);
    }
    draft.rows.clone()
}

/// The offered values the query keeps, plus the typed value when the kind accepts one.
#[must_use]
pub(super) fn candidates(
    state: &AppState,
    draft: &CardPickerState,
    query: &str,
) -> Vec<PickerOption> {
    let schema = property_kind(state, &draft.kind);
    let query = query.trim();
    // Folded once, not once per offered value: this runs on every keystroke and every draw.
    let needle = query.to_lowercase();
    let mut rows: Vec<PickerOption> = options(state, &draft.kind, draft.card_id.as_ref())
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
        // The three agent kinds answer from the card's own choice and never from the column's:
        // an inherited value opens on `column default`, which is the row that clears it.
        PickerKind::Provider => Some(
            card.agent
                .as_ref()
                .and_then(|prefs| prefs.provider)
                .map_or_else(String::new, |provider| provider.executable().to_owned()),
        ),
        PickerKind::Model => Some(
            card.agent
                .as_ref()
                .and_then(|prefs| prefs.model.clone())
                .unwrap_or_default(),
        ),
        PickerKind::Effort => Some(
            card.agent
                .as_ref()
                .and_then(|prefs| prefs.effort.clone())
                .unwrap_or_default(),
        ),
        // The two link kinds are multi-valued, so they open on `selected` like `Labels`.
        PickerKind::Labels | PickerKind::BlockedBy | PickerKind::Blocks => None,
    }
}
