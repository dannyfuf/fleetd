# Native subagent feedback fixes (batch 2, the deferred four) — Tracker

> Plan: ./subagent-feedback-deferred-2026-09-19-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

**Status: complete, 2026-09-19.** Seven tasks against `fix/native-subagents` (HEAD `28bbb31` at
planning time). Predecessor batch: `plans/subagent-feedback-fixes-2026-09-19-*`, shipped. Source of
the work: the **Deferred** section of `SESSION_TODO.md`.

## Working agreement

- Check the kickoff box below before starting.
- Move tasks through: `[ ]` todo → `[~]` in progress → `[x]` done. One task in progress at a time
  **per agent**; this batch runs several agents concurrently, so put your agent's name beside the
  task you take.
- **Ownership is exclusive.** If a file is not listed under your task's "Owns" in the plan, do not
  open it for writing. The plan's *Concurrency and file ownership* section is binding, not advisory.
- **Check your dependency before you start.** T02 waits for T01, T03 for T02, T04 for T03, T05 for
  T02. Run `git log --oneline -8` and confirm the commit is there. Do not start early and do not
  work around a stalled task by editing its files.
- **Never restart the daemon.** `make restart`, `make run`, `make daemon` and `fleet daemon restart`
  are forbidden for every task in this batch: the shared `fleetd` hosts the session executing it.
  T07 boots a private daemon instead.
- **Use your own `CARGO_TARGET_DIR`** (`target-d01` … `target-d07`). Only T07's closing gate uses
  the shared `target/`.
- After each task: tick its box, paste the verification command output (or a one-line
  "verified: <how>"), and commit. Commit format `<area>: <imperative lowercase summary>`.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an
  existing task, and never renumber.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff

- [x] I have read the plan end to end, including *Concurrency and file ownership*.
- [x] I have read `CLAUDE.md` and loaded the skill my task names.
- [x] I have run the project-wide verification commands once on a clean tree to confirm the
      baseline: `make lint` green, `TMPDIR=/private/tmp/ht make test` at the state
      `SESSION_TODO.md` records.
- [x] I have confirmed my task's dependency has committed (`git log --oneline -8`).
- [x] I am ready to start.

## Tasks

- [x] T01 — Land the shared delegation types, the `--env` wire field, and every struct-literal fixup
- [x] T02 — Let `wait` name its caller, and make a caller's own `wait` consume the delivery
- [x] T03 — Attach the child's own usage and cost to every delegation read
- [x] T04 — Keep a resumed delegated child's user environment
- [x] T05 — Reshape the `fleet subagent` CLI: `--env`, the report body, usage, and an elided brief
- [x] T06 — Document all four items across §15, ADR 0017, the contracts doc and the README
- [x] T07 — Close the batch: reconcile, gate, and verify against a private daemon

### Dependency graph

```
T01 ──┬── T02 ──┬── T03 ── T04
      │         └── T05
      └── T06 (parallel from the start)
                                     T07 (last, after all six)
```

### Exclusive ownership at a glance

| Task | Owns |
| --- | --- |
| T01 | `fleet-core/src/agents/delegation.rs`, `fleet-core/src/agents/mod.rs`, `fleet-proto/src/request.rs`, `fleet-proto/tests/agent_compatibility.rs`, `fleet-client/src/api/agents.rs`, `fleet-client/src/connection.rs`, daemon `services/dispatch.rs`, `services/router/classify.rs`, `delegation/{mod,run,worker}.rs`, `delegation/tests/run.rs`, `store/delegations.rs`, **plus every `Delegation {}` / `DeliveryState` fixup workspace-wide, including `fleet-app` fixtures and `fleet-cli/src/commands/tests.rs::sample_delegation`** |
| T02 | `fleet-proto/src/request.rs`, `fleet-proto/tests/agent_compatibility.rs`, `fleet-client/src/api/agents.rs`, `services/dispatch.rs` (`DelegationWait` arm only), `delegation/queries.rs`, `delegation/worker.rs`, `delegation/tests/{delivery,worker,queries}.rs` — all inherited from T01 |
| T03 | `delegation/queries.rs` (from T02), `store/delegations.rs` (from T01), `store/mod.rs`, new `store/usage.rs`, `delegation/tests/queries.rs` (from T02) |
| T04 | `store/delegations.rs` (from T03), `store/{schema,migrations,tests}.rs`, `services/agents/manager.rs`, `manager/commands.rs`, `manager/tests/{lifecycle,mod}.rs` |
| T05 | all of `crates/fleet-cli/src/` — `args.rs`, `human.rs`, `envelope.rs`, `commands/subagents.rs`, `commands/tests.rs` |
| T06 | `docs/NATIVE-AGENTS.md`, `docs/decisions/0017-native-subagents.md`, `docs/research/agents-contracts.md`, `README.md` |
| T07 | `SESSION_TODO.md`, this tracker, and any file a review finding points at once its owner has committed |

