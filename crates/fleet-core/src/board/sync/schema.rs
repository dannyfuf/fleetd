use super::*;

/// Merge backend schema into the board: on a board that still has `default_statuses()` and no remote
/// cards, adopt the remote statuses wholesale (ids slugified from names, category from remote or guessed
/// by name: backlog/todo/open→Unstarted, progress/review/doing→Started, done/closed/resolved→Completed,
/// cancel*/won't→Canceled); otherwise map by exact name (case-insensitive), then by category, and
/// record unmapped remote statuses. Backend properties replace `source == Backend` entries; local ones stay.
/// A schema with no statuses, properties or labels describes nothing and changes nothing.
/// Because this signature receives no cards, the service must ensure that an unsynced
/// default board has no linked cards before calling. Existing sync metadata prevents
/// wholesale adoption on subsequent calls, even when remote statuses equal the defaults.
pub fn adopt_schema(board: &mut Board, schema: &BackendSchema, now: &str) -> Vec<String> /* unmapped */
{
    // A schema that describes nothing is a degraded answer, not an authoritative empty one:
    // adopting it would erase the status map (so no transition could ever be pushed again)
    // and every backend-sourced property, whose values the caller then drops from the cards.
    if schema.statuses.is_empty() && schema.properties.is_empty() && schema.labels.is_empty() {
        return Vec::new();
    }
    // Statuses alone are enough to make the answer degraded on a board that already had a map.
    // A backend that samples them from live issues answers "no statuses" for a filter that
    // momentarily matches nothing — a label typo, `sprint in openSprints()` between sprints —
    // and the schema still carries the fixed properties it always does, so the guard above
    // never fires. Rebuilding the map from it disarms every transition push on the board.
    if schema.statuses.is_empty() && !board.sync.status_map.local_to_remote.is_empty() {
        return Vec::new();
    }
    let adopt = board.statuses == default_statuses()
        && board.sync.last_synced_at.is_none()
        && board.sync.status_map.remote_to_local.is_empty()
        && !schema.statuses.is_empty();
    if adopt {
        let mut ids = BTreeSet::new();
        board.statuses = schema
            .statuses
            .iter()
            .map(|remote| {
                let id = unique_slug(&remote.name, "status", &mut ids);
                // A column header with nothing in it names no status a key can move a card to,
                // and `validate_board` refuses one: the remote key is the only other name the
                // backend gave us for it.
                let name = if remote.name.trim().is_empty() {
                    remote_status_key(remote)
                } else {
                    remote.name.clone()
                };
                Status {
                    id: StatusId::try_from(id).expect("generated status slug is valid"),
                    name,
                    category: remote_category(remote).unwrap_or(StatusCategory::Unstarted),
                    color: None,
                }
            })
            .collect();
    }
    let mut map = StatusMap::default();
    let mut unmapped = Vec::new();
    // A column this board already pushes transitions to keeps the status it was bound to. The
    // category fallback in `match_status` is what makes this necessary: a backend that samples
    // its statuses from live issues stops naming `In Progress` the moment the last issue leaves
    // it and starts naming `In Review` instead, and both are `Started`. Without this guard the
    // fallback handed `In Review` the `in-progress` column, `local_to_remote` kept that first
    // binding forever, and every later move into that column transitioned the issue to a status
    // the user never chose — reported as a success, and never healed. A status with no column of
    // its own comes back `unmapped`, which `readopt_pulled_statuses` turns into a new column.
    let claimed: BTreeSet<StatusId> = if adopt {
        BTreeSet::new()
    } else {
        board
            .sync
            .status_map
            .local_to_remote
            .keys()
            .filter(|id| board.statuses.iter().any(|status| status.id == **id))
            .cloned()
            .collect()
    };
    // Two passes, so a binding the previous map already held wins the `or_insert` below against
    // a status matched by name or category later in the same schema.
    let resolved: Vec<Option<StatusId>> = schema
        .statuses
        .iter()
        .enumerate()
        .map(|(index, remote)| {
            if adopt {
                return board.statuses.get(index).map(|status| status.id.clone());
            }
            board
                .sync
                .status_map
                .remote_to_local
                .get(&remote_status_key(remote))
                .filter(|id| board.statuses.iter().any(|status| status.id == **id))
                .cloned()
        })
        .collect();
    let mut ordered: Vec<(usize, StatusId)> = Vec::new();
    for (index, remote) in schema.statuses.iter().enumerate() {
        let status = resolved[index].clone().or_else(|| {
            (!adopt)
                .then(|| match_status(board, remote, &claimed).map(|status| status.id.clone()))
                .flatten()
        });
        match status {
            Some(status) => ordered.push((index, status)),
            None => unmapped.push(remote_status_key(remote)),
        }
    }
    // Previously bound remotes first, then the freshly matched ones.
    ordered.sort_by_key(|(index, _)| usize::from(resolved[*index].is_none()));
    for (index, status_id) in ordered {
        let remote = &schema.statuses[index];
        let key = remote_status_key(remote);
        map.remote_to_local.insert(key.clone(), status_id.clone());
        // Include names for backends which alternate between ids and display names.
        if !remote.name.is_empty() {
            map.remote_to_local
                .entry(remote.name.clone())
                .or_insert_with(|| status_id.clone());
        }
        map.local_to_remote.entry(status_id).or_insert(key);
    }
    // A backend that samples its statuses from live issues stops naming a column the moment its
    // last card leaves it. Dropping that column's `local_to_remote` entry would keep the column
    // on the board, droppable into, and make every move into it push nothing at all: the card
    // stays dirty forever while the sync reports success. The column is still ours, so the
    // remote name we last knew for it survives a schema that no longer mentions it; a status the
    // remote really removed fails the transition loudly instead of swallowing it.
    for (status_id, remote_key) in &board.sync.status_map.local_to_remote {
        if map.local_to_remote.contains_key(status_id)
            || !board.statuses.iter().any(|status| status.id == *status_id)
        {
            continue;
        }
        map.local_to_remote
            .insert(status_id.clone(), remote_key.clone());
        map.remote_to_local
            .entry(remote_key.clone())
            .or_insert_with(|| status_id.clone());
    }
    board.sync.status_map = map;
    board
        .properties
        .retain(|property| property.source != PropertySource::Backend);
    for property in &schema.properties {
        if !board
            .properties
            .iter()
            .any(|local| local.key == property.key)
        {
            let mut property = property.clone();
            property.source = PropertySource::Backend;
            if property.name.trim().is_empty() {
                property.name.clone_from(&property.key);
            }
            board.properties.push(property);
        }
    }
    for name in schema.labels.iter().filter(|n| !n.trim().is_empty()) {
        ensure_label(board, name);
    }
    // The backend owns this list outright: a field it stopped refusing must stop being refused,
    // so the schema replaces it rather than merging into it.
    board
        .sync
        .readonly_fields
        .clone_from(&schema.readonly_fields);
    board.updated_at = now.into();
    unmapped
}

