# Native subagent feedback fixes (batch 2, the deferred four) — Plan

> Tracker: ./subagent-feedback-deferred-2026-09-19-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Fleet's native subagent feature (`fleet subagent run|complete|wait|status|list|cancel`) shipped in
`plans/native-subagents-2026-09-17-*`. An orchestrator agent drove it end to end and reported eight
pain points; five were fixed in `plans/subagent-feedback-fixes-2026-09-19-*` (batch 1, shipped on
`fix/native-subagents`). This plan fixes the four that were deliberately deferred and are listed
under **Deferred** in `SESSION_TODO.md`:

1. **Cost visibility.** A caller cannot see what a child cost. Token totals and dollar cost already
   exist per thread and the GUI renders them; the `fleet subagent` CLI shows neither.
2. **Per-child environment.** There is no way to hand a child an environment variable, so four
   children sharing one worktree fight over one `CARGO_TARGET_DIR` and serialise on the cargo
   build lock.
3. **Result delivery.** `fleet subagent status` never prints the child's report at all, so an
   orchestrator that wanted the body went and read files out of `/tmp`; and a caller that already
   consumed a result through `fleet subagent wait` is sent the same result *again* as a user
   message when its turn settles — eight children meant eight duplicate messages.
4. **Brief echo.** `fleet subagent run --json` serialises the whole `Delegation`, brief included,
   so launching a child with a 250-line brief costs the orchestrator 250 lines of its own context
   right back.

None of this renders a pixel. It is CLI surface, one daemon read path, one daemon delivery rule,
one child-environment rule, three additive wire fields and one additive database column.

## Sizing call

**Standard.** Four small features against one subsystem, spanning `fleet-core`, `fleet-proto`,
`fleet-client`, `fleet-daemon` and `fleet-cli`. Each is tens of lines of behaviour. I considered
phasing and rejected it: there is no intermediate state that has to ship on its own and no
migration that needs a deprecation window — the one new database column (T04) is additive, written
by the daemon that added it, and read by nothing else.

The binding constraint is not size, it is **concurrency**. These tasks are executed by separate
agents at the same time in one shared worktree, so the cut is by file ownership, not
one-task-per-reported-item. Three of the four items need `crates/fleet-proto/src/request.rs`,
`crates/fleet-daemon/src/services/dispatch.rs` or
`crates/fleet-daemon/src/services/agents/delegation/queries.rs`, and all four need
`crates/fleet-cli/src/commands/subagents.rs`. See *Concurrency and file ownership*; it is the
reason there are seven tasks and why the first three are a chain rather than a fan-out.

## Repository context

- **Rust Cargo workspace**, edition 2024, macOS GPUI app (`fleet`) + daemon (`fleetd`) + CLI over
  twelve crates in `crates/`. Branch `fix/native-subagents`, HEAD `28bbb31`, working tree clean.
- **Lint:** `make lint` = `cargo fmt --check` + `cargo clippy --workspace --all-targets
  --all-features -D warnings`. There is no separate type-check step; clippy is it.
- **Test:** `make test` builds `fleet-daemon`, `fleet-app` and `fleet-harness` first, then
  `cargo test --workspace`. On this machine it needs `TMPDIR=/private/tmp/ht` — short *and*
  canonical — for reasons recorded in `SESSION_TODO.md` that predate this branch.
- **Harness:** `make harness` drives the real GUI. **Not required here**; nothing in this batch
  renders. Confirm that in T07 rather than skipping it silently.
- **Do not restart the daemon.** No task may run `make restart`, `make run`, `make daemon` or
  `fleet daemon restart`: the shared `fleetd` on this machine hosts the session executing this
  batch. T07 boots a private daemon under its own `FLEET_HOME` for the live field check, the way
  batch 1's T05 did — the recipe is in that batch's tracker Notes.
- **Build isolation.** Agents run concurrently in one worktree and would otherwise serialise on the
  cargo build lock. Every task uses its own `CARGO_TARGET_DIR` (`target-d01` … `target-d07`); only
  T07's closing `make lint` / `make test` uses the shared `target/`.
- **Project skills** in `.claude/skills/`, named per task below. `zed-quality-review` is the gate
  for every task.
- **Docs are authoritative** (`CLAUDE.md`): `docs/NATIVE-AGENTS.md` §15 is the subagent section,
  `docs/decisions/0017-native-subagents.md` is the ADR, `docs/research/agents-contracts.md` is the
  CLI and wire contract, `README.md` carries the CLI table.
- **Commits:** `<area>: <imperative lowercase summary>`, area ∈ {`core`, `proto`, `client`,
  `daemon`, `cli`, `docs`, `tests`}.

### What the code actually does today (read this before planning your own approach)

- `ThreadProjection` (`crates/fleet-core/src/agents/projection/mod.rs:212`) carries
  `cumulative_usage: Usage`, `cumulative_cost_usd: Option<f64>` and `context_pct: f32`.
  `cumulative_usage` is `aggregate_usage(&self.turns)` over **settled** turns, plus the live turn's
  latest `TokenUsage` while one is running (`projection/reduce.rs:339`). Cost and context are the
  latest *reported* values, never blanked by a frame that omits them (`reduce.rs:364-376`).
- `turns.usage_json` already exists as a column (`store/schema.rs:205`), and
  `agent_events` has `idx_agent_events_thread_kind_seq`. Both matter to T03.
- `AgentSessionManager::projection()` (`manager/delegation.rs:163`) **hydrates a cold thread and
  clones its whole transcript**. It is not a cheap read. See the contradiction note in T03.
- `DelegationResult.text` is truncated **at ingest** by `reported_result`
  (`delegation/complete.rs:219`) to `RESULT_CAP_BYTES` = `ITEM_BODY_MAX_CHUNK_BYTES` = 256 KiB.
  Above that the tail is discarded and nothing anywhere keeps it.
- `CreateOptions.extra_env` (`manager/commands.rs:68`) reaches the provider as `StartRequest.env`;
  `delegation/run.rs:198` fills it with `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN`. The
  **resume** path (`services/agents/manager.rs:427-450`) rebuilds `env` from scratch and keeps only
  those two.
- `delegations.delivery` is a bare `TEXT NOT NULL` column with no `CHECK`
  (`store/migrations.rs:141`), decoded by a string match at `store/delegations.rs:854`.
- `RequestBody::DelegationWait` carries only `{ delegation, timeout_ms }`
  (`fleet-proto/src/request.rs:394`). The daemon cannot tell who is waiting.
- `fleet subagent status` renders through `human::subagents`
  (`crates/fleet-cli/src/human.rs:195`), a six-field tab-separated line: id, status, provider,
  child, duration, delivery. **It prints no report body and no brief.**
