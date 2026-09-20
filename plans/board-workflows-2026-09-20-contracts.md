# Board workflows — Contracts (authoritative for the build)

> This file fixes every shared type, signature, constant, sentence and file boundary the nine phase
> plans leave open, so a phase can code against a later phase's work before it exists.
> Roadmap: ./board-workflows-2026-09-20-roadmap.md. Phase plans: ./board-workflows-2026-09-20-phase-N-plan.md.
> When a phase plan and this file disagree on a *shape*, this file wins. When they disagree on a
> *behaviour*, the phase plan wins. Anything not fixed here follows the nearest existing pattern in
> the crate being edited. The design doc's Decisions section is binding on both.

## 0. Rules every agent follows

- Repo: `/home/df/.fleet/worktrees/dannyfuf/fleetd/feat-workflows-v1`. Rust 2024, toolchain pinned.
- Read, in order: this file, the phase plan for your task, then the files you own. Load the
  `.claude/skills/` skill the phase plan names before editing that area.
- Own only the files your task lists. Anything you need elsewhere goes under `INTEGRATION NOTES`
  in your report (exact file, exact code). Never reformat or "tidy" a file you do not own.
- No git operations. The orchestrator commits. No new dependencies unless the task says so.
- `cargo fmt` only on files you own (`rustfmt --edition 2024 <file>`); never `cargo fmt --all`
  while other agents edit the tree.
- Verify with the narrowest command that proves your work, then the phase's verification block.
- Repo non-negotiables (`CLAUDE.md`): no `unwrap`/`todo!`/`unimplemented!`/`dbg!`/`TODO` in
  production code; `expect` only for static invariants with a message; never `let _ =` a fallible
  call; render prepares nothing; kit components take no domain types and no literal colours, sizes
  or durations; every spawned fallible task ends in `.detach_and_log_err(cx)` or is stored; docs are
  authoritative and ride with the code.
- Serde conventions (copied from `fleet-core`): structs `#[serde(rename_all = "camelCase")]`;
  unit enums `#[serde(rename_all = "snake_case")]`; every new `Option` field
  `#[serde(default, skip_serializing_if = "Option::is_none")]`; every new `Vec`
  `#[serde(default, skip_serializing_if = "Vec::is_empty")]`; every new `bool`
  `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`. A new field without these
  changes a byte-exact golden and is a bug.
- Report format, ≤ 40 lines, last thing you print:

```
STATUS: GREEN|RED|BLOCKED — one line why
FILES: <every file created or modified, one per line>
TESTS: <commands run and their result>
DEVIATIONS: <where you departed from the contract or plan, and why; "none" otherwise>
INTEGRATION NOTES: <exact edits another owner must make, with file and code; "none" otherwise>
```

## 1. `fleet-core::board` — model (phase 1)

All in `crates/fleet-core/src/board/model.rs` unless stated. `AgentKind`, `PermissionMode` come
from `fleet_core::agents`; `DelegationId`, `ThreadId` from `fleet_core::agents::ids`.

### 1.1 Column automation (on `Status`)

```rust
pub struct Status {
    // id, name, category, color unchanged
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<ColumnAutomation>,
}

/// Every field optional. `ops::patches::normalise_automation(&mut [Status])` turns an all-default
/// block into `None`; `apply_board_patch` calls it after setting `statuses`, and so does
/// `apply_workflow_preset`.
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
    /// Printed in the footer as `The card expects: …`. May be empty.
    #[serde(default, skip_serializing_if = "String::is_empty")] pub expect: String,
    #[serde(default, skip_serializing_if = "ColumnAgentPrefs::is_empty")] pub agent: ColumnAgentPrefs,
    /// `KEY=VALUE`; `{key}` substituted in the value. The five `--env` refusals apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub env: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind {
    Prompt,
    Skill { name: String, #[serde(default, skip_serializing_if = "String::is_empty")] args: String },
}
impl ActionKind { pub fn word(&self) -> String /* "run card" | "run skill deep-review" */; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ColumnAgentPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub provider: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub effort: Option<String>,
    /// Permission mode is a workflow policy; it lives on the column only. Default `full_access`.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub mode: Option<PermissionMode>,
}
impl ColumnAgentPrefs { pub fn is_empty(&self) -> bool; }
```

### 1.2 Board settings and summary

```rust
pub struct BoardSettings {
    // existing fields unchanged
    /// Live runs allowed at once across the board. None = 1. Validated 1..=MAX_LIVE_RUNS_PER_BOARD.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub max_live_runs: Option<u32>,
}
impl BoardSettings { pub fn max_live_runs(&self) -> u32 /* unwrap_or(1) */; }

pub struct BoardSummary {
    // existing counts unchanged
    #[serde(default, skip_serializing_if = "is_zero")] pub working_count: u32,   // live or pending runs
    #[serde(default, skip_serializing_if = "is_zero")] pub attention_count: u32, // attention(card, now)
}
```
`pub fn is_zero(n: &u32) -> bool` lives beside `default_true` in `ops.rs`; `model.rs` imports it
the way it already imports `default_true` (model.rs:376). `ops::summarize` gains a
`live: &[LiveRun]` and `now: &str` argument to fill the two counts; phase 1 changes the signature
and every caller passes `&[]`, phase 3 passes the join.

### 1.3 Card