/// The key a remote status is filed under in [`StatusMap`]: its id, or its name when the
/// backend gives ids no meaning of their own (Jira transitions by status name).
#[must_use]
pub fn remote_status_key(remote: &RemoteStatus) -> String {
    if remote.id.is_empty() {
        remote.name.clone()
    } else {
        remote.id.clone()
    }
}

/// The category a remote status declares, or the one its name implies.
///
/// Public so a service adopting a status the backend's `describe` never mentioned files it
/// under the same category this module would have given it.
#[must_use]
pub fn remote_category(remote: &RemoteStatus) -> Option<StatusCategory> {
    remote.category.or_else(|| {
        let name = remote.name.to_lowercase();
        if name.contains("cancel") || name.contains("won't") || name.contains("won’t") {
            Some(StatusCategory::Canceled)
        } else if ["progress", "review", "doing"]
            .iter()
            .any(|word| name.contains(word))
        {
            Some(StatusCategory::Started)
        } else if ["done", "closed", "resolved"].contains(&name.as_str()) {
            Some(StatusCategory::Completed)
        } else if ["backlog", "todo", "open"].contains(&name.as_str()) {
            Some(StatusCategory::Unstarted)
        } else {
            None
        }
    })
}

/// The column a remote status belongs to when the map does not already name one.
///
/// The name match is exact and safe. The category fallback is a guess, and `claimed` is what
/// keeps it from guessing over a column that already pushes transitions somewhere: it may only
/// land on a column no surviving binding speaks for.
fn match_status<'a>(
    board: &'a Board,
    remote: &RemoteStatus,
    claimed: &BTreeSet<StatusId>,
) -> Option<&'a Status> {
    board
        .statuses
        .iter()
        .find(|status| status.name.to_lowercase() == remote.name.to_lowercase())
        .or_else(|| {
            remote_category(remote).and_then(|category| {
                board
                    .statuses
                    .iter()
                    .find(|status| status.category == category && !claimed.contains(&status.id))
            })
        })
}

pub(super) fn mapped_status(board: &Board, remote: &RemoteStatus) -> Option<StatusId> {
    [&remote.id, &remote.name]
        .into_iter()
        .filter_map(|key| board.sync.status_map.remote_to_local.get(key))
        .find(|id| board.statuses.iter().any(|status| status.id == **id))
        .cloned()
        // Placement only, never a binding: a card whose status the map does not name still has
        // to land in a column, so nothing is off limits here.
        .or_else(|| match_status(board, remote, &BTreeSet::new()).map(|status| status.id.clone()))
}

fn unique_slug(name: &str, fallback: &str, used: &mut BTreeSet<String>) -> String {
    let mut base = normalize_context_id(name);
    if base.is_empty() {
        base = fallback.into();
    }
    let mut slug = base.clone();
    let mut suffix = 2;
    while !used.insert(slug.clone()) {
        slug = format!("{base}-{suffix}");
        suffix += 1;
    }
    slug
}

pub(super) fn ensure_label(board: &mut Board, name: &str) -> LabelId {
    if let Some(label) = board.labels.iter().find(|label| label.name == name) {
        return label.id.clone();
    }
    let mut used = board
        .labels
        .iter()
        .map(|label| label.id.to_string())
        .collect();
    let id = LabelId::try_from(unique_slug(name, "label", &mut used))
        .expect("generated label slug is valid");
    board.labels.push(Label {
        id: id.clone(),
        name: name.into(),
        color: None,
    });
    id
}
