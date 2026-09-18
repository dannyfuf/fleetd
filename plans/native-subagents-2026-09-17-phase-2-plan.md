# Native subagents, phase 2: delegation model, wire variants and storage — Plan
> Tracker: ./native-subagents-2026-09-17-phase-2-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Add every type, wire variant, golden and table that the delegation service and the app will write
to, and serve none of it yet. After this phase a `Delegation` record exists in `fleet-core`, six
requests, three responses and one event exist in `fleet-proto` with byte-exact goldens, the agents
database runs at migration 3 with a `delegations` table and an outbox, and a thread record can name
its parent. A daemon built from this phase behaves exactly as before for every client, because the
new capability string is defined but not advertised.

## Sizing call

**Phased, phase 2 of 7.** See ./native-subagents-2026-09-17-roadmap.md. This phase is the widest
mechanical sweep of the initiative (three crates, one migration, one exhaustive-match sweep in the
app) and the one with the least judgement in it. It is its own pull request so the goldens and the
migration get a review of their own.

## Repository context

- Rust workspace; this phase touches `fleet-core`, `fleet-proto`, `fleet-daemon` (store only) and
  the smallest possible change in `fleet-app` (compile fallbacks for two new variants).
- Lint: `make lint`. Test: `make test`. Targeted: `cargo test -p fleet-core`,
  `cargo test -p fleet-proto --test agent_compatibility`, `cargo test -p fleet-daemon store`.
- Ids: `uuid_id!` macro in `crates/fleet-core/src/agents/ids.rs:71`; `ThreadId`, `TurnId`,
  `ItemId`, `GateId` are its existing uses.
- Items: `ItemKind` at `crates/fleet-core/src/agents/items.rs:104`; `UserMessage` at line 106
  already carries an additive `steered` field with the pattern to copy. `ItemPatch::UserMessage` at
  line 172.
- Provider input: `UserInput` at `crates/fleet-core/src/agents/provider.rs:103` with the additive
  `item: Option<ItemId>` field at line 117 as the pattern.
- Summary: `AgentThreadSummary` at `crates/fleet-core/src/agents/projection/summary.rs:66`; the
  additive `host` field at line 72 is the pattern.
- Thread record: `crates/fleet-daemon/src/services/agents/record.rs:45`.
- Capabilities: `crates/fleet-proto/src/lib.rs`, `AGENT_CAPABILITIES` at line 70 and the
  `AGENT_ACCOUNT_CAPABILITY` const just above it. Requests: `crates/fleet-proto/src/request.rs`;
  the per-thread ordered mutation list is at line 955 (`thread_of` style match). Responses:
  `response.rs`. Events: `event.rs` (capability-gated emission documented around line 202).
- Client timeouts: `crates/fleet-client/src/connection.rs` — `REQUEST_TIMEOUT` 10 s,
  `AGENT_HARNESS_TIMEOUT` 45 s, and `request_timeout(&body)` at line 226 selects per request.
- Goldens: `crates/fleet-proto/tests/agent_compatibility.rs` (request, response, event and
  per-`AgentEvent` persisted-payload goldens; a test at line 205 fails if a variant has no golden),
  legacy fixtures in `tests/agent_compatibility/legacy.rs`, helpers in `tests/support/mod.rs`.
- Store: schema in `crates/fleet-daemon/src/services/agents/store/schema.rs` (`threads` table at
  line 119), migration ladder in `store/migrations.rs` (`MIGRATIONS` at line 47, slots 1 and 2, each
  with a sha256 of its source and a `to_inclusive` ceiling for tests), owned writer in `writer.rs`,
  read pool in `read.rs`, projector in `project.rs` with `append_event`, `project_event`,
  `rebuild_thread`, `quarantine_after`.
- App: `crates/fleet-app/src/screens/agent_thread/rows/item.rs` maps `ItemKind` to transcript rows;
  `rows/tests.rs` has the exhaustive row tests.
- Skills: `rust-ipc-protocol` (proto and goldens), `rust-workspace-architecture` (new module,
  migration discipline), `rust-gpui-testing` (tests), `gpui-components` only for the two app
  fallbacks, `zed-quality-review` at the end.
