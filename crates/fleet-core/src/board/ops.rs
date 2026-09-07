//! Pure board validation, ordering, and mutation contracts.
use super::{model::*, property::*};
use crate::ids::{CardId, LabelId, RepoId, StatusId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
/// Values used to create a new card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardDraft {
    /// Title.
    pub title: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Status id.
    #[serde(default)]
    pub status_id: Option<StatusId>,
    /// Priority.
    #[serde(default)]
    pub priority: Priority,
    /// Labels.
    #[serde(default)]
    pub labels: Vec<LabelId>,
    /// Assignee.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Estimate.
    #[serde(default)]
    pub estimate: Option<u32>,
    /// Due date.
    #[serde(default)]
    pub due_date: Option<String>,
    /// Parent id.
    #[serde(default)]
    pub parent_id: Option<CardId>,
    /// Repo id.
    #[serde(default)]
    pub repo_id: Option<RepoId>,
    /// Properties.
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
}

/// `None` = leave unchanged; `Some(None)` = clear. All fields optional.
/// Optional changes; nested options distinguish clearing from omission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardPatch {
    /// Title.
    pub title: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Status id.
    pub status_id: Option<StatusId>,
    /// Priority.
    pub priority: Option<Priority>,
    /// Labels.
    pub labels: Option<Vec<LabelId>>,
    /// Assignee.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee: Option<Option<String>>,
    /// Estimate.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub estimate: Option<Option<u32>>,
    /// Due date.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_date: Option<Option<String>>,
    /// Parent id.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Option<Option<CardId>>,
    /// Repo id.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub repo_id: Option<Option<RepoId>>,
    /// Properties.
    pub properties: Option<BTreeMap<String, PropertyValue>>, /* merge; Null removes */
    /// Archived.
    pub archived: Option<bool>,
}
impl CardPatch {
    /// Whether every patch field is absent.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Optional board configuration changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoardPatch {
    /// Name.
    pub name: Option<String>,
    /// Prefix.
    pub prefix: Option<String>,
    /// Backend.
    pub backend: Option<BackendRef>,
    /// Statuses.
    pub statuses: Option<Vec<Status>>,
    /// Labels.
    pub labels: Option<Vec<Label>>,
    /// Properties.
    pub properties: Option<Vec<PropertySchema>>,
    /// Default repo id.
    #[serde(
        default,
        deserialize_with = "nested_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_repo_id: Option<Option<RepoId>>,
    /// Settings.
    pub settings: Option<BoardSettings>,
}
impl BoardPatch {
    /// Whether every patch field is absent.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Validation, lookup, or backend failure in a board operation.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BoardError {
    /// Board not found.
    #[error("board not found: {0}")]
    BoardNotFound(String),
    /// Card not found.
    #[error("card not found: {0}")]
    CardNotFound(String),
    /// Unknown status.
    #[error("unknown status: {0}")]
    UnknownStatus(String),
    /// Unknown label.
    #[error("unknown label: {0}")]
    UnknownLabel(String),
    /// A named input field violates its validation rules.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// Rejected field name.
        field: String,
        /// Explanation of the violated rule.
        reason: String,
    },
    /// The context already owns a board.
    #[error("board already exists for context {0}")]
    Duplicate(String),
    /// Unknown backend.
    #[error("backend `{0}` is not registered")]
    UnknownBackend(String),
    /// Unsupported.
    #[error("backend does not support {0}")]
    Unsupported(&'static str),
    /// Backend.
    #[error("backend error: {0}")]
    Backend(String),
    /// Conflicted.
    #[error("card {0} has an unresolved conflict")]
    Conflicted(String),
    /// A standard card field the board's backend declared it cannot write back.
    #[error("{0} is read-only on this board's backend")]
    ReadOnlyField(String),
}

fn nested_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn invalid(field: &str, reason: &str) -> BoardError {
    BoardError::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

/// Checks the board's prefix and unique schema identifiers.
pub fn validate_board(board: &Board) -> Result<(), BoardError> {
    if board.name.trim().is_empty() {
        return Err(invalid("name", "must not be empty"));
    }
    if !(1..=8).contains(&board.prefix.len())
        || !board
            .prefix
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(invalid(
            "prefix",
            "expected 1..=8 uppercase ASCII letters or digits",
        ));
    }
    if board.statuses.is_empty() {
        return Err(invalid("statuses", "at least one status is required"));
    }
    let mut statuses = HashSet::new();
    if !board.statuses.iter().all(|s| statuses.insert(&s.id)) {
        return Err(invalid("statuses", "duplicate status id"));
    }
    // A display name is what every surface prints for a status, a label or a property: a blank
    // one renders a column header, a chip or a detail row with nothing in it, which no key can
    // then name. `Board.name` and `Card.title` refuse a blank for the same reason.
    if board.statuses.iter().any(|s| s.name.trim().is_empty()) {
        return Err(invalid("statuses", "status names must not be empty"));
    }
    let mut labels = HashSet::new();
    if !board.labels.iter().all(|l| labels.insert(&l.id)) {
        return Err(invalid("labels", "duplicate label id"));
    }
    if board.labels.iter().any(|l| l.name.trim().is_empty()) {
        return Err(invalid("labels", "label names must not be empty"));
    }
    let mut keys = HashSet::new();
    if !board
        .properties
        .iter()
        .all(|p| !p.key.trim().is_empty() && keys.insert(&p.key))
    {
        return Err(invalid("properties", "keys must be nonempty and unique"));
    }
    if board.properties.iter().any(|p| p.name.trim().is_empty()) {
        return Err(invalid("properties", "property names must not be empty"));
    }
    Ok(())
}

/// Checks card references, schema value types, and ISO calendar dates.
pub fn validate_card(board: &Board, card: &Card) -> Result<(), BoardError> {
    if card.board_id != board.id {
        return Err(invalid("board_id", "card belongs to another board"));
    }
    if card.title.trim().is_empty() {
        return Err(invalid("title", "must not be empty"));
    }
    if !board.statuses.iter().any(|s| s.id == card.status_id) {
        return Err(BoardError::UnknownStatus(card.status_id.to_string()));
    }
    for id in &card.labels {
        if !board.labels.iter().any(|l| l.id == *id) {
            return Err(BoardError::UnknownLabel(id.to_string()));
        }
    }
    for (key, value) in &card.properties {
        let Some(schema) = board.properties.iter().find(|p| p.key == *key) else {
            return Err(invalid("properties", &format!("unknown property {key}")));
        };
        if !value.matches_kind(schema.kind) {
            return Err(invalid("properties", &format!("wrong kind for {key}")));
        }
        // A `Date` property is a surface that offers a date, so it validates with the one
        // function every other one uses: `2026-02-30` is not a day a picker can ever re-pick.
        if let PropertyValue::Date(date) = value
            && !valid_date(date)
        {
            return Err(invalid(
                "properties",
                &format!("expected a valid YYYY-MM-DD date for {key}"),
            ));
        }
    }
    if card
        .due_date
        .as_deref()
        .is_some_and(|date| !valid_date(date))
    {
        return Err(invalid("due_date", "expected a valid YYYY-MM-DD date"));
    }
    Ok(())
}

