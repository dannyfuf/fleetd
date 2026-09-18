# Native subagents — Contracts (authoritative for the build)

> This file fixes every shared type, signature, constant, text and file boundary the seven phase
> plans leave open, so parallel agents can code against each other's work before it exists.
> Roadmap: ./native-subagents-2026-09-17-roadmap.md. Phase plans: ./native-subagents-2026-09-17-phase-N-plan.md.
> When a phase plan and this file disagree on a *shape*, this file wins. When they disagree on a
> *behaviour*, the phase plan wins. Anything not fixed here follows the nearest existing pattern
> in the crate you are editing.

## 0. Rules every agent follows

- Repo: `/home/df/.fleet/worktrees/dannyfuf/fleetd/feat-agnostic-subagents`. Rust 2024, toolchain pinned.
- Read, in order: this file, then the phase plan section for your task, then the files you own.
- **Own only the files your stage lists.** Anything you need elsewhere goes under `INTEGRATION NOTES`
  in your report (exact file, exact code). Never reformat, reorder or "tidy" a file you do not own.
- **No git operations.** No commits, branches, stashes, resets. The orchestrator commits.
- **No new dependencies** and no `Cargo.lock` changes unless your stage says so explicitly.
- `cargo fmt` only on files you own: `rustfmt --edition 2024 <file>...`. Never `cargo fmt --all`
  while other agents are editing.
- Verify with the narrowest command that proves your work (`cargo check -p <crate> --all-targets`,
  `cargo test -p <crate> <filter>`, `cargo clippy -p <crate> --all-targets --all-features -- -D warnings`).
  Other agents edit the same tree concurrently: a compile error **in a file you do not own** is not
  yours to fix. Wait 60 s and retry up to 5 times; if it persists, finish your own files against the
  contract, note it under `DEVIATIONS`, and report. Never "fix" another agent's file to make your
  check pass.
- The cargo build directory is shared and locked; "Blocking waiting for file lock" is normal. Do
  not delete `target/`.
- Repo non-negotiables (CLAUDE.md): no `unwrap`/`todo!`/`unimplemented!`/`dbg!`/`TODO` in production
  code; `expect` only for static invariants with a message; never `let _ =` a fallible call; render
  prepares nothing; kit components take no domain types and no literal colours/sizes; every spawned
  fallible task ends in `.detach_and_log_err(cx)` or is stored; docs are authoritative and ride with code.
- Report format, ≤ 40 lines, last thing you print:

```
STATUS: GREEN|RED|BLOCKED — one line why
FILES: <every file created or modified, one per line>
TESTS: <commands run and their result>
DEVIATIONS: <where you departed from the contract or plan, and why; "none" otherwise>
INTEGRATION NOTES: <exact edits another owner must make, with file and code; "none" otherwise>
```

## 1. `fleet-core` types (owner: stage `contracts-core`)

Serde conventions, copied from the crate: structs `#[serde(rename_all = "camelCase")]`; enums with
payload `#[serde(tag = "type", content = "data", rename_all = "snake_case")]`; unit-only enums
`#[serde(rename_all = "snake_case")]`; every new `Option` field `#[serde(default, skip_serializing_if = "Option::is_none")]`;
every new `bool` `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`; every new `Vec`
`#[serde(default, skip_serializing_if = "Vec::is_empty")]`. A new field without these breaks a golden.

### 1.1 `crates/fleet-core/src/agents/ids.rs`
`uuid_id!(DelegationId, ...)` invoked exactly as `ThreadId` is, directly after it.

### 1.2 New `crates/fleet-core/src/agents/delegation.rs`, re-exported from `agents/mod.rs`

```rust
/// The durable link between a caller thread and the child it spawned. The token is never on this
/// type: the daemon stores its SHA-256 and only `RequestBody::DelegationComplete` carries plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delegation {
    pub id: DelegationId,
    pub caller: ThreadId,
    pub caller_turn: TurnId,
    pub caller_item: ItemId,
    pub child: ThreadId,
    pub provider: AgentKind,
    pub depth: u8,
    pub brief: String,
    pub expectation: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub eager: bool,
    pub status: DelegationStatus,
    /// Why the status is what it is, when a word is not enough (`"reported blocked"`,
    /// `"provider exited twice"`, `"harness unavailable: ..."`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_payload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<DelegationResult>,
    #[serde(default)]
    pub nudges: u8,
    #[serde(default)]
    pub recoveries: u8,
    pub delivery: DeliveryState,
    pub created: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatus { Starting, Running, Blocked, Settling, Succeeded, Incomplete, Failed, Cancelled }
impl DelegationStatus {
    pub const fn is_terminal(self) -> bool  // Succeeded | Incomplete | Failed | Cancelled
    pub const fn is_live(self) -> bool      // !is_terminal
    pub const fn word(self) -> &'static str // "starting" "working" "blocked" "settling" "done" "incomplete" "failed" "cancelled"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_changed: Vec<String>,
    pub source: ResultSource,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub elided: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSource { Reported, LastAssistantText }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Delivered { seq: Seq, turn: TurnId },
    Undeliverable { reason: String },
}
impl DeliveryState { pub const fn is_pending(&self) -> bool; pub const fn word(&self) -> &'static str /* "pending" "delivered" "undeliverable" */ }
```

`Delegation::elapsed(&self, now: DateTime<Utc>) -> chrono::Duration` (created → finished or now).
Round-trip tests for every enum variant in `delegation.rs` `mod tests`, using the crate's existing
serde test pattern.

### 1.3 `crates/fleet-core/src/agents/state.rs`
```rust
/// Why a thread stopped, kept on the record so delivery can tell a user's Stop from a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopCause { User, ProviderExit }
```
Re-exported beside `SessionState`.