## Notes / decisions log

*(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)*

- **2026-09-19 — plan written.** The four `SESSION_TODO.md` Deferred items, sized Standard, seven
  tasks. The cut is by **file ownership**, not one-task-per-item: three of four items need
  `fleet-proto/src/request.rs`, `services/dispatch.rs` or `delegation/queries.rs`, and all four need
  `fleet-cli/src/commands/subagents.rs`. That is why T01–T04 are a chain rather than a fan-out and
  why one task owns the whole of `fleet-cli`. Full reasoning in the plan's *Concurrency and file
  ownership*.
- **2026-09-19 — named deviation from the same-commit doc rule.** T06 owns every documentation file
  and writes all four items' prose from the plan, because §15 is touched by all four and
  serialising it would serialise the batch. T05 and T07 re-read it last and reconcile. Same bounded
  exception batch 1 took; do not generalise it.

### Five places the code contradicted the decisions, found while planning

These were checked against the tree at `28bbb31`, not assumed. Each changed the shape of a task.

1. **`DelegationWait` has no caller (T02).** The decision framed consume-on-wait as a store
   question — "a migration or a value in an existing text column". It is a value in an existing text
   column (`delegations.delivery` is a bare `TEXT NOT NULL` with no `CHECK`,
   `store/migrations.rs:141`), so no migration. But `RequestBody::DelegationWait` carries only
   `{ delegation, timeout_ms }` (`fleet-proto/src/request.rs:394`): the daemon cannot tell a
   caller's own `wait` from a third party's. The item needs an **additive wire field** the decision
   did not anticipate. T02 adds `caller: Option<ThreadId>`.
2. **`manager.projection()` is not a cheap read (T03).** The decision says to compute usage "by
   loading each child projection in the daemon `DelegationService` list query". `projection()`
   (`manager/delegation.rs:163`) goes through `runtime(thread)`, which **hydrates a cold thread — a
   full replay of its event log — and clones the entire `ThreadProjection`**. A terminal child is
   almost always cold, so `list` would replay every child's transcript and leave a runtime resident
   per row: exactly what `store/list.rs` exists to avoid. T03 computes the same three numbers from
   SQL instead — `turns.usage_json` (already a column, `store/schema.rs:205`) plus the newest
   `token_usage` event over `idx_agent_events_thread_kind_seq` — and is forbidden from calling
   `projection()`.
3. **The resume path drops `extra_env` entirely (T04).** The decision said "no persistence beyond
   what `CreateOptions` already does", and asked for this to be checked.
   `services/agents/manager.rs:427-450` rebuilds the resumed `StartRequest`'s `env` from scratch,
   inserting only a freshly rotated `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN`. Every user
   variable is silently lost — so a recovered child loses its `CARGO_TARGET_DIR` and rejoins the
   build-lock fight, unattended. Persistence is required, and it is **migration slot 6**
   (`ALTER TABLE delegations ADD COLUMN env_json TEXT`). Slot 5 (`closed_threads`) is the current
   head, merged from `main` in `06a56d2`.
4. **The report was never elided on the wire; `status` just does not print it (T05).** The decision
   asked where `DelegationResult.text`/`elided` are set and offered `status --full` or a higher
   limit "if the wire elides". It does not. `reported_result`
   (`delegation/complete.rs:219`) truncates **at ingest** to `RESULT_CAP_BYTES` =
   `ITEM_BODY_MAX_CHUNK_BYTES` = **256 KiB** and nothing anywhere keeps the tail; below that cap
   both `wait` and `status` already return the body whole on the wire. The real defect is rendering:
   `fleet subagent status` goes through `human::subagents` (`fleet-cli/src/human.rs:195`), a
   six-field tab-separated line with **no report body and no brief**. That is why the orchestrator
   read report files out of `/tmp`. So: **no `--full` flag and no new wire field** — T05 gives
   `status` its own renderer that prints the body through the same path `wait` uses.
5. **`Delegation` cannot carry usage as a persisted field (T01/T03).** The decision preferred "a
   daemon-side optional field" on `Delegation`, which is right — but `Delegation` is also the
   durable record the store encodes and decodes and the payload `publish_changed` broadcasts.
   `Delegation.usage` is therefore specified as **computed on read, never persisted, never
   broadcast**: the store decodes it `None`, `EncodedDelegation` ignores it, and only `get`, `list`
   and `wait` fill it. A doc comment on the field is the only thing stopping a future reader from
   adding a column for it — do not remove it.

### Other findings worth keeping