- The last agent-database migration slot is **5** (`closed_threads`, merged from `main` in
  `06a56d2`). A new slot is 6.

## Assumptions

- The four decisions in `SESSION_TODO.md`'s **Deferred** section are settled. This plan implements
  them; where the code contradicted one, the contradiction is named in the task and the smallest
  correct alternative is specified rather than re-litigated. Five such contradictions exist; each
  is marked **⚠ contradiction** in its task and listed again in the tracker's decisions log.
- `PROTOCOL` (the CLI JSON envelope version, `crates/fleet-cli/src/envelope.rs:18`) stays `1`.
  Every envelope change here is additive and `skip_serializing_if`, which is the same rule the wire
  follows.
- No capability string and no `PROTOCOL_VERSION` bump for the three new wire fields, for the reason
  batch 1 recorded for `fleet_path`: an optional field whose absence is byte-indistinguishable from
  an older peer's payload needs no negotiation. `DelegationWait.caller` is the one to think twice
  about — an old client omitting it simply never consumes a delivery, which is exactly today's
  behaviour.
- Tasks run concurrently against one worktree and commit to `fix/native-subagents` directly. There
  is no branch per task and no merge step.

## Out of scope

- `fleet agent list` and `AgentThreadSummary` gain no usage columns. That needs a `threads`-table
  migration and a projector change; the decision defers it.
- Delegation-**tree** rollups. Every number this batch shows is the child's **own thread only**,
  descendants excluded. Say so in the docs; do not sum a subtree.
- `--eager` behaviour is unchanged.
- The `complete` and `cancel` JSON envelopes keep the full brief. The decision names `run`, `wait`
  and `list`; `cancel` shares `single_delegation` with `status`, which keeps it by design. Eliding
  the brief from `complete` — which echoes a brief back to the child that already has it — is a
  reasonable follow-up and is recorded in the tracker, not done here.
- A `--follow` on `wait`. Batch 1 uncapped the timeout; T05 makes the report reachable from
  `status`. Neither of those needs a follow mode, and the item said to consider one only if the
  first two proved insufficient.
- The three pre-existing failures in `SESSION_TODO.md`'s "Found during the batch" section
  (`pty_holder.rs` flake, harness `window_rect`, the `$TMPDIR` / `platform_root_alias` bug). They
  are unrelated and A/B-proved to predate this branch.

## Affected areas

Single repository: `/Users/danny/.swarm/worktrees/dannyfuf/fleetd/fix-native-subagents`.

| Area | Files |
| --- | --- |
| Core types | `crates/fleet-core/src/agents/delegation.rs` |
| Wire | `crates/fleet-proto/src/request.rs`, `crates/fleet-proto/tests/agent_compatibility.rs` |
| Client | `crates/fleet-client/src/api/agents.rs`, `crates/fleet-client/src/connection.rs` (tests) |
| Daemon — dispatch | `crates/fleet-daemon/src/services/dispatch.rs`, `crates/fleet-daemon/src/services/router/classify.rs` (tests) |
| Daemon — delegation | `services/agents/delegation/{mod,run,queries,worker}.rs` and `delegation/tests/*` |
| Daemon — manager | `services/agents/manager.rs` (resume), `services/agents/manager/commands.rs` |
| Daemon — store | `services/agents/store/{delegations,schema,migrations,tests}.rs` |
| CLI | `crates/fleet-cli/src/{args,human,envelope}.rs`, `src/commands/subagents.rs`, `src/commands/tests.rs` |
| App (fixups only) | `crates/fleet-app/src/state/{connection,agents,harness}/tests.rs`, `src/screens/workspace/agent/tests.rs`, `src/screens/agent_thread/tests/{fixtures,view}.rs` |
| Docs | `docs/NATIVE-AGENTS.md` §15, `docs/decisions/0017-native-subagents.md`, `docs/research/agents-contracts.md`, `README.md`, `SESSION_TODO.md` |

## Concurrency and file ownership

These tasks are executed by separate agents **at the same time in one shared worktree**. Two agents
editing one file clobber each other. Ownership below is exclusive and absolute: **if a file is not
listed under your task's "Owns", do not open it for writing.**

### Contested files and how they are resolved

| Contested file | Wanted by | Resolution |
| --- | --- | --- |
| `crates/fleet-proto/src/request.rs` + `tests/agent_compatibility.rs` | D2 (`env`), D3b (`caller`) | **Ordered.** T01 owns them and commits; T02 owns them afterwards. |
| `crates/fleet-daemon/src/services/dispatch.rs` | D2, D3b | **Ordered**, same pair, same order. T01 leaves the `DelegationWait` arm untouched; T02 edits only that arm. |
| `crates/fleet-client/src/api/agents.rs` | D2, D3b | **Ordered**, same pair, same order. |
| `crates/fleet-daemon/.../delegation/queries.rs` | D1 (usage fill on the three read verbs), D3b (`wait` consumes) | **Ordered.** T02 owns it first, T03 afterwards. |
| `crates/fleet-daemon/.../store/delegations.rs` | D1 (usage read), D2 (persisted env), D3b (`consumed` encode/decode) | **Ordered.** T01 (encode/decode of `consumed`) → T03 (a narrow usage read) → T04 (the `env` column, `DELEGATION_COLUMNS`, `RawDelegation`, `EncodedDelegation`). Three tasks, three disjoint edits, strictly serialised. |
| `crates/fleet-cli/src/args.rs`, `commands/subagents.rs`, `human.rs`, `envelope.rs`, `commands/tests.rs` | all four items | **Merged into T05.** Splitting a clap field's declaration from its use leaves the declaring task with a dead field and `-D warnings` failing; batch 1 learned this. One task owns all of `fleet-cli`. T01 makes the one mechanical `sample_delegation()` fixup in `commands/tests.rs` before T05 starts. |
| `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`, ADR 0017, `README.md` | all four items | **Merged into T06**, which writes every doc change for the whole batch from this plan. Named deviation, below. |

### Named deviation from the same-commit doc rule

`CLAUDE.md` requires a behaviour change and its doc change to land in one commit.
`docs/NATIVE-AGENTS.md` §15 is touched by all four items, so honouring that literally would
serialise the whole batch behind one file. Instead **T06 owns every documentation file and writes
all four items' doc changes from this plan**, which is the specification — the doc does not need to
read T01–T05's diffs. T05 and T07 then re-read §15 and the contracts doc as their last step and fix
any place where the shipped behaviour and the doc disagree. This is the same bounded exception
batch 1 took; do not generalise it.

### Struct-literal fixups, which leak outside ownership

Adding a field to `Delegation` breaks every struct literal of it, and adding a variant to
`DeliveryState` breaks every exhaustive match. Batch 1 was bitten by exactly this and no task owned
the sites. **T01 owns all of them** and must fix every one so that the tree compiles before any
other task starts. The known sites, from a workspace grep at `28bbb31`:

