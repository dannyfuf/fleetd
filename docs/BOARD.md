# Board — contracts and architecture

**Status: authoritative.** This document owns the board's domain model, its reconciliation engine,
its persistence, its wire messages and its surface. The signatures below are the seams every other
crate is written against; changing one is a deliberate, workspace-wide change, and this document
changes in the same pass. Where the code and this file disagree, the code is the bug.

## Decision records

- [ADR 0008](decisions/0008-board-model-and-sync.md) establishes the backend-agnostic core, pure
  reconciliation engine, and original board wire family.
- [ADR 0019](decisions/0019-worktree-scoped-boards.md) adds the optional worktree scope, field-based
  lookup, deletion cascade, and capability-gated requests.
- [ADR 0021](decisions/0021-hosted-worktree-boards-route-to-owner.md) routes a hosted worktree's
  board to the daemon that owns the worktree.
- [ADR 0022](decisions/0022-board-workflows.md) turns the worktree board into a control plane: a
  column may run a card, a card carries links and a run history, and §11 is its model.

## 0. What we are building

A Linear-style kanban **board of cards**, one board per **context** (`fleet_core::Context`) and,
optionally, one additional board per **worktree**. A worktree board remains associated with the
worktree's context and repository while keeping that worktree's cards separate from the context
board. Moving the repository to another context rehomes every one of its worktree boards before
the state move is published, so deleting the old context cannot remove their cards. A worktree
board is a document of the daemon that **owns** the worktree; an app or CLI on another machine
reaches it by asking its local daemon, whose router forwards every board and card request for that
board to the owner (`docs/REMOTE-MACHINES.md` §6). Cards are the
unit of project tracking: identifier (`FLT-12`), title, markdown description,
status column, priority, labels, assignee, estimate, due date, parent, custom properties, comments,
activity. A card can **spawn a worktree** (the existing prepared-copy pipeline) and remembers it.

The **core is backend-agnostic and reusable**. A `BoardBackend` adapter (daemon side) plus a
**pure reconciliation engine** (core side) let a board mirror a remote system — Jira via `acli`,
Notion, anything — without the core knowing their shape. Backend-specific fields ride in
`Card.properties`, described by `PropertySchema` so the generic UI can render/edit them. The first
backend is `local` (no remote). Jira is the second (separate contract, later).

Non-goals for v1: multiple boards per scope in the UI (the model allows it; the Hub shows the
context's first board and a worktree's Workspace tab shows that worktree's, and no surface shows
two at once), cycles/projects/milestones, attachments, rich-text editing beyond a plain
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
| Dispatch arms | `crates/fleet-daemon/src/services/dispatch.rs` |
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<WorktreeId>,
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
                    #[serde(default)] pub color: Option<String>, /* token name, e.g. "accent" */
                    /// What this column does to a card entering it. `None` on every column until
                    /// someone opts in. See §11.
                    #[serde(default, skip_serializing_if = "Option::is_none")] pub automation: Option<ColumnAutomation> }

// The automation types below borrow `AgentKind`, `PermissionMode`, `DelegationId`, `ThreadId`
// and `DelegationStatus` from `fleet_core::agents` — same crate, so no new crate edge appears.

/// §11. An all-default block is not automation: `ops::normalise_automation` turns one into `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ColumnAutomation {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub on_enter: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub on_success: Option<StatusId>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub advance_when_unblocked: Option<StatusId>,
}
impl ColumnAutomation { pub fn is_empty(&self) -> bool; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub kind: ActionKind,
    /// Prepended to the brief. Markdown. `{key}` and `{title}` are substituted.
    #[serde(default, skip_serializing_if = "String::is_empty")] pub instructions: String,
    /// Printed in the run's footer as `The card expects: …`. May be empty.
    #[serde(default, skip_serializing_if = "String::is_empty")] pub expect: String,
    #[serde(default, skip_serializing_if = "ColumnAgentPrefs::is_empty")] pub agent: ColumnAgentPrefs,
    /// `KEY=VALUE`; `{key}` substituted in the value. `validate_env` applies the five `--env` rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub env: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind { Prompt, Skill { name: String, #[serde(default, skip_serializing_if = "String::is_empty")] args: String } }
impl ActionKind { pub fn word(&self) -> String; }   // "run card" | "run skill deep-review"

/// Permission mode is a workflow policy, so it lives on the column and never on a card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ColumnAgentPrefs { #[serde(default, skip_serializing_if = "Option::is_none")] pub provider: Option<AgentKind>,
                              #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
                              #[serde(default, skip_serializing_if = "Option::is_none")] pub effort: Option<String>,
                              #[serde(default, skip_serializing_if = "Option::is_none")] pub mode: Option<PermissionMode> }
impl ColumnAgentPrefs { pub fn is_empty(&self) -> bool; }

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
    /// The agent this card asks for, overriding what its column asks for. §11.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent: Option<CardAgentPrefs>,
    /// Cards that must reach a `Completed` column before this one may start. §11.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub blocked_by: Vec<CardId>,
    /// A run this card is waiting for a free slot to start. §11.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub pending_run: Option<PendingRun>,
    /// Runs this card has had, oldest first, capped at `MAX_RUNS_PER_CARD`. §11.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub runs: Vec<CardRun>,
    pub created_at: String,
    pub updated_at: String,
}

/// A card carries no permission mode: the mode is its column's policy over every card that
/// passes through it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardAgentPrefs { #[serde(default, skip_serializing_if = "Option::is_none")] pub provider: Option<AgentKind>,
                            #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
                            #[serde(default, skip_serializing_if = "Option::is_none")] pub effort: Option<String> }