/// Whether `date` is a real `YYYY-MM-DD` calendar date, leap years included.
///
/// Every surface that offers a due date validates it with this one function: a client that
/// accepted `2026-02-31` locally would only learn it is not a date from the daemon's refusal.
#[must_use]
pub fn valid_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        date[..4].parse::<u32>(),
        date[5..7].parse::<u32>(),
        date[8..].parse::<u32>(),
    ) else {
        return false;
    };
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= max
}

/// Applies a validated board patch atomically and reports whether anything changed.
///
/// A patch that leaves every field as it found it is not a mutation: stamping it would rewrite
/// the document, emit a change event and advance `updated_at` past a board nobody edited —
/// exactly what [`apply_card_patch`] and [`move_card`] refuse to do. The caller must not save,
/// stamp or announce a `false`.
///
/// The service must reject removed status/label ids still referenced by its cards;
/// this operation has no access to cards.
pub fn apply_board_patch(
    board: &mut Board,
    patch: BoardPatch,
    now: &str,
) -> Result<bool /* changed */, BoardError> {
    let mut next = board.clone();
    macro_rules! set {
        ($($field:ident),+ $(,)?) => { $(
            if let Some(value) = patch.$field { next.$field = value; }
        )+ };
    }
    set!(
        name,
        prefix,
        backend,
        statuses,
        labels,
        properties,
        default_repo_id,
        settings
    );
    validate_board(&next)?;
    if next == *board {
        return Ok(false);
    }
    next.updated_at = now.into();
    *board = next;
    Ok(true)
}
/// Creates a numbered card with the supplied stable identifier.
pub fn create_card(
    board: &mut Board,
    cards: &[Card],
    id: CardId,
    draft: CardDraft,
    now: &str,
) -> Result<Card, BoardError> {
    validate_board(board)?;
    if cards.iter().any(|card| card.id == id) {
        return Err(invalid("id", "card id must be unique"));
    }
    let next_number = board
        .next_number
        .checked_add(1)
        .ok_or_else(|| invalid("next_number", "card numbers exhausted"))?;
    let status_id = draft
        .status_id
        .or_else(|| {
            first_status_in(board, StatusCategory::Unstarted)
                .or_else(|| board.statuses.first())
                .map(|status| status.id.clone())
        })
        .ok_or_else(|| invalid("statuses", "at least one status is required"))?;
    let position = cards
        .iter()
        .filter(|card| card.board_id == board.id && !card.archived && card.status_id == status_id)
        .map(|card| card.position)
        .max()
        .map_or(Ok(0), |position| {
            position
                .checked_add(10)
                .ok_or_else(|| invalid("position", "column positions exhausted"))
        })?;
    for key in draft.properties.keys() {
        check_editable(board, key)?;
    }
    let mut card = Card {
        id,
        board_id: board.id.clone(),
        number: board.next_number,
        title: draft.title,
        description: draft.description,
        status_id,
        priority: draft.priority,
        labels: dedupe_labels(draft.labels),
        assignee: draft.assignee,
        estimate: draft.estimate,
        due_date: draft.due_date,
        parent_id: draft.parent_id,
        repo_id: draft.repo_id,
        worktree_id: None,
        // `Null` means "no value" on a patch; a draft must not be able to store it as one.
        properties: draft
            .properties
            .into_iter()
            .filter(|(_, value)| *value != PropertyValue::Null)
            .collect(),
        comments: Vec::new(),
        activity: Vec::new(),
        remote: None,
        conflict: None,
        dirty: !board.backend.is_local(),
        archived: false,
        position,
        created_at: now.into(),
        updated_at: now.into(),
    };
    validate_card(board, &card)?;
    push_activity(&mut card, ActivityKind::Created, None, "Created card", now);
    board.next_number = next_number;
    board.updated_at = now.into();
    Ok(card)
}
/// The same labels in the same order, keeping only the first mention of each.
fn dedupe_labels(labels: Vec<LabelId>) -> Vec<LabelId> {
    let mut seen = HashSet::new();
    labels
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// Refuses a property key whose schema the backend owns.
///
/// `editable == false` marks a value no local surface may write: the next `apply_remote`
/// discards it, and until then it can raise a `Manual` conflict on a field the user was never
/// offered. The app hides those rows; this is where the wire path meets the same rule.
fn check_editable(board: &Board, key: &str) -> Result<(), BoardError> {
    if board
        .properties
        .iter()
        .any(|schema| schema.key == key && !schema.editable)
    {
        return Err(invalid(
            "properties",
            &format!("property {key} is owned by the backend and is not editable"),
        ));
    }
    Ok(())
}

/// Refuses a draft that sets a standard field the board's backend cannot write back.
///
/// A create is a write like any other: `card new --priority urgent` on a Jira board is accepted
/// today, dropped by the backend's create payload, acknowledged as pushed, and then overwritten
/// by the next pull — three user-typed values gone without a word, while `card edit --priority`
/// refuses the same edit outright.
///
/// This is the *local* write path only, which is why it is not inside [`create_card`]:
/// `sync::reconcile` creates cards from what a pull reported, and a field the remote owns is
/// exactly the field it must be able to set there.
///
/// `parent_id` is absent on purpose: a backend's create can carry the hierarchy even where its
/// `edit` cannot, which is why the check is over what the draft sets rather than the whole list.
pub fn check_draft_writable(board: &Board, draft: &CardDraft) -> Result<(), BoardError> {
    for (field, set) in [
        ("status_id", draft.status_id.is_some()),
        ("priority", draft.priority != Priority::default()),
        ("assignee", draft.assignee.is_some()),
        ("estimate", draft.estimate.is_some()),
        ("due_date", draft.due_date.is_some()),
    ] {
        if set {
            check_writable(board, field)?;
        }
    }
    Ok(())
}

/// Refuses a standard card field the board's backend declared it cannot write back.
///
/// A local board declares nothing, so this is a no-op there. On a linked board the list comes
/// from `BackendSchema::readonly_fields` through `sync::adopt_schema`: writing such a field
/// would mark the card dirty forever, since no push can ever acknowledge it and the next pull
/// overwrites it — a silent edit that only ever produces a conflict.
fn check_writable(board: &Board, field: &str) -> Result<(), BoardError> {
    if !board.backend.is_local()
        && board
            .sync
            .readonly_fields
            .iter()
            .any(|readonly| readonly == field)
    {
        return Err(BoardError::ReadOnlyField(field.to_owned()));
    }
    Ok(())
}

/// Parses repeated `k=v` settings and merges them into a backend settings object.
///
/// Each value is parsed as JSON when it parses (`10`, `true`, `["a","b"]`, `null`) and kept as a
/// string otherwise, so `--setting project=SP` needs no quoting while `--setting
/// maxConcurrency=8` still arrives as a number. `base` must be an object or null; a `null` value
/// removes the key so a setting can be cleared. Pure: `base` is never mutated.
pub fn merge_settings(
    base: &serde_json::Value,
    pairs: &[(String, String)],
) -> Result<serde_json::Value, BoardError> {
    let mut object = match base {
        serde_json::Value::Null => serde_json::Map::new(),
        serde_json::Value::Object(map) => map.clone(),
        _ => {
            return Err(invalid(
                "settings",
                "backend settings must be a JSON object",
            ));
        }
    };
    for (key, value) in pairs {
        let key = key.trim();
        if key.is_empty() {
            return Err(invalid("setting", "key must not be empty"));
        }
        let parsed = serde_json::from_str::<serde_json::Value>(value.trim())
            .unwrap_or_else(|_| serde_json::Value::String(value.clone()));
        if parsed.is_null() {
            object.remove(key);
        } else {
            object.insert(key.to_owned(), parsed);
        }
    }
    Ok(serde_json::Value::Object(object))
}

/// Applies a patch and returns changed field names.
pub fn apply_card_patch(
    board: &Board,
    card: &mut Card,
    patch: CardPatch,
    now: &str,
) -> Result<Vec<String>, BoardError> {
    let mut patch = patch;
    // The CLI dedupes what it resolves; the protocol accepts whatever a client sends, and one
    // label listed twice would render twice on the card forever.
    patch.labels = patch.labels.map(dedupe_labels);
    let mut next = card.clone();
    let mut changed = Vec::new();
    macro_rules! set {
        ($($field:ident),+ $(,)?) => { $(
            if let Some(value) = patch.$field && next.$field != value {
                next.$field = value;
                changed.push(stringify!($field).to_owned());
            }
        )+ };
    }
    set!(
        title,
        description,
        status_id,
        priority,
        labels,
        assignee,
        estimate,
        due_date,
        parent_id,
        repo_id,
        archived
    );
    if let Some(properties) = patch.properties {
        for (key, value) in properties {
            check_editable(board, &key)?;
            if value == PropertyValue::Null {
                next.properties.remove(&key);
            } else {
                next.properties.insert(key, value);
            }
        }
        if next.properties != card.properties {
            changed.push("properties".into());
        }
    }
    for field in &changed {
        // The same exemption [`check_draft_writable`] makes, for the same reason: a backend's
        // *create* can carry the hierarchy even where its `edit` cannot, and a card with no
        // remote yet is a card whose first push is a create. Refusing it here meant a parent
        // could only ever be set in the one draft that created the card — the CLI and the app
        // both create cards without one — so a Jira board could never express a subtask, and
        // the create payload's parent branch was unreachable from every surface but the wire.
        if field == "parent_id" && card.remote.is_none() {
            continue;
        }
        check_writable(board, field)?;
    }
    validate_card(board, &next)?;
    // A patch that changes nothing is not a mutation: stamping it would rewrite the document,
    // emit a change event and advance `updated_at` past a card nobody edited.
    if changed.is_empty() {
        return Ok(changed);
    }
    next.updated_at = now.into();
    next.dirty = !board.backend.is_local();
    push_activity(
        &mut next,
        ActivityKind::Updated,
        None,
        format!("Updated {}", changed.join(", ")),
        now,
    );
    *card = next;
    Ok(changed)
}
/// Moves a card, renumbers the destination column's positions, and reports whether it moved.
///
/// A move that lands the card back on the status and position it already held changes nothing
/// and returns `false`: the caller must not write, stamp or announce it.
pub fn move_card(
    board: &Board,
    cards: &mut [Card],
    card_id: &CardId,
    status_id: &StatusId,
    index: Option<usize>,
    now: &str,
) -> Result<bool, BoardError> {
    if !board.statuses.iter().any(|status| status.id == *status_id) {
        return Err(BoardError::UnknownStatus(status_id.to_string()));
    }
    let moving = cards
        .iter()
        .position(|card| card.id == *card_id && card.board_id == board.id)
        .ok_or_else(|| BoardError::CardNotFound(card_id.to_string()))?;
    if cards[moving].archived {
        return Err(invalid("archived", "cannot move an archived card"));
    }
    let mut column: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(i, card)| {
            *i != moving
                && card.board_id == board.id
                && !card.archived
                && card.status_id == *status_id
        })
        .map(|(i, _)| i)
        .collect();
    column.sort_by_key(|&i| (cards[i].position, &cards[i].created_at, cards[i].number));
    // Index refers to the destination after removing the moving card; oversized indices append.
    column.insert(index.unwrap_or(column.len()).min(column.len()), moving);
    let previous = cards[moving].status_id.clone();
    let transitioned = previous != *status_id;
    if transitioned {
        check_writable(board, "status_id")?;
    }
    // Landing where it already was is not a mutation: stamping it would rewrite the document,
    // announce a change nobody made, and append an activity entry that pushes real history
    // out of the 200-entry cap.
    if !transitioned
        && column
            .iter()
            .enumerate()
            .all(|(rank, &i)| cards[i].position == rank as u64 * 10)
    {
        return Ok(false);
    }
    cards[moving].status_id = status_id.clone();
    for (position, i) in column.into_iter().enumerate() {
        let position = position as u64 * 10;
        if cards[i].position != position || i == moving {
            cards[i].position = position;
            cards[i].updated_at = now.into();
            // `position` is a local ordering no backend is told about, so only a real status
            // transition owes the remote a push; reordering never dirties a card.
            if transitioned && i == moving {
                cards[i].dirty = !board.backend.is_local();
            }
        }
    }
    let message = if transitioned {
        format!("Moved from {previous} to {status_id}")
    } else {
        format!("Reordered in {status_id}")
    };
    push_activity(&mut cards[moving], ActivityKind::Moved, None, message, now);
    Ok(true)
}
/// Adds a nonempty local comment and its activity record.
pub fn add_comment(
    card: &mut Card,
    id: String,
    author: Option<String>,
    body: String,
    now: &str,
) -> Result<(), BoardError> {
    if body.trim().is_empty() {
        return Err(invalid("body", "must not be empty"));
    }
    if id.trim().is_empty() || card.comments.iter().any(|c| c.id == id) {
        return Err(invalid("comment.id", "must be nonempty and unique"));
    }
    card.comments.push(Comment {
        id,
        author: author.clone(),
        body,
        created_at: now.into(),
        remote_id: None,
    });
    card.dirty |= card.remote.is_some();
    push_activity(
        card,
        ActivityKind::Commented,
        author,
        "Added a comment",
        now,
    );
    Ok(())
}
/// Appends activity, retaining the most recent 200 entries, and stamps the card.
pub fn push_activity(
    card: &mut Card,
    kind: ActivityKind,
    actor: Option<String>,
    message: impl Into<String>,
    now: &str,
) {
    card.activity.push(Activity {
        at: now.into(),
        kind,
        actor,
        message: message.into(),
    });
    if card.activity.len() > 200 {
        card.activity.drain(..card.activity.len() - 200);
    }
    card.updated_at = now.into();
}
/// Nonarchived cards in stable column order.
pub fn column_cards<'a>(cards: &'a [Card], status_id: &StatusId) -> Vec<&'a Card> {
    let mut column: Vec<_> = cards
        .iter()
        .filter(|c| !c.archived && c.status_id == *status_id)
        .collect();
    column.sort_by(|a, b| {
        (a.position, &a.created_at, a.number).cmp(&(b.position, &b.created_at, b.number))
    });
    column
}
/// First configured status in the requested category.
pub fn first_status_in(board: &Board, category: StatusCategory) -> Option<&Status> {
    board.statuses.iter().find(|s| s.category == category)
}
/// Renders the branch template as a nonempty slug of at most 48 ASCII characters.
///
/// The result also names a Git branch, so it never carries a `.`: a trailing period, a `..`
/// pair or a `.lock` component are all rejected by `validate_branch`, and an ordinary title
/// ("Fix the login bug.") must not be able to fail the whole card → worktree flow.
pub fn worktree_slug(board: &Board, card: &Card) -> String {
    let rendered = board
        .settings
        .branch_template
        .replace("{key}", &card.display_key(board))
        .replace("{slug}", &crate::slug::slugify(&card.title));
    let mut slug = branch_safe_slug(&rendered);
    if slug.is_empty() {
        slug = branch_safe_slug(&card.local_key(board));
    }
    if slug.is_empty() {
        slug = "card".into();
    }
    slug.truncate(48);
    let trimmed = slug.trim_matches(['-', '_']);
    if trimmed.is_empty() {
        "card".into()
    } else {
        trimmed.to_owned()
    }
}

