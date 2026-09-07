use super::*;

/// Match `pull.cards` to local cards by `remote.key`; create/update/delete; detect conflicts
/// (local `dirty` and remote changed since `remote.remote_updated_at`/`version`) and resolve by policy;
/// emit `to_push` for dirty non-conflicted linked cards (Update/Transition/AddComment) and, when
/// `settings.push_new_cards`, `Create` for unlinked cards. Remote comments merge by remote id under
/// every policy, `LocalWins` included, because the advanced baseline is never offered again.
/// A card's `parent_id` follows its `remote.parent_key` only while it has a link: an unlinked card
/// keeps the local parent no pull knows about. Under `Manual`, a dirty card whose fields all match
/// the remote converges instead of recording a conflict with nothing to choose.
/// Pure and idempotent.
#[must_use]
pub fn reconcile(
    board: &Board,
    cards: &[Card],
    pull: &PullResult,
    caps: BackendCapabilities,
    now: &str,
) -> Reconciled {
    let mut result = Reconciled {
        board: board.clone(),
        cards: cards.to_vec(),
        to_push: Vec::new(),
        unpushed: Vec::new(),
        summary: SyncSummary {
            pulled: pull.cards.len(),
            ..SyncSummary::default()
        },
    };
    result.board.sync.last_error = None;
    let remotes: BTreeMap<_, _> = pull
        .cards
        .iter()
        .map(|remote| (remote.key.as_str(), remote))
        .collect();
    let deleted: BTreeSet<_> = pull.deleted_keys.iter().map(String::as_str).collect();
    // `failed_keys` carries "KEY: reason"; only the key half decides what stays.
    let failed: BTreeSet<&str> = pull
        .failed_keys
        .iter()
        .map(|failure| failure.split(':').next().unwrap_or(failure).trim())
        .collect();
    let mut linked: BTreeMap<_, _> = result
        .cards
        .iter()
        .enumerate()
        .filter(|(_, card)| card.board_id == board.id)
        .filter_map(|(index, card)| card.remote.as_ref().map(|link| (link.key.clone(), index)))
        .collect();
    let mut created = BTreeSet::new();
    // Allocate all identities first so children may precede parents in a pull.
    for (&key, remote) in &remotes {
        if linked.contains_key(key) || deleted.contains(key) {
            continue;
        }
        let id = remote_card_id(&result.board, key, &result.cards);
        let draft = CardDraft {
            title: remote.title.clone(),
            status_id: mapped_status(&result.board, &remote.status),
            ..CardDraft::default()
        };
        let next_number = result.board.next_number;
        match create_card(&mut result.board, &result.cards, id, draft, now) {
            Ok(mut card) => {
                // The draft carries the title and status; every other remote field arrives in
                // the update pass below. One this board cannot hold must not import half a
                // card that no later pull can complete — and that would take the number of a
                // card the next pull creates all over again.
                let mut board = result.board.clone();
                let mut scratch = card.clone();
                apply_remote(&mut board, &mut scratch, remote, now);
                if let Err(error) = validate_card(&board, &scratch) {
                    result.summary.skipped.push(format!("{key}: {error}"));
                    result.board.next_number = next_number;
                    continue;
                }
                card.activity.clear();
                let index = result.cards.len();
                linked.insert(key.to_owned(), index);
                created.insert(index);
                result.cards.push(card);
                result.summary.created += 1;
            }
            // One unimportable remote issue must not discard the cards that did import:
            // the caller persists what reconciled and reports the rest as skipped.
            Err(error) => result.summary.skipped.push(format!("{key}: {error}")),
        }
    }
    let parents: BTreeMap<_, _> = linked
        .iter()
        .map(|(key, &index)| (key.clone(), result.cards[index].id.clone()))
        .collect();
    for (&key, remote) in &remotes {
        let Some(&index) = linked.get(key) else {
            continue;
        };
        if deleted.contains(key) {
            continue;
        }
        if mapped_status(&result.board, &remote.status).is_none() {
            let status = remote_status_key(&remote.status);
            if !result.summary.unmapped_statuses.contains(&status) {
                result.summary.unmapped_statuses.push(status);
            }
        }
        let card = &mut result.cards[index];
        merge_comments(card, remote);
        let changed = card
            .remote
            .as_ref()
            .is_none_or(|link| remote_changed(link, remote));
        let parent = remote
            .parent_key
            .as_ref()
            .and_then(|key| parents.get(key))
            .cloned();
        // An unsent comment only keeps a card dirty when the backend can ever take it; without
        // `caps.comments` no push op will ever be planned, so the flag would never clear.
        if card.dirty
            && card.conflict.is_none()
            && differing_fields(&result.board, card, remote, Some(parent.clone())).is_empty()
            && (!caps.comments
                || card
                    .comments
                    .iter()
                    .all(|comment| comment.remote_id.is_some()))
        {
            card.dirty = false;
        }
        // The push pass skips archived cards, so a dirty archived card can never converge:
        // recording a conflict on one hides it from `summarize` and from `summary.conflicts`
        // (both exclude archived cards) until an un-archive pops it out. An archived card is
        // out of the push loop, so the remote is simply the truth for it.
        if created.contains(&index) || (changed && (!card.dirty || card.archived)) {
            if let Some(mut applied) = apply_remote_checked(
                &mut result.board,
                card,
                remote,
                now,
                &mut result.summary.skipped,
            ) {
                applied |= card.parent_id != parent;
                card.parent_id = parent;
                if !created.contains(&index) && applied {
                    result.summary.updated += 1;
                }
            }
        } else if changed && card.dirty && !card.archived {
            match result.board.settings.conflict_policy {
                ConflictPolicy::Manual => {
                    let fields = differing_fields(&result.board, card, remote, Some(parent));
                    if fields.is_empty() {
                        // Nothing differs; the card is dirty only for a comment the backend has
                        // not taken yet. Asking the user to choose between two identical cards
                        // would leave a conflict no resolution can clear and a comment
                        // `plan_push` never reaches, because it skips conflicted cards.
                        card.conflict = None;
                        refresh_link(card, &result.board.backend.kind, remote, now);
                    } else {
                        let already_recorded = card.conflict.as_ref().is_some_and(|conflict| {
                            conflict.remote == **remote && conflict.fields == fields
                        });
                        if !already_recorded {
                            card.conflict = Some(Conflict {
                                detected_at: now.into(),
                                remote: (*remote).clone(),
                                fields,
                            });
                            push_activity(
                                card,
                                ActivityKind::ConflictDetected,
                                None,
                                "Local and remote changes conflict",
                                now,
                            );
                        }
                    }
                }
                ConflictPolicy::RemoteWins => {
                    if let Some(mut applied) = apply_remote_checked(
                        &mut result.board,
                        card,
                        remote,
                        now,
                        &mut result.summary.skipped,
                    ) {
                        applied |= card.parent_id != parent;
                        card.parent_id = parent;
                        if applied {
                            result.summary.updated += 1;
                        }
                    }
                }
                ConflictPolicy::LocalWins => {
                    let previous_link = card.remote.clone();
                    let had_conflict = card.conflict.take().is_some();
                    refresh_link(card, &result.board.backend.kind, remote, now);
                    // `refresh_link` always rewrites `synced_at`, so comparing the whole link
                    // would report a change on every replay: a backend that reports neither a
                    // version nor a timestamp would then gain one entry per sync until the
                    // 200-entry cap evicted the card's real history.
                    if had_conflict || link_changed(previous_link.as_ref(), card.remote.as_ref()) {
                        push_activity(
                            card,
                            ActivityKind::ConflictResolved,
                            None,
                            "Kept local changes by board policy",
                            now,
                        );
                    }
                }
            }
        }
    }
    // A full pull that brought back nothing whatsoever — no card, no deletion — while the board
    // still holds linked cards is not a project that emptied itself: it is a filter that matched
    // nothing (a label typo, a sprint between sprints, a project key that moved). Archiving on it
    // takes the whole board out in one sync, clears the `dirty` flag of every card whose queued
    // push never went out, and reports the run as a success. A real emptying survives the next
    // full pull, which will carry the deletions the backend actually knows about.
    let empty_full_pull = pull.cards.is_empty()
        && pull.deleted_keys.is_empty()
        && result
            .cards
            .iter()
            .any(|card| card.board_id == board.id && card.remote.is_some() && !card.archived);
    for card in &mut result.cards {
        if card.board_id != board.id {
            continue;
        }
        // The remote owns the hierarchy of the cards it knows about, and only those: a card
        // with no link has a purely local parent that no pull has anything to say about.
        if let Some(link) = card.remote.as_mut()
            && !card.dirty
            && card.conflict.is_none()
        {
            // A pull that carried this issue owns its hierarchy even when nothing else about
            // it changed: the stored link may hold a `parent_key` an ack wrote from the
            // previous one, which is exactly what the answer must not be derived from.
            if let Some(remote) = remotes.get(link.key.as_str()) {
                link.parent_key.clone_from(&remote.parent_key);
            }
            let parent = link
                .parent_key
                .as_ref()
                .and_then(|key| parents.get(key))
                .cloned();
            card.parent_id = parent;
        }
        let removed = card.remote.as_ref().is_some_and(|link| {
            deleted.contains(link.key.as_str())
                || (pull.full
                    && !empty_full_pull
                    && !remotes.contains_key(link.key.as_str())
                    // A key this pull could not read is not a key the pull says is gone.
                    && !failed.contains(link.key.as_str()))
        });
        if removed && !card.archived {
            card.archived = true;
            card.dirty = false;
            card.conflict = None;
            push_activity(card, ActivityKind::Synced, None, ARCHIVED_BY_SYNC, now);
            result.summary.deleted += 1;
        }
        // The other direction, which nothing else does: a card this sync archived because the
        // remote was missing has to come back when the remote does. A key can go missing for a
        // reason that is not a deletion (a permission blip, an unreadable field, a filter that
        // moved), and without this the only way back is `card edit --archive false` by UUID.
        // Only an archive this loop made is undone: one the user made is theirs.
        if !removed
            && card.archived
            && !card.dirty
            && card.conflict.is_none()
            && archived_by_sync(card)
            && card
                .remote
                .as_ref()
                .is_some_and(|link| remotes.contains_key(link.key.as_str()))
        {
            card.archived = false;
            push_activity(card, ActivityKind::Synced, None, RESTORED_BY_SYNC, now);
            result.summary.updated += 1;
        }
        if card.archived || card.conflict.is_some() {
            continue;
        }
        let remote = card
            .remote
            .as_ref()
            .and_then(|link| remotes.get(link.key.as_str()).copied());
        let parent = remote.map(|remote| {
            remote
                .parent_key
                .as_ref()
                .and_then(|key| parents.get(key))
                .cloned()
        });
        if card.dirty
            && let Some(remote) = remote
        {
            let fields = differing_fields(&result.board, card, remote, parent.clone());
            let unmapped_status = fields.iter().any(|field| field == "status_id")
                && !result
                    .board
                    .sync
                    .status_map
                    .local_to_remote
                    .contains_key(&card.status_id);
            if fields.iter().any(|field| match field.as_str() {
                "status_id" => !caps.transitions || unmapped_status,
                "properties" => !caps.push_updates || !caps.custom_properties,
                _ => !caps.push_updates,
            }) {
                result.unpushed.push(card.id.clone());
            }
            // A column no remote status is mapped to swallows the move: `plan_push` plans no
            // transition, the card stays dirty forever and nothing says why. The sync names
            // the column, in the same list a remote status nobody could map lands in.
            if unmapped_status && caps.transitions {
                let column = result
                    .board
                    .statuses
                    .iter()
                    .find(|status| status.id == card.status_id)
                    .map(|status| status.name.clone());
                if let Some(column) = column {
                    result.summary.unmapped_statuses.push(column);
                }
            }
        }
        // A card the backend would take, held back by a board flag: the count is what lets the
        // sync say so, instead of reporting `0 pushed` about a card that was never offered.
        if card.remote.is_none()
            && !result.board.backend.is_local()
            && caps.push_create
            && !result.board.settings.push_new_cards
        {
            result.summary.kept_local += 1;
        }
        plan_push(
            &result.board,
            card,
            remote,
            parent,
            caps,
            &mut result.to_push,
        );
    }
    // The remote owns the hierarchy of the cards it knows about, but not the invariant: a
    // `parent_key` naming its own card — or closing a longer loop — is dropped here exactly
    // as `resolve_conflict` drops one, because the store would persist a cycle no walk ends.
    break_parent_cycles(&mut result.cards);
    result.summary.conflicts = result
        .cards
        .iter()
        .filter(|card| card.board_id == board.id && !card.archived && card.conflict.is_some())
        .count();
    // No push has executed yet. The service counts successful acknowledgements.
    result.board.sync.last_synced_at = Some(now.into());
    if pull.cursor.is_some() || pull.full {
        result.board.sync.cursor = pull.cursor.clone();
    }
    result.board.updated_at = now.into();
    result
}

