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
- [ADR 0024](decisions/0024-review-boards.md) adds a second board per context, the Reviews board:
  cards carry a pull request, and each card runs in its own pull-request worktree.
- [ADR 0025](decisions/0025-scheduled-agent-tasks.md) adds board-owned schedules that run a prompt
  headless as a daemon job; §12 is their model.

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

A context may also hold one **Reviews board** (ADR 0024): a second context board whose cards each
carry a pull request someone asked the user to review, whose columns run an agent review in each
pull request's own worktree, and which the Pull requests screen's Review tab shows. Board-owned
**schedules** (§12, ADR 0025) run a prompt headless on a cadence and fill it with cards.

Non-goals for v1: multiple boards per scope in the UI (the model allows it; the Hub shows the
context's task board, a worktree's Workspace tab shows that worktree's, the Pull requests screen's
Review tab shows the context's Reviews board, and no surface shows two at once), cycles/projects/milestones, attachments, rich-text editing beyond a plain
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
| Schedules: model, store, service, CLI | `crates/fleet-core/src/schedule.rs`, `crates/fleet-daemon/src/stores/schedules.rs`, `crates/fleet-daemon/src/services/schedules/{mod,runner,tick}.rs`, `crates/fleet-cli/src/commands/schedules.rs` (§12) |
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
    /// `Tasks` for every board before review boards; `Reviews` for a context's Reviews board (§4, ADR 0024).
    #[serde(default, skip_serializing_if = "BoardKind::is_tasks")] pub kind: BoardKind,
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

/// What a board is for. Chooses nothing in the engine by itself: a Reviews board's behaviour comes
/// from its preset columns and its `run_location`, and `kind` is what lookup and the app key on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BoardKind { #[default] Tasks, Reviews }
impl BoardKind { pub fn is_tasks(&self) -> bool; }

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
    /// Prepended to the brief. Markdown. `{key}` and `{title}` are substituted, and on a card with a
    /// pull request `{pr_url}`, `{pr_repo}` and `{pr_number}` (`render_card_template`).
    #[serde(default, skip_serializing_if = "String::is_empty")] pub instructions: String,
    /// Printed in the run's footer as `The card expects: …`. May be empty.
    #[serde(default, skip_serializing_if = "String::is_empty")] pub expect: String,
    #[serde(default, skip_serializing_if = "ColumnAgentPrefs::is_empty")] pub agent: ColumnAgentPrefs,
    /// `KEY=VALUE`; the value rendered with `render_card_template`. `validate_env` applies the five `--env` rules.
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
    /// The pull request this card reviews. Set at creation only, never patched, unique per board.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub pull_request: Option<PullRequestRef>,
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

/// A GitHub pull request. `url` is always the normalised `https://github.com/<owner>/<name>/pull/<n>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestRef { pub repo: RepoId, pub number: u64, pub url: String }
impl PullRequestRef {
    /// Accepts `https://github.com/<owner>/<name>/pull/<n>` (optionally `http://` or `www.`, and
    /// anything after the number — `/files`, `/commits/…`, `#…`, `?…`) and `<owner>/<name>#<n>`,
    /// `n > 0`. Anything else: `invalid pull_request: expected a GitHub pull request URL or owner/name#number`.
    pub fn parse(text: &str) -> Result<PullRequestRef, BoardError>;
    pub fn key(&self) -> String;   // "<owner>/<name>#<n>"
}

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
    /// The worktree this run executed in. Written for every new run; `None` on runs recorded
    /// before review boards, which ran in the board's worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub worktree_id: Option<WorktreeId>,
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
    /// Where this board's runs execute: `BoardWorktree` in `board.worktree_id` (every board before
    /// review boards), `CardWorktree` in each card's own `card.worktree_id`, created from the card's
    /// pull request when missing (§4.1). Not editable in the app in v1.
    #[serde(default, skip_serializing_if = "RunLocation::is_board_worktree")] pub run_location: RunLocation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunLocation { #[default] BoardWorktree, CardWorktree }
impl RunLocation { pub fn is_board_worktree(&self) -> bool; }
impl BoardSettings { pub fn max_live_runs(&self) -> u32; }   // unwrap_or(1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy { #[default] Manual, RemoteWins, LocalWins }
// BoardSettings implements Default explicitly to match serde defaults:
// start_on_worktree = true, branch_template = "{key}-{slug}", Manual, push_new_cards = false,
// max_live_runs = None, run_location = BoardWorktree.

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
                          /// Same attributes as `Board.kind`; the app finds a context's Reviews board by it.
                          #[serde(default, skip_serializing_if = "BoardKind::is_tasks")] pub kind: BoardKind,
                          pub name: String, pub prefix: String,
                          pub backend_kind: String, pub card_count: usize, pub open_count: usize,
                          pub dirty_count: usize, pub conflict_count: usize,
                          /// Cards with a live or pending run, and cards whose last run wants a human.
                          #[serde(default, skip_serializing_if = "is_zero")] pub working_count: u32,
                          #[serde(default, skip_serializing_if = "is_zero")] pub attention_count: u32,
                          /// Idle cards in a Started column, apart from `attention_count` (§11.5).
                          #[serde(default, skip_serializing_if = "is_zero")] pub idle_started: u32,
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
pub const BOARD_DOCUMENT_VERSION: u32 = 3;
pub const BOARD_DOCUMENT_MIN_VERSION: u32 = 1;
/// 3 when the board is not a `Tasks` board, its `run_location` is not `BoardWorktree`, or any card
/// carries a `pull_request` or a run with a `worktree_id`. Otherwise 2 when any column carries
/// `automation`, the settings carry `max_live_runs`, or any card carries `blocked_by`, `agent`, a
/// `pending_run`, `runs`, or a comment with `run_id`; else 1.
pub fn document_version(board: &Board, cards: &[Card]) -> u32;
```

**The document version bumps lazily.** `BoardStore::save` stamps
`doc.version = document_version(&doc.board, &doc.cards)` before it validates, so a board nobody
automated goes on writing version 1 and a daemon built before this feature goes on reading it. A
board that has opted in writes 2, and keeps writing 2 for as long as any card still carries a link
or a run: an older daemon cannot represent either and would drop the history on its next save.
`load` and `peek` accept `BOARD_DOCUMENT_MIN_VERSION..=BOARD_DOCUMENT_VERSION` and refuse anything
else by name — `board {id} uses document version {v} (this build reads 1..=3)` — without
quarantining the file, because a document this build is too old to read is intact, not damaged.

**Version 3 is stamped just as lazily** (ADR 0024). Only a board that uses a review field writes 3:
a `Reviews` kind, a `CardWorktree` run location, a card with a pull request, or a run that records
its worktree. A board that never uses one keeps writing 2 or 1, byte for byte as before, and a
daemon built before review boards refuses a version-3 document by name rather than silently
dropping the fields it cannot represent. Because every new run records `worktree_id`, a workflow
board that runs a card after this feature writes 3 from then on.

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
pub fn reviews_board_id(context: &ContextId) -> BoardId; // "reviews-" + context id, capped at 64 bytes, trailing '-' trimmed
/// `new_board`, then: id `reviews_board_id`, name "Reviews", prefix "REV", kind `Reviews`,
/// `reviews_preset()`, labels `github` and `chat`, `run_location = CardWorktree`,
/// `max_live_runs = Some(2)`, `start_on_worktree = false`. §11.6.
pub fn new_reviews_board(context: &Context, now: &str) -> Board;

// §11 — the one workflow preset and its text.
pub const PRESET_INSTRUCTIONS_IMPLEMENT: &str = "Implement this card in the current worktree. Do not commit.";
pub const PRESET_EXPECT_IMPLEMENT: &str = "make lint and make test pass";
pub const PRESET_EXPECT_REVIEW: &str = "the review finds no blocking issue";
pub const PRESET_REVIEW_SKILL: &str = "deep-review";
pub fn workflow_preset() -> Vec<Status>;          // backlog, todo, ready, in-progress, in-review, done, canceled
// §11.6 — the reviews preset and its text (exact strings live in the code).
pub const PRESET_INSTRUCTIONS_REVIEW_PR: &str;     // review {pr_url}; report verdict, summary, numbered findings; post nothing
pub const PRESET_EXPECT_REVIEW_PR: &str;           // "a review report: a verdict line, a summary, and numbered findings with file:line"
pub const PRESET_INSTRUCTIONS_PUBLISH_REVIEW: &str; // post the newest succeeded report, with "Notes from you" applied, as one gh review
pub const PRESET_EXPECT_PUBLISH_REVIEW: &str;      // "the URL of the review now visible on the pull request"
pub fn reviews_preset() -> Vec<Status>;           // pending, reviewing, reviewed, published, dismissed
/// Adds the preset columns missing **by `StatusId`**, in preset order relative to the neighbours
/// already present; never touches an existing column. `true` when it changed anything.
pub fn apply_workflow_preset(board: &mut Board) -> bool;
/// `{key}` then `{title}`; anything else in braces is left alone.
pub fn render_template(text: &str, key: &str, title: &str) -> String;
/// `render_template`, then — only when `card.pull_request` is `Some` — `{pr_url}`, `{pr_repo}` and
/// `{pr_number}`. Without a pull request the three are left as written: a Tasks board may
/// legitimately print `{pr_url}` in its instructions. The brief and column `env` values use this.
pub fn render_card_template(text: &str, key: &str, card: &Card) -> String;
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub blocked_by: Vec<CardId>,
    /// The card's pull request. `CardPatch` has no counterpart: a card's pull request never changes.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub pull_request: Option<PullRequestRef> }

/// What `upsert_pull_request_card` did. On the wire as `CardUpsert.outcome` (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpsertOutcome { Created, Existing, Reopened }

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
// Document validation (run by the store on every save) also refuses two cards with one pull
// request: `invalid pull_request: {pr key} is already on this board as {KEY}`. A race between two
// upserts can therefore never persist a duplicate.
pub fn valid_date(date: &str) -> bool;  // real YYYY-MM-DD calendar day; every surface offering a due date uses this one
/// The `bool` is whether anything changed: a patch that leaves every field as it found it is not
/// a mutation, so it stamps nothing and the service does not save or announce it.
pub fn apply_board_patch(board: &mut Board, patch: BoardPatch, now: &str) -> Result<bool /* changed */, BoardError>;
/// Assigns number/id-less card; caller supplies the id. Status defaults to first Unstarted, else first status.
/// A draft or patch naming a property whose schema is `editable: false` is refused: the backend owns it.
pub fn create_card(board: &mut Board, cards: &[Card], id: CardId, draft: CardDraft, now: &str) -> Result<Card, BoardError>;
/// Idempotent creation of a pull-request card; the `usize` is the affected card's index in `cards`.
/// A draft without `pull_request` is refused (`a pull request card needs a pull request`). The card
/// is looked up by `pull_request.key()`, archived cards included:
/// - none → `create_card`, pushed, `Created`;
/// - found in a `Completed` column or archived, with `requested_at` later than its last completion
///   (the `at` of its newest `Moved`/`AutoMoved` activity, else `updated_at`; compared as times) →
///   unarchived, moved to `draft.status_id` or the first `Unstarted` column, an `Updated` activity
///   `Review re-requested` pushed, `Reopened`;
/// - anything else — including every `Canceled` card, since dismissing is a decision → `Existing`,
///   nothing changed.
/// An unparsable `requested_at` is `invalid requested_at: must be an RFC 3339 time`.
pub fn upsert_pull_request_card(board: &mut Board, cards: &mut Vec<Card>, id: CardId, draft: CardDraft,
                                requested_at: Option<&str>, now: &str) -> Result<(UpsertOutcome, usize), BoardError>;
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
/// `pending_run` is `Some` and names a column other than the card's own: the card waits in a
/// routing column for a run slot (§11.7 rule 0). `attention` ignores a queued card's wait.
pub fn queued(card: &Card) -> bool;
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
pub struct LocalBackend;  // kind "local"; caps all false; describe → the board's own statuses, label ids, properties and prefix; pull → empty full=false; push → empty
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
    /// Get-or-create the context's Reviews board (`defaults::new_reviews_board`), found by
    /// `documents::reviews_board`: the `reviews_board_id` fast path, then a scan for
    /// `context_id == context`, no `worktree_id`, `kind == Reviews`. `ensure` line for line — the
    /// same double-check under the gate and the same `-2`..`-99` id allocation. An unknown context
    /// is `NotFound`.
    pub async fn ensure_reviews(&self, context: &ContextId) -> DaemonResult<BoardView>;
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
    /// `ops::upsert_pull_request_card` under the board gate; see "Pull-request cards" below.
    pub async fn upsert_pull_request_card(&self, board: &BoardId, draft: CardDraft, requested_at: Option<String>) -> DaemonResult<(Card, UpsertOutcome)>;
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
reported the same way and skipped when `create_worktree_from_card` picks a repository. A card on
a Reviews board is held only to its repository existing, in any context, because its pull
request's repository may live in another one (§4.1). On a card-worktree board,
`create_worktree_from_card` on an unlinked pull request's card does not make a slug branch off
the base: it makes or adopts that pull request's worktree exactly as a run's start does (§4.1,
steps 3–6, under `pull_request_links`), then applies `start_on_worktree`; with a `host` it is
refused (`a pull request's worktree is made on this daemon, not on host {host}`). `ensure`/`create` refuse a context whose
board document is quarantined rather than creating an empty board over it. Cascades preserve both
a live document this build cannot read and any quarantined remains, because their persisted
context/worktree scope cannot be inferred safely from the shared board id. Explicit board deletion
can remove quarantined remains because the caller supplies the authoritative board identity.
`context_board` accepts only a `Tasks` board with the requested `context_id` and no `worktree_id`,
on both its derived-id fast path and its fallback scan, so it never returns the context's Reviews
board and the Hub's context board is unaffected by one existing;
`worktree_board` similarly treats the derived id only as a fast path and falls back to the
persisted `worktree_id`. Creating either scope appends `-2` through `-99` when another scope
already occupies its default id, and refuses creation if all candidates are occupied. `summaries`
reparses a board document only when its `stamp` changed or
this daemon rewrote it. Boards whose context no longer exists are skipped by `list`/`summaries`;
a worktree board is skipped as well once local state no longer names its
worktree. Deleting a context deletes its boards — its Reviews board included — in the same cascade
(`delete_for_context`) so a later context deriving the same id cannot adopt one. After any worktree deletion moves the worktree to trash, the late-bound
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

**The Reviews board.** A context has at most one task board (`ensure`) and at most one Reviews
board (`ensure_reviews`); both are context boards (no `worktree_id`) and they never stand in for
each other. The Reviews board spans repositories: its cards review pull requests in any Fleet
repository, whichever context holds it, and each runs in that pull request's own worktree (§4.1).

**Pull-request cards.** `upsert_pull_request_card` takes the gate, loads the document and runs the
pure upsert (§2). On a Reviews board a pull request's repository may belong to any context, so the
draft's `repo_id` is not held to the board's context: it is set to the pull request's repository
when that is a Fleet repository and left `None` otherwise — the card is still created and visible,
and its run refuses later with the sentence of §11.4. It then runs `validate_links` and commits
with the card as the seed for `Created` and `Reopened`; `Existing` changed nothing, so it saves
nothing, publishes nothing and answers the card as loaded. A pull request is matched without case
(`PullRequestRef::same_pull_request`: GitHub names are case-insensitive), and its repository is
linked under the id Fleet registered it with. A card whose run is still live answers `Existing`
before anything else: reopening it would move it out from under its run, which `move_card`
refuses. On a card-worktree board the starts the commit decides are handed off rather than awaited
(`apply_starts_after_answer`): creating the card's worktree fetches the pull request, and the
request answers once its save lands, holding the reservation until the start is recorded. Seeding the new card is what moves a
card created in *Pending review* on to *Reviewing* (§11.7 rule 0); nothing special-cases it. Two
schedule runs upserting one pull request at the same moment are serialised by the gate: the second
sees the first card and answers `Existing`, and document validation refuses a duplicate even so.

### 4.1 Automation

`Boards::new` takes one more argument, `Option<Automation>`, and the service answers the three run
verbs only when it has one:

```rust
// crates/fleet-daemon/src/services/boards/automation.rs
pub struct Automation { /* delegations: DelegationService, checkpoints: Arc<Checkpoints>,
                          in_flight: Mutex<BTreeMap<BoardId, BTreeSet<CardId>>>,  // per board:
                          //   the ceiling it is counted against is one board's
                          pending_boards: Mutex<BTreeSet<BoardId>> */ }
impl Boards {
    pub async fn start_run(&self, card: &CardId) -> DaemonResult<Card>;
    pub async fn cancel_run(&self, card: &CardId) -> DaemonResult<Card>;
    pub async fn wait_run(&self, card: &CardId, timeout_ms: u64) -> DaemonResult<Card>;
}
```

`composition.rs` passes `Some` exactly when `agents::delegation::install` returned a service. A
card run *is* a delegation, so a daemon whose agent database never opened has nothing to run one
with: it refuses the three verbs with `Unsupported("the native-agent database is unavailable, so
board automation is refused")` and serves every other board request exactly as before. The same
composition installs `Boards` on the delegation service as a `Weak<dyn RunDeliveryHook>`, and that
weak handle is the only edge back — the delegation service never names `Boards`, because it is the
lower of the two (`NATIVE-AGENTS.md` §15.7).

**What makes the engine walk.** These callers seed `fleet_core::board::re_evaluate` (§11.7) and
nothing else does. A card merely *standing* in an action column is never a seed.

| Trigger | Seeds |
| --- | --- |
| `create_card` | the new card |
| `update_card` | the card, when its `status_id` or `archived` changed |
| `move_card` | the moved card |
| `delete_card` | every card the deleted one had been blocking |
| `update` (board patch) | every card whose column changed category |
| `start_run` | the named card |
| `on_run_delivered` | the card the run ended on — as an *entry* when the outcome moved it, and as **settled** when it did not, which is the difference between a chain and a column that re-runs one card for ever (§11.7) |
| boot recovery and a freed slot | §11.7 |

`cancel_run` on a card that is only *owed* a run has no child to stop: it clears the `pending_run`
and writes `Run canceled: it was still waiting for a slot` (§11.3), which is what `X` and
`card cancel` promise a waiting card. `move_card --cancel-run` drops the gate while the cancel is
asked — the gates are not reentrant — and the cancelled run is usually still live when it comes
back, because a child's `Cancelled` arrives with its delivery; so the move compares run *ids* on
the way back in, and refuses when the card gained a different live run in that window rather than
leaving a live child reporting into a column the card has left.

**The plan is applied outside the gate.** Every one of those sites loads the document, evaluates
and saves under that one board's gate, notes the pending memo, then **drops the guard** and calls
`apply_starts`. The gate is not reentrant and `start_for_card` takes it again to write the run row,
so a start attempted under the caller's own guard would deadlock the board it is starting on. The
same rule covers the reads either side: `cancel_run` reaches the delegation service with no gate
held, and `on_run_delivered` takes the usage read and the Git diff before the gate.

**The refusals this layer adds.** Every sentence is printed verbatim by the CLI and the app;
`{KEY}` is the card's display key and `{column}` its column name.

| Raised by | Kind | Sentence |
| --- | --- | --- |
| `start_run` | `Validation` | `{column} has no action` |
| `start_run`, `update_card --archive`, `delete_card` | `Conflict` | `{KEY} is working; cancel the run first` |
| `move_card` without `cancel_run` | `Conflict` | `{KEY} is working; pass --cancel-run to move it` |
| `move_card --cancel-run`, when a *different* live run appeared while the cancel was asked | `Conflict` | `{KEY} is working; cancel the run first` |
| `cancel_run`, with neither a live run nor an owed one | `NotFound` | `{KEY} has no live run` |
| `update` removing a column a run names | `Conflict` | `column has {n} live runs; cancel them first` |
| `update`, when the patch asks for automation | `Invalid { field: "automation" }` | §11.4's three `automation` sentences |
| any of the three verbs, with no delegation service | `Unsupported` | `the native-agent database is unavailable, so board automation is refused` |

`Boards::require_automatable` is the one implementation of the three `automation` sentences, and
`update` runs it only when the patch *asks* for automation — a column gained or changed a block, or
`max_live_runs` was set or raised. Holding every patch to it would strand a board whose worktree a
host adopted afterwards: it could no longer be renamed and, worse, its automation could no longer be
taken off. The **start** path runs the same three rules again, from `prepare_run`, and raises the
same `Invalid { field: "automation" }` sentences: a worktree can be adopted by a host long after
its column was written, and the run that would touch it is the thing that has to refuse. There it
is recorded on the card rather than returned, as the paragraph below describes. On a board whose
`run_location` is `CardWorktree` the first rule (no worktree) does not apply and the third (the
host of the board worktree) applies only when the board has one; the host check moves to the start
path, card by card, because each card's worktree can differ (§11.4). Boot recovery
(`resume.rs::automated_boards`) and the delivery path include card-worktree boards: automation still
happens in a worktree and nowhere else, but in the card's rather than the board's.

**A card-worktree run gets its worktree first.** On a `CardWorktree` board, `start_for_card` calls
`worktree.rs::ensure_pull_request_worktree` right after the `automation()` check, holding the run
reservation throughout, so a slow fetch counts against `max_live_runs` — on purpose. It follows
`create_and_link_worktree`'s gate discipline, because creating a worktree runs `git fetch` and
hooks and can take minutes:

1. Under the gate, load the card. Done when the board runs in its own worktree. When the card
   already links a worktree that still exists in state, a worktree another host owns is refused
   (`automation is unavailable on a worktree owned by host {host}`); otherwise, with no pull
   request there is nothing to do, and with one a start in the board's **first running column** —
   the first column, in board order, whose automation has an `on_enter` action: *Reviewing* on a
   Reviews board — makes the worktree **follow the pull request's head**, outside every gate. A
   later column's start (*Review published*) leaves the checkout at the head the review read, so
   the report's file:line findings land on the commits they were written against.
   `Worktrees::follow_pull_request_head` fetches the head exactly as `create_from_pr` does
   (`refs/swarm/pulls/<n>/head`); a worktree already at it is left alone; one with no changes to
   tracked files is moved to the head (its branch reset to it, or a detached `HEAD` moved) when
   the head contains its `HEAD`, **or** when its `HEAD` is still the head Fleet last placed there
   — recorded in the worktree's own `refs/fleet/pulls/<n>/placed`, set at creation and after every
   move — so a force-pushed or rebased pull request is followed too. Anything else — changes to
   tracked files, or commits Fleet did not place that the head lacks — refuses the run with `The
   worktree for {owner/name}#{n} has local changes; commit or discard them before the review
   runs.` and leaves the worktree exactly as it was; a refused follow leaves the marker alone, so
   once the change is discarded the next start moves it. Untracked files are never in the way: a
   checkout keeps them, and fails with Git's own error rather than overwrite one. A failed fetch
   refuses the run with its own error.
2. With no pull request, refuse: `{KEY} has no worktree to run in; link a pull request or create
   its worktree first`.
3. Look the pull request's repository up in `state.repos`, in any context; missing, refuse:
   `{owner/name} is not a Fleet repository; clone it into this context first`.
4. **Drop the gate**, and `Worktrees::create_from_pr(repo, number)` — which adopts an existing
   worktree for that pull request. A worktree with `host: Some(host)` is refused with
   `automation is unavailable on a worktree owned by host {host}`. An adopted worktree (one the
   pre-Reviews-board PR list or `fleet create` made) holds whatever it was left at, so in the
   first running column it follows the head exactly as step 1 describes, refusals included —
   after a card on another board that already links it is refused, so its checkout is never moved
   on this card's behalf.
5. Take `Boards.pull_request_links` — the one lock every pull-request link is written under, so
   two boards cannot both claim a worktree — and refuse with `worktree_owned_by_another_card`
   (`worktree {id} already belongs to another card`) when a card on **any other board** links the
   worktree: the same pull request on two contexts' Reviews boards is reviewed on the first one to
   start, and the second records the refusal instead of running (and later publishing) a second
   review in the same checkout. Then take the gate again and reload the card. Deleted or archived
   meanwhile is a `Conflict` naming the worktree, in `create_and_link_worktree`'s words; a
   worktree another card of this board links is `worktree_owned_by_another_card`.
6. Otherwise set `card.worktree_id` and `card.repo_id`, push the `WorktreeCreated` activity
   `create_and_link_worktree` pushes, and save with `BoardChangeReason::CardChanged`.

Any refusal is recorded as a failed `CardRun` through `prepare_run` and `record_run`, exactly as
the existing start refusals are.

**A start that ends without a run hands its slot on.** A start that records a failed run, or that
is abandoned because the card left its column, was deleted or archived first, holds no slot once
its reservation goes — and no delegation will ever end to say so. `start_for_card` therefore
hands the slot on itself, on a tracked task of its own (`hand_on_slot`): `release_slot` gives it
to the card that has waited longest (§11.7), and an abandoned start's card is then evaluated again
as an entry into the column it stands in, because the entry that put it there was refused while
this start still held it (rule 0 skips a reserved card) — a card moved back to *Pending review*
mid-fetch advances or queues rather than standing there with no marker. An upsert treats a card
whose start is in flight as working, exactly as `start_run` does, and answers `Existing`.

**Card-worktree starts never hold a request or a loop.** On a `CardWorktree` board every trigger —
a card write (`commit`), a board patch (`update`), a delivery (`on_run_delivered`), a freed slot
(`release_slot`), boot recovery and `start_run` — hands its starts to one tracked background task
(`apply_starts_after_answer`) instead of awaiting them, because a start may fetch a pull request
first. The reservation keeps the slot counted meanwhile. `start_run` alone waits for that task,
for at most 20 seconds (`START_ANSWER_WAIT`, inside the client's request timeout), so a start or
refusal that lands quickly is in its answer; a slower one is answered as the card stands and keeps
going. On a `BoardWorktree` board the starts are awaited as before. `card_request` then picks the run's worktree by `run_location` —
`board.worktree_id` or `card.worktree_id` — through the same small function `prepare_run` uses to
fill `RunRow.worktree`, which `started()` and `failed()` copy into `CardRun.worktree_id`. Every
later run of the card reuses the linked worktree, and two cards of one Reviews board run at once in
two worktrees.

Several sentences are *written* rather than raised, because a card is where their reader is
looking. A column that loses its action clears the cards parked for it with the activity entry
`Run canceled: {column} no longer runs an action`; deleting a blocker writes
`Unblocked: {KEY} was deleted` on every card it freed (§11.3); and a start `start_for_card` cannot
make — a column whose `env` breaks one of the five `--env` rules, a board with no worktree, a
card-worktree card with neither a worktree nor a pull request, a pull request in a repository Fleet
does not hold, a worktree this daemon cannot reach, a worktree with local changes that following
its pull request's head would touch, a worktree another board's card already links — becomes a `CardRun` with `thread_id: None` carrying the refusal
in its `detail`, so the person who wrote the column reads it on the card rather than losing it to a
log line nobody asked for.

A pull never evaluates. `sync.rs` says why in a comment: a backend decides where a card stands on
the *remote's* terms, and letting a remote transition start a local run would make a Jira automation
rule a trigger for this daemon's agents.

## 5. Protocol (`fleet-proto`, version 8)

Worktree-board requests are an additive protocol-8 extension advertised through the
`board.worktree` capability. The capability lets clients avoid sending variants an older daemon
cannot decode without forcing every local and remote daemon to upgrade in lockstep.

```rust
pub const BOARD_WORKTREE_CAPABILITY: &str = "board.worktree";
```

Column automation is a second such extension, advertised through `board.automation`:

```rust
pub const BOARD_AUTOMATION_CAPABILITY: &str = "board.automation";
```

It gates three additive requests and one additive field. `MoveCard` gained `cancel_run: bool`,
carrying `#[serde(default, skip_serializing_if = "std::ops::Not::not")]` like every other new
`bool`, so an ordinary move is byte-identical to what it always was and an older client's move
decodes here unchanged, still meaning "refuse if the card is working". The three requests are new
variants and do need the capability, and a peer that names
`board.automation` is also the only peer shown a card-called `DelegationChanged` or a card-called
row in a `Delegations` listing (`NATIVE-AGENTS.md` §15.7).

| Request | Answers |
| --- | --- |
| `CardRunStart { card_id }` | the card after the start was decided, whatever its last run ended as |
| `CardRunCancel { card_id }` | the card after the cancel was asked for; the outcome arrives with the delivery |
| `CardRunWait { card_id, timeout_ms }` | the card once its newest run ends, or as it stands when the wait times out |

All three are `Target::Local` unless the card's board belongs to a host, in which case they route
to the owner exactly as `MoveCard` does (ADR 0021): the board document and the delegation behind
the run are both the owner's. Their transport deadlines differ and each says why in
`fleet-client`'s `request_timeout`: `CardRunStart` pays `AGENT_HARNESS_TIMEOUT`, because it spawns
a child through the same harness probe `DelegationRun` pays for; `CardRunWait` is the caller's
`timeout_ms` plus fifteen seconds of slack for the daemon to answer the card once that deadline
expires; `CardRunCancel` has no deadline at all, for the reason `DelegationCancel` has none — the
cancel walks a tree bounded by the live-run ceiling, not by anything this side can predict.

Review boards are a third, advertised through `board.reviews` (ADR 0024). Neither of its two
requests bumps `PROTOCOL_VERSION`; the daemon advertises the capability only once the whole review
flow is served, and the client refuses both requests to a daemon that does not name it. The client
names `board.reviews` in its own hello too, and only such a peer is shown a Reviews board in a
`Snapshot`, a `SnapshotChanged` or a `ListBoards` answer: a client from before Reviews boards takes
the first unscoped summary of a context to be its board, and `reviews-<context>` sorts before most
context ids, so a new daemon would otherwise change what it shows.

```rust
pub const BOARD_REVIEWS_CAPABILITY: &str = "board.reviews";
pub enum UpsertOutcome { Created, Existing, Reopened }   // fleet_core::board, snake_case on the wire
```

| Request | Answers |
| --- | --- |
| `EnsureReviewsBoard { context_id }` | the context's Reviews board, created with the reviews preset on first ask (§4) |
| `UpsertPullRequestCard { board_id, draft, requested_at }` | `CardUpsert { card, outcome }`: the created, existing or reopened card; `draft.pull_request` must be `Some` |

`requested_at` carries `#[serde(default, skip_serializing_if = "Option::is_none")]`, so an upsert
without a request time serialises without the field.

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
MoveCard { card_id, status_id, index: Option<usize>, #[serde(default)] cancel_run: bool } → Card
CardRunStart { card_id }                                            → Card    // board.automation
CardRunCancel { card_id }                                           → Card    // board.automation
CardRunWait { card_id, timeout_ms: u64 }                            → Card    // board.automation
DeleteCard { card_id }                                              → Ack
AddCardComment { card_id, body: String }                            → Card
CreateWorktreeFromCard { card_id, repo_id: Option<RepoId>, base: Option<String>, host: Option<HostId> } → CardWorktree { card: Card, worktree: Worktree, created: bool }
SyncBoard { board_id, #[serde(default)] full: bool }                 → ResponseBody::Job(JobRecord); Client::sync_board returns its JobId
ListBoardBackends                                                   → BoardBackends(Vec<BackendDescriptor>)
ResolveCardConflict { card_id, resolution: ConflictResolution }     → Card
DescribeBoardBackend { board_id }                                   → BoardBackendSchema(BackendSchema)
EnsureReviewsBoard { context_id: ContextId }                        → Board(BoardView)   // board.reviews
UpsertPullRequestCard { board_id, draft: CardDraft, #[serde(default, skip_serializing_if = "Option::is_none")] requested_at: Option<String> }
                                                                    → CardUpsert { card: Card, outcome: UpsertOutcome }   // board.reviews

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
and these three automation additions, beside a `move_card` that grew a fourth argument:

```rust
pub async fn card_run_start(&self, card_id: CardId) -> Result<Card>;
pub async fn card_run_cancel(&self, card_id: CardId) -> Result<Card>;
pub async fn card_run_wait(&self, card_id: CardId, timeout_ms: u64) -> Result<Card>;
pub async fn move_card(&self, card_id: CardId, status_id: StatusId, index: Option<usize>, cancel_run: bool) -> Result<Card>;
```

and these two review additions, both gated on `board.reviews` in `required_capability`, so an
older daemon gets the same "run `fleet daemon restart`" guidance before anything is sent:

```rust
pub async fn ensure_reviews_board(&self, context_id: ContextId) -> Result<BoardView>;
pub async fn upsert_pull_request_card(&self, board_id: BoardId, draft: CardDraft, requested_at: Option<String>) -> Result<(Card, UpsertOutcome)>;
```

The three run methods check `board.automation` the same way and in the same two places, and answer
their own sentence — "this daemon does not support board automation; run `fleet daemon restart`" —
rather than the worktree-board one, so a user reading it is told which feature is missing.
`move_card` needs no capability, because a field is not a variant: `cancel_run` is skipped when it
is false, so the common move is byte-identical to what it always was, and a daemon old enough to
ignore the field is also old enough to have no run to cancel.

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
consumed as the optional value; `--reviews` selects the Reviews board of `--context <id>` or of the
active context through `EnsureReviewsBoard`, creating it on first use, and conflicts with `--board`
and `--worktree`):

```rust
pub enum BoardWorktreeSelector { Explicit(WorktreeId), FromSession }
```

```
fleet board show [--context C|--worktree[=W]|--board B|--reviews]  # columns + cards; a Reviews board's header reads `· reviews of <context>`
fleet board list                                                  # summaries; SCOPE reads `context`, `reviews` (a context's Reviews board) or the worktree id
fleet board create [--context C|--worktree[=W]] [--name N] [--prefix P] [--backend local|jira] [--setting k=v]...
fleet board set [--name] [--prefix] [--default-repo owner/name] [--clear-default-repo] [--start-on-worktree [true|false]] [--conflict-policy manual|remote_wins|local_wins] [--push-new-cards [true|false]] [--branch-template "{key}-{slug}"] [--add-label NAME]... [--remove-label L]... [--backend KIND] [--setting k=v]...
fleet board backends                                              # registered kinds, capabilities, setting keys
fleet board describe [--context C|--worktree[=W]|--board B]       # what this board's backend reports about itself; a local board reports its own columns, labels, properties and prefix
fleet board sync [--wait] [--full]                                # --full ignores the incremental cursor
fleet board set [... above ...] [--max-live-runs N]               # the board's throttle; 1 when unset
fleet board columns                                               # the columns, their category and the automation they carry
fleet board columns add <name> [--id ID] [--category backlog|unstarted|started|completed|canceled] [--after C|--before C]
fleet board columns edit <id|name> [--name N] [--category K] [--color C] [--on-enter none|prompt|skill:<name>[:<args>]] [--provider claude|codex] [--model M] [--effort E] [--mode ask|accept-edits|plan|auto|dont-ask|full-access] [--instructions T|--instructions-file F] [--expect T] [--on-success C|--no-on-success] [--when-unblocked C|--no-when-unblocked] [--env KEY=VALUE]... [--clear-env]
fleet board columns move <id|name> --after C|--before C
fleet board columns remove <id|name> [--move-cards-to C]
fleet board columns preset workflow                               # adds the columns the preset names; never rewrites one that exists
fleet board card new <title> [--desc|--desc-file F] [--status S] [--priority urgent|high|medium|low|none] [--label L]... [--assignee] [--estimate] [--due YYYY-MM-DD] [--repo] [--provider claude|codex] [--model M] [--effort E] [--blocked-by KEY]... [--blocks KEY]...
fleet board card new <title> --pr <url|owner/name#n> [--requested-at <rfc3339>] [same flags as new]
                                                                  # UpsertPullRequestCard; first line `Created KEY`, `Existing KEY` or `Reopened KEY`
fleet board card show <key|id>                                    # a pull-request card adds `Pull request: owner/name#n  <url>`
fleet board card edit <key|id> [same flags as new] [--clear-labels|--clear-assignee|--clear-estimate|--clear-due|--clear-repo|--clear-agent] [--archive [true|false]] [--add-blocked-by KEY]... [--remove-blocked-by KEY]... [--clear-blocked-by] [--add-blocks KEY]... [--remove-blocks KEY]...
fleet board card move <key|id> <status> [--index N] [--cancel-run]
fleet board card run <key|id>                                     # start a run for a card in an action column; a refused start prints `run <id> failed: <sentence>` and exits 1
fleet board card cancel <key|id>                                  # cancel the live run, or drop the slot an owed one waits for
fleet board card runs <key|id>                                    # one tab-separated line per run, newest last; the last field is a refused start's sentence
fleet board card attach <key|id>                                  # the thread id the run is talking in
fleet board card wait <key|id> [--timeout 540]                    # 0 when the newest run is terminal, 2 otherwise
fleet board card comment <key|id> <body>
fleet board card delete <key|id>
fleet board card worktree <key|id> [--repo owner/name] [--base REF] [--host H]   # prints the created worktree like `fleet create`
fleet board card resolve <key|id> keep-local|take-remote
```
`card new --pr` parses its reference with `PullRequestRef::parse` and prints that refusal verbatim;
`--requested-at` without `--pr` is refused with `--requested-at needs --pr`, and a `--requested-at`
that is not RFC 3339 with `invalid requested_at: must be an RFC 3339 time` — all three before the
board is resolved, so `--reviews` never creates a Reviews board for a command it then refuses.
Without `--pr` `card new` is exactly what it was. With `--pr` the first line of output is `Created {KEY}`,
`Existing {KEY}` or `Reopened {KEY}`, followed by the card as `card new` prints it; with `--json`
the command prints a `BoardCardUpsertEnvelope { protocol: 1, card, outcome }`. A pull request
already on the board is never duplicated, so a scheduled agent runs it for every request it finds
(§12):

```
$ fleet board --reviews card new "Fix the login race" --pr https://github.com/acme/api/pull/412 --label github
Created REV-7
…
```

`board list` accepts `--board` to narrow the table and `--context` to restrict the daemon query;
it rejects `--worktree` because listing does not ensure or resolve a board.
`<key|id>` accepts a display key (`FLT-12`, `PROJ-123`) or a CardId, and the local key of a card
with no remote link — a mirrored card's local key is not a selector, because a board mirroring the
Jira project its own prefix names would have two namespaces of the same shape overlapping. `board
card show` prints `Local key:` for exactly the cards that answer to one.

Every `columns` verb is a read-modify-write of the whole column vector and sends one
`UpdateBoard`, so a concurrent editor loses — exactly as `board set` already behaves. `columns
remove` without `--move-cards-to` is refused by the daemon while any card still stands in the
column; with it, the cards move first, archived ones included, and the removal follows in the same
command. With `--json`, every `columns` verb prints the `BoardEnvelope`, whose `board.statuses`
*are* the columns; there is no envelope of its own. `BoardEnvelope` gained one field, `liveRuns`,
omitted rather than `[]` when nothing is live, so the envelope a reader parsed before automation
existed is byte-identical.

`card wait` exits **0** when the card's newest run is terminal and **2** when it is still live or
no run started before the timeout — the pair an orchestrator scripts against, and the same pair
`fleet subagent wait` answers with. A card the board still *owes* a run exits **2** as well, and
at once: its newest row is an earlier attempt, and reading the exit code off that would answer 0
about a run nothing has started. There is nothing live to wait on, so the wait returns rather than
holding the timeout — `card wait` is a wait for *this* run, never a barrier for a chain. Every other refusal exits 1. `card run`, `card cancel` and
`card wait` need the daemon to advertise `board.automation`; an older one is refused by the client
with "this daemon does not support board automation; run `fleet daemon restart`". `card runs` and
`card attach` answer from the board the command already read and send no second request.

A run may not move its own card. With `FLEET_DELEGATION` set, `card move` compares the `<key|id>`
it was given against `FLEET_CARD` case-insensitively and refuses with "a run cannot move its own
card; its report moves the card when it finishes" before building any request. It is advisory —
the child could move the card by its id under another spelling — and it is there because the
column's `on_success` is what routes a card, so a child that moved itself would race the route it
is about to be given.

`scripts/board-workflow-smoke.sh` (`make smoke-workflow`) is the end-to-end proof of all of it: a
private `FLEET_HOME`, a local git origin, scripted Codex and Claude binaries on a private `PATH`,
the workflow preset, a four-card diamond, and one assertion — every card reaches Done and `card
wait` exits 0. `scripts/reviews-smoke.sh` (`make smoke-reviews`, part of `make ci`) is the same
proof for review boards: a local bare-repo origin with a `refs/pull/1/head`, a scripted agent that
completes with a fixed report, `fleet board --reviews card new "Fixture PR" --pr <owner>/<name>#1`,
and the assertions that the card reaches *Reviewed* with a succeeded run and a linked worktree and
that a second `card new --pr` prints `Existing`. It then proves the schedule path of §12: a fake
`claude` on `PATH` creates a card through the footer's command, and `fleet schedule run --wait`
records a `Succeeded` run whose summary is the fake's `SUMMARY:` line.

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
    pub fn reference(self, Option<SharedString>) -> Self;         // short muted mono line (a review card's `owner/name#123`); the kit knows no pull request
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
  The title bar's context switcher scopes it: the board shown is `EnsureBoard(active_context)`.
  The Pull requests screen's **Review** tab is a third surface: it shows the active context's
  Reviews board, `EnsureReviewsBoard(active_context)`, in the list-and-detail area, with the repos
  rail hidden as on the Board tab, because a Reviews board spans repositories (`UX-SPEC.md` §3.5).
  Leaving the tab gives the mirror back to the Hub's previous scope.
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
  `BoardScope` is `Context(ContextId) | Worktree(WorktreeId) | Reviews(ContextId)` — the last is
  *the context's Reviews board — `EnsureReviewsBoard(context)`* — and one mirror holds whichever
  is on screen, since the Hub's Board tab, the PR screen's Review tab and the Workspace are never
  visible together. `None` means the Hub's default, the active context. A view is applied only when the
  scope admits it — a worktree's board carries that worktree, a context's board carries that
  context, **no** worktree and kind `tasks`, and a Reviews board carries that context, no worktree
  and kind `reviews` (so the two scopes never take each other's board) — while `apply_card` and `board_stale` keep keying on the board id
  that is on screen.
- **Bridge**: board requests use `Bridge::request` reply receivers, with no new `BridgeEvent`
  variants. Responses land in `AppState` reducers (`apply_board_view`, `apply_card`); board loads
  use scope/generation guards, and the scope picks the request:
  `EnsureBoard { context_id }`, `EnsureWorktreeBoard { worktree_id }` or
  `EnsureReviewsBoard { context_id }`, which is also what a `BoardChanged` for the shown Reviews
  board re-sends. The app never derives a
  board id of its own from a context or a worktree (§0). A scope change strands the previous
  scope's replies through the same generation counter a context switch uses.
  `screens::board::{enter_context_scope, enter_worktree_scope}` point the mirror and load;
  entering a worktree scope is refused on a daemon that does not advertise `board.worktree`,
  with the CLI's own sentence as a toast — "this daemon does not support worktree boards; run
  `fleet daemon restart`" — and no change to the scope or the shown board.
  `enter_reviews_scope` is `enter_context_scope` with the reviews request, and a daemon without
  `board.reviews` gets `REVIEW_BOARDS_UNSUPPORTED` — "this daemon does not support review boards;
  run `fleet daemon restart`" — shown the same way, with nothing sent; the PR screen then keeps
  the old flat review list. `refresh_card_marks` folds a Reviews board's runs exactly as it folds
  a worktree board's.
  A schedules mirror, `Vec<Schedule>` per board, sits next to the board state: loaded with
  `ListSchedules { board_id }` when Board settings opens on a board or a board is first shown,
  marked stale by `SchedulesChanged { board_id }` and re-read once per applied event batch (and
  once more when that re-read answers while still stale), cleared on every connection change, and
  read by Board settings' Schedules section and the header strip (§11.8, §12). A failed load is
  asked again when Board settings opens on the board or the board is shown again, never from the
  observation that runs on every notify: a lasting failure would otherwise be a request loop. A board runs its cards — the run keys and their palette rows apply — when it is a
  worktree board, a Reviews board, or a context board whose `runLocation` is `card_worktree`.
- **Dialogs** (`Dialogs` variants; serializable drafts and live `Entity<TextInput>` owners in
  `DialogHost`): `CardDetail` (`dialogs/card_detail.rs`,
  `CardDetailState`), `CardCreate` (`dialogs/card_create.rs`), `CardPicker`
  (`dialogs/card_picker.rs`, `PickerKind { Status, Priority, Assignee, Labels, Estimate, DueDate,
  Repo, Property(key) }`), `BoardSettings` (`dialogs/board_settings.rs`: name, prefix, default
  repo, start-on-worktree, push-new-cards, conflict policy, and the backend — a kind cycler over
  `ListBoardBackends` plus one generic row per `settings_schema` entry; see `docs/BOARD-JIRA.md`
  §6; then Columns, §11.10, and Schedules, §12).
- **Card detail layout** (UX-SPEC § Card detail): a right-side `Sheet` `sheet_w_detail` wide in
  the band between the two bars, over the board under its scrim. Header: key, status button
  (`s`), backend line, `Open in <backend>` (`x`), a `⋯` (`w`, `K` / `R` while conflicted, Delete)
  and the ✕. Left: title (a click or `i` edits it), conflict callout with `Keep local` / `Take
  remote`, the run card and its buttons, description (`MarkdownText`; `d` opens the shared
  multi-line `TextInput`; `ctrl-s` / `ctrl-enter` / `esc` save or cancel), comments with avatars
  and the inline composer (`c`). Right: property list — Status, Priority, Assignee, Labels,
  Estimate, Due, Parent, Repo, Worktree (opens its session), Pull request (opens it, §11.9),
  Remote (opens the issue), then custom
  properties from `board.properties`; `j/k` select a row, and `enter`, a click on the row or the
  board's own field key (`s p a t e b m o x`) opens what edits it — one path,
  `card_detail::open_row`. Activity folds under the properties.
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
| `T` | Hub > Board | Board settings, on its Schedules section (§12) |
| `R` | Hub > Board | run every enabled schedule of the board now (§12) |
| every `Hub > Board` key but `p` | Hub > Prs > Board | the same action, on the Review tab's Reviews board |
| `tab` / `shift-tab`, `p` / `q` | Hub > Prs > Board | Mine ⇄ Review tabs; back to worktrees |
| `A` / `X` / `>` | Hub > Prs > Board | attach, cancel, run now — a review card runs in its own worktree |
| `B` / `y` | Hub > Prs > Board | open the card's pull request in the browser / copy its URL |
| `esc` | Dialog > CardDetail | close (saves nothing pending) |
| `i` | Dialog > CardDetail | edit title |
| `d` | Dialog > CardDetail | edit description |
| `c` | Dialog > CardDetail | add comment |
| `j` / `k` | Dialog > CardDetail | select property row |
| `enter` | Dialog > CardDetail | edit selected property |
| `s` / `p` / `a` / `t` / `e` | Dialog > CardDetail | the field's picker for the card on show |
| `o` | Dialog > CardDetail | open the card's linked worktree session |
| `w` | Dialog > CardDetail | create worktree from card |
| `x` | Dialog > CardDetail | open the card's remote issue in the browser |
| `B` / `y` | Dialog > CardDetail | open the card's pull request in the browser / copy its URL |
| `K` / `R` | Dialog > CardDetail | resolve conflict keep-local / take-remote |
| `ctrl-s` / `ctrl-enter` | Dialog > CardDetail | save current text edit |

Palette commands mirror every row above, under their action-catalogue labels (`New card`,
`Sync with the tracker`, …; `KEYMAP.md` § *Action catalogue*).

- **Board surface** (`views/board_screen.rs`, `UX-SPEC.md` § Board): a page header (name, prefix,
  the counts line; the sync button and its ⋯, the filter field, Board settings and its ⋯, the
  primary New card), error callouts with the buttons that answer them, then the columns. Every
  control dispatches the action its key runs, so the pointer adds no behaviour of its own; the
  two that are not keys are a column's `+` / `Add card` — New card with `CardDraft.status_id` set
  to that column, carried to the dialog's seed through `DialogHost.card_create_in` — and a
  column's automation pill, which opens Board settings drilled into it through
  `DialogHost.board_settings_column`. A card's `⋯` and right-click hold the card actions,
  leaving out what could only refuse (`CardMenu`, `ReadonlyFields` in the board model). The model
  — tile strings, run and blocked words, the linked branch and its PR badge, the menu facts — is
  built by `screens/board/projection.rs`, keyed on the board revision, the marks revision, the
  filter, the linked branches and, only while a run is live, the minute.
- **Drag and drop** (`views/board_screen/drag.rs`, `screens/board/actions.rs`): a tile carries a
  `CardDrag` (card id, key, the column and row it was drawn at) through gpui's `on_drag`; every
  column listens with `on_drag_move` and `on_drop`. While a drag is over a column the column wears
  an accent hairline and a `DropSlot` sits at the insertion point, read off the column list's own
  layout (before the first tile whose middle is below the pointer). The slot says what the drop
  does: `Drop to start FLT-3 · codex will pick it up` into a column with an `on_enter` action,
  `Drop to move FLT-3 to Todo` into another, `Drop to put FLT-3 here` inside its own; no slot is
  drawn where the drop would move nothing — before or after the card itself, or anywhere on a
  board whose backend owns `status_id`. The dragged tile stays as a faded, dashed place-holder
  and the preview under the pointer is the same tile, lifted. The drop selects the card, exactly
  as a press on its tile does, and then takes **the path `[` / `]` take** (`move_card` with a
  `Destination`): the unreachable-daemon refusal, the read-only toast, the confirm over a live run
  (`MoveCancelsRun`, which stages the index beside the column) and the daemon's own refusals are
  the same sentences. The only difference is the request's `index`: the drawn slot is mapped onto
  `ops::move_card`'s index over the *whole* column without the card, so under a filter the card
  lands before the drawn card it was dropped above, wherever hidden cards sit. A drop back where
  the card stands sends nothing. The answer carries only the moved card, so the app replays
  `ops::move_card` over the shown cards with the same index (`AppState::apply_placed_card`) and the
  column draws the daemon's order before the `BoardChanged` reload arrives. Reordering changes only
  `position`, which is local ordering, so it dirties nothing and needs no backend support; a board whose
  backend owns the status refuses every drop as it refuses `[` / `]`.

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
Parent is read-only in this milestone. Card detail is a 736 px right-side sheet with two columns. `ctrl-enter` in
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

## 11. Automation — the model and the engine

**What this section covers.** §11.1 to §11.6 are *the model*: the shapes a board document may hold,
the rules that refuse a bad one, and the reads every surface derives from them. §11.7 is *the
engine*: the walk that acts on them, what it reserves, what it throttles, how a run's outcome is
recorded, and what a restart does with what it finds. The daemon's own half — where the engine is
called from, what it refuses, and the gate discipline around it — is §4.1; the three requests are
§5 and their client methods §6, whose CLI fence carries the `fleet board` verbs that drive them.
§11.8 to §11.10 are *the app*: the tile marks and the pane header, the card-detail run row and its
property rows, and the Board settings Columns pane.
`board.automation` (`fleet_proto::response::BOARD_AUTOMATION_CAPABILITY`) is advertised from the
build that serves the three requests, and by nothing before it.
[ADR 0022](decisions/0022-board-workflows.md) records why.

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
`render_card_template` substitutes `{key}` and `{title}` in `instructions` and in each `env` value,
and — on a card with a pull request — `{pr_url}`, `{pr_repo}` and `{pr_number}`; on a card without
one those three are left as written. `expect` is printed as written.

**The brief** a run's child receives is, in order: the skill invocation (a skill action only), the
rendered instructions, `# {KEY} — {title}` and the card's description, then two sections only a
review card is likely to carry — `## Pull request`, one line `{pr_repo}#{pr_number} · {pr_url}`,
when the card has one; and `## Notes from you`, every comment with no `run_id` made after the
newest **succeeded** run's `started_at` (every such comment when no run has succeeded), oldest
first, each as `- {author}: {body}` with `you` for a missing author, omitted when there is none —
and last `## Previous run reports`, newest first, each headed `### Report from {created_at} ·
{outcome word}` (the outcome of the run that wrote it; no suffix when that run is no longer on the
card). The notes are how a person corrects a review before it is published: a comment on a
*Reviewed* card reaches the *Review published* run — and still reaches its retry after a publish
that failed, because a failed run did nothing with them. The cut is the run's start because its
brief was written then: a note made while the review was running reaches the publish run. The outcome label is what lets the
publish preset name "the newest succeeded report" when a failed publish left a report of its own.
`agent` (`ColumnAgentPrefs`) is what the column asks for; a card's own `agent` (`CardAgentPrefs`)
wins over it. Permission `mode` exists only on the column: it is the column's policy over every
card that passes through it, and a card able to widen it would be a card able to grant itself
access its column deliberately withheld.

A block whose every field is empty is not automation. `ops::normalise_automation` turns one into
`None`, so a column a user has just cleared stops answering `automation.is_some()` and stops
holding the document at version 2. `apply_board_patch` and `apply_workflow_preset` both call it.

`settings.max_live_runs` is the board's throttle, `None` meaning one. It is a property of the
board rather than of a column because every run of one board edits the same checkout — or, on a
board whose `run_location` is `CardWorktree`, because every run of the board spends from one
budget of agents and tokens.

`settings.run_location` says where a board's runs execute. `BoardWorktree` — every board before
review boards — runs every card in `board.worktree_id`. `CardWorktree` runs each card in its own
`card.worktree_id`, created from the card's pull request on its first run (§4.1); it is what lets a
*context* board, which has no worktree of its own, automate at all. The Reviews board ships with it
(§11.6).

### 11.2 What a card remembers

`agent`, `blocked_by`, `pending_run` and `runs` (§2). `runs` is oldest first, capped at
`MAX_RUNS_PER_CARD` (20) with the oldest dropped. A `CardRun` is written twice — once when the
delegation exists, once when it ends — so a run is visible while it works and legible long after.

Live progress is never stored on the card. It belongs to the delegation, and a card is not a
mirror of one: what is live arrives joined onto `BoardView.live_runs` on read and is never
persisted. A run whose start failed has `thread_id: None` (`failed_to_start()`), which is the one
case where a `CardRun` exists with no thread to attach to. `CardRun.worktree_id` records the
worktree the run executed in — the board's or the card's — for every run recorded since review
boards, and is `None` on older runs.

Report excerpts live in the card's comments, with `Comment.run_id` set. A body is capped at
`REPORT_EXCERPT_CAP_BYTES` (8 KiB) and a card keeps at most `MAX_REPORT_COMMENTS_PER_CARD` (3);
the oldest is dropped past that, clearing its run's `report_comment_id`.

A report comment ends with what the run left in the worktree, under the heading `## Files changed
since this run started`, one line per file as `{M|A|D} {path}`. The diff is taken **when the run
is delivered**, not while a brief is assembled, and it is the thread's *first* checkpoint tree
against a snapshot of **the run's own worktree** as it is then — `CardRun.worktree_id`, falling
back to `board.worktree_id` for a run recorded before that field existed (`NATIVE-AGENTS.md` §5) — so it is the tree the
run left behind, and because it is tree-to-tree rather than per-edit attribution it also contains
whatever a person changed in that checkout while the run worked. A run that changed nothing, a
checkout that is not a Git working tree and a thread whose provider never took a checkpoint all
produce **no** heading at all; the same empty list is what a failed diff answers, and a heading
over nothing would read as a claim that nothing changed. The section is appended **after** the
`REPORT_EXCERPT_CAP_BYTES` cut, so an over-long report loses its own prose and never the file
list. When the board runs in its own worktree and `settings.max_live_runs` is above one, the list
is followed by `Other runs share this worktree; some of these changes may be theirs.` — with one
checkout per board, the sentence is the whole of what v1 does about a shared tree. A card-worktree
board never prints it: its runs never share a checkout. A review run is told to change nothing, so
on a Reviews board the section appears only when a run broke that instruction — which is itself
worth reading. The list travels to the next run of the card inside
`## Previous run reports`, which is the point of putting it in the comment; `CardRun.files_changed`
keeps only the count, and the paths themselves also reach the delegation's own
`DelegationResult.files_changed`.

### 11.3 The activity sentences

`ActivityKind` gains `RunStarted`, `RunEnded` and `AutoMoved`. The exact text, so every writer
copies rather than invents it:

| Kind | Message |
| --- | --- |
| `RunStarted` | `Run started · {provider} · {model} · {effort}` — a missing model or effort drops with its separator; provider is `AgentKind::executable()`. |
| `RunEnded` | `Run ended · {outcome word} · {Nm SSs}`, plus ` · ${cost:.2}` when the cost is known. |
| `AutoMoved` | `Moved to {column name}: unblocked by {KEY} reaching {column name}`. |
| `AutoMoved` | `Moved to {column name}: nothing blocks it`, when a card entering a routing column advances at once (§11.7 rule 0). |
| `AutoMoved` | `Moved to {column name}: a run slot freed`, when a card queued in a routing column is moved on (§11.7). |
| `Updated` | `Review re-requested`, when `upsert_pull_request_card` reopens a completed or archived card. |
| `Updated` | `Run canceled: {column name} no longer runs an action`, when a column loses its action. |
| `Updated` | `Run canceled: it was still waiting for a slot`, when a cancel drops a run a card was only owed. |
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
| `on_success` / `advance_when_unblocked` | `{column} routes to {target}, which is not a column on this board` · `{column} may not route to itself` · `{column} routes to {target}, which is not a later column` — each names the column carrying the route, because the edit that raises it is usually to another column |
| `on_enter` | `a skill action needs a name` · `skill actions run on claude only; put the invocation in the column's instructions for codex` |
| `env` | the five `fleet subagent run --env` sentences with `--env ` dropped: not `KEY=VALUE`, an empty key, a `FLEET_`-prefixed key, `PATH`, the same key twice |
| `max_live_runs` | `must be between 1 and 8` |
| `blocked_by` | `{KEY} is not on this board` · `a card cannot block itself` · `would close a cycle: {KEY} → {KEY} → {KEY}` |
| `automation` | `automation is available on worktree boards only` (a board that runs in its own worktree, only) · `automation is available on local boards only` · `automation is unavailable on a worktree owned by host {host}` · on a card-worktree board, at start: `{KEY} has no worktree to run in; link a pull request or create its worktree first` · `{owner/name} is not a Fleet repository; clone it into this context first` · `The worktree for {owner/name}#{n} has local changes; commit or discard them before the review runs.` |
| `pull_request` | `expected a GitHub pull request URL or owner/name#number` · `a pull request card needs a pull request` · `{pr key} is already on this board as {KEY}` |
| `requested_at` | `must be an RFC 3339 time` |

Routing may only ever point forward. A column that sent a card back would let one run's success
start the run of a column the card had already passed, and the pair would trade the card between
them for as long as the runs kept succeeding — a loop no later refusal can break, because every
individual move in it is legal.

The cycle sentence lists the path in display keys from the card being written back to itself, so a
two-card cycle reads `would close a cycle: FLT-1 → FLT-2 → FLT-1`. A blocker that is not on this
board is named by its card id, which is the only name the board has for it.

The `automation` sentences are raised by the daemon (§4.1), not by `fleet-core`; they are fixed
here so the app and the CLI show the same words. The first is raised only for a board whose
`run_location` is `BoardWorktree`: a context board that runs in card worktrees automates, and a
context board that runs in its own worktree still refuses with the original sentence. The host
sentence is raised for the board's worktree on a board that has one, and for each card's worktree
when a card-worktree run starts. The three card-worktree sentences are recorded on the card as a
failed run rather than returned, as every start refusal is; the local-changes one is raised when a
card's existing worktree cannot be brought to its pull request's current head without touching
the user's work (§4.1). `fleet-cli`'s own `child_environment` keeps
its `--env `-prefixed sentences and is not changed by this feature.

### 11.5 The derived reads

None of these is persisted; all are pure functions of a board and its cards.

| Read | Answers |
| --- | --- |
| `blocks(cards, card)` | The cards this one blocks — the reverse of everyone's `blocked_by`. |
| `is_satisfied(board, cards, blocker)` | Whether a blocker has reached a `Completed` column. A canceled card is a decision not to do the work, not a report that it is done, so it never satisfies. |
| `blocked(board, cards, card)` | `Some(Blocked { unsatisfied, tone })` while any blocker is unsatisfied. `tone` is `Warning` when one of them is canceled, archived or no longer on the board — nothing will release this card on its own — and `Muted` otherwise. |
| `latest_run(card)` | `runs.last()`. |
| `attention(card, now)` | Whether a person has to look: the latest run's outcome `needs_attention()` with no later manual `Moved`, or a `pending_run` older than `PENDING_AMBER_AFTER_SECS` (60) on a card that is not `queued`. |
| `queued(card)` | `pending_run` is `Some` and names a column other than the card's own: the card waits in a routing column for a run slot in its target (§11.7 rule 0). Waiting in line is the throttle working, not a problem, so a queued card never raises `attention`. |

`summarize` fills `BoardSummary.working_count` (a live or pending run) and `attention_count` from
the same two, which is how the board list and the pane header count them. It also fills
`idle_started`: the cards standing in a `Started`-category column with no live or owed run and not
counted by `attention` — disjoint from `attention_count` by construction, so `attention_count +
idle_started` is the number of cards waiting on a person, the count the Hub's Review tab shows
whether or not that board is open (`UX-SPEC.md` §2.2). It is on the wire as `idleStarted`, absent
at zero.

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
blocking it is done. A card moved into Ready with nothing blocking it starts at once, or waits in
Ready for a free slot (§11.7 rule 0). Nothing a person leaves in Todo can start itself. The preset sets no
`max_live_runs`, which leaves the board at one live run — a second concurrent run over one
checkout is a decision its owner makes deliberately.

Because an existing column is never rewritten, applying the preset to a board built from
`default_statuses()` adds Ready and In review but leaves the shipped `in-progress` column with
**no** `on_enter` action: that column already exists, and its automation is its owner's. A board
that wants the whole pipeline either starts from `workflow_preset()` or edits `in-progress`
afterwards.

**The reviews preset.** `reviews_preset()` is the column set of a Reviews board, and
`new_reviews_board(context, now)` builds the whole board around it: id `reviews_board_id(context)`
(`reviews-<context>`, capped at 64 bytes, trailing `-` trimmed), name `Reviews`, prefix `REV`,
kind `Reviews`, the labels `github` and `chat`, `run_location = CardWorktree`,
`max_live_runs = Some(2)` and `start_on_worktree = false`. No column names a provider, so the
daemon's default (Claude) runs every review.

| id | Name | Category | On enter | On success | When unblocked |
| --- | --- | --- | --- | --- | --- |
| `pending` | Pending review | Unstarted | — | — | `reviewing` |
| `reviewing` | Reviewing | Started | prompt, `PRESET_INSTRUCTIONS_REVIEW_PR`, expects `PRESET_EXPECT_REVIEW_PR` | `reviewed` | — |
| `reviewed` | Reviewed | Started | — | — | — |
| `published` | Review published | Completed | prompt, `PRESET_INSTRUCTIONS_PUBLISH_REVIEW`, expects `PRESET_EXPECT_PUBLISH_REVIEW` | — | — |
| `dismissed` | Dismissed | Canceled | — | — | — |

A card created in *Pending review* advances to *Reviewing* at once (rule 0), where a run reviews
the pull request in its own worktree and reports a verdict, a summary and numbered findings
without posting, committing or pushing anything; success carries it to *Reviewed*, where it waits
for a person. That person reads the report, comments corrections on the card if any (§11.1, *Notes
from you*), and moves it to *Review published*, whose run posts the review — with the notes
applied — as one `gh` review with inline comments, and reports its URL. *Dismissed* is a decision:
a dismissed pull request is never reopened (§2, `upsert_pull_request_card`). Only the *Review
published* column posts anything. The instructions and expectations are exact strings in
`defaults.rs` (`docs/decisions/0024-review-boards.md`); the review instructions use `{pr_url}`,
`{pr_repo}` and `{pr_number}`.

### 11.7 The engine

`fleet_core::board::automation` is pure: no daemon, no provider, no clock. `now` is passed in and
the side effects — reserving a slot, spawning a child, writing the document — are the daemon's
(§4.1).

```rust
pub fn re_evaluate(board: &Board, cards: &mut [Card], seeds: &[CardId],
                   live: &LiveIndex, in_flight: &mut BTreeSet<CardId>, now: &str) -> Result<Plan, BoardError>;
/// The same walk from seeds that did *not* enter their column: rule 1 is skipped for them.
pub fn re_evaluate_settled(board: &Board, cards: &mut [Card], seeds: &[CardId],
                   live: &LiveIndex, in_flight: &mut BTreeSet<CardId>, now: &str) -> Result<Plan, BoardError>;
pub struct Plan { pub starts: Vec<StartRun>, pub queued: Vec<CardId>, pub moved: Vec<(CardId, StatusId)> }
pub fn next_pending<'a>(board: &Board, cards: &'a [Card]) -> Option<&'a Card>;
/// `next_pending`, passing over the cards a freed slot already went to (the board's in-flight set).
pub fn next_pending_except<'a>(board: &Board, cards: &'a [Card], reserved: &BTreeSet<CardId>) -> Option<&'a Card>;
/// Clears the marker of every queued card rule 0 would now refuse; true when it cleared any.
pub fn clear_stale_queues(board: &Board, cards: &mut [Card]) -> bool;
pub fn brief(action: &Action, key: &str, card: &Card, reports: &[&Comment]) -> String;
pub fn resolve_prefs(card: &Card, action: &Action) -> ResolvedPrefs;
```

**The entry loop.** `re_evaluate` is breadth-first from `seeds` over the derived `blocks` index,
with a `seen` set so a diamond is visited once and a hand-built cycle terminates. Each visited card
gets three rules in this order:

0. *Advance on entry.* Run right after rule 1 for a card that *entered* (a move, a creation, a
   cascade move; never a settled seed), and only when its column names `advance_when_unblocked =
   Some(T)`, the card is not archived, not live, not in flight, and every blocker is satisfied — a
   card with no blockers qualifies. If `T` has an `on_enter` action and the board is at its
   ceiling (`live.len() + in_flight.len() >= settings.max_live_runs()`), the card **stays** where
   it is and is parked for `T` — `pending_run = PendingRun { status_id: T, since }`, keeping an
   existing `since` for the same `T` — and is now `queued` (§11.5). Otherwise it is moved to `T`
   as an `AutoMoved` reading `Moved to {T name}: nothing blocks it` (or `Moved to {T name}: a run
   slot freed` when it was queued) and queued in the walk, so rule 1 starts it in `T`.
1. *Start on entry.* If the card's own column has an `on_enter` action and the card is neither
   archived, nor live, nor reserved, nor already carrying a live run row, the walk either pushes a
   `StartRun` or parks the card.
2. *Advance the dependants.* Every card this one blocks whose column names
   `advance_when_unblocked`, is not archived, is not working, is not owed a run, and has no
   remaining unsatisfied blocker, is advanced there through the same step rule 0 uses — moved as an
   `AutoMoved` and **queued** in the walk, so the cascade runs to its end in one pass, or, when the
   target is out of run slots, left in its routing column parked for the target.

A cascade still only ever reaches a card through a *blocker* it has just visited, and a card
merely *standing* in a routing column is still released by nothing: rule 0 fires on **entry**. A
card that nothing blocks, entering a routing column — moved there by a person, created there, or
carried there by a cascade — advances at once or waits in place for a slot; it no longer waits
forever. That is why a card created in a Reviews board's *Pending review* starts reviewing with no
special case, and why a workflow card moved into Ready with nothing blocking it starts at once.
Nothing fires for a card that was already standing in a column when the board changed around it.

**Entered, or settled.** Rule 1 is what a column does *on entry*, and a seed is not always an
entry: a card a run has just ended on is standing exactly where it stood while the run worked. A
trigger therefore says which it has: every column change seeds through `re_evaluate`, and a
delivery whose outcome moved nothing seeds through `re_evaluate_settled`, which skips rule 1 for
the seeds and keeps rule 2. Without that distinction rule 1 cannot tell the two apart — the card
is in an action column with no live run either way — and a run that ended would start its own
column again, and again when that one ended: an action column would spend a board's whole
allowance re-running one card, and a cancel would be followed by a replacement run. A card the
cascade *moves* has entered its new column and gets both rules, settled seed or not, which is how
a routed success carries a card down a chain.

**The reservation.** `in_flight` is one `BTreeMap<CardId, StartReservation>` **per board**, held
by `Automation` in an outer `BTreeMap` keyed by board id and lent to every walk through the single
`Boards::evaluate_with_reservation`. Per board because the ceiling it is counted against is:
`settings.max_live_runs` belongs to one board, and one daemon-wide set would let a card reserved
on one board park a card on another — a park only some *other* board's freed slot would ever
release. Each entry carries a phase. A `StartRun` inserts its card as **creating the worktree**;
in that phase move, archive and delete remain allowed, because `prepare_run` re-reads the card
after worktree creation and abandons a start whose card changed or disappeared. Once that final
read succeeds, the entry becomes **launching** before the board gate is released. Move, archive
and delete then refuse with `card is starting`, because the provider is being created for the
recorded status and action. A cancel in this phase marks the reservation; when the provider
answers, `record_run` stops the new delegation through the normal cancellation path and records no
live row. `record_run` also revalidates the card, status and action under the board gate, stopping
the delegation and logging when any no longer matches. It removes the entry only after recording
the run, stopping it, or recording the refusal. Without the reservation two evaluations racing on
one board would each see a free slot and start two runs for one ceiling; it is also what makes
`start_run`'s "is working" refusal true before a run row exists.

**The throttle, in order.** The ceiling is `live.len() + in_flight.len() >= settings.max_live_runs()`
— the runs the document remembers plus the runs this daemon has promised. A card that meets it is
*parked*: `pending_run { status_id, since }`, and nothing is announced, because a card waiting for a
slot has had nothing happen to it. A card bound for a full column through a routing column waits
**in its routing column**, parked for the column it is bound for (`queued`), rather than being
moved into an action column it cannot start in. A card already parked for that same column **keeps its original
`since`**, so re-evaluating a board does not send its longest waiter to the back of the queue. When
a run ends, the daemon's maintenance task sees the terminal `DelegationChanged` and calls
`on_slot_released`, which asks `next_pending_except` for the oldest `since` on each board that
holds one and evaluates from there. It passes over the cards the board's in-flight set holds: a card
an earlier freed slot went to keeps its marker until its start is recorded, so two runs ending
together would otherwise both hand their slot to that one card, and the second slot would be lost
until another run ended. When that card is queued — standing in a routing column, parked for the
column after it — the evaluation moves it on through rule 0 (`Moved to {T name}: a run slot
freed`), which clears the marker, and starts it; a queued card keeps its place in line because
re-evaluating never restamps its `since`. Before it asks, the slot release runs
`clear_stale_queues`: a queued card that something now blocks, or whose column no longer routes to
the column it waits for, is one rule 0 would refuse, and left in line it would take every freed slot
and start nothing, so every card behind it would wait for ever. Its marker is cleared silently; rule
2 moves it on, and it queues again, once nothing blocks it. Nothing bypasses the ceiling, `start_run` included: it lets a person
re-run a card whatever its last run ended as, but a board that is already full parks the card
instead, because the runs of one board share one checkout and no request can make that untrue.

**The outcome table.** A card-called delegation is terminal exactly once, and `on_run_delivered`
maps it:

| `DelegationStatus` | `RunOutcome` | What else happens |
| --- | --- | --- |
| `Succeeded` | `Succeeded` | the `on_success` move, as an `AutoMoved` reading `Moved to {column}: run succeeded`; the card has entered a column, so that column runs |
| `Failed` with `status_payload == "reported blocked"` | `NeedsYou` | nothing moves; `attention` raises the card |
| `Failed` | `Failed` | nothing moves |
| `Incomplete` | `Incomplete` | nothing moves |
| `Cancelled` | `Cancelled` | nothing moves |
| `Starting` · `Running` · `Blocked` · `Settling` | — | not terminal: the run row is left open and the delivery row closed, because the next drain would find it no more terminal than this one |

Every terminal row but the first leaves the card where it is, and so does a success whose column
routes nowhere: those deliveries seed the evaluation as **settled**, so the card keeps its outcome
for a person to read and only the cards it blocks are walked. `>` (`start_run`) is the one verb
that runs a card standing still.

The write is idempotent by delegation id — a run whose row already has an `outcome` writes nothing
and still closes its delivery — and it is one write: `ended_at`, the outcome, `detail`,
`files_changed`, the cost and tokens, the capped report comment, the `RunEnded` entry, the
`on_success` move, and the evaluation that cascade produces, all saved once. Only after that is the
`Deliver` row marked done (`NATIVE-AGENTS.md` §15.7).

**What a restart does.** `resume_automation` runs once, from the maintenance task that also serves
the freed slot — it subscribes to the bus *before* it sweeps, so a run that ends while recovery is
still walking is not lost. The sweep does not wait for the delegation worker's first drain, and
does not need to: every disposition below is idempotent, so the two may interleave in either order.
It reads the delegation service *before* it takes each board's gate, and classifies every open run
row by what that service still knows:

| What the store says about the run | Disposition |
| --- | --- |
| still live | **adopt** — the row stands; the worker will close it |
| terminal, delivery `Pending` or `Delivered` | **adopt** — the outbox still owes it, and the first drain delivers it |
| terminal, delivery `Recorded`, `Consumed` or `Undeliverable` | **deliver** — nothing will ever deliver it again, so the sweep calls `on_run_delivered` itself |
| no such delegation | **close** — `Incomplete`, with `the daemon lost this run's record while it was down, so what it did was never reported` |
| a storage failure | left open — the card keeps refusing moves until the next restart, which is the safe half |

A live delegation whose card carries **no** row for it is the crash window between `run_for_card`
answering and the row being written: the sweep writes the row from the delegation's own `created`,
with a `RunStarted` entry reading `Run adopted after the daemon restarted`. Only then does the
board evaluate, and its seeds are the cards the board still *owes* a run — one carrying a
`pending_run`, or one whose `RunStarted` entry has no run row at or after it. Seeding every card
would start a run for every card merely standing in an action column — the one thing §4.1 promises
never happens — and on a board whose runs have all finished it would re-run all of them on every
restart.

### 11.8 On the board face

The app draws automation in four places and derives none of them in a `render`
(`APP-CONTRACTS.md`): one fold, `AppState::refresh_card_marks`, turns the view, the delegation
mirror and the clock into `BoardState.marks`, and the projection reads that map.

The mirror is the half that does not wait for a round trip, and a card's run mark is read from it
first. The daemon records a run on the card and announces it as a `BoardChanged`, which the app
answers with a whole `EnsureWorktreeBoard`; a card-called `DelegationChanged` names its own board
and card and lands in milliseconds. So a card whose child the mirror holds live reads `working` —
or `needs you` the moment that child blocks — before any board response carries the run at all,
and a run that lives five seconds is marked for the whole of it rather than for whatever is left
once the reload arrives. The card stays the authority on a run it has already ended: once its own
row carries an outcome, a mirror row that has not caught up says nothing.

The keys read that same join. `[` / `]` asks `Move {KEY}?` when the mirror holds a live
card-called child **for this card**, and `X` finds a run to stop on the same evidence — neither
waits for the card's own `runs` row, because a face that says `working` and a key that says
`{KEY} has no live run` would be two answers about one card (`UX-SPEC.md` § Board).

A card's key line is a row whose right end carries one **state pill**: either its **run mark**, in
words, or what blocks it — the run mark wins, because a card that is running has nothing left to
wait for. The five marks are `pending`, `stalled`, `working`, `needs you` and `done` (§11.5 is
where the first four come from), and the pill says them as `waiting`, `waiting 2m`,
`working 4m · codex` (the age from the live run's `started_at`, the provider from its row),
`needs you`, and `review passed` (named after what the column that ran it does); a canceled run
draws nothing, and a success draws nothing either when the column that ran it carries the card on
by itself — a check beside the key would otherwise mark every card the workflow already advanced.
The blocked pill names the one unsatisfied blocker by key (`blocked by FLT-5`) or counts them
(`blocked by 2 cards`); it is neutral while a blocker can still finish and amber when one of them is
canceled, archived or gone from the board, which is `blocked()`'s own tone. A card whose run needs
you carries an **Answer** button in its meta row on the worktree board, which runs `A`.

A column whose entry runs an action wears a pill under its header naming it — `On enter: codex
implements` — from `on_enter` alone, never `on_success` or `advance_when_unblocked`, which move a
card the column has already finished with. The provider is the action's own (`agent` when it
leaves the choice to the card); the verb is read from the skill's name or the first word of the
prompt's instructions, and falls back to `runs the card` / `runs <skill>`. Clicking the pill opens
Board settings drilled into that column (§11.10).

The board header's subtitle carries two zero-suppressed counts: `1 of 2 runs working` over
`settings.max_live_runs`, counting the cards that hold a run slot, or — while a card is owed a run
the limit has no slot for — `1 working · 1 waiting`, the live and the owed runs stated apart so
the count never reads over the limit; and, in amber, `1 needs you`: the cards `attention()` raises,
plus any card whose child is still out and parked on a person's answer — `attention()` reads the
card alone and cannot see that child, but its tile already says `needs you` from the delegation
mirror (§11.8), and the header counts what the tiles say. Clicking the count selects the first
card whose mark waits on a person.
They are the board-face version of `BoardSummary`'s two counts (§11.5), computed in the app from
the same definitions — the needs-you count adding only what the mirror knows before the card does. The harness's `board.summary` row keeps its
compact `1/1 working · 1 needs you` form, whose numerator counts live **and** owed runs, and
marks the owed ones `waiting:N` (`TESTING-HARNESS.md` §3).

A run whose start never reached a thread has no mark to draw, so it raises the sticky error once
instead, with the run's own `detail`. `UX-SPEC.md` § Board states the glyphs, tones and wording.

A card with a pull request carries its reference on its tile — `owner/name#123`, a short muted
mono line through `CardTile::reference`, filled from `pull_request.key()` by the projection memo,
never in render. The kit tile knows nothing about pull requests.

**The schedules strip.** A board with schedules (§12), on a daemon that advertises `schedules`,
shows one compact strip in its header: one schedule reads `⟳ GitHub reviews · next 14:05 · last 3
created` (its name, its next fire time, its last summary); several read `⟳ 3 schedules · next
14:05`. A failed or timed-out last run tints the strip with the theme's warning tone, as the
header's conflict counter is tinted. The strings are precomputed in the board projection whenever
the schedules mirror or the minute tick changes — the way `synced <age>` stays current — and never
formatted in render. Clicking the strip or pressing `T` opens Board settings on its Schedules
section; `R` runs every enabled schedule of the board now, one `RunScheduleNow` each. (`S` and `F`
were the first choice and are taken on the board by sync and full sync.) `R` on a board with no
enabled schedule says `No enabled schedule on this board`, before the board's schedules are listed
`schedules are still loading` (or the load's error), and on a daemon without `schedules` the
capability sentence; nothing is sent in any of these. A run-now the daemon records as `Skipped`,
because the schedule's previous run is still going, says `skipped {name}: {summary}`. The next time reads `HH:MM` in local time today,
`Sep 25 09:00` on another day and `now` once due — with several schedules, the earliest enabled
one — and `disabled` when none is enabled. The last clause is the first clause of the newest
non-skipped run's `SUMMARY:` (`last 3 created`), else `last succeeded` / `last failed` / `last timed
out`, and `running` while a run is live; the summary clause is cut at 40 columns with `…`, since a
run with no `SUMMARY:` line carries its last output line there. The board asks for its schedules
the first time it is drawn, so the strip does not wait for Board settings to be opened. Without the
capability, or with no schedule, there is no strip. A worktree board stored on another host has no
schedules surfaces at all — no strip, no Schedules section, no palette rows — because schedules run
on this machine's daemon, which does not store that board; `T` and `R` there say `this board is
stored on {host}; schedules run only on this machine's boards`.

### 11.9 On the card detail

The card detail states a run rather than counting it. Between the title and the description, a
card with a run — or owed one — carries a **run row**: the board's own mark, then `working 4m ·
codex · gpt-5 · high`, with the token count and cost appended once the run is over. A missing
model or effort drops with its separator. A card waiting for a slot reads `pending 2m · waiting
for a slot`; an owed run wins over a finished one. The sheet draws it as a card — the state
sentence-cased (`Working 4m`), the usage at its right end — with `Attach A`, `Re-run >` and
`Cancel run X` beneath, each drawn only when its key would work (`CardMenu::of`).

A card with a pull request carries a `Pull request` fact row, `owner/name#123 · open ↗`, in the
same row style as `Worktree`: `enter` on it opens the pull request's URL in the browser, as the
PR screen's `b` does. `B` opens it and `y` copies its URL (toast `PR URL copied`) from the detail
and from the Review board (`b` stays *blocked by* on every board surface); the card's `⋯` and
right-click menu offer the same two as `Open pull request` and `Copy pull request link`. On a card
without one either key says `No pull request on this card` — on the detail's error line while the
sheet is up, as a toast on the board.

Under `Status`, five zero-suppressed property rows: `Provider`, `Model` and `Effort` while the
card's column runs an action — showing what `resolve_prefs` (§11.7) will actually use, with a
muted `column default` beside a value the column supplied — then `Blocked by` and `Blocks`, one
row per link, as soon as the board uses links at all. A satisfied blocker is checked rather than
dropped, and `Blocks` is derived from everyone else's `blocked_by`, never stored. Each row opens
the picker that edits it; a link that would close a cycle is offered disabled with `would cycle`,
the same answer `validate_links` would give (§11.4).

A run's report comment (§11.2) renders with its provider as the author and a `run {n}` badge, and
folds at eight lines, the fold a delivered child result gets in the transcript. `UX-SPEC.md` § Board,
"Card detail", states the rest.

### 11.10 Configuring columns in the app

Board settings gained a **Columns** section beside General and Backend (`,` opens the dialog where
it was last left, `C` opens it on Columns, and a column's automation pill on the board opens it
drilled into that column). It is the same read-modify-write of the whole
`statuses` vector that `fleet board columns` performs, with the same rules. The list is one card
of rows, each led by its category's glyph, whose helper names the category, what entering the
column runs (`⚡ codex runs the prompt`) and where the card goes after (`then → In review`): `n`
new, `d` delete, `J`/`K` reorder, `P` for the missing preset columns (§11.6, never rewriting one
the board already has), and `⏎` to drill into a column. The footer carries *New column* and
*Apply preset*; Move up, Move down and Delete column are the row's hover actions and its
right-click menu, not footer buttons. A column opens under a breadcrumb (`‹ Columns  In review`)
as three cards: Name and Category; *When a card enters*, holding On enter and, while it is not
`none`, the seven action rows; and *After the run*, holding On success and When unblocked. `on
enter` is spelled as `--on-enter` spells it.

Nothing is sent until `^s`, which is one `UpdateBoard` carrying the whole vector — so a reorder, a
rename and a routing change are one request, and every refusal of §11.4 arrives against the board
the user actually means to save. Deleting a column that holds cards asks where they go and moves
them first, one `MoveCard` each in column order, stopping at the first refusal and leaving what
already moved where it is. On a board automation is not available for, a column's form folds
the *When a card enters* and *After the run* cards away behind one callout — `Automation is
available on worktree boards.` on a context board with no worktree, `Automation is available on
local boards.` on a board whose backend is not local — and the list shows no notice. That is a board with no worktree
that runs in the board's worktree, or a board whose backend is not local:
`(worktree_id.is_none() && run_location.is_board_worktree()) || backend.kind != LOCAL`. A Reviews
board — a context board that runs in each card's worktree — therefore edits its columns and actions
like a worktree board does. General's *Runs* card carries `Max live runs` with the helper *Runs
on this board share one checkout, so keep it small.* (*Each run works in its card's worktree.* on
a Reviews board), and a read-only row `Runs in` reading `each card's worktree` or `this worktree`
from `settings.run_location`, which the dialog's subtitle repeats. It is not editable in v1: changing it on a board with live runs would
strand them.

Board settings has a fourth section, **Schedules**, shown only when the daemon advertises
`schedules` and stores the board (not on a remote worktree's board, §11.8); §12 describes it.

## 12. Schedules

[ADR 0025](decisions/0025-scheduled-agent-tasks.md) records why. A **schedule** is a prompt a
board owns, run headless by the daemon on a cadence — every N minutes, or once at a time — as one
`JobKind::ScheduledTask` job per run. The agent uses whatever tools, MCP servers and skills the
user already has installed, and its output is cards: the daemon appends a footer that teaches it to
record every pull request it finds with `fleet board card new --pr …`, which is idempotent (§2,
§6). The shipped use is filling a Reviews board (§11.6) from GitHub and from sources Fleet cannot
see, such as a chat channel an MCP server reads.

### 12.1 The model (`fleet_core::schedule`)

```rust
// crates/fleet-core/src/ids.rs
string_id!(ScheduleId, "schedule", validate_slug);   // generated as "sch-" + 8 lowercase hex
pub fn new_schedule_id() -> ScheduleId;

// crates/fleet-core/src/schedule.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: ScheduleId,
    pub board_id: BoardId,                 // the owner; deleting the board deletes the schedule
    pub name: String,                      // 1..=SCHEDULE_NAME_MAX_CHARS
    pub prompt: String,                    // the user's text; placeholders rendered at fire time
    pub cadence: Cadence,
    pub agent: ScheduleAgent,
    pub enabled: bool,
    pub timeout_minutes: u32,              // 1..=SCHEDULE_MAX_TIMEOUT_MINUTES
    pub created_at: String,
    pub updated_at: String,
    pub runs: Vec<ScheduleRun>,            // oldest first, capped at MAX_RUNS_PER_SCHEDULE
    pub next_run_at: Option<String>,       // recomputed by the daemon on every change and run
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cadence { Every { minutes: u32 }, Once { at: String /* RFC 3339 */ } }

/// Default: Claude, no model, no effort, `FullAccess` — nobody is there to answer a prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleAgent { pub provider: AgentKind, pub model: Option<String>, pub effort: Option<String>, pub mode: PermissionMode }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRun { pub job_id: Option<JobId>, pub started_at: String, pub ended_at: Option<String>,
                         pub outcome: Option<ScheduleOutcome>, pub summary: Option<String>,
                         pub cost_usd: Option<f64>, pub log_path: Option<String> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleOutcome { Succeeded, Failed, TimedOut, Skipped }

pub struct ScheduleDraft { pub board_id: BoardId, pub name: String, pub prompt: String, pub cadence: Cadence,
                           pub agent: Option<ScheduleAgent>, pub enabled: Option<bool>, pub timeout_minutes: Option<u32> }
pub struct SchedulePatch { /* every ScheduleDraft field but board_id, each optional */ }

pub const SCHEDULE_MIN_EVERY_MINUTES: u32 = 5;
pub const SCHEDULE_MAX_EVERY_MINUTES: u32 = 1440;
pub const SCHEDULE_DEFAULT_TIMEOUT_MINUTES: u32 = 20;
pub const SCHEDULE_MAX_TIMEOUT_MINUTES: u32 = 120;
pub const MAX_RUNS_PER_SCHEDULE: usize = 20;
pub const SCHEDULE_PROMPT_MAX_BYTES: usize = 16 * 1024;
pub const SCHEDULE_NAME_MAX_CHARS: usize = 80;
pub const SCHEDULE_SUMMARY_MAX_CHARS: usize = 280;
pub const SCHEDULES_DOCUMENT_VERSION: u32 = 1;
pub struct SchedulesDocument { pub version: u32, pub schedules: Vec<Schedule> }

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ScheduleError { #[error("invalid {field}: {reason}")] Invalid { field: String, reason: String },
                         #[error("schedule not found: {0}")] NotFound(String) }

pub fn validate_schedule(schedule: &Schedule) -> Result<(), ScheduleError>;
/// Defaults: enabled = true, timeout_minutes = 20, agent = ScheduleAgent::default().
pub fn apply_draft(draft: ScheduleDraft, id: ScheduleId, now: &str) -> Schedule;
pub fn apply_patch(schedule: &mut Schedule, patch: SchedulePatch, now: &str);
pub fn next_run_at(schedule: &Schedule, now: DateTime<Utc>) -> Option<DateTime<Utc>>;
/// Appends, drops the oldest past 20, and returns the dropped runs' `log_path`s for deletion.
pub fn push_run(schedule: &mut Schedule, run: ScheduleRun) -> Vec<String>;
pub fn render_prompt(schedule: &Schedule, fleet: &str, now: &str) -> String;
/// The last line starting `SUMMARY:`, prefix and whitespace trimmed, cut to 280 chars at a char boundary.
pub fn summary_line(final_message: &str) -> Option<String>;
pub const SCHEDULE_FOOTER_TEMPLATE: &str;
pub const STARTER_PROMPT_GITHUB_REVIEWS: &str;
```

**The refusals** (`ScheduleError::Invalid { field, reason }`, printed verbatim by every surface):

| field | reason |
| --- | --- |
| `name` | `must not be empty` · `must not contain a NUL byte` · `must be at most 80 characters` |
| `prompt` | `must not be empty` · `must not contain a NUL byte` · `must be at most 16 KiB` |
| `cadence` | `every must be between 5 and 1440 minutes` · `once needs an RFC 3339 time` |
| `model`, `effort` | `must not be blank` |
| `timeout_minutes` | `must be between 1 and 120` |
| `mode` | `{mode} is not supported by {provider}` |
| `board_id` | `no board {id}` — the daemon's check, on create |

**When a schedule is due.** `next_run_at(schedule, now)` is `None` for a disabled schedule. A
`Once { at }` is due at `max(at, now)` until any run — including `Skipped` — has a `started_at` at
or after `at`; a manual run before `at` does not consume it, and editing a fired schedule to a new
future `at` makes it due again. Create and edit normalise `at` to UTC RFC 3339 to the second. An
`Every { minutes }` is due at `min(last started_at, now) + minutes`, clamped to at least `now`, or
at `now` when it has never run; this catches up once after downtime and still advances after the
wall clock steps backwards. A stored time that does not parse counts as "never ran". The minimum
cadence is five minutes because every run is real spend.

**The prompt a run receives** is the user's prompt with `{board}` (the board id), `{last_run_at}`
(the previous run's `started_at`, or `never`; a `Skipped` fire launched no agent and read no
source, so it is not a previous run) and `{now}` substituted, then a blank line and the
footer, with `{name}`, `{board}`, `{last_run_at}` and `{fleet}` (the quoted absolute path to the
`fleet` binary, or `fleet`) substituted:

```
--- Fleet scheduled task "{name}" for board {board} ---
Record every pull request you are asked to review as a card on this board with:
  {fleet} board --board {board} card new "<pull request title>" --pr <pull request URL> --requested-at <RFC 3339 time the review was requested> --label <source>
<source> is one of the board's labels; run `{fleet} board --board {board} describe` to list them.
The command prints "Created <KEY>", "Existing <KEY>" or "Reopened <KEY>" first; a pull request already on the board is never duplicated, so run it for every request you find.
Only consider requests made after {last_run_at} when a source keeps old messages (chat channels, email).
Do not review, comment on, or change any pull request. Do not edit files.
End your reply with exactly one line: SUMMARY: <n> created, <n> existing, <n> reopened, <anything the user must know>
```

The footer is the contract between a schedule and its board: the card-creation line it teaches is
§6's, and a card is reopened only when `--requested-at` is later than its last completion, so a
chat message that stays in its channel forever never resurrects a published review.
`STARTER_PROMPT_GITHUB_REVIEWS` is the prompt of the default schedule — the query the Review tab
used to run itself:

```
List every open pull request where my review is requested, directly or through one of my teams, with gh search prs --review-requested=@me --state=open --json url,title,repository,updatedAt. For each one, use its updatedAt as the requested time and github as the source label.
```

### 12.2 Storage

Schedules live in one document, `<FLEET_HOME>/schedules.json` (`FleetHome::schedules_path()`),
`SchedulesDocument` at version `SCHEDULES_DOCUMENT_VERSION` (1), owned by
`stores/schedules.rs::ScheduleStore` in the shape of the state store: a `tokio::sync::Mutex` gate,
file IO on `spawn_blocking`, and `atomic_write_text`. `load` answers an empty version-1 document
for a missing file; `transaction(f)` loads, applies, validates every schedule and saves, all under
the gate, so an invalid schedule refuses the save and leaves the file as it was. A document whose
version is not 1 is **refused** by name and left in place — a newer daemon's file must survive an
older daemon. An unparsable document is **quarantined** to `schedules.json.broken-<epoch millis>`
and read as empty, with one `warn` naming the path. A readable one loses only what this build
refuses: a blank `model` or `effort` — which a build before that refusal could store — reads as
`None`, the provider's default, which is what it always meant; a schedule `validate_schedule` still
refuses, or a second one with an id already seen (`duplicate schedule <id>`), is set aside — the
file as it was is copied to `schedules.json.broken-<epoch millis>`, one `warn` names each schedule
and the copy, and the repaired document is written at once — and every other schedule is kept. Every
time the service writes is RFC 3339 UTC to the second (`2026-09-24T10:00:00Z`).

Each schedule has a directory, `<FLEET_HOME>/schedules/<id>/`: `work/` is every run's working
directory, and `logs/<YYYYMMDDTHHMMSSZ>.log` holds one run's output, named from its `started_at`.
Each stdout or stderr line is capped at 64 KiB including a truncation marker; the rest of an
over-long line is drained without buffering. A log stores at most 16 MiB of complete lines plus
one truncation-marker line. Output after that cap is still drained and parsed for the provider's
result, but is not persisted. A run's `started_at` is its key — its job, its outcome and its log are
matched by it — so it is unique within the schedule: a fire in the same second as a run already
recorded is stamped one second later. The log of a run `push_run` drops past the cap is deleted with
it; the cap drops the oldest *finished* runs and never a live one, whose child is still writing its
log. Retention unlinks a stored `log_path` only when its absolute, parent-dir-free path has a
canonical parent inside that schedule's `logs/` directory; malformed or escaped paths are removed
from history but left on disk. Deleting a schedule deletes its directory.

### 12.3 How a schedule runs

`services/schedules/runner.rs::ScheduleRunner::run(schedule, prompt, cancel)` launches the
provider's CLI through the `Shell` adapter — never `std::process` directly — with the prompt as the
last argument, after `--` so a prompt that opens with `-` (a Markdown list) is never read as an
option, passed as argv with no shell:

```
claude -p --output-format stream-json --verbose --permission-mode <wire> [--model M] [--effort E] -- <prompt>
codex exec --json --skip-git-repo-check <mode flags> [-m M] [-c model_reasoning_effort="E"] -o <last-message file> -- <prompt>
```

`<wire>` is the Claude adapter's own `permission_mode_to_wire`. Codex's mode flags are
`--dangerously-bypass-approvals-and-sandbox` for full access, `-s workspace-write` for accept-edits,
and `-s read-only` for ask and plan; `codex exec` has no prompt channel, so *ask* cannot ask anyone.
The program is the configured `agent_binaries` entry for the provider, else bare `claude` / `codex`.

The run's working directory is the schedule's `work/`, created on demand. Its environment is the
login environment of that directory, with the directory of the `fleet` binary prepended to `PATH`,
`FLEET_HOME=<daemon home>`, `FLEET_BOARD=<board id>` and `FLEET_SCHEDULE=<schedule id>` set, and the
variables the Claude and Codex adapters strip removed. The footer's `{fleet}` is
`ScheduleRunner::fleet_program`, the very binary whose directory leads that `PATH`. The timeout is
`timeout_minutes`; a timeout, a cancel and a
daemon shutdown that drops the run all kill the child's whole process group, and so does the
agent's own exit (`ShellCommand::kill_group_on_exit`), so the MCP servers and tool shells an agent
started go with it. Any command streamed through the shell whose descendants keep its output open
after it exited is not failed for it: after the two-second drain those descendants are killed, a
`warn` names the command, and the exit status stands. A shell error names the program and its
subcommands only, never the prompt. Every capped output line reaches the result parser and, until
the run-log cap, the log file. For Claude the last `{"type":"result"}` line is kept (non-JSON lines
are ignored); for Codex the `-o` file is read after exit, lossily like the output stream, so an
invalid byte does not cost the run its `SUMMARY:` line.

| What happened | Outcome | Summary |
| --- | --- | --- |
| exit 0 and the result is not `is_error` | `Succeeded` | `summary_line` of the final message |
| the timeout elapsed | `TimedOut` | the summary line, else the last non-empty output line, else `stopped after {timeout_minutes} min`, cut to 280 |
| cancelled | `Failed` | `canceled` |
| anything else | `Failed` | the summary line, else the last non-empty output line, cut to 280 |
| the previous run was still live when this one was due | `Skipped` | `the previous run was still going` |
| the daemon stopped while the run was live | `Failed` | `the daemon stopped during this run`, written on the next start |
| the schedule's board no longer exists when it is due | `Failed`, launching nothing, and the schedule is disabled | `board {id} no longer exists; the schedule is disabled` |

The cost is Claude's `total_cost_usd`; Codex's exec JSON carries none, so its runs record no cost.

### 12.4 Firing (`services/schedules`)

`Schedules` is a daemon service composed next to `Boards`: it owns the store and the runner, reads
the boards service to check a board exists, and keeps a `live` map from schedule to the exact
`started_at` and job this daemon owns. Every mutation — `create` (refusing an unknown board with `no board {id}`), `update`, `delete`,
`run_now` — goes through `store.transaction`, recomputes `next_run_at` from the daemon clock,
publishes `Event::SchedulesChanged { board_id }` and wakes the loop through a `Notify`, so an edit
re-plans at once.

The loop, `tick.rs::run_schedules`, is one of the maintenance task's periodic sweeps
(`ARCHITECTURE.md`): it loads the document, finds the earliest `next_run_at`, and sleeps until then,
until a change wakes it, or for at most 60 seconds — so a wall-clock jump or a machine that slept is
noticed within a minute — and stops on shutdown. On waking it fires every schedule whose
`next_run_at` has passed. A store that keeps failing is warned about through `RepeatedFailure`, not
on every tick.

**Firing** a schedule whose previous run is still in `live` records a `Skipped` run and starts
nothing. Otherwise it renders the prompt, records a `ScheduleRun` with `started_at`, the log path
and no outcome, saves, and submits a job — kind `ScheduledTask`, target the schedule id, title
`Scheduled: {name}`, cancellable, not retryable. The job calls the runner with its cancellation
token and then writes the outcome, summary, cost and `ended_at` onto that same run (matched by
`started_at`), deletes the logs `push_run` dropped, leaves `live`, recomputes `next_run_at`, saves and
publishes. A job that errors still records a `Failed` run, and a failed or timed-out run fails its
job too. A job the manager drops before its operation reports — deduplicated against a finishing
job, or dropped after the cancel grace — records a run through a drop guard: `Skipped` when it
never started, `Failed` with `the run stopped before it finished` when it had. So a run is visible in the Jobs panel
(`J`, slug `sched`), cancellable there, and its log outlives it.

On start the service marks every run with no outcome `Failed` (`the daemon stopped during this
run`) and gives it an `ended_at`, except the one exact `started_at` its current `live` entry owns.
Deleting a board — directly, through its context's cascade, or
with the worktree a worktree board belongs to — deletes its schedules and their directories. The
dispatch site that deletes boards makes the call, and `Schedules` is the worktree cascade
`Worktrees` runs (it deletes the worktree's boards through `Boards`, then their schedules), so
`Boards` does not depend on `Schedules`. Board existence is also checked when a schedule
fires, because a create can interleave with its board's delete and a cascade that fails is only
logged: a schedule whose board is gone records the `Failed` run above, launches no agent and is
disabled — not deleted, so a board whose document was quarantined and repaired finds its
schedules again.

### 12.5 Wire and client

An additive extension advertised through `schedules`; `PROTOCOL_VERSION` stays 8.

```rust
pub const SCHEDULES_CAPABILITY: &str = "schedules";
ListSchedules { #[serde(default, skip_serializing_if = "Option::is_none")] board_id: Option<BoardId> } → Schedules(Vec<Schedule>)
CreateSchedule { draft: ScheduleDraft }                                    → Schedule(Schedule)
UpdateSchedule { id: ScheduleId, patch: SchedulePatch }                    → Schedule(Schedule)
DeleteSchedule { id: ScheduleId }                                          → Ack
RunScheduleNow { id: ScheduleId }                                          → Schedule(Schedule)   // the run recorded as started
// EventKind::SchedulesChanged ; Event::SchedulesChanged { board_id: BoardId }
// JobKind::ScheduledTask — the job a schedule run is; on the wire {"custom":"scheduled_task"}, the
// extension point a peer built before schedules already decodes (a bare "scheduled_task" still reads)
```

| Request | Answers |
| --- | --- |
| `ListSchedules { board_id }` | every schedule, or one board's |
| `CreateSchedule { draft }` | the new schedule, with its `next_run_at` |
| `UpdateSchedule { id, patch }` | the schedule after the patch |
| `DeleteSchedule { id }` | `Ack`; the schedule's directory goes with it |
| `RunScheduleNow { id }` | the schedule truncated so its newest run is the one this request recorded as started (or `Skipped` while one is live) |

`SchedulesChanged` is published to subscribed local clients as `BoardChanged` is, and never
forwarded from a remote host: schedules are local to the daemon that runs them. A client names
`SchedulesChanged` in its `Subscribe` only to a daemon that advertises `schedules`, because an older
daemon cannot decode the kind and would refuse the whole subscription. The client has
`list_schedules`, `create_schedule`, `update_schedule`, `delete_schedule` and `run_schedule_now`,
each mapped to `schedules` in `required_capability`, so an older daemon gets the "run `fleet daemon
restart`" error before anything is sent.

### 12.6 CLI (`fleet schedule`)

The same board selectors as `fleet board`, resolved by the same code — `--board`, `--worktree`,
`--context`, `--reviews` — and `--json` for `ScheduleEnvelope { protocol: 1, schedule }` /
`SchedulesEnvelope { protocol: 1, schedules }`:

```
fleet schedule list                                   # header row, then id  name  cadence  next  last outcome  last summary
fleet schedule show <id>                              # fact lines and the last five runs, newest last
fleet schedule new --name N (--prompt T | --prompt-file F | --starter github-reviews) (--every MIN | --once RFC3339)
                   [--provider claude|codex] [--model M] [--effort E] [--mode ask|accept-edits|plan|auto|dont-ask|full-access]
                   [--timeout MIN] [--disabled]
fleet schedule edit <id> [any flag of new]            # an empty patch is refused: `nothing to change`
fleet schedule rm <id>
fleet schedule run <id> [--wait]                      # prints `Started {job id}` (or `Skipped {id}: {summary}`); --wait polls once a second until the run has an outcome
fleet schedule runs <id>                              # header row, then every run, newest last (as `card runs`), with its log path
```

`--prompt`, `--prompt-file` and `--starter` are mutually exclusive and one is required on `new`,
as are `--every` and `--once`; `edit` takes `--enable` / `--disable` in place of `--disabled`. `new`
and `edit` treat an empty `--model ""` or `--effort ""` as clearing that value; changing
`--provider` without either flag clears both because they belong to the old provider. `new`
prints `Created {id}` and `edit` `Updated {id}`, each followed by the `show` text; `rm` prints
`Deleted {id}` (`{"protocol":1,"ok":true}` with `--json`); `list` on a board with none prints `No
schedules on board {board}`. The verbs that name an id find it across every board when no selector
is given, and only on the selected board when one is (`schedule not found: {id} on board {board}`).
`run --wait` exits 1 when the run failed or timed out and 0 when it succeeded or was skipped. Times
print as `YYYY-MM-DD HH:MM` in UTC, as the daemon records them. Cadence cells read `every 15m` or `once 2026-09-23 09:00`, and an
empty cell `—`. The GitHub review schedule on a context's Reviews board:

```
$ fleet schedule --reviews new --name "GitHub reviews" --starter github-reviews --every 15
$ fleet schedule --reviews run sch-3f9a01c2 --wait
$ fleet board --reviews show
```

### 12.7 In the app

Board settings has a fourth section, **Schedules**, shown only when the daemon advertises
`schedules`. It is shaped like Columns (§11.10): a list level and a form level, `⏎` drills in, `^s`
saves. The list is one card of rows, one per schedule, each led by its enabled switch: the name,
the helper `every 15 min · claude · full access`, and at its end when it runs next (`next 14:05`,
`running · 2 min` or `disabled`) over the last result's glyph and tone (a check success, a cross
failed or timed out, a slash skipped) and its summary. `n` new, `⏎` edit, `space` or the switch
toggles enabled (`UpdateSchedule`), `r` or the row's hover *Run now* runs it now
(`RunScheduleNow`), `d` asks on the dialog's amber footer line (`delete {name} and its run logs? d
again to delete, esc to keep it`) and a second `d` deletes; the footer carries *New schedule* and
*Delete*, and a right-click on a row opens *Open*, *Run now* and *Delete*. The form opens under a
breadcrumb (`‹ Schedules  GitHub reviews  next 14:05`) as cards: Name and Enabled; *Cadence*,
`Repeat` (`every` / `once`) with `Every [15] min` or `Once at [2026-09-23 09:00]`, and Timeout;
*Agent*, Provider, Model (a dropdown of the reported models, or free text as a column's is),
Effort and Mode (the provider's supported modes, full access by default); Prompt (the multi-line
box a column's Instructions row uses); and a read-only *Last runs* card with the last five runs'
outcome, time, summary and log path. `^s` sends `CreateSchedule` or `UpdateSchedule`; a daemon
refusal appears in the dialog's red footer line and keeps it open, and leaving a form with unsaved
edits asks once, in amber (`Unsaved schedule. Press Esc again to discard it.`). `n` on a Reviews board pre-fills the starter —
name `GitHub reviews`, every 15 minutes, `STARTER_PROMPT_GITHUB_REVIEWS` — and on any other board
leaves the prompt empty. The empty Review tab's `⏎ add the GitHub review schedule` opens here in
that state. The list is the schedules mirror (§8), refreshed on `SchedulesChanged`; nothing is read
in render. The board header's strip (§11.8) shows the same schedules at a glance.