impl CardAgentPrefs { pub fn is_empty(&self) -> bool; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRun { pub status_id: StatusId, pub since: String /* RFC 3339, the board clock */ }

/// Identity plus terminal facts. Written twice: when the delegation exists and when it ends.
/// Live progress is never stored here — it belongs to the delegation (§11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardRun {
    pub id: DelegationId,
    /// `None` only when the start failed before a thread existed.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub thread_id: Option<ThreadId>,
    pub status_id: StatusId,
    pub action: ActionKind,
    pub provider: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub effort: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub outcome: Option<RunOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub report_comment_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")] pub files_changed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub tokens: Option<u64>,
}
impl CardRun { pub fn is_live(&self) -> bool; pub fn failed_to_start(&self) -> bool; }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome { Succeeded, NeedsYou, Failed, Incomplete, Cancelled }
impl RunOutcome { pub const fn word(self) -> &'static str; pub const fn needs_attention(self) -> bool; }

pub const MAX_RUNS_PER_CARD: usize = 20;
pub const MAX_REPORT_COMMENTS_PER_CARD: usize = 3;
pub const REPORT_EXCERPT_CAP_BYTES: usize = 8 * 1024;
pub const PENDING_AMBER_AFTER_SECS: u64 = 60;
/// Equal to the daemon's `MAX_LIVE_DELEGATIONS`; a daemon test asserts they agree.
pub const MAX_LIVE_RUNS_PER_BOARD: u32 = 8;
impl Card {
    /// `remote.key` when linked (e.g. "PROJ-123"), else `"{prefix}-{number}"`.
    pub fn display_key(&self, board: &Board) -> String;
    pub fn local_key(&self, board: &Board) -> String;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment { pub id: String, #[serde(default)] pub author: Option<String>, pub body: String,
                     pub created_at: String, #[serde(default)] pub remote_id: Option<String>,
                     /// Set on a run's report excerpt; renders with a run badge instead of an author.
                     #[serde(default, skip_serializing_if = "Option::is_none")] pub run_id: Option<DelegationId> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity { pub at: String, pub kind: ActivityKind, #[serde(default)] pub actor: Option<String>, pub message: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind { Created, Updated, Moved, Commented, WorktreeCreated, Synced, ConflictDetected, ConflictResolved,
                        /// §11. `Moved` stays a human's or the CLI's move; automation writes `AutoMoved`.
                        RunStarted, RunEnded, AutoMoved }

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
    /// Live runs allowed at once across this board. `None` is one. Validated `1..=MAX_LIVE_RUNS_PER_BOARD`.
    /// A board property rather than a column one: every run of one board shares one checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub max_live_runs: Option<u32>,
}
impl BoardSettings { pub fn max_live_runs(&self) -> u32; }   // unwrap_or(1)
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
    /// Standard card fields this board's backend cannot write back, copied from
    /// `BackendSchema::readonly_fields` by `sync::adopt_schema`. §10.
    #[serde(default)] pub readonly_fields: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatusMap { pub remote_to_local: BTreeMap<String, StatusId>, pub local_to_remote: BTreeMap<StatusId, String> }

/// Lightweight row for `Snapshot.boards`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSummary { pub id: BoardId, pub context_id: ContextId,
                          #[serde(default, skip_serializing_if = "Option::is_none")] pub worktree_id: Option<WorktreeId>,
                          pub name: String, pub prefix: String,
                          pub backend_kind: String, pub card_count: usize, pub open_count: usize,
                          pub dirty_count: usize, pub conflict_count: usize,
                          /// Cards with a live or pending run, and cards whose last run wants a human.
                          #[serde(default, skip_serializing_if = "is_zero")] pub working_count: u32,
                          #[serde(default, skip_serializing_if = "is_zero")] pub attention_count: u32,
                          pub last_synced_at: Option<String>, pub last_error: Option<String> }

/// Full board payload for the UI/CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardView { pub board: Board, pub cards: Vec<Card>,
                       /// Joined from the delegation store on read. Never persisted.
                       #[serde(default, skip_serializing_if = "Vec::is_empty")] pub live_runs: Vec<LiveRun> }

/// What a live run is doing right now, joined onto a board view on read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRun { pub card_id: CardId, pub run: DelegationId, pub status: DelegationStatus,
                     #[serde(default, skip_serializing_if = "Option::is_none")] pub headline: Option<String>,
                     pub started: String }

