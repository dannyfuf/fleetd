use super::*;

/// Whether the last thing that archived this card was a pull, rather than the user.
///
/// The activity trail is the only record of who archived a card; it is read newest-first, and a
/// card whose trail no longer reaches the archive (200 entries) keeps it, because leaving a
/// card archived is the recoverable half of the two mistakes.
pub(super) fn archived_by_sync(card: &Card) -> bool {
    card.activity
        .iter()
        .rev()
        .find_map(|entry| match entry.message.as_str() {
            ARCHIVED_BY_SYNC => Some(true),
            message if message.contains("archived") => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

/// Apply acks: link cards (`remote`), clear `dirty`, mark comment remote ids, record failures in activity.
pub fn apply_push_result(
    cards: &mut [Card],
    result: &PushResult,
    backend: &str,
    unpushed: &[CardId],
    now: &str,
) {
    for ack in &result.acks {
        let Some(card) = cards.iter_mut().find(|card| card.id == ack.card_id) else {
            continue;
        };
        card.remote = Some(RemoteLink {
            parent_key: card
                .remote
                .as_ref()
                .and_then(|link| link.parent_key.clone()),
            backend: backend.into(),
            key: ack.key.clone(),
            url: ack.url.clone(),
            version: ack.version.clone(),
            synced_at: now.into(),
            // The ack's own stamp when the backend read one back; otherwise the stored one,
            // because dropping it would leave the card with no `remoteUpdatedAt` until some
            // later pull happened to change `version`.
            remote_updated_at: ack.remote_updated_at.clone().or_else(|| {
                card.remote
                    .as_ref()
                    .and_then(|link| link.remote_updated_at.clone())
            }),
        });
        for (local_id, remote_id) in &ack.comment_ids {
            if let Some(comment) = card
                .comments
                .iter_mut()
                .find(|comment| comment.id == *local_id)
            {
                comment.remote_id = Some(remote_id.clone());
            }
        }
        card.dirty = unpushed.contains(&card.id)
            || card
                .comments
                .iter()
                .any(|comment| comment.remote_id.is_none());
        push_activity(
            card,
            ActivityKind::Synced,
            None,
            "Pushed local changes",
            now,
        );
    }
    // The link of an acked card has to carry the parent the push just filed, and only the
    // finished ack set can name it: a create mints the parent's key in the same batch. Left at
    // the previous link's value — `None` for a card this push created — the next reconcile
    // derives `parent_id` from it and unparents a card the backend does hold, in a field
    // `READONLY_FIELDS` then refuses to let anyone put back.
    let keys: BTreeMap<CardId, String> = cards
        .iter()
        .filter_map(|card| {
            card.remote
                .as_ref()
                .map(|link| (card.id.clone(), link.key.clone()))
        })
        .collect();
    for ack in &result.acks {
        let Some(card) = cards.iter_mut().find(|card| card.id == ack.card_id) else {
            continue;
        };
        let parent_key = card.parent_id.as_ref().and_then(|id| keys.get(id)).cloned();
        if let Some(link) = card.remote.as_mut() {
            link.parent_key = parent_key;
        }
    }
    for failure in &result.failures {
        if let Some(card) = cards.iter_mut().find(|card| card.id == failure.card_id) {
            // A card may have both successful and failed operations in one batch.
            card.dirty = backend != BackendRef::LOCAL;
            push_activity(
                card,
                ActivityKind::Synced,
                None,
                format!("Push failed: {}", failure.error),
                now,
            );
        }
    }
}

/// Resolves a card conflict by explicitly selecting local or remote data.
/// For `TakeRemote`, the service must first materialize any missing board labels
/// (using `apply_remote` on a scratch card) and then resolve the remote parent key
/// against its card set. Missing labels produce an atomic validation error here.
pub fn resolve_conflict(
    board: &Board,
    card: &mut Card,
    resolution: ConflictResolution,
    now: &str,
) -> Result<(), BoardError> {
    let conflict = card.conflict.clone().ok_or_else(|| BoardError::Invalid {
        field: "conflict".into(),
        reason: "card has no conflict to resolve".into(),
    })?;
    let mut next = card.clone();
    match resolution {
        ConflictResolution::KeepLocal => {
            // "Keep local" is a choice over the fields the user can hold. The ones the remote
            // owns outright are not among them, and `differing_fields` already drops them from
            // the conflict for that reason — so leaving them behind here froze the local copy
            // at a value no push can carry and no later pull ever revisits: the link this
            // resolution stamps carries the remote's own version, so `remote_changed` answers
            // "no" from then on and the divergence is permanent and silent.
            adopt_unownable(board, &mut next, &conflict.remote);
            merge_comments(&mut next, &conflict.remote);
            refresh_link(&mut next, &board.backend.kind, &conflict.remote, now);
            next.dirty = !board.backend.is_local();
        }
        ConflictResolution::TakeRemote => {
            // The public contract borrows the board immutably. The service must first
            // materialize missing labels with apply_remote on a scratch card.
            let mut schema = board.clone();
            apply_remote(&mut schema, &mut next, &conflict.remote, now);
            validate_card(board, &next)?;
        }
    }
    next.conflict = None;
    push_activity(
        &mut next,
        ActivityKind::ConflictResolved,
        None,
        match resolution {
            ConflictResolution::KeepLocal => "Kept local changes",
            ConflictResolution::TakeRemote => "Accepted remote changes",
        },
        now,
    );
    *card = next;
    Ok(())
}

/// Copies the sides of a conflict's remote that no local edit is allowed to own.
///
/// The set is exactly `board.sync.readonly_fields` plus the backend's own non-editable
/// properties: `ops::check_writable` and `ops::check_editable` refuse every local write to
/// them, so a difference there is never a side the user chose to keep.
///
/// `labels` is deliberately unhandled: adopting one would have to materialize a `Label` on the
/// board, which this contract borrows immutably. `parent_id` is adopted only when the remote
/// names no parent — resolving a remote key into a `CardId` needs the whole card set, which
/// `reconcile` has and this path does not.
fn adopt_unownable(board: &Board, card: &mut Card, remote: &RemoteCard) {
    let mut scratch = board.clone();
    let mut projected = card.clone();
    apply_remote(&mut scratch, &mut projected, remote, &card.updated_at);
    for field in &board.sync.readonly_fields {
        match field.as_str() {
            "title" => card.title.clone_from(&projected.title),
            "description" => card.description.clone_from(&projected.description),
            "status_id" => card.status_id.clone_from(&projected.status_id),
            "priority" => card.priority = projected.priority,
            "assignee" => card.assignee.clone_from(&projected.assignee),
            "estimate" => card.estimate = projected.estimate,
            "due_date" => card.due_date.clone_from(&projected.due_date),
            "parent_id" if remote.parent_key.is_none() => card.parent_id = None,
            _ => {}
        }
    }
    for schema in board
        .properties
        .iter()
        .filter(|schema| schema.source == PropertySource::Backend && !schema.editable)
    {
        match projected.properties.get(&schema.key) {
            Some(value) => {
                card.properties.insert(schema.key.clone(), value.clone());
            }
            None => {
                card.properties.remove(&schema.key);
            }
        }
    }
}

/// Remote → local field mapping used by reconcile and TakeRemote (labels by name → create missing Label on board).
/// A missing parent key clears the parent; a present key must be resolved by the
/// caller with its complete card set. Until then, the existing parent is retained.
///
/// Returns whether any domain field changed. Link freshness (`remote.synced_at`) and
/// `updated_at` are not domain fields: a backend that reports neither a version nor a
/// remote timestamp is replayed on every sync, and stamping those would rewrite the card
/// and push one "Synced" entry per sync until the 200-entry cap evicted its real history.
pub fn apply_remote(board: &mut Board, card: &mut Card, remote: &RemoteCard, now: &str) -> bool {
    let before = card.clone();
    card.title = remote.title.clone();
    card.description = remote.description.clone();
    let mapped = mapped_status(board, &remote.status);
    if let Some(status) = &mapped {
        card.status_id = status.clone();
    }
    card.priority = remote.priority.unwrap_or_default();
    // A blank label name is a label no chip and no picker row can ever show, and
    // `validate_board` refuses one: importing it would wedge the next board edit.
    card.labels = remote
        .labels
        .iter()
        .filter(|name| !name.trim().is_empty())
        .map(|name| ensure_label(board, name))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    card.assignee = remote.assignee.clone();
    card.estimate = remote.estimate;
    card.due_date = remote.due_date.clone();
    // Resolving a remote parent key requires the complete card set (handled by reconcile).
    // A missing parent explicitly clears it; otherwise preserve it until the service maps it.
    if remote.parent_key.is_none() {
        card.parent_id = None;
    }
    // Local schemas own their values even when the backend omits or collides with them.
    card.properties.retain(|key, _| {
        board
            .properties
            .iter()
            .any(|schema| schema.key == *key && schema.source == PropertySource::Local)
    });
    // A key the backend never declared has no schema to validate against, so storing it would
    // make `validate_card` reject the document and wedge every later sync of this board.
    card.properties.extend(
        remote
            .properties
            .iter()
            .filter(|(key, value)| {
                // `Null` is how every local path spells "no value" — `create_card` filters it
                // out of a draft and `apply_card_patch` removes the key — so storing a remote
                // one as a present entry would render a value no local path can produce. A
                // `Date` that is not a calendar day is refused by `validate_card`, and storing
                // it would skip the card on every later pull.
                **value != PropertyValue::Null
                    && !matches!(value, PropertyValue::Date(date) if !valid_date(date))
                    && board.properties.iter().any(|schema| {
                        schema.key == **key
                            && schema.source != PropertySource::Local
                            && value.matches_kind(schema.kind)
                    })
            })
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    merge_comments(card, remote);
    refresh_link(card, &board.backend.kind, remote, now);
    card.conflict = None;
    card.dirty = false;
    board.updated_at = now.into();
    // Replaying the same pull must not duplicate history, whatever the clock says.
    if !changed_beyond_link(&before, card) {
        return false;
    }
    card.updated_at = now.into();
    let message = if mapped.is_none() {
        format!(
            "Synced {}; unmapped status {} (kept local status)",
            remote.key,
            remote_status_key(&remote.status)
        )
    } else {
        format!("Synced {}", remote.key)
    };
    push_activity(card, ActivityKind::Synced, None, message, now);
    true
}

/// Whether anything but the link's freshness stamp and `updated_at` differs.
fn changed_beyond_link(before: &Card, after: &Card) -> bool {
    let mut probe = after.clone();
    probe.updated_at.clone_from(&before.updated_at);
    if let (Some(link), Some(previous)) = (probe.remote.as_mut(), before.remote.as_ref()) {
        link.synced_at.clone_from(&previous.synced_at);
    }
    probe != *before
}

pub(super) fn merge_comments(card: &mut Card, remote: &RemoteCard) {
    for remote_comment in &remote.comments {
        if let Some(comment) = card
            .comments
            .iter_mut()
            .find(|comment| comment.remote_id.as_ref() == Some(&remote_comment.id))
        {
            comment.author = remote_comment.author.clone();
            comment.body = remote_comment.body.clone();
            comment.created_at = remote_comment.created_at.clone();
        } else {
            let base = format!("remote:{}", remote_comment.id);
            let mut id = base.clone();
            let mut suffix = 2;
            while card.comments.iter().any(|comment| comment.id == id) {
                id = format!("{base}:{suffix}");
                suffix += 1;
            }
            card.comments.push(Comment {
                id,
                author: remote_comment.author.clone(),
                body: remote_comment.body.clone(),
                created_at: remote_comment.created_at.clone(),
                remote_id: Some(remote_comment.id.clone()),
            });
        }
    }
}

pub(super) fn refresh_link(card: &mut Card, backend: &str, remote: &RemoteCard, now: &str) {
    card.remote = Some(RemoteLink {
        parent_key: remote.parent_key.clone(),
        backend: backend.into(),
        key: remote.key.clone(),
        url: remote.url.clone(),
        version: remote.version.clone(),
        synced_at: now.into(),
        remote_updated_at: remote.updated_at.clone(),
    });
}

/// Whether two links differ in anything but the local `synced_at` stamp.
pub(super) fn link_changed(previous: Option<&RemoteLink>, next: Option<&RemoteLink>) -> bool {
    match (previous, next) {
        (Some(previous), Some(next)) => {
            let mut probe = next.clone();
            probe.synced_at.clone_from(&previous.synced_at);
            probe != *previous
        }
        (None, None) => false,
        _ => true,
    }
}

pub(super) fn remote_changed(link: &RemoteLink, remote: &RemoteCard) -> bool {
    if let (Some(previous), Some(current)) = (&link.version, &remote.version) {
        previous != current
    } else if let (Some(previous), Some(current)) = (&link.remote_updated_at, &remote.updated_at) {
        previous != current
    } else {
        true
    }
}