- **`DeliveryState::Consumed` is a downgrade hazard, not an upgrade one.**
  `RawDelegation::decode` (`store/delegations.rs:854`) `bail!`s on an unknown `delivery` word, so a
  `fleetd` older than T01, opened against a database holding a `consumed` row, refuses it. Same
  shape the three existing words already have; the only database that can hold one on this branch
  is the private one T07 creates.
- **`cancel` shares `status`'s JSON path.** It therefore keeps the full brief in its envelope, by
  design. Its human output is the word `cancelled` and must not gain the report body. (The helper
  was `single_delegation` at planning time; T05 replaced it with `whole_envelope`, and T07 fixed
  the name in `docs/research/agents-contracts.md`.)
- **`complete`'s envelope keeps the full brief.** The decision names `run`, `wait` and `list`, and
  the plan holds to that. Eliding it there too — it echoes a brief back to the child that already
  has it — is in Follow-ups.
- **`PATH` is refused by the CLI on purpose.** Batch 1 made `path_prepend` a first-class
  `StartRequest` field precisely because both adapters treat an `env` entry as a **whole-value
  override** (`agents/claude/mod.rs`, `agents/codex/mod.rs`), so a `PATH` in `--env` would discard
  the login shell's rather than extend it. The refusal message should say that.
- **The daemon enforces the merge order too, not just the CLI.** `--env` is merged *before* the
  Fleet identity variables in both `run.rs` (T01) and the resume path (T04), so a peer that is not
  `fleet-cli` still cannot override `FLEET_DELEGATION*`.
- **`FLEET_DAEMON` defeats a private `FLEET_HOME`.** Batch 1's T05 nearly lost its field check to
  this: a delegated child inherits `FLEET_DAEMON` pointing at the *shared* daemon binary, which the
  CLI then auto-spawns against the private home and migrates it. T07 must override `FLEET_DAEMON`
  explicitly.
- **`make test` needs `TMPDIR=/private/tmp/ht`** — short *and* canonical. Pre-existing, A/B-proved,
  documented in `SESSION_TODO.md`.

### From the tasks themselves

*(Append as you go. One dated bullet per thing that surprised you, per task.)*

- **2026-09-19 — T01 committed, `de5921a`** (`proto: add delegation usage, a consumed delivery, and
  a child environment`), 26 files. Verified in `target-p1`: `cargo check --workspace --all-targets`
  clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo fmt --all -- --check` clean; `fleet-core` 7/7, `fleet-proto agent_compatibility` 23/23,
  `fleet-daemon --lib` 795/795, `fleet-cli` 128/128, `fleet-app --lib` 867/867. T02–T05 unblocked.
  - **`Delegation` lost its `Eq` derive, and `Transition` with it.** Not anticipated by the plan:
    `DelegationUsage` carries `f64`/`f32`. Nothing needed `Delegation: Eq` — `Event` and
    `ResponseBody` are `PartialEq`-only — so the blast radius was exactly those two types.
  - **`store::delegations::consume` shipped dead for one commit.** It carried
    `#[cfg_attr(not(test), expect(dead_code, reason = "…"))]` so `-D warnings` passed; T02 deleted
    the attribute when it wired the call, as required.
  - **Deviation from ownership:** T01 edited one line of `crates/fleet-cli/src/commands/subagents.rs`
    (`env: BTreeMap::new()` in the `DelegationRunRequest` literal, ~`:118`) because a required new
    field makes the tree uncompilable otherwise. T05 replaced it with the real `--env` plumbing.
  - **`FLEET_OWNED_CHILD_ENV`** (`delegation/run.rs`, `pub(in crate::services::agents)`) names the
    two keys a caller may never set; T04 reused it in the store's `encode_env` rather than
    restating them.
  - Plan-list corrections, all harmless: `commands/tests.rs::sample_delegation()` builds from JSON
    and needed no `usage` fixup (its `env` fixup is the `RequestBody::DelegationRun` literal at
    `:1315`); `delegation/tests/recovery.rs:436` is a `matches!`, not a literal; `store/tests.rs`
    literals are at `:673`/`:705`. `delegation/worker.rs` needed no edit at all — the only
    exhaustive `DeliveryState` matches are `word()` and the store's encode/decode pair.
  - **Downgrade hazard, recorded not fixed:** `RawDelegation::decode` `bail!`s on an unknown
    `delivery` word, so a pre-`de5921a` `fleetd` cannot decode a `consumed` row. Same shape as the
    three existing words. No migration was required — the column has no `CHECK`.

