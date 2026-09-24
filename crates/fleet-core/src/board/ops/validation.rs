use super::*;

/// Environment-variable names a column may never set on its runs.
///
/// The same prefix `fleet subagent run --env` refuses: `FLEET_*` is the delegation's own
/// identity, the daemon writes those variables itself, and a column that set one would only be
/// overwritten without a word.
const RESERVED_ENV_PREFIX: &str = "FLEET_";

/// Checks the board's prefix, its unique schema identifiers and its column automation.
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
    validate_automation(board)?;
    Ok(())
}

/// Checks every column's automation block and the board's live-run throttle.
///
/// Routing may only ever point forward. A column that sent a card back would let one run's
/// success start the run of a column the card had already passed, and the pair would trade the
/// card between them for as long as the runs kept succeeding — a loop no refusal further down
/// can break, because every individual move in it is legal.
pub fn validate_automation(board: &Board) -> Result<(), BoardError> {
    for (index, status) in board.statuses.iter().enumerate() {
        let Some(automation) = status.automation.as_ref() else {
            continue;
        };
        for (field, target) in [
            ("on_success", automation.on_success.as_ref()),
            (
                "advance_when_unblocked",
                automation.advance_when_unblocked.as_ref(),
            ),
        ] {
            let Some(target) = target else { continue };
            // Every sentence names the column that carries the route. A routing column is
            // written once and validated on every later board patch, so the refusal a person
            // reads is usually raised by an edit somewhere else — removing the column this one
            // routes to, or reordering the two — and a reason that named only the field would
            // leave them looking for a mistake in what they just typed.
            let name = status.name.as_str();
            let Some(position) = board.statuses.iter().position(|s| s.id == *target) else {
                return Err(invalid(
                    field,
                    &format!("{name} routes to {target}, which is not a column on this board"),
                ));
            };
            if position == index {
                return Err(invalid(field, &format!("{name} may not route to itself")));
            }
            if position < index {
                return Err(invalid(
                    field,
                    &format!("{name} routes to {target}, which is not a later column"),
                ));
            }
        }
        if let Some(action) = automation.on_enter.as_ref() {
            if let ActionKind::Skill { name, .. } = &action.kind {
                if name.trim().is_empty() {
                    return Err(invalid("on_enter", "a skill action needs a name"));
                }
                // Skills are a Claude concept: Codex has no verb that takes one, so a column
                // asking for both is asking for something no provider can serve.
                if action.agent.provider == Some(AgentKind::Codex) {
                    return Err(invalid(
                        "on_enter",
                        "skill actions run on claude only; put the invocation in the column's instructions for codex",
                    ));
                }
            }
            validate_env(&action.env)?;
        }
    }
    if board
        .settings
        .max_live_runs
        .is_some_and(|runs| !(1..=MAX_LIVE_RUNS_PER_BOARD).contains(&runs))
    {
        return Err(invalid(
            "max_live_runs",
            &format!("must be between 1 and {MAX_LIVE_RUNS_PER_BOARD}"),
        ));
    }
    Ok(())
}

/// Checks the `KEY=VALUE` entries a column hands its runs.
///
/// These are the five rules `fleet subagent run --env` already applies, restated here without
/// the flag name: a column's env reaches a child through the daemon rather than through the
/// CLI, and a board settings dialog must be able to refuse the same pair with the same words.
pub fn validate_env(env: &[String]) -> Result<(), BoardError> {
    let mut seen = HashSet::new();
    for pair in env {
        let Some((key, _)) = pair.split_once('=') else {
            return Err(invalid(
                "env",
                &format!("{pair} is not KEY=VALUE: every entry needs an `=`"),
            ));
        };
        if key.is_empty() {
            return Err(invalid("env", &format!("{pair} has an empty key")));
        }
        if key.starts_with(RESERVED_ENV_PREFIX) {
            return Err(invalid(
                "env",
                &format!(
                    "{key} is refused: {RESERVED_ENV_PREFIX}* names are the delegation's own identity and the daemon sets them itself"
                ),
            ));
        }
        if key == "PATH" {
            return Err(invalid(
                "env",
                "PATH is refused: an entry replaces the value outright rather than extending the login shell's, and Fleet already prepends the directory holding this fleet so the child can run `fleet subagent complete`",
            ));
        }
        if !seen.insert(key) {
            return Err(invalid("env", &format!("{key} is given twice")));
        }
    }
    Ok(())
}