```rust
pub struct Card {
    // every existing field unchanged
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent: Option<CardAgentPrefs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]   pub blocked_by: Vec<CardId>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub pending_run: Option<PendingRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]   pub runs: Vec<CardRun>, // oldest first, newest last
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CardAgentPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub provider: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub effort: Option<String>,
}
impl CardAgentPrefs { pub fn is_empty(&self) -> bool; }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRun { pub status_id: StatusId, pub since: String /* RFC 3339, same clock as `now` */ }

/// Identity plus terminal facts. Written twice: when the delegation exists and when it ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardRun {
    /// The delegation id, or a freshly minted id for a run whose start failed (no delegation exists).
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
impl CardRun {
    pub fn is_live(&self) -> bool;          // outcome.is_none()
    pub fn failed_to_start(&self) -> bool;  // thread_id.is_none()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome { Succeeded, NeedsYou, Failed, Incomplete, Cancelled }
impl RunOutcome {
    pub const fn word(self) -> &'static str;      // "succeeded" "needs you" "failed" "incomplete" "cancelled"
    pub const fn needs_attention(self) -> bool;   // NeedsYou | Failed | Incomplete
}
```

Constants (`model.rs`, `pub const`): `MAX_RUNS_PER_CARD: usize = 20`;
`MAX_REPORT_COMMENTS_PER_CARD: usize = 3`; `REPORT_EXCERPT_CAP_BYTES: usize = 8 * 1024`;
`PENDING_AMBER_AFTER_SECS: u64 = 60`; `MAX_LIVE_RUNS_PER_BOARD: u32 = 8` (equals the daemon's
`MAX_LIVE_DELEGATIONS`; the daemon test asserts they agree).

### 1.4 Activity, comments, view

```rust
pub enum ActivityKind { /* existing */ RunStarted, RunEnded, AutoMoved }

pub struct Comment {
    // existing
    /// Set on a run's report excerpt; renders with a run badge instead of an author.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub run_id: Option<DelegationId>,
}

pub struct BoardView {
    pub board: Board, pub cards: Vec<Card>,
    /// Joined from the delegation store on read. Never persisted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub live_runs: Vec<LiveRun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRun {
    pub card_id: CardId,
    pub run: DelegationId,
    pub status: DelegationStatus,                                   // Starting | Running | Blocked | Settling
    #[serde(default, skip_serializing_if = "Option::is_none")] pub headline: Option<String>,
    pub started: String,
}
```

Activity messages, exact:
- `RunStarted`: `Run started · {provider} · {model} · {effort}` (drop a missing model/effort with its
  separator; provider is `AgentKind::executable()`).
- `RunEnded`: `Run ended · {outcome word} · {Nm SSs}` plus ` · ${cost:.2}` when cost is known.
- `AutoMoved`: `Moved to {column name}: unblocked by {KEY} reaching {column name}`.
- `Updated` (column loses its action): `Run canceled: {column name} no longer runs an action`.
- `Updated` (blocker deleted): `Unblocked: {KEY} was deleted`.
- `Moved` keeps today's text and is written only by a human or CLI move. An outcome move is
  `AutoMoved` with `Moved to {column name}: run succeeded`, so `attention` can tell the two apart:
  a card needs attention while its latest run `needs_attention` and no `Moved` entry is newer than
  that run's `ended_at`.

### 1.5 Drafts and patches (`ops.rs`)

```rust
pub struct CardDraft { /* existing */
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent: Option<CardAgentPrefs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]   pub blocked_by: Vec<CardId>,
}
pub struct CardPatch { /* existing */
    /// `Some(None)` clears, `Some(Some(p))` replaces, `None` leaves. Uses the crate's `nested_option`.
    #[serde(default, deserialize_with = "nested_option", skip_serializing_if = "Option::is_none")]
    pub agent: Option<Option<CardAgentPrefs>>,
    /// Whole set; `Some(vec![])` clears.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub blocked_by: Option<Vec<CardId>>,
}
```
`CardPatch::is_empty` and `apply_card_patch` learn both fields; the patch reports `"agent"` and
`"blocked_by"` in its changed-field list. `BoardPatch` is unchanged: column automation and
`max_live_runs` ride on `statuses` and `settings`.

### 1.6 Document version (`model.rs` + `crates/fleet-daemon/src/stores/board.rs`)

```rust
pub const BOARD_DOCUMENT_VERSION: u32 = 2;      // what an opted-in document writes
pub const BOARD_DOCUMENT_MIN_VERSION: u32 = 1;  // oldest the store reads
/// 2 when any column carries automation, the settings carry `max_live_runs`, or any card carries
/// links, agent prefs, a pending run, runs, or a comment with `run_id`; else 1.
pub fn document_version(board: &Board, cards: &[Card]) -> u32;
```
Store: `load`/`peek` accept `BOARD_DOCUMENT_MIN_VERSION..=BOARD_DOCUMENT_VERSION`; the refusal
sentence keeps its shape with the range: `board {id} uses document version {v} (this build reads
1..=2)`. `save` stamps `doc.version = document_version(&doc.board, &doc.cards)` before validating.
The future-version test moves to `BOARD_DOCUMENT_VERSION + 1` and a new test proves a v1 fixture
without automation still writes 1.

### 1.7 Validation (`ops/validation.rs`)

```rust
/// Called by `validate_board`. Checks every status's automation block and `settings.max_live_runs`.
pub fn validate_automation(board: &Board) -> Result<(), BoardError>;
/// Links of one card against the card set. Called by the daemon beside `validate_parent`.
pub fn validate_links(board: &Board, cards: &[Card], card: &Card) -> Result<(), BoardError>;
pub fn validate_env(env: &[String]) -> Result<(), BoardError>;   // the five `--env` refusals
```
Refusals, all `BoardError::Invalid { field, reason }`:
| field | reason |
| --- | --- |
| `on_success` / `advance_when_unblocked` | `must name a status on this board` · `may not name its own column` · `must name a later column` |
| `on_enter` | `a skill action needs a name` · `skill actions run on claude only; put the invocation in the column's instructions for codex` |
| `env` | the five sentences from `crates/fleet-cli/src/commands/subagents.rs:357-380` with `--env ` dropped |
| `max_live_runs` | `must be between 1 and 8` |
| `blocked_by` | `{KEY} is not on this board` · `a card cannot block itself` · `would close a cycle: {KEY} → {KEY} → {KEY}` |
| `automation` | `automation is available on worktree boards only` · `automation is available on local boards only` · `automation is unavailable on a worktree owned by host {host}` |