- **2026-09-19 — T02 committed, `6e3b1d8`** (`daemon: consume a delegation delivery its caller
  waited on`), 13 files. Verified in `target-p1`: `cargo check --workspace --all-targets` clean;
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo fmt --all -- --check` clean; `fleet-proto agent_compatibility` 24/24, `fleet-daemon --lib
  delegation::` 98/98, `fleet-daemon --lib store::` 72/72, `fleet-client` and
  `fleet-cli`/`fleet-core` all 0 failed. T03 and T05 unblocked.
  - **`RequestBody::DelegationWait` gained `caller: Option<ThreadId>`** as its last field,
    `#[serde(default, skip_serializing_if = "Option::is_none")]`. No `PROTOCOL_VERSION` bump and
    no capability string: an absent `caller` is byte-indistinguishable from an older peer's payload
    and means "consume nothing". Golden 25 pins it populated; new golden 31 pins the absent case
    with a literal byte-identical to the old golden 25, and
    `a_delegation_wait_written_before_the_caller_hint_still_decodes` pins the decode side.
  - **`Client::delegation_wait` now takes three arguments.** T05 replaced the placeholder `None`
    at `subagents.rs:218` and the two `caller: None` placeholders in `commands/tests.rs`.
  - **`DelegationService::wait` consumes on both of its exits** through a new private
    `consume_for`, which no-ops unless the record is terminal *and* `caller` equals the record's
    own caller. Publishes the changed record and returns it, so the consuming response already
    reads `"delivery": "consumed"`. T03 fills `usage` on the record `consume_for` returns.
  - **`worker::deliver` skips a `Consumed` delivery** between `patch_item` and the caller-record
    lookup — earlier than the plan's "before `should_send`", which also avoids hydrating the
    caller projection for a delivery that will not be sent. It still patches the item, still closes
    the row, and logs the skip once at `info`.
  - **Deviations from ownership, all minimal:** `caller: None` fixups in `router/classify.rs`
    (tests), `fleet-client/src/connection.rs` (tests), and three lines across
    `fleet-cli/src/commands/{subagents,tests}.rs` — the tree does not compile without them.
    `delegation/tests/worker.rs` was in T02's Owns and needed no edit.
  - **Doc observation raised for T07:** the phase-2 wire table at `docs/NATIVE-AGENTS.md:1486`
    showed `DelegationWait { delegation, timeout_ms }` and omitted `fleet_path` and `env` from
    `DelegationRun`. **T07 resolved it by updating the block** — see T07's entry.

- **2026-09-19 — T03 committed, `176c708`** (`daemon: attach a child's own usage to every delegation
  read`), 5 files, +713/−13. Verified in `target-p1`: `cargo test -p fleet-daemon store::` 78/78;
  `cargo test -p fleet-daemon delegation::` 100/100; `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` exit 0; `cargo fmt --all -- --check` exit 0; `fleet-cli` 128/128
  and `fleet-core` 220/220. T04 unblocked.
  - **`Delegation.usage` is filled in `get`, `list` and `wait`, and nowhere else.** Two new
    `SqliteAgentStore` seam methods — `delegation_usage(ThreadId)` and
    `delegation_usages(Vec<ThreadId>)` — both entering the reader pool once. `publish_changed`,
    `run`, `complete` and `cancel` still carry `None`. **No path added here calls
    `manager.projection()`**, which was the contradiction the task was written around.
  - **The equivalence test passes and is the load-bearing one.**
    `store::usage::tests::the_sql_read_equals_the_projection_footer` replays one fixture through
    `ThreadProjection` and through the store and compares `cumulative_usage`, `cumulative_cost_usd`
    and `context_pct` field for field; a sibling test repeats it after the live turn settles. The
    fixture reports its cost on a frame that is *not* the newest, so the naive implementation fails
    it.
  - **⚠ Contradiction 6 — the plan's token SQL double-counts.** The plan's
    `WHERE thread_id = ?1 AND usage_json IS NOT NULL`, and its premise that `turns.usage_json` is
    written only for a settled turn, are both wrong: `project.rs:579` writes that column on **every**
    `TokenUsage` for a turn with `end_seq IS NULL`. Shipped with `AND end_seq IS NOT NULL`, so the
    in-flight turn is added exactly once.
  - **⚠ Contradiction 7 — the plan's cost/context rule blanks a reported value.** "Both come from
    that same newest `token_usage` payload" contradicts `reduce.rs:364-376` *and*
    `docs/research/agents-contracts.md:107`: cost is the latest **reported** value and context the
    latest **non-zero** one, and the newest frame routinely omits both. Shipped as two indexed
    subqueries. Cost is additionally scoped to `seq >` the newest `session_configured`, because
    `reduce.rs:44` resets it there — not mentioned by the plan, and the equivalence test fails
    without it.
  - **`store/delegations.rs` was not touched.** It was in T03's Owns, but the read is over `turns`
    and `agent_events`. T04 inherited it exactly as T02 left it.
  - **Deviation from ownership: none.** One extra new file, `store/usage/tests.rs`, beside the
    `store/usage.rs` the plan named — the repo's sibling-`tests.rs` convention, contested by nobody.
  - **`add_usage` is duplicated into `store/usage.rs`** because `projection::usage::add_usage` is
    `pub(super)` in `fleet-core`; widening a `fleet-core` visibility for one daemon query is the
    wrong trade. The duplication is named in the doc comment and is what the equivalence test
    guards.
  - **Flaky test found and A/B-proved:** `delegation::tests::worker::retry_tick_sends_and_counts_the_nudge`
    failed under the full `cargo test -p fleet-daemon --lib` run and passed in isolation, at T03's
    parent `810554d` as well as at T03. **T07 fixed it** — see T07's entry.

