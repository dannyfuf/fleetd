//! Board documents and backend-independent card data.
use super::{
    ops::{default_branch_template, default_true, is_zero},
    property::*,
    sync::RemoteCard,
};
use crate::{
    agents::{AgentKind, DelegationId, DelegationStatus, PermissionMode, ThreadId},
    ids::{BoardId, CardId, ContextId, LabelId, RepoId, StatusId, WorktreeId},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
/// A context-scoped board and its optional worktree scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    /// Id.
    pub id: BoardId,
    /// Context id.
    pub context_id: ContextId,
    /// Worktree id when this board is scoped to one worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<WorktreeId>,
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
    /// What this column does to a card that enters it. `None` on a column that runs nothing,
    /// which is every column of every board until someone opts in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<ColumnAutomation>,
}

/// What a column does to a card entering it, and where the card goes next.
///
/// Every field is optional, and an all-default block is not automation: `ops::patches::
/// normalise_automation` turns one into `None` so a column a user emptied stops answering
/// `is_some()` and stops holding the document at version 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ColumnAutomation {
    /// Run this when a card enters the column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_enter: Option<Action>,
    /// Move the card here when its run succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success: Option<StatusId>,
    /// Move a card waiting here once every card blocking it is satisfied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advance_when_unblocked: Option<StatusId>,
}
impl ColumnAutomation {
    /// Whether this block asks for nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// What a column runs, and what it tells the run about the card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// Prompt or skill.
    pub kind: ActionKind,
    /// Prepended to the brief. Markdown. `{key}` and `{title}` are substituted.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    /// Printed in the footer as `The card expects: …`. May be empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expect: String,
    /// Which agent the column asks for; a card's own preference wins over it.
    #[serde(default, skip_serializing_if = "ColumnAgentPrefs::is_empty")]
    pub agent: ColumnAgentPrefs,
    /// `KEY=VALUE`; `{key}` substituted in the value. The five `--env` refusals apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
}

/// The two things a column can run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind {
    /// Run the card itself: instructions, then the card.
    Prompt,
    /// Invoke one of the agent's skills.
    Skill {
        /// Skill name, as the agent spells it.
        name: String,
        /// Arguments passed to the skill.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        args: String,
    },
}
impl ActionKind {
    /// How a surface names this action in a sentence: `run card`, `run skill deep-review`.
    #[must_use]
    pub fn word(&self) -> String {
        match self {
            Self::Prompt => "run card".to_owned(),
            Self::Skill { name, .. } => format!("run skill {name}"),
        }
    }
}

/// The agent a column asks for. Empty means "whatever the card or the daemon default says".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ColumnAgentPrefs {
    /// Provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentKind>,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Permission mode is a workflow policy, so it lives on the column and never on a card.
    /// `None` runs the column's action with `full_access`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
}
impl ColumnAgentPrefs {
    /// Whether this block asks for nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
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
    /// The agent this card asks for, overriding what its column asks for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<CardAgentPrefs>,
    /// Cards that must reach a `Completed` column before this one may start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<CardId>,
    /// A run this card is waiting for a free slot to start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_run: Option<PendingRun>,
    /// Runs this card has had, oldest first, capped at [`MAX_RUNS_PER_CARD`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<CardRun>,
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
        "agent" => "Agent",
        "blocked_by" => "Blocked by",
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

/// The agent a card asks for, whatever its column asks for.
///
/// A card carries no permission mode: the mode is the column's policy over every card that
/// passes through it, and a card able to widen it would be a card able to grant itself access
/// its column deliberately withheld.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardAgentPrefs {
    /// Provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentKind>,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}
impl CardAgentPrefs {
    /// Whether this block asks for nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// A run the board owes this card as soon as a slot frees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRun {
    /// The column whose action is owed.
    pub status_id: StatusId,
    /// RFC 3339, on the same clock as the `now` every board write is stamped with.
    pub since: String,
}

/// One run of one card: its identity, what it was asked to do, and how it ended.
///
/// Written twice — once when the delegation exists, once when it ends — so a run is visible
/// while it works and legible long after. Live progress is never stored here: it belongs to
/// the delegation, and a card is not a mirror of one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardRun {
    /// The delegation id, or a freshly minted id for a run whose start failed.
    pub id: DelegationId,
    /// `None` only when the start failed before a thread existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    /// The column that started it.
    pub status_id: StatusId,
    /// What that column asked for.
    pub action: ActionKind,
    /// Provider.
    pub provider: AgentKind,
    /// Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Started at.
    pub started_at: String,
    /// Ended at; `None` while the run is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// How it ended; `None` while the run is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
    /// The reported sentence behind the outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The comment holding this run's report excerpt, while the card still keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_comment_id: Option<String>,
    /// Files the run changed in the worktree.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub files_changed: u32,
    /// Cost in US dollars, when the provider reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Tokens, when the provider reported them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
}
impl CardRun {
    /// Whether this run has not ended yet.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.outcome.is_none()
    }
    /// Whether this run never reached a thread, so there is nothing to attach to.
    #[must_use]
    pub fn failed_to_start(&self) -> bool {
        self.thread_id.is_none()
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// The run reported a finished, verified result.
    Succeeded,
    /// The run reported that it is blocked and needs a human.
    NeedsYou,
    /// The run failed.
    Failed,
    /// The run stopped without reporting.
    Incomplete,
    /// The run was cancelled.
    Cancelled,
}
impl RunOutcome {
    /// How every surface says this outcome.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::NeedsYou => "needs you",
            Self::Failed => "failed",
            Self::Incomplete => "incomplete",
            Self::Cancelled => "cancelled",
        }
    }
    /// Whether a card whose last run ended this way is waiting on a human.
    #[must_use]
    pub const fn needs_attention(self) -> bool {
        matches!(self, Self::NeedsYou | Self::Failed | Self::Incomplete)
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
    /// Set on a run's report excerpt; the comment renders with a run badge, not an author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<DelegationId>,
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
    /// A run started on this card.
    RunStarted,
    /// A run on this card ended.
    RunEnded,
    /// Automation moved this card; a `Moved` entry is only ever a human's or the CLI's move.
    AutoMoved,
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
    /// Live runs allowed at once across this board. `None` is one.
    ///
    /// The runs of one board share one checkout, so this is a property of the board rather
    /// than of a column: two columns each running a card would still have them editing the
    /// same worktree. Validated `1..=MAX_LIVE_RUNS_PER_BOARD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_live_runs: Option<u32>,
}
impl BoardSettings {
    /// Live runs allowed at once across this board; one when unset.
    #[must_use]
    pub fn max_live_runs(&self) -> u32 {
        self.max_live_runs.unwrap_or(1)
    }
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
    /// Worktree id when this board is scoped to one worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<WorktreeId>,
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
    /// Cards with a live or pending run.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub working_count: u32,
    /// Cards whose last run is waiting on a human.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attention_count: u32,
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
    /// Joined from the delegation store on read. Never persisted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_runs: Vec<LiveRun>,
}