/// Checks one card's blockers against the board's card set.
///
/// Pure, and separate from [`validate_card`] because it needs every card rather than one: the
/// daemon calls it beside its parent check on the write paths that can set links.
pub fn validate_links(board: &Board, cards: &[Card], card: &Card) -> Result<(), BoardError> {
    for blocker in &card.blocked_by {
        if *blocker == card.id {
            return Err(invalid("blocked_by", "a card cannot block itself"));
        }
        if !cards.iter().any(|other| other.id == *blocker) {
            return Err(invalid(
                "blocked_by",
                &format!("{blocker} is not on this board"),
            ));
        }
    }
    let mut path = vec![card.display_key(board)];
    let mut seen = HashSet::new();
    if let Some(cycle) = closing_cycle(board, cards, card, &card.id, &mut path, &mut seen) {
        return Err(invalid(
            "blocked_by",
            &format!("would close a cycle: {}", cycle.join(" → ")),
        ));
    }
    Ok(())
}

/// Refuses two cards on one board that hold the same pull request, archived cards included.
///
/// Pure, and separate from [`validate_card`] because it needs every card: the daemon's store
/// runs it on every save, so two racing upserts can never persist a duplicate.
pub fn validate_pull_requests(board: &Board, cards: &[Card]) -> Result<(), BoardError> {
    let mut holders: std::collections::HashMap<String, &Card> = std::collections::HashMap::new();
    for card in cards {
        let Some(pull_request) = &card.pull_request else {
            continue;
        };
        // Keyed without case, for the reason `PullRequestRef::same_pull_request` gives.
        let identity = pull_request.key().to_ascii_lowercase();
        if let Some(holder) = holders.get(&identity) {
            return Err(invalid(
                "pull_request",
                &format!(
                    "{} is already on this board as {}",
                    pull_request.key(),
                    holder.display_key(board)
                ),
            ));
        }
        holders.insert(identity, card);
    }
    Ok(())
}

/// The display-key path from `card` back to `target`, when following blockers reaches it.
///
/// `seen` keeps a diamond from being walked twice and makes a cycle that does not involve
/// `target` — one a broken document could already hold — terminate instead of recursing forever.
fn closing_cycle(
    board: &Board,
    cards: &[Card],
    card: &Card,
    target: &CardId,
    path: &mut Vec<String>,
    seen: &mut HashSet<CardId>,
) -> Option<Vec<String>> {
    for blocker in &card.blocked_by {
        if blocker == target {
            let mut closed = path.clone();
            closed.push(path[0].clone());
            return Some(closed);
        }
        if !seen.insert(blocker.clone()) {
            continue;
        }
        let Some(next) = cards.iter().find(|other| other.id == *blocker) else {
            continue;
        };
        path.push(next.display_key(board));
        if let Some(closed) = closing_cycle(board, cards, next, target, path, seen) {
            return Some(closed);
        }
        path.pop();
    }
    None
}

/// Checks card references, schema value types, and ISO calendar dates.
pub fn validate_card(board: &Board, card: &Card) -> Result<(), BoardError> {
    if card.board_id != board.id {
        return Err(invalid("board_id", "card belongs to another board"));
    }
    if card.title.trim().is_empty() {
        return Err(invalid("title", "must not be empty"));
    }
    if let Some(pull_request) = &card.pull_request {
        pull_request.validate()?;
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