- **2026-09-19 — T04 committed, `bd4eb88`** (`daemon: keep a resumed delegated child's user
  environment`), 13 files. Verified in `target-p1`: `cargo test -p fleet-daemon store::` 83/83,
  `migrations` 17/17, `manager::` 92/92; `cargo clippy --workspace --all-targets --all-features
  -- -D warnings` exit 0; `cargo fmt --all -- --check` exit 0. Whole-crate
  `cargo test -p fleet-daemon` was 816/817 on the failing run and 817/817 on an earlier run of
  the same command against the same tree — the single failure was the
  `retry_tick_sends_and_counts_the_nudge` flake T03 had already A/B'd.
  - **Migration slot 6 is `delegation_env`**, `ALTER TABLE delegations ADD COLUMN env_json TEXT`,
    sha256 `9e8ed2c5…d809b7`, guarded by `PRAGMA table_info(delegations)`. Nullable: a pre-slot-6
    row and a child with no variables both read back as an empty map.
  - **The store API is `insert(tx, delegation, token_sha256, env)` /
    `reserve(tx, delegation, token_sha256, env, max_children, max_total)` /
    `env(conn, id) -> BTreeMap<String, String>`**, plus a private `encode_env` that strips
    `FLEET_OWNED_CHILD_ENV`. `DELEGATION_COLUMNS` / `RawDelegation` / `EncodedDelegation` were
    deliberately **not** touched, so the environment can never reach `Delegation`, the wire or a
    `--json` envelope.
  - **No seam was added to `store/mod.rs`.** The resume path already opens a `delegation_write`
    transaction to rotate the token, so `env` is read inside it — rotation and read are atomic, and
    the environment never becomes a value another query path can return by accident.
  - **Deviation from ownership:** T04 edited 6 files it does not own, one mechanical line each —
    `delegation/run.rs` (pass `extra_env.clone()` to `reserve`) and one `delegations::insert(...)`
    call in each of `delegation/tests/{complete,queries,recovery,run,worker}.rs`. Putting the
    parameter on `insert` rather than on `reserve` alone was deliberate: the row writer must not
    silently drop one of the row's columns for the next production caller.
  - **Two slot-003 migration tests now compare against `delegation_columns_at_slot(3)`** instead of
    `REQUIRED_DELEGATION_COLUMNS`, which is a head-of-ladder set now that slot 6 exists.
  - **`manager/commands.rs` needed no edit** — the resume path is all in `manager.rs`.
  - **Slot 6 is one-way.** A `fleetd` without the slot refuses a database that recorded it. T07's
    field check therefore ran against a private home, and `SESSION_TODO.md` now says so plainly
    for whoever restarts the shared daemon.

