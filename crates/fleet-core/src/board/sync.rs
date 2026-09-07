//! Pure backend reconciliation contracts.

use super::{
    defaults::default_statuses,
    model::{
        ActivityKind, BackendRef, Board, Card, Comment, Conflict, ConflictPolicy, Label, Priority,
        RemoteLink, Status, StatusCategory, StatusMap,
    },
    ops::{BoardError, CardDraft, create_card, push_activity, valid_date, validate_card},
    property::{PropertySchema, PropertySource, PropertyValue},
};
use crate::{
    ids::{CardId, LabelId, StatusId},
    slug::normalize_context_id,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod apply;
mod push;
mod reconcile;
mod schema;
#[cfg(test)]
mod tests;

pub use apply::{apply_push_result, apply_remote, resolve_conflict};
pub use reconcile::reconcile;
pub use schema::{adopt_schema, remote_category, remote_status_key};

use apply::{archived_by_sync, link_changed, merge_comments, refresh_link, remote_changed};
use push::{differing_fields, plan_push};
use schema::{ensure_label, mapped_status};

/// What a card's activity says when a pull could not find its remote any more.
///
/// The sentence is the provenance: an archive this string explains belongs to the sync and is
/// undone when the remote comes back, while one a user made stands until they undo it.
pub const ARCHIVED_BY_SYNC: &str = "Remote card deleted; archived locally";
/// What it says when the remote came back and the sync undid its own archive.
pub const RESTORED_BY_SYNC: &str = "Remote card is back; unarchived locally";

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

/// The selected side of a conflicted card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    /// Keep local.
    KeepLocal,
    /// Take remote.
    TakeRemote,
}