/// On-disk document: `$FLEET_HOME/boards/<board-id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardDocument { pub version: u32, pub board: Board, pub cards: Vec<Card> }
/// Newest version this build writes, and the oldest it reads.
pub const BOARD_DOCUMENT_VERSION: u32 = 2;
pub const BOARD_DOCUMENT_MIN_VERSION: u32 = 1;
/// 2 when any column carries `automation`, the settings carry `max_live_runs`, or any card carries
/// `blocked_by`, `agent`, a `pending_run`, `runs`, or a comment with `run_id`; else 1.
pub fn document_version(board: &Board, cards: &[Card]) -> u32;
```

**The document version bumps lazily.** `BoardStore::save` stamps
`doc.version = document_version(&doc.board, &doc.cards)` before it validates, so a board nobody
automated goes on writing version 1 and a daemon built before this feature goes on reading it. A
board that has opted in writes 2, and keeps writing 2 for as long as any card still carries a link
or a run: an older daemon cannot represent either and would drop the history on its next save.
`load` and `peek` accept `BOARD_DOCUMENT_MIN_VERSION..=BOARD_DOCUMENT_VERSION` and refuse anything
else by name — `board {id} uses document version {v} (this build reads 1..=2)` — without
quarantining the file, because a document this build is too old to read is intact, not damaged.

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
pub const BOARD_ID_MAX_LEN: usize = 64;
pub fn default_statuses() -> Vec<Status>;   // backlog/Backlog, todo/Unstarted, in-progress/Started, done/Completed, canceled/Canceled
pub fn default_prefix(context: &Context) -> String;   // first 3 alnum chars of context name uppercased, fallback "FLT"
pub fn new_board(context: &Context, now: &str) -> Board; // id = context.id as BoardId, name = context.name, default statuses, next_number = 1
pub fn worktree_board_id(worktree: &WorktreeId) -> BoardId; // wt-<owner>-<repo>-<slug>, slugified and capped at 64 bytes
pub fn new_worktree_board(context: &Context, worktree: &Worktree, now: &str) -> Board; // worktree scope, slug name/prefix, worktree repo default

// §11 — the one workflow preset and its text.
pub const PRESET_INSTRUCTIONS_IMPLEMENT: &str = "Implement this card in the current worktree. Do not commit.";
pub const PRESET_EXPECT_IMPLEMENT: &str = "make lint and make test pass";
pub const PRESET_EXPECT_REVIEW: &str = "the review finds no blocking issue";
pub const PRESET_REVIEW_SKILL: &str = "deep-review";
pub fn workflow_preset() -> Vec<Status>;          // backlog, todo, ready, in-progress, in-review, done, canceled
/// Adds the preset columns missing **by `StatusId`**, in preset order relative to the neighbours
/// already present; never touches an existing column. `true` when it changed anything.
pub fn apply_workflow_preset(board: &mut Board) -> bool;
/// `{key}` then `{title}`; anything else in braces is left alone.
pub fn render_template(text: &str, key: &str, title: &str) -> String;
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
    #[serde(default)] pub properties: BTreeMap<String, PropertyValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent: Option<CardAgentPrefs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub blocked_by: Vec<CardId> }

/// `None` = leave unchanged; `Some(None)` = clear. All fields optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardPatch { pub title: Option<String>, pub description: Option<String>,
    pub status_id: Option<StatusId>, pub priority: Option<Priority>, pub labels: Option<Vec<LabelId>>,
    pub assignee: Option<Option<String>>, pub estimate: Option<Option<u32>>, pub due_date: Option<Option<String>>,
    pub parent_id: Option<Option<CardId>>, pub repo_id: Option<Option<RepoId>>,
    pub properties: Option<BTreeMap<String, PropertyValue>> /* merge; Null removes */, pub archived: Option<bool>,
    /// `Some(None)` clears the card's agent preferences; the whole `blocked_by` set is replaced.
    pub agent: Option<Option<CardAgentPrefs>>, pub blocked_by: Option<Vec<CardId>> }
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
    #[error("board already exists for scope {0}")] Duplicate(String),
    #[error("backend `{0}` is not registered")] UnknownBackend(String),
    #[error("backend does not support {0}")] Unsupported(&'static str),
    #[error("backend error: {0}")] Backend(String),
    #[error("card {0} has an unresolved conflict")] Conflicted(String),
}

pub fn validate_board(board: &Board) -> Result<(), BoardError>;   // prefix, unique status/label ids, ≥1 status, unique property keys, nonempty status/label/property names, then validate_automation
pub fn validate_card(board: &Board, card: &Card) -> Result<(), BoardError>; // status/labels exist, property kinds match schema, `Date` property values and `due_date` are real dates
/// Called by `validate_board`: every column's automation block and `settings.max_live_runs` (§11).
pub fn validate_automation(board: &Board) -> Result<(), BoardError>;
/// The five `--env` rules of `fleet subagent run`, without the flag name (§11).
pub fn validate_env(env: &[String]) -> Result<(), BoardError>;
/// One card's `blocked_by` against the board's card set; the daemon calls it beside its parent
/// check on every write path that can set links (§11).
pub fn validate_links(board: &Board, cards: &[Card], card: &Card) -> Result<(), BoardError>;
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
/// `live` is the delegation join a board view carries; a caller with nothing joined passes `&[]`
/// and still counts the runs the cards themselves record. `now` is RFC 3339, for `attention`.
pub fn summarize(board: &Board, cards: &[Card], live: &[LiveRun], now: &str) -> BoardSummary;
/// Derived reads shared by the app, the CLI and the engine (§11).
pub fn blocks<'a>(cards: &'a [Card], card: &CardId) -> Vec<&'a Card>;
pub fn is_satisfied(board: &Board, cards: &[Card], blocker: &CardId) -> bool;
pub struct Blocked { pub unsatisfied: u32, pub tone: BlockedTone }
pub enum BlockedTone { Muted, Warning }
pub fn blocked(board: &Board, cards: &[Card], card: &Card) -> Option<Blocked>;
pub fn latest_run(card: &Card) -> Option<&CardRun>;
pub fn attention(card: &Card, now: &str) -> bool;
/// Drops an automation block that asks for nothing; `apply_board_patch` and
/// `apply_workflow_preset` both call it after they set `statuses`.
pub fn normalise_automation(statuses: &mut [Status]);
pub fn default_true() -> bool; pub fn default_branch_template() -> String; pub fn is_zero(count: &u32) -> bool;
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
    pub(crate) fn quarantined_documents(&self) -> DaemonResult<BTreeMap<BoardId, Vec<PathBuf>>>; // one directory scan for id lookup
    pub fn stamp(&self, id: &BoardId) -> Option<(u64, SystemTime)>;          // size+mtime; memoizes summaries
    pub fn save(&self, doc: &BoardDocument) -> DaemonResult<()>;
    pub fn delete(&self, id: &BoardId) -> DaemonResult<()>;   // board and its quarantined remains to trash_dir
}
// FleetHome (contracts stage): pub fn boards_dir(&self) -> PathBuf /* $FLEET_HOME/boards */; pub fn board_path(&self, id: &BoardId) -> PathBuf
```

