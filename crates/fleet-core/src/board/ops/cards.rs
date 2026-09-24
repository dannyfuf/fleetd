use super::*;

/// Creates a card for a pull request the board does not hold yet, returns the card that holds
/// it otherwise, and reopens a completed or archived card when the review is requested again.
///
/// The `usize` is the index of the affected card in `cards`. A card is reopened only when
/// `requested_at` is later than its last completion (the newest `Moved` or `AutoMoved`
/// activity, else `updated_at`); a card in a `Canceled` column is never reopened, because
/// dismissing a review is a decision.
pub fn upsert_pull_request_card(
    board: &mut Board,
    cards: &mut Vec<Card>,
    id: CardId,
    draft: CardDraft,
    requested_at: Option<&str>,
    now: &str,
) -> Result<(UpsertOutcome, usize), BoardError> {
    let Some(pull_request) = draft.pull_request.as_ref() else {
        return Err(invalid(
            "pull_request",
            "a pull request card needs a pull request",
        ));
    };
    pull_request.validate()?;
    let requested_at = requested_at.map(parse_requested_at).transpose()?;
    let existing = cards.iter().position(|card| {
        card.board_id == board.id
            && card
                .pull_request
                .as_ref()
                .is_some_and(|held| held.same_pull_request(pull_request))
    });
    let Some(index) = existing else {
        let card = create_card(board, cards, id, draft, now)?;
        cards.push(card);
        return Ok((UpsertOutcome::Created, cards.len() - 1));
    };
    let card = &cards[index];
    let category = board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .map(|status| status.category);
    let reopenable = category != Some(StatusCategory::Canceled)
        && (category == Some(StatusCategory::Completed) || card.archived);
    let requested_later = requested_at.is_some_and(|requested| {
        // A completion time this build cannot read is treated as older than any request: the
        // reopen stamps a fresh one, so the next request compares correctly again.
        last_completion(card).is_none_or(|completed| requested > completed)
    });
    if !reopenable || !requested_later {
        return Ok((UpsertOutcome::Existing, index));
    }
    reopen(board, cards, index, draft.status_id, now)?;
    Ok((UpsertOutcome::Reopened, index))
}

/// Reads a pull request's `requested_at`, refusing anything that is not RFC 3339 in the words
/// every surface uses, so the CLI can refuse it before a board is touched.
///
/// # Errors
/// [`BoardError::Invalid`] on `requested_at`.
pub fn parse_requested_at(at: &str) -> Result<chrono::DateTime<chrono::FixedOffset>, BoardError> {
    chrono::DateTime::parse_from_rfc3339(at)
        .map_err(|_| invalid("requested_at", "must be an RFC 3339 time"))
}

/// When the card last reached where it is: the newest move, else its last update.
fn last_completion(card: &Card) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let at = card
        .activity
        .iter()
        .rev()
        .find(|entry| matches!(entry.kind, ActivityKind::Moved | ActivityKind::AutoMoved))
        .map_or(card.updated_at.as_str(), |entry| entry.at.as_str());
    chrono::DateTime::parse_from_rfc3339(at).ok()
}

/// Unarchives the card and moves it to `status_id`, or else the first `Unstarted` column.
///
/// Every refusal is checked before the card changes, so an error leaves `cards` untouched.
fn reopen(
    board: &Board,
    cards: &mut [Card],
    index: usize,
    status_id: Option<StatusId>,
    now: &str,
) -> Result<(), BoardError> {
    let target = status_id
        .or_else(|| first_status_in(board, StatusCategory::Unstarted).map(|s| s.id.clone()))
        .ok_or_else(|| {
            invalid(
                "statuses",
                "an unstarted status is required to reopen a card",
            )
        })?;
    if !board.statuses.iter().any(|status| status.id == target) {
        return Err(BoardError::UnknownStatus(target.to_string()));
    }
    let card = &mut cards[index];
    let (was_archived, was_dirty) = (card.archived, card.dirty);
    if was_archived {
        check_writable(board, "archived")?;
    }
    if card.status_id != target {
        check_writable(board, "status_id")?;
    }
    let card_id = card.id.clone();
    card.archived = false;
    if was_archived {
        card.dirty = !board.backend.is_local();
    }
    if let Err(error) = move_card(board, cards, &card_id, &target, None, now) {
        cards[index].archived = was_archived;
        cards[index].dirty = was_dirty;
        return Err(error);
    }
    push_activity(
        &mut cards[index],
        ActivityKind::Updated,
        None,
        "Review re-requested",
        now,
    );
    Ok(())
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
        pull_request: draft.pull_request,
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
        agent: draft.agent,
        blocked_by: draft.blocked_by,
        pending_run: None,
        runs: Vec::new(),
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
        run_id: None,
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