- `crates/fleet-daemon/src/services/dispatch.rs` — the `DelegationRun` match arm (destructuring).
- `crates/fleet-daemon/src/services/router/classify.rs` — `mod tests`.
- `crates/fleet-client/src/connection.rs` — `mod tests`.
- `Delegation { … }` literals: `services/agents/delegation/run.rs:230`, `delegation/footer.rs:129`,
  `delegation/transition.rs:325`, `delegation/tests/run.rs:418`, `delegation/tests/queries.rs:103`,
  `delegation/tests/recovery.rs:211` and `:436`, `delegation/tests/worker.rs:~241`,
  `delegation/tests/complete.rs:~250`, `store/delegations.rs` (`RawDelegation::decode`),
  `store/tests.rs:~690` and `:~721`, `server/connection/events.rs:173`.
- fleet-app fixtures: `state/connection/tests.rs:301`, `state/agents/tests.rs:49`,
  `state/harness/tests.rs:459`, `screens/workspace/agent/tests.rs:210`,
  `screens/agent_thread/tests/fixtures.rs:155`, `screens/agent_thread/tests/view.rs:852`.
- `crates/fleet-cli/src/commands/tests.rs` — `sample_delegation()`.

The list is a starting point, not a contract: `cargo check --workspace --all-targets` is what tells
you when you are done.

### Execution order

```
T01  core + wire + `--env` on the child, and every struct-literal fixup   ── must commit first
  ├── T02  wait knows its caller and consumes the delivery                ── owns the wire files next
  │     └── T03  child usage on every delegation read                     ── inherits queries.rs
  │           └── T04  a resumed child keeps its user environment         ── inherits store/delegations.rs
  │     └── T05  all of fleet-cli                                         ── needs T02's client signature
  └── T06  docs                                                           ── parallel from the start
                                                                          
T07  close-out: reconcile, gate, field check                              ── last, after all six
```

- **T02 may not start before T01 has committed**: it inherits `request.rs`,
  `tests/agent_compatibility.rs`, `dispatch.rs` and `api/agents.rs`.
- **T03 may not start before T02 has committed**: it inherits `delegation/queries.rs`.
- **T04 may not start before T03 has committed**: it inherits `store/delegations.rs`.
- **T05 may not start before T02 has committed**: it calls `Client::delegation_wait`'s new
  signature and needs `commands/tests.rs` free of T01's fixup.
- **T06 runs from the start**, in parallel with everything, working from this plan.
- T03 and T05 are genuinely concurrent with each other, and both are concurrent with T06.

## Tasks

### T01 — Land the shared delegation types, the `--env` wire field, and every struct-literal fixup

**Intent:** Put `Delegation.usage`, `DeliveryState::Consumed` and `RequestBody::DelegationRun.env`
into the core and the wire, make a delegated child's environment carry user variables, and leave the
whole workspace compiling so five other agents can start.

**Skill:** `rust-ipc-protocol` (mandatory — additive versioning and goldens), then
`rust-workspace-architecture`. **Docs:** none; T06 writes them.

**Owns (exclusively):**
`crates/fleet-core/src/agents/delegation.rs`,
`crates/fleet-core/src/agents/mod.rs` (owned, but almost certainly needs no edit: line 14 is
`pub use delegation::*;`, so a new public type is exported automatically),
`crates/fleet-proto/src/request.rs`,
`crates/fleet-proto/tests/agent_compatibility.rs`,
`crates/fleet-client/src/api/agents.rs`,
`crates/fleet-client/src/connection.rs`,
`crates/fleet-daemon/src/services/dispatch.rs`,
`crates/fleet-daemon/src/services/router/classify.rs`,
`crates/fleet-daemon/src/services/agents/delegation/mod.rs`,
`crates/fleet-daemon/src/services/agents/delegation/run.rs`,
`crates/fleet-daemon/src/services/agents/delegation/worker.rs`,
`crates/fleet-daemon/src/services/agents/delegation/tests/run.rs`,
`crates/fleet-daemon/src/services/agents/store/delegations.rs`,
**and every struct-literal fixup site listed above**, including the ones in `fleet-app` and the
`sample_delegation()` helper in `crates/fleet-cli/src/commands/tests.rs`.

**Steps:**

- In `fleet-core`, add `DelegationUsage { usage: Usage, cost_usd: Option<f64>, context_pct: f32 }`
  — reuse `fleet_core::agents::Usage` rather than restating its eight counters, so the CLI shows the
  same arithmetic the GUI does. Add `Delegation.usage: Option<DelegationUsage>` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Document `usage` on the type as **computed on read, never persisted and never broadcast**: the
  store decodes it as `None`, `EncodedDelegation` ignores it, and `publish_changed` sends a record
  whose `usage` is `None`. A doc comment that says this is the only thing stopping a future reader
  from adding a column for it.
- Add `DeliveryState::Consumed` with no payload, `word()` → `"consumed"`. Leave `is_pending`
  matching `Pending` alone. In `store/delegations.rs`, encode it to the existing `delivery` TEXT
  column (leaving `delivered_seq`, `delivered_turn`, `delivery_reason` NULL) and decode `"consumed"`
  back. **No migration**: the column has no `CHECK` constraint.
- Add a `pub(crate) fn consume(tx, id, now) -> anyhow::Result<Option<Delegation>>` to
  `store/delegations.rs`: if the row is terminal and its delivery is `Pending`, set it to
  `Consumed`, `mark_done_for(tx, id, OutboxAction::Deliver, now)`, and return the updated record;
  otherwise return `None` and change nothing. T02 calls it; T01 only defines and unit-tests it.
- Add `env: BTreeMap<String, String>` to `RequestBody::DelegationRun`
  (`#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`, placed after `fleet_path` and
  before `eager`), to `fleet_client::DelegationRunRequest`, and to
  `services::agents::delegation::RunRequest`. Bind it in the `dispatch.rs` `DelegationRun` arm.
  Copy the shape and the doc-comment discipline `fleet_path` got in `fdaabfc`.
- In `delegation/run.rs`, build `extra_env` as **user env first, Fleet identity variables second**,
  so `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN` always win. The CLI also rejects those keys
  (T05), but the daemon must not depend on the CLI for that: any peer can send this field.
- Goldens: regenerate/extend `agent_request_wire_goldens` for a populated `delegation_run` carrying
  `env`; add a `consumed` fixture to `delegation_value_types_have_one_wire_golden_per_variant`; add
  a decode test beside `a_delegation_run_written_before_the_fleet_path_hint_still_decodes` proving a
  `delegation_run` written before `env` still decodes. Confirm the existing `Delegation` goldens are
  **byte-identical** with `usage: None` — if they are not, `skip_serializing_if` is wrong.