The `automation` field refusals are raised by the daemon (§3.5) on `UpdateBoard` and at entry; the
sentence is fixed here so app and CLI show the same words. Cycle text lists the path from the card
being written back to itself using display keys. `validate_env` duplicates the CLI's five rules in
`fleet-core` for column env; `crates/fleet-cli/src/commands/subagents.rs::child_environment` keeps
its own sentences (they carry the `--env ` prefix) and is not changed by this feature.

### 1.8 Derived reads (`ops/query.rs`)

```rust
pub fn blocks<'a>(cards: &'a [Card], card: &CardId) -> Vec<&'a Card>;          // reverse index
pub fn is_satisfied(board: &Board, cards: &[Card], blocker: &CardId) -> bool;  // column category Completed
pub struct Blocked { pub unsatisfied: u32, pub tone: BlockedTone }
pub enum BlockedTone { Muted, Warning }      // Warning when any blocker is canceled or archived
pub fn blocked(board: &Board, cards: &[Card], card: &Card) -> Option<Blocked>;
pub fn attention(card: &Card, now: &str) -> bool;  // last run needs_attention and no later manual Moved, or pending older than PENDING_AMBER_AFTER_SECS
pub fn latest_run(card: &Card) -> Option<&CardRun>;
```

### 1.9 Preset and templates (`defaults.rs`)

```rust
pub const PRESET_INSTRUCTIONS_IMPLEMENT: &str = "Implement this card in the current worktree. Do not commit.";
pub const PRESET_EXPECT_IMPLEMENT: &str = "make lint and make test pass";
pub const PRESET_EXPECT_REVIEW: &str = "the review finds no blocking issue";
pub const PRESET_REVIEW_SKILL: &str = "deep-review";
/// backlog, todo, ready, in-progress, in-review, done, canceled — ids, names, categories, automation.
pub fn workflow_preset() -> Vec<Status>;
/// Adds the preset columns missing by id, in preset order relative to their neighbours; never
/// touches an existing one. Returns true when it changed anything.
pub fn apply_workflow_preset(board: &mut Board) -> bool;
/// `{key}` and `{title}` substitution for instructions and env values.
pub fn render_template(text: &str, key: &str, title: &str) -> String;
```
Preset names: `Ready`, `In review` (the shipped `In Progress` keeps its name).

## 2. `fleet-core::board::automation` — the pure engine (phase 3)

New file `crates/fleet-core/src/board/automation.rs`, re-exported from `board.rs`.

```rust
pub struct LiveIndex { /* card ids with a live run */ }
impl LiveIndex { pub fn from_runs(cards: &[Card]) -> Self; pub fn contains(&self, card: &CardId) -> bool; pub fn len(&self) -> usize; }

pub struct StartRun { pub card: CardId, pub status_id: StatusId, pub action: Action }
#[derive(Default)]
pub struct Plan { pub starts: Vec<StartRun>, pub queued: Vec<CardId>, pub moved: Vec<(CardId, StatusId)> }

/// Breadth-first from `seeds` over the derived `blocks` index with a `seen` set. Mutates `cards`
/// in memory (moves, pending_run, activity) and returns the starts to apply outside the gate.
/// `in_flight` is the reservation set; the function inserts into it for every `StartRun`.
pub fn re_evaluate(
    board: &Board, cards: &mut [Card], seeds: &[CardId],
    live: &LiveIndex, in_flight: &mut BTreeSet<CardId>, now: &str,
) -> Result<Plan, BoardError>;

/// Candidate order when a slot frees: later column first, then oldest `since`.
pub fn next_pending<'a>(board: &Board, cards: &'a [Card]) -> Option<&'a Card>;
/// card → column → None (the daemon fills the provider config default). For `ActionKind::Skill`
/// the provider is always `claude`: a card's `provider: codex` is ignored, not refused, because a
/// move cannot be refused for a preference; the run row's model/effort still come from the card.
pub fn resolve_prefs(card: &Card, action: &Action) -> ResolvedPrefs;
pub struct ResolvedPrefs { pub provider: Option<AgentKind>, pub model: Option<String>, pub effort: Option<String>, pub mode: PermissionMode }
/// The brief without the footer: instructions, `# KEY — title`, description, previous reports.
pub fn brief(action: &Action, key: &str, card: &Card, reports: &[&Comment]) -> String;
```
Seeds are the cards whose column changed (entry, auto-advance, outcome move) plus every card
whose blocker changed satisfaction. Rules inside `re_evaluate`, in order for each visited card:
1. If the card's column has `on_enter`, the card has no live run and is not in `in_flight`:
   if `live.len() + in_flight.len() < max_live_runs` push `StartRun`, insert into `in_flight`,
   push activity `RunStarted`; else set `pending_run = { status_id, since: now }` and write nothing.
2. For each dependant in `blocks(cards, card)` whose column has `advance_when_unblocked`, whose
   blockers are all satisfied, and that has neither a live run nor a `pending_run`: `ops::move_card`
   it in memory to the target, push `AutoMoved`, enqueue it.
A `seen` set visits a diamond once and makes a cycle harmless.

## 3. `fleet-daemon` (phases 2 to 4)

### 3.1 `fleet-core::agents::delegation` (phase 2)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DelegationCaller { Thread(ThreadId), Card { board: BoardId, card: CardId } }
impl DelegationCaller { pub fn thread(&self) -> Option<&ThreadId>; pub fn card(&self) -> Option<(&BoardId, &CardId)>; pub fn is_card(&self) -> bool; }

pub struct Delegation {
    pub caller: DelegationCaller,   // replaces `caller: ThreadId`; a thread caller encodes byte for byte as today
    #[serde(default, skip_serializing_if = "Option::is_none")] pub caller_turn: Option<TurnId>,  // Some iff Thread
    #[serde(default, skip_serializing_if = "Option::is_none")] pub caller_item: Option<ItemId>,  // Some iff Thread
    // everything else unchanged
}
pub enum DeliveryState { Pending, Delivered { seq, turn }, Consumed, Undeliverable { reason },
    /// A card caller: the board write that recorded the outcome has committed.
    Recorded }
```
`DeliveryState::Recorded` serialises as `{"type":"recorded"}`; `word()` answers `"recorded"`;
`is_pending()` is false. The repair sweep never touches it (its predicate is `delivery = 'pending'
AND caller_kind = 'thread'`).
Goldens: `tests/agent_compatibility.rs` adds one card-caller fixture; the existing thread fixture is
not edited.