/// Slugifies `value` and folds the dots a slug allows but a branch name cannot carry.
fn branch_safe_slug(value: &str) -> String {
    crate::slug::slugify(&crate::slug::slugify(value).replace('.', "-"))
}
/// Summarizes nonarchived cards for snapshots.
pub fn summarize(board: &Board, cards: &[Card]) -> BoardSummary {
    let cards: Vec<_> = cards.iter().filter(|c| !c.archived).collect();
    BoardSummary {
        id: board.id.clone(),
        context_id: board.context_id.clone(),
        name: board.name.clone(),
        prefix: board.prefix.clone(),
        backend_kind: board.backend.kind.clone(),
        card_count: cards.len(),
        open_count: cards
            .iter()
            .filter(|c| {
                board.statuses.iter().any(|s| {
                    s.id == c.status_id
                        && !matches!(
                            s.category,
                            StatusCategory::Completed | StatusCategory::Canceled
                        )
                })
            })
            .count(),
        dirty_count: cards.iter().filter(|c| c.dirty).count(),
        conflict_count: cards.iter().filter(|c| c.conflict.is_some()).count(),
        last_synced_at: board.sync.last_synced_at.clone(),
        last_error: board.sync.last_error.clone(),
    }
}
/// Serde default for enabled settings.
pub fn default_true() -> bool {
    true
}
/// Default worktree branch naming template.
pub fn default_branch_template() -> String {
    "{key}-{slug}".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{board::defaults::new_board, ids::ContextId, model::Context};

    const NOW: &str = "2026-09-06T12:00:00Z";
    const LATER: &str = "2026-09-06T13:00:00Z";

    fn board() -> Board {
        new_board(
            &Context {
                id: ContextId::try_from("work").unwrap(),
                name: "Fleet".into(),
                owners: vec![],
                created_at: NOW.into(),
            },
            NOW,
        )
    }

    fn create(board: &mut Board, cards: &[Card], id: &str) -> Card {
        create_card(
            board,
            cards,
            id.parse().unwrap(),
            CardDraft {
                title: "Fix login".into(),
                ..CardDraft::default()
            },
            NOW,
        )
        .unwrap()
    }

    fn property(key: &str) -> PropertySchema {
        PropertySchema {
            key: key.into(),
            name: key.into(),
            kind: PropertyKind::Number,
            options: vec![],
            editable: true,
            source: PropertySource::Local,
            show_on_card: false,
        }
    }

    #[test]
    fn a_label_listed_twice_is_stored_once() {
        let mut board = board();
        board.labels = vec![Label {
            id: "bug".parse().unwrap(),
            name: "Bug".into(),
            color: None,
        }];
        let bug: LabelId = "bug".parse().unwrap();
        // The CLI dedupes what it resolves; the protocol takes whatever a client sends, and a
        // label repeated in a draft would render twice on the card forever.
        let mut card = create_card(
            &mut board,
            &[],
            "a".parse().unwrap(),
            CardDraft {
                title: "Fix login".into(),
                labels: vec![bug.clone(), bug.clone()],
                ..CardDraft::default()
            },
            NOW,
        )
        .unwrap();
        assert_eq!(card.labels, std::slice::from_ref(&bug));
        card.labels.clear();
        apply_card_patch(
            &board,
            &mut card,
            CardPatch {
                labels: Some(vec![bug.clone(), bug.clone()]),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert_eq!(card.labels, [bug]);
    }

    #[test]
    fn create_numbers_from_counter_and_records_creation() {
        let mut board = board();
        board.next_number = 42;
        let card = create(&mut board, &[], "a");
        assert_eq!((card.number, board.next_number, card.position), (42, 43, 0));
        assert_eq!(card.status_id.as_str(), "todo");
        assert_eq!(card.activity.len(), 1);
        assert_eq!(card.activity[0].kind, ActivityKind::Created);
        assert_eq!(card.created_at, NOW);
        assert!(!card.dirty);
    }

    #[test]
    fn create_uses_first_status_without_unstarted() {
        let mut board = board();
        board
            .statuses
            .retain(|status| status.category != StatusCategory::Unstarted);
        assert_eq!(create(&mut board, &[], "a").status_id.as_str(), "backlog");
    }

    #[test]
    fn create_honors_explicit_status_and_all_draft_fields() {
        let mut board = board();
        board.backend.kind = "fake".into();
        board.properties.push(property("score"));
        board.labels.push(Label {
            id: "bug".parse().unwrap(),
            name: "Bug".into(),
            color: None,
        });
        let draft = CardDraft {
            title: "Title".into(),
            description: "Details".into(),
            status_id: Some("done".parse().unwrap()),
            priority: Priority::High,
            labels: vec!["bug".parse().unwrap()],
            assignee: Some("A".into()),
            estimate: Some(3),
            due_date: Some("2028-02-29".into()),
            parent_id: Some("parent".parse().unwrap()),
            repo_id: Some("org/repo".parse().unwrap()),
            properties: BTreeMap::from([("score".into(), PropertyValue::Number(2.0))]),
        };
        let card = create_card(&mut board, &[], "a".parse().unwrap(), draft.clone(), NOW).unwrap();
        assert_eq!(card.title, draft.title);
        assert_eq!(card.description, draft.description);
        assert_eq!(Some(card.status_id), draft.status_id);
        assert_eq!(card.priority, draft.priority);
        assert_eq!(card.labels, draft.labels);
        assert_eq!(card.assignee, draft.assignee);
        assert_eq!(card.estimate, draft.estimate);
        assert_eq!(card.due_date, draft.due_date);
        assert_eq!(card.parent_id, draft.parent_id);
        assert_eq!(card.repo_id, draft.repo_id);
        assert_eq!(card.properties, draft.properties);
        assert!(card.dirty);
    }

    #[test]
    fn create_appends_after_maximum_live_position() {
        let mut board = board();
        let mut first = create(&mut board, &[], "a");
        first.position = 77;
        let mut archived = first.clone();
        archived.id = "archived".parse().unwrap();
        archived.position = 900;
        archived.archived = true;
        let mut other = first.clone();
        other.id = "other".parse().unwrap();
        other.status_id = "done".parse().unwrap();
        other.position = 1000;
        assert_eq!(
            create(&mut board, &[other, first, archived], "b").position,
            87
        );
    }

    #[test]
    fn invalid_create_is_atomic() {
        for draft in [
            CardDraft::default(),
            CardDraft {
                title: "  \n".into(),
                ..CardDraft::default()
            },
            CardDraft {
                title: "x".into(),
                status_id: Some("missing".parse().unwrap()),
                ..CardDraft::default()
            },
            CardDraft {
                title: "x".into(),
                labels: vec!["missing".parse().unwrap()],
                ..CardDraft::default()
            },
            CardDraft {
                title: "x".into(),
                due_date: Some("2025-02-29".into()),
                ..CardDraft::default()
            },
        ] {
            let mut board = board();
            let before = board.clone();
            assert!(create_card(&mut board, &[], "a".parse().unwrap(), draft, LATER).is_err());
            assert_eq!(board, before);
        }
    }

    #[test]
    fn duplicate_card_ids_and_exhausted_counters_fail() {
        let mut board = board();
        let card = create(&mut board, &[], "a");
        let draft = CardDraft {
            title: "x".into(),
            ..CardDraft::default()
        };
        assert!(
            create_card(
                &mut board,
                &[card],
                "a".parse().unwrap(),
                draft.clone(),
                NOW
            )
            .is_err()
        );
        board.next_number = u64::MAX;
        assert!(create_card(&mut board, &[], "b".parse().unwrap(), draft, NOW).is_err());
        assert_eq!(board.next_number, u64::MAX);
    }

    #[test]
    fn exhausted_column_position_does_not_consume_number() {
        let mut board = board();
        let mut card = create(&mut board, &[], "a");
        card.position = u64::MAX;
        let before = board.clone();
        assert!(
            create_card(
                &mut board,
                &[card],
                "b".parse().unwrap(),
                CardDraft {
                    title: "x".into(),
                    ..CardDraft::default()
                },
                LATER
            )
            .is_err()
        );
        assert_eq!(board, before);
    }

    #[test]
    fn patch_reports_actual_fields_and_compact_activity() {
        let mut board = board();
        let mut card = create(&mut board, &[], "a");
        let changed = apply_card_patch(
            &board,
            &mut card,
            CardPatch {
                title: Some("New".into()),
                description: Some("Details".into()),
                priority: Some(Priority::High),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert_eq!(changed, ["title", "description", "priority"]);
        assert_eq!(card.updated_at, LATER);
        assert!(!card.dirty);
        assert_eq!(
            card.activity.last().unwrap().message,
            "Updated title, description, priority"
        );
    }

    #[test]
    fn patch_dirties_only_remote_board_and_leaves_noops_alone() {
        let mut board = board();
        let mut card = create(&mut board, &[], "a");
        board.backend.kind = "fake".into();
        assert!(
            apply_card_patch(&board, &mut card, CardPatch::default(), LATER)
                .unwrap()
                .is_empty()
        );
        // A patch that changes nothing is not a mutation: it neither stamps nor dirties.
        assert_eq!(card.updated_at, NOW);
        assert!(!card.dirty);
        assert_eq!(card.activity.len(), 1);
        let repeat = CardPatch {
            title: Some(card.title.clone()),
            ..CardPatch::default()
        };
        assert!(
            apply_card_patch(&board, &mut card, repeat, LATER)
                .unwrap()
                .is_empty()
        );
        assert_eq!(card.updated_at, NOW);
        assert!(!card.dirty);
        apply_card_patch(
            &board,
            &mut card,
            CardPatch {
                title: Some("Edited".into()),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert!(card.dirty);
        board.backend = BackendRef::default();
        apply_card_patch(
            &board,
            &mut card,
            CardPatch {
                title: Some("Edited again".into()),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert!(!card.dirty);
    }

    #[test]
    fn patch_properties_merge_remove_and_preserve_unmentioned() {
        let mut board = board();
        board
            .properties
            .extend([property("a"), property("b"), property("c")]);
        let mut card = create(&mut board, &[], "a");
        card.properties.extend([
            ("a".into(), PropertyValue::Number(1.0)),
            ("b".into(), PropertyValue::Number(2.0)),
        ]);
        let changed = apply_card_patch(
            &board,
            &mut card,
            CardPatch {
                properties: Some(BTreeMap::from([
                    ("a".into(), PropertyValue::Null),
                    ("c".into(), PropertyValue::Number(3.0)),
                ])),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert_eq!(changed, ["properties"]);
        assert_eq!(
            card.properties,
            BTreeMap::from([
                ("b".into(), PropertyValue::Number(2.0)),
                ("c".into(), PropertyValue::Number(3.0))
            ])
        );
    }

    #[test]
    fn invalid_patch_rolls_back_all_fields() {
        let mut board = board();
        board.properties.push(property("score"));
        let mut card = create(&mut board, &[], "a");
        for patch in [
            CardPatch {
                title: Some(" ".into()),
                ..CardPatch::default()
            },
            CardPatch {
                title: Some("New".into()),
                status_id: Some("missing".parse().unwrap()),
                ..CardPatch::default()
            },
            CardPatch {
                title: Some("New".into()),
                properties: Some(BTreeMap::from([(
                    "score".into(),
                    PropertyValue::Text("bad".into()),
                )])),
                ..CardPatch::default()
            },
            CardPatch {
                properties: Some(BTreeMap::from([(
                    "unknown".into(),
                    PropertyValue::Bool(true),
                )])),
                ..CardPatch::default()
            },
        ] {
            let before = card.clone();
            assert!(apply_card_patch(&board, &mut card, patch, LATER).is_err());
            assert_eq!(card, before);
        }
    }

    #[test]
    fn patch_clears_nullable_fields_and_can_archive() {
        let mut board = board();
        let mut card = create(&mut board, &[], "a");
        card.assignee = Some("A".into());
        card.estimate = Some(3);
        card.due_date = Some("2026-09-07".into());
        card.parent_id = Some("parent".parse().unwrap());
        card.repo_id = Some("org/repo".parse().unwrap());
        let fields = apply_card_patch(
            &board,
            &mut card,
            CardPatch {
                assignee: Some(None),
                estimate: Some(None),
                due_date: Some(None),
                parent_id: Some(None),
                repo_id: Some(None),
                archived: Some(true),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert_eq!(
            fields,
            [
                "assignee",
                "estimate",
                "due_date",
                "parent_id",
                "repo_id",
                "archived"
            ]
        );
        assert!(card.assignee.is_none() && card.estimate.is_none() && card.due_date.is_none());
        assert!(card.parent_id.is_none() && card.repo_id.is_none() && card.archived);
    }

    #[test]
    fn a_board_patch_that_changes_nothing_stamps_nothing() {
        let mut board = board();
        assert!(
            !apply_board_patch(&mut board, BoardPatch::default(), LATER).unwrap(),
            "an empty patch changes nothing"
        );
        assert_eq!(board.updated_at, NOW);
        let repeat = BoardPatch {
            name: Some(board.name.clone()),
            prefix: Some(board.prefix.clone()),
            settings: Some(board.settings.clone()),
            ..BoardPatch::default()
        };
        assert!(
            !apply_board_patch(&mut board, repeat, LATER).unwrap(),
            "a patch that restates the board changes nothing"
        );
        assert_eq!(
            board.updated_at, NOW,
            "stamping a no-op rewrites the document and announces a change nobody made"
        );
        assert!(
            apply_board_patch(
                &mut board,
                BoardPatch {
                    name: Some("Renamed".into()),
                    ..BoardPatch::default()
                },
                LATER
            )
            .unwrap()
        );
        assert_eq!(board.updated_at, LATER);
    }

    #[test]
    fn blank_display_names_are_refused_everywhere_a_name_is_shown() {
        let base = board();
        let mut blank_status = base.clone();
        blank_status.statuses[0].name = "  ".into();
        assert!(validate_board(&blank_status).is_err());

        let mut blank_label = base.clone();
        blank_label.labels.push(Label {
            id: "bug".parse().unwrap(),
            name: String::new(),
            color: None,
        });
        assert!(validate_board(&blank_label).is_err());

        let mut blank_property = base;
        let mut schema = property("eta");
        schema.name = String::new();
        blank_property.properties.push(schema);
        assert!(validate_board(&blank_property).is_err());
    }

    #[test]
    fn a_date_property_takes_only_a_real_calendar_day() {
        let mut board = board();
        let mut schema = property("eta");
        schema.kind = PropertyKind::Date;
        board.properties.push(schema);
        let mut card = create(&mut board, &[], "a");
        for value in ["not-a-date", "2026-02-30", "2026-13-01"] {
            let patch = CardPatch {
                properties: Some(
                    [("eta".to_owned(), PropertyValue::Date(value.into()))]
                        .into_iter()
                        .collect(),
                ),
                ..CardPatch::default()
            };
            assert!(
                apply_card_patch(&board, &mut card.clone(), patch, LATER).is_err(),
                "`{value}` is not a day any picker could offer back"
            );
        }
        let patch = CardPatch {
            properties: Some(
                [("eta".to_owned(), PropertyValue::Date("2028-02-29".into()))]
                    .into_iter()
                    .collect(),
            ),
            ..CardPatch::default()
        };
        assert!(apply_card_patch(&board, &mut card, patch, LATER).is_ok());
    }

    #[test]
    fn a_backend_owned_property_is_not_writable_by_a_patch_or_a_draft() {
        let mut board = board();
        let mut schema = property("points");
        schema.editable = false;
        schema.source = PropertySource::Backend;
        board.properties.push(schema);
        let properties: BTreeMap<_, _> = [("points".to_owned(), PropertyValue::Number(3.0))]
            .into_iter()
            .collect();
        let mut card = create(&mut board, &[], "a");
        let patch = CardPatch {
            properties: Some(properties.clone()),
            ..CardPatch::default()
        };
        assert!(
            apply_card_patch(&board, &mut card, patch, LATER).is_err(),
            "the next `apply_remote` discards it, and until then it raises a conflict on a \
             field no surface offers"
        );
        assert!(
            create_card(
                &mut board,
                &[card],
                "b".parse().unwrap(),
                CardDraft {
                    title: "Fix login".into(),
                    properties,
                    ..CardDraft::default()
                },
                LATER
            )
            .is_err()
        );
    }

    #[test]
    fn board_patch_validates_prefix_atomically() {
        for prefix in ["", "lower", "ABCDEFGHI", "AB-1", "É"] {
            let mut board = board();
            let before = board.clone();
            assert!(
                apply_board_patch(
                    &mut board,
                    BoardPatch {
                        name: Some("New".into()),
                        prefix: Some(prefix.into()),
                        ..BoardPatch::default()
                    },
                    LATER
                )
                .is_err()
            );
            assert_eq!(board, before);
        }
        let mut board = board();
        apply_board_patch(
            &mut board,
            BoardPatch {
                prefix: Some("ABC12345".into()),
                ..BoardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert_eq!(board.prefix, "ABC12345");
        assert_eq!(board.updated_at, LATER);
    }

    #[test]
    fn board_patch_rejects_empty_and_duplicate_schema_ids() {
        let mut board = board();
        let label = Label {
            id: "bug".parse().unwrap(),
            name: "Bug".into(),
            color: None,
        };
        for patch in [
            BoardPatch {
                statuses: Some(vec![]),
                ..BoardPatch::default()
            },
            BoardPatch {
                statuses: Some(vec![board.statuses[0].clone(); 2]),
                ..BoardPatch::default()
            },
            BoardPatch {
                labels: Some(vec![label; 2]),
                ..BoardPatch::default()
            },
            BoardPatch {
                properties: Some(vec![property("a"); 2]),
                ..BoardPatch::default()
            },
        ] {
            let before = board.clone();
            assert!(apply_board_patch(&mut board, patch, LATER).is_err());
            assert_eq!(board, before);
        }
    }

    #[test]
    fn board_patch_allows_status_removal_for_service_to_check() {
        let mut board = board();
        let statuses = vec![board.statuses[0].clone()];
        apply_board_patch(
            &mut board,
            BoardPatch {
                statuses: Some(statuses.clone()),
                ..BoardPatch::default()
            },
            LATER,
        )
        .unwrap();
        assert_eq!(board.statuses, statuses);
    }

    #[test]
    fn move_same_column_uses_index_after_removal() {
        let mut board = board();
        let a = create(&mut board, &[], "a");
        let b = create(&mut board, std::slice::from_ref(&a), "b");
        let c = create(&mut board, &[a.clone(), b.clone()], "c");
        let mut cards = vec![a, b, c];
        move_card(
            &board,
            &mut cards,
            &"a".parse().unwrap(),
            &"todo".parse().unwrap(),
            Some(1),
            LATER,
        )
        .unwrap();
        let column = column_cards(&cards, &"todo".parse().unwrap());
        assert_eq!(
            column
                .iter()
                .map(|card| (card.id.as_str(), card.position))
                .collect::<Vec<_>>(),
            [("b", 0), ("a", 10), ("c", 20)]
        );
        assert_eq!(cards[0].activity.last().unwrap().kind, ActivityKind::Moved);
        assert!(!cards[0].dirty);
    }

    #[test]
    fn a_move_that_changes_nothing_reports_it_and_writes_no_history() {
        let mut board = board();
        let a = create(&mut board, &[], "a");
        let b = create(&mut board, std::slice::from_ref(&a), "b");
        let mut cards = vec![a, b];
        let before = cards.clone();
        for _ in 0..3 {
            assert!(
                !move_card(
                    &board,
                    &mut cards,
                    &"a".parse().unwrap(),
                    &"todo".parse().unwrap(),
                    Some(0),
                    LATER,
                )
                .unwrap()
            );
        }
        // Six identical moves used to append six activity entries and restamp the card.
        assert_eq!(cards, before);
        // A move that does reorder still reports and records one.
        assert!(
            move_card(
                &board,
                &mut cards,
                &"a".parse().unwrap(),
                &"todo".parse().unwrap(),
                Some(1),
                LATER,
            )
            .unwrap()
        );
        assert_eq!(cards[0].activity.len(), before[0].activity.len() + 1);
    }

    #[test]
    fn move_cross_column_renumbers_target_and_marks_changes() {
        let mut board = board();
        board.backend.kind = "fake".into();
        let mut a = create(&mut board, &[], "a");
        let mut b = create(&mut board, &[], "b");
        a.dirty = false;
        b.dirty = false;
        b.status_id = "done".parse().unwrap();
        b.position = 45;
        let mut cards = vec![a, b];
        move_card(
            &board,
            &mut cards,
            &"a".parse().unwrap(),
            &"done".parse().unwrap(),
            Some(0),
            LATER,
        )
        .unwrap();
        assert_eq!((cards[0].position, cards[1].position), (0, 10));
        assert!(cards.iter().all(|card| card.updated_at == LATER));
        // Only the card that changed status owes the backend a push; `position` is local.
        assert!(cards[0].dirty);
        assert!(!cards[1].dirty);
    }

    #[test]
    fn same_column_reorder_never_dirties_a_card_and_reads_as_a_reorder() {
        let mut board = board();
        board.backend.kind = "fake".into();
        let mut a = create(&mut board, &[], "a");
        let mut b = create(&mut board, &[], "b");
        a.dirty = false;
        b.dirty = false;
        b.position = 10;
        let mut cards = vec![a, b];
        move_card(
            &board,
            &mut cards,
            &"b".parse().unwrap(),
            &"todo".parse().unwrap(),
            Some(0),
            LATER,
        )
        .unwrap();
        assert!(cards.iter().all(|card| !card.dirty));
        assert_eq!(
            cards[1].activity.last().unwrap().message,
            "Reordered in todo"
        );
    }

    #[test]
    fn move_none_and_oversized_indices_append_excluding_archived() {
        for index in [None, Some(999)] {
            let mut board = board();
            let a = create(&mut board, &[], "a");
            let b = create(&mut board, &[], "b");
            let mut archived = create(&mut board, &[], "archived");
            archived.archived = true;
            archived.position = 42;
            let mut cards = vec![a, b, archived.clone()];
            move_card(
                &board,
                &mut cards,
                &"a".parse().unwrap(),
                &"todo".parse().unwrap(),
                index,
                LATER,
            )
            .unwrap();
            assert_eq!((cards[0].position, cards[1].position), (10, 0));
            assert_eq!(cards[2], archived);
        }
    }

    #[test]
    fn invalid_move_preserves_cards() {
        let mut board = board();
        let mut cards = vec![create(&mut board, &[], "a")];
        let before = cards.clone();
        assert!(matches!(
            move_card(
                &board,
                &mut cards,
                &"a".parse().unwrap(),
                &"bad".parse().unwrap(),
                None,
                LATER
            ),
            Err(BoardError::UnknownStatus(_))
        ));
        assert!(matches!(
            move_card(
                &board,
                &mut cards,
                &"bad".parse().unwrap(),
                &"todo".parse().unwrap(),
                None,
                LATER
            ),
            Err(BoardError::CardNotFound(_))
        ));
        assert_eq!(cards, before);
    }

    #[test]
    fn worktree_slug_uses_crate_slugify_and_unicode_is_safe() {
        let mut board = board();
        let mut card = create(&mut board, &[], "a");
        card.title = "Fix_API.v2 / 中文 héllo".into();
        assert_eq!(worktree_slug(&board, &card), "fle-1-fix_api-v2-h-llo");
        board.settings.branch_template = "Feature/{slug}/{key}".into();
        assert_eq!(
            worktree_slug(&board, &card),
            "feature-fix_api-v2-h-llo-fle-1"
        );
        board.settings.branch_template = "{slug}".into();
        card.title = "中".repeat(80);
        assert_eq!(worktree_slug(&board, &card), "fle-1");
    }

    #[test]
    fn worktree_slug_truncates_and_trims_dash_at_boundary() {
        let mut board = board();
        let mut card = create(&mut board, &[], "a");
        board.settings.branch_template = "{slug}".into();
        card.title = format!("{} tail", "x".repeat(47));
        assert_eq!(worktree_slug(&board, &card), "x".repeat(47));
        card.title = "x".repeat(100);
        assert_eq!(worktree_slug(&board, &card).len(), 48);
    }

    #[test]
    fn worktree_slug_is_always_a_valid_branch_name() {
        let mut board = board();
        let mut card = create(&mut board, &[], "a");
        for title in [
            "Fix the login bug.",
            "release v1.lock",
            "a..b regression",
            "...",
            "____",
            ".hidden",
            "Ship it!!!",
        ] {
            card.title = title.into();
            for template in ["{key}-{slug}", "{slug}", "feature/{slug}"] {
                board.settings.branch_template = template.into();
                let slug = worktree_slug(&board, &card);
                assert!(!slug.is_empty(), "{title} / {template}");
                crate::validate::validate_branch(&slug)
                    .unwrap_or_else(|error| panic!("{title} / {template}: {error}"));
                crate::validate::validate_slug(&slug)
                    .unwrap_or_else(|error| panic!("{title} / {template}: {error}"));
            }
        }
    }

    #[test]
    fn summary_excludes_archived_and_both_terminal_categories() {
        let mut board = board();
        let mut cards: Vec<_> = (0..5)
            .map(|i| create(&mut board, &[], &format!("c{i}")))
            .collect();
        cards[1].status_id = "in-progress".parse().unwrap();
        cards[2].status_id = "done".parse().unwrap();
        cards[3].status_id = "canceled".parse().unwrap();
        cards[4].archived = true;
        cards[4].dirty = true;
        cards[0].dirty = true;
        cards[0].conflict = Some(Conflict {
            detected_at: NOW.into(),
            remote: Default::default(),
            fields: vec![],
        });
        let summary = summarize(&board, &cards);
        assert_eq!(
            (
                summary.card_count,
                summary.open_count,
                summary.dirty_count,
                summary.conflict_count
            ),
            (4, 2, 1, 1)
        );
    }

    /// A board whose backend refuses a field refuses it here, and nowhere else.
    #[test]
    fn a_read_only_field_is_refused_on_a_linked_board_only() {
        let mut local = board();
        let card = create(&mut local, &[], "a");
        // The list only means anything next to a backend: a local board declares none, and one
        // left over from a board that was unlinked must not freeze its own cards.
        local.sync.readonly_fields = vec!["priority".into(), "status_id".into()];
        assert_eq!(
            apply_card_patch(
                &local,
                &mut card.clone(),
                CardPatch {
                    priority: Some(Priority::Urgent),
                    ..CardPatch::default()
                },
                LATER,
            )
            .unwrap(),
            vec!["priority".to_owned()]
        );

        let mut board = local.clone();
        board.backend = BackendRef {
            kind: "jira".into(),
            settings: serde_json::json!({"project": "SP"}),
        };
        let mut linked = card.clone();
        let error = apply_card_patch(
            &board,
            &mut linked,
            CardPatch {
                priority: Some(Priority::Urgent),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap_err();
        assert!(matches!(&error, BoardError::ReadOnlyField(field) if field == "priority"));
        assert_eq!(
            error.to_string(),
            "priority is read-only on this board's backend"
        );
        // The refusal is total: the card the caller handed in is untouched.
        assert_eq!(linked, card);

        // A field the backend does write goes through, and dirties the card for the push.
        assert_eq!(
            apply_card_patch(
                &board,
                &mut linked,
                CardPatch {
                    title: Some("Fix logout".into()),
                    ..CardPatch::default()
                },
                LATER,
            )
            .unwrap(),
            vec!["title".to_owned()]
        );
        assert!(linked.dirty);

        // A patch that names the field but changes nothing is not an edit, and is not refused.
        let same = linked.priority;
        assert!(
            apply_card_patch(
                &board,
                &mut linked,
                CardPatch {
                    priority: Some(same),
                    ..CardPatch::default()
                },
                LATER,
            )
            .unwrap()
            .is_empty()
        );
    }

    /// The parent of a card the backend has never seen is the create's to carry.
    ///
    /// `check_draft_writable` leaves `parent_id` out for exactly that reason, and the patch had
    /// to make the same exemption or the field could only ever be set in the one draft that
    /// created the card — which no surface offers — leaving a Jira board unable to express a
    /// subtask at all.
    #[test]
    fn a_read_only_parent_is_still_settable_until_the_card_is_linked() {
        let mut board = board();
        board.backend = BackendRef {
            kind: "jira".into(),
            settings: serde_json::json!({"project": "SP"}),
        };
        board.sync.readonly_fields = vec!["parent_id".into()];
        let parent = create(&mut board, &[], "parent");
        let mut card = create(&mut board, std::slice::from_ref(&parent), "a");
        let patch = || CardPatch {
            parent_id: Some(Some(parent.id.clone())),
            ..CardPatch::default()
        };
        assert_eq!(
            apply_card_patch(&board, &mut card, patch(), LATER).unwrap(),
            vec!["parent_id".to_owned()]
        );
        assert_eq!(card.parent_id, Some(parent.id.clone()));

        // Once the card is linked the backend owns it, and the same edit is refused.
        let mut linked = create(&mut board, std::slice::from_ref(&parent), "b");
        linked.remote = Some(RemoteLink {
            backend: "jira".into(),
            key: "SP-9".into(),
            url: None,
            version: None,
            remote_updated_at: None,
            parent_key: None,
            synced_at: NOW.into(),
        });
        let before = linked.clone();
        assert!(matches!(
            apply_card_patch(&board, &mut linked, patch(), LATER),
            Err(BoardError::ReadOnlyField(field)) if field == "parent_id"
        ));
        assert_eq!(linked, before);
    }

    /// `status_id` is read-only through both doors: the patch and the move.
    #[test]
    fn a_read_only_status_refuses_a_move_but_still_reorders() {
        let mut board = board();
        let mut cards = vec![create(&mut board, &[], "a")];
        cards.push(create(&mut board, &cards, "b"));
        board.backend = BackendRef {
            kind: "jira".into(),
            settings: serde_json::Value::Null,
        };
        board.sync.readonly_fields = vec!["status_id".into()];
        let done: StatusId = "done".parse().unwrap();
        let id: CardId = "a".parse().unwrap();
        assert!(matches!(
            move_card(&board, &mut cards, &id, &done, None, LATER),
            Err(BoardError::ReadOnlyField(field)) if field == "status_id"
        ));
        assert_eq!(cards[0].status_id.as_str(), "todo");
        // Position is local ordering no backend is told about, so reordering stays allowed.
        let todo: StatusId = "todo".parse().unwrap();
        assert!(move_card(&board, &mut cards, &id, &todo, Some(1), LATER).unwrap());
        assert_eq!(cards[0].position, 10);
        assert!(!cards[0].dirty);
    }

    #[test]
    fn settings_pairs_merge_as_json_when_they_parse_and_as_text_otherwise() {
        let base = serde_json::json!({"project": "OLD", "jql": "sprint in openSprints()"});
        let merged = merge_settings(
            &base,
            &[
                ("project".into(), "SP".into()),
                ("maxConcurrency".into(), "8".into()),
                ("statuses".into(), r#"["To Do","Done"]"#.into()),
                ("fullSync".into(), "true".into()),
                ("jql".into(), "null".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            merged,
            serde_json::json!({
                "project": "SP",
                "maxConcurrency": 8,
                "statuses": ["To Do", "Done"],
                "fullSync": true
            })
        );
        // `base` is untouched, and a null base starts from an empty object.
        assert_eq!(base["project"], serde_json::json!("OLD"));
        assert_eq!(
            merge_settings(&serde_json::Value::Null, &[("project".into(), "SP".into())]).unwrap(),
            serde_json::json!({"project": "SP"})
        );
        // A value that is not JSON is the string the user typed, quoting and all.
        assert_eq!(
            merge_settings(
                &serde_json::Value::Null,
                &[("jql".into(), "assignee = currentUser()".into())]
            )
            .unwrap(),
            serde_json::json!({"jql": "assignee = currentUser()"})
        );
        assert!(matches!(
            merge_settings(&serde_json::Value::Null, &[(" ".into(), "x".into())]),
            Err(BoardError::Invalid { .. })
        ));
        assert!(matches!(
            merge_settings(&serde_json::json!([1]), &[]),
            Err(BoardError::Invalid { .. })
        ));
    }
}
