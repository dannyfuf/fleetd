//! Pure backend reconciliation contracts.
use super::{defaults::default_statuses, model::*, ops::*, property::*};
use crate::{
    ids::{CardId, LabelId, StatusId},
    slug::normalize_context_id,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What a card's activity says when a pull could not find its remote any more.
///
/// The sentence is the provenance: an archive this string explains belongs to the sync and is
/// undone when the remote comes back, while one a user made stands until they undo it.
pub const ARCHIVED_BY_SYNC: &str = "Remote card deleted; archived locally";
/// What it says when the remote came back and the sync undid its own archive.
pub const RESTORED_BY_SYNC: &str = "Remote card is back; unarchived locally";

/// Whether the last thing that archived this card was a pull, rather than the user.
///
/// The activity trail is the only record of who archived a card; it is read newest-first, and a
/// card whose trail no longer reaches the archive (200 entries) keeps it, because leaving a
/// card archived is the recoverable half of the two mistakes.
fn archived_by_sync(card: &Card) -> bool {
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
/// What a backend returns for one remote issue. Backend maps its native shape into this.
/// A backend-mapped remote issue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCard {
    /// Key.
    pub key: String,
    /// Url.
    pub url: Option<String>,
    /// Version.
    pub version: Option<String>,
    /// Updated at.
    pub updated_at: Option<String>,
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Status.
    pub status: RemoteStatus,
    /// Priority.
    pub priority: Option<Priority>,
    /// Labels.
    pub labels: Vec<String>, /* names */
    /// Assignee.
    pub assignee: Option<String>,
    /// Estimate.
    pub estimate: Option<u32>,
    /// Due date.
    pub due_date: Option<String>,
    /// Parent key.
    pub parent_key: Option<String>,
    /// Properties.
    pub properties: BTreeMap<String, PropertyValue>,
    /// Comments.
    pub comments: Vec<RemoteComment>,
}
/// Remote lifecycle status and optional category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    /// Id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Category.
    pub category: Option<StatusCategory>,
}
/// A remote comment with stable identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteComment {
    /// Id.
    pub id: String,
    /// Author.
    pub author: Option<String>,
    /// Body.
    pub body: String,
    /// Created at.
    pub created_at: String,
}

/// Remote changes and the next incremental cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PullResult {
    /// Cards.
    pub cards: Vec<RemoteCard>,
    /// Deleted keys.
    pub deleted_keys: Vec<String>,
    /// Cursor.
    pub cursor: Option<String>,
    /// true = `cards` is the complete remote set (absent keys are gone); false = incremental.
    pub full: bool,
    /// Keys the backend listed but could not read this time, each with its reason.
    ///
    /// A full pull says "everything absent from `cards` is gone", which is a sentence a key
    /// that merely failed to load must be kept out of: one throttled `view` would otherwise
    /// archive a live issue. The caller reports them and leaves the cursor where it was.
    #[serde(default)]
    pub failed_keys: Vec<String>,
}

/// What the backend describes about itself for a given board (statuses, labels, properties, people).
/// Remote statuses, properties, labels, and people.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendSchema {
    /// Statuses.
    pub statuses: Vec<RemoteStatus>,
    /// Labels.
    pub labels: Vec<String>,
    /// Properties.
    pub properties: Vec<PropertySchema>,
    /// Assignees.
    pub assignees: Vec<String>,
    /// Key prefix.
    pub key_prefix: Option<String>,
    /// Standard card fields the backend cannot write back; the core rejects local edits to them.
    ///
    /// Values: `title` `description` `status_id` `priority` `labels` `assignee` `estimate`
    /// `due_date` `parent_id`.
    #[serde(default)]
    pub readonly_fields: Vec<String>,
}

/// What a backend kind looks like to clients (for `fleet board set --backend` and the
/// settings dialog).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDescriptor {
    /// Registry key, e.g. `jira`.
    pub kind: String,
    /// Human label, e.g. `Jira (acli)`.
    pub label: String,
    /// Operations the backend supports.
    pub capabilities: BackendCapabilities,
    /// Settings rendered generically: `key` is the JSON key inside `BackendRef.settings`.
    pub settings_schema: Vec<PropertySchema>,
}

/// Operations supported by a board backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendCapabilities {
    /// Pull.
    pub pull: bool,
    /// Push updates.
    pub push_updates: bool,
    /// Push create.
    pub push_create: bool,
    /// Transitions.
    pub transitions: bool,
    /// Comments.
    pub comments: bool,
    /// Custom properties.
    pub custom_properties: bool,
    /// Incremental.
    pub incremental: bool,
}

/// One requested remote mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PushOp {
    /// Create a remote issue for a local card.
    Create {
        /// Card id.
        card_id: CardId,
    },
    /// Push changed fields to a linked issue.
    Update {
        /// Card id.
        card_id: CardId,
        /// Fields.
        fields: Vec<String>,
    },
    /// Move a linked issue to a remote status.
    Transition {
        /// Card id.
        card_id: CardId,
        /// Remote status.
        remote_status: String,
    },
    /// Publish one local comment on a linked issue.
    AddComment {
        /// Card id.
        card_id: CardId,
        /// Comment id.
        comment_id: String,
    },
}
/// Successful and failed remote mutations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PushResult {
    /// Acks.
    pub acks: Vec<PushAck>,
    /// Failures.
    pub failures: Vec<PushFailure>,
}
/// Acknowledged remote identity and comment links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushAck {
    /// Card id.
    pub card_id: CardId,
    /// Key.
    pub key: String,
    /// Url.
    pub url: Option<String>,
    /// Version.
    pub version: Option<String>,
    /// The remote's own `updated` stamp, when the backend read one back with the version.
    ///
    /// A card fleet created has no earlier link to inherit this from, and the ack's `version`
    /// already matches the remote, so no later pull ever reports the card as changed and fills
    /// it in: without this the field stays `null` for the life of a fleet-made card, and
    /// `board show --json` publishes that null.
    #[serde(default)]
    pub remote_updated_at: Option<String>,
    /// Comment ids.
    pub comment_ids: Vec<(String /* local */, String /* remote */)>,
}
/// A rejected remote mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushFailure {
    /// Card id.
    pub card_id: CardId,
    /// Error.
    pub error: String,
}

/// Counts and unmapped statuses produced by synchronization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    /// Pulled.
    pub pulled: usize,
    /// Created.
    pub created: usize,
    /// Updated.
    pub updated: usize,
    /// Deleted.
    pub deleted: usize,
    /// Conflicts.
    pub conflicts: usize,
    /// Pushed.
    pub pushed: usize,
    /// Unmapped statuses.
    pub unmapped_statuses: Vec<String>,
    /// Remote keys this pull could not import, with the reason each was skipped.
    #[serde(default)]
    pub skipped: Vec<String>,
    /// Cards on a linked board that `settings.push_new_cards` keeps out of the backend.
    ///
    /// The flag defaults to off, so a card made on a linked board files no issue, stays dirty
    /// forever and used to leave the sync reporting `0 pushed` with nothing anywhere saying
    /// why. Counted so the sync can say it.
    #[serde(default)]
    pub kept_local: usize,
}

/// Pure reconciliation output and pending remote operations.
pub struct Reconciled {
    /// Cards.
    pub cards: Vec<Card>,
    /// Board.
    pub board: Board,
    /// To push.
    pub to_push: Vec<PushOp>,
    /// Cards with local fields that this push cannot acknowledge.
    pub unpushed: Vec<CardId>,
    /// Summary.
    pub summary: SyncSummary,
}

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

/// Match `pull.cards` to local cards by `remote.key`; create/update/delete; detect conflicts
/// (local `dirty` and remote changed since `remote.remote_updated_at`/`version`) and resolve by policy;
/// emit `to_push` for dirty non-conflicted linked cards (Update/Transition/AddComment) and, when
/// `settings.push_new_cards`, `Create` for unlinked cards. Remote comments merge by remote id under
/// every policy, `LocalWins` included, because the advanced baseline is never offered again.
/// A card's `parent_id` follows its `remote.parent_key` only while it has a link: an unlinked card
/// keeps the local parent no pull knows about. Under `Manual`, a dirty card whose fields all match
/// the remote converges instead of recording a conflict with nothing to choose.
/// Pure and idempotent.
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

/// The selected side of a conflicted card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    /// Keep local.
    KeepLocal,
    /// Take remote.
    TakeRemote,
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