### 3.2 Store (phase 2) — `services/agents/store/migrations.rs` slot 007

Rebuild `delegations` as `delegations_v7` then rename: `caller_kind TEXT NOT NULL DEFAULT 'thread'`,
`caller_thread TEXT`, `caller_turn TEXT`, `caller_item TEXT` (all nullable), `caller_board TEXT`,
`caller_card TEXT`; every other column verbatim from slots 003 to 006. Guard: skip when
`PRAGMA table_info(delegations)` already lists `caller_kind`. Indexes re-created:
`idx_delegations_caller(caller_thread, created)` and new `idx_delegations_card(caller_board,
caller_card)`. `REQUIRED_DELEGATION_COLUMNS` and `DELEGATION_COLUMNS` grow by three,
`REQUIRED_INDEXES` by one; `insert`, `decode`, `EncodedDelegation` learn the kind. New store verbs:
`live_for_board(conn, &BoardId) -> Vec<Delegation>` and `live_for_card(conn, &BoardId, &CardId) ->
Option<Delegation>` (live = status non-terminal, a seek on `idx_delegations_card`). Constraint check: `caller_kind = 'thread'` ⇒ the
three thread columns are non-null; `'card'` ⇒ board and card non-null (enforced in `insert`, and
by the slot test). Why-comment on the slot: the first rebuild, because NOT NULL cannot be dropped.

### 3.3 Delegation service (phase 2) — `services/agents/delegation/`