- Fix every struct literal and exhaustive match until `cargo check --workspace --all-targets` is
  clean. Prefer a real value over `..Default::default()`; these are fixtures and an explicit `None`
  reads better than a defaulted one.
- **Forward-compatibility note to record in the tracker, not to fix:** `RawDelegation::decode`
  `bail!`s on an unknown `delivery` word. A `fleetd` older than this commit, opened against a
  database that already holds a `consumed` row, refuses to decode it. That is a downgrade hazard,
  not an upgrade one, and it is the same shape the existing three words already have.

**Verification:**

```sh
CARGO_TARGET_DIR=target-d01 cargo test -p fleet-core --lib agents::delegation
CARGO_TARGET_DIR=target-d01 cargo test -p fleet-proto --test agent_compatibility
CARGO_TARGET_DIR=target-d01 cargo check --workspace --all-targets
CARGO_TARGET_DIR=target-d01 cargo clippy --workspace --all-targets --all-features -- -D warnings
```

**Regression test:** `fleet-proto/tests/agent_compatibility.rs` — one golden proving a populated
`delegation_run` carries `env` in its exact wire position, one proving a payload without `env` still
decodes, and one `consumed` fixture in the per-variant delivery golden. Plus a `store/tests.rs`
round trip writing `DeliveryState::Consumed` and reading it back.

**Done when:** `cargo check --workspace --all-targets` is clean at HEAD, the three new goldens pass,
and `git grep -n "usage: None"` shows every `Delegation` fixture in the workspace updated.

---

### T02 — Let `wait` name its caller, and make a caller's own `wait` consume the delivery

**Intent:** Stop an orchestrator that waited on eight children from receiving eight duplicate result
messages when its turn finally settles.

**Skill:** `rust-ipc-protocol`, then `rust-gpui-testing` for the daemon tests.
**Docs:** none; T06 writes them (ADR 0017 gets a dated amendment).

**Depends on:** T01 committed.

**Owns (exclusively, inherited from T01):**
`crates/fleet-proto/src/request.rs`,
`crates/fleet-proto/tests/agent_compatibility.rs`,
`crates/fleet-client/src/api/agents.rs`,
`crates/fleet-daemon/src/services/dispatch.rs` (the `DelegationWait` arm only),
`crates/fleet-daemon/src/services/agents/delegation/queries.rs`,
`crates/fleet-daemon/src/services/agents/delegation/worker.rs`,
`crates/fleet-daemon/src/services/agents/delegation/tests/delivery.rs`,
`crates/fleet-daemon/src/services/agents/delegation/tests/worker.rs`,
`crates/fleet-daemon/src/services/agents/delegation/tests/queries.rs`.

**⚠ contradiction with the decision as written.** The decision says to check "whether that is a
migration or a value in an existing text column". It is a value in an existing text column (T01
proved it) — but that is only half the problem. `RequestBody::DelegationWait` carries
`{ delegation, timeout_ms }` and **nothing else**: the daemon has no idea who is waiting, so it
cannot tell a caller's own `wait` from a third party's. This item therefore needs an additive wire
field as well, which the decision did not anticipate. That is what this task adds.

**Steps:**

