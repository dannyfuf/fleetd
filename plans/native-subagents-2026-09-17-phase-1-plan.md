# Native subagents, phase 1: prerequisites in the manager and reducer — Plan
> Tracker: ./native-subagents-2026-09-17-phase-1-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Three small changes the delegation service (phase 3) will rely on and that are wrong or missing
today. The harness answer `Submitted.queued` conflates "joined the running turn" with "queued as a
separate turn", and the manager records every `queued` as a steer bubble. The reducer closes every
open item of a turn when the turn settles, including Claude background tasks that are still live,
which the Claude adapter deliberately keeps open. And nothing records whether a thread stopped because
the user pressed Stop or because the provider died, which delivery-on-resume needs to tell apart.
None of this is user-visible; all of it is testable with the fake harness.

## Sizing call

**Phased, phase 1 of 7.** See ./native-subagents-2026-09-17-roadmap.md. On its own this phase is
Standard: three independent, mechanical changes in `fleet-daemon` and `fleet-core`, one pull request.
It is split out because every later phase depends on it and because it is the one phase a reviewer
can approve without reading the design.

## Repository context

- Rust workspace, edition 2024, toolchain pinned by `rust-toolchain.toml` (Rust 1.97.1). Twelve
  crates under `crates/`; this phase touches `fleet-core` and `fleet-daemon` only.