```rust
pub(crate) struct CardRunRequest {
    pub board: BoardId, pub card: CardId, pub key: String /* display key, e.g. FLT-7 */,
    pub worktree: WorktreeId,
    pub provider: AgentKind, pub brief: String, pub expectation: String,
    pub mode: PermissionMode, pub model: Option<ModelSelection>, pub title: String,
    pub env: Vec<(String, String)>,
    // no `fleet_path`: a card run has no caller hint, so `resolve_fleet_program` (run.rs:61-83)
    // takes its daemon-sibling branch.
}
impl DelegationService {
    /// Phase 4: writes `result_files` on a terminal card run after `changed_since` ran. Idempotent.
    pub(crate) async fn set_result_files(&self, id: &DelegationId, files: Vec<String>) -> DaemonResult<()>;
    /// Writes `depth = 1` directly. Skips the caller-exists, caller-locality, running-turn,
    /// depth-limit and live-child-limit rules and the caller-transcript append (run.rs:331-345);
    /// keeps provider-binary, worktree-resolution and the daemon-wide limit (re-checked in
    /// `reserve`); adds a worktree-host rule: a worktree with `host.is_some()` is refused
    /// `Unsupported("automation is unavailable on a worktree owned by host {host}")`.
    pub(crate) async fn run_for_card(&self, request: CardRunRequest) -> DaemonResult<(Delegation, Option<String>)>;
    pub(crate) fn set_run_delivery_hook(&self, hook: Weak<dyn RunDeliveryHook>);
    pub(crate) async fn live_for_board(&self, board: &BoardId) -> DaemonResult<Vec<Delegation>>;
    pub(crate) async fn live_for_card(&self, board: &BoardId, card: &CardId) -> DaemonResult<Option<Delegation>>;
}
#[async_trait::async_trait]
pub(crate) trait RunDeliveryHook: Send + Sync {
    /// Called once per terminal card-called delegation. `Ok` marks the Deliver row done and the
    /// delivery `Recorded`; `Err` leaves the row open for the next drain.
    async fn on_run_delivered(&self, board: &BoardId, card: &CardId, delegation: &Delegation) -> DaemonResult<()>;
}
```
Decided worker behaviours: `repair_missing_callers` adds `AND caller_kind = 'thread'`; the drain
throttle keys on `enum ThrottleKey { Thread(ThreadId), Board(BoardId) }`; `wait` consumes only when
the delegation's caller is `Thread(t)` and `t == caller`. An unset hook makes a card delivery
`Undeliverable { reason: "no board service" }`. Child title: `↳ {KEY} — {title cut at 48}`.
`CardRunRequest.env` may not name a `FLEET_*` key (the daemon sets `FLEET_DELEGATION`,
`FLEET_DELEGATION_TOKEN`, `FLEET_SESSION`; it also sets `FLEET_CARD={KEY}` and
`FLEET_BOARD={board id}` so the child's CLI can refuse a self-move). `FLEET_CARD` and `FLEET_BOARD`
join `FLEET_OWNED_CHILD_ENV` (run.rs:36) so a column env cannot shadow them.

Footer for a card run (`footer.rs`, `CARD_FOOTER_TEMPLATE`), verbatim:
```
--- Fleet run {id} for card {key} on board {board} ---
You are running as a subagent. No human is watching this session.
The card expects: {expectation}
When the work is fully finished and verified, report it with exactly one command:
  {fleet} subagent complete --result-file <path-to-your-report.md>
Write the report first, then run the command. Do not run it before you are done.
If you are blocked and cannot finish, run:
  {fleet} subagent complete --blocked --result-file <path-with-what-you-need>
Do not ask the user questions; state assumptions in the report instead.
Do not move card {key}; its report moves it when you finish. You may comment on it and move other cards.
```
An empty `expectation` prints `The card expects: (the column names no expectation)`.

Per-peer filter: `server/connection/events.rs::event_visible` gains
`Event::DelegationChanged(d) if d.caller.is_card() => client.supports(BOARD_AUTOMATION_CAPABILITY)`
before the existing arm; `ResponseBody::Delegations` is filtered the same way where the connection
writes a response (`server/connection.rs`), and `DelegationGet` of a card-called id is answered to
any peer that named `agent.delegation` (an explicit id is not a listing).

### 3.4 Checkpoints (phase 4)

```rust
pub enum ChangeKind { Modified, Added, Deleted }      // public; distinct from the private git.rs `Change`
pub struct ChangedFile { pub path: String, pub kind: ChangeKind }
impl Checkpoints {
    /// Diff of the thread's first checkpoint tree against a fresh snapshot of the worktree.
    /// A worktree that is not a git repository, or a thread with no checkpoint, answers `Ok(vec![])`.
    pub async fn changed_since(&self, worktree: &Path, thread: &ThreadId) -> Result<Vec<ChangedFile>, CheckpointError>;
}
```
When the diff is taken: **at delivery**, inside `on_run_delivered` before the board write, so it
describes the tree the run left. The section `## Files changed since this run started` (one line
per file `{M|A|D} {path}`, and when `max_live_runs > 1` the sentence `Other runs share this
worktree; some of these changes may be theirs.`) is appended to that run's report comment; the next
run of any card on the board reads it through `brief`'s previous-reports part. Empty list ⇒ no
section. The paths go to `DelegationResult.files_changed` (a `Vec<String>`) through
`set_result_files`; the count goes to `CardRun.files_changed`. Phase 3 writes `0` and no section.

### 3.5 Boards service (phase 3) — `services/boards/automation.rs` (new) + `boards.rs`

```rust
pub(crate) struct Automation {
    delegations: DelegationService,
    checkpoints: Arc<Checkpoints>,                   // phase 4 uses it; phase 3 stores it
    in_flight: Mutex<BTreeSet<CardId>>,             // the reservation
    pending_boards: Mutex<BTreeSet<BoardId>>,       // memo: boards holding a pending_run
}
impl Boards {
    pub fn new(/* existing */, automation: Option<Automation>) -> Self;
    pub async fn start_run(&self, card: &CardId) -> DaemonResult<Card>;                     // CardRunStart; re-run allowed
    pub async fn cancel_run(&self, card: &CardId) -> DaemonResult<Card>;                    // CardRunCancel
    pub async fn wait_run(&self, card: &CardId, timeout_ms: u64) -> DaemonResult<Card>;     // CardRunWait
    pub async fn move_card(&self, card: &CardId, status: &StatusId, index: Option<usize>, cancel_run: bool) -> DaemonResult<Card>;
    pub(crate) async fn resume_automation(&self) -> DaemonResult<()>;                       // once, after the worker's first drain
    pub(crate) async fn on_slot_released(&self) -> DaemonResult<()>;                        // from the subscriber
}
#[async_trait::async_trait]
impl RunDeliveryHook for Boards { async fn on_run_delivered(&self, board, card, delegation) -> DaemonResult<()>; }
```
Composition (`composition.rs`): `Boards::new(.., automation)` where `automation` is `Some` iff
`delegation::install` returned `Some`; then `delegations.set_run_delivery_hook(Arc::downgrade(&boards))`.
Maintenance (`maintenance.rs`): after `run_delegation_outbox`, spawn `run_board_automation(services,
shutdown)` which calls `resume_automation` once and then subscribes to the bus, calling
`on_slot_released` on every terminal `DelegationChanged` while `pending_boards` is non-empty.

Trigger sites (each: acquire gate → reload → apply change → `re_evaluate` → save once → drop gate →
apply `plan.starts` via `start_for_card`): `move_card`, `update_card` with a status or `archived`
change, `create_card` with a status, `delete_card` (drops links first; seeds = former dependants),
`update` when a status's category changed or a column lost its action (clears matching
`pending_run`s with the `Updated` sentence). The reconcile path never calls it.

`start_for_card(board, card)`: resolve prefs (card → column → provider config default for the
model; mode from the column; `full_access` default), assemble `brief` (previous reports already
carry any files section) + footer, call `run_for_card`; on `Ok` re-acquire the gate, push `CardRun` (cap `MAX_RUNS_PER_CARD`,
drop oldest), clear `pending_run`, remove from `in_flight`, save; on `Err` re-acquire, push a
`CardRun { id: DelegationId::new(), thread_id: None, outcome: Some(Failed), ended_at: Some(now),
detail: Some(sentence), .. }`, clear, remove, save, and emit `BoardChanged { CardChanged }`. The app
raises the sticky error when a view arrives carrying a run id it did not hold before whose
`failed_to_start()` is true (§5.5). `card attach` on such a run answers `{KEY}'s last run never
started`.