- Add `caller: Option<ThreadId>` to `RequestBody::DelegationWait`
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`) and a matching parameter to
  `Client::delegation_wait`. Document it as advisory *identity*, not authorisation: it decides
  whether a delivery is consumed, never whether the wait is answered. An absent `caller` — an older
  `fleet`, or a `wait` from a shell with no `FLEET_SESSION` — consumes nothing, which is exactly
  today's behaviour.
- Thread it through the `dispatch.rs` `DelegationWait` arm into
  `DelegationService::wait(delegation, timeout_ms, caller)`.
- In `queries.rs`, after `wait` has resolved a **terminal** record, call `store::delegations::consume`
  in a `delegation_write` **only when** `caller == Some(record.caller)`. Publish the changed record
  through `publish_changed` so the GUI repaints. Return the record with its new delivery state, so
  the caller's `--json` shows `"delivery": "consumed"` on the very response that consumed it.
- The consume is best-effort and idempotent: `consume` returns `None` if the row was already
  `Delivered`, `Undeliverable` or `Consumed`, and `wait` still answers. A `wait` that races the
  delivery worker and loses simply gets `Delivered`; that is correct, not an error.
- In `worker.rs`, `deliver` must skip injection for a consumed delivery: patch the caller's
  transcript item (the row still has to stop saying "working"), close the outbox row, and return
  **without** `send_durable`. Put the skip after the `patch_item` call and before the
  `should_send` computation, and log it once at `info` with the delegation and caller.
- A `wait` from anything other than the caller consumes nothing. Assert it.

**Verification:**

```sh
CARGO_TARGET_DIR=target-d02 cargo test -p fleet-proto --test agent_compatibility
CARGO_TARGET_DIR=target-d02 cargo test -p fleet-daemon delegation::
CARGO_TARGET_DIR=target-d02 cargo test -p fleet-client
CARGO_TARGET_DIR=target-d02 cargo clippy --workspace --all-targets --all-features -- -D warnings
```

**Regression test:** in `delegation/tests/delivery.rs`, a test named for the behaviour — a caller
that waits on its own terminal child gets the record, the record's delivery is `consumed`, and after
the worker drains, the caller's transcript holds **no** `MessageOrigin::Delegation` user message for
it. A sibling test proves a `wait` with `caller: None` and one with a *different* caller both leave
the delivery `Pending` and still produce the injection. Plus one golden for `delegation_wait`
carrying `caller`, and one proving a payload without it still decodes.

**Done when:** a caller's own `wait` on a terminal child leaves the delegation `consumed` and the
caller receives no duplicate user message, while every other waiter's behaviour is byte-identical to
today.

---

### T03 — Attach the child's own usage and cost to every delegation read

**Intent:** Make `fleet subagent status` and `fleet subagent list` able to say what a child cost,
without hydrating a transcript and without a migration.

**Skill:** `rust-workspace-architecture` (the store is a layering question), then
`rust-gpui-testing`. **Docs:** none; T06 writes them.

**Depends on:** T02 committed.

**Owns (exclusively):**
`crates/fleet-daemon/src/services/agents/delegation/queries.rs` (inherited from T02),
`crates/fleet-daemon/src/services/agents/store/delegations.rs` (inherited from T01),
`crates/fleet-daemon/src/services/agents/store/mod.rs`,
a new `crates/fleet-daemon/src/services/agents/store/usage.rs`,
`crates/fleet-daemon/src/services/agents/delegation/tests/queries.rs` (inherited from T02).

**⚠ contradiction with the decision as written.** The decision says the daemon should compute this
"by loading each child projection in the daemon `DelegationService` list query (bounded N)".
`AgentSessionManager::projection()` (`manager/delegation.rs:163`) goes through `runtime(thread)`,
which **hydrates a cold thread — a full replay of its event log — and then clones the entire
`ThreadProjection`, items and turns included**. A terminal child is almost always cold. Doing that
once per row in `list` would replay every child's whole transcript and leave a runtime resident for
each, which is precisely the failure `store/list.rs` was written to avoid. Do not use `projection()`
here.

**Steps:**

- Add a store read that computes the same three numbers directly from SQL, with no hydrate:
  - **Tokens.** `SELECT usage_json FROM turns WHERE thread_id = ?1 AND usage_json IS NOT NULL`,
    decode each to `Usage`, fold with the same arithmetic `projection::usage::add_usage` uses.
    `turns.usage_json` is written only for a settled turn, which is exactly the set
    `aggregate_usage` folds.
  - **The live turn.** `reduce.rs:357` adds the newest `TokenUsage` on top when its turn has not
    ended. Reproduce that: read the newest `token_usage` event for the thread
    (`SELECT payload FROM agent_events WHERE thread_id = ?1 AND kind = 'token_usage' ORDER BY seq
    DESC LIMIT 1`, served by `idx_agent_events_thread_kind_seq`), and add its `usage` only if its
    turn is not among the settled ones.
  - **Cost and context.** Both come from that same newest `token_usage` payload: `cost_usd` is the
    latest *reported* value and `context_pct` the latest non-zero one, per `reduce.rs:364-376`.
- Expose it as one narrow async method on `SqliteAgentStore` returning
  `Option<DelegationUsage>` — `None` for a thread with no usage at all, so the CLI can print `-`
  rather than a fake zero.
- In `queries.rs`, fill `Delegation.usage` in `get`, `list` and `wait` before building the
  `ResponseBody`. `list` does one read per row; keep it a single `spawn_blocking` batch over the
  reader pool rather than N round trips if the store's shape allows it, and say in a comment why the
  N is bounded (delegation rows are few and `LIST_PAGE`-capped).
- Do **not** fill `usage` in `publish_changed`, `run`, `complete` or `cancel`. The bus carries a
  repaint hint and the GUI already has its own numbers; computing usage on every
  `DelegationChanged` would put a SQL read on the event path.
- Document, in the method's doc comment and in the type: **own thread only, descendants excluded**,
  and **settled turns plus the in-flight turn's latest report** — the same definition the GUI's
  footer uses.

**Verification:**

```sh
CARGO_TARGET_DIR=target-d03 cargo test -p fleet-daemon store::
CARGO_TARGET_DIR=target-d03 cargo test -p fleet-daemon delegation::
CARGO_TARGET_DIR=target-d03 cargo clippy --workspace --all-targets --all-features -- -D warnings
```

**Regression test:** the equivalence test is the one that matters — build a thread with several
settled turns and one live turn through the existing test harness, then assert the store-computed
`DelegationUsage` equals that thread's `ThreadProjection::{cumulative_usage, cumulative_cost_usd,
context_pct}` field for field. If the two ever drift, this test is what says so. Add a second test
proving `list` fills `usage` for every row and a third proving a child with no turns yields `None`.

**Done when:** `fleet subagent status`'s and `list`'s daemon responses carry the child's usage, the
equivalence test passes, and no code path added here calls `manager.projection()`.

---

### T04 — Keep a resumed delegated child's user environment

**Intent:** A child recovered after its provider exits must come back with the `--env` variables it
was started with, not just Fleet's two identity variables.

**Skill:** `rust-workspace-architecture` (migration discipline — read the schema-migration section
before writing slot 6), then `rust-gpui-testing`. **Docs:** none; T06 writes them.

**Depends on:** T03 committed.

**Owns (exclusively):**
`crates/fleet-daemon/src/services/agents/store/delegations.rs` (inherited from T03),
`crates/fleet-daemon/src/services/agents/store/schema.rs`,
`crates/fleet-daemon/src/services/agents/store/migrations.rs`,
`crates/fleet-daemon/src/services/agents/store/tests.rs`,
`crates/fleet-daemon/src/services/agents/manager.rs`,
`crates/fleet-daemon/src/services/agents/manager/commands.rs`,
`crates/fleet-daemon/src/services/agents/manager/tests/lifecycle.rs`,
`crates/fleet-daemon/src/services/agents/manager/tests/mod.rs`.

**⚠ contradiction with the decision as written.** The decision says "no persistence beyond what
`CreateOptions` already does". That is not sufficient, and the item itself asked for this to be
checked: `services/agents/manager.rs:427-450` rebuilds the resumed `StartRequest`'s `env` **from
scratch**, inserting only a freshly rotated `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN`. Every
user variable is silently dropped on resume — which, for the motivating case, means a recovered
child loses its `CARGO_TARGET_DIR` and starts fighting its siblings over the build lock again, in
the one situation where nobody is watching. Persisting is therefore required, and it is a migration.

**Steps:**

- Add **migration slot 6**, `delegation_env`: `ALTER TABLE delegations ADD COLUMN env_json TEXT`.
  Follow the slot conventions in `store/schema.rs`'s header comment and the `create_if_missing`
  idiom in `migrations.rs`. Slot 5 (`closed_threads`) is the current head; slots are never
  renumbered or edited after shipping.
- Do **not** put the environment on `fleet_core::agents::Delegation`. It would then be echoed in
  every `--json` envelope — the exact context bloat D4 exists to stop — and a user variable may hold
  a secret. Expose it instead as a narrow store read used only by the resume path. Record in the
  commit message that the environment is stored in the same database as the delegation token hash,
  is never put on the wire, never rendered by the CLI and never logged.
- Write it at `insert` from the `RunRequest.env` T01 threaded in, encoded as a JSON object; read it
  back on resume for a thread whose record has a `delegation`.
- In `manager.rs`'s resume path, build `env` as **persisted user variables first, the rotated
  `FLEET_DELEGATION`/`FLEET_DELEGATION_TOKEN` second** — the same precedence `run.rs` uses, for the
  same reason.
- Add the column to `REQUIRED_DELEGATION_COLUMNS` in `schema.rs` so the column-set test protects it,
  and to `DELEGATION_COLUMNS` / `RawDelegation` / `EncodedDelegation` only if you choose to carry it
  through the normal decode path; a separate targeted `SELECT env_json` is smaller and is preferred.
- A row written before slot 6 decodes to an empty map. Assert it.

**Verification:**

```sh
CARGO_TARGET_DIR=target-d04 cargo test -p fleet-daemon store::
CARGO_TARGET_DIR=target-d04 cargo test -p fleet-daemon migrations
CARGO_TARGET_DIR=target-d04 cargo test -p fleet-daemon manager::
CARGO_TARGET_DIR=target-d04 cargo clippy --workspace --all-targets --all-features -- -D warnings
```

**Regression test:** in `manager/tests/lifecycle.rs`, extend or mirror
`a_delegated_child_starts_with_its_extra_environment_and_its_caller` with a resume case: start a
delegated child with a user variable, drive it through the provider-exit resume path, and assert the
resumed `StartRequest.env` still holds that variable **and** a *rotated* delegation token. Plus a
migration test that slot 6 applies to a slot-5 database and that a pre-slot-6 row reads as an empty
environment.

**Done when:** a resumed delegated child receives its user environment and its rotated token, the
slot-6 migration applies cleanly on top of slot 5, and the column-set test names `env_json`.

---

### T05 — Reshape the `fleet subagent` CLI: `--env`, the report body, usage, and an elided brief

**Intent:** Everything an orchestrator actually reads or types. This is where all four reported
items become visible.

**Skill:** `rust-workspace-architecture`, then `rust-gpui-testing` for the pinned-output tests.
**Docs:** T06 writes prose; this task owns clap help text, which is documentation the user reads at
the terminal — keep it in sync with what T06 is writing from this plan.

**Depends on:** T02 committed (for `Client::delegation_wait`'s signature and for `commands/tests.rs`
to be free of T01's fixup). T03 and T04 may still be in flight; this task renders `Delegation.usage`
as `Option`, so it is correct whether or not T03 has landed.

**Owns (exclusively):** all of `crates/fleet-cli/src/` — in particular `args.rs`, `human.rs`,
`envelope.rs`, `commands/subagents.rs`, `commands/tests.rs`.

**Steps — D2, `--env`:**

- Add a repeatable `--env KEY=VALUE` to `SubagentRunArgs` (`#[arg(long = "env", value_name =
  "KEY=VALUE")] pub env: Vec<String>`), parsed into a `BTreeMap<String, String>` in
  `commands/subagents.rs` and passed as `DelegationRunRequest.env`.
- Validate at the CLI, with a `validation(...)` error, not a panic: a value with no `=` is an error;
  an empty key is an error; a **duplicate key** is an error (silently keeping the last would hide a
  typo); a key starting with `FLEET_` is refused, naming the variable; and **`PATH` is refused** —
  it is owned by batch 1's `path_prepend`, and both adapters treat an `env` entry as a *whole-value
  override*, so a `PATH` here would discard the login shell's own rather than extend it
  (`agents/claude/mod.rs`, `agents/codex/mod.rs`). Say that in the error message.
- Do not put cargo-specific advice in the generic delegation footer; that belongs in the
  orchestrator's brief. The clap help may name `CARGO_TARGET_DIR` as an example.

**Steps — D3a, the report body:**

- **⚠ contradiction with the decision as written.** The decision asks where `text`/`elided` are set
  and whether the full text survives, then offers "`status --full` or a higher limit" if the wire
  elides. Neither applies. `reported_result` (`delegation/complete.rs:219`) truncates **at ingest**
  to 256 KiB and nothing keeps the tail; below that cap the store holds the body whole and both
  `wait` and `status` already return it on the wire. The real defect is in rendering: **`fleet
  subagent status` prints a six-field tab-separated line and no body at all** (`human::subagents`,
  `human.rs:195`). That is why an orchestrator went and read report files out of `/tmp` — not
  elision, and not discoverability of `wait`. So: **no `--full` flag and no new wire field.**
- Give `status` its own human renderer instead of sharing `human::subagents` with `list`: the
  fixed-field line, then the brief, then — for a terminal delegation — the child's report body
  rendered through the same `delivered_message` path `wait` uses, so the two agree byte for byte
  and an orchestrator that greps one can grep the other. Keep the `(report elided at N bytes)`
  suffix; it is now the only signal that 256 KiB was exceeded.
- `cancel` keeps sharing `single_delegation`'s JSON path but must **not** acquire the new body
  rendering — its human output is the word `cancelled`, unchanged.

**Steps — D1, usage:**

- `status`: print the child's usage under the fixed-field line — total tokens, input/output,
  cache read/write, `context N%`, and `$X.XX` when `cost_usd` is present. A missing `usage` prints
  nothing rather than zeros.
- `list`: `human::subagents` gains **two** fields on its fixed-field line — total tokens and cost —
  after `duration` and before `delivery`, printing `-` for each when unknown. It is a fixed-field
  contract that `docs/research/agents-contracts.md` describes and `commands/tests.rs` pins;
  changing the field count is deliberate and T06 updates the contract.
- `--json` needs no CLI work: `delegation.usage` is already on the envelope via `Delegation`.

**Steps — D4, the elided brief:**

- Elide `brief` from the `run`, `wait` and `list` JSON envelopes; keep it whole in `status` (and
  therefore `cancel`, which shares the same helper). Human output is unchanged everywhere.
- Do it **at render time in the CLI**, so the wire is untouched: clone the `Delegation`, replace
  `brief` with its first 200 characters on a character boundary, and add
  `brief_elided: bool` to `SubagentEnvelope` and `SubagentsEnvelope`
  (`#[serde(skip_serializing_if = "std::ops::Not::not")]`, camelCase `briefElided`). `PROTOCOL`
  stays `1`; the field is additive and absent when false.
