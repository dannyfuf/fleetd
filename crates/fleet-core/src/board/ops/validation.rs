use super::*;

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

/// Refuses a property key whose schema the backend owns.
///
/// `editable == false` marks a value no local surface may write: the next `apply_remote`
/// discards it, and until then it can raise a `Manual` conflict on a field the user was never
/// offered. The app hides those rows; this is where the wire path meets the same rule.
pub(super) fn check_editable(board: &Board, key: &str) -> Result<(), BoardError> {
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
pub(super) fn check_writable(board: &Board, field: &str) -> Result<(), BoardError> {
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
