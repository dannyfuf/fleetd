//! Board documents and backend-independent card data.
use super::{
    ops::{default_branch_template, default_true},
    property::*,
    sync::RemoteCard,
};
use crate::ids::{BoardId, CardId, ContextId, LabelId, RepoId, StatusId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
/// A context-scoped board and its backend configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    /// Id.
    pub id: BoardId,
    /// Context id.
    pub context_id: ContextId,
    /// Name.
    pub name: String,
    /// Identifier prefix; `FLT` → `FLT-12`. Uppercase, 1..=8 chars, [A-Z0-9].
    pub prefix: String,
    /// Next number.
    pub next_number: u64,
    /// Backend.
    #[serde(default)]
    pub backend: BackendRef,
    /// Statuses.
    pub statuses: Vec<Status>, // ordered = column order
    /// Labels.
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Properties.
    #[serde(default)]
    pub properties: Vec<PropertySchema>,
    /// Default repo id.
    #[serde(default)]
    pub default_repo_id: Option<RepoId>,
    /// Settings.
    #[serde(default)]
    pub settings: BoardSettings,
    /// Sync.
    #[serde(default)]
    pub sync: SyncState,
    /// Created at.
    pub created_at: String,
    /// Updated at.
    pub updated_at: String,
}

/// Which backend mirrors this board. `kind` is a registry key ("local", "jira", "notion").
/// `settings` is backend-owned JSON, deserialized by the backend into its own typed struct.
/// Backend registry key and backend-owned settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendRef {
    /// Kind.
    pub kind: String,
    /// Settings.
    #[serde(default)]
    pub settings: serde_json::Value,
}
impl Default for BackendRef {
    fn default() -> Self {
        Self {
            kind: "local".into(),
            settings: serde_json::Value::Null,
        }
    }
}
impl BackendRef {
    /// Built-in local backend registry key.
    pub const LOCAL: &'static str = "local";
    /// Whether this board has no remote backend.
    pub fn is_local(&self) -> bool {
        self.kind == Self::LOCAL
    }
}

/// An ordered board column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Id.
    pub id: StatusId,
    /// Name.
    pub name: String,
    /// Category.
    pub category: StatusCategory,
    /// Color.
    #[serde(default)]
    pub color: Option<String>, /* token name, e.g. "accent" */
}

/// Backend-independent lifecycle category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCategory {
    /// Backlog.
    Backlog,
    /// Unstarted.
    Unstarted,
    /// Started.
    Started,
    /// Completed.
    Completed,
    /// Canceled.
    Canceled,
}

/// A reusable card label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    /// Id.
    pub id: LabelId,
    /// Name.
    pub name: String,
    /// Color.
    #[serde(default)]
    pub color: Option<String>,
}

/// Linear ordering: lower value sorts first on the board.
/// Card urgency ordered from highest to lowest.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Urgent.
    Urgent,
    /// High.
    High,
    /// Medium.
    Medium,
    /// Low.
    Low,
    /// None.
    #[default]
    None,
}
impl Priority {
    /// Priorities in board sort order.
    pub const ALL: [Priority; 5] = [
        Self::Urgent,
        Self::High,
        Self::Medium,
        Self::Low,
        Self::None,
    ];
    /// Human-readable priority name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Urgent => "Urgent",
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
            Self::None => "None",
        }
    }
    /// Compact priority glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Urgent => "!!!",
            Self::High => "!!",
            Self::Medium => "!",
            Self::Low => "·",
            Self::None => "",
        }
    }
}

/// A tracked task with local and optional remote identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    /// Id.
    pub id: CardId,
    /// Board id.
    pub board_id: BoardId,
    /// Number.
    pub number: u64, // local identifier number
    /// Title.
    pub title: String,
    /// Description.
    #[serde(default)]
    pub description: String, // markdown
    /// Status id.
    pub status_id: StatusId,
    /// Priority.
    #[serde(default)]
    pub priority: Priority,
    /// Labels.
    #[serde(default)]
    pub labels: Vec<LabelId>,
    /// Assignee.
    #[serde(default)]
    pub assignee: Option<String>,
    /// Estimate.
    #[serde(default)]
    pub estimate: Option<u32>,
    /// Due date.
    #[serde(default)]
    pub due_date: Option<String>, // YYYY-MM-DD
    /// Parent id.
    #[serde(default)]
    pub parent_id: Option<CardId>,
    /// Repo id.
    #[serde(default)]
    pub repo_id: Option<RepoId>,
    /// Worktree id.
    #[serde(default)]
    pub worktree_id: Option<WorktreeId>,
    /// Properties.
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
    /// Comments.
    #[serde(default)]
    pub comments: Vec<Comment>,
    /// Activity.
    #[serde(default)]
    pub activity: Vec<Activity>, // capped at 200, oldest dropped
    /// Remote.
    #[serde(default)]
    pub remote: Option<RemoteLink>,
    /// Conflict.
    #[serde(default)]
    pub conflict: Option<Conflict>,
    /// Local edits not yet pushed to the remote (always false on local boards).
    #[serde(default)]
    pub dirty: bool,
    /// Archived.
    #[serde(default)]
    pub archived: bool,
    /// Sort key inside its column; renumbered by `ops::move_card`.
    #[serde(default)]
    pub position: u64,
    /// Created at.
    pub created_at: String,
    /// Updated at.
    pub updated_at: String,
}
/// A card field as every surface names it, never as the wire spells it.
///
/// `differing_fields` answers in model field names — `status_id`, `due_date`, `parent_id` — and
/// a conflict reported as "status_id, due_date" is a sentence the reader has to translate. The
/// app's conflict banner and the CLI's card report print the same list, so they read it here.
#[must_use]
pub fn field_label(field: &str) -> &str {
    match field {
        "title" => "Title",
        "description" => "Description",
        "status_id" => "Status",
        "priority" => "Priority",
        "assignee" => "Assignee",
        "labels" => "Labels",
        "estimate" => "Estimate",
        "due_date" => "Due",
        "parent_id" => "Parent",
        "properties" => "Properties",
        other => other,
    }
}