- `complete`'s envelope is out of scope (see *Out of scope*); note the asymmetry in the tracker's
  Follow-ups so the next reader does not think it was missed.

**Steps — wiring:**

- Pass `Some(caller)` to `Client::delegation_wait` from `fleet subagent wait`, resolved the same way
  `run` resolves it: an explicit flag, else `FLEET_SESSION`. Add `--caller <THREAD>` to
  `SubagentWaitArgs` for symmetry with `run`, and **do not** make it required — a `wait` from a
  plain shell with no `FLEET_SESSION` must keep working, it simply consumes nothing.
- Update `validate_context` so `Wait` no longer falls in the "needs nothing" arm if you make the
  caller resolvable; a missing caller is not an error.

**Verification:**

```sh
CARGO_TARGET_DIR=target-d05 cargo test -p fleet-cli
CARGO_TARGET_DIR=target-d05 cargo clippy --workspace --all-targets --all-features -- -D warnings
CARGO_TARGET_DIR=target-d05 cargo run -p fleet-cli --bin fleet -- subagent run --help
CARGO_TARGET_DIR=target-d05 cargo run -p fleet-cli --bin fleet -- subagent wait --help
```

**Regression test:** `crates/fleet-cli/src/commands/tests.rs`. These pinned tests change and the
task must touch every one of them rather than letting a stale literal pass by luck:

- `every_subagent_verb_parses_its_flags_and_defaults` — every `SubagentRunArgs` literal gains `env`,
  every `SubagentWaitArgs` literal gains `caller`, and new cases cover `--env` repeated twice and
  `--caller` on `wait`.