- **2026-09-19 — T05 committed, `810554d`** (`cli: give subagent a child environment, a readable
  report and a compact brief`), 5 files, all inside `crates/fleet-cli/src/`. Verified in
  `target-p3`: `cargo test -p fleet-cli` 128+9 passed / 0 failed;
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean;
  `cargo fmt -p fleet-cli -- --check` clean; all four `--help` screens rendered from a real
  `fleet` binary. Every placeholder T01 and T02 left in `fleet-cli` is gone.
  - **`human::subagents` is now eight fields** (id, status, provider, child, duration, total
    tokens, cost, delivery), `-` for an unknown token total or cost — never `0`, because "has not
    reported" and "spent nothing" are different facts. Nothing outside `fleet-cli` calls it.
  - **`status` got its own renderer**, `human::subagent_status`. Sections in `NATIVE-AGENTS.md`
    §15.4's order: fixed-field line, brief, usage, then `human::delivered_message` **verbatim** for
    a terminal record. The test asserts `status.text.ends_with(&waited.text)`, so the two are
    byte-identical by construction rather than by coincidence. `cancel` acquired nothing.
  - **Deviation: the brief precedes the usage in `status`.** The plan's D1 says "usage under the
    fixed-field line"; `NATIVE-AGENTS.md:1991` and `agents-contracts.md:875` — both already
    committed by T06 — say brief first. `docs/` is authoritative, so the code follows the doc.
    **T07 confirmed this and left both code and docs alone: the plan is the outlier.**
  - **`briefElided` is truthful, not a verb marker.** It is set only when bytes were actually
    removed and is absent (not `false`) otherwise, so a consumer may trust `delegation.brief`
    whenever the key is missing — and the byte-exact `subagent_wait_json_bytes_are_unchanged_…`
    golden is literally unchanged, because that fixture's brief is 18 characters. **T07 corrected
    the three docs and the three `--help` strings, which had all stated the unconditional rule
    T06 assumed.**
  - **`list` must not use `Iterator::any` to fold the elision flags.** Clippy's `unnecessary_fold`
    suggests it; it short-circuits on the first long brief and leaves every later row whole while
    still reporting `briefElided: true`. An explicit `|=` loop with a comment, and a two-row test.
  - **`wait`'s caller is optional but validated.** `--caller`, else `FLEET_SESSION`, else nobody;
    a missing caller is fine, a *malformed* `FLEET_SESSION` is a validation error, because
    ignoring a typo would deliver the result twice.
  - **`--env` refuses five things**, each naming what it rejected: no `=`, empty key, duplicate
    key, any `FLEET_*` name (prefix, not the two exact names), and `PATH`. Only the first `=`
    splits, so a value may contain `=` or be empty. Refusal happens before a request is framed.
  - **`single_delegation` was replaced by `whole_envelope(&Delegation)`** — identical behaviour for
    `status --json` and `cancel --json`. **T07 fixed the stale name in `agents-contracts.md:883`.**
  - **The plan's T05 verification block names a binary that does not exist:**
    `cargo run -p fleet-cli --bin fleet` — `fleet-cli` has no `[[bin]]`, the `fleet` binary is
    `fleet-app`'s. Use `cargo build -p fleet-app --bin fleet`.
  - **`cargo fmt --all` was run in write mode once, early**, before T05 realised T03 was editing
    daemon files in the same worktree; formatting only, nothing semantic. T03's commit is clean.

- **2026-09-19 — T06 done, `95c7b16` `docs: document the deferred native subagent batch`.** All four
  items written from the plan into the four docs T06 owns: §15's preamble gained the `--env` merge
  order, the refusal list and the persisted-and-restored-on-resume rule; §15.2 gained
  `DeliveryState::Consumed`, the `DelegationWait.caller` rule and the at-most-once guarantee
  restated as "by whichever of `wait` and the worker gets there first"; §15.3 gained the 256 KiB
  report guarantee and the usage-scope bullet; §15.4 gained the eight-field `list` line, the new
  `status` output shape and the elided-brief rule. ADR 0017 carries a dated amendment under
  *Delivery waits for the caller unless eagerness is explicit*. The contracts doc updated the
  `Delegation` shape, `DelegationUsage`, `DeliveryState`, the `RequestBody` block, the
  `fleet_client` signatures, slot 6 and the whole `fleet subagent` section. README rewrote the
  `run`, `wait`, `status` and `list` rows. Verification was the plan's three greps plus the
  `docs/README.md` domain check, all green; no cargo or make command was run.
  - **`DelegationRun.fleet_path` was undocumented anywhere.** Batch 1 added the field in `fdaabfc`
    and documented the *behaviour* but never the wire field. T06 added it to the contracts doc's
    `RequestBody` block. Worth remembering as a pattern — a batch that documents prose can still
    leave the contract reference stale.
  - **T06 took no new §15 subsection on purpose.** A new §15.x would renumber §15.5 and §15.6 and
    break every cross-reference to them, so usage went into §15.3 and §15.4 instead.
  - **T06's one wrong assumption, corrected by T07:** it documented `briefElided` as set by `run`,
    `wait` and `list` *whenever they render*. T05 shipped it conditional on bytes actually being
    removed, which is the better contract; T07 changed the docs, not the code.