/// What a live run is doing right now, joined onto a board view on read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRun {
    /// The card being run.
    pub card_id: CardId,
    /// The delegation serving it.
    pub run: DelegationId,
    /// Starting, Running, Blocked or Settling; a terminal status is not a live run.
    pub status: DelegationStatus,
    /// The child's latest headline, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    /// Started at.
    pub started: String,
}

/// On-disk document: `$FLEET_HOME/boards/<board-id>.json`.
/// Versioned board persistence document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardDocument {
    /// Version; stamped by the store from [`document_version`] on every save.
    pub version: u32,
    /// Board.
    pub board: Board,
    /// Cards.
    pub cards: Vec<Card>,
}
/// Newest board persistence schema version this build writes.
pub const BOARD_DOCUMENT_VERSION: u32 = 2;
/// Oldest board persistence schema version this build reads.
pub const BOARD_DOCUMENT_MIN_VERSION: u32 = 1;

/// The version a document holding this board and these cards must be written at.
///
/// The bump is lazy on purpose: a board that never opts into automation keeps writing 1, so a
/// daemon from before this feature goes on reading it. A board that has opted in keeps writing
/// 2 for as long as any card still carries a link or a run — an older daemon cannot represent
/// either, and would silently drop the history on its next save.
#[must_use]
pub fn document_version(board: &Board, cards: &[Card]) -> u32 {
    let board_opted_in = board.settings.max_live_runs.is_some()
        || board.statuses.iter().any(|s| s.automation.is_some());
    let cards_opted_in = cards.iter().any(|card| {
        card.agent.is_some()
            || !card.blocked_by.is_empty()
            || card.pending_run.is_some()
            || !card.runs.is_empty()
            || card.comments.iter().any(|c| c.run_id.is_some())
    });
    if board_opted_in || cards_opted_in {
        BOARD_DOCUMENT_VERSION
    } else {
        BOARD_DOCUMENT_MIN_VERSION
    }
}

impl Default for BoardSettings {
    fn default() -> Self {
        Self {
            start_on_worktree: default_true(),
            branch_template: default_branch_template(),
            conflict_policy: ConflictPolicy::default(),
            push_new_cards: false,
            max_live_runs: None,
        }
    }
}

/// Runs kept on one card; the oldest is dropped once a new one passes it.
pub const MAX_RUNS_PER_CARD: usize = 20;
/// Run report comments kept on one card; the oldest is dropped once a new one passes it.
pub const MAX_REPORT_COMMENTS_PER_CARD: usize = 3;
/// Ceiling on the run report excerpt copied onto a card as a comment.
pub const REPORT_EXCERPT_CAP_BYTES: usize = 8 * 1024;
/// How long a run may sit pending before every surface marks it stalled.
pub const PENDING_AMBER_AFTER_SECS: u64 = 60;
/// Ceiling on `BoardSettings::max_live_runs`.
///
/// This equals the daemon's `MAX_LIVE_DELEGATIONS`: a board allowed more live runs than the
/// daemon will hold delegations would queue work the daemon then refuses. A daemon-side test
/// asserts the two agree.
pub const MAX_LIVE_RUNS_PER_BOARD: u32 = 8;