```rust
// crates/fleet-daemon/src/services/boards.rs
pub struct Boards { /* store, state_store (contexts/repos/worktrees lookup), remote_worktrees: OnceLock<Arc<dyn RemoteWorktrees>> (the mirror, installed after composition; card links only), backends, clock, jobs, worktrees: Arc<Worktrees>, events broadcaster, index: RwLock<HashMap<CardId, BoardId>> */ }
impl Boards {
    pub fn new(store: Arc<BoardStore>, state_store: Arc<StateStore>, backends: BoardBackends, clock: Arc<dyn Clock>, jobs: Arc<JobManager>, worktrees: Arc<Worktrees>, events: BroadcastBus) -> Self;
    /// Skips a document this build cannot read; a `context` that does not exist is `NotFound`.
    pub async fn list(&self, context: Option<&ContextId>) -> DaemonResult<Vec<BoardSummary>>;
    pub async fn get(&self, id: &BoardId) -> DaemonResult<BoardView>;
    /// Get-or-create the context's unscoped board (`defaults::new_board`). Errors if the context does not exist.
    pub async fn ensure(&self, context: &ContextId) -> DaemonResult<BoardView>;
    pub async fn create(&self, context: &ContextId, name: Option<String>, prefix: Option<String>, backend: Option<BackendRef>) -> DaemonResult<BoardView>;
    /// Finds the board whose persisted scope names this worktree; the derived id is only a fast path.
    pub(super) fn worktree_board(&self, worktree: &WorktreeId) -> DaemonResult<Option<BoardId>>;
    /// Get-or-create the board scoped to one worktree this daemon published
    /// (`defaults::new_worktree_board`). A worktree only the mirror names is `NotFound`: its board
    /// is the owning daemon's document and the router forwards the request there.
    pub async fn ensure_for_worktree(&self, worktree: &WorktreeId) -> DaemonResult<BoardView>;
    /// Create the worktree's only board; a second board is `BoardError::Duplicate`.
    pub async fn create_for_worktree(&self, worktree: &WorktreeId, name: Option<String>, prefix: Option<String>, backend: Option<BackendRef>) -> DaemonResult<BoardView>;
    /// A patch that changes nothing writes nothing and emits nothing, as an empty card patch does.
    pub async fn update(&self, id: &BoardId, patch: BoardPatch) -> DaemonResult<BoardView>;   // validates+normalizes backend settings via the registry, and only when the patch changed the BackendRef: a rename or a label must not wait on (or fail with) a backend it never mentioned
    pub async fn delete(&self, id: &BoardId) -> DaemonResult<()>;
    /// Deletes every board of a context; the `DeleteContext` cascade calls it before the context goes.
    /// An unreadable document is preserved because its persisted scope cannot be verified from
    /// the filename in the board id space shared by contexts and worktrees.
    pub async fn delete_for_context(&self, context: &ContextId) -> DaemonResult<()>;
    /// Bundles the scoped board into the worktree's trash entry after the worktree moves there.
    pub async fn delete_for_worktree(&self, worktree: &WorktreeId, trash: &Path) -> DaemonResult<()>;
    /// Restores a scoped board bundled inside a restored worktree directory.
    pub async fn restore_for_worktree(&self, worktree: &WorktreeId, destination: &Path) -> DaemonResult<()>;
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
The late-bound deletion seam keeps `Worktrees` independent of `Boards`, which already depends on
`Worktrees` for card-to-worktree creation. `Worktrees` stores the observer weakly so composing the
two services does not keep either allocation alive:

```rust
#[async_trait::async_trait]
pub(super) trait WorktreeCascade: Send + Sync {
    async fn delete_for_worktree(&self, worktree: &WorktreeId, trash: &Path) -> DaemonResult<()>;
    async fn restore_for_worktree(&self, worktree: &WorktreeId, destination: &Path) -> DaemonResult<()>;
}
impl Worktrees {
    pub(super) fn set_cascade(&self, cascade: Arc<dyn WorktreeCascade>);
}
```
A second late-bound seam, in the same shape and for the same reason, gives `Boards` the worktrees
other hosts own. `Mirror` is composed after `Boards`, and `Boards` must not depend on the router:

```rust
pub(super) trait RemoteWorktrees: Send + Sync {
    fn worktrees(&self) -> Vec<Worktree>;   // impl'd for services::mirror::Mirror
}
impl Boards {
    pub(super) fn set_remote_worktrees(&self, remote: Arc<dyn RemoteWorktrees>);
}
```
That seam serves **card links only**. A card on a context board may link a worktree another host
owns — `CreateWorktreeFromCard { host }` creates exactly that — and the link stays live while the
mirror names the worktree, so the card scrub in `documents.rs`, the `get` scrub in `lifecycle.rs`
and the repository check in `cards.rs` read local state first and the mirror second. Board **scope**
is local-only: a worktree board is this daemon's document only while its worktree is in
`State.worktrees`. `ensure_for_worktree` and `create_for_worktree` for a worktree only the mirror
names are therefore `not found: worktree <id>`, and a document scoped to such a worktree is skipped
by `list`/`summaries`. That board belongs to the daemon that owns the worktree, and a client reaches
it through its local daemon's router (`docs/REMOTE-MACHINES.md` §6,
`docs/decisions/0021-hosted-worktree-boards-route-to-owner.md`). A document this daemon wrote for a
worktree a host owns is retired rather than served:

```rust
/// Moves this daemon's own document for a worktree another host owns into the trash.
pub async fn retire_hosted_worktree_board(&self, worktree: &WorktreeId, host: &HostId) -> DaemonResult<Option<RetiredBoard>>;
pub struct RetiredBoard { pub board: BoardId, pub path: PathBuf, pub cards: usize }
```
Dispatch calls it before every `EnsureWorktreeBoard`/`CreateWorktreeBoard` the router routes to a
host — a no-op once nothing is left — and still forwards when it fails, with the failure warned. It
finds the document whose persisted `worktree_id` names that worktree (`worktree_board`), moves it and any
quarantined remains to the trash through `BoardStore::delete`, drops it from the `summaries` memo
and the card `index`, and logs exactly one line: `info` naming the trashed path for a document with
no cards, `warn` naming the path and the card count for one that held cards, so those cards can be
re-entered by hand. It returns `Ok(None)` when there is nothing to retire, and never merges two
documents.

Every mutation: load doc → apply pure op → `validate_card` → save → emit `Event::BoardChanged`.
Cards whose `worktree_id` names neither a worktree in `State.worktrees` nor a mirrored one are
reported with `worktree_id: None` (not persisted), and a `repo_id` — on a card or as
`Board.default_repo_id` — naming a repository the state no longer has in the board's context is
reported the same way and skipped when `create_worktree_from_card` picks a repository. `ensure`/`create` refuse a context whose
board document is quarantined rather than creating an empty board over it. Cascades preserve both
a live document this build cannot read and any quarantined remains, because their persisted
context/worktree scope cannot be inferred safely from the shared board id. Explicit board deletion
can remove quarantined remains because the caller supplies the authoritative board identity.
`context_board` accepts only a board with the requested `context_id` and no `worktree_id`, on both
its derived-id fast path and its fallback scan;
`worktree_board` similarly treats the derived id only as a fast path and falls back to the
persisted `worktree_id`. Creating either scope appends `-2` through `-99` when another scope
already occupies its default id, and refuses creation if all candidates are occupied. `summaries`
reparses a board document only when its `stamp` changed or
this daemon rewrote it. Boards whose context no longer exists are skipped by `list`/`summaries`;
a worktree board is skipped as well once local state no longer names its
worktree. Deleting a context deletes its boards in the same cascade (`delete_for_context`) so a
later context deriving the same id cannot adopt one. After any worktree deletion moves the worktree to trash, the late-bound
`WorktreeCascade` bundles the board document inside that same trash entry. Restoring the worktree
restores its board and cards; expiry removes both together. A cascade failure is warned and
swallowed because the lifecycle move has already committed. An unreadable restored board stays in
place without being quarantined again, so a newer build or manual repair can recover it. Board
locks are per board: no board's clone, sync or hook run blocks another board's requests. Creating a
missing worktree board also takes that worktree's lifecycle claim through its save, so deletion and
restore cannot pass its missing-board read. Board creation additionally holds one short allocator
claim from suffix selection through the document save, preventing distinct base ids from reserving
the same suffix; backend validation stays outside that claim. Reads of existing boards remain
lock-free. Repository moves use the context lifecycle gate and every affected worktree lifecycle
claim while rewriting the scoped board documents and repository state; a failed state publication
rolls those documents back to their prior context.

## 5. Protocol (`fleet-proto`, version 8)

Worktree-board requests are an additive protocol-8 extension advertised through the
`board.worktree` capability. The capability lets clients avoid sending variants an older daemon
cannot decode without forcing every local and remote daemon to upgrade in lockstep.

```rust
pub const BOARD_WORKTREE_CAPABILITY: &str = "board.worktree";
```

```rust
// RequestBody discriminants and fields use snake_case, like their siblings; domain payloads use camelCase.
ListBoards { context_id: Option<ContextId> }                       → ResponseBody::Boards(Vec<BoardSummary>)
GetBoard { board_id: BoardId }                                      → Board(BoardView)
EnsureBoard { context_id: ContextId }                               → Board(BoardView)
EnsureWorktreeBoard { worktree_id: WorktreeId }                    → Board(BoardView)
CreateBoard { context_id, name: Option<String>, prefix: Option<String>, backend: Option<BackendRef> } → Board
CreateWorktreeBoard { worktree_id, name: Option<String>, prefix: Option<String>, backend: Option<BackendRef> } → Board
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
`fleet_client::Client` has one typed method per request above (`list_boards`, `get_board`,
`ensure_board`, `create_board`, `update_board`, `delete_board`, `create_card`, `update_card`,
`move_card`, `delete_card`, `add_card_comment`, `create_worktree_from_card`, `sync_board`,
`resolve_card_conflict`, `describe_board_backend`) plus these worktree-scope additions:

