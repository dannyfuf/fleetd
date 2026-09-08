use super::*;

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