/// Applies a remote card onto clones and commits them only if the result is a card this board
/// can hold; a refused remote reports `"{key}: {reason}"` and leaves the local card untouched.
///
/// `create_card` already routes an unimportable remote *draft* into `summary.skipped`; an
/// unimportable remote *update* had nothing guarding it, so a blank title or an impossible due
/// date reached the store, which validates the whole document and rejects the pull that carried
/// it — discarding every other card that reconciled in the same batch.
///
/// Returns `None` when the remote was refused, and otherwise whether a domain field changed.
fn apply_remote_checked(
    board: &mut Board,
    card: &mut Card,
    remote: &RemoteCard,
    now: &str,
    skipped: &mut Vec<String>,
) -> Option<bool> {
    let mut next_board = board.clone();
    let mut next = card.clone();
    let applied = apply_remote(&mut next_board, &mut next, remote, now);
    if let Err(error) = validate_card(&next_board, &next) {
        skipped.push(format!("{}: {error}", remote.key));
        return None;
    }
    *board = next_board;
    *card = next;
    Some(applied)
}

/// Clears the parent of every card that can reach itself through `parent_id`.
fn break_parent_cycles(cards: &mut [Card]) {
    let parents: BTreeMap<_, _> = cards
        .iter()
        .filter_map(|card| {
            card.parent_id
                .clone()
                .map(|parent| (card.id.clone(), parent))
        })
        .collect();
    let cyclic: BTreeSet<_> = cards
        .iter()
        .filter(|card| {
            let mut seen = BTreeSet::new();
            let mut current = card.parent_id.clone();
            while let Some(id) = current {
                if id == card.id {
                    return true;
                }
                if !seen.insert(id.clone()) {
                    return false;
                }
                current = parents.get(&id).cloned();
            }
            false
        })
        .map(|card| card.id.clone())
        .collect();
    for card in cards.iter_mut().filter(|card| cyclic.contains(&card.id)) {
        card.parent_id = None;
    }
}

fn remote_card_id(board: &Board, key: &str, cards: &[Card]) -> CardId {
    // Stable FNV-1a identity, deliberately independent of clock and pull ordering.
    // This is an opaque domain id, not a security hash. Check collisions locally.
    let mut hash = 0x6c62272e07bb014262b821756295c58d_u128;
    for part in [board.id.as_str(), board.backend.kind.as_str(), key] {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash = (hash ^ u128::from(byte)).wrapping_mul(0x0000000001000000000000000000013b);
        }
    }
    loop {
        // Preserve the contract's UUID v4 wire shape without introducing randomness.
        let mut bytes = hash.to_be_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let id = CardId::try_from(uuid::Uuid::from_bytes(bytes).to_string())
            .expect("generated UUID is a valid card id");
        if !cards.iter().any(|card| card.id == id) {
            return id;
        }
        hash = hash.wrapping_add(1);
    }
}