```rust
pub async fn ensure_worktree_board(&self, worktree_id: WorktreeId) -> Result<BoardView>;
pub async fn create_worktree_board(
    &self,
    worktree_id: WorktreeId,
    name: Option<String>,
    prefix: Option<String>,
    backend: Option<BackendRef>,
) -> Result<BoardView>;
```
Both typed worktree methods check `board.worktree` before enqueueing a request, and the connection
actor checks again against the currently negotiated connection immediately before writing it. A
request queued across reconnect therefore cannot send a new variant to an older replacement
daemon; every consumer gets the same restart guidance. When the worktree belongs to a host, the
local daemon checks that host's advertised capability once more before forwarding and refuses with
the same sentence prefixed `host <id>: ` — "this daemon does not support worktree boards; run
`fleet daemon restart`" — so the old daemon on the other machine is named rather than the one the
client is talking to (`docs/REMOTE-MACHINES.md` §6).

CLI (`fleet board …`, JSON envelopes v1 with `--json`, human tables otherwise; board resolved from
`--board <id>` else `--worktree[=<owner/name#slug>]` else `--context <id>` else the active context
via `EnsureBoard`; a bare `--worktree` resolves `FLEET_SESSION` against one snapshot — its
sessions first, then its agent threads, because in a native agent thread `FLEET_SESSION` is the
thread id and names no session — while an explicit id requires `=` so a subcommand name is never
consumed as the optional value):

```rust
pub enum BoardWorktreeSelector { Explicit(WorktreeId), FromSession }
```

```
fleet board show [--context C|--worktree[=W]|--board B]           # columns + cards
fleet board list                                                  # summaries with context/worktree scope
fleet board create [--context C|--worktree[=W]] [--name N] [--prefix P] [--backend local|jira] [--setting k=v]...
fleet board set [--name] [--prefix] [--default-repo owner/name] [--clear-default-repo] [--start-on-worktree [true|false]] [--conflict-policy manual|remote_wins|local_wins] [--push-new-cards [true|false]] [--branch-template "{key}-{slug}"] [--add-label NAME]... [--remove-label L]... [--backend KIND] [--setting k=v]...
fleet board backends                                              # registered kinds, capabilities, setting keys
fleet board describe [--context C|--worktree[=W]|--board B]       # what this board's backend reports about itself
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
`board list` accepts `--board` to narrow the table and `--context` to restrict the daemon query;
it rejects `--worktree` because listing does not ensure or resolve a board.
`<key|id>` accepts a display key (`FLT-12`, `PROJ-123`) or a CardId, and the local key of a card
with no remote link — a mirrored card's local key is not a selector, because a board mirroring the
Jira project its own prefix names would have two namespaces of the same shape overlapping. `board
card show` prints `Local key:` for exactly the cards that answer to one.

## 7. UI-kit components (`fleet-ui-kit`, gpui only, tokens only)

```rust
// input.rs — the one text editor, single-line and multi-line modes of the same entity (ADR 0020).
// A description or a comment is an `Entity<TextInput>` the dialog owns; DESIGN-SYSTEM §6.4 is its contract.
pub struct TextInput { /* focus handle, InputBuffer, selection, undo, IME bridge, layout cache, scroll */ }
impl TextInput {
    pub fn new(mode: InputMode, cx: &mut Context<Self>) -> Self;   // InputMode::Multiline { min_rows, max_rows }
    pub fn text(&self) -> &str; pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>);
    pub fn set_label(&mut self, label: Option<SharedString>, cx: &mut Context<Self>);
    pub fn set_placeholder(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>);
    pub fn set_mono(&mut self, mono: bool, cx: &mut Context<Self>); pub fn set_invalid(&mut self, message: Option<SharedString>, cx: &mut Context<Self>);
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>);
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