`on_run_delivered`: under the gate write `ended_at`, `outcome` (table in the design doc:
Succeeded→Succeeded; Failed with payload `reported blocked`→NeedsYou; Failed→Failed;
Incomplete→Incomplete; Cancelled→Cancelled), `detail` = `status_payload`, `files_changed`,
`cost_usd`/`tokens` from `delegations.get(id)` (usage read path); add the report comment (`run_id`
set, body capped at `REPORT_EXCERPT_CAP_BYTES` with `…` and the trailing line `(report elided; the
full report is in the run's thread)`), drop the oldest report comment past
`MAX_REPORT_COMMENTS_PER_CARD` (clearing that run's `report_comment_id`); if Succeeded and
`on_success` is set on the *current* board, `ops::move_card` to it with the outcome `Moved`
sentence; `re_evaluate` with the card as seed; save once; drop; apply starts. Idempotent: a run
already terminal on the card returns `Ok` and writes nothing.

Refusals (daemon):
| Situation | Error | Sentence |
| --- | --- | --- |
| move out of a column with a live run without `cancel_run` | `Conflict` | `{KEY} is working; pass --cancel-run to move it` |
| delete or archive a card with a live run | `Conflict` | `{KEY} is working; cancel the run first` |
| remove a column with live runs | `Conflict` | `column has {n} live runs; cancel them first` |
| `start_run` on a column with no action | `Validation` | `{column name} has no action` |
| `cancel_run` with no live run | `NotFound` | `{KEY} has no live run` |
| automation on a context / Jira / hosted board | `Validation` | §1.7 `automation` sentences |
| no delegation service | `Unsupported` | `the native-agent database is unavailable, so board automation is refused` |

### 3.6 Wire (phase 3) — `fleet-proto`

```rust
pub const BOARD_AUTOMATION_CAPABILITY: &str = "board.automation";  // response.rs, beside BOARD_WORKTREE_CAPABILITY; defined in phase 1, advertised in phase 3
// RequestBody
MoveCard { card_id, status_id, index, #[serde(default, skip_serializing_if = "std::ops::Not::not")] cancel_run: bool }
CardRunStart { card_id: CardId }                   → ResponseBody::Card(Card)
CardRunCancel { card_id: CardId }                  → ResponseBody::Card(Card)
CardRunWait { card_id: CardId, timeout_ms: u64 }   → ResponseBody::Card(Card)   // the CLI reads `runs.last()`
```
The three run requests classify like `MoveCard`: `host_or_local(resolver.host_of_card(card_id))` (`router/classify.rs`, ADR 0021), and `MoveCard.cancel_run` rides through the existing arm. Client (`api/boards.rs`):
`card_run_start(CardId) -> Result<Card>`, `card_run_cancel(CardId) -> Result<Card>`,
`card_run_wait(CardId, timeout_ms: u64) -> Result<Card>`, `move_card(.., cancel_run: bool)`;
`required_capability` maps the three run requests to `BOARD_AUTOMATION_CAPABILITY` with the error
`this daemon does not support board automation; run `fleet daemon restart``; `request_timeout`:
`CardRunStart => AGENT_HARNESS_TIMEOUT`, `CardRunCancel => None`, `CardRunWait { timeout_ms } =>
timeout_ms + 15 s`. Goldens in `tests/compatibility.rs`: phase 1 adds a `Card` with every new
field and proves the legacy `Card` fixture unchanged; phase 3 adds the three requests, a `MoveCard`
with `cancel_run: true`, and a `BoardView` with one `live_runs` entry.

## 4. CLI (phase 5) — `crates/fleet-cli`

Flags and verbs exactly as the design doc's CLI blocks; additions fixed here:
- `--effort` is free text; `--provider` is `claude|codex`; `--clear-agent` conflicts with the three.
- `--blocks KEY` writes `KEY.blocked_by += this` after the card is created and prints both cards.
- `--desc-file PATH` reads the file (reuse `read_text`); conflicts with `--desc`.
- `card move <key> <status> [--index N] [--cancel-run]`; without the flag the daemon's `Conflict`
  sentence is printed verbatim.
- `card run|cancel|attach|wait <key>`; `card runs <key>` prints one tab-separated line per run:
  `id  column  state  provider  model  effort  duration  tokens  cost  thread` (`—` for unknown).
  `wait --timeout S` default 540; exit 0 when the newest run is terminal, 2 when still live or none
  started within the timeout.
- `card attach <key>` prints the live run's thread id, or the newest run's when none is live;
  refuses `{KEY} has no run` when the card has none.
- Self-move refusal (client-side, before any request): when `FLEET_DELEGATION` is set and the raw
  `<key>` argument equals `FLEET_CARD` case-insensitively: `a run cannot move its own card; its
  report moves the card when it finishes` (exit 1). A card id or a key the run typed differently
  is not caught; the refusal is advisory.
- Human output: `card run` prints `run {id} started, thread {thread}` then the card (like every
  mutating verb); `card cancel` prints the card; `card attach` prints the thread id alone.
- `fleet subagent list` and `status` gain a ninth field `caller` at the end of the line: `thread
  {id}` or `card {KEY}`; `docs/NATIVE-AGENTS.md` §15.4's list line moves with it.
- `columns` family sends one `UpdateBoard { patch: BoardPatch { statuses: Some(all) } }`;
  `remove --move-cards-to ID` sends one `MoveCard` per card first. `columns` with no verb prints
  `id  name  category  on enter  on success  when unblocked`. `--on-enter none|prompt|skill:<name>[:<args>]`.
- `board set --max-live-runs N` rides on `BoardPatch.settings`.
- `board show` marks: before the priority word, `● working` for a live run, `! needs you` when
  `attention`, `… pending` for a pending run; after the title ` ⊘ n` when blocked; column header
  `Name (n) ⚡` when the column has an action; header line gains ` · 1/1 working` and ` · 1 needs you`
  while non-zero. `board list` adds `WORKING` and `NEEDS YOU` columns after `CONFLICTS`.
- `card show` adds sections `Agent` (resolved provider/model/effort with `(column default)`),
  `Blocked by`, `Blocks`, `Runs` (same line as `card runs`).
- JSON: `BoardEnvelope` gains `liveRuns`; every `columns` verb with `--json` prints the
  `BoardEnvelope` (the columns are `board.statuses`), no new envelope; `card runs --json` prints
  `BoardCardEnvelope` (the runs are on the card).
- Skills: `.claude/skills/fleet-board-planning` gains "Building a chain" (create in Todo, link,
  move to Ready, `card wait`, never move a card with a live run) and the new verbs in
  `references/commands.md`; `.claude/skills/fleet-subagent-cli` gains the `FLEET_CARD`/`FLEET_BOARD`
  variables and the self-move rule.

## 5. App and ui-kit (phases 6 to 9)

### 5.1 ui-kit (`crates/fleet-ui-kit`)
```rust
pub enum RunMark { Pending, Stalled, Working, NeedsYou, Succeeded }   // components/card_tile.rs; no domain type
pub enum BlockedTone { Muted, Warning }
impl CardTile { pub fn run(self, mark: RunMark) -> Self; pub fn blocked(self, count: u32, tone: BlockedTone) -> Self; }
impl KanbanColumn { pub fn action(self, has_action: bool) -> Self; }   // ⚡ Icon::Zap muted after the count
```
Rendering: `Pending`/`Working` = `Spinner` small `Tone::Secondary`; `Stalled` (a pending run older
than `PENDING_AMBER_AFTER_SECS`) and `NeedsYou` = `StatusDot::small(Tone::Warning)`; `Succeeded` =
`Icon::Check` faint. Harness mark words: `pending`, `stalled`, `working`, `needs you`, `done`. The key line becomes a row: key text, flex spacer, mark or
`⊘ n` (`Text::data_small`, `.muted()` or `Tone::Warning`). Gallery: `card_tile_section` gains one
tile per mark and one blocked tile; `live_board_section` gains a `⚡` column.

### 5.2 App state (`crates/fleet-app/src/state/board.rs`, phase 6)
```rust
pub struct CardMarks { pub by_card: HashMap<CardId, TileMarks>, pub working: u32, pub needs_you: u32, pub revision: u64 }
pub struct TileMarks { pub run: Option<RunMark>, pub blocked: Option<(u32, BlockedTone)> }
impl AppState {
    /// Recomputed after `apply_board_view`, `apply_card`, after `apply_delegation` for a
    /// card-called delegation on the shown board, and from the app's existing clock tick (the one
    /// that runs `expire_toasts`, notifications.rs:43) so `Stalled` appears without an event.
    /// Formats `now` to RFC 3339 once for `ops::query::attention`. Bumps `revision` only when the
    /// map differs.
    pub fn refresh_card_marks(&mut self, now: DateTime<Utc>);
}
```
`screens/board/projection.rs::ProjectionKey` gains `marks: u64` (= `CardMarks.revision`). The
failed-start sticky error: `apply_board_view` keeps the previous view's newest run id per card
while it swaps views; a card whose newest run id is new and `failed_to_start()` raises
`StickyError { text: detail, job: None, retryable: false }` through `screens::board::lifecycle::fail`. The
pane header trailing slot gets `1/1 working` (`Tone::Secondary`) and `1 needs you`
(`Tone::Warning`) when non-zero; `working` is `live + pending`, the denominator is
`max_live_runs()`.

### 5.3 Card detail (phase 7)
`PickerKind::{BlockedBy, Blocks, Provider, Model, Effort}`. `BlockedBy` and `Blocks` are
multi-select over every non-archived card except this one; a row that would close a cycle is
disabled with detail `would cycle` (`PickerOption` gains `disabled: bool`, mapped onto the kit's
`FuzzyItem::disabled`). Applying `BlockedBy` sends one `UpdateCard { blocked_by }` for this card;
applying `Blocks` sends one `UpdateCard { blocked_by }` per dependant whose membership changed, in
key order, stopping at the first refusal and showing its sentence on the dialog's error line (the
cards already patched stay patched). `Provider` rows: `column default`, `claude`, `codex`;
`Model`/`Effort` rows: `column default`, then the same list source the agent composer's model
picker uses (confirm with `grep -rn "SetModel" crates/fleet-app/src/screens/agent_thread`), then
the typed query as a free row. Each of the three rows reads the card's whole `agent`, replaces its
one field and sends the whole `CardPatch.agent` (last writer wins, as labels do today). Property
rows `Provider`/`Model`/`Effort` appear only while the column has an action; `Blocked by` and
`Blocks` appear when non-empty or the board has any link. Run row between title and description
when `latest_run` or `pending_run` exists: `{mark} {state word} {elapsed} · {provider} · {model} ·
{effort}` and, when terminal, ` · {tokens} tok · ${cost}`. The run row's hint line `A attach · X
cancel · > re-run` is drawn by phase 9 with the keys, never before. Report comments render a
`run {n}` badge and collapse at `DELEGATION_RESULT_COLLAPSE_LINES` (reuse the app constant) with
`⏎ expand`.

### 5.4 Board settings (phase 8)
`Dialogs::BoardSettings` width `WIDE_W`, height `SETTINGS_H`. `BoardSection { General, Backend, Columns }`
with `Section::ALL`-style iteration; the Columns pane is a list of columns (`⚡` when automated,
`n new · d delete · J/K reorder · P preset · ⏎ open`) drilling into one column's rows: Name,
Category, On enter, then (when not none) Provider, Model, Effort, Mode, Instructions, Expect, Env,
then On success, When unblocked. Keys: `⏎` drills into a column from the list and opens a row's
editor inside a column (the existing `BoardSettingsEditing` `enter` commits an edit); `ctrl-s`
saves one `UpdateBoard` and the primary button reads `^s Save`; `esc` goes back a level, and at
the top level with unsaved edits asks once. `,` opens the dialog on the section last used in this
app session (General on first open); `C` opens it on Columns. On a context or Jira board the
automation rows are disabled with the trailer `Automation is available on worktree boards`.
Deleting a column with cards requires a target and moves the cards first, one `MoveCard` each in
column order; a refusal mid-sequence stops there, keeps the column, shows the daemon's sentence on
the error line, and leaves the cards already moved where they are (each move was recorded as a
`Moved` entry). General gains `Max live runs` with the hint `runs share one checkout`.

### 5.5 Keys, actions, palette, confirm (phase 9)
| Key | Action | Contexts | Palette label |
| --- | --- | --- | --- |
| `A` | `board::AttachRun` | `Workspace > Native > Board`, `Dialog > CardDetail` | `Board: Attach run` |
| `X` | `board::CancelRun` | same | `Board: Cancel run` |
| `>` | `board::RunNow` | same | `Board: Run now` |
| `b` | `board::PickBlockedBy` | `Hub > Board`, `Workspace > Native > Board`, `Dialog > CardDetail` | `Board: Blocked by` |
| `m` | `board::PickAgent` | same three | `Board: Agent` |
| `C` | `board::Columns` | `Hub > Board`, `Workspace > Native > Board` | `Board: Columns` |
`A` on a card with no run toasts `{KEY} has no run`; `X` on a card whose marks show no live run
toasts `{KEY} has no live run` (the daemon's `NotFound` sentence, so both surfaces agree); `>`
outside an action column toasts `{column name} has no action`. Palette validity: `Board: Attach
run`, `Cancel run` and `Run now` are valid when `board_pane_is_active()` or when `Dialog >
CardDetail` is open over a `BoardScope::Worktree` board; never over the Hub's context board.
`ConfirmRequest::MoveCancelsRun { card: CardId, key: String, target: String, elapsed: String }`
with title `Move {KEY}?` and consequence `{KEY} is working ({elapsed}). Move to {target} and cancel
the run?`; the app asks only when its delegation mirror holds a live card-called delegation for
`latest_run.id` (elapsed from `Delegation::elapsed`); otherwise it sends the plain move and a
daemon `Conflict` is reported through `lifecycle::fail`. Confirming sends `MoveCard { cancel_run:
true }`. The failed-start sticky error is defined in §5.2.

### 5.6 Harness (additive, `docs/TESTING-HARNESS.md` §3 and §4)
`board.cards` rows: `marks` gains one of `working`, `pending`, `stalled`, `needs you`, `done`, and
`blocked:n`; `board` (column) rows: `marks` gains `action`. New list `board.summary` with one row
whose label is the header's `1/1 working · 1 needs you` text (absent when both are zero). Card
detail `dialog` gains list `card.runs` (one row per run, label = the run row text). Board settings
`dialog` gains a `fields` entry named `section` (value `General`/`Backend`/`Columns`) and list
`settings.columns`; no new `DialogSnapshot` key.

**Fixture preset `board-workflow`** (phase 6 adds it; the `fixtures` row of §10 owns it). This is
the third additive preset after `agents-subagent` and `agents-subagent-other-worktree`
(commit 0b76217 added those to the §2 `fixture:` enumeration and described them under §4 as
additive), so it joins that enumeration the same way and is the only §2 line this feature touches.
It seeds what `agents-subagent` seeds plus: a worktree board on `acme/api#agent` with the workflow
preset applied and `max_live_runs = 1`; four cards in Todo whose keys phase 6 confirms from the
board's prefix — card 1 blocks card 3, cards 1 and 2 block card 4; Codex serves
`subagent-child.json` (completes) and Claude serves `subagent-child-blocked.json` (reports
blocked) as child transcripts. A scenario reaches `working`/`pending` by moving two cards into
`In progress` and taking its shot or snapshot before the child transcript ends; `needs you` by
running a card whose provider is Claude; `blocked:n` from the links. Whether the scripted
provider can hold a turn open long enough for a `shot` is phase 6's first question; if it cannot,
the shot covers the states the transcript can hold and the tracker records the rest.