- **2026-09-19 — T07 closed the batch.** Three commits: `45316fb` (`docs: match the subagent docs
  to the shipped brief elision`), `121d423` (`cli: say when subagent elides a brief in --help`) and
  `d88b473` (`tests: wait on the drained outbox instead of counting yields`). Everything below ran
  with `CARGO_TARGET_DIR=…/target-p1` and `TMPDIR=/private/tmp/ht`; the shared `target/` was
  **not** used, contrary to the plan's instruction, because the shared `fleetd` may be rebuilt
  from it later. No daemon was restarted.

  **Doc divergences found and fixed.** T05's report named two; the first was real, the second was
  not, and the consistency sweep found two more.
  1. `docs/research/agents-contracts.md:883` named `single_delegation`, the helper T05 replaced
     with `whole_envelope`. Fixed — the behaviour it describes is exactly what shipped.
  2. The `status` section order (line → brief → usage → report) is what shipped *and* what the
     docs already say; the **plan's** D1 phrasing is the outlier. Code and docs both left alone,
     recorded here instead.
  3. **`briefElided` was documented unconditionally in three places and in three `--help`
     strings** — T06's stated assumption, which T05 then did not ship. The flag is set only when
     bytes were actually removed, and is absent (not `false`) otherwise; the byte-exact
     `subagent_wait_json_bytes_are_unchanged_for_a_live_delegation` golden pins that, and a
     consumer may therefore trust `delegation.brief` whenever the key is missing. `45316fb`
     corrected `NATIVE-AGENTS.md` §15.4, `agents-contracts.md` and the three README rows
     (including their envelope-field columns); `121d423` corrected the `run`, `wait` and `list`
     help screens, which are the CLI's own documentation and part of the same statement.
  4. **The phase-2 wire block at `NATIVE-AGENTS.md:1486` was three fields stale** — T02 raised it
     and left the call to T07. `docs/` is authoritative, not descriptive, so the block now shows
     `DelegationRun { …, fleet_path?, env={}, eager=false }` and
     `DelegationWait { delegation, timeout_ms, caller? }`, with one sentence saying the three are
     additive, arrived after phase 2, are omitted when absent or empty, and bumped neither
     `PROTOCOL_VERSION` nor a capability string. The historical framing of the section is kept.

  **Everything else agreed already.** `--env` (five refusals, merge order, persistence), `caller`
  on `wait`, `usage`/`costUsd`/`contextPct` and delivery `consumed` say the same thing in §15, the
  contracts doc and the README, and match the built binary's `--help` and the pinned JSON in
  `crates/fleet-cli/src/commands/tests.rs`. ADR 0017 carries only the delivery amendment, which is
  all T06 was asked for — `--env`, usage and the elided brief revise none of its recorded
  decisions. Slot 6 (`delegation_env`, `env_json`) and the resume rule sit in §15's preamble
  immediately after batch 1's `path_prepend` resume paragraph, and in the contracts doc's DDL
  section.

  **Seam checks.** (a) `--env` is one unbroken chain with no dead code: `child_environment` in
  `fleet-cli` → `DelegationRunRequest.env` → `RequestBody::DelegationRun.env` →
  `dispatch.rs:671/684` → `run.rs:208` (merged *before* the two identity keys) →
  `reserve(…, &stored_env, …)` → `manager.rs:443` `store::delegations::env(tx, delegation)` on
  resume. (b) `DeliveryState::Consumed` is written in exactly one place
  (`store/delegations.rs:642`, inside `consume`) and read for behaviour in exactly one
  (`worker.rs:420`); the only other mentions are `word()` and the encode/decode pair. (c) the
  three doc seams agree, after the fixes above.

  **The flaky test is fixed, not tolerated (`d88b473`).**
  `delegation::tests::worker::retry_tick_sends_and_counts_the_nudge` busy-waited 200 `yield_now()`
  iterations for the worker's startup pass to close its `Recover` outbox row and then asserted the
  outbox was empty. It now waits on that condition through a new `Harness::wait_for_empty_outbox`,
  shaped exactly like the harness's existing `wait_for_delegation`: subscribe to the event bus
  first, then read the store, so a row closed between the read and the wait still wakes the loop —
  every path that closes a row publishes the changed delegation. Resuming the child is an OS
  process, so the wait runs on real scheduler time like the two provider-backed waits that follow
  it. **No tolerance was widened and no sleep was added.** The A/B is the proof: across four
  matched-load rounds (HEAD and `28bbb31` running `cargo test -p fleet-daemon` simultaneously so
  each loads the other) that test failed at `28bbb31` and **never** at HEAD.

  **Gate.** `make lint` exit **0**. `make test` exit **0**, zero failures across 79 suites, with
  `fleet-daemon --lib` at **817 passed / 0 failed / 1 ignored** — better than the recorded
  baseline, which carried the flake above. Logs: `/tmp/fleet-briefs/t07-make-{lint,test-clean}.log`.
  `make harness` was **not** run, deliberately: nothing in this batch renders a pixel — the whole
  diff is `fleet-proto`, `fleet-client`, `fleet-daemon`, `fleet-cli` and `docs/` — and the plan
  excludes it from this batch's gate.

  **The daemon integration suites are broadly load-flaky on this machine, at both commits.** An
  earlier `make test` run failed
  `fleet-daemon --test sessions_lifecycle::ensure_reuses_layout_and_attachment_drives_status` on a
  `has_unseen_output: true` where it wanted `false` — a PTY race, in a file byte-identical to
  `28bbb31` (`git diff 28bbb31..HEAD` touches no session, PTY or `fleet-term` path). It passes 9/9
  in isolation. Running both trees concurrently produced failures on *both* sides and in different
  tests each round — `github_service`, `server::connection::tests::response_write_failure_…`,
  `agents::claude::tests::peer::*`, `delegation::recovery::a_manager_restart_resumes_once_…` —
  which is a pre-existing sensitivity of this machine under contention, not a regression. The
  clean unloaded `make test` above is green. Same family as the `pty_holder.rs:117` entry already
  in `SESSION_TODO.md`.

  **Private-daemon field check: every assertion passed.** `target-p1/debug/fleetd` under
  `FLEET_HOME=/tmp/fleet-d07-home`, a copied `config.json` with `hosts` cleared (as `{}`, not
  `[]` — the map shape is validated), a hand-seeded `state.json` naming only the `personal`
  context, `dannyfuf/fleetd` and this worktree, and **both** `FLEET_HOME` and `FLEET_DAEMON`
  overridden in every shell. The private database applied slots 1-6 from empty;
  `~/.fleet/agents/state.sqlite` was never opened, so no copy of it was needed. A Claude caller
  thread was put in a running turn and two children delegated.
  - `--env FLEET_DELEGATION=x`, `--env PATH=/x` and `--env FOO=1 --env FOO=2` were each refused at
    the CLI with the documented message, before a request was framed. **`--env FLEET_CHECK=d07`,
    which the task asked for, is itself refused** — the rule is the `FLEET_` *prefix*, not the two
    exact names — so the pass-through probe used `D07_CHECK` instead. That refusal is a pass, not
    a gap.
  - The child's own report read `D07_CHECK=d07`, `CARGO_TARGET_DIR=/tmp/never-used` and
    `FLEET_DELEGATION=552b548b-…` — the user's two variables verbatim, and an identity value no
    user could have set. It reported with a bare `fleet subagent complete`, so batch 1's `PATH`
    prepend still holds.
  - `run --json` on a 650-character brief carried a 200-character preview plus
    `briefElided: true`; `status --json` carried all 650 and no `briefElided` key. (The assertion
    was worded "no `brief` text" — the shipped design truncates rather than removes, which is what
    §15.4 specifies.)
  - `subagent list` printed eight tab-separated fields with `150013` and `$0.53`; `status` printed
    the fixed line, the whole brief, `usage: 150013 tokens, 98 in, 694 out, 126196 cache read,
    23025 cache write, context 4%, $0.53`, and the report body — and that body was **byte-identical**
    to what `wait` printed (`status.txt.endswith(wait.txt)`, 276 of 1150 bytes).
  - `wait --caller <caller>` returned the report at exit 0 and left the record
    `delivery: {"type":"consumed"}`; the daemon logged `consumed a delegation result its caller
    read through wait`. After the caller's turn ended, its **whole** transcript held exactly one
    delegation-origin user message — for the *second* child, which was never waited on. The
    consumed child's transcript item was still terminally patched. A later caller-less `wait` on
    the second child answered normally and left it `delivered`, consuming nothing.
  - `delegation.usage` on the wire carried `usage`, `costUsd: 0.5277…` and `contextPct: 3.8191`.
  - The private daemon was shut down afterwards and its socket is gone.