// kanban_column.rs — a column: header (name, count, category color), virtualized body of tiles, empty hint
pub struct KanbanColumn { /* RenderOnce */ }
impl KanbanColumn {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self;
    pub fn count(self, usize) -> Self; pub fn accent(self, Option<Hsla>) -> Self /* tokens only at call site */;
    pub fn focused(self, bool) -> Self; pub fn width(self, Pixels) -> Self;
    pub fn empty_hint(self, impl Into<SharedString>) -> Self;
    pub fn tiles(self, impl IntoIterator<Item = AnyElement>) -> Self;   // a fixed handful of rows
    pub fn rows(self, ListState, usize, impl FnMut(usize, &mut Window, &mut App) -> AnyElement) -> Self;  // data-bounded, virtualized
    pub fn list_state() -> ListState;                                   // the column's own overdraw
    pub fn scroll_handle(self, ScrollHandle) -> Self;                   // only meaningful with tiles(..)
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
  A worktree Workspace's `fleet://board` tab (`ctrl-s b`) is the board's second surface and
  shows `EnsureWorktreeBoard(worktree)`. It builds nothing of its own: `Shell` owns the one
  `BoardScreen` and lends it to whichever surface is drawing, since the two are never visible
  together. `WorkspaceScreen::sync_board_scope` points the mirror when that tab is the session's
  active one and gives it back when it is not, and the Hub's own observation takes the context
  scope back, so a worktree scope never outlives the pane that asked for it.
  The tab is created on demand — `prefix::OpenBoard` (`ctrl-s b`) selects the session's terminal
  whose command is `fleet://board`, or asks for one with
  `NewTerminal { name: "board", command: "fleet://board", cwd }` and selects the reply — and is
  never written into `windows[]` by Fleet, so a slept session loses it and `ctrl-s b` brings it
  back. A user may add the `windows[]` entry themselves; the tab is matched by its reserved
  command, never by its name, so a renamed or user-configured one is still the board tab.
- **State** (`state.rs`): `AppState.board: BoardState { scope: Option<BoardScope>,
  view: Option<BoardView>, loading: bool,
  error: Option<String>, focus: BoardFocus { column: usize, row: usize }, filter: String,
  filter_editing: bool, group_secondary: Option<GroupBy> }`. Loaded on tab open / context switch
  (`EnsureBoard`), refreshed on `Event::BoardChanged` for the shown board id.
  The active context's unscoped `AppState.snapshot.boards` summary drives the tab badge (open
  count, conflict dot); worktree-scoped summaries never contribute to the Hub tab.
  `BoardScope` is `Context(ContextId) | Worktree(WorktreeId)`: one mirror holds either the
  active context's board or one worktree's, since the Hub and the Workspace are never visible
  together. `None` means the Hub's default, the active context. A view is applied only when the
  scope admits it — a worktree's board carries that worktree, a context's board carries that
  context and **no** worktree — while `apply_card` and `board_stale` keep keying on the board id
  that is on screen.
- **Bridge**: board requests use `Bridge::request` reply receivers, with no new `BridgeEvent`
  variants. Responses land in `AppState` reducers (`apply_board_view`, `apply_card`); board loads
  use scope/generation guards, and the scope picks the request:
  `EnsureBoard { context_id }` or `EnsureWorktreeBoard { worktree_id }`. The app never derives a
  board id of its own from a context or a worktree (§0). A scope change strands the previous
  scope's replies through the same generation counter a context switch uses.
  `screens::board::{enter_context_scope, enter_worktree_scope}` point the mirror and load;
  entering a worktree scope is refused on a daemon that does not advertise `board.worktree`,
  with the CLI's own sentence as a toast — "this daemon does not support worktree boards; run
  `fleet daemon restart`" — and no change to the scope or the shown board.
- **Dialogs** (`Dialogs` variants; serializable drafts and live `Entity<TextInput>` owners in
  `DialogHost`): `CardDetail` (`dialogs/card_detail.rs`,
  `CardDetailState`), `CardCreate` (`dialogs/card_create.rs`), `CardPicker`
  (`dialogs/card_picker.rs`, `PickerKind { Status, Priority, Assignee, Labels, Estimate, DueDate,
  Repo, Property(key) }`), `BoardSettings` (`dialogs/board_settings.rs`: name, prefix, default
  repo, start-on-worktree, push-new-cards, conflict policy, and the backend — a kind cycler over
  `ListBoardBackends` plus one generic row per `settings_schema` entry; see `docs/BOARD-JIRA.md`
  §6).
- **Card detail layout** (UX-SPEC §board): two panes. Left: key + title (editable, `i`),
  description (`MarkdownText`; `d` opens the shared multi-line `TextInput`;
  `ctrl-s`/`esc` saves/cancels), comments (list + `c` to add through that input), activity
  (last 10). Right: property list —
  Status, Priority, Assignee, Labels, Estimate, Due, Parent, Repo, Worktree (enter = open its
  session), Remote (key/url/synced/dirty), then custom properties from `board.properties`; `j/k`
  select row, `enter` opens the matching picker. Conflict banner with `K` keep-local / `R`
  take-remote when `card.conflict` is set.
- **Keymap** (`docs/KEYMAP.md` rows, contexts `Hub > Board` and `Dialog > CardDetail` etc., one
  action per row). Every `Hub > Board` row below is bound a second time, verbatim and against
  the same action, on `Workspace > Native > Board` — the board pane's key context — and
  `Filter > BoardFilter` serves both surfaces. `o` answers "Already in this worktree" instead of
  re-opening the session when the card names the worktree the pane is standing in:

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

The app refreshes through the request its scope names (`EnsureBoard(active_context)` on the Hub)
after BoardChanged. `filter_editing`
selects the Filter key context while typing, with two-stage Escape. `group_secondary` is reserved;
Parent is read-only in this milestone. Card detail is an 880 px two-pane dialog. `ctrl-enter` in
CardCreate creates and opens detail; label pickers use Space for multi-select; BoardSettings
reuses the settings row keys. Delete uses `ConfirmRequest::DeleteCard`. These supplemental dialog
keys are listed in KEYMAP; create-and-open exists only as that chord inside the dialog and has no
palette command.
Description/comment editors are multi-line `TextInput` entities the dialog creates when an edit
begins and drops when it ends; no surface decodes editing keys (ADR 0020). Card tiles suppress None priority, while standalone
PriorityGlyph still renders it. Label colors remain token names.

## 9. What the tests hold

The board's regression surface is spread across the crates it touches, and each layer holds one
thing so a failure names the layer that broke.

- **Core** (`crates/fleet-core/src/board/`) covers the pure rules: create, patch, move and the
  fractional positions they produce; validation and `worktree_slug`; and the reconciliation engine
  — `adopt_schema`'s status mapping, `reconcile`'s create/update/delete/conflict decisions under
  each `ConflictPolicy`, the push operations it emits, and `apply_push_result`. Worktree-board
  cases pin legacy JSON without `worktreeId`, scoped JSON round trips, derived ids (including
  normalization and length), and the scoped board's name, prefix, context, and default repository.