- `subagent_run_accepts_every_shared_permission_mode` — the `SubagentRunArgs` literal gains `env`.
- `subagent_verbs_use_typed_requests_and_render_human_and_json_output` — the `DelegationRun`
  assertion gains `env`, the `DelegationWait` assertion gains `caller`, the `complete` envelope
  literal `json!({"protocol": 1, "delegation": delegation})` is unchanged (complete keeps its
  brief), and the `run`/`wait`/`list` envelopes must now assert `briefElided: true` and a truncated
  `delegation.brief`.
- `subagent_context_uses_environment_fallbacks_and_refuses_a_missing_caller` and
  `subagent_complete_names_each_environment_variable_the_child_is_missing` — re-check if
  `validate_context` changes.
- New tests: one per `--env` refusal (no `=`, empty key, duplicate key, `FLEET_*`, `PATH`), each
  asserting the message names the offending key; one proving `status` prints the report body for a
  terminal delegation and `list` does not; one proving `status`'s body is byte-identical to the one
  `wait` prints for the same record.

**Done when:** `fleet subagent run --env A=1 --env B=2` reaches the daemon with both variables,
`fleet subagent status <id>` prints the child's report and its usage, `fleet subagent run --json`
no longer echoes the brief, and every pinned literal in `commands/tests.rs` has been re-derived
rather than patched around.

---

### T06 — Document all four items across §15, ADR 0017, the contracts doc and the README

**Intent:** Leave `docs/` authoritative rather than descriptive, for a batch whose code lands in
five other commits.

**Skill:** `rust-workspace-architecture` (the ADR and doc conventions section).
**Docs:** this task *is* the docs.

**Depends on:** nothing. Runs from the start, from this plan, which is the specification. Re-read
the shipped code before your final commit and fix anything this plan got wrong — that is the price
of the deviation below.

**Owns (exclusively):** `docs/NATIVE-AGENTS.md`, `docs/decisions/0017-native-subagents.md`,
`docs/research/agents-contracts.md`, `README.md`.

**Steps:**

- `docs/NATIVE-AGENTS.md` §15:
  - §15.2 (*Delivery and exactly once*) gains the `consumed` state: what it means, who sets it, that
    only a `wait` whose `caller` equals the delegation's caller sets it, that the delivery worker
    still patches the transcript row and closes the outbox but sends no user message, and that an
    absent `caller` consumes nothing. Name the guarantee plainly: **a result is delivered to a
    caller at most once, by whichever of `wait` and the worker reaches it first.**
  - §15.1 or §15.3 gains the child-environment rule: `--env` merges under Fleet's identity
    variables, `FLEET_*` and `PATH` are refused, the environment is persisted with the delegation
    and restored on resume, and it is never put on the wire, rendered or logged.
  - A new short subsection, or an addition to §15.4, on **usage**: `status` and `list` report the
    child's **own thread only, descendants excluded**; the numbers are the settled turns plus the
    in-flight turn's latest report, the same definition the GUI footer uses; `fleet agent list` is
    unchanged and why.
  - The **report-body guarantee**: `wait` and `status` both return the child's stored report in
    full; the only loss is a report over 256 KiB, whose tail is discarded at ingest and whose record
    carries `result.elided`. There is no second verb and no `--full`.
- `docs/decisions/0017-native-subagents.md`: a **dated amendment** under *Delivery waits for the
  caller unless eagerness is explicit*, in the ADR's own voice, keeping the original reasoning
  visible — exactly the form `132` and `159` already use. The amendment: end-of-turn injection
  remains the default, and a caller that has already taken the result through its own `wait`
  consumes the delivery, because the reason for injection is that the caller has not seen the
  result, and a caller that waited has.
- `docs/research/agents-contracts.md`: update the `RequestBody` block (`DelegationRun` gains `env`,
  `DelegationWait` gains `caller`), the `fleet_client` signature list (`delegation_wait` now takes
  three arguments), the `Delegation` field list (`usage`), the `DeliveryState` variants
  (`consumed`), the `delegations` DDL (`env_json`, slot 6), and the `fleet subagent` section —
  including the new fixed-field count for `list` and the new `status` output shape.
- `README.md`: the three `fleet subagent` rows for `run`, `wait`, `status` and `list`, with the new
  flags and the new envelope fields.

**Verification:** `make lint` is not a doc check; the verification here is structural. Confirm, and
paste into the tracker, that:

```sh
grep -n "consumed"  docs/NATIVE-AGENTS.md docs/research/agents-contracts.md
grep -n "env_json\|--env"  docs/NATIVE-AGENTS.md docs/research/agents-contracts.md README.md
grep -n "Amended 2026-09-19" docs/decisions/0017-native-subagents.md
```

each return the expected hits, and that `docs/README.md`'s domain assignment still holds for every
file you touched.

**Regression test:** none — this task ships no code. Its correctness is checked by T07, which
re-reads §15, the ADR and the contracts doc against the shipped behaviour.

**Done when:** all four items are documented in every doc that owns part of them, the ADR carries a
dated amendment, and the README's CLI table matches `--help`.

---

### T07 — Close the batch: reconcile, gate, and verify against a private daemon

**Intent:** Make the tracker true, make `SESSION_TODO.md` true, and prove the four items work
against real binaries without touching the shared daemon.

**Skill:** `zed-quality-review` (mandatory — this is the gate), then whichever area skill a finding
sends you to.

**Depends on:** T01–T06 all committed.

**Owns (exclusively):** `SESSION_TODO.md`,
`plans/subagent-feedback-deferred-2026-09-19-tracker.md`, and — for fixes only — any file a finding
points at, after confirming its owning task has committed.

**Steps:**