- Docs: `docs/NATIVE-AGENTS.md` §8 storage, §10 protocol additions, §13 status;
  `docs/research/agents-contracts.md`.

## Assumptions

- The `Delegation` record on the wire never carries the token. The daemon stores `token_sha256`
  and only `RequestBody::DelegationComplete` carries the plaintext, from the child's CLI to the
  daemon.
- `Delegation.headline: Option<String>` is on the record from the start (the design adds it to the
  table listing; the SQL block in the design omits it). Migration 3 includes a `headline TEXT`
  column.
- Migration 3 also adds `threads.stop_cause TEXT` for phase 1's `StopCause`, so a list read keeps
  its "never touches items, turns or gates" invariant. One migration, not two.
- The `delegations` table is **not** rebuilt by `rebuild_thread` and not touched by
  `quarantine_after`: its brief, expectation and token hash are not in `agent_events`. Only its
  `status`, `result`, `delivery` and `headline` are derived from child events, and phase 3's
  transition half owns those. The design's sentence "a replay can rebuild it" is read as "the
  status columns", and this plan says so in the doc edit.
- `ItemKind::Delegation` carries `{ id, provider, child, status }` and is patched by a new
  `ItemPatch::Delegation { status }`. The headline is *not* on the item; it travels on
  `Event::DelegationChanged` so the caller's projection is not rewritten per child token.
- `DelegationWait` gets a client-side timeout of `timeout_ms` plus a 15 s margin. The CLI default
  for `timeout_ms` is set in phase 3.
- The app's two fallbacks (a delegation item drawn as a plain text row; a delegation-origin user
  message drawn as an ordinary bubble) are temporary and replaced in phase 4. They exist only so
  the exhaustive matches compile and nothing panics.

## Out of scope

- Serving any of the six requests. Dispatch returns nothing new until phase 3.
- Advertising `agent.delegation` in `AGENT_CAPABILITIES`. Phase 3, in the commit that serves the
  verbs.
- `DelegationRow` and `DelegationResultCard` in the kit. Phase 4.
- Any write to `delegations` from a live path. Phase 3.

## Affected areas

- `crates/fleet-core/src/agents/ids.rs`, new `crates/fleet-core/src/agents/delegation.rs`,
  `agents/mod.rs` (re-exports), `agents/items.rs`, `agents/provider.rs`,
  `agents/projection/reduce.rs`, `agents/projection/summary.rs`, `agents/projection/tests/`.
- `crates/fleet-proto/src/lib.rs`, `request.rs`, `response.rs`, `event.rs`,
  `tests/agent_compatibility.rs`, `tests/agent_compatibility/legacy.rs`, `tests/support/mod.rs`.
- `crates/fleet-client/src/connection.rs` (`request_timeout`).
- `crates/fleet-daemon/src/services/agents/record.rs`, `store/schema.rs`, `store/migrations.rs`,
  new `store/delegations.rs`, `store/read.rs`, `store/list.rs`, `store/project.rs`, `store/tests.rs`.
- `crates/fleet-app/src/screens/agent_thread/rows/item.rs` and `rows/tests.rs` (fallbacks only).
- `docs/NATIVE-AGENTS.md` §8, §10, §13; `docs/research/agents-contracts.md`.

## Tasks

### P2-T01 — Add `DelegationId` and the `Delegation` record family to `fleet-core`
- **Intent:** Define the one new entity of the design as plain data with stable serde shapes.
- **Touches:** `crates/fleet-core/src/agents/ids.rs`, new `crates/fleet-core/src/agents/delegation.rs`,
  `crates/fleet-core/src/agents/mod.rs`.