- **Daemon** (`crates/fleet-daemon/tests/boards_*.rs`) covers the service against a real store: the
  ensure/create/patch/move/delete/comment round trip, persistence and quarantine, worktree-from-card
  over a fake git, a full sync against `FakeBackend` including a conflict and its resolution, and
  the events each mutation emits. Worktree-board cases pin scope-aware context lookup and listing,
  orphan filtering, id-collision suffixes, idempotent ensure, duplicate and missing-worktree errors,
  and deletion through both direct and prune paths without touching the context board.
- **Protocol** (`crates/fleet-proto/tests/compatibility.rs`) pins byte-exact frames for both new
  requests, the `board.worktree` handshake capability, and old/new `Board` and `BoardSummary`
  payload compatibility.
- **Client** covers one daemon-socket round trip for each new typed method.
- **CLI** covers bare and explicit `--worktree` parsing, selector conflicts, `FLEET_SESSION`
  resolution from both a worktree session and a native agent thread, the refusal cases, capability
  gating, worktree creation, and the scope column/header.
- **App** covers the reducers rather than rendered strings: the board mirror's staleness and
  generation rules, the focus clamp under a filter, and the two-stage filter `Esc`. The keymap
  drift test keeps `docs/KEYMAP.md` and `keymap.rs` in agreement.
  The scope is held by `state/board/tests.rs`:
  `a_view_from_the_other_scope_never_lands_in_the_board_slot` (a context board and a worktree
  board name the same context, and only `worktree_id` tells them apart),
  `a_scope_switched_away_from_and_back_rejects_the_answer_it_left_behind` (A → B → A through the
  one generation counter), `a_board_changed_event_only_makes_the_board_on_screen_stale`, and
  `a_daemon_without_worktree_boards_refuses_the_scope_and_keeps_the_board_it_shows`.
  The pane's key context is held by `state/navigation/tests.rs`
  (`the_board_pane_publishes_its_own_key_context`, `only_the_board_tab_publishes_the_board_word`),
  and `ctrl-s b` itself by `screens/workspace/tests.rs`
  (`the_board_key_selects_the_tab_the_session_already_has`,
  `the_board_key_creates_the_tab_and_selects_the_reply`,
  `the_board_tab_is_recognised_by_its_command_not_its_name`,
  `the_board_key_says_why_a_session_without_a_worktree_opens_nothing`,
  `the_board_band_needs_a_worktree_to_be_the_board_of`,
  `selecting_another_tab_ends_a_pending_board_claim`,
  `the_reserved_command_decides_the_native_tab_kind`).
  Five `shell/root/tests.rs` tests drive the real shell end to end — the refusal on an old daemon,
  the palette row reaching the same handler, the pane binding every board key while `ctrl-s` stays
  the prefix, the scope going back to the context on the way out, and `o` refusing the worktree
  the pane is standing in
  (`real_shell_board_key_refuses_a_daemon_without_worktree_boards`,
  `real_shell_board_palette_row_reaches_the_same_handler`,
  `real_shell_board_pane_binds_the_board_keys_and_keeps_the_prefix`,
  `real_shell_board_pane_gives_the_prefix_and_the_scope_back`,
  `real_shell_board_pane_refuses_to_reopen_its_own_worktree`). The palette row's own gate is
  `the_board_tab_row_needs_a_worktree_session_on_screen`, and the reserved command is pinned in
  `fleet-core` by `only_the_process_backed_reserved_command_degrades_when_proxied` and in the
  settings dialog by `every_reserved_window_command_reads_as_built_in`.
- **Harness** drives the tab as a user does: `scenarios/workspace/board-tab.scenario` opens a
  worktree session, presses `ctrl-s b`, and asserts the new tab, its `native` badge, the
  `Workspace > Native > Board` key context, this worktree's own cards, a `]` that reaches the
  pane, a second `ctrl-s b` that creates no fourth tab, and — after `ctrl-s s` and `g b` — the
  Hub still showing the **context** board, unmoved. The `board` fixture preset is what makes that
  last part an oracle: it seeds a context board of `FLT-…` cards and the worktree's own `FEA-…`
  board under the same context, so the card set alone says which of the two a surface is drawing.
  The Hub's `scenarios/board/*` keep covering the context board on its own.

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

## 11. Automation — the model

**What this section covers.** Everything below is *the model*: the shapes a board document may
now hold, the rules that refuse a bad one, and the reads every surface derives from them. Nothing
in this build serves it. The engine that acts on a card entering an automated column, and the
three run requests that let a client start, cancel and wait for a run, arrive in **phase 3**; the
`fleet board` verbs that drive them in **phase 5**; the tile marks, the card-detail run row and
the Board settings Columns pane in **phases 6 to 8**. `board.automation`
(`fleet_proto::response::BOARD_AUTOMATION_CAPABILITY`) is defined and deliberately **not**
advertised, so a daemon built from this phase behaves for every client exactly as the one before
it did. [ADR 0022](decisions/0022-board-workflows.md) records why.

### 11.1 What a column does

A `Status` may carry one optional `ColumnAutomation` (§2). It has three independent parts:

| Field | Meaning |
| --- | --- |
| `on_enter` | The `Action` to run on a card that enters this column. |
| `on_success` | Where the card goes when that run succeeds. |
| `advance_when_unblocked` | Where a card *waiting here* goes once every card blocking it is satisfied. |

An `Action` is either `ActionKind::Prompt` — run the card's own brief — or
`ActionKind::Skill { name, args }`, which invokes one of the agent's skills. `instructions` is
prepended to the brief and `expect` is printed in the run's footer as `The card expects: …`.
`render_template` substitutes `{key}` and `{title}` in `instructions` and in each `env` value;
`expect` is printed as written.
`agent` (`ColumnAgentPrefs`) is what the column asks for; a card's own `agent` (`CardAgentPrefs`)
wins over it. Permission `mode` exists only on the column: it is the column's policy over every
card that passes through it, and a card able to widen it would be a card able to grant itself
access its column deliberately withheld.

A block whose every field is empty is not automation. `ops::normalise_automation` turns one into
`None`, so a column a user has just cleared stops answering `automation.is_some()` and stops
holding the document at version 2. `apply_board_patch` and `apply_workflow_preset` both call it.

`settings.max_live_runs` is the board's throttle, `None` meaning one. It is a property of the
board rather than of a column because every run of one board edits the same checkout.

### 11.2 What a card remembers