impl Card {
    /// `remote.key` when linked (e.g. "PROJ-123"), else `"{prefix}-{number}"`.
    pub fn display_key(&self, board: &Board) -> String {
        self.remote
            .as_ref()
            .map_or_else(|| self.local_key(board), |remote| remote.key.clone())
    }
    /// Local identifier formed from the board prefix and card number.
    pub fn local_key(&self, board: &Board) -> String {
        format!("{}-{}", board.prefix, self.number)
    }
}

/// A timestamped markdown comment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// Id.
    pub id: String,
    /// Author.
    #[serde(default)]
    pub author: Option<String>,
    /// Body.
    pub body: String,
    /// Created at.
    pub created_at: String,
    /// Remote id.
    #[serde(default)]
    pub remote_id: Option<String>,
}

/// One timestamped card activity entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// At.
    pub at: String,
    /// Kind.
    pub kind: ActivityKind,
    /// Actor.
    #[serde(default)]
    pub actor: Option<String>,
    /// Message.
    pub message: String,
}
/// The action recorded by an activity entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// Created.
    Created,
    /// Updated.
    Updated,
    /// Moved.
    Moved,
    /// Commented.
    Commented,
    /// Worktree created.
    WorktreeCreated,
    /// Synced.
    Synced,
    /// Conflict detected.
    ConflictDetected,
    /// Conflict resolved.
    ConflictResolved,
}

/// Identity and synchronization metadata for a linked remote card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLink {
    /// Remote parent identity, retained until its local card is available.
    #[serde(default)]
    pub parent_key: Option<String>,
    /// Backend.
    pub backend: String,
    /// Key.
    pub key: String,
    /// Url.
    #[serde(default)]
    pub url: Option<String>,
    /// Version.
    #[serde(default)]
    pub version: Option<String>,
    /// Synced at.
    pub synced_at: String,
    /// Remote updated at.
    #[serde(default)]
    pub remote_updated_at: Option<String>,
}

/// Remote state requiring a local conflict resolution decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    /// Detected at.
    pub detected_at: String,
    /// Remote.
    pub remote: RemoteCard,
    /// Fields.
    pub fields: Vec<String>, /* field names that differ */
}

/// Board behavior and synchronization preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSettings {
    /// Move a Backlog/Unstarted card to the first `Started` status when a worktree is created from it.
    #[serde(default = "default_true")]
    pub start_on_worktree: bool,
    /// `{key}` `{slug}` placeholders; default "{key}-{slug}" → "flt-12-fix-login".
    #[serde(default = "default_branch_template")]
    pub branch_template: String,
    /// Conflict policy.
    #[serde(default)]
    pub conflict_policy: ConflictPolicy,
    /// Create remote issues for local-only cards on push (only if backend supports it).
    #[serde(default)]
    pub push_new_cards: bool,
}
/// Automatic or manual handling of concurrent local and remote edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Manual.
    #[default]
    Manual,
    /// Remote wins.
    RemoteWins,
    /// Local wins.
    LocalWins,
}

/// Last synchronization result and remote cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    /// Last synced at.
    #[serde(default)]
    pub last_synced_at: Option<String>,
    /// Cursor.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Last error.
    #[serde(default)]
    pub last_error: Option<String>,
    /// remote status id/name → local StatusId, and the inverse, kept by `sync::adopt_schema`.
    #[serde(default)]
    pub status_map: StatusMap,
    /// Standard card fields this board's backend cannot write back, copied from
    /// `BackendSchema::readonly_fields` by `sync::adopt_schema`. Local edits to them are
    /// refused by `ops::apply_card_patch` and `ops::move_card`.
    #[serde(default)]
    pub readonly_fields: Vec<String>,
}
/// Bidirectional mapping of remote and local statuses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatusMap {
    /// Remote to local.
    pub remote_to_local: BTreeMap<String, StatusId>,
    /// Local to remote.
    pub local_to_remote: BTreeMap<StatusId, String>,
}

/// Lightweight row for `Snapshot.boards`.
/// Lightweight board counts and synchronization state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSummary {
    /// Id.
    pub id: BoardId,
    /// Context id.
    pub context_id: ContextId,
    /// Name.
    pub name: String,
    /// Prefix.
    pub prefix: String,
    /// Backend kind.
    pub backend_kind: String,
    /// Card count.
    pub card_count: usize,
    /// Open count.
    pub open_count: usize,
    /// Dirty count.
    pub dirty_count: usize,
    /// Conflict count.
    pub conflict_count: usize,
    /// Last synced at.
    pub last_synced_at: Option<String>,
    /// Last error.
    pub last_error: Option<String>,
}

/// Full board payload for the UI/CLI.
/// Complete board payload for clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardView {
    /// Board.
    pub board: Board,
    /// Cards.
    pub cards: Vec<Card>,
}

/// On-disk document: `$FLEET_HOME/boards/<board-id>.json`.
/// Versioned board persistence document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardDocument {
    /// Version.
    pub version: u32, /* = 1 */
    /// Board.
    pub board: Board,
    /// Cards.
    pub cards: Vec<Card>,
}
/// Current board persistence schema version.
pub const BOARD_DOCUMENT_VERSION: u32 = 1;

impl Default for BoardSettings {
    fn default() -> Self {
        Self {
            start_on_worktree: default_true(),
            branch_template: default_branch_template(),
            conflict_policy: ConflictPolicy::default(),
            push_new_cards: false,
        }
    }
}