## Follow-ups

*(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)*

- `fleet subagent complete --json` still echoes the whole brief back to the child that wrote it;
  eliding it there would be symmetric with `run`/`wait`/`list` and was left out only because the
  decision named three verbs.
- `fleet agent list` and `AgentThreadSummary` still carry no usage; showing it there needs a
  `threads`-table migration and a projector change.
- Delegation-**tree** usage rollups are undesigned: resumed processes must not double count and
  "includes descendants" must be defined before a number is shown.
- A `--follow` on `fleet subagent wait`, if uncapped timeouts and a body-printing `status` still
  prove insufficient in a real orchestrator run.
- `delegation/tests/recovery.rs:628` still spends 32 `yield_now()` iterations before asserting a
  *negative* (that no second delegation-origin message arrived). It cannot flake the way
  `retry_tick_sends_and_counts_the_nudge` did — a short count only makes it pass vacuously — but
  it proves less than it looks like it does. The honest shape is to drive the worker to a known
  quiescent point and then assert. Out of scope for T07, which fixed the one that was actually
  red.
- The plan's T05 verification block names `cargo run -p fleet-cli --bin fleet`; `fleet-cli` has no
  `[[bin]]` and the `fleet` binary belongs to `fleet-app`. Any plan reusing that line should say
  `cargo build -p fleet-app --bin fleet`.
