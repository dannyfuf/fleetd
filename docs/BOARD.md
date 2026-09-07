# Board — contracts and architecture

**Status: authoritative.** This document owns the board's domain model, its reconciliation engine,
its persistence, its wire messages and its surface. The signatures below are the seams every other
crate is written against; changing one is a deliberate, workspace-wide change, and this document
changes in the same pass. Where the code and this file disagree, the code is the bug.

The decisions behind the shape — a backend-agnostic core with a pure reconciliation engine, no
markdown crate, and the protocol bump the board's messages needed — are recorded in
`docs/decisions/0008-board-model-and-sync.md`.

## 0. What we are building

A Linear-style kanban **board of cards**, one board per **context** (`fleet_core::Context`). Cards
are the unit of project tracking: identifier (`FLT-12`), title, markdown description, status
column, priority, labels, assignee, estimate, due date, parent, custom properties, comments,
activity. A card can **spawn a worktree** (the existing prepared-copy pipeline) and remembers it.

The **core is backend-agnostic and reusable**. A `BoardBackend` adapter (daemon side) plus a
**pure reconciliation engine** (core side) let a board mirror a remote system — Jira via `acli`,
Notion, anything — without the core knowing their shape. Backend-specific fields ride in
`Card.properties`, described by `PropertySchema` so the generic UI can render/edit them. The first
backend is `local` (no remote). Jira is the second (separate contract, later).