- Read the diff of the whole batch (`git log --oneline` from T01's commit, then `git diff`) and run
  `zed-quality-review` over it. Record ranked findings in the tracker; fix what the review's rules
  call a defect, and record what you deliberately left.
- Reconcile the three doc seams, the way batch 1's T05 did: (a) `--env` is produced by `fleet-cli`,
  carried by `fleet-client` and `fleet-proto`, bound in `dispatch.rs`, merged in `run.rs`, persisted
  in slot 6 and restored in `manager.rs` — one unbroken chain with no dead code; (b) `consumed` is
  set in exactly one place and read in exactly one place; (c) §15, ADR 0017, the contracts doc and
  the README say the same thing about the report-body guarantee, the usage definition and the
  delivery rule.
- Full gate, on the shared target directory so the artifacts are the ones a human would get:

  ```sh
  make lint
  TMPDIR=/private/tmp/ht make test
  ```

  `make test` is red on this machine for reasons `SESSION_TODO.md` records and A/B proves predate
  this branch. Re-establish the same baseline rather than accepting or arguing: if a suite fails,
  A/B it against `28bbb31` before calling it pre-existing, and write the result down.
- `make harness` is **not** required. Confirm that in the tracker with the reason — nothing in this
  batch renders — rather than omitting it silently.
- **Private-daemon field check**, the batch-1 T05 recipe (its tracker Notes have the details): build
  into a private `CARGO_TARGET_DIR`, boot `fleetd` under a private `FLEET_HOME` with a hand-seeded
  `state.json` naming only this worktree, and **override `FLEET_DAEMON` explicitly** — `FLEET_HOME`
  alone is not enough, and the shared release `fleetd` will otherwise be spawned and apply its
  migrations to your private home. Never run `make restart`. Then assert, per item:
  - **D2:** `fleet subagent run --env CARGO_TARGET_DIR=/tmp/x --env FOO=bar` starts a child whose
    own `printenv CARGO_TARGET_DIR` is `/tmp/x`; `--env PATH=/x` and `--env FLEET_SESSION=x` are
    both refused by name; a duplicate `--env FOO=1 --env FOO=2` is refused.
  - **D3a:** `fleet subagent status <id>` on a terminal child prints the report body, and it is
    byte-identical to what `fleet subagent wait <id>` printed.
  - **D3b:** a caller that `wait`s on its own child sees `delivery: consumed` and, after the worker
    has drained, has **no** duplicate result message in its transcript; a `wait` on the same
    delegation from a shell with no `FLEET_SESSION` leaves an unconsumed delegation delivering
    normally.
  - **D1:** `fleet subagent status <id>` and `list` show the child's tokens and cost, and
    `--json` carries `delegation.usage`.
  - **D4:** `fleet subagent run --json` output does not contain the full brief and carries
    `briefElided: true`; `status --json` still carries it whole.
- Move the four **Deferred** items in `SESSION_TODO.md` into a short **Shipped** note naming the
  commits, with the contradictions that changed the shape of the work (T02's wire field, T03's
  projection cost, T04's migration, T05's `status` finding) recorded there for whoever restarts the
  shared daemon. Leave every other section of `SESSION_TODO.md` untouched — "Found during the batch"
  and "Verify after the first batch lands" are still open and still true.
- Consolidate the tracker: every box ticked, every verification pasted, the decisions log complete.

**Verification:**

```sh
make lint
TMPDIR=/private/tmp/ht make test
```

plus the private-daemon transcript, pasted into the tracker.

**Regression test:** none of its own; T07 is the gate over everyone else's.

**Done when:** `make lint` is green, `make test` is at or better than the recorded baseline with any
difference A/B'd, the private-daemon field check passed every assertion above, `SESSION_TODO.md`'s
Deferred section is empty and its Shipped note names the commits, and the tracker matches reality.

## Verification

Project-wide, run from the repository root:

```sh
make lint                          # cargo fmt --check + clippy --workspace --all-targets --all-features -D warnings
TMPDIR=/private/tmp/ht make test   # builds fleetd/fleet-app/fleet-harness, then cargo test --workspace
```

Per-task, to avoid the cargo build lock in a shared worktree:

```sh
CARGO_TARGET_DIR=target-dNN cargo clippy --workspace --all-targets --all-features -- -D warnings
CARGO_TARGET_DIR=target-dNN cargo test -p <crate> <filter>
```

`make harness` is **not** part of this batch's gate; nothing here renders. T07 records that
decision explicitly.

Forbidden for every task: `make restart`, `make run`, `make daemon`, `fleet daemon restart`.

## Definition of done

- [ ] Every task T01–T07 is ticked in the tracker, with its verification output or a one-line
      "verified: <how>" pasted beside it.
- [ ] `make lint` is green on the shared target directory.
- [ ] `TMPDIR=/private/tmp/ht make test` is at or better than the baseline recorded in
      `SESSION_TODO.md`, and any difference has been A/B'd against `28bbb31`, not argued.
- [ ] Every behaviour change has its doc change on `docs/` (T06), and T07 has confirmed the three
      doc seams agree with the shipped code.
- [ ] The private-daemon field check passed every assertion in T07, against real binaries.
- [ ] `SESSION_TODO.md`'s **Deferred** section is empty and replaced by a Shipped note naming the
      commits; its other sections are untouched.
- [ ] The tracker reflects reality, including the five code/decision contradictions and anything
      that surprised the agents.
- [ ] Follow-ups discovered mid-flight are captured in the tracker's Follow-ups section, not left in
      a commit message.

## Risks and rollback

- **A concurrent agent edits a file it does not own.** This is the failure mode that actually
  happens. Mitigation: the ownership table above is exclusive, the chain T01 → T02 → T03 → T04 is
  strict, and every task's first action is `git log --oneline -5` to confirm its dependency has
  landed. Rollback is per-commit `git revert`; the tasks are independent commits by design.
- **T01 is the long pole and everything waits on it.** It carries three type changes and ~20
  mechanical fixups. Mitigation: the fixup sites are enumerated above and
  `cargo check --workspace --all-targets` is the completion signal. If T01 stalls, nothing else can
  safely start — do not work around it by editing its files.
- **Migration slot 6 is the only irreversible step.** An `ALTER TABLE … ADD COLUMN` cannot be undone
  by reverting the commit: a database that applied slot 6 records it in `fleet_migrations`, and a
  build without the slot will refuse to open it — the exact failure `SESSION_TODO.md` describes for
  slot 5. Mitigation: T04 is late in the chain and its migration is tested against a slot-5
  database first; anyone who applies it to `~/.fleet/agents/state.sqlite` and then needs to go back
  must restore that file from a copy. Take one before the field check.
- **`DeliveryState::Consumed` is a downgrade hazard.** An older `fleetd` opened against a database
  holding a `consumed` row `bail!`s on decode. Same mitigation, same scope: this is a development
  branch and the only database that can hold one is the private one T07 creates.
- **The usage numbers could drift from the GUI's.** Two implementations of one definition is exactly
  how that happens. Mitigation: T03's equivalence test asserts the store-computed value equals
  `ThreadProjection`'s field for field, so a change to `reduce.rs` that is not mirrored fails the
  test rather than shipping a second, quieter truth.
- **`list` grows a per-row store read.** Bounded by the delegation count and served by existing
  indexes, but it is new work on a read path. Mitigation: T03 forbids `manager.projection()`, keeps
  the read to two indexed statements per row, and says why the N is bounded in a comment. If it
  ever is not, the fix is to drop `usage` from `list` and keep it in `status`.
- **Changing `list`'s fixed-field line breaks a parser.** It is a documented contract. Mitigation:
  T06 updates `docs/research/agents-contracts.md` and the README in the same batch, and
  `commands/tests.rs` pins the new shape. Nothing outside this repository consumes it today.