## 6. Where the tests live

| What | Where |
| --- | --- |
| model round-trips, `document_version`, validation sentences, preset, templates | `crates/fleet-core/src/board/tests.rs`, `ops/tests/validation.rs`, `ops/tests/automation.rs` (new) |
| `re_evaluate` table tests (diamond, throttle order, cycle harmless, column loses action) | `crates/fleet-core/src/board/automation/tests.rs` (new) |
| store version range | `crates/fleet-daemon/src/stores/board.rs` `mod tests` |
| wire goldens | `crates/fleet-proto/tests/compatibility.rs`, `tests/agent_compatibility.rs` |
| slot 007, column goldens | `crates/fleet-daemon/src/services/agents/store/migrations.rs` tests, `schema.rs` |
| card-called delegation lifecycle, hook, filter, throttle, wait | `crates/fleet-daemon/src/services/agents/delegation/tests/card.rs` (new) |
| engine end to end (diamond, restart adoption, refusals) | `crates/fleet-daemon/tests/boards_automation.rs` (new; private `fleetd` home + scripted provider) |
| `changed_since` | `crates/fleet-daemon/src/services/checkpoints/tests.rs` |
| CLI parsing, rendering, request shapes | `crates/fleet-cli/src/commands/board/tests.rs`, `columns/tests.rs` (new) |
| chain script | `scripts/board-workflow-smoke.sh` (new), invoked by `make smoke-workflow` |
| marks memo, run-mark map | `crates/fleet-app/src/state/board/tests.rs`, `screens/board/tests.rs` |
| GUI | `scenarios/board/workflow-*.scenario` |