- **Steps:**
  - `uuid_id!(DelegationId, ...)` beside `ThreadId`.
  - New module `delegation.rs` with: `Delegation` (fields `id`, `caller: ThreadId`,
    `caller_turn: TurnId`, `caller_item: ItemId`, `child: ThreadId`, `provider: AgentKind`,
    `depth: u8`, `brief`, `expectation`, `eager: bool`, `status: DelegationStatus`,
    `result: Option<DelegationResult>`, `nudges: u8`, `recoveries: u8`, `delivery: DeliveryState`,
    `created`, `finished: Option<DateTime<Utc>>`, `headline: Option<String>`);
    `DelegationStatus { Starting, Running, Blocked, Settling, Succeeded, Incomplete, Failed,
    Cancelled }` with `is_terminal()`; `DelegationResult { text, files_changed: Vec<String>,
    source: ResultSource, elided: bool }`; `ResultSource { Reported, LastAssistantText }`;
    `DeliveryState { Pending, Delivered { seq: Seq, turn: TurnId }, Undeliverable { reason } }`.
  - Serde: `rename_all = "camelCase"` on structs, adjacently tagged enums matching the existing
    agent enums, `#[serde(default)]` on every field that can default, and `skip_serializing_if` on
    every `Option` and `bool` that is false, so future additions stay byte-exact.
  - Doc comment on `Delegation` stating the token is never on this type. Round-trip tests for each
    enum variant using the crate's existing pattern.
- **Verification:** `cargo test -p fleet-core delegation`, `make lint`.
- **Done when:** The types compile, round-trip, and are re-exported from `fleet_core::agents`.

### P2-T02 — Add `MessageOrigin` and `ItemKind::Delegation`, with reducer exemptions
- **Intent:** Give a delivered result an origin the app can draw as a card, and give the caller's
  transcript a row that stands for the child.
- **Touches:** `crates/fleet-core/src/agents/items.rs`, `agents/provider.rs`,
  `agents/projection/reduce.rs`, `agents/projection/tests/`, `crates/fleet-app/src/screens/agent_thread/rows/item.rs`,
  `rows/tests.rs`.