### 1.4 `crates/fleet-core/src/agents/items.rs`
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum MessageOrigin { #[default] User, Delegation { id: DelegationId } }
impl MessageOrigin { pub const fn is_user(&self) -> bool; pub fn delegation(&self) -> Option<DelegationId> }
```
- `ItemKind::UserMessage` gains `#[serde(default, skip_serializing_if = "MessageOrigin::is_user")] origin: MessageOrigin` after `steered`.
- New variant, last in the enum:
  `ItemKind::Delegation { id: DelegationId, provider: AgentKind, child: ThreadId, status: DelegationStatus }`.
- `ItemPayloadPatch::Delegation { status: DelegationStatus }`, last in that enum (the crate's
  `ItemPatch` is a struct wrapping `ItemPayloadPatch`; wherever this file says `ItemPatch::Delegation`
  read `ItemPatch { payload: ItemPayloadPatch::Delegation { status } , .. }` in the crate's shape).
- `crates/fleet-core/src/agents/provider.rs` `UserInput` gains the same `origin` field after `item`.

### 1.5 Records and summaries
- `crates/fleet-daemon/src/services/agents/record.rs` `AgentThreadRecord` gains, defaulted and skipped when `None`:
  `parent: Option<ThreadId>`, `delegation: Option<DelegationId>`, `stop_cause: Option<StopCause>`.
- `crates/fleet-core/src/agents/projection/summary.rs` `AgentThreadSummary` gains `parent: Option<ThreadId>`
  (defaulted, skipped when `None`). `ThreadProjection` gains `pub parent: Option<ThreadId>` (not serialized
  if the projection is not serialized; if it is, same attributes) and `summary()` copies it. The manager
  sets `projection.parent` from the record at create and hydrate.

### 1.6 Daemon-internal
`crates/fleet-daemon/src/agents/harness/mod.rs`:
```rust
/// What the harness did with a submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submitted {
    /// Folded into the turn that was already running (a steer).
    JoinedActive { turn: TurnId },
    /// Accepted as its own turn: running now, or queued behind the current one.
    QueuedNew { turn: TurnId },
}
impl Submitted { pub const fn turn(self) -> TurnId; pub const fn joined_active(self) -> bool }
```

## 2. `fleet-proto` and `fleet-client` (owner: stage `contracts-core`; goldens: stage `proto-goldens`)

`crates/fleet-proto/src/lib.rs`:
```rust
/// Gates the six `Delegation*` requests, their responses and `Event::DelegationChanged`.
pub const AGENT_DELEGATION_CAPABILITY: &str = "agent.delegation";
```
Not added to `AGENT_CAPABILITIES` until stage `service-queries` (phase 3). Leave a comment saying so.

`crates/fleet-proto/src/request.rs`, variants added at the end of the agent family, attributes copied
from `AgentThreadCreate`:
```rust
DelegationRun {
    caller: ThreadId,
    provider: AgentKind,
    brief: String,
    expectation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] worktree: Option<WorktreeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")] mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")] model: Option<ModelSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")] title: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")] eager: bool,
},
DelegationComplete { delegation: DelegationId, child: ThreadId, token: String, result: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")] blocked: bool },
DelegationList { #[serde(default, skip_serializing_if = "Option::is_none")] caller: Option<ThreadId> },
DelegationGet { delegation: DelegationId },
DelegationCancel { delegation: DelegationId },
DelegationWait { delegation: DelegationId, timeout_ms: u64 },
```
`agent_request_is_serialized` (request.rs ~955): `DelegationRun` keyed by `caller`, `DelegationComplete`
and `DelegationCancel` keyed by the child / delegation thread as that function's shape allows (if it keys
by `ThreadId` and `DelegationCancel` has none, key `DelegationCancel` as *not* serialized and say so in a
comment; the service serializes it itself).

`response.rs`:
```rust
DelegationStarted { delegation: Delegation, #[serde(default, skip_serializing_if = "Option::is_none")] warning: Option<String> },
Delegations(Vec<Delegation>),
Delegation(Delegation),
```
`event.rs`: `Event::DelegationChanged(Delegation)`, documented as part of the `AgentSummary` subscription
family and gated at the emission site on `AGENT_DELEGATION_CAPABILITY` exactly as `agent.account` gates its
event (find the emission-site gating for account and copy it).

`crates/fleet-client/src/connection.rs` `request_timeout`: `DelegationWait { timeout_ms, .. }` →
`Duration::from_millis(*timeout_ms) + Duration::from_secs(15)`; `DelegationRun { .. }` → `AGENT_HARNESS_TIMEOUT`.

`crates/fleet-client` typed methods on `Client`, following `agent_thread_create`'s shape and error mapping:
```rust
pub async fn delegation_run(&self, request: DelegationRunRequest) -> Result<(Delegation, Option<String>), ProtoError>
pub async fn delegation_complete(&self, delegation: DelegationId, child: ThreadId, token: String, result: String, blocked: bool) -> Result<Delegation, ProtoError>
pub async fn delegation_list(&self, caller: Option<ThreadId>) -> Result<Vec<Delegation>, ProtoError>
pub async fn delegation_get(&self, delegation: DelegationId) -> Result<Delegation, ProtoError>
pub async fn delegation_cancel(&self, delegation: DelegationId) -> Result<Delegation, ProtoError>
pub async fn delegation_wait(&self, delegation: DelegationId, timeout_ms: u64) -> Result<Delegation, ProtoError>
```
where `DelegationRunRequest` is a plain pub struct in `fleet-client` mirroring the request fields.
`DelegationCancel` answers `ResponseBody::Delegation` (the record after cancel); `DelegationWait` answers
`ResponseBody::Delegation` (terminal or current at timeout).

Goldens (`crates/fleet-proto/tests/agent_compatibility.rs`, stage `proto-goldens`): one per new request,
response and event; a `SeqEvent` carrying `ItemStarted` with `ItemKind::Delegation` and one carrying
`ItemUpdated` with `ItemPatch::Delegation`; a `UserMessage` with `origin: Delegation` and one proving
`origin: User` serializes to no `origin` key; a legacy fixture proving a pre-phase payload without `origin`
or `parent` decodes. Every existing golden byte-identical. `PROTOCOL_VERSION` stays 7.

## 3. Store (owner: stage `migration-store`; transition SQL: stage `store-transition`)

### 3.1 Migration 3 (`store/migrations.rs`), slot 3, name `delegations`, idempotent like m002
```sql
CREATE TABLE delegations (
  id TEXT PRIMARY KEY, token_sha256 TEXT NOT NULL,
  caller_thread TEXT NOT NULL, caller_turn TEXT NOT NULL, caller_item TEXT NOT NULL,
  child_thread TEXT NOT NULL UNIQUE, provider TEXT NOT NULL, depth INTEGER NOT NULL,
  brief TEXT NOT NULL, expectation TEXT NOT NULL, eager INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL, status_payload TEXT, result TEXT, result_source TEXT,
  result_files TEXT, result_elided INTEGER NOT NULL DEFAULT 0,
  nudges INTEGER NOT NULL DEFAULT 0, recoveries INTEGER NOT NULL DEFAULT 0,
  delivery TEXT NOT NULL, delivered_seq INTEGER, delivered_turn TEXT, delivery_reason TEXT,
  headline TEXT,
  reported_at TEXT, report_sha256 TEXT,
  created TEXT NOT NULL, finished TEXT
);
CREATE INDEX idx_delegations_caller ON delegations(caller_thread, created);
CREATE TABLE delegation_outbox (
  id INTEGER PRIMARY KEY, delegation TEXT NOT NULL,
  action TEXT NOT NULL,  -- deliver | nudge | settle | recover | cancel_children | mirror
  created TEXT NOT NULL, done TEXT
);
CREATE INDEX idx_delegation_outbox_open ON delegation_outbox(id) WHERE done IS NULL;
ALTER TABLE threads ADD COLUMN parent_thread_id TEXT;
ALTER TABLE threads ADD COLUMN delegation_id TEXT;
ALTER TABLE threads ADD COLUMN stop_cause TEXT;
```
`status` and `delivery` store the snake_case words; `result_files` is a JSON array; timestamps RFC 3339
like the rest of the schema (copy `created_at`'s encoding). `reported_at` and `report_sha256` are
store-only (never on the wire): the time the first `complete` was accepted and the SHA-256 of the
full, untruncated report, so a repeated `complete` can be judged byte-identical after truncation and a
different one refused naming the first report's time. Slot 3 has not shipped, so amending its SQL (and
its recorded sha256) is allowed until the branch merges. `threads` insert/list/read statements read and
write the three new columns; `stop_cause` is written from the record on every metadata write.
`rebuild_thread` and `quarantine_after` never touch `delegations` (comment says why).

### 3.2 New `store/delegations.rs` — the only statements that touch the two tables
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboxAction { Deliver, Nudge, Settle, Recover, CancelChildren, Mirror }
impl OutboxAction { pub(crate) const fn as_str(self) -> &'static str; pub(crate) fn parse(s: &str) -> anyhow::Result<Self> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutboxRow { pub id: i64, pub delegation: DelegationId, pub action: OutboxAction, pub created: DateTime<Utc> }

// Writes take a transaction so the transition half can run inside `project_event`'s.
pub(super) fn insert(tx: &Transaction, delegation: &Delegation, token_sha256: &str) -> anyhow::Result<()>;
pub(super) fn update(tx: &Transaction, delegation: &Delegation) -> anyhow::Result<()>;   // every mutable column
pub(super) fn get(conn: &Connection, id: DelegationId) -> anyhow::Result<Option<Delegation>>;
pub(super) fn get_by_child(conn: &Connection, child: ThreadId) -> anyhow::Result<Option<Delegation>>;
pub(super) fn token_hash(conn: &Connection, id: DelegationId) -> anyhow::Result<Option<String>>;
pub(super) fn list(conn: &Connection, caller: Option<ThreadId>) -> anyhow::Result<Vec<Delegation>>; // newest first, LIMIT 1000
pub(super) fn live(conn: &Connection, caller: Option<ThreadId>) -> anyhow::Result<Vec<Delegation>>; // non-terminal
pub(super) fn enqueue(tx: &Transaction, delegation: DelegationId, action: OutboxAction, now: DateTime<Utc>) -> anyhow::Result<i64>;
pub(super) fn open_rows(conn: &Connection) -> anyhow::Result<Vec<OutboxRow>>;                    // done IS NULL, id ASC, LIMIT 1000
pub(super) fn open_rows_for(conn: &Connection, delegation: DelegationId) -> anyhow::Result<Vec<OutboxRow>>;
pub(super) fn mark_done(tx: &Transaction, row: i64, now: DateTime<Utc>) -> anyhow::Result<()>;
pub(super) fn mark_done_for(tx: &Transaction, delegation: DelegationId, action: OutboxAction, now: DateTime<Utc>) -> anyhow::Result<usize>;
pub(super) fn caller_exists(conn: &Connection, caller: ThreadId) -> anyhow::Result<bool>;        // threads row, not deleted
pub(crate) fn set_report(tx: &Transaction, id: DelegationId, result: &DelegationResult, report_sha256: &str, now: DateTime<Utc>) -> anyhow::Result<()>;
pub(crate) fn report_meta(conn: &Connection, id: DelegationId) -> anyhow::Result<Option<(String, DateTime<Utc>)>>; // (report_sha256, reported_at)
```
`Transaction` and `Connection` are `rusqlite`'s. The file imports nothing from `services::agents::manager`.
**Visibility (decided 2026-09-18):** the module is declared `pub(crate) mod delegations;` in `store/mod.rs`
and every function above is `pub(crate)`, so the service's `delegation_write` closures call
`store::delegations::{insert, update, get, token_hash, enqueue, mark_done, mark_done_for, set_report, report_meta, ...}`
directly. That is the only sanctioned way for the service to write inside a transaction; no SQL lives outside
`store/delegations.rs`.

### 3.3 `SqliteAgentStore` additions (`store/mod.rs`, bodies in `writer.rs`/`delegations.rs`)
```rust
pub(crate) async fn delegation(&self, id: DelegationId) -> anyhow::Result<Option<Delegation>>;
pub(crate) async fn delegation_by_child(&self, child: ThreadId) -> anyhow::Result<Option<Delegation>>;
pub(crate) async fn delegation_token_hash(&self, id: DelegationId) -> anyhow::Result<Option<String>>;
pub(crate) async fn delegations(&self, caller: Option<ThreadId>) -> anyhow::Result<Vec<Delegation>>;
pub(crate) async fn live_delegations(&self, caller: Option<ThreadId>) -> anyhow::Result<Vec<Delegation>>;
pub(crate) async fn delegation_outbox(&self) -> anyhow::Result<Vec<OutboxRow>>;
/// One closure, one transaction, on the writer thread. The closure returns `(value, wake)`; when
/// `wake` is true the writer pokes the wake channel after COMMIT.
pub(crate) async fn delegation_write<T, F>(&self, what: &'static str, f: F) -> anyhow::Result<T>
where F: FnOnce(&Transaction) -> anyhow::Result<(T, bool)> + Send + 'static, T: Send + 'static;
/// Installed once by composition. The writer thread calls these after COMMIT, never inside it.
pub(crate) fn install_delegation_hooks(&self, hooks: DelegationHooks);
pub(crate) struct DelegationHooks {
    pub wake: tokio::sync::mpsc::UnboundedSender<()>,
    pub changed: Arc<dyn Fn(Delegation) + Send + Sync>,   // publishes Event::DelegationChanged
}
/// `append` with the facts the transition half needs about the thread the event belongs to.
pub(crate) async fn append_with_facts(&self, thread: ThreadId, event: &SeqEvent, facts: DelegationFacts) -> anyhow::Result<()>;
```
`append` keeps working (facts default). Inside the writer's `project_event`, after the item/turn/gate
projections of the same event: `delegations::transition(tx, thread, event, &facts, now)` (3.4). The
writer collects `(wake, Vec<Delegation> changed)` from it and, after COMMIT, sends on `wake` once if
asked and calls `changed` once per delegation.

### 3.4 Transition half (SQL side in `store/delegations.rs`, rules in `delegation/transition.rs`)
```rust
// crates/fleet-daemon/src/services/agents/delegation/transition.rs — pure, no SQL, no manager import.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DelegationFacts {
    /// A question/plan/permission gate is open on this thread (before applying this event).
    pub gate_open: bool,
    /// `ThreadProjection::background_tasks` is non-empty (before applying this event).
    pub background_live: bool,
    /// The text of the latest assistant message item, first 4 KiB.
    pub last_assistant_text: Option<String>,
    /// Paths this thread's edit-like tool calls touched, deduped, in first-seen order.
    pub files_changed: Vec<String>,
    /// The record's stop cause, when the thread is stopped.
    pub stop_cause: Option<StopCause>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transition { pub next: Delegation, pub actions: Vec<OutboxAction>, pub changed: bool }
/// The child-side rules of NATIVE-AGENTS.md §15. `now` stamps `finished`.
pub(crate) fn child_transition(current: &Delegation, event: &AgentEvent, facts: &DelegationFacts, now: DateTime<Utc>) -> Transition;
/// The last-assistant-text capture: every child `TurnSettled` sets `result` with
/// `source: LastAssistantText` unless a `Reported` result exists. Part of `child_transition`.
```
Rules (`child_transition`), each arm unit-tested:
- `SessionConfigured` while `Starting` → `Running` + `Mirror`.
- `GateOpened` (Question | Plan | Permission) → `Blocked` + `Mirror`. `GateResolved`/`GateWithdrawn` while `Blocked` → `Running` + `Mirror`.
- `TurnSettled { Completed }`: reported result and `!background_live` → `Succeeded` + `Deliver`; reported result and `background_live` → `Settling` + `Settle`; no report and `nudges < MAX_NUDGES` → `Settling` + `Nudge`; no report and nudges exhausted → `Incomplete` (result = last text, `LastAssistantText`) + `Deliver`. Status `Blocked` with `status_payload == "reported blocked"` at settle → `Failed` + `Deliver`.
- `TurnSettled { Error | MaxTurns | BudgetExhausted | Denied | Other }` → `Failed`, `status_payload` = outcome name (+ message), result = last text + `Deliver`.
- `TurnSettled { Interrupted }` and `TurnAborted { User | SessionStopped | Timeout | Superseded | Other }` → `Cancelled` + `Deliver` + `CancelChildren`.
- `TurnAborted { ProviderExited }`: phase 3 → `Failed` (`"provider exited"`) + `Deliver`. Phase 6 replaces: `recoveries == 0` → keep status, `recoveries = 1`, + `Recover`; else `Failed` (`"provider exited twice"`) + `Deliver`.
- `RuntimeError { fatal: true }`, `SessionExited { expected: false }` while live → `Failed` + `Deliver`. `SessionExited { expected: true }` while live → `Cancelled` + `Deliver` + `CancelChildren`.
- Headline: on child `ItemStarted` (tool: its summary/command), terminal `ItemUpdated`/`ItemCompleted`, and `TurnSettled` (first line of last assistant text). Never on `ContentDelta`. `changed = true` only when a column moved.
- A terminal event on an already-terminal delegation changes nothing.

SQL side `delegations::transition(tx, thread, event: &SeqEvent, facts, now) -> anyhow::Result<TransitionOutcome { wake: bool, changed: Vec<Delegation> }>`:
- Load by child; if found, run `child_transition`, `update`, `enqueue` each action, `wake = !actions.is_empty()`.
- Then treat `thread` as a **caller**: `ItemStarted` with `UserMessage { origin: Delegation { id } }` → that delegation's `delivery = Delivered { seq, turn }`, `mark_done_for(id, Deliver)`, changed; `TurnSettled | SessionConfigured | GateResolved | SessionStateChanged(Ready)` with any open `Deliver` row for a delegation whose caller is `thread` → `wake = true`.

## 4. Daemon service (owner: stage `service-skeleton` for shapes; bodies by stage)

Module `crates/fleet-daemon/src/services/agents/delegation/{mod.rs, run.rs, complete.rs, queries.rs, cancel.rs, worker.rs, transition.rs, footer.rs, limits.rs, tests/}`.

```rust
// mod.rs
#[derive(Clone)]
pub(crate) struct DelegationService { inner: Arc<Inner> }
struct Inner { store: SqliteAgentStore, manager: AgentSessionManager, events: BroadcastBus, wake: mpsc::UnboundedSender<()>, config: Arc<ConfigStore>, worktrees: Worktrees }
impl DelegationService {
    /// Returns the service and the worker that owns the wake receiver. Composition installs
    /// `DelegationHooks` on the store from the returned sender and spawns the worker.
    pub(crate) fn new(store, manager, events, config, worktrees) -> (Self, DelegationWorker);
    pub(crate) fn wake_sender(&self) -> mpsc::UnboundedSender<()>;
    pub(crate) async fn run(&self, request: RunRequest) -> Result<ResponseBody, ProtoError>;          // run.rs
    pub(crate) async fn complete(&self, request: CompleteRequest) -> Result<ResponseBody, ProtoError>; // complete.rs
    pub(crate) async fn list(&self, caller: Option<ThreadId>) -> Result<ResponseBody, ProtoError>;    // queries.rs
    pub(crate) async fn get(&self, id: DelegationId) -> Result<ResponseBody, ProtoError>;             // queries.rs
    pub(crate) async fn wait(&self, id: DelegationId, timeout_ms: u64) -> Result<ResponseBody, ProtoError>; // queries.rs
    pub(crate) async fn cancel(&self, id: DelegationId) -> Result<ResponseBody, ProtoError>;          // cancel.rs
    pub(crate) fn publish_changed(&self, delegation: Delegation);  // Event::DelegationChanged on the bus
}
pub(crate) struct RunRequest { caller: ThreadId, provider: AgentKind, brief: String, expectation: String, worktree: Option<WorktreeId>, mode: Option<PermissionMode>, model: Option<ModelSelection>, title: Option<String>, eager: bool }
pub(crate) struct CompleteRequest { delegation: DelegationId, child: ThreadId, token: String, result: String, blocked: bool }
pub(crate) struct DelegationWorker { /* wake receiver + service clone */ }
impl DelegationWorker { pub(crate) async fn run(self, shutdown: CancellationToken); }
```
Errors use the manager's helper shapes (`validation`, `conflict`, `not_found`, plus a local `unsupported`),
messages naming the rule that refused. Composition: `services/composition.rs` constructs the service after
the manager, installs the hooks, and `start_periodic_tasks` spawns `worker.run(shutdown)` beside the other
loops. Dispatch: six arms `RequestBody::Delegation*` → `self.agent_response(self.delegations.<verb>(..).await)`.

`complete` (decided 2026-09-18): the first accepted report calls `set_report` with the SHA-256 of the
untruncated text and `now`; a repeat whose SHA-256 equals `report_meta`'s hash is an idempotent success
that changes nothing; a repeat with a different hash is refused with `conflict("delegation <id> already
reported at <reported_at RFC 3339>")`. Token comparison is constant time over the two SHA-256 digests.

### 4.1 Manager verbs (`manager/commands.rs`, `manager/apply.rs`, `manager.rs`)
```rust
pub struct CreateOptions {
    pub worktree: WorktreeId, pub provider: AgentKind, pub model: Option<ModelSelection>, pub mode: PermissionMode,
    pub resume_cursor: Option<String>, pub title: Option<String>,
    pub parent: Option<ThreadId>, pub delegation: Option<DelegationId>, pub extra_env: BTreeMap<String, String>,
}
pub async fn create_with(&self, options: CreateOptions) -> Result<ResponseBody, ProtoError>;  // `create` becomes a wrapper
pub async fn running_turn(&self, thread: ThreadId) -> Result<Option<TurnId>, ProtoError>;      // under the operation lock
/// Appends `ItemStarted { item, kind }` under `turn` with a caller-minted id (the client's
/// optimistic-id pattern, `UserInput.item`); refuses (`conflict`) when that turn is not running.
/// Decided 2026-09-18: the id is minted by the caller so `run` can write the delegation row (which
/// needs `caller_item`) before the item exists, in the order row -> item -> first message.
pub async fn append_item(&self, thread: ThreadId, turn: TurnId, item: ItemId, kind: ItemKind) -> Result<(), ProtoError>;
/// Appends `ItemUpdated { item, patch }` and, when `complete` is `Some`, `ItemCompleted { item, status }`.
pub async fn patch_item(&self, thread: ThreadId, item: ItemId, patch: ItemPatch, complete: Option<ItemStatus>) -> Result<(), ProtoError>;
pub async fn record(&self, thread: ThreadId) -> Result<AgentThreadRecord, ProtoError>;         // a clone of the record
pub async fn projection(&self, thread: ThreadId) -> Result<ThreadProjection, ProtoError>;      // a clone
pub(crate) fn delegation_facts(projection: &ThreadProjection, record: &AgentThreadRecord) -> DelegationFacts;
```
`StartRequest.env` carries `extra_env`; both adapters already `overrides.extend(request.start.env)` before
`FLEET_SESSION`, so `extra_env` reaches the child with no adapter change beyond making sure it is not
stripped. `apply_event` computes `delegation_facts` from the pre-event projection and calls
`store.append_with_facts`. `send` already carries `UserInput.origin` into the recorded `UserMessage`
(both the announced-turn path and the steer path; verify with a test).

### 4.2 Constants and texts (`delegation/limits.rs`, `delegation/footer.rs`)
```rust
pub(crate) const MAX_DEPTH: u8 = 3;
pub(crate) const MAX_LIVE_CHILDREN_PER_CALLER: usize = 4;
pub(crate) const MAX_LIVE_DELEGATIONS: usize = 8;
pub(crate) const MAX_NUDGES: u8 = 2;
pub(crate) const SETTLE_GRACE: Duration = Duration::from_secs(30);
pub(crate) const RETRY_TICK: Duration = Duration::from_secs(60);
pub(crate) const RESULT_CAP_BYTES: usize = fleet_proto::agents::ITEM_BODY_MAX_CHUNK_BYTES as usize;
pub(crate) const WAIT_DEFAULT_SECS: u64 = 540;   // also the ceiling, enforced by the CLI
```
```rust
/// The completion footer appended to the child's first message (design doc, verbatim).
pub(crate) fn footer(id: DelegationId, expectation: &str) -> String  // renders FOOTER_TEMPLATE
pub(crate) const FOOTER_TEMPLATE: &str = "--- Fleet delegation {id} ---\n\
You are running as a subagent. No human is watching this session.\n\
The caller expects: {expectation}\n\
When the work is fully finished and verified, report it with exactly one command:\n  \
fleet subagent complete --result-file <path-to-your-report.md>\n\
Write the report first, then run the command. Do not run it before you are done.\n\
If you are blocked and cannot finish, run:\n  \
fleet subagent complete --blocked --result-file <path-with-what-you-need>\n\
Do not ask the user questions; state assumptions in the report instead.";
pub(crate) const NUDGE: &str = "You have not reported a result. If the work is done, run `fleet subagent complete --result-file <path>`. If not, continue.";
pub(crate) const RESUME_NUDGE: &str = "The session was restarted. Continue, and report with `fleet subagent complete` when done.";
pub(crate) const SAME_WORKTREE_WARNING: &str = "the child edits the caller's worktree; end your turn before it works, or pass --worktree";
/// First message: `{brief}\n\n{footer}`.
pub(crate) fn first_message(brief: &str, id: DelegationId, expectation: &str) -> String;
/// `[fleet subagent <id> finished: succeeded]\nprovider: codex, thread: <child>, duration: 14m 02s, files changed: 6\n\n<text>` plus
/// `\n\n(report elided at <n> bytes)` when elided. Status words: succeeded | incomplete | failed | cancelled.
pub(crate) fn delivered_message(delegation: &Delegation, now: DateTime<Utc>) -> String;
/// `↳ <provider> — <first line of the brief, cut at 48 chars>`
pub(crate) fn child_title(provider: AgentKind, brief: &str) -> String;
```
Token: two `uuid::Uuid::new_v4().simple()` concatenated (64 hex chars); stored as `sha2::Sha256` hex.
Compare hashes with a constant-time fold (`iter().zip().fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0`).
Mode default `PermissionMode::FullAccess`.

### 4.3 Worker rules (`worker.rs`)
Loop: drain once at start (before serving), then `select!` on wake, a `RETRY_TICK` interval, and
shutdown. Drain: `open_rows` in id order, one row per caller per pass. Per action:
- `Mirror`: `patch_item(caller, caller_item, ItemPatch::Delegation { status }, None)`; mark done. On a terminal delegation the same patch with `complete: Some(Completed | Failed)` (Succeeded → Completed, else Failed) happens as the first step of `Deliver`.
- `Nudge`: `send(child, UserInput { text: NUDGE, origin: User, .. })`, bump `nudges` in a `delegation_write`, mark done.
- `Settle`: no live background task, or `created + SETTLE_GRACE` passed → finalize `Succeeded` + `Deliver` in one `delegation_write`; else leave the row open.
- `Recover` (phase 6): `send(child, RESUME_NUDGE)`; on `conflict`/not-live with no resume cursor → finalize `Failed` + `Deliver`. Mark done.
- `CancelChildren` (phase 6): for every live delegation whose caller is this delegation's child, `cancel` it (stack, not recursion). Mark done.
- `Deliver`: read caller record + summary; compose `delivered_message`; then by caller state: `Ready` and idle → `send(caller, UserInput { text, origin: Delegation { id }, .. })` (delivery is recorded by the caller-item rule, row stays open until that commit marks it); running and `!eager` → leave open; running and `eager` → `send`; stopped with `StopCause::ProviderExit` → `send` (resumes by cursor); stopped with `StopCause::User` → leave open; stopped with no cursor, or record missing → `Undeliverable { reason }`, mark done; caller blocked on a gate → leave open. A `conflict` from `send` leaves the row open; any other error logs `delegation`, `action`, `caller`, `child` and leaves it open.
- Every action publishes `DelegationChanged` after its write. One `tracing::info!` per action with those four fields.

## 5. `fleet subagent` CLI (owner: stage `cli-subagent`)
`crates/fleet-cli/src/args.rs`: `Command::Subagent(SubagentArgs)` after `Agent`; `SubagentCommand { Run(SubagentRunArgs), Complete(SubagentCompleteArgs), Wait(SubagentWaitArgs), Status(SubagentIdArgs), List(SubagentListArgs), Cancel(SubagentIdArgs) }`, every verb with `--json`.
- `run --provider <claude|codex> [--brief-file F] --expect <text> [--worktree W] [--mode M] [--model M] [--title T] [--eager] [--caller <thread>]`; brief from the file, else stdin. Caller: `--caller`, else `FLEET_SESSION`, else `validation("fleet subagent run requires --caller <thread> or FLEET_SESSION")`. Prints `delegation <id> started, child thread <thread>` and the warning on a second line; exit 0.
- `complete [<id>] [--result-file F] [--blocked] [--json-result]`; id else `FLEET_DELEGATION`; child = `FLEET_SESSION`; token = `FLEET_DELEGATION_TOKEN`; result from file (error names the path) else stdin; `--json-result` validates JSON; truncate at `RESULT_CAP_BYTES` with a stderr notice; prints `reported`.
- `wait <id> [--timeout S]` default and ceiling 540; prints `delivered_message`-shaped text (compose it from the record: same format); exit 0 when terminal, 2 on timeout.
- `status <id>` and `list [--caller T]`: `id\tstatus\tprovider\tchild\tage\tdelivery` per line.
- `cancel <id>`: prints `cancelled`.
JSON envelopes in `envelope.rs`: `SubagentEnvelope { protocol, delegation: &Delegation, warning: Option<&str> }`, `SubagentsEnvelope { protocol, delegations: &[Delegation] }`. Human output in `human.rs::subagents(&[Delegation], now)`.

## 6. Kit (owner: stage `kit-rows`)
`crates/fleet-ui-kit/src/components/agent/rows.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationRowStatus { Starting, Working, Blocked, Done, Incomplete, Failed, Cancelled }
impl DelegationRowStatus { pub const fn word(self) -> &'static str; pub const fn is_live(self) -> bool }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationRow { pub provider: SharedString, pub title: SharedString, pub status: DelegationRowStatus,
    pub headline: Option<SharedString>, pub elapsed: SharedString, pub hint: SharedString }
#[derive(Debug, Clone, PartialEq)]
pub struct DelegationResultCard { pub header: SharedString, pub body: <the same document type AssistantRow carries>,
    pub collapsible: bool, pub expanded: bool, pub hint: SharedString }
TranscriptRowKind::Delegation(DelegationRow), TranscriptRowKind::DelegationResult(DelegationResultCard)
```
Rendering in new `rows/delegation.rs`: leading `↳` + provider glyph (`Icon::Bot` family as SubagentRow), title, status word, one mark (spinner gray while `Starting|Working`, amber dot `Blocked`, `circle-check` `Done`, `circle-x` `Failed|Incomplete|Cancelled`), second line headline, trailing elapsed and hint. Tones from the theme, sizes from tokens. `is_expandable`: `DelegationResult` → `collapsible`; `Delegation` → false. `rhythm`: both `Work`. The card collapses to eight lines with the tool rows' fold affordance.
`MetadataSegment` gains `pub target: Option<SharedString>` + `pub fn target(self, id: impl Into<SharedString>) -> Self`; `MetadataRow` gains `.on_target(impl Fn(SharedString, &mut Window, &mut App) + 'static)`; a segment with a target draws in the link tone with the kit's focus ring and calls the handler on click/enter.
Galleries: every `DelegationRowStatus`, both card states, one targeted segment. `every_row_kind` and `every_component_state_draws` extended.

## 7. App (owners: `app-state`, `app-actions-keymap`, `app-rows`, `app-child-tab`, `app-palette`)
`crates/fleet-app/src/state/agents.rs` (`AgentThreads`):
```rust
delegations: HashMap<DelegationId, Delegation>, delegations_revision: u64, attached: HashSet<ThreadId>,
pub fn delegation(&self, id: DelegationId) -> Option<&Delegation>;
pub fn delegations_of_caller(&self, caller: ThreadId) -> Vec<&Delegation>;   // creation order
pub fn delegation_of_child(&self, child: ThreadId) -> Option<&Delegation>;
pub fn delegations_revision(&self) -> u64;
pub fn seed_delegations(&mut self, list: Vec<Delegation>);   // after Hello when `agent.delegation` advertised
pub fn apply_delegation(&mut self, d: Delegation);            // DelegationChanged; bumps revision if changed
pub fn is_attached(&self, thread) -> bool; pub fn attach(&mut self, thread) -> bool /* also clears closed */; pub fn detach(&mut self, thread) -> bool; pub fn reopen(&mut self, thread) -> bool;
pub fn caller_of(&self, thread) -> Option<ThreadId>;          // summary.parent
pub fn children_of(&self, caller) -> Vec<&AgentThreadSummary>; // creation order
```
`of_worktree`: callers (no parent) in daemon order not closed, each followed by its attached children; a
child is present only when attached. `attention(caller)` = max(own, children needs_you, children working)
via each child's `attention_for` this client's cursor; `counts()` folds children under the caller;
`is_working(caller)` also true while any child is live. `BridgeEvent::Delegations(Vec<Delegation>)` seeded
after `Capabilities` when advertised; `Event::DelegationChanged(d)` → `apply_delegation`.
Snapshot (`state/harness.rs` + `projection.rs`, additive): `agents.threads[].parent: Option<String>`,
`agents.threads[].attached: bool`, `agents.delegations: Vec<{id, status, caller, child, delivery, headline: Option<String>}>` (status/delivery are the `word()`s); `lists.tabs` rows gain badge `child` for a child tab. `ProjectionKey` gains `delegations_revision` and an attached-set revision.

Actions (`actions.rs`): `native_agent::AttachChild`, `native_agent::CancelDelegation`, `prefix::UpToCaller`, `prefix::AgentsPicker`.
Keys (`keymap.rs`): `ctrl-s u` → `prefix::UpToCaller` and `ctrl-s d` → `prefix::AgentsPicker` in `Workspace > Prefix` and every agent sub-mode's prefix rows; `x` in `Agent > AgentNativeScroll > AgentRow` → `native_agent::CancelDelegation`; `enter` stays `ExpandRow` (the handler attaches when the focused row is a delegation row or card); `y` `CopyRow` copies the delegation id on those rows; `ctrl-s x` stays `native_agent::CloseTab` (its handler detaches a child instead of closing).
Rows (`screens/agent_thread/rows/item.rs`): `ItemKind::Delegation` → `DelegationRow` joined to `delegations.get(id)` (fallback to the item's status when the map lacks it); `UserMessage { origin: Delegation { id } }` → `DelegationResultCard` with header `↳ <provider> finished · <word> · <elapsed> · <n> files`. `RowsKey` gains `delegations_rev: u64`. `RowInputs` gains `delegations: &HashMap<DelegationId, Delegation>`.
Child tab: strip title `↳ <provider> — <title>`; pinned leading metadata segment `for [<n>] <provider> — <title>` (`·` when the caller is not attached) with `target` = caller thread; composer placeholder `Steering a subagent of [<n>]. It reports to its caller when it finishes.`; `^s u` selects the caller (attaching first); `^s x` on a child detaches.
Palette: seeded query `agents`, section `PaletteSectionKind::Agents` titled `AGENTS`; rows: key column strip index or `·`, glyph = attention mark, primary `↳ <provider> — <title>` / `<provider> — <title>`, `detail` = `<word> · <age>` (`blocked · question` when a gate is open), trailing `attach`/`go`; order callers then children then other-worktree children with ` · <worktree>` suffix; `⏎` = attached → select; hidden child → attach + select; closed caller → reopen + select; other worktree → switch session, attach, select. `ctrl-s d` opens it like `ctrl-s W` opens `sessions`.

## 8. Harness (owner: stage `harness-shell-step`)
Transcript step, additive: `{"type":"shell","command":"<sh -c string>","name":"Bash"}` (`name` defaults to `Bash`). Both players run `sh -c <command>` with the player's own environment, wait for exit, and present it as an ordinary tool call named `name` with stdout (then stderr) as output. Role rule in the launcher script: when `FLEET_DELEGATION` is set, play `subagent-child.json` (or `subagent-child-blocked.json` for the Claude player); else the preset's transcript. The fixture's tool dir gains a `fleet` shim: `#!/bin/sh\nFLEET_HOME=<harness home> exec <FLEET_APP> "$@"`. New transcripts in `crates/fleet-harness/transcripts/`: `subagent-caller.json`, `subagent-caller-other-worktree.json`, `subagent-child.json`, `subagent-child-blocked.json`. The `agents` preset's Claude transcript becomes the caller when the scenario says so — pick the simplest additive way (a preset field or a second preset `agents-subagent`) and document it in `docs/TESTING-HARNESS.md` §5 under the additive rule. §2 is byte-identical.

## 9. Ownership table (path → stage; a path listed once is edited by that stage only)

| Path | Stage |
| --- | --- |
| `crates/fleet-core/src/agents/{ids.rs,delegation.rs,mod.rs,items.rs,provider.rs,state.rs,projection/summary.rs,projection/mod.rs}` | contracts-core |
| `crates/fleet-daemon/src/services/agents/record.rs`, `agents/harness/mod.rs` (Submitted enum + `turn()`), every exhaustive-match arm the new variants break (minimal `// phase 4 replaces` fallbacks) | contracts-core |
| `crates/fleet-proto/src/{lib.rs,request.rs,response.rs,event.rs}`, `crates/fleet-client/src/**` | contracts-core |
| `crates/fleet-proto/tests/**` | proto-goldens |
| `crates/fleet-core/src/agents/projection/reduce.rs`, `projection/tests/**`, `crates/fleet-daemon/src/services/agents/store/project/**` | reducer-rules |
| `crates/fleet-daemon/src/agents/claude/**`, `agents/codex/**`, `services/agents/manager/**`, `services/agents/manager.rs` | manager-p1 (wave B), then service-skeleton (wave C), then integrate |
| `crates/fleet-daemon/src/services/agents/store/{migrations.rs,schema.rs,delegations.rs,read.rs,list.rs,mod.rs,writer.rs,project.rs,tests.rs}` | migration-store (wave B), then store-transition (wave C) |
| `crates/fleet-daemon/src/services/agents/delegation/{transition.rs,footer.rs,limits.rs}` | transition-pure (wave B); phase 6 edits by recovery |
| `crates/fleet-daemon/src/services/agents/delegation/{mod.rs}`, `services/composition.rs`, `services/dispatch.rs`, `services/maintenance.rs`, `services/agents/mod.rs` | service-skeleton, then integrate |
| `.../delegation/run.rs` | service-run |
| `.../delegation/complete.rs` | service-complete |
| `.../delegation/worker.rs` | service-worker; phase 6 edits by recovery |
| `.../delegation/queries.rs`, `.../delegation/cancel.rs`, `crates/fleet-proto/src/lib.rs` (`AGENT_CAPABILITIES` line only) | service-queries |
| `.../delegation/tests/**` | service-tests; `tests/recovery.rs` by recovery |
| `crates/fleet-cli/src/**` | cli-subagent |
| `crates/fleet-ui-kit/src/components/agent/{rows.rs,rows/render.rs,rows/delegation.rs,metadata_row.rs,tests.rs}`, `components/palette.rs` (Agents section kind + optional slots), `examples/**` | kit-rows |
| `crates/fleet-ui-kit/src/components/terminal_tab_strip.rs` | app-child-tab |
| `crates/fleet-app/src/state/agents.rs`, `state/agents/tests.rs`, `state/connection.rs`, `bridge/**`, `state/harness.rs`, `state/harness/projection.rs`, `state/notifications.rs` (tests only) | app-state |
| `crates/fleet-app/src/actions.rs`, `keymap.rs`, `docs/KEYMAP.md` | app-actions-keymap |
| `crates/fleet-app/src/screens/agent_thread/rows/**`, `screens/agent_thread/mod.rs` (RowsKey), `screens/agent_thread/sync.rs`, `screens/agent_thread/tests/**`, `dialogs/confirm.rs`, the AgentRow key handlers | app-rows |
| `crates/fleet-app/src/views/workspace_tabs.rs`, `screens/agent_thread/presentation.rs`, `screens/workspace/**` (prefix handlers), `shell/chrome.rs` | app-child-tab |
| `crates/fleet-app/src/dialogs/palette.rs`, `dialogs/host.rs`, `state/navigation.rs` | app-palette |
| `crates/fleet-harness/**`, `docs/TESTING-HARNESS.md` §5 | harness-shell-step; §3 by app-state |
| `scenarios/agents/**` | scenarios-p4, scenarios-p5 |
| `crates/fleet-daemon/src/services/doctor.rs`, `docs/DEVELOPMENT.md` | doctor |
| `docs/decisions/0017-native-subagents.md`, `docs/README.md` | adr |
| `TODO.md` | todo-deferred |
| `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`, `docs/UX-SPEC.md` | one docs stage per wave, sequential |

## 10. Definition of done for the initiative
`make lint`, `cargo check --workspace --all-targets`, `make test`, `make harness` green; every tracker box
ticked with a verification line; `agent.delegation` advertised; ADR 0017 indexed; every scenario in the
phase 4 and 5 plans passing; docs audited (phase 7).