Non-goals for v1: multiple boards per context in the UI (the model allows it, the UI shows the
context's first board), cycles/projects/milestones, attachments, rich-text editing beyond a plain
multi-line editor with a read-mode markdown renderer.

## 1. Crate placement

| Piece | Crate / path |
|---|---|
| Domain types, ids, pure ops, sync engine | `crates/fleet-core/src/board.rs` and `board/{defaults,model,property,ops,sync}.rs` |
| Ids `BoardId CardId StatusId LabelId` | `crates/fleet-core/src/ids.rs` (via `string_id!`) |
| Paths `boards_dir()`, `board_path()` | `crates/fleet-core/src/paths.rs` |
| Wire types (requests/responses/events/snapshot) | `crates/fleet-proto/src/{request,response,event,snapshot}.rs` |
| Backend trait + registry + local backend | `crates/fleet-daemon/src/adapters/board.rs` and `adapters/board/local.rs` |
| Board store (per-board JSON document) | `crates/fleet-daemon/src/stores/board.rs` |
| `Boards` service + sync job + worktree-from-card | `crates/fleet-daemon/src/services/boards.rs` and `services/boards/{cards,documents,lifecycle,sync,worktree}.rs` |
| Dispatch arms | `crates/fleet-daemon/src/services/mod.rs` |
| Client API | `crates/fleet-client/src/api/boards.rs` |
| CLI `fleet board …` | `crates/fleet-cli/src/{args.rs,commands.rs,envelope.rs,human.rs,commands/board.rs}` |
| UI-kit components | `crates/fleet-ui-kit/src/components/{card_tile,kanban_column,markdown_text,priority_glyph,text_area}.rs` |
| App screen, views, dialogs, state, keymap | `crates/fleet-app/src/{screens/board.rs,views/board_*.rs,dialogs/card_*.rs,dialogs/board_settings.rs,state/board.rs,shell/root/board.rs,keymap.rs,actions.rs,dialogs/palette.rs}` |
| Docs | `docs/{BOARD,ARCHITECTURE,APP-CONTRACTS,KEYMAP,UX-SPEC,DESIGN-SYSTEM}.md` |

The board adds no workspace dependency: `async-trait` already exists in `fleet-daemon`, and the
markdown reader and writer are hand-written on purpose
(`docs/decisions/0008-board-model-and-sync.md`).
Timestamps are RFC3339 strings on the wire, as `Context.created_at` is, produced daemon-side through
the `Clock` adapter so `fleet-core` stays free of clocks. Dates (`due_date`) are `YYYY-MM-DD`
strings.

## 2. Domain model (`fleet_core::board`)

```rust
// crates/fleet-core/src/ids.rs — add with the existing string_id! macro
string_id!(BoardId,  "board",  validate_slug);        // e.g. "work", same charset as ContextId
string_id!(CardId,   "card",   validate_opaque_id);   // uuid v4 text; non-empty, no whitespace, ≤ 64
string_id!(StatusId, "status", validate_slug);        // "in-progress"
string_id!(LabelId,  "label",  validate_slug);        // "bug"

// crates/fleet-core/src/board/model.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub id: BoardId,
    pub context_id: ContextId,
    pub name: String,
    /// Identifier prefix; `FLT` → `FLT-12`. Uppercase, 1..=8 chars, [A-Z0-9].
    pub prefix: String,
    pub next_number: u64,
    #[serde(default)] pub backend: BackendRef,
    pub statuses: Vec<Status>,                 // ordered = column order
    #[serde(default)] pub labels: Vec<Label>,
    #[serde(default)] pub properties: Vec<PropertySchema>,
    #[serde(default)] pub default_repo_id: Option<RepoId>,
    #[serde(default)] pub settings: BoardSettings,
    #[serde(default)] pub sync: SyncState,
    pub created_at: String,
    pub updated_at: String,
}

/// Which backend mirrors this board. `kind` is a registry key ("local", "jira", "notion").
/// `settings` is backend-owned JSON, deserialized by the backend into its own typed struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendRef { pub kind: String, #[serde(default)] pub settings: serde_json::Value }
impl Default for BackendRef { fn default() -> Self { Self { kind: "local".into(), settings: Value::Null } } }
impl BackendRef { pub const LOCAL: &'static str = "local"; pub fn is_local(&self) -> bool; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status { pub id: StatusId, pub name: String, pub category: StatusCategory,
                    #[serde(default)] pub color: Option<String> /* token name, e.g. "accent" */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCategory { Backlog, Unstarted, Started, Completed, Canceled }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label { pub id: LabelId, pub name: String, #[serde(default)] pub color: Option<String> }

/// Linear ordering: lower value sorts first on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Priority { Urgent, High, Medium, Low, #[default] None }
impl Priority { pub const ALL: [Priority; 5]; pub fn label(self) -> &'static str; pub fn glyph(self) -> &'static str /* "!!!" "!!" "!" "·" "" */; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: CardId,
    pub board_id: BoardId,
    pub number: u64,                                    // local identifier number
    pub title: String,
    #[serde(default)] pub description: String,         // markdown
    pub status_id: StatusId,
    #[serde(default)] pub priority: Priority,
    #[serde(default)] pub labels: Vec<LabelId>,
    #[serde(default)] pub assignee: Option<String>,
    #[serde(default)] pub estimate: Option<u32>,
    #[serde(default)] pub due_date: Option<String>,    // YYYY-MM-DD
    #[serde(default)] pub parent_id: Option<CardId>,
    #[serde(default)] pub repo_id: Option<RepoId>,
    #[serde(default)] pub worktree_id: Option<WorktreeId>,
    #[serde(default)] pub properties: BTreeMap<String, PropertyValue>,
    #[serde(default)] pub comments: Vec<Comment>,
    #[serde(default)] pub activity: Vec<Activity>,     // capped at 200, oldest dropped
    #[serde(default)] pub remote: Option<RemoteLink>,
    #[serde(default)] pub conflict: Option<Conflict>,
    /// Local edits not yet pushed to the remote (always false on local boards).
    #[serde(default)] pub dirty: bool,
    #[serde(default)] pub archived: bool,
    /// Sort key inside its column; renumbered by `ops::move_card`.
    #[serde(default)] pub position: u64,
    pub created_at: String,
    pub updated_at: String,
}
impl Card {
    /// `remote.key` when linked (e.g. "PROJ-123"), else `"{prefix}-{number}"`.
    pub fn display_key(&self, board: &Board) -> String;
    pub fn local_key(&self, board: &Board) -> String;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment { pub id: String, #[serde(default)] pub author: Option<String>, pub body: String,
                     pub created_at: String, #[serde(default)] pub remote_id: Option<String> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity { pub at: String, pub kind: ActivityKind, #[serde(default)] pub actor: Option<String>, pub message: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind { Created, Updated, Moved, Commented, WorktreeCreated, Synced, ConflictDetected, ConflictResolved }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLink { pub backend: String, pub key: String, #[serde(default)] pub url: Option<String>,
                        #[serde(default)] pub version: Option<String>, pub synced_at: String,
                        #[serde(default)] pub remote_updated_at: Option<String> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict { pub detected_at: String, pub remote: RemoteCard, pub fields: Vec<String> /* field names that differ */ }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSettings {
    /// Move a Backlog/Unstarted card to the first `Started` status when a worktree is created from it.
    #[serde(default = "default_true")] pub start_on_worktree: bool,
    /// `{key}` `{slug}` placeholders; default "{key}-{slug}" → "flt-12-fix-login".
    #[serde(default = "default_branch_template")] pub branch_template: String,
    #[serde(default)] pub conflict_policy: ConflictPolicy,
    /// Create remote issues for local-only cards on push (only if backend supports it).
    #[serde(default)] pub push_new_cards: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy { #[default] Manual, RemoteWins, LocalWins }
// BoardSettings implements Default explicitly to match serde defaults:
// start_on_worktree = true, branch_template = "{key}-{slug}", Manual, push_new_cards = false.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    #[serde(default)] pub last_synced_at: Option<String>,
    #[serde(default)] pub cursor: Option<String>,
    #[serde(default)] pub last_error: Option<String>,
    /// remote status id/name → local StatusId, and the inverse, kept by `sync::adopt_schema`.
    #[serde(default)] pub status_map: StatusMap,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatusMap { pub remote_to_local: BTreeMap<String, StatusId>, pub local_to_remote: BTreeMap<StatusId, String> }

/// Lightweight row for `Snapshot.boards`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSummary { pub id: BoardId, pub context_id: ContextId, pub name: String, pub prefix: String,
                          pub backend_kind: String, pub card_count: usize, pub open_count: usize,
                          pub dirty_count: usize, pub conflict_count: usize,
                          pub last_synced_at: Option<String>, pub last_error: Option<String> }

/// Full board payload for the UI/CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardView { pub board: Board, pub cards: Vec<Card> }

/// On-disk document: `$FLEET_HOME/boards/<board-id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardDocument { pub version: u32 /* = 1 */, pub board: Board, pub cards: Vec<Card> }
pub const BOARD_DOCUMENT_VERSION: u32 = 1;
```

```rust
// crates/fleet-core/src/board/property.rs — backend-agnostic custom properties
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertySchema { pub key: String, pub name: String, pub kind: PropertyKind,
                            #[serde(default)] pub options: Vec<PropertyOption>,
                            #[serde(default = "default_true")] pub editable: bool,
                            #[serde(default)] pub source: PropertySource,
                            #[serde(default)] pub show_on_card: bool }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyOption { pub value: String, pub label: String, #[serde(default)] pub color: Option<String> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyKind { Text, Number, Bool, Date, Select, MultiSelect, User, Url }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PropertySource { #[default] Local, Backend }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PropertyValue { Text(String), Number(f64), Bool(bool), Date(String), Select(String),
                         MultiSelect(Vec<String>), User(String), Url(String), Null }
impl PropertyValue { pub fn display(&self) -> String; pub fn matches_kind(&self, kind: PropertyKind) -> bool; }
```

```rust
// crates/fleet-core/src/board/defaults.rs
pub fn default_statuses() -> Vec<Status>;   // backlog/Backlog, todo/Unstarted, in-progress/Started, done/Completed, canceled/Canceled
pub fn default_prefix(context: &Context) -> String;   // first 3 alnum chars of context name uppercased, fallback "FLT"
pub fn new_board(context: &Context, now: &str) -> Board; // id = context.id as BoardId, name = context.name, default statuses, next_number = 1
```

```rust
// crates/fleet-core/src/board/ops.rs — pure, deterministic, unit-tested; every mutation stamps updated_at & dirty
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardDraft { pub title: String, #[serde(default)] pub description: String,
    #[serde(default)] pub status_id: Option<StatusId>, #[serde(default)] pub priority: Priority,
    #[serde(default)] pub labels: Vec<LabelId>, #[serde(default)] pub assignee: Option<String>,
    #[serde(default)] pub estimate: Option<u32>, #[serde(default)] pub due_date: Option<String>,
    #[serde(default)] pub parent_id: Option<CardId>, #[serde(default)] pub repo_id: Option<RepoId>,
    #[serde(default)] pub properties: BTreeMap<String, PropertyValue> }

/// `None` = leave unchanged; `Some(None)` = clear. All fields optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardPatch { pub title: Option<String>, pub description: Option<String>,
    pub status_id: Option<StatusId>, pub priority: Option<Priority>, pub labels: Option<Vec<LabelId>>,
    pub assignee: Option<Option<String>>, pub estimate: Option<Option<u32>>, pub due_date: Option<Option<String>>,
    pub parent_id: Option<Option<CardId>>, pub repo_id: Option<Option<RepoId>>,
    pub properties: Option<BTreeMap<String, PropertyValue>> /* merge; Null removes */, pub archived: Option<bool> }
impl CardPatch { pub fn is_empty(&self) -> bool; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoardPatch { pub name: Option<String>, pub prefix: Option<String>, pub backend: Option<BackendRef>,
    pub statuses: Option<Vec<Status>>, pub labels: Option<Vec<Label>>, pub properties: Option<Vec<PropertySchema>>,
    pub default_repo_id: Option<Option<RepoId>>, pub settings: Option<BoardSettings> }

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BoardError {
    #[error("board not found: {0}")] BoardNotFound(String),
    #[error("card not found: {0}")] CardNotFound(String),
    #[error("unknown status: {0}")] UnknownStatus(String),
    #[error("unknown label: {0}")] UnknownLabel(String),
    #[error("invalid {field}: {reason}")] Invalid { field: String, reason: String },
    #[error("board already exists for context {0}")] Duplicate(String),
    #[error("backend `{0}` is not registered")] UnknownBackend(String),
    #[error("backend does not support {0}")] Unsupported(&'static str),
    #[error("backend error: {0}")] Backend(String),
    #[error("card {0} has an unresolved conflict")] Conflicted(String),
}

pub fn validate_board(board: &Board) -> Result<(), BoardError>;   // prefix, unique status/label ids, ≥1 status, unique property keys, nonempty status/label/property names
pub fn validate_card(board: &Board, card: &Card) -> Result<(), BoardError>; // status/labels exist, property kinds match schema, `Date` property values and `due_date` are real dates
pub fn valid_date(date: &str) -> bool;  // real YYYY-MM-DD calendar day; every surface offering a due date uses this one
/// The `bool` is whether anything changed: a patch that leaves every field as it found it is not
/// a mutation, so it stamps nothing and the service does not save or announce it.
pub fn apply_board_patch(board: &mut Board, patch: BoardPatch, now: &str) -> Result<bool /* changed */, BoardError>;
/// Assigns number/id-less card; caller supplies the id. Status defaults to first Unstarted, else first status.
/// A draft or patch naming a property whose schema is `editable: false` is refused: the backend owns it.
pub fn create_card(board: &mut Board, cards: &[Card], id: CardId, draft: CardDraft, now: &str) -> Result<Card, BoardError>;
/// Refuses a draft that sets a standard field the backend declared it cannot write back — the local
/// write path only, which is why it is not inside `create_card`: `sync::reconcile` creates cards from
/// what a pull reported, and a field the remote owns is exactly the field it must set there.
/// `BoardService::create_card` calls it; `parent_id` is absent on purpose, because a backend's create
/// can carry the hierarchy even where its `edit` cannot.
pub fn check_draft_writable(board: &Board, draft: &CardDraft) -> Result<(), BoardError>;
pub fn apply_card_patch(board: &Board, card: &mut Card, patch: CardPatch, now: &str) -> Result<Vec<String> /* changed fields */, BoardError>;
/// Moves to `status_id` at `index` (None = end) and renumbers `position` in the target column (0,10,20…).
/// The `bool` is whether anything moved: a move onto the status and position the card already
/// held is not a mutation, so it stamps nothing, logs no activity, and the service does not save.
pub fn move_card(board: &Board, cards: &mut [Card], card_id: &CardId, status_id: &StatusId, index: Option<usize>, now: &str) -> Result<bool /* moved */, BoardError>;
pub fn add_comment(card: &mut Card, id: String, author: Option<String>, body: String, now: &str) -> Result<(), BoardError>;
pub fn push_activity(card: &mut Card, kind: ActivityKind, actor: Option<String>, message: impl Into<String>, now: &str);
/// Cards of a column, sorted by (position, created_at, number); excludes archived.
pub fn column_cards<'a>(cards: &'a [Card], status_id: &StatusId) -> Vec<&'a Card>;
pub fn first_status_in(board: &Board, category: StatusCategory) -> Option<&Status>;
/// "{key}-{slug}" rendering used for worktree slug/branch; slugified, ≤ 48 chars, never empty,
/// and never dotted — the same string names a Git branch, where `.` is illegal in several positions.
pub fn worktree_slug(board: &Board, card: &Card) -> String;
pub fn summarize(board: &Board, cards: &[Card]) -> BoardSummary;
pub fn default_true() -> bool; pub fn default_branch_template() -> String;
```

## 3. Sync engine (`fleet_core::board::sync`) — pure, no I/O

```rust
/// What a backend returns for one remote issue. Backend maps its native shape into this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCard { pub key: String, pub url: Option<String>, pub version: Option<String>,
    pub updated_at: Option<String>, pub title: String, pub description: String,
    pub status: RemoteStatus, pub priority: Option<Priority>, pub labels: Vec<String> /* names */,
    pub assignee: Option<String>, pub estimate: Option<u32>, pub due_date: Option<String>,
    pub parent_key: Option<String>, pub properties: BTreeMap<String, PropertyValue>,
    pub comments: Vec<RemoteComment> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus { pub id: String, pub name: String, pub category: Option<StatusCategory> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteComment { pub id: String, pub author: Option<String>, pub body: String, pub created_at: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PullResult { pub cards: Vec<RemoteCard>, pub deleted_keys: Vec<String>, pub cursor: Option<String>,
    /// true = `cards` is the complete remote set (absent keys are gone); false = incremental.
    pub full: bool,
    /// Keys the backend listed but could not read this time, each as `"KEY: reason"`. A full
    /// pull says "everything absent from `cards` is gone", and an unread key is not absent:
    /// `reconcile` neither updates nor archives one, and the service reports them the way it
    /// reports `summary.skipped` — as a warning on `sync.last_error`, never as a failed sync.
    #[serde(default)] pub failed_keys: Vec<String> }

/// What the backend describes about itself for a given board (statuses, labels, properties, people).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendSchema { pub statuses: Vec<RemoteStatus>, pub labels: Vec<String>,
    pub properties: Vec<PropertySchema>, pub assignees: Vec<String>, pub key_prefix: Option<String> }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendCapabilities { pub pull: bool, pub push_updates: bool, pub push_create: bool,
    pub transitions: bool, pub comments: bool, pub custom_properties: bool, pub incremental: bool }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PushOp { Create { card_id: CardId }, Update { card_id: CardId, fields: Vec<String> },
                  Transition { card_id: CardId, remote_status: String }, AddComment { card_id: CardId, comment_id: String } }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PushResult { pub acks: Vec<PushAck>, pub failures: Vec<PushFailure> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushAck { pub card_id: CardId, pub key: String, pub url: Option<String>, pub version: Option<String>,
                     pub comment_ids: Vec<(String /* local */, String /* remote */)> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushFailure { pub card_id: CardId, pub error: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary { pub pulled: usize, pub created: usize, pub updated: usize, pub deleted: usize,
    pub conflicts: usize, pub pushed: usize, pub unmapped_statuses: Vec<String>,
    /// Remote keys this pull could not import ("KEY: reason"); the rest of the pull still applies.
    #[serde(default)] pub skipped: Vec<String>,
    /// Unlinked cards a linked board's `pushNewCards = false` held back; the sync names the count.
    #[serde(default)] pub kept_local: usize }

pub struct Reconciled { pub cards: Vec<Card>, pub board: Board, pub to_push: Vec<PushOp>,
    /// Cards whose local fields this push cannot acknowledge (unsupported capability or unmapped status).
    pub unpushed: Vec<CardId>, pub summary: SyncSummary }

/// Merge backend schema into the board: on a board that still has `default_statuses()` and no remote
/// cards, adopt the remote statuses wholesale (ids slugified from names, category from remote or guessed
/// by name: backlog/todo/open→Unstarted, progress/review/doing→Started, done/closed/resolved→Completed,
/// cancel*/won't→Canceled); otherwise reuse the mapping already recorded for that remote id, then map
/// by exact name (case-insensitive), then by category, and record unmapped remote statuses. Backend properties replace `source == Backend` entries; local ones stay.
/// A schema with no statuses, properties or labels describes nothing and changes nothing: it is a
/// degraded answer, and adopting it would erase the status map and every backend property. So is one
/// with no statuses at all on a board that already had a map — a backend that samples its statuses
/// from live issues answers that for a filter matching nothing, while still carrying the fixed
/// properties it always does. A column the new schema no longer names but the board still shows keeps
/// the `local_to_remote` entry it had: dropping it would make every move into that column push nothing
/// while the card stays dirty forever.
pub fn adopt_schema(board: &mut Board, schema: &BackendSchema, now: &str) -> Vec<String> /* unmapped */;

/// Match `pull.cards` to local cards by `remote.key`; create/update/delete; detect conflicts
/// (local `dirty` and remote changed since `remote.remote_updated_at`/`version`) and resolve by policy;
/// emit `to_push` for dirty non-conflicted linked cards (Update/Transition/AddComment) and, when
/// `settings.push_new_cards`, `Create` for unlinked cards. Remote comments merge by remote id under every
/// policy, `LocalWins` included, because the advanced baseline is never offered again. A card's `parent_id`
/// follows its `remote.parent_key` only while it has a link: an unlinked card keeps the local parent no pull
/// knows about. Under `Manual`, a dirty card whose fields all match the remote converges instead of
/// recording a conflict with nothing to choose. A field in `sync.readonly_fields` is never a conflict
/// side and never an `Update`: the local surfaces refuse to write it, so a difference there is the
/// remote's alone. An unlinked card's `Create` is followed by the `Transition` of the column it was
/// born in, because no create carries a status and the ack's version hides the mismatch from every
/// later pull. A **full** pull that brought back no card and no deletion at all, on a board that still
/// holds linked cards, archives nothing: it is a filter that matched nothing, not an emptied project,
/// and archiving would also clear the `dirty` flag of every queued push. Pure and idempotent.
pub fn reconcile(board: &Board, cards: &[Card], pull: &PullResult, caps: BackendCapabilities, now: &str) -> Reconciled;

/// Apply acks: link cards (`remote`), clear `dirty` unless the card is in `unpushed` or still holds an
/// unsent comment, mark comment remote ids, record failures in activity.
pub fn apply_push_result(cards: &mut [Card], result: &PushResult, backend: &str, unpushed: &[CardId], now: &str);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution { KeepLocal, TakeRemote }
pub fn resolve_conflict(board: &Board, card: &mut Card, resolution: ConflictResolution, now: &str) -> Result<(), BoardError>;

/// Remote → local field mapping used by reconcile and TakeRemote (labels by name → create missing Label on board).
/// Remote properties without a matching non-Local schema entry are dropped (a card the board cannot
/// validate would wedge every later sync). Returns whether a domain field changed; `synced_at` and
/// `updated_at` do not count, so replaying a pull from a backend with no version or timestamp neither
/// stamps the card nor appends activity.
pub fn apply_remote(board: &mut Board, card: &mut Card, remote: &RemoteCard, now: &str) -> bool;
```

## 4. Daemon

```rust
// crates/fleet-daemon/src/adapters/board.rs
#[async_trait::async_trait]
pub trait BoardBackend: Send + Sync {
    fn kind(&self) -> &'static str;
    fn capabilities(&self) -> BackendCapabilities;
    /// Validate `BackendRef.settings` for this kind (shape + reachability if cheap). Called on create/update.
    async fn validate(&self, settings: &serde_json::Value) -> Result<(), BoardError>;
    /// The settings as this backend will actually read them, for the service to store (default: identity).
    /// A backend that rewrites what it was given must say so here, or the settings dialog and
    /// `board show --json` keep showing a value it has been ignoring. Only keys the caller supplied may
    /// be rewritten: filling defaults in would turn every unset optional row into a chosen value.
    async fn normalize(&self, settings: &serde_json::Value) -> Result<serde_json::Value, BoardError>;
    async fn describe(&self, board: &Board) -> Result<BackendSchema, BoardError>;
    async fn pull(&self, board: &Board, cursor: Option<&str>) -> Result<PullResult, BoardError>;
    /// `cards` are the current local cards referenced by `ops`.
    async fn push(&self, board: &Board, cards: &[Card], ops: &[PushOp]) -> Result<PushResult, BoardError>;
}
#[derive(Clone, Default)]
pub struct BoardBackends { inner: Arc<HashMap<String, Arc<dyn BoardBackend>>> }
impl BoardBackends {
    pub fn new(backends: Vec<Arc<dyn BoardBackend>>) -> Self;
    pub fn get(&self, kind: &str) -> Result<Arc<dyn BoardBackend>, BoardError>;
    pub fn kinds(&self) -> Vec<&'static str>;
    pub fn system() -> Self;   // [LocalBackend]; Jira added later
}
// crates/fleet-daemon/src/adapters/board/local.rs
pub struct LocalBackend;  // kind "local"; caps all false; describe → default schema; pull → empty full=false; push → empty
// registered as `Adapters.board_backends: BoardBackends` (adapters/mod.rs), fakes.rs gets `FakeBackend` (scripted pull/push, records calls)
```

```rust
// crates/fleet-daemon/src/stores/board.rs — one JSON file per board, atomic write, quarantine on parse error (mirror StateStore)
pub struct BoardStore { home: FleetHome, files: Arc<dyn Files> }
impl BoardStore {
    pub fn new(home: FleetHome, files: Arc<dyn Files>) -> Self;
    pub fn list(&self) -> DaemonResult<Vec<BoardId>>;
    /// Reports a damaged document *and* quarantines it as `<id>.json.broken-<uuid>`: the read a
    /// writer makes, so nothing is ever written over a document this build could not read.
    pub fn load(&self, id: &BoardId) -> DaemonResult<Option<BoardDocument>>;
    /// The same read without the quarantine, for scans: moving the file aside while scanning
    /// would let the `ensure` that scanned for a board create an empty one in its place.
    pub fn peek(&self, id: &BoardId) -> DaemonResult<Option<BoardDocument>>;
    pub fn quarantined(&self, id: &BoardId) -> DaemonResult<Vec<PathBuf>>;   // `<id>.json.broken-*`
    pub fn stamp(&self, id: &BoardId) -> Option<(u64, SystemTime)>;          // size+mtime; memoizes summaries
    pub fn save(&self, doc: &BoardDocument) -> DaemonResult<()>;
    pub fn delete(&self, id: &BoardId) -> DaemonResult<()>;   // board and its quarantined remains to trash_dir
}
// FleetHome (contracts stage): pub fn boards_dir(&self) -> PathBuf /* $FLEET_HOME/boards */; pub fn board_path(&self, id: &BoardId) -> PathBuf
```

```rust
// crates/fleet-daemon/src/services/boards.rs
pub struct Boards { /* store, state_store (contexts/repos/worktrees lookup), backends, clock, jobs, worktrees: Arc<Worktrees>, events broadcaster, index: RwLock<HashMap<CardId, BoardId>> */ }
impl Boards {
    pub fn new(store: Arc<BoardStore>, state_store: Arc<StateStore>, backends: BoardBackends, clock: Arc<dyn Clock>, jobs: Arc<JobManager>, worktrees: Arc<Worktrees>, events: BroadcastBus) -> Self;
    /// Skips a document this build cannot read; a `context` that does not exist is `NotFound`.
    pub async fn list(&self, context: Option<&ContextId>) -> DaemonResult<Vec<BoardSummary>>;
    pub async fn get(&self, id: &BoardId) -> DaemonResult<BoardView>;
    /// Get-or-create the context's board (`defaults::new_board`). Errors if the context does not exist.
    pub async fn ensure(&self, context: &ContextId) -> DaemonResult<BoardView>;
    pub async fn create(&self, context: &ContextId, name: Option<String>, prefix: Option<String>, backend: Option<BackendRef>) -> DaemonResult<BoardView>;
    /// A patch that changes nothing writes nothing and emits nothing, as an empty card patch does.
    pub async fn update(&self, id: &BoardId, patch: BoardPatch) -> DaemonResult<BoardView>;   // validates+normalizes backend settings via the registry, and only when the patch changed the BackendRef: a rename or a label must not wait on (or fail with) a backend it never mentioned
    pub async fn delete(&self, id: &BoardId) -> DaemonResult<()>;
    /// Deletes every board of a context; the `DeleteContext` cascade calls it before the context goes.
    /// A document this build cannot read goes to trash with its context rather than blocking it:
    /// the cascade has already deleted the context's repositories by then.
    pub async fn delete_for_context(&self, context: &ContextId) -> DaemonResult<()>;
    pub async fn create_card(&self, board: &BoardId, draft: CardDraft) -> DaemonResult<Card>;
    /// A patch that changes `status_id` appends the card to its new column, as `move_card` would —
    /// including its refusals: an archived card cannot be moved through the patch path either.
    /// Changing `repo_id` — clearing it, or naming any repository but the worktree's own — while
    /// the card holds a live worktree is `Conflict`: the card could otherwise reach no repository again.
    pub async fn update_card(&self, card: &CardId, patch: CardPatch) -> DaemonResult<Card>;
    pub async fn move_card(&self, card: &CardId, status: &StatusId, index: Option<usize>) -> DaemonResult<Card>;
    /// Refused (`Validation`, naming the issue) for a card with a `remote` link: nothing carries
    /// the deletion to the backend, so the next pull files the issue again as a new card — with a
    /// new number and none of the comments, activity, worktree link or position this row held.
    pub async fn delete_card(&self, card: &CardId) -> DaemonResult<()>;
    pub async fn add_comment(&self, card: &CardId, body: String) -> DaemonResult<Card>;
    /// Slug/branch from `ops::worktree_slug`; repo = arg → card.repo_id → board.default_repo_id → error.
    /// Delegates to `Worktrees::create`; on success links card.worktree_id/repo_id, logs activity, applies `start_on_worktree`.
    /// The `bool` is `created`: false when an existing worktree was adopted. A worktree another
    /// card already links is never adopted (`Conflict`), and a card already linked to a live worktree
    /// in another repository is refused (`Conflict`). Repositories must share the board's context.
    /// Two concurrent calls for one card make one worktree: the loser adopts it rather than
    /// reporting the winner's `already exists`.
    pub async fn create_worktree_from_card(&self, card: &CardId, repo: Option<RepoId>, base: Option<String>, host: Option<HostId>) -> DaemonResult<(Card, Worktree, bool)>;
    /// Submits `JobKind::Custom("board.sync")`: describe→adopt_schema→pull→reconcile→push→apply_push_result→save→emit BoardChanged. Returns immediately.
    /// A backend kind this build does not register is refused before any job is submitted.
    pub async fn sync(&self, id: &BoardId) -> DaemonResult<JobId>;
    pub async fn resolve_conflict(&self, card: &CardId, resolution: ConflictResolution) -> DaemonResult<Card>;
    pub async fn describe_backend(&self, id: &BoardId) -> DaemonResult<BackendSchema>;
    pub async fn summaries(&self) -> Vec<BoardSummary>;   // for Snapshot.boards
}
```
Every mutation: load doc → apply pure op → `validate_card` → save → emit `Event::BoardChanged`.
Cards whose `worktree_id` no longer exists in `State.worktrees` are reported with `worktree_id:
None` (not persisted), and a `repo_id` — on a card or as `Board.default_repo_id` — naming a
repository the state no longer has in the board's context is reported the same way and skipped
when `create_worktree_from_card` picks a repository. `ensure`/`create` refuse a context whose
board document is quarantined rather than creating an empty board over it; `delete_for_context`
takes the quarantined remains with it. `summaries` reparses a board document only when its `stamp`
changed or this daemon rewrote it. Boards whose context no longer exists are skipped by
`list`/`summaries`, and deleting a context deletes its board in the same cascade
(`delete_for_context`) so a later context deriving the same id cannot adopt it. Board locks are
per board: no board's clone, sync or hook run blocks another board's requests, and `ensure` reads
an existing board without taking one.

## 5. Protocol (`fleet-proto`, version 5)

```rust
// RequestBody discriminants and fields use snake_case, like their siblings; domain payloads use camelCase.
ListBoards { context_id: Option<ContextId> }                       → ResponseBody::Boards(Vec<BoardSummary>)
GetBoard { board_id: BoardId }                                      → Board(BoardView)
EnsureBoard { context_id: ContextId }                               → Board(BoardView)
CreateBoard { context_id, name: Option<String>, prefix: Option<String>, backend: Option<BackendRef> } → Board
UpdateBoard { board_id, patch: BoardPatch }                         → Board
DeleteBoard { board_id }                                            → Ack
CreateCard { board_id, draft: CardDraft }                           → Card(Card)
UpdateCard { card_id, patch: CardPatch }                            → Card
MoveCard { card_id, status_id, index: Option<usize> }               → Card
DeleteCard { card_id }                                              → Ack
AddCardComment { card_id, body: String }                            → Card
CreateWorktreeFromCard { card_id, repo_id: Option<RepoId>, base: Option<String>, host: Option<HostId> } → CardWorktree { card: Card, worktree: Worktree, created: bool }
SyncBoard { board_id, #[serde(default)] full: bool }                 → ResponseBody::Job(JobRecord); Client::sync_board returns its JobId
ListBoardBackends                                                   → BoardBackends(Vec<BackendDescriptor>)
ResolveCardConflict { card_id, resolution: ConflictResolution }     → Card
DescribeBoardBackend { board_id }                                   → BoardBackendSchema(BackendSchema)

// EventKind::BoardChanged ; Event::BoardChanged { board_id: BoardId, reason: BoardChangeReason }
enum BoardChangeReason { Created, Updated, Deleted, CardChanged, Synced, SyncFailed }
// Snapshot: #[serde(default)] pub boards: Vec<BoardSummary>
```

## 6. Client and CLI

`Client::create_worktree_from_card` returns `(Card, Worktree, bool /* created */)`.
`fleet_client::Client` gains one typed method per request above (`list_boards`, `get_board`,
`ensure_board`, `create_board`, `update_board`, `delete_board`, `create_card`, `update_card`,
`move_card`, `delete_card`, `add_card_comment`, `create_worktree_from_card`, `sync_board`,
`resolve_card_conflict`, `describe_board_backend`).

CLI (`fleet board …`, JSON envelopes v1 with `--json`, human tables otherwise; board resolved from
`--board <id>` else `--context <id>` else the active context via `EnsureBoard`):
```
fleet board show [--context C|--board B]                          # columns + cards
fleet board list                                                  # summaries
fleet board create [--context C] [--name N] [--prefix P] [--backend local|jira] [--setting k=v]...
fleet board set [--name] [--prefix] [--default-repo owner/name] [--clear-default-repo] [--start-on-worktree [true|false]] [--conflict-policy manual|remote_wins|local_wins] [--push-new-cards [true|false]] [--branch-template "{key}-{slug}"] [--add-label NAME]... [--remove-label L]... [--backend KIND] [--setting k=v]...
fleet board backends                                              # registered kinds, capabilities, setting keys
fleet board describe [--context C|--board B]                      # what this board's backend reports about itself
fleet board sync [--wait] [--full]                                # --full ignores the incremental cursor
fleet board card new <title> [--desc] [--status S] [--priority urgent|high|medium|low|none] [--label L]... [--assignee] [--estimate] [--due YYYY-MM-DD] [--repo]
fleet board card show <key|id>
fleet board card edit <key|id> [same flags as new] [--clear-labels|--clear-assignee|--clear-estimate|--clear-due|--clear-repo] [--archive [true|false]]
fleet board card move <key|id> <status> [--index N]
fleet board card comment <key|id> <body>
fleet board card delete <key|id>
fleet board card worktree <key|id> [--repo owner/name] [--base REF] [--host H]   # prints the created worktree like `fleet create`
fleet board card resolve <key|id> keep-local|take-remote
```
`<key|id>` accepts a display key (`FLT-12`, `PROJ-123`) or a CardId, and the local key of a card
with no remote link — a mirrored card's local key is not a selector, because a board mirroring the
Jira project its own prefix names would have two namespaces of the same shape overlapping. `board
card show` prints `Local key:` for exactly the cards that answer to one.

## 7. UI-kit components (`fleet-ui-kit`, gpui only, tokens only)

```rust
// text_area.rs — multi-line sibling of text_field.rs, same three layers
pub struct TextAreaState { /* text: String, cursor: byte offset, preferred_col, scroll_row */ }
impl TextAreaState {
    pub fn new() -> Self; pub fn from_text(text: impl Into<String>) -> Self;
    pub fn text(&self) -> &str; pub fn set_text(&mut self, text: impl Into<String>); pub fn cursor(&self) -> usize;
    pub fn line_col(&self) -> (usize, usize);
    /// Handles insert, backspace/delete, word ops, left/right/up/down, home/end, ctrl-a/e, enter (newline), tab (2 spaces). Returns true if consumed.
    pub fn handle_keystroke(&mut self, keystroke: &Keystroke) -> bool;
    pub fn insert(&mut self, s: &str);
}
pub struct TextArea { /* RenderOnce, presentational */ }
impl TextArea {
    pub fn new(value: impl Into<SharedString>) -> Self;
    pub fn cursor(self, byte_offset: usize) -> Self; pub fn focused(self, bool) -> Self;
    pub fn placeholder(self, impl Into<SharedString>) -> Self; pub fn label(self, impl Into<SharedString>) -> Self;
    pub fn rows(self, u32) -> Self /* min visible rows, default 6 */; pub fn mono(self, bool) -> Self; pub fn invalid(self, bool) -> Self;
}

// markdown_text.rs — read-mode renderer, no dependency; supports #/##/### headings, paragraphs, `-`/`*`/`1.` lists,
// ``` fenced code (mono block), `inline code`, **bold**, blank-line paragraph breaks, bare URLs (accent color). Anything else = plain text.
pub struct MarkdownText { /* RenderOnce */ }
impl MarkdownText { pub fn new(source: impl Into<SharedString>) -> Self; pub fn muted(self, bool) -> Self; }
pub fn parse_markdown(source: &str) -> Vec<MdBlock>;   // pure, unit-tested
pub enum MdBlock { Heading { level: u8, text: String }, Paragraph(Vec<MdSpan>), List { ordered: bool, items: Vec<Vec<MdSpan>> }, Code(String) }
pub enum MdSpan { Text(String), Code(String), Bold(String), Link(String) }

// priority_glyph.rs
pub enum PriorityLevel { Urgent, High, Medium, Low, None }
pub struct PriorityGlyph { /* RenderOnce: "!!!"-style stacked bars/glyph using tokens; Urgent uses the danger token */ }
impl PriorityGlyph { pub fn new(level: PriorityLevel) -> Self; pub fn with_label(self, bool) -> Self; }

// card_tile.rs — one kanban card
pub struct CardTile { /* RenderOnce */ }
impl CardTile {
    pub fn new(id: impl Into<ElementId>, key: impl Into<SharedString>, title: impl Into<SharedString>) -> Self;
    pub fn priority(self, PriorityLevel) -> Self;
    pub fn labels(self, Vec<(SharedString, Option<SharedString> /* color token */)>) -> Self;
    pub fn assignee(self, Option<SharedString>) -> Self;          // initials chip
    pub fn estimate(self, Option<u32>) -> Self;
    pub fn due(self, Option<SharedString>) -> Self;
    pub fn worktree(self, bool) -> Self;                          // branch glyph when linked
    pub fn dirty(self, bool) -> Self; pub fn conflict(self, bool) -> Self;   // small status dots
    pub fn selected(self, bool) -> Self; pub fn focused(self, bool) -> Self;
    pub fn extras(self, Vec<SharedString>) -> Self;               // custom `show_on_card` property values
    pub fn on_click(self, impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> Self;
}

// kanban_column.rs — a column: header (name, count, category color), scrollable body of children, empty hint
pub struct KanbanColumn { /* RenderOnce */ }
impl KanbanColumn {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self;
    pub fn count(self, usize) -> Self; pub fn accent(self, Option<Hsla>) -> Self /* tokens only at call site */;
    pub fn focused(self, bool) -> Self; pub fn width(self, Pixels) -> Self;
    pub fn empty_hint(self, impl Into<SharedString>) -> Self;
    pub fn children(self, impl IntoIterator<Item = AnyElement>) -> Self;
    pub fn scroll_handle(self, ScrollHandle) -> Self;
}
pub struct KanbanBoard { /* RenderOnce: horizontal scroller of columns with gutter */ }
impl KanbanBoard { pub fn new(id: impl Into<ElementId>) -> Self; pub fn columns(self, impl IntoIterator<Item = AnyElement>) -> Self; pub fn scroll_handle(self, ScrollHandle) -> Self; }
```
All exported from `components/mod.rs` and shown in the `kit_gallery` example in both themes.
Property rows in the card detail reuse `KeyValueList`/`FactRow`; pickers reuse
`FuzzyList`/`Select`.

## 8. App (`fleet-app`)

- **Screen**: the board is a hub tab: `HubTab::Board`, key `g b`, tab label "Board", rendered by
  `screens/board.rs::BoardScreen` with the frozen screen signature (`docs/APP-CONTRACTS.md` §2).
  The hub context bar scopes it: the board shown is `EnsureBoard(active_context)`.
- **State** (`state.rs`): `AppState.board: BoardState { view: Option<BoardView>, loading: bool,
  error: Option<String>, focus: BoardFocus { column: usize, row: usize }, filter: String,
  filter_editing: bool, group_secondary: Option<GroupBy> }`. Loaded on tab open / context switch
  (`EnsureBoard`), refreshed on `Event::BoardChanged` for the shown board id.
  `AppState.snapshot.boards` summaries drive the tab badge (open count, conflict dot).
- **Bridge**: board requests use `Bridge::request` reply receivers, with no new `BridgeEvent`
  variants. Responses land in `AppState` reducers (`apply_board_view`, `apply_card`); board loads
  use context/generation guards.
- **Dialogs** (`Dialogs` variants; state in `DialogHost`): `CardDetail` (`dialogs/card_detail.rs`,
  `CardDetailState`), `CardCreate` (`dialogs/card_create.rs`), `CardPicker`
  (`dialogs/card_picker.rs`, `PickerKind { Status, Priority, Assignee, Labels, Estimate, DueDate,
  Repo, Property(key) }`), `BoardSettings` (`dialogs/board_settings.rs`: name, prefix, default
  repo, start-on-worktree, push-new-cards, conflict policy, and the backend — a kind cycler over
  `ListBoardBackends` plus one generic row per `settings_schema` entry; see `docs/BOARD-JIRA.md`
  §6).
- **Card detail layout** (UX-SPEC §board): two panes. Left: key + title (editable, `i`),
  description (`MarkdownText`; `d` toggles `TextArea` edit; `ctrl-s`/`esc` saves/cancels),
  comments (list + `c` to add via a `TextArea`), activity (last 10). Right: property list —
  Status, Priority, Assignee, Labels, Estimate, Due, Parent, Repo, Worktree (enter = open its
  session), Remote (key/url/synced/dirty), then custom properties from `board.properties`; `j/k`
  select row, `enter` opens the matching picker. Conflict banner with `K` keep-local / `R`
  take-remote when `card.conflict` is set.
- **Keymap** (`docs/KEYMAP.md` rows, contexts `Hub > Board` and `Dialog > CardDetail` etc., one
  action per row):

| Key | Context | Action |
|---|---|---|
| `g b` | Hub | go to Board tab |
| `h` / `l`, `left` / `right` | Hub > Board | previous / next column |
| `j` / `k`, `down` / `up` | Hub > Board | next / previous card |
| `enter` | Hub > Board | open card detail |
| `c` | Hub > Board | new card (CardCreate) |
| `s` | Hub > Board | status picker |
| `p` | Hub > Board | priority picker |
| `a` | Hub > Board | assignee picker |
| `t` | Hub > Board | labels picker |
| `e` | Hub > Board | estimate picker |
| `[` / `]` | Hub > Board | move card to previous / next column |
| `w` | Hub > Board | create worktree from card |
| `o` | Hub > Board | open linked worktree session |
| `S` | Hub > Board | sync board |
| `F` | Hub > Board | full sync (ignore the cursor) |
| `x` | Hub > Board | open the card's remote issue in the browser |
| `d` | Hub > Board | delete card (confirm) |
| `,` | Hub > Board | board settings |
| `r` | Hub > Board | reload board |
| `/` | Hub > Board | filter cards (title/key/label/assignee substring) |
| `esc` | Dialog > CardDetail | close (saves nothing pending) |
| `i` | Dialog > CardDetail | edit title |
| `d` | Dialog > CardDetail | edit description |
| `c` | Dialog > CardDetail | add comment |
| `j` / `k` | Dialog > CardDetail | select property row |
| `enter` | Dialog > CardDetail | edit selected property |
| `w` | Dialog > CardDetail | create worktree from card |
| `x` | Dialog > CardDetail | open the card's remote issue in the browser |
| `K` / `R` | Dialog > CardDetail | resolve conflict keep-local / take-remote |
| `ctrl-s` | Dialog > CardDetail | save current text edit |

Palette commands mirror every row above (`Board: New card`, `Board: Sync`, …).

### Integrated implementation decisions

Public function signatures above are preserved. Additive helpers/builders and app editing fields
are documented in APP-CONTRACTS and DESIGN-SYSTEM. `BoardSummary` also derives `Eq`. Nested
optional patch fields preserve missing = unchanged and JSON null = clear.

The daemon supplies the context that pure sync signatures cannot inspect: before schema adoption
it seeds missing sync metadata from linked cards, and remaps unlinked cards when status IDs
change. Before TakeRemote it materializes labels with `apply_remote` on a scratch card, then
resolves parent keys against stored cards, dropping a resolved parent that would close a cycle
exactly as it drops a key that names no local card. Standalone core callers must do the same
parent lookup and supply missing labels before calling immutable-board `resolve_conflict`.
Successful push acknowledgements supply `SyncSummary.pushed`; job progress carries the counts.
Remote sync jobs are retryable and noncancellable, checkpoint pulls before pushes, and persist
failures in `Board.sync.last_error`. Local sync returns Unsupported without scheduling a job. A
pull carrying an unimportable issue persists every card that did reconcile, leaves the cursor
where it was so the next pull offers the bad one again, and **continues into the push**: the
skipped keys (joined with the pull's own `failed_keys`) are reported as a warning on
`Board.sync.last_error`, which `reconcile` clears on the next sync. The job fails only when
nothing reconciled and there was nothing to push — one bad issue must never strand every queued
local edit, or pin the board to full pulls forever. That covers remote *updates* as much as remote
drafts: a remote field the board cannot hold leaves the local card untouched and reports `"{key}:
{reason}"`. A remote `parent_key` that would close a cycle is dropped, exactly as
`resolve_conflict` drops one. `position` is local ordering: reordering a column dirties nothing,
and only a real status change dirties the card that moved. A card's `parent_id` must name another
card of the same board and may not close a cycle.

The app refreshes through `EnsureBoard(active_context)` after BoardChanged. `filter_editing`
selects the Filter key context while typing, with two-stage Escape. `group_secondary` is reserved;
Parent is read-only in this milestone. Card detail is an 880 px two-pane dialog. `ctrl-enter` in
CardCreate creates and opens detail; label pickers use Space for multi-select; BoardSettings
reuses the settings row keys. Delete uses `ConfirmRequest::DeleteCard`. These supplemental dialog
keys are listed in KEYMAP; context-only create-and-open has no palette command.
Description/comment editors compose presentational TextArea with its state and key handling; a
separate live input entity is not required. Card tiles suppress None priority, while standalone
PriorityGlyph still renders it. Label colors remain token names.

## 9. What the tests hold

The board's regression surface is spread across the crates it touches, and each layer holds one
thing so a failure names the layer that broke.

- **Core** (`crates/fleet-core/src/board/`) covers the pure rules: create, patch, move and the
  fractional positions they produce; validation and `worktree_slug`; and the reconciliation engine
  — `adopt_schema`'s status mapping, `reconcile`'s create/update/delete/conflict decisions under
  each `ConflictPolicy`, the push operations it emits, and `apply_push_result`.
- **Daemon** (`crates/fleet-daemon/tests/boards_*.rs`) covers the service against a real store: the
  ensure/create/patch/move/delete/comment round trip, persistence and quarantine, worktree-from-card
  over a fake git, a full sync against `FakeBackend` including a conflict and its resolution, and
  the events each mutation emits.
- **CLI and client** cover the JSON envelopes and one socket round trip per typed method.
- **App** covers the reducers rather than rendered strings: the board mirror's staleness and
  generation rules, the focus clamp under a filter, and the two-stage filter `Esc`. The keymap
  drift test keeps `docs/KEYMAP.md` and `keymap.rs` in agreement.

The board's own surfaces answer the same standing rule the rest of the kit does: a state that is
not in `cargo run -p fleet-ui-kit --example gallery_board` is not implemented.

## 10. Adding a backend

A new backend = one module under `adapters/board/<kind>.rs` implementing `BoardBackend`, a typed
settings struct deserialized from `BackendRef.settings`, registration in
`BoardBackends::system()`, and (optionally) a `board set --backend <kind> --setting k=v` CLI path.
It maps native issues into `RemoteCard` (native-only fields go into `properties` with a
`PropertySchema` from `describe`), and applies `PushOp`s. It never touches the store or the UI.

Jira is the worked example, and it is implemented:
`adapters/board/jira.rs` and `jira/{acli,adf,map,push,schema,settings,users}.rs`, contract in
`docs/BOARD-JIRA.md`.
Three of its lessons generalize. **A backend declares what it cannot write.**
`BackendSchema.readonly_fields` lists standard card fields the remote refuses post-create (for
Jira: priority, estimate, due date, parent); `adopt_schema` copies the list into
`board.sync.readonly_fields`, and `ops::apply_card_patch`/`move_card`/`check_draft_writable`
refuse a local change to one with `BoardError::ReadOnlyField`, which the CLI and app show verbatim
— so a field the remote would silently drop never becomes a dirty card that can never be pushed.
`parent_id` is the one exemption, on both doors: a backend's *create* can carry the hierarchy even
where its `edit` cannot, so a draft may name a parent and so may a patch of a card that has no
`remote` yet. `sync::resolve_conflict`'s `KeepLocal` copies these fields (and the backend's
non-editable properties) from the conflict's remote side for the same reason: the user's "keep
local" is a choice over the fields they can own, and the link it stamps carries the remote's
version, so nothing would ever revisit them again. The list is *replaced* on every adoption, so
every `adopt_schema` call site must re-state it. **A backend describes its own settings.**
`BoardBackend::label()` and `settings_schema()` feed `BackendDescriptor`, published by
`ListBoardBackends`; the CLI's `fleet board backends` and the app's board settings dialog render
any backend's form from that schema alone, so adding a backend needs no client change. **A
backend's shape is its remote's shape.** Jira's per-key fetch, name-keyed statuses, JQL
time-window incremental pulls and pure ADF⇄markdown conversion are all consequences of `acli`'s
limits, kept inside the adapter: the core, the store and the UI stay backend-agnostic, and outside
`adapters/board/jira/` the word "jira" appears in `fleet-app` only in tests and doc comments —
never in a rendered string or a branch.