- **Steps:**
  - `MessageOrigin { User, Delegation { id: DelegationId } }` in `items.rs`, default `User`,
    `skip_serializing_if` is-default. Add `origin` to `UserInput` and to `ItemKind::UserMessage`,
    both defaulted, following the `steered` and `item` patterns exactly.
  - `ItemKind::Delegation { id: DelegationId, provider: AgentKind, child: ThreadId,
    status: DelegationStatus }` and `ItemPatch::Delegation { status }`. Reducer: apply the patch;
    exempt the kind from `close_open_items` (a delegation row is never closed by its turn's settle)
    and from ever entering `background_tasks`; `is_working` must not count it.
  - Confirm every match over `ItemKind` and `ItemPatch` in `fleet-core`, `fleet-daemon` (store
    projector, adapters) and `fleet-app` compiles. In the store projector, an item of this kind is
    persisted like any other item row.
  - App fallbacks: map `ItemKind::Delegation` to the plain text row the transcript already has for
    unknown work (one line: provider, child id, status word), and leave a user message with
    `origin: Delegation` drawing as an ordinary bubble. Mark both with a comment naming phase 4.
  - Reducer tests: a `Delegation` item survives `TurnSettled` non-terminal; `ItemPatch::Delegation`
    updates the status; `background_tasks` never contains it. Row test: the fallback row draws.
- **Verification:** `cargo test -p fleet-core`, `cargo test -p fleet-app rows`, `make lint`.
- **Done when:** All three crates compile with the new variants and the four tests pass.

### P2-T03 — Add `parent` and `delegation` to the thread record and `parent` to the summary
- **Intent:** Let a child thread name its caller so lists, the strip and the picker can group by it.
- **Touches:** `crates/fleet-daemon/src/services/agents/record.rs`,
  `crates/fleet-core/src/agents/projection/summary.rs`, `store/list.rs`, `store/read.rs`,
  `store/schema.rs` doc comment, `manager/tests/lifecycle.rs`.
- **Steps:**
  - `AgentThreadRecord`: `parent: Option<ThreadId>`, `delegation: Option<DelegationId>`, both
    defaulted. `AgentThreadSummary`: `parent: Option<ThreadId>`, defaulted and skipped when `None`.
  - The summary is built from the record in one place (find it with
    `grep -rn "AgentThreadSummary {" crates/fleet-daemon/src`); copy `parent` there.
  - The store's list read (`store/list.rs`) and thread read select the two new columns from P2-T04
    and populate the record. The write path that inserts a thread row writes them.
  - Test: a record created with `parent` set lists with `summary.parent == Some(caller)`.
- **Verification:** `cargo test -p fleet-daemon`, `make lint`.
- **Done when:** A thread row round-trips `parent` and `delegation` through the store and the
  summary carries `parent`.

### P2-T04 — Migration 3: `delegations`, `delegation_outbox`, and the three `threads` columns
- **Intent:** Persist the delegation record, its follow-up actions, the child's parent link and the
  stop cause, in one forward-only slot.
- **Touches:** `crates/fleet-daemon/src/services/agents/store/migrations.rs`, new
  `store/delegations.rs`, `store/mod.rs`, `store/project.rs`, `store/tests.rs`.
- **Steps:**
  - Slot 3, name `delegations`, `source` is the SQL below verbatim, `sha256` computed from it. Keep
    the `m002` idempotence style: check `PRAGMA table_info(threads)` before each `ALTER`.
    ```sql
    CREATE TABLE delegations (
      id TEXT PRIMARY KEY, token_sha256 TEXT NOT NULL,
      caller_thread TEXT NOT NULL, caller_turn TEXT NOT NULL, caller_item TEXT NOT NULL,
      child_thread TEXT NOT NULL UNIQUE, provider TEXT NOT NULL, depth INTEGER NOT NULL,
      brief TEXT NOT NULL, expectation TEXT NOT NULL, eager INTEGER NOT NULL DEFAULT 0,
      status TEXT NOT NULL, status_payload TEXT, result TEXT, result_source TEXT,
      nudges INTEGER NOT NULL DEFAULT 0, recoveries INTEGER NOT NULL DEFAULT 0,
      delivery TEXT NOT NULL, delivered_seq INTEGER, delivered_turn TEXT,
      headline TEXT,
      created TEXT NOT NULL, finished TEXT
    );
    CREATE INDEX idx_delegations_caller ON delegations(caller_thread, created);
    CREATE TABLE delegation_outbox (
      id INTEGER PRIMARY KEY, delegation TEXT NOT NULL,
      action TEXT NOT NULL,  -- deliver | nudge | recover
      created TEXT NOT NULL, done TEXT
    );
    ALTER TABLE threads ADD COLUMN parent_thread_id TEXT;
    ALTER TABLE threads ADD COLUMN delegation_id TEXT;
    ALTER TABLE threads ADD COLUMN stop_cause TEXT;
    ```
  - `store/delegations.rs`: the only statements that touch the two tables. Functions the service
    will call: insert a delegation (with token hash), get by id, get by child, list (all, by
    caller), count live (per caller and per daemon), update status and payload, set result, set
    delivery, bump nudges and recoveries, set headline, set finished; outbox insert, claim
    undone rows in id order, mark done. Every write takes a `&Transaction` so phase 3 can run it
    inside `project_event`'s transaction. Reads take a pooled connection.
  - `threads` insert and list statements read and write the three new columns. `stop_cause` is
    written from the record on every metadata update.
  - `rebuild_thread` and `quarantine_after` leave `delegations` alone; add a comment saying why.
  - Tests: the ladder test with `to_inclusive` at slot 2 then 3; a fresh database applies all three;
    a delegation row round-trips every column; the outbox claims rows in order and marks them done;
    the `UNIQUE` on `child_thread` refuses a second delegation for one child.
- **Verification:** `cargo test -p fleet-daemon store`, `cargo test -p fleet-daemon migrations`,
  `make lint`.
- **Done when:** A daemon started on a slot-2 database reaches slot 3 and the tests above pass.

### P2-T05 — Wire variants, capability string, timeouts and goldens in `fleet-proto`
- **Intent:** Define every request, response and event of the feature, additively, behind
  `agent.delegation`, with a byte-exact golden per shape.
- **Touches:** `crates/fleet-proto/src/lib.rs`, `request.rs`, `response.rs`, `event.rs`,
  `tests/agent_compatibility.rs`, `tests/agent_compatibility/legacy.rs`, `tests/support/mod.rs`,
  `crates/fleet-client/src/connection.rs`.
- **Steps:**
  - `AGENT_DELEGATION_CAPABILITY: &str = "agent.delegation"` with a doc comment naming what it
    gates. Do **not** add it to `AGENT_CAPABILITIES` yet; leave a comment pointing at phase 3.
  - Requests: `DelegationRun { caller, provider, brief, expectation, worktree: Option<WorktreeId>,
    mode: Option<PermissionMode>, model: Option<String>, title: Option<String>, eager: bool }`,
    `DelegationComplete { delegation, child, token, result, blocked: bool }`,
    `DelegationList { caller: Option<ThreadId> }`, `DelegationGet { delegation }`,
    `DelegationCancel { delegation }`, `DelegationWait { delegation, timeout_ms: u64 }`.
    `DelegationRun`, `DelegationComplete` and `DelegationCancel` join the ordered mutation set at
    `request.rs:955`, keyed by the thread they mutate (caller for run, child for complete, child for
    cancel).
  - Responses: `DelegationStarted(Delegation)`, `Delegations(Vec<Delegation>)`,
    `Delegation(Delegation)`.
  - Event: `DelegationChanged(Delegation)` in the `AgentSummary` subscription family, gated on the
    new capability exactly as `agent.account` gates `AccountChanged`.
  - `fleet-client` `request_timeout`: `DelegationWait` gets `timeout_ms + 15 s`; `DelegationRun`
    gets `AGENT_HARNESS_TIMEOUT` because it starts a harness; the rest default.
  - Goldens: one per new request, response and event in `agent_compatibility.rs`; one golden for
    `ItemKind::Delegation` and `ItemPatch::Delegation` inside a `SeqEvent`; one golden for a
    `UserMessage` with `origin: Delegation` and one proving `origin: User` is absent from the
    bytes; a legacy fixture proving a pre-phase payload without `origin` or `parent` still decodes.
    The test at line 205 that demands a persisted golden per `AgentEvent` variant must stay green.
- **Verification:** `cargo test -p fleet-proto`, `cargo test -p fleet-client`, `make lint`.
- **Done when:** Every new variant has a golden, every existing golden is unchanged, and
  `PROTOCOL_VERSION` is still 7.

### P2-T06 — Describe the new shapes and the migration in `docs/`
- **Intent:** Keep the two authoritative documents ahead of the code that will use these shapes.
- **Touches:** `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`.
- **Steps:**
  - `NATIVE-AGENTS.md` §8 gains migration 3 with its two tables and three columns and the sentence
    that `delegations` is not rebuilt from the log (only its status columns are derived). §10 gains
    the six requests, three responses, one event and the capability string, marked "defined in
    phase 2, served from phase 3". §13 gains a row "9 — Delegations" with status "model and wire
    landed; nothing served".
  - `agents-contracts.md` gains `DelegationId`, the `Delegation` family, `MessageOrigin`,
    `ItemKind::Delegation`, the record and summary fields, with module paths and serialized shape.
- **Verification:** `git diff docs/` reviewed beside the code; nothing to run.
- **Done when:** A reader of §10 can write a client for the six verbs without reading Rust.

## Verification

```sh
make lint
cargo test -p fleet-core
cargo test -p fleet-proto
cargo test -p fleet-daemon store
make test
```

No screen changes; `make harness` is not required. `make restart` after the phase so the running
daemon opens the database at slot 3 before phase 3 work starts.

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean.
- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `make test` passes, including every existing golden unchanged.
- [ ] `agent.delegation` is defined and **not** advertised.
- [ ] `docs/NATIVE-AGENTS.md` §8, §10, §13 and `agents-contracts.md` describe the shapes.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **Migration 3 is forward-only.** A database at slot 3 is refused by an older daemon. Rolling back
  this phase after a daemon has run it means deleting the agents database or restoring a backup
  (`FleetHome::agents_*` paths). Say so in the PR.
- **The exhaustive-match sweep is wide.** Budget an hour for clippy `-D warnings` across
  `fleet-app`'s row mapping and the daemon adapters; use `cargo check --workspace --all-targets`
  first to find every arm.
- **A missed `skip_serializing_if` changes existing goldens.** Treat a golden diff as a bug in the
  new field, never as a fixture to refresh.
