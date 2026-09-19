# Native subagent feedback fixes (batch 2, the deferred four) — Tracker

> Plan: ./subagent-feedback-deferred-2026-09-19-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

**Status: not started, 2026-09-19.** Seven tasks against `fix/native-subagents` (HEAD `28bbb31` at
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

- [ ] I have read the plan end to end, including *Concurrency and file ownership*.
- [ ] I have read `CLAUDE.md` and loaded the skill my task names.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm the
      baseline: `make lint` green, `TMPDIR=/private/tmp/ht make test` at the state
      `SESSION_TODO.md` records.
- [ ] I have confirmed my task's dependency has committed (`git log --oneline -8`).
- [ ] I am ready to start.

## Tasks

- [ ] T01 — Land the shared delegation types, the `--env` wire field, and every struct-literal fixup
- [ ] T02 — Let `wait` name its caller, and make a caller's own `wait` consume the delivery
- [ ] T03 — Attach the child's own usage and cost to every delegation read
- [ ] T04 — Keep a resumed delegated child's user environment
- [ ] T05 — Reshape the `fleet subagent` CLI: `--env`, the report body, usage, and an elided brief
- [ ] T06 — Document all four items across §15, ADR 0017, the contracts doc and the README
- [ ] T07 — Close the batch: reconcile, gate, and verify against a private daemon

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
- **`cancel` shares `single_delegation` with `status`.** It therefore keeps the full brief in its
  JSON envelope, by design. Its human output is the word `cancelled` and must not gain the report
  body.
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