fn merge_comments(card: &mut Card, remote: &RemoteCard) {
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

/// The key a remote status is filed under in [`StatusMap`]: its id, or its name when the
/// backend gives ids no meaning of their own (Jira transitions by status name).
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

fn mapped_status(board: &Board, remote: &RemoteStatus) -> Option<StatusId> {
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

fn ensure_label(board: &mut Board, name: &str) -> LabelId {
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

fn refresh_link(card: &mut Card, backend: &str, remote: &RemoteCard, now: &str) {
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
fn link_changed(previous: Option<&RemoteLink>, next: Option<&RemoteLink>) -> bool {
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

fn remote_changed(link: &RemoteLink, remote: &RemoteCard) -> bool {
    if let (Some(previous), Some(current)) = (&link.version, &remote.version) {
        previous != current
    } else if let (Some(previous), Some(current)) = (&link.remote_updated_at, &remote.updated_at) {
        previous != current
    } else {
        true
    }
}

fn differing_fields(
    board: &Board,
    card: &Card,
    remote: &RemoteCard,
    parent: Option<Option<CardId>>,
) -> Vec<String> {
    let mut schema = board.clone();
    let mut projected = card.clone();
    apply_remote(&mut schema, &mut projected, remote, &card.updated_at);
    if let Some(parent) = parent {
        projected.parent_id = parent;
    }
    let mut fields = Vec::new();
    macro_rules! compare {
        ($($field:ident),+ $(,)?) => { $(
            if card.$field != projected.$field { fields.push(stringify!($field).into()); }
        )+ };
    }
    compare!(
        title,
        description,
        status_id,
        priority,
        assignee,
        estimate,
        due_date,
        parent_id,
        properties
    );
    // Labels represent a set, not an order, and remote ids may differ from local names.
    if card.labels.iter().collect::<BTreeSet<_>>()
        != projected.labels.iter().collect::<BTreeSet<_>>()
    {
        fields.push("labels".into());
    }
    // A field the backend cannot write back is never a side the user can hold. `ops` refuses a
    // local edit to one, so a difference here is always the remote's alone: listing it makes the
    // conflict banner offer a choice, and `KeepLocal` then plans an `Update` the backend drops
    // on the floor while acknowledging it as pushed — the card records "Pushed local changes",
    // clears `dirty`, and the next pull puts the remote's value back. The remote simply owns it.
    fields.retain(|field| {
        !board
            .sync
            .readonly_fields
            .iter()
            .any(|readonly| readonly == field)
    });
    fields
}

fn plan_push(
    board: &Board,
    card: &Card,
    remote: Option<&RemoteCard>,
    parent: Option<Option<CardId>>,
    caps: BackendCapabilities,
    ops: &mut Vec<PushOp>,
) {
    if board.backend.is_local() {
        return;
    }
    if card.remote.is_none() {
        if board.settings.push_new_cards && caps.push_create {
            ops.push(PushOp::Create {
                card_id: card.id.clone(),
            });
            // A create carries no status: the remote files the issue in whatever its workflow
            // opens with, so a card born in any other column lands in the wrong one — and the
            // ack stamps the create's own version, so no later pull, incremental or full, ever
            // notices. The move has to be pushed like any other, right behind the create that
            // gave it something to move. A backend already sitting on that status answers the
            // transition as the no-op it is.
            if caps.transitions
                && let Some(remote_status) =
                    board.sync.status_map.local_to_remote.get(&card.status_id)
            {
                ops.push(PushOp::Transition {
                    card_id: card.id.clone(),
                    remote_status: remote_status.clone(),
                });
            }
        } else {
            return;
        }
    } else if card.dirty && remote.is_none() {
        // An incremental omission is not a baseline. The service fetches one before pushing.
        return;
    } else if card.dirty {
        let Some(remote) = remote else {
            return;
        };
        let mut fields = differing_fields(board, card, remote, parent);
        let transition = fields.iter().any(|field| field == "status_id");
        fields.retain(|field| {
            field != "status_id" && (caps.custom_properties || field != "properties")
        });
        if caps.push_updates && !fields.is_empty() {
            ops.push(PushOp::Update {
                card_id: card.id.clone(),
                fields,
            });
        }
        if transition
            && caps.transitions
            && let Some(remote_status) = board.sync.status_map.local_to_remote.get(&card.status_id)
        {
            ops.push(PushOp::Transition {
                card_id: card.id.clone(),
                remote_status: remote_status.clone(),
            });
        }
    }
    if caps.comments {
        ops.extend(
            card.comments
                .iter()
                .filter(|comment| comment.remote_id.is_none())
                .map(|comment| PushOp::AddComment {
                    card_id: card.id.clone(),
                    comment_id: comment.id.clone(),
                }),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{board::defaults::new_board, model::Context};

    const NOW: &str = "2026-09-06T12:00:00Z";
    const LATER: &str = "2026-09-06T13:00:00Z";

    fn board() -> Board {
        let mut board = new_board(
            &Context {
                id: "work".parse().unwrap(),
                name: "Fleet".into(),
                owners: vec![],
                created_at: NOW.into(),
            },
            NOW,
        );
        board.backend.kind = "fake".into();
        board
    }

    fn card(board: &mut Board, id: &str) -> Card {
        create_card(
            board,
            &[],
            id.parse().unwrap(),
            CardDraft {
                title: "Title".into(),
                ..CardDraft::default()
            },
            NOW,
        )
        .unwrap()
    }

    fn status(id: &str, name: &str, category: Option<StatusCategory>) -> RemoteStatus {
        RemoteStatus {
            id: id.into(),
            name: name.into(),
            category,
        }
    }

    fn remote(key: &str) -> RemoteCard {
        RemoteCard {
            key: key.into(),
            title: "Title".into(),
            version: Some("1".into()),
            updated_at: Some(NOW.into()),
            status: status("remote-todo", "Todo", Some(StatusCategory::Unstarted)),
            ..RemoteCard::default()
        }
    }

    fn linked(board: &mut Board) -> Card {
        let mut card = card(board, "a");
        apply_remote(board, &mut card, &remote("R-1"), NOW);
        card
    }

    fn caps() -> BackendCapabilities {
        BackendCapabilities {
            pull: true,
            push_updates: true,
            push_create: true,
            transitions: true,
            comments: true,
            custom_properties: true,
            incremental: true,
        }
    }

    fn pull(remote: RemoteCard) -> PullResult {
        PullResult {
            cards: vec![remote],
            ..PullResult::default()
        }
    }

    fn property(key: &str, source: PropertySource) -> PropertySchema {
        PropertySchema {
            key: key.into(),
            name: key.into(),
            kind: PropertyKind::Text,
            options: vec![],
            editable: true,
            source,
            show_on_card: false,
        }
    }

    fn comment(id: &str) -> RemoteComment {
        RemoteComment {
            id: id.into(),
            author: Some("A".into()),
            body: "Comment".into(),
            created_at: NOW.into(),
        }
    }

    fn conflict(card: &mut Card, remote: RemoteCard) {
        card.dirty = true;
        card.conflict = Some(Conflict {
            detected_at: NOW.into(),
            remote,
            fields: vec!["title".into()],
        });
    }

    #[test]
    fn remote_updates_preserve_local_property_ownership() {
        let mut board = board();
        board.properties.push(PropertySchema {
            key: "note".into(),
            name: "Note".into(),
            kind: PropertyKind::Text,
            options: vec![],
            editable: true,
            source: PropertySource::Local,
            show_on_card: false,
        });
        board
            .properties
            .push(property("backend", PropertySource::Backend));
        let mut card = linked(&mut board);
        card.properties
            .insert("note".into(), PropertyValue::Text("private".into()));
        card.properties
            .insert("backend".into(), PropertyValue::Text("old".into()));
        let mut remote = remote("R-1");
        remote
            .properties
            .insert("backend".into(), PropertyValue::Text("new".into()));
        // Neither schema declares this key, so it must not reach the card: a property the
        // board cannot validate would make every later `validate_card` reject the document.
        remote
            .properties
            .insert("undeclared".into(), PropertyValue::Text("x".into()));
        apply_remote(&mut board, &mut card, &remote, LATER);
        assert!(!card.properties.contains_key("undeclared"));
        assert!(validate_card(&board, &card).is_ok());
        assert_eq!(
            card.properties["note"],
            PropertyValue::Text("private".into())
        );
        assert_eq!(
            card.properties["backend"],
            PropertyValue::Text("new".into())
        );
        remote
            .properties
            .insert("note".into(), PropertyValue::Text("collision".into()));
        apply_remote(&mut board, &mut card, &remote, LATER);
        assert_eq!(
            card.properties["note"],
            PropertyValue::Text("private".into())
        );
    }

    #[test]
    fn a_reverted_edit_or_reorder_clears_dirty_only_with_no_pending_comments() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.dirty = true;
        card.position = 20;
        let result = reconcile(&board, &[card.clone()], &pull(remote("R-1")), caps(), NOW);
        assert!(!result.cards[0].dirty);
        assert!(result.to_push.is_empty());
        add_comment(&mut card, "pending".into(), None, "Hello".into(), NOW).unwrap();
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), NOW);
        assert!(result.cards[0].dirty);
        assert!(matches!(
            result.to_push.as_slice(),
            [PushOp::AddComment { .. }]
        ));
    }

    #[test]
    fn adopt_pristine_schema_wholesale_with_guessed_categories() {
        let mut board = board();
        let schema = BackendSchema {
            statuses: vec![
                status("1", "Backlog", None),
                status("2", "Open", None),
                status("3", "In Review", None),
                status("4", "Resolved", None),
                status("5", "Won't do", None),
            ],
            ..BackendSchema::default()
        };
        assert!(adopt_schema(&mut board, &schema, LATER).is_empty());
        assert_eq!(
            board
                .statuses
                .iter()
                .map(|status| status.category)
                .collect::<Vec<_>>(),
            [
                StatusCategory::Unstarted,
                StatusCategory::Unstarted,
                StatusCategory::Started,
                StatusCategory::Completed,
                StatusCategory::Canceled,
            ]
        );
        assert_eq!(board.statuses[2].id.as_str(), "in-review");
        assert_eq!(
            board.sync.status_map.remote_to_local["3"].as_str(),
            "in-review"
        );
        assert_eq!(
            board.sync.status_map.local_to_remote[&board.statuses[2].id],
            "3"
        );
        assert_eq!(board.updated_at, LATER);
    }

    #[test]
    fn adopt_disambiguates_slug_collisions_and_empty_slugs() {
        let mut board = board();
        let schema = BackendSchema {
            statuses: vec![
                status("1", "A!", None),
                status("2", "A?", None),
                status("3", "中文", None),
            ],
            ..BackendSchema::default()
        };
        adopt_schema(&mut board, &schema, NOW);
        assert_eq!(
            board
                .statuses
                .iter()
                .map(|status| status.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "a-2", "status"]
        );
        assert!(validate_board(&board).is_ok());
    }

    #[test]
    fn adopt_custom_schema_maps_name_before_category() {
        let mut board = board();
        board.statuses[0].name = "Queue".into();
        let original = board.statuses.clone();
        let schema = BackendSchema {
            statuses: vec![
                status("q", "QUEUE", Some(StatusCategory::Completed)),
                status("s", "Running", Some(StatusCategory::Started)),
            ],
            ..BackendSchema::default()
        };
        assert!(adopt_schema(&mut board, &schema, NOW).is_empty());
        assert_eq!(board.statuses, original);
        assert_eq!(
            board.sync.status_map.remote_to_local["q"].as_str(),
            "backlog"
        );
        assert_eq!(
            board.sync.status_map.remote_to_local["s"].as_str(),
            "in-progress"
        );
    }

    #[test]
    fn adopt_previously_synced_default_board_only_maps() {
        let mut board = board();
        board.sync.last_synced_at = Some(NOW.into());
        let schema = BackendSchema {
            statuses: vec![status("s", "Running", Some(StatusCategory::Started))],
            ..BackendSchema::default()
        };
        adopt_schema(&mut board, &schema, LATER);
        assert_eq!(board.statuses, default_statuses());
        assert_eq!(
            board.sync.status_map.remote_to_local["s"].as_str(),
            "in-progress"
        );
    }

    #[test]
    fn re_adopting_keeps_same_named_remote_statuses_apart() {
        let mut board = board();
        let schema = BackendSchema {
            statuses: vec![
                status("s1", "Open", Some(StatusCategory::Unstarted)),
                status("s2", "Open", Some(StatusCategory::Started)),
            ],
            ..BackendSchema::default()
        };
        assert!(adopt_schema(&mut board, &schema, NOW).is_empty());
        let first = board.sync.status_map.clone();
        assert_ne!(first.remote_to_local["s1"], first.remote_to_local["s2"]);
        // A second describe must not collapse both onto the name's first match,
        // which would strip the reverse mapping and block transitions into `s2`.
        assert!(adopt_schema(&mut board, &schema, LATER).is_empty());
        assert_eq!(board.sync.status_map.remote_to_local, first.remote_to_local);
        assert_eq!(board.sync.status_map.local_to_remote, first.local_to_remote);
        assert_eq!(board.sync.status_map.local_to_remote.len(), 2);
    }

    #[test]
    fn adopt_reports_unmapped_and_clears_stale_maps() {
        let mut board = board();
        board.statuses = vec![board.statuses[0].clone()];
        board
            .sync
            .status_map
            .remote_to_local
            .insert("old".into(), board.statuses[0].id.clone());
        let schema = BackendSchema {
            statuses: vec![status("unknown", "Mystery", None)],
            ..BackendSchema::default()
        };
        assert_eq!(adopt_schema(&mut board, &schema, NOW), ["unknown"]);
        assert!(board.sync.status_map.remote_to_local.is_empty());
        assert!(board.sync.status_map.local_to_remote.is_empty());
    }

    #[test]
    fn adopt_empty_schema_keeps_at_least_one_status() {
        let mut board = board();
        adopt_schema(&mut board, &BackendSchema::default(), NOW);
        assert_eq!(board.statuses, default_statuses());
        // A schema that describes nothing is a degraded answer, not an authoritative empty
        // one: adopting it would erase the status map, so no transition could ever be pushed
        // again, and every backend property, whose values the service then drops from cards.
        board.properties = vec![property("remote", PropertySource::Backend)];
        board
            .sync
            .status_map
            .local_to_remote
            .insert(board.statuses[0].id.clone(), "open".into());
        let before = board.clone();
        assert!(adopt_schema(&mut board, &BackendSchema::default(), LATER).is_empty());
        assert_eq!(board, before);
    }

    #[test]
    fn an_unlinked_card_keeps_the_local_parent_a_pull_knows_nothing_about() {
        let mut board = board();
        let mut parent = card(&mut board, "p");
        let mut child = card(&mut board, "c");
        child.parent_id = Some(parent.id.clone());
        parent.dirty = false;
        child.dirty = false;
        let result = reconcile(
            &board,
            &[parent.clone(), child],
            &PullResult::default(),
            caps(),
            LATER,
        );
        // The remote owns the hierarchy of the cards it linked, and only those: attaching a
        // remote backend to a local board used to flatten every hierarchy on the first pull.
        assert_eq!(result.cards[1].parent_id.as_ref(), Some(&parent.id));
    }

    #[test]
    fn an_unsent_comment_never_becomes_a_conflict_with_nothing_to_choose() {
        let mut board = board();
        let mut card = linked(&mut board);
        add_comment(&mut card, "c-1".into(), None, "Hello".into(), NOW).unwrap();
        card.dirty = true;
        let mut moved = remote("R-1");
        moved.version = Some("2".into());
        let result = reconcile(&board, &[card], &pull(moved), caps(), LATER);
        let synced = &result.cards[0];
        // Nothing differs; the card is dirty only for a comment the backend has not taken.
        // A conflict here has no field to resolve and `plan_push` skips conflicted cards, so
        // the comment could never reach the backend at all.
        assert_eq!(synced.conflict, None);
        assert_eq!(result.summary.conflicts, 0);
        assert!(synced.dirty);
        assert_eq!(
            synced.remote.as_ref().unwrap().version.as_deref(),
            Some("2")
        );
        assert!(
            result
                .to_push
                .iter()
                .any(|op| matches!(op, PushOp::AddComment { .. }))
        );
    }

    #[test]
    fn adopt_replaces_backend_properties_and_preserves_local_keys() {
        let mut board = board();
        board.properties = vec![
            property("local", PropertySource::Local),
            property("old", PropertySource::Backend),
        ];
        let mut replacement = property("local", PropertySource::Backend);
        replacement.kind = PropertyKind::Bool;
        let schema = BackendSchema {
            properties: vec![replacement, property("new", PropertySource::Local)],
            labels: vec!["Bug".into()],
            ..BackendSchema::default()
        };
        adopt_schema(&mut board, &schema, NOW);
        assert_eq!(
            board.properties,
            [
                property("local", PropertySource::Local),
                property("new", PropertySource::Backend)
            ]
        );
        assert_eq!(board.labels[0].name, "Bug");
        let before = board.clone();
        adopt_schema(&mut board, &schema, NOW);
        assert_eq!(board, before);
    }

    #[test]
    fn remote_versions_take_precedence_over_timestamps() {
        let mut board = board();
        let card = linked(&mut board);
        let link = card.remote.unwrap();
        let mut remote = remote("R-1");
        remote.updated_at = Some(LATER.into());
        assert!(!remote_changed(&link, &remote));
        remote.version = Some("2".into());
        remote.updated_at = Some(NOW.into());
        assert!(remote_changed(&link, &remote));
    }

    #[test]
    fn remote_change_falls_back_to_time_then_always_changed() {
        let mut board = board();
        let card = linked(&mut board);
        let mut link = card.remote.unwrap();
        let mut remote = remote("R-1");
        remote.version = None;
        assert!(!remote_changed(&link, &remote));
        remote.updated_at = Some(LATER.into());
        assert!(remote_changed(&link, &remote));
        remote.updated_at = None;
        assert!(remote_changed(&link, &remote));
        link.version = None;
        link.remote_updated_at = None;
        assert!(remote_changed(&link, &remote));
    }

    #[test]
    fn reconcile_creates_numbered_linked_cards_without_mutating_inputs() {
        let mut board = board();
        board.next_number = 42;
        let pull = pull(remote("R-1"));
        let before_board = board.clone();
        let before_pull = pull.clone();
        let result = reconcile(&board, &[], &pull, caps(), LATER);
        assert_eq!(board, before_board);
        assert_eq!(pull, before_pull);
        assert_eq!(result.cards.len(), 1);
        assert_eq!(result.cards[0].number, 42);
        let uuid = uuid::Uuid::parse_str(result.cards[0].id.as_str()).unwrap();
        assert_eq!(uuid.get_version_num(), 4);
        assert_eq!(result.board.next_number, 43);
        assert_eq!(result.cards[0].remote.as_ref().unwrap().key, "R-1");
        assert!(!result.cards[0].dirty);
        assert_eq!(result.cards[0].activity.len(), 1);
        assert_eq!(result.cards[0].activity[0].kind, ActivityKind::Synced);
        assert_eq!(
            (
                result.summary.pulled,
                result.summary.created,
                result.summary.updated
            ),
            (1, 1, 0)
        );
        assert_eq!(
            reconcile(&board, &[], &pull, caps(), LATER).cards,
            result.cards
        );
    }

    #[test]
    fn reconcile_replay_does_not_duplicate_cards_or_activity() {
        let board = board();
        let pull = pull(remote("R-1"));
        let first = reconcile(&board, &[], &pull, caps(), NOW);
        let second = reconcile(&first.board, &first.cards, &pull, caps(), NOW);
        assert_eq!(first.cards, second.cards);
        assert_eq!(first.board, second.board);
        assert_eq!(
            (
                second.summary.created,
                second.summary.updated,
                second.summary.deleted
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn version_less_replays_advance_the_clock_without_rewriting_history() {
        let mut board = board();
        let mut card = card(&mut board, "a");
        let mut remote = remote("R-1");
        remote.version = None;
        remote.updated_at = None;
        apply_remote(&mut board, &mut card, &remote, NOW);
        let pull = pull(remote);
        let mut cards = vec![card];
        let history = cards[0].activity.len();
        for now in [LATER, "2026-09-06T14:00:00Z"] {
            let result = reconcile(&board, &cards, &pull, caps(), now);
            // The backend reports neither a version nor a timestamp, so every sync re-applies
            // the same remote: that must not count as an update or grow the activity log.
            assert_eq!(result.summary.updated, 0, "{now}");
            assert_eq!(result.cards[0].activity.len(), history, "{now}");
            assert_eq!(result.cards[0].updated_at, NOW, "{now}");
            board = result.board;
            cards = result.cards;
        }
    }

    #[test]
    fn one_unimportable_remote_card_never_discards_the_rest_of_the_pull() {
        let board = board();
        let mut bad = remote("B-1");
        bad.title = "  ".into();
        let pull = PullResult {
            cards: vec![remote("G-1"), bad],
            ..PullResult::default()
        };
        let result = reconcile(&board, &[], &pull, caps(), NOW);
        assert_eq!(result.summary.created, 1);
        assert_eq!(result.cards.len(), 1);
        assert_eq!(
            result.cards[0]
                .remote
                .as_ref()
                .map(|link| link.key.as_str()),
            Some("G-1")
        );
        assert!(result.board.sync.last_error.is_none());
        assert_eq!(result.summary.skipped.len(), 1);
    }

    #[test]
    fn a_local_comment_never_pins_dirty_on_a_backend_without_comments() {
        let mut board = board();
        let mut card = linked(&mut board);
        add_comment(&mut card, "local".into(), None, "Note".into(), LATER).unwrap();
        card.dirty = true;
        let caps = BackendCapabilities {
            comments: false,
            ..caps()
        };
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps, LATER);
        assert!(!result.cards[0].dirty);
        assert!(result.to_push.is_empty());
    }

    #[test]
    fn local_wins_replay_without_remote_change_markers_is_idempotent() {
        let mut board = board();
        board.settings.conflict_policy = ConflictPolicy::LocalWins;
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.dirty = true;
        let mut remote = remote("R-1");
        remote.version = None;
        remote.updated_at = None;
        let pull = pull(remote);
        let first = reconcile(&board, &[card], &pull, caps(), LATER);
        let replay = reconcile(&first.board, &first.cards, &pull, caps(), LATER);
        assert_eq!(first.cards, replay.cards);
        assert_eq!(first.to_push, replay.to_push);
    }

    #[test]
    fn reconcile_clean_changed_remote_applies_and_counts_update() {
        let mut board = board();
        let card = linked(&mut board);
        let before = card.clone();
        let mut remote = remote("R-1");
        remote.title = "Remote edit".into();
        remote.version = Some("2".into());
        let result = reconcile(
            &board,
            std::slice::from_ref(&card),
            &pull(remote),
            caps(),
            LATER,
        );
        assert_eq!(card, before);
        assert_eq!(result.cards[0].title, "Remote edit");
        assert_eq!(result.summary.updated, 1);
        assert!(!result.cards[0].dirty);
    }

    #[test]
    fn manual_conflict_records_only_differing_fields_and_blocks_push() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.dirty = true;
        let mut remote = remote("R-1");
        remote.version = Some("2".into());
        let pull = pull(remote);
        let result = reconcile(&board, &[card], &pull, caps(), LATER);
        assert_eq!(result.cards[0].title, "Local");
        assert_eq!(result.cards[0].conflict.as_ref().unwrap().fields, ["title"]);
        assert!(result.to_push.is_empty());
        assert_eq!(result.summary.conflicts, 1);
        assert_eq!(
            result.cards[0].activity.last().unwrap().kind,
            ActivityKind::ConflictDetected
        );
        let replay = reconcile(&result.board, &result.cards, &pull, caps(), LATER);
        assert_eq!(replay.cards, result.cards);
    }

    #[test]
    fn remote_wins_policy_discards_local_edits_and_conflict() {
        let mut board = board();
        board.settings.conflict_policy = ConflictPolicy::RemoteWins;
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.dirty = true;
        let mut remote = remote("R-1");
        remote.title = "Remote".into();
        remote.version = Some("2".into());
        let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
        assert_eq!(result.cards[0].title, "Remote");
        assert!(!result.cards[0].dirty);
        assert!(result.cards[0].conflict.is_none());
        assert!(result.to_push.is_empty());
        assert_eq!(result.summary.updated, 1);
    }

    #[test]
    fn local_wins_policy_keeps_local_and_plans_update() {
        let mut board = board();
        board.settings.conflict_policy = ConflictPolicy::LocalWins;
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.dirty = true;
        let mut remote = remote("R-1");
        remote.version = Some("2".into());
        let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
        assert_eq!(result.cards[0].title, "Local");
        assert_eq!(result.cards[0].updated_at, LATER);
        assert!(result.cards[0].dirty);
        assert!(result.cards[0].conflict.is_none());
        assert_eq!(
            result.to_push,
            [PushOp::Update {
                card_id: "a".parse().unwrap(),
                fields: vec!["title".into()]
            }]
        );
        assert_eq!(
            result.cards[0].remote.as_ref().unwrap().version.as_deref(),
            Some("2")
        );
    }

    #[test]
    fn local_wins_replayed_over_one_unchanged_pull_logs_one_resolution() {
        let mut board = board();
        board.settings.conflict_policy = ConflictPolicy::LocalWins;
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.dirty = true;
        // A backend that reports neither a version nor a timestamp is replayed on every sync:
        // `remote_changed` is always true, so every replay reaches the LocalWins arm.
        let mut incoming = remote("R-1");
        incoming.version = None;
        incoming.updated_at = None;
        // Each sync carries its own clock reading, which is what `refresh_link` writes into
        // `synced_at`: comparing the whole link would report a change every single time.
        const LATEST: &str = "2026-09-06T14:00:00Z";
        const LAST: &str = "2026-09-06T15:00:00Z";
        let first = reconcile(&board, &[card], &pull(incoming.clone()), caps(), LATER);
        let entries = first.cards[0].activity.len();
        let second = reconcile(
            &board,
            &first.cards,
            &pull(incoming.clone()),
            caps(),
            LATEST,
        );
        let third = reconcile(&board, &second.cards, &pull(incoming), caps(), LAST);
        assert_eq!(
            third.cards[0].activity.len(),
            entries,
            "one entry per sync evicts the card's real history through the 200-entry cap and \
             rewrites the document every time"
        );
    }

    #[test]
    fn an_archived_card_takes_the_remote_instead_of_hiding_a_conflict() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.dirty = true;
        card.archived = true;
        let mut incoming = remote("R-1");
        incoming.version = Some("2".into());
        incoming.title = "Remote".into();
        let result = reconcile(&board, &[card], &pull(incoming), caps(), LATER);
        assert!(
            result.cards[0].conflict.is_none(),
            "the push pass skips archived cards, so a conflict on one is invisible to \
             `summarize` and to `summary.conflicts` until an un-archive pops it out"
        );
        assert_eq!(result.cards[0].title, "Remote");
        assert!(!result.cards[0].dirty);
        assert_eq!(result.summary.conflicts, 0);
    }

    #[test]
    fn a_remote_null_or_impossible_date_is_never_stored_as_a_property_value() {
        let mut board = board();
        board.properties.push(PropertySchema {
            kind: PropertyKind::Date,
            ..property("eta", PropertySource::Backend)
        });
        board
            .properties
            .push(property("note", PropertySource::Backend));
        let mut card = card(&mut board, "a");
        let mut incoming = remote("R-1");
        incoming
            .properties
            .insert("eta".into(), PropertyValue::Date("2026-02-30".into()));
        incoming
            .properties
            .insert("note".into(), PropertyValue::Null);
        apply_remote(&mut board, &mut card, &incoming, LATER);
        assert!(
            !card.properties.contains_key("note"),
            "`Null` is how every local path spells no value; storing it renders one no local \
             path can produce"
        );
        assert!(
            !card.properties.contains_key("eta"),
            "`validate_card` refuses the date, so storing it skips the card on every pull"
        );
        assert!(validate_card(&board, &card).is_ok());
    }

    #[test]
    fn local_wins_still_takes_remote_comments_and_never_re_sends_them() {
        let mut board = board();
        board.settings.conflict_policy = ConflictPolicy::LocalWins;
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.dirty = true;
        let mut incoming = remote("R-1");
        incoming.version = Some("2".into());
        incoming.comments = vec![comment("rc-1")];
        let result = reconcile(&board, &[card], &pull(incoming.clone()), caps(), LATER);
        assert_eq!(result.cards[0].title, "Local");
        assert_eq!(result.cards[0].comments.len(), 1);
        assert_eq!(
            result.cards[0].comments[0].remote_id.as_deref(),
            Some("rc-1")
        );
        // An already-remote comment is never queued back to the backend.
        assert_eq!(
            result.to_push,
            [PushOp::Update {
                card_id: "a".parse().unwrap(),
                fields: vec!["title".into()]
            }]
        );
        // The baseline moved, so replaying the same pull must not duplicate the comment.
        let replay = reconcile(&board, &result.cards, &pull(incoming), caps(), LATER);
        assert_eq!(replay.cards[0].comments.len(), 1);
    }

    #[test]
    fn unchanged_remote_dirty_card_plans_update_and_mapped_transition() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.dirty = true;
        card.title = "Local".into();
        card.status_id = "in-progress".parse().unwrap();
        board
            .sync
            .status_map
            .local_to_remote
            .insert(card.status_id.clone(), "remote-progress".into());
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), LATER);
        assert_eq!(
            result.to_push,
            [
                PushOp::Update {
                    card_id: "a".parse().unwrap(),
                    fields: vec!["title".into()]
                },
                PushOp::Transition {
                    card_id: "a".parse().unwrap(),
                    remote_status: "remote-progress".into()
                }
            ]
        );
        assert_eq!(result.summary.conflicts, 0);
        assert_eq!(result.summary.pushed, 0); // Execution belongs to the daemon.
    }

    #[test]
    fn transition_requires_status_difference_capability_and_mapping() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.dirty = true;
        board
            .sync
            .status_map
            .local_to_remote
            .insert(card.status_id.clone(), "remote-todo".into());
        assert!(
            reconcile(
                &board,
                std::slice::from_ref(&card),
                &pull(remote("R-1")),
                caps(),
                NOW
            )
            .to_push
            .is_empty()
        );
        card.status_id = "done".parse().unwrap();
        assert!(
            reconcile(
                &board,
                std::slice::from_ref(&card),
                &pull(remote("R-1")),
                caps(),
                NOW
            )
            .to_push
            .is_empty()
        );
        board
            .sync
            .status_map
            .local_to_remote
            .insert(card.status_id.clone(), "remote-done".into());
        let mut caps = caps();
        caps.transitions = false;
        assert!(
            reconcile(&board, &[card], &pull(remote("R-1")), caps, NOW)
                .to_push
                .is_empty()
        );
    }

    #[test]
    fn push_capabilities_gate_updates_properties_and_comments() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.dirty = true;
        card.title = "Local".into();
        card.properties
            .insert("custom".into(), PropertyValue::Text("x".into()));
        add_comment(&mut card, "c".into(), None, "Hi".into(), NOW).unwrap();
        assert!(
            reconcile(
                &board,
                std::slice::from_ref(&card),
                &pull(remote("R-1")),
                BackendCapabilities::default(),
                NOW
            )
            .to_push
            .is_empty()
        );
        let caps = BackendCapabilities {
            push_updates: true,
            ..BackendCapabilities::default()
        };
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps, NOW);
        assert_eq!(
            result.to_push,
            [PushOp::Update {
                card_id: "a".parse().unwrap(),
                fields: vec!["title".into()]
            }]
        );
    }

    #[test]
    fn incremental_missing_dirty_link_waits_for_a_baseline() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.dirty = true;
        card.status_id = "done".parse().unwrap();
        let result = reconcile(&board, &[card], &PullResult::default(), caps(), NOW);
        assert!(result.to_push.is_empty());
        assert!(result.cards[0].dirty);
    }

    #[test]
    fn new_local_create_requires_setting_and_capability() {
        let mut board = board();
        let card = card(&mut board, "local");
        assert!(
            reconcile(
                &board,
                std::slice::from_ref(&card),
                &PullResult::default(),
                caps(),
                NOW
            )
            .to_push
            .is_empty()
        );
        board.settings.push_new_cards = true;
        assert!(
            reconcile(
                &board,
                std::slice::from_ref(&card),
                &PullResult::default(),
                BackendCapabilities::default(),
                NOW
            )
            .to_push
            .is_empty()
        );
        assert_eq!(
            reconcile(&board, &[card], &PullResult::default(), caps(), NOW).to_push,
            [PushOp::Create {
                card_id: "local".parse().unwrap()
            }]
        );
    }

    /// `pushNewCards` is off by default, so a card made on a linked board files no issue and
    /// stays dirty forever. The sync reported `0 pushed` and nothing else; the count is what
    /// lets it name the setting that is holding the card.
    #[test]
    fn a_card_push_new_cards_holds_back_is_counted_so_the_sync_can_say_so() {
        let mut board = board();
        let card = card(&mut board, "local");
        let held = reconcile(
            &board,
            std::slice::from_ref(&card),
            &PullResult::default(),
            caps(),
            NOW,
        );
        assert!(held.to_push.is_empty());
        assert_eq!(held.summary.kept_local, 1);
        board.settings.push_new_cards = true;
        let pushed = reconcile(
            &board,
            std::slice::from_ref(&card),
            &PullResult::default(),
            caps(),
            NOW,
        );
        assert!(!pushed.to_push.is_empty());
        assert_eq!(
            pushed.summary.kept_local, 0,
            "a card the push carries is not kept local"
        );
        // A local board has no backend to withhold anything from.
        let mut local = board.clone();
        local.backend.kind = BackendRef::LOCAL.into();
        local.settings.push_new_cards = false;
        assert_eq!(
            reconcile(&local, &[card], &PullResult::default(), caps(), NOW)
                .summary
                .kept_local,
            0
        );
    }

    /// A create carries no status, so a card born anywhere but the backend's opening column
    /// lands in the wrong one — and the create's own ack stamps a version no later pull beats.
    #[test]
    fn a_created_card_pushes_the_column_it_was_born_in() {
        let mut board = board();
        let schema = BackendSchema {
            statuses: vec![
                status("todo", "Todo", Some(StatusCategory::Unstarted)),
                status("done-id", "Done", Some(StatusCategory::Completed)),
            ],
            ..BackendSchema::default()
        };
        adopt_schema(&mut board, &schema, NOW);
        board.settings.push_new_cards = true;
        let done = board.sync.status_map.remote_to_local["done-id"].clone();
        let mut fresh = card(&mut board, "local");
        fresh.status_id = done.clone();
        let result = reconcile(&board, &[fresh], &PullResult::default(), caps(), NOW);
        assert_eq!(
            result.to_push,
            [
                PushOp::Create {
                    card_id: "local".parse().unwrap()
                },
                PushOp::Transition {
                    card_id: "local".parse().unwrap(),
                    remote_status: "done-id".into()
                }
            ],
            "the column has to be pushed right behind the create that gave it something to move"
        );
    }

    /// A field the backend cannot write back is never a side the user can hold: offering it as a
    /// conflict lets `KeepLocal` plan an `Update` the backend drops while acknowledging it.
    #[test]
    fn a_read_only_field_is_neither_a_conflict_nor_an_update() {
        let mut board = board();
        board.sync.readonly_fields = vec!["priority".into()];
        board.settings.conflict_policy = ConflictPolicy::Manual;
        let mut card = linked(&mut board);
        card.title = "Renamed here".into();
        card.dirty = true;
        let mut remote = remote("R-1");
        remote.version = Some("2".into());
        remote.priority = Some(Priority::Urgent);
        let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
        let conflict = result.cards[0]
            .conflict
            .as_ref()
            .expect("the title still conflicts");
        assert_eq!(
            conflict.fields,
            ["title"],
            "priority is Jira's alone; the user was never offered a side to keep"
        );
    }

    #[test]
    fn unpushed_comments_emit_once_per_local_id_even_on_clean_cards() {
        let mut board = board();
        let mut card = linked(&mut board);
        add_comment(&mut card, "local".into(), None, "Local".into(), NOW).unwrap();
        add_comment(&mut card, "sent".into(), None, "Sent".into(), NOW).unwrap();
        card.comments[1].remote_id = Some("remote".into());
        card.dirty = false;
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), NOW);
        assert_eq!(
            result.to_push,
            [PushOp::AddComment {
                card_id: "a".parse().unwrap(),
                comment_id: "local".into()
            }]
        );
    }

    #[test]
    fn unlinked_comments_follow_create_and_are_not_sent_without_create() {
        let mut board = board();
        let mut card = card(&mut board, "local");
        add_comment(&mut card, "c".into(), None, "Hi".into(), NOW).unwrap();
        assert!(
            reconcile(
                &board,
                std::slice::from_ref(&card),
                &PullResult::default(),
                caps(),
                NOW
            )
            .to_push
            .is_empty()
        );
        board.settings.push_new_cards = true;
        assert_eq!(
            reconcile(&board, &[card], &PullResult::default(), caps(), NOW).to_push,
            [
                PushOp::Create {
                    card_id: "local".parse().unwrap()
                },
                PushOp::AddComment {
                    card_id: "local".parse().unwrap(),
                    comment_id: "c".into()
                }
            ]
        );
    }

    #[test]
    fn full_pull_archives_missing_link_but_incremental_does_not() {
        let mut board = board();
        let linked = linked(&mut board);
        let local = card(&mut board, "local");
        let cards = [linked, local];
        let incremental = reconcile(&board, &cards, &PullResult::default(), caps(), NOW);
        assert!(!incremental.cards[0].archived);
        assert_eq!(incremental.summary.deleted, 0);
        // The pull reports a deletion of its own, so it is a pull that saw the project — not the
        // wholly empty answer a broken filter gives, which archives nothing.
        let pull = PullResult {
            full: true,
            deleted_keys: vec!["R-9".into()],
            ..PullResult::default()
        };
        let full = reconcile(&board, &cards, &pull, caps(), LATER);
        assert!(full.cards[0].archived);
        assert!(!full.cards[1].archived);
        assert_eq!(full.summary.deleted, 1);
        assert_eq!(full.cards.len(), 2);
        assert_eq!(
            full.cards[0].activity.last().unwrap().kind,
            ActivityKind::Synced
        );
        let replay = reconcile(&full.board, &full.cards, &pull, caps(), LATER);
        assert_eq!(replay.summary.deleted, 0);
        assert_eq!(replay.cards, full.cards);
    }

    /// A filter that matches nothing answers exactly like a project that emptied itself, and
    /// only one of the two is worth archiving a whole board over.
    #[test]
    fn a_full_pull_that_brought_back_nothing_at_all_archives_nothing() {
        let mut board = board();
        let mut dirty = linked(&mut board);
        dirty.status_id = board.statuses[2].id.clone();
        dirty.dirty = true;
        let cards = [dirty];
        let empty = PullResult {
            full: true,
            ..PullResult::default()
        };
        let result = reconcile(&board, &cards, &empty, caps(), LATER);
        assert!(
            !result.cards[0].archived,
            "an empty full pull is a filter that matched nothing, not an emptied project"
        );
        assert_eq!(result.summary.deleted, 0);
        assert!(
            result.cards[0].dirty,
            "the queued push must survive: archiving would have cleared it for good"
        );
        // The board still has a real emptying to report the moment the backend says so.
        let deleted = PullResult {
            full: true,
            deleted_keys: vec!["R-1".into()],
            ..PullResult::default()
        };
        let gone = reconcile(&board, &cards, &deleted, caps(), LATER);
        assert!(gone.cards[0].archived);
    }

    /// A status the schema stopped naming is still a column cards can be dropped into: losing
    /// its remote key would make every move into it push nothing and stay dirty forever.
    #[test]
    fn a_column_the_schema_no_longer_names_keeps_the_remote_status_it_was_mapped_to() {
        let mut board = board();
        let schema = BackendSchema {
            statuses: vec![
                status("todo", "Todo", Some(StatusCategory::Unstarted)),
                status("blocked", "Blocked", Some(StatusCategory::Started)),
            ],
            ..BackendSchema::default()
        };
        assert!(adopt_schema(&mut board, &schema, NOW).is_empty());
        let blocked = board
            .sync
            .status_map
            .remote_to_local
            .get("blocked")
            .cloned()
            .expect("Blocked was adopted");
        // The last card leaves Blocked, so a backend that samples its statuses from live issues
        // stops naming it.
        let thinner = BackendSchema {
            statuses: vec![status("todo", "Todo", Some(StatusCategory::Unstarted))],
            ..schema.clone()
        };
        adopt_schema(&mut board, &thinner, LATER);
        assert_eq!(
            board.sync.status_map.local_to_remote.get(&blocked),
            Some(&"blocked".to_owned()),
            "the column is still on the board, so the status it pushes to must be too"
        );
    }

    /// A synced column keeps the status it pushes to, whatever the next sample happens to name.
    ///
    /// The sample is taken from live issues, so an ordinary Jira workflow move — the last two
    /// `In Progress` issues going to `In Review` — makes `describe` stop naming `In Progress`
    /// and start naming `In Review`. Both are `Started`, so `match_status`'s category fallback
    /// used to hand `In Review` the `in-progress` column: `local_to_remote` kept that first
    /// binding forever, and from then on every move into the `In Progress` column transitioned
    /// the issue to `In Review` — written to Jira, reported as a success, never healed.
    #[test]
    fn a_status_the_sample_stopped_naming_never_loses_its_column_to_another_one() {
        let mut board = board();
        let described = |names: &[(&str, StatusCategory)]| BackendSchema {
            statuses: names
                .iter()
                .map(|(name, category)| status(name, name, Some(*category)))
                .collect(),
            ..BackendSchema::default()
        };
        let first = described(&[
            ("To Do", StatusCategory::Unstarted),
            ("In Progress", StatusCategory::Started),
            ("Done", StatusCategory::Completed),
        ]);
        assert!(adopt_schema(&mut board, &first, NOW).is_empty());
        board.sync.last_synced_at = Some(NOW.into());
        let in_progress = board
            .sync
            .status_map
            .remote_to_local
            .get("In Progress")
            .cloned()
            .expect("In Progress was adopted");

        // Every issue leaves In Progress for In Review, an ordinary workflow step.
        let moved = described(&[
            ("To Do", StatusCategory::Unstarted),
            ("In Review", StatusCategory::Started),
            ("Done", StatusCategory::Completed),
        ]);
        assert_eq!(adopt_schema(&mut board, &moved, LATER), ["In Review"]);
        assert_eq!(
            board.sync.status_map.local_to_remote.get(&in_progress),
            Some(&"In Progress".to_owned()),
            "the column still pushes to the status the user bound it to"
        );
        assert!(
            !board
                .sync
                .status_map
                .remote_to_local
                .contains_key("In Review"),
            "a status with no column of its own is unmapped, so a column is made for it"
        );

        // And when In Progress comes back beside In Review, it is still its own column.
        let both = described(&[
            ("To Do", StatusCategory::Unstarted),
            ("In Review", StatusCategory::Started),
            ("In Progress", StatusCategory::Started),
            ("Done", StatusCategory::Completed),
        ]);
        adopt_schema(&mut board, &both, LATER);
        assert_eq!(
            board.sync.status_map.local_to_remote.get(&in_progress),
            Some(&"In Progress".to_owned())
        );
    }

    /// A schema with no statuses at all is a degraded answer on a board that already had a map.
    #[test]
    fn a_schema_with_no_statuses_never_disarms_an_existing_status_map() {
        let mut board = board();
        let schema = BackendSchema {
            statuses: vec![status("todo", "Todo", Some(StatusCategory::Unstarted))],
            ..BackendSchema::default()
        };
        adopt_schema(&mut board, &schema, NOW);
        let before = board.sync.status_map.clone();
        // The properties every Jira schema carries keep the "describes nothing" guard from
        // firing, so statuses alone have to be enough.
        let degraded = BackendSchema {
            statuses: vec![],
            properties: vec![property("jira.created", PropertySource::Backend)],
            ..BackendSchema::default()
        };
        assert!(adopt_schema(&mut board, &degraded, LATER).is_empty());
        assert_eq!(board.sync.status_map, before);
    }

    /// A key can go missing for a reason that is not a deletion; the archive must be undone
    /// when it comes back, or a permission blip costs the board every card it touched.
    #[test]
    fn a_card_the_sync_archived_comes_back_when_its_remote_does() {
        let mut board = board();
        let card = linked(&mut board);
        let vanished = PullResult {
            full: true,
            deleted_keys: vec!["R-9".into()],
            ..PullResult::default()
        };
        let gone = reconcile(&board, &[card], &vanished, caps(), LATER);
        assert!(gone.cards[0].archived);
        let back = reconcile(
            &gone.board,
            &gone.cards,
            &pull(remote("R-1")),
            caps(),
            LATER,
        );
        assert!(
            !back.cards[0].archived,
            "the remote is back; the card is not"
        );
        assert_eq!(
            back.cards[0].activity.last().unwrap().message,
            RESTORED_BY_SYNC
        );
        // Idempotent: a second pull of the same remote changes nothing about the archive.
        let again = reconcile(
            &back.board,
            &back.cards,
            &pull(remote("R-1")),
            caps(),
            LATER,
        );
        assert!(!again.cards[0].archived);
    }

    /// The other half of the rule: an archive the user made is theirs, and no pull undoes it.
    #[test]
    fn a_card_the_user_archived_stays_archived_through_a_pull() {
        let mut board = board();
        let mut card = linked(&mut board);
        apply_card_patch(
            &board,
            &mut card,
            CardPatch {
                archived: Some(true),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap();
        card.dirty = false;
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), LATER);
        assert!(result.cards[0].archived);
    }

    /// A move into a column no remote status is mapped to is dropped by `plan_push`; without
    /// this the card is dirty forever and the sync reports nothing at all.
    #[test]
    fn a_move_into_an_unmapped_column_is_named_by_the_summary() {
        let mut board = board();
        let mut card = linked(&mut board);
        // The remote sits in the first column, the card was moved to the second, and no remote
        // status is mapped to that one — exactly what a sampled `describe` leaves behind when
        // no issue happens to be in that status.
        board
            .sync
            .status_map
            .remote_to_local
            .insert("remote-todo".into(), board.statuses[0].id.clone());
        board.sync.status_map.local_to_remote.clear();
        card.dirty = true;
        card.status_id = board.statuses[1].id.clone();
        let column = board.statuses[1].name.clone();
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), LATER);
        assert!(
            result
                .to_push
                .iter()
                .all(|op| !matches!(op, PushOp::Transition { .. }))
        );
        assert!(
            result.summary.unmapped_statuses.contains(&column),
            "{:?}",
            result.summary.unmapped_statuses
        );
    }

    #[test]
    fn explicit_deletions_archive_even_dirty_conflicts_and_win_over_pull() {
        let mut board = board();
        let mut card = linked(&mut board);
        conflict(&mut card, remote("R-1"));
        let pull = PullResult {
            cards: vec![remote("R-1")],
            deleted_keys: vec!["R-1".into()],
            ..PullResult::default()
        };
        let result = reconcile(&board, &[card], &pull, caps(), LATER);
        assert!(result.cards[0].archived);
        assert!(!result.cards[0].dirty);
        assert!(result.cards[0].conflict.is_none());
        assert!(result.to_push.is_empty());
        assert_eq!((result.summary.deleted, result.summary.conflicts), (1, 0));
    }

    #[test]
    fn archived_cards_are_not_pushed() {
        let mut board = board();
        board.settings.push_new_cards = true;
        let mut card = card(&mut board, "a");
        card.archived = true;
        assert!(
            reconcile(&board, &[card], &PullResult::default(), caps(), NOW)
                .to_push
                .is_empty()
        );
    }

    /// A create files the parent alongside the child, but the ack that links the child only
    /// carries its own key. Rebuilding the link from the previous one leaves `parent_key` at
    /// `None`, and the next pull — which reports the very `updated` the ack stored, so nothing
    /// looks changed — then derives `parent_id` from that `None` and unparents a card Jira
    /// holds, in a read-only field the user cannot put back.
    #[test]
    fn a_created_child_keeps_the_parent_the_push_filed_for_it() {
        let mut board = board();
        let mut parent = card(&mut board, "a");
        parent.dirty = true;
        let mut child = card(&mut board, "b");
        child.parent_id = Some("a".parse().unwrap());
        child.dirty = true;
        let mut cards = vec![parent, child];
        let acks = PushResult {
            acks: vec![
                PushAck {
                    card_id: "a".parse().unwrap(),
                    key: "R-1".into(),
                    url: None,
                    version: Some("1".into()),
                    remote_updated_at: None,
                    comment_ids: vec![],
                },
                PushAck {
                    card_id: "b".parse().unwrap(),
                    key: "R-2".into(),
                    url: None,
                    version: Some("1".into()),
                    remote_updated_at: None,
                    comment_ids: vec![],
                },
            ],
            failures: vec![],
        };
        apply_push_result(&mut cards, &acks, "fake", &[], NOW);
        assert_eq!(
            cards[1].remote.as_ref().unwrap().parent_key.as_deref(),
            Some("R-1")
        );
        // The pull that follows repeats the version the ack stored, so nothing about the child
        // reads as changed; the hierarchy still has to survive it.
        let mut remote_child = remote("R-2");
        remote_child.parent_key = Some("R-1".into());
        let pull = PullResult {
            cards: vec![remote("R-1"), remote_child],
            ..PullResult::default()
        };
        let result = reconcile(&board, &cards, &pull, caps(), LATER);
        let child = result
            .cards
            .iter()
            .find(|card| card.id.as_str() == "b")
            .unwrap();
        assert_eq!(child.parent_id.as_ref().map(CardId::as_str), Some("a"));
        assert_eq!(
            child.remote.as_ref().unwrap().parent_key.as_deref(),
            Some("R-1")
        );
    }

    /// "Absent from a full pull" means deleted — but a key the backend listed and then could
    /// not read is not absent, it is unread. Archiving it takes a live issue off the board and
    /// clears the queued push of every field on it.
    #[test]
    fn a_key_the_pull_could_not_read_is_not_archived_by_a_full_pull() {
        let mut first = board();
        let card = linked(&mut first);
        let pull = PullResult {
            cards: vec![remote("R-2")],
            full: true,
            failed_keys: vec!["R-1: 429 Too Many Requests".into()],
            ..PullResult::default()
        };
        let result = reconcile(&first, &[card], &pull, caps(), LATER);
        let kept = result
            .cards
            .iter()
            .find(|card| card.remote.as_ref().is_some_and(|link| link.key == "R-1"))
            .expect("the unread card is still on the board");
        assert!(!kept.archived);
        assert_eq!(result.summary.deleted, 0);
        // A key that really is gone from the same pull still archives.
        let gone = PullResult {
            cards: vec![remote("R-2")],
            full: true,
            ..PullResult::default()
        };
        let mut second = board();
        let card = linked(&mut second);
        let result = reconcile(&second, &[card], &gone, caps(), LATER);
        assert_eq!(result.summary.deleted, 1);
    }

    #[test]
    fn reconcile_resolves_parent_keys_in_both_orders_and_existing_ids() {
        let mut board = board();
        let parent = linked(&mut board);
        let mut child = remote("A-child");
        child.parent_key = Some("R-1".into());
        let pull = PullResult {
            cards: vec![child, remote("R-1")],
            ..PullResult::default()
        };
        let result = reconcile(&board, &[parent], &pull, caps(), NOW);
        let child = result
            .cards
            .iter()
            .find(|card| card.remote.as_ref().unwrap().key == "A-child")
            .unwrap();
        assert_eq!(child.parent_id.as_ref().unwrap().as_str(), "a");
        let result = reconcile(&board, &[], &pull, caps(), NOW);
        let parent = result
            .cards
            .iter()
            .find(|card| card.remote.as_ref().unwrap().key == "R-1")
            .unwrap();
        let child = result
            .cards
            .iter()
            .find(|card| card.remote.as_ref().unwrap().key == "A-child")
            .unwrap();
        assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
    }

    #[test]
    fn a_parent_arriving_later_links_even_when_the_child_did_not_change() {
        let board = board();
        let mut child = remote("A-child");
        child.parent_key = Some("R-1".into());
        let orphaned = reconcile(&board, &[], &pull(child.clone()), caps(), NOW);
        assert_eq!(orphaned.cards.len(), 1);
        assert_eq!(orphaned.cards[0].parent_id, None);
        // The same unchanged child, now pulled alongside its parent: the relationship
        // must resolve without any remote version bump to trigger an update.
        let both = PullResult {
            cards: vec![child, remote("R-1")],
            ..PullResult::default()
        };
        let result = reconcile(&board, &orphaned.cards, &both, caps(), LATER);
        let parent = result
            .cards
            .iter()
            .find(|card| card.remote.as_ref().unwrap().key == "R-1")
            .unwrap();
        let child = result
            .cards
            .iter()
            .find(|card| card.remote.as_ref().unwrap().key == "A-child")
            .unwrap();
        assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
    }

    #[test]
    fn reconcile_reports_unknown_status_and_preserves_cursor_on_empty_incremental() {
        let mut board = board();
        board.sync.cursor = Some("old".into());
        board.sync.last_error = Some("old error".into());
        let mut remote = remote("R-1");
        remote.status = status("mystery", "Mystery", None);
        let result = reconcile(&board, &[], &pull(remote), caps(), LATER);
        assert_eq!(result.summary.unmapped_statuses, ["mystery"]);
        assert_eq!(result.board.sync.cursor.as_deref(), Some("old"));
        assert_eq!(result.board.sync.last_synced_at.as_deref(), Some(LATER));
        assert!(result.board.sync.last_error.is_none());
        let full = reconcile(
            &board,
            &[],
            &PullResult {
                full: true,
                ..PullResult::default()
            },
            caps(),
            LATER,
        );
        assert!(full.board.sync.cursor.is_none());
    }

    #[test]
    fn reconcile_bad_new_remote_does_not_panic_or_consume_number() {
        let board = board();
        let mut remote = remote("bad");
        remote.title = " ".into();
        let result = reconcile(&board, &[], &pull(remote), caps(), NOW);
        assert!(result.cards.is_empty());
        assert_eq!(result.board.next_number, board.next_number);
        // The pull is reported as partially skipped, never turned into a board-wide failure
        // that would throw away everything else the same pull imported.
        assert!(result.board.sync.last_error.is_none());
        assert_eq!(result.summary.skipped.len(), 1);
        assert!(result.summary.skipped[0].starts_with("bad: "));
    }

    #[test]
    fn apply_remote_maps_status_by_explicit_map_before_name_and_category() {
        let mut board = board();
        let mut card = card(&mut board, "a");
        board
            .sync
            .status_map
            .remote_to_local
            .insert("remote-todo".into(), "done".parse().unwrap());
        apply_remote(&mut board, &mut card, &remote("R-1"), NOW);
        assert_eq!(card.status_id.as_str(), "done");
        board.sync.status_map = StatusMap::default();
        let mut remote = remote("R-1");
        remote.status.category = Some(StatusCategory::Started);
        apply_remote(&mut board, &mut card, &remote, NOW);
        assert_eq!(card.status_id.as_str(), "todo");
        remote.status.name = "Something".into();
        apply_remote(&mut board, &mut card, &remote, NOW);
        assert_eq!(card.status_id.as_str(), "in-progress");
    }

    #[test]
    fn apply_remote_unknown_status_keeps_local_and_records_reason() {
        let mut board = board();
        let mut card = card(&mut board, "a");
        let mut remote = remote("R-1");
        remote.status = status("weird", "Weird", None);
        apply_remote(&mut board, &mut card, &remote, LATER);
        assert_eq!(card.status_id.as_str(), "todo");
        assert!(
            card.activity
                .last()
                .unwrap()
                .message
                .contains("unmapped status weird")
        );
    }

    #[test]
    fn apply_remote_creates_missing_labels_with_unique_slug_ids() {
        let mut board = board();
        let mut card = card(&mut board, "a");
        let mut remote = remote("R-1");
        remote.labels = vec!["Bug!".into(), "Bug?".into(), "中文".into(), "Bug!".into()];
        apply_remote(&mut board, &mut card, &remote, NOW);
        assert_eq!(
            board
                .labels
                .iter()
                .map(|label| label.id.as_str())
                .collect::<Vec<_>>(),
            ["bug", "bug-2", "label"]
        );
        assert_eq!(card.labels.len(), 3);
        assert!(validate_card(&board, &card).is_ok());
        let before = board.clone();
        apply_remote(&mut board, &mut card, &remote, NOW);
        assert_eq!(board, before);
    }

    #[test]
    fn apply_remote_copies_fields_and_keeps_local_worktree_metadata() {
        let mut board = board();
        let mut card = card(&mut board, "a");
        card.repo_id = Some("org/repo".parse().unwrap());
        card.parent_id = Some("parent".parse().unwrap());
        card.position = 70;
        card.archived = true;
        let mut remote = remote("R-1");
        remote.description = "Details".into();
        remote.priority = Some(Priority::Urgent);
        remote.assignee = Some("A".into());
        remote.estimate = Some(8);
        remote.due_date = Some("2026-09-07".into());
        board
            .properties
            .push(property("a", PropertySource::Backend));
        remote
            .properties
            .insert("a".into(), PropertyValue::Text("value".into()));
        remote.url = Some("https://example.test/R-1".into());
        apply_remote(&mut board, &mut card, &remote, LATER);
        assert_eq!(card.description, "Details");
        assert_eq!(card.priority, Priority::Urgent);
        assert_eq!(card.assignee, remote.assignee);
        assert_eq!(card.estimate, remote.estimate);
        assert_eq!(card.due_date, remote.due_date);
        assert_eq!(card.properties, remote.properties);
        assert!(card.parent_id.is_none());
        assert_eq!(card.position, 70);
        assert!(card.archived);
        assert_eq!(card.repo_id.unwrap().as_str(), "org/repo");
        let link = card.remote.unwrap();
        assert_eq!(link.version, remote.version);
        assert_eq!(link.remote_updated_at, remote.updated_at);
        assert_eq!(link.url, remote.url);
        assert_eq!(link.synced_at, LATER);
        assert!(!card.dirty);
    }

    #[test]
    fn apply_remote_merges_comments_by_remote_id_and_preserves_unsent() {
        let mut board = board();
        let mut card = card(&mut board, "a");
        add_comment(&mut card, "remote:1".into(), None, "Unsent".into(), NOW).unwrap();
        let mut remote = remote("R-1");
        remote.comments = vec![comment("1")];
        apply_remote(&mut board, &mut card, &remote, NOW);
        assert_eq!(card.comments.len(), 2);
        assert_ne!(card.comments[0].id, card.comments[1].id);
        let local_id = card.comments[1].id.clone();
        remote.comments[0].body = "Edited".into();
        apply_remote(&mut board, &mut card, &remote, LATER);
        assert_eq!(card.comments.len(), 2);
        assert_eq!(card.comments[0].body, "Unsent");
        assert_eq!(card.comments[1].body, "Edited");
        assert_eq!(card.comments[1].id, local_id);
        let before = card.clone();
        apply_remote(&mut board, &mut card, &remote, LATER);
        assert_eq!(card, before);
    }

    #[test]
    fn apply_push_result_links_card_and_acknowledges_comment_ids() {
        let mut board = board();
        let mut card = card(&mut board, "a");
        add_comment(&mut card, "c".into(), None, "Hi".into(), NOW).unwrap();
        let mut cards = vec![card];
        apply_push_result(
            &mut cards,
            &PushResult {
                acks: vec![PushAck {
                    card_id: "a".parse().unwrap(),
                    key: "R-9".into(),
                    url: Some("url".into()),
                    version: Some("2".into()),
                    remote_updated_at: None,
                    comment_ids: vec![("c".into(), "remote-c".into())],
                }],
                failures: vec![],
            },
            "fake",
            &[],
            LATER,
        );
        assert!(!cards[0].dirty);
        assert_eq!(cards[0].remote.as_ref().unwrap().key, "R-9");
        assert_eq!(cards[0].remote.as_ref().unwrap().synced_at, LATER);
        assert_eq!(cards[0].comments[0].remote_id.as_deref(), Some("remote-c"));
        assert_eq!(cards[0].activity.last().unwrap().kind, ActivityKind::Synced);
    }

    #[test]
    fn a_transition_only_ack_keeps_fields_the_backend_never_accepted_dirty() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.status_id = "in-progress".parse().unwrap();
        card.dirty = true;
        board
            .sync
            .status_map
            .local_to_remote
            .insert(card.status_id.clone(), "remote-progress".into());
        let caps = BackendCapabilities {
            push_updates: false,
            ..caps()
        };
        let result = reconcile(&board, &[card], &pull(remote("R-1")), caps, LATER);
        // Only the transition can be sent; the retitle has nowhere to go.
        assert_eq!(
            result.to_push,
            [PushOp::Transition {
                card_id: "a".parse().unwrap(),
                remote_status: "remote-progress".into()
            }]
        );
        assert_eq!(result.unpushed, ["a".parse::<CardId>().unwrap()]);
        let mut cards = result.cards;
        apply_push_result(
            &mut cards,
            &PushResult {
                acks: vec![PushAck {
                    card_id: "a".parse().unwrap(),
                    key: "R-1".into(),
                    url: None,
                    version: Some("2".into()),
                    remote_updated_at: None,
                    comment_ids: vec![],
                }],
                failures: vec![],
            },
            "fake",
            &result.unpushed,
            LATER,
        );
        // Clearing dirty here would let the next pull overwrite the unsent title.
        assert!(cards[0].dirty);
        assert_eq!(cards[0].title, "Local");
    }

    #[test]
    fn push_partial_failure_keeps_dirty_and_records_error_after_ack() {
        let mut board = board();
        let card = linked(&mut board);
        let mut cards = vec![card];
        let result = PushResult {
            acks: vec![PushAck {
                card_id: "a".parse().unwrap(),
                key: "R-1".into(),
                url: None,
                version: Some("2".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![PushFailure {
                card_id: "a".parse().unwrap(),
                error: "Transition rejected".into(),
            }],
        };
        apply_push_result(&mut cards, &result, "fake", &[], LATER);
        assert!(cards[0].dirty);
        assert!(
            cards[0]
                .activity
                .last()
                .unwrap()
                .message
                .contains("Transition rejected")
        );
        // The ack carries a version, not the remote's own stamp: the last one a pull saw
        // stands, because dropping it leaves the card with no `remoteUpdatedAt` at all until
        // some later pull happens to change `version`.
        assert_eq!(
            cards[0]
                .remote
                .as_ref()
                .unwrap()
                .remote_updated_at
                .as_deref(),
            Some("2026-09-06T12:00:00Z")
        );
    }

    #[test]
    fn push_results_ignore_missing_card_and_comment_ids() {
        let mut board = board();
        let card = linked(&mut board);
        let mut cards = vec![card.clone()];
        apply_push_result(
            &mut cards,
            &PushResult {
                acks: vec![PushAck {
                    card_id: "missing".parse().unwrap(),
                    key: "R-1".into(),
                    url: None,
                    version: None,
                    remote_updated_at: None,
                    comment_ids: vec![],
                }],
                failures: vec![PushFailure {
                    card_id: "missing".parse().unwrap(),
                    error: "Oops".into(),
                }],
            },
            "fake",
            &[],
            NOW,
        );
        assert_eq!(cards, [card]);
    }

    #[test]
    fn resolve_keep_local_updates_baseline_and_avoids_repeat_conflict() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.title = "Local".into();
        let mut remote = remote("R-1");
        remote.version = Some("2".into());
        conflict(&mut card, remote.clone());
        resolve_conflict(&board, &mut card, ConflictResolution::KeepLocal, LATER).unwrap();
        assert_eq!(card.title, "Local");
        assert!(card.dirty);
        assert!(card.conflict.is_none());
        assert_eq!(
            card.activity.last().unwrap().kind,
            ActivityKind::ConflictResolved
        );
        let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
        assert_eq!(result.summary.conflicts, 0);
        assert_eq!(result.to_push.len(), 1);
    }

    /// The other half of "keep local": the fields the *remote* owns are not a side the user
    /// can hold. `differing_fields` already drops them from the conflict for that reason, and
    /// leaving them behind froze the local copy at a value no push could carry — the link this
    /// resolution stamps carries the remote's own version, so no later pull ever revisited it.
    #[test]
    fn resolve_keep_local_still_adopts_the_fields_the_remote_owns() {
        let mut board = board();
        board.sync.readonly_fields = vec!["priority".into(), "due_date".into()];
        board.properties.push(PropertySchema {
            key: "jira.issue_type".into(),
            name: "Issue type".into(),
            kind: PropertyKind::Select,
            options: vec![],
            editable: false,
            source: PropertySource::Backend,
            show_on_card: false,
        });
        let mut card = linked(&mut board);
        card.title = "Local".into();
        card.priority = Priority::Medium;
        card.properties.insert(
            "jira.issue_type".into(),
            PropertyValue::Select("Task".into()),
        );
        let mut remote = remote("R-1");
        remote.version = Some("2".into());
        remote.title = "Remote".into();
        remote.priority = Some(Priority::Urgent);
        remote.due_date = Some("2026-12-31".into());
        remote.properties.insert(
            "jira.issue_type".into(),
            PropertyValue::Select("Bug".into()),
        );
        conflict(&mut card, remote.clone());
        resolve_conflict(&board, &mut card, ConflictResolution::KeepLocal, LATER).unwrap();
        // The side the user chose to keep.
        assert_eq!(card.title, "Local");
        // The sides only the remote can write.
        assert_eq!(card.priority, Priority::Urgent);
        assert_eq!(card.due_date.as_deref(), Some("2026-12-31"));
        assert_eq!(
            card.properties.get("jira.issue_type"),
            Some(&PropertyValue::Select("Bug".into()))
        );
        // And no second conflict over any of them.
        let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
        assert_eq!(result.summary.conflicts, 0);
    }

    #[test]
    fn resolve_take_remote_applies_fields_and_clears_dirty() {
        let mut board = board();
        let mut card = linked(&mut board);
        card.title = "Local".into();
        let mut remote = remote("R-1");
        remote.title = "Remote".into();
        conflict(&mut card, remote);
        resolve_conflict(&board, &mut card, ConflictResolution::TakeRemote, LATER).unwrap();
        assert_eq!(card.title, "Remote");
        assert!(!card.dirty);
        assert!(card.conflict.is_none());
        assert_eq!(
            card.activity.last().unwrap().kind,
            ActivityKind::ConflictResolved
        );
    }

    #[test]
    fn resolve_missing_conflict_is_an_atomic_error() {
        let mut board = board();
        let mut card = linked(&mut board);
        let before = card.clone();
        assert!(resolve_conflict(&board, &mut card, ConflictResolution::KeepLocal, LATER).is_err());
        assert_eq!(card, before);
    }

    #[test]
    fn resolve_take_remote_requires_service_to_materialize_new_labels() {
        let mut board = board();
        let mut card = linked(&mut board);
        let mut remote = remote("R-1");
        remote.labels = vec!["New label".into()];
        conflict(&mut card, remote.clone());
        let before = card.clone();
        assert!(matches!(
            resolve_conflict(&board, &mut card, ConflictResolution::TakeRemote, LATER),
            Err(BoardError::UnknownLabel(_))
        ));
        assert_eq!(card, before);
        apply_remote(&mut board, &mut card.clone(), &remote, LATER);
        resolve_conflict(&board, &mut card, ConflictResolution::TakeRemote, LATER).unwrap();
        assert_eq!(card.labels[0].as_str(), "new-label");
    }

    #[test]
    fn local_backend_never_plans_remote_writes() {
        let mut board = board();
        board.backend = BackendRef::default();
        board.settings.push_new_cards = true;
        let card = card(&mut board, "a");
        assert!(
            reconcile(&board, &[card], &PullResult::default(), caps(), NOW)
                .to_push
                .is_empty()
        );
    }

    #[test]
    fn an_unimportable_remote_update_is_skipped_and_keeps_every_card_that_reconciled() {
        let mut board = board();
        let mut first = linked(&mut board);
        first.id = "a".parse().unwrap();
        let mut second = card(&mut board, "b");
        apply_remote(&mut board, &mut second, &remote("R-2"), NOW);
        let mut broken = remote("R-1");
        broken.title = "   ".into();
        broken.version = Some("2".into());
        let mut good = remote("R-2");
        good.title = "Renamed".into();
        good.version = Some("2".into());
        let result = reconcile(
            &board,
            &[first.clone(), second],
            &PullResult {
                cards: vec![broken, good],
                ..PullResult::default()
            },
            caps(),
            LATER,
        );
        assert_eq!(result.summary.skipped.len(), 1);
        assert!(result.summary.skipped[0].starts_with("R-1: "));
        // The refused remote leaves its card exactly as it was, and the other one still lands.
        assert_eq!(result.cards[0].title, first.title);
        assert_eq!(result.cards[1].title, "Renamed");
        assert_eq!(result.summary.updated, 1);
        for card in &result.cards {
            validate_card(&result.board, card).unwrap();
        }
    }

    #[test]
    fn an_impossible_remote_due_date_is_skipped_instead_of_wedging_the_pull() {
        let board = board();
        let mut broken = remote("R-9");
        broken.due_date = Some("2026-02-31".into());
        let result = reconcile(&board, &[], &pull(broken), caps(), NOW);
        assert!(result.cards.is_empty());
        assert_eq!(result.summary.created, 0);
        // The number the refused draft took must go back: the next pull creates it for real.
        assert_eq!(result.board.next_number, board.next_number);
        assert_eq!(result.summary.skipped.len(), 1);
        assert!(result.summary.skipped[0].starts_with("R-9: "));
    }

    #[test]
    fn a_remote_parent_key_that_closes_a_cycle_is_dropped() {
        let mut board = board();
        let first = linked(&mut board);
        let mut second = card(&mut board, "b");
        apply_remote(&mut board, &mut second, &remote("R-2"), NOW);
        let mut own = remote("R-1");
        own.parent_key = Some("R-1".into());
        own.version = Some("2".into());
        let result = reconcile(
            &board,
            std::slice::from_ref(&first),
            &pull(own),
            caps(),
            LATER,
        );
        assert_eq!(result.cards[0].parent_id, None);

        let mut one = remote("R-1");
        one.parent_key = Some("R-2".into());
        one.version = Some("2".into());
        let mut two = remote("R-2");
        two.parent_key = Some("R-1".into());
        two.version = Some("2".into());
        let result = reconcile(
            &board,
            &[first, second],
            &PullResult {
                cards: vec![one, two],
                ..PullResult::default()
            },
            caps(),
            LATER,
        );
        assert!(result.cards.iter().all(|card| card.parent_id.is_none()));
    }
}