- Lint: `make lint` (`cargo fmt --all -- --check` then
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`). Clippy also denies
  `todo`, `unimplemented`, `dbg_macro`.
- Test: `make test` (builds `fleetd`, `fleet`, `fleet-harness`, then `cargo test --workspace`).
  Targeted: `cargo test -p fleet-core`, `cargo test -p fleet-daemon`.
- Type check: `cargo check --workspace --all-targets` (`make check`).
- The harness seam is `crates/fleet-daemon/src/agents/harness/mod.rs`; `Submitted` is defined there
  at line 183, not in `fleet-core` as the design doc says.
- The manager's send path is `crates/fleet-daemon/src/services/agents/manager/commands.rs` around
  lines 271 to 354. The write path is `manager/apply.rs`; `update_record` at line 276 is where a
  record learns from an event.
- The reducer is `crates/fleet-core/src/agents/projection/reduce.rs`; `close_open_items` at line 430
  is the turn-settle closure. `background_tasks` lives on `ThreadProjection`
  (`projection/mod.rs:188`).
- The store projector is `crates/fleet-daemon/src/services/agents/store/project.rs` with per-table
  modules under `store/project/` (`items.rs`, `turns.rs`, `gates.rs`, `attention.rs`).
- Manager tests: `crates/fleet-daemon/src/services/agents/manager/tests/{lifecycle,controls,
  restart,mirror}.rs`, all `#[tokio::test]` against a scripted fake harness.
- Skills to load before editing: `rust-async-background-work` (manager), `rust-gpui-testing`
  (any test), `rust-workspace-architecture` (a new type or module), `zed-quality-review` before
  calling the phase done.
- Authoritative docs to keep in step: `docs/NATIVE-AGENTS.md` (§3.3 thread state, §13 status),
  `docs/research/agents-contracts.md` (shipped shapes).

## Assumptions

- `Submitted` becomes an enum in the daemon's harness module, not in `fleet-core`. The design doc
  lists it under `fleet-core`; the code has it in `crates/fleet-daemon/src/agents/harness/mod.rs`.
  Nothing on the wire carries it, so the crate does not matter to any peer.
- `StopCause` is derived from events already in the log, so it needs no new event variant:
  `SessionExited { expected: true }` or `TurnAborted { reason: User | SessionStopped }` means the
  user stopped it; `SessionExited { expected: false }` or `TurnAborted { reason: ProviderExited }`
  means the provider died. It lives in `fleet-core::agents::state` so phase 2 can persist it in a
  column, but this phase keeps it in memory and rebuilds it on hydrate.
- "Live background item" means an item whose id is in `ThreadProjection.background_tasks` at the
  moment its turn settles. The projector mirrors the reducer's rule, so the SQL `items.status` of such
  an item stays non-terminal.

## Out of scope

- Any delegation type, table or verb. That is phase 2 onward.
- Changing what the Claude or Codex adapters emit. Only what the manager does with the answer.
- Exposing `StopCause` on the wire or in the app.

## Affected areas

- `crates/fleet-daemon/src/agents/harness/mod.rs` — `Submitted`.
- `crates/fleet-daemon/src/agents/claude/session.rs`, `crates/fleet-daemon/src/agents/codex/session.rs`
  — the two producers of `Submitted`.
- `crates/fleet-daemon/src/services/agents/manager/commands.rs` — the consumer in `send`.
- `crates/fleet-daemon/src/services/agents/manager/apply.rs` — `update_record`.
- `crates/fleet-daemon/src/services/agents/record.rs` — `AgentThreadRecord`.
- `crates/fleet-daemon/src/services/agents/manager/hydrate.rs` — rebuild on start.
- `crates/fleet-daemon/src/services/agents/manager/tests/lifecycle.rs`, `restart.rs`.
- `crates/fleet-core/src/agents/state.rs` — `StopCause`.
- `crates/fleet-core/src/agents/projection/reduce.rs`, `projection/tests/validation.rs`.
- `crates/fleet-daemon/src/services/agents/store/project/items.rs` (or wherever the projector closes
  items at settle; confirm with `grep -n "terminal\|Completed" store/project/*.rs`).
- `docs/NATIVE-AGENTS.md` §3.3 and §13; `docs/research/agents-contracts.md` adapter-boundary and
  manager sections.

## Tasks

### P1-T01 — Split `Submitted.queued` into `JoinedActive` and `QueuedNew`
- **Intent:** Make the harness answer say which of two different things happened, so the manager
  records a steer bubble only for a message that actually joined the running turn.
- **Touches:** `crates/fleet-daemon/src/agents/harness/mod.rs`, `agents/claude/session.rs`,
  `agents/codex/session.rs`, `services/agents/manager/commands.rs`, `manager/tests/lifecycle.rs`.
- **Steps:**
  - Replace the `Submitted { turn, queued }` struct with an enum of two variants, each carrying the
    `TurnId`: `JoinedActive` (folded into a running turn) and `QueuedNew` (accepted as its own turn,
    running now or queued behind the current one). Add a `turn()` accessor so call sites that only
    want the id stay one line.
  - Claude adapter: the coalesce path that reports `queued_turn_count` answers `JoinedActive`; a
    message accepted while no turn runs answers `QueuedNew`.
  - Codex adapter: an accepted `turn/steer` answers `JoinedActive`; a `turn/start`, including one the
    adapter queued because the running turn was not steerable, answers `QueuedNew`.
  - Manager `send`: only `JoinedActive` takes the "record the bubble now, marked steered" branch.
    `QueuedNew` pushes the input onto `pending_inputs` exactly as a fresh turn does today, so the
    bubble is recorded when the harness announces the turn.
  - Extend the existing test `steering_a_running_turn_records_a_user_message_in_that_turn` and add
    one regression: a scripted harness that answers `QueuedNew` while a turn runs produces no steered
    bubble and one ordinary user item when the queued turn starts.
- **Verification:** `cargo test -p fleet-daemon manager`, then `make lint`.
- **Done when:** No `queued: bool` remains in the workspace and the two tests above pass.

### P1-T02 — Keep live background items open when their turn settles
- **Intent:** Match the reducer and the store projector to the Claude adapter, which keeps a
  background task open past the `result` frame.
- **Touches:** `crates/fleet-core/src/agents/projection/reduce.rs`,
  `crates/fleet-core/src/agents/projection/tests/validation.rs`,
  `crates/fleet-daemon/src/services/agents/store/project/items.rs` (and `turns.rs` if it also
  closes items), `crates/fleet-daemon/src/services/agents/store/tests.rs`.
- **Steps:**
  - In `close_open_items`, skip any item whose id is in `background_tasks`, and stop the trailing
    `retain` from dropping ids whose item belongs to the settling turn. A background item leaves the
    set only through its own terminal `ItemUpdated`, as today at lines 245 and 257.
  - Confirm `ThreadProjection::is_working` (`projection/mod.rs:548`) still reports working while a
    background task is live after settle; that is the desired outcome, not a regression.
  - Find the projector's turn-settle closure in `store/project/` and apply the same exemption, so the
    SQL `items` row keeps its non-terminal status and `rebuild_thread` reproduces the reducer.
  - Add one reducer test: a tool item started as a background task, then `TurnSettled`, leaves the
    item `InProgress` and the projection working; a later terminal `ItemUpdated` closes it and the
    projection goes idle. Add the matching store test through `rebuild_thread`.
- **Verification:** `cargo test -p fleet-core`, `cargo test -p fleet-daemon store`, `make lint`.
- **Done when:** Both new tests pass and the existing settle tests still pass unchanged.

### P1-T03 — Record why a thread stopped on its record
- **Intent:** Let a later reader distinguish a thread the user stopped from one whose provider died,
  which decides whether delivery may resume it.
- **Touches:** `crates/fleet-core/src/agents/state.rs`, `crates/fleet-daemon/src/services/agents/record.rs`,
  `manager/apply.rs`, `manager/hydrate.rs`, `manager/tests/lifecycle.rs`, `manager/tests/restart.rs`.
- **Steps:**
  - Add `StopCause { User, ProviderExit }` to `fleet-core::agents::state` with serde snake_case,
    beside `SessionState`. Add `stop_cause: Option<StopCause>` to `AgentThreadRecord`, defaulted.
  - In `update_record`, set it from `SessionExited { expected }` (true is `User`, false is
    `ProviderExit`) and from `TurnAborted { reason }` (`User` or `SessionStopped` is `User`,
    `ProviderExited` is `ProviderExit`). Clear it on `SessionConfigured`.
  - Confirm hydrate replays `update_record` for every event so a restart rebuilds the value; if the
    orphan pass in `hydrate.rs` appends `TurnAborted { ProviderExited }` itself, that append must go
    through the same path.
  - Tests: `stop_exits_the_session_and_refuses_later_sends` asserts `StopCause::User`; a new restart
    test asserts an orphaned thread carries `StopCause::ProviderExit` after hydrate.
- **Verification:** `cargo test -p fleet-daemon manager`, `make lint`.
- **Done when:** Both assertions pass and the record field is set on every stop path the fake
  harness can produce.

### P1-T04 — Update the two documents that describe `Submitted` and thread state
- **Intent:** Keep `docs/` authoritative for the shapes this phase changed.
- **Touches:** `docs/NATIVE-AGENTS.md`, `docs/research/agents-contracts.md`.
- **Steps:**
  - `NATIVE-AGENTS.md` §13 row 3 names `Submitted{turn,queued}`; rewrite that clause for the two
    variants. §3.3 gains one sentence: a stopped thread records whether the user or the provider
    stopped it. Add one line to §5 or §3.3 stating that a live background item survives its turn's
    settle in the projection.
  - `agents-contracts.md`: update the adapter-boundary entry for `Submitted` and add `StopCause`
    under state, with module paths.
- **Verification:** `git diff docs/` reads as a description of the code in this branch; `make lint`
  is unaffected.
- **Done when:** No sentence in either document contradicts the code of this phase.

## Verification

```sh
make lint
make test
```

`make test` runs the harness-headless subset too; nothing in this phase changes a screen, so
`make harness` is not required.

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean.
- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `make test` passes.
- [ ] `docs/NATIVE-AGENTS.md` and `docs/research/agents-contracts.md` describe the new shapes.
- [ ] The tracker's notes reflect what actually happened, including anything skipped.
- [ ] Follow-ups discovered here are listed in the tracker, not folded into a task.

## Risks and rollback

- **A Codex `turn/start` queued behind a running turn was previously recorded as a steer.** After
  P1-T01 the bubble appears when the queued turn starts instead. This is the correct behaviour per
  `NATIVE-AGENTS.md` §7.2, but it is a visible timing change for a human steering Codex; note it in
  the PR.
- **Keeping background items open changes `is_working`.** A thread with a live background task stays
  `working` after its turn settles. Check the `thread-streaming.scenario` expectation of idle after
  the scripted turn; the scripted provider emits no background tasks, so it should be unaffected.
- **Rollback** is a revert of the single phase PR; nothing is persisted in a new shape yet.