`agent`, `blocked_by`, `pending_run` and `runs` (§2). `runs` is oldest first, capped at
`MAX_RUNS_PER_CARD` (20) with the oldest dropped. A `CardRun` is written twice — once when the
delegation exists, once when it ends — so a run is visible while it works and legible long after.

Live progress is never stored on the card. It belongs to the delegation, and a card is not a
mirror of one: what is live arrives joined onto `BoardView.live_runs` on read and is never
persisted. A run whose start failed has `thread_id: None` (`failed_to_start()`), which is the one
case where a `CardRun` exists with no thread to attach to.

Report excerpts live in the card's comments, with `Comment.run_id` set. A body is capped at
`REPORT_EXCERPT_CAP_BYTES` (8 KiB) and a card keeps at most `MAX_REPORT_COMMENTS_PER_CARD` (3);
the oldest is dropped past that, clearing its run's `report_comment_id`.

### 11.3 The activity sentences

`ActivityKind` gains `RunStarted`, `RunEnded` and `AutoMoved`. The exact text, so every writer
copies rather than invents it:

| Kind | Message |
| --- | --- |
| `RunStarted` | `Run started · {provider} · {model} · {effort}` — a missing model or effort drops with its separator; provider is `AgentKind::executable()`. |
| `RunEnded` | `Run ended · {outcome word} · {Nm SSs}`, plus ` · ${cost:.2}` when the cost is known. |
| `AutoMoved` | `Moved to {column name}: unblocked by {KEY} reaching {column name}`. |
| `Updated` | `Run canceled: {column name} no longer runs an action`, when a column loses its action. |
| `Updated` | `Unblocked: {KEY} was deleted`, when a blocker is deleted. |

`Moved` keeps today's text and is written **only** by a human's or the CLI's move. An outcome move
is an `AutoMoved` reading `Moved to {column name}: run succeeded`. That separation is what lets
`attention` tell the two apart: a card wants a human while its latest run needs one and no `Moved`
entry is newer than that run's `ended_at`.

### 11.4 The refusals

Every one is `BoardError::Invalid { field, reason }`, and every surface prints the sentence
verbatim.

| field | reason |
| --- | --- |
| `on_success` / `advance_when_unblocked` | `must name a status on this board` · `may not name its own column` · `must name a later column` |
| `on_enter` | `a skill action needs a name` · `skill actions run on claude only; put the invocation in the column's instructions for codex` |
| `env` | the five `fleet subagent run --env` sentences with `--env ` dropped: not `KEY=VALUE`, an empty key, a `FLEET_`-prefixed key, `PATH`, the same key twice |
| `max_live_runs` | `must be between 1 and 8` |
| `blocked_by` | `{KEY} is not on this board` · `a card cannot block itself` · `would close a cycle: {KEY} → {KEY} → {KEY}` |
| `automation` | `automation is available on worktree boards only` · `automation is available on local boards only` · `automation is unavailable on a worktree owned by host {host}` |

Routing may only ever point forward. A column that sent a card back would let one run's success
start the run of a column the card had already passed, and the pair would trade the card between
them for as long as the runs kept succeeding — a loop no later refusal can break, because every
individual move in it is legal.

The cycle sentence lists the path in display keys from the card being written back to itself, so a
two-card cycle reads `would close a cycle: FLT-1 → FLT-2 → FLT-1`. A blocker that is not on this
board is named by its card id, which is the only name the board has for it.

The three `automation` sentences are raised by the daemon (phase 3), not by `fleet-core`; they are
fixed here so the app and the CLI show the same words. `fleet-cli`'s own `child_environment` keeps
its `--env `-prefixed sentences and is not changed by this feature.

### 11.5 The derived reads

None of these is persisted; all are pure functions of a board and its cards.

| Read | Answers |
| --- | --- |
| `blocks(cards, card)` | The cards this one blocks — the reverse of everyone's `blocked_by`. |
| `is_satisfied(board, cards, blocker)` | Whether a blocker has reached a `Completed` column. A canceled card is a decision not to do the work, not a report that it is done, so it never satisfies. |
| `blocked(board, cards, card)` | `Some(Blocked { unsatisfied, tone })` while any blocker is unsatisfied. `tone` is `Warning` when one of them is canceled, archived or no longer on the board — nothing will release this card on its own — and `Muted` otherwise. |
| `latest_run(card)` | `runs.last()`. |
| `attention(card, now)` | Whether a person has to look: the latest run's outcome `needs_attention()` with no later manual `Moved`, or a `pending_run` older than `PENDING_AMBER_AFTER_SECS` (60). |

`summarize` fills `BoardSummary.working_count` (a live or pending run) and `attention_count` from
the same two, which is how the board list and the pane header count them.

### 11.6 The workflow preset

`workflow_preset()` is the one shipped pipeline; `apply_workflow_preset(board)` adds the columns a
board is missing **by `StatusId`**, in preset order relative to the neighbours already present,
and never touches an existing column's name, category, colour or automation.

| id | Name | Category | On enter | On success | When unblocked |
| --- | --- | --- | --- | --- | --- |
| `backlog` | Backlog | Backlog | — | — | — |
| `todo` | Todo | Unstarted | — | — | — |
| `ready` | Ready | Unstarted | — | — | `in-progress` |
| `in-progress` | In Progress | Started | prompt, `PRESET_INSTRUCTIONS_IMPLEMENT`, expects `PRESET_EXPECT_IMPLEMENT` | `in-review` | — |
| `in-review` | In review | Started | skill `deep-review`, expects `PRESET_EXPECT_REVIEW` | `done` | — |
| `done` | Done | Completed | — | — | — |
| `canceled` | Canceled | Canceled | — | — | — |

Todo stays human on purpose and Ready is the routing column: a card is put in Ready once it is
meant to run, and `advance_when_unblocked` releases it into In Progress as soon as every card
blocking it is done. Nothing a person leaves in Todo can start itself. The preset sets no
`max_live_runs`, which leaves the board at one live run — a second concurrent run over one
checkout is a decision its owner makes deliberately.

Because an existing column is never rewritten, applying the preset to a board built from
`default_statuses()` adds Ready and In review but leaves the shipped `in-progress` column with
**no** `on_enter` action: that column already exists, and its automation is its owner's. A board
that wants the whole pipeline either starts from `workflow_preset()` or edits `in-progress`
afterwards.
