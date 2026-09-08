use super::*;

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
pub(super) fn dedupe_labels(labels: Vec<LabelId>) -> Vec<LabelId> {
    let mut seen = HashSet::new();
    labels
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
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
