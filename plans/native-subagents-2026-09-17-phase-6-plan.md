# Native subagents, phase 6: recovery after a daemon restart and cancel propagation — Plan
> Tracker: ./native-subagents-2026-09-17-phase-6-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Make a delegation survive the daemon dying underneath it. After phase 3, a restart orphan-settles
the child with `TurnAborted { ProviderExited }` and the delegation ends `Failed`. This phase resumes
the child once with a nudge that tells it the session restarted, counts that in `recoveries`, and
only ends it `Failed` on a second orphaning. It also marks delegations whose caller no longer exists
as `Undeliverable` at start, and makes `cancel` walk a subtree so cancelling a delegation with live
grandchildren stops all of them before the child. Everything here is daemon-only and tested with the
scripted harness and a manager restart.

## Sizing call

**Phased, phase 6 of 7.** See ./native-subagents-2026-09-17-roadmap.md. Standard on its own: three
rules and a test that restarts the manager. It is separate from phase 3 so the completion rules
could ship and be reviewed without the restart matrix, and it depends only on phase 3, so it can run
in parallel with phases 4 and 5.

## Repository context

- Rust workspace; this phase touches `fleet-daemon` only.
- Lint: `make lint`. Test: `make test`; targeted `cargo test -p fleet-daemon delegation`,
  `cargo test -p fleet-daemon restart`. After daemon changes: `make restart`.
- Start sequence: `crates/fleet-daemon/src/services/agents/manager/hydrate.rs` runs the orphan
  pass (`work.orphans` loop at line 113; `recover_orphan` at line 146 appends
  `TurnAborted { ProviderExited }` through the reducer). The delegation worker from phase 3
  drains the outbox at start (`services/agents/delegation/worker.rs`).
- Restart tests: `crates/fleet-daemon/src/services/agents/manager/tests/restart.rs` shows how to
  drop a manager and construct a new one over the same database with the scripted harness.
- Transition rules: `services/agents/delegation/transition.rs` (pure) and
  `store/delegations.rs::transition` (SQL), from phase 3.
- Skills: `rust-async-background-work`, `rust-gpui-testing`, `zed-quality-review`.
- Docs: `docs/NATIVE-AGENTS.md` §15 (recovery subsection), §13.

## Assumptions

- The orphan pass appends `TurnAborted { ProviderExited }` through `apply_event`, so the
  transition half sees it inside the same transaction. The one-resume rule therefore lives in
  `transition.rs`: `ProviderExited` with `recoveries == 0` → status stays `Running` (or `Blocked`
  stays `Blocked`), `recoveries` becomes 1, outbox `recover`; with `recoveries >= 1` → `Failed` +
  `deliver`.
- The `recover` action is `send` of the constant resume nudge: "The session was restarted.
  Continue, and report with `fleet subagent complete` when done." `send` resumes the child by
  cursor. A child with no resume cursor ends `Failed` immediately with the reason in
  `status_payload`.
- The undeliverable pass runs once at start after the outbox drain: every terminal delegation with
  `delivery = Pending` whose caller thread has no record becomes `Undeliverable { reason:
  "caller deleted" }`. A caller that exists but is stopped by the user stays `Pending` (held), as
  phase 3 decided.
- Cancel propagation is depth-first: list live delegations whose caller is this delegation's
  child, cancel each recursively, then cancel this one. Each cancelled delegation delivers
  `Cancelled` to its own caller; a grandchild's delivery lands in the child, which is then stopped.
  That delivery may be `Undeliverable` if the child is stopped before it drains, which is
  acceptable and recorded.

## Out of scope

- Resuming the *caller* after a restart: phase 3's deliver action already does that through
  `send`.
- Any app change. The app draws whatever status the daemon publishes.
- A retry ladder beyond one resume.

## Affected areas

- `crates/fleet-daemon/src/services/agents/delegation/{transition.rs, worker.rs, mod.rs,
  footer.rs}`, `delegation/tests/recovery.rs`.
- `crates/fleet-daemon/src/services/agents/store/delegations.rs`.
- `crates/fleet-daemon/src/services/agents/manager/hydrate.rs` (ordering only, if the
  undeliverable pass needs a hook after the orphan pass).
- `docs/NATIVE-AGENTS.md` §13, §15.

## Tasks

### P6-T01 — Resume an orphaned child once with a nudge
- **Intent:** Turn a daemon restart into one extra turn instead of a failed delegation.
- **Touches:** `delegation/transition.rs`, `delegation/worker.rs`, `delegation/footer.rs`,
  `store/delegations.rs`.
- **Steps:**
  - In the pure rules, split the `TurnAborted { ProviderExited }` arm on `recoveries`: zero →
    bump `recoveries`, keep the non-terminal status, emit `recover`; one or more → `Failed` with
    `status_payload = "provider exited twice"` + `deliver`.
  - Worker `recover`: `send` the resume-nudge constant with `origin: User`; on a `conflict` or
    `not live` error that says the thread has no resume cursor, finalize `Failed` with that reason
    and `deliver`. Mark the row done.
  - Publish `DelegationChanged` on both paths.
- **Verification:** `cargo test -p fleet-daemon delegation::recovery`, `make lint`.
- **Done when:** A test that kills the scripted provider once sees `recoveries == 1`, a resumed
  child and eventually `Succeeded`; a test that kills it twice sees `Failed` delivered.

### P6-T02 — Mark delegations with a missing caller undeliverable at start
- **Intent:** Never leave a terminal delegation `Pending` forever.
- **Touches:** `delegation/worker.rs`, `store/delegations.rs`.
- **Steps:**
  - After the start drain, one query: terminal delegations with `delivery = 'pending'` whose
    `caller_thread` has no `threads` row. Set `Undeliverable { reason: "caller deleted" }` and
    mark their open `deliver` rows done, in one transaction.
  - Log one `warn!` per row with the delegation and caller ids.
  - The same rule applies at delivery time in phase 3's worker (record missing → undeliverable);
    this pass only catches rows whose caller vanished while the daemon was down.
- **Verification:** `cargo test -p fleet-daemon delegation::recovery`, `make lint`.
- **Done when:** A test that deletes the caller's thread row between two manager lifetimes sees
  `Undeliverable` after the second start, with the result text still on the record.

### P6-T03 — Cancel a delegation tree from the top
- **Intent:** Make `cancel` and Stop on a child stop every live grandchild first.
- **Touches:** `delegation/mod.rs` (`cancel`), `store/delegations.rs` (list live by caller).
- **Steps:**
  - `cancel(id)`: load; refuse when terminal; list live delegations whose caller is this
    delegation's child; cancel each recursively; then interrupt and stop the child as phase 3 does.
  - A UI Stop or `fleet agent stop` on a child thread: the manager's `stop` path must call the
    service's propagation for any live delegation whose caller is the stopped thread, before it
    stops the provider. Add a `before_stop(thread)` hook on the service that the manager calls
    outside its own operation lock, or have the transition half emit a `cancel_children` outbox
    action on the child's `TurnAborted { SessionStopped }` and let the worker do it. Prefer the
    outbox route: it takes no manager lock and survives a restart.
  - Depth is bounded at 3, so recursion is at most two levels; still write it as a loop over a
    stack to keep the reviewer's job simple.
- **Verification:** `cargo test -p fleet-daemon delegation::recovery`, `make lint`.
- **Done when:** A test with a caller, child and grandchild cancels the child's delegation and sees
  the grandchild `Cancelled` before the child, with both deliveries recorded.

### P6-T04 — The restart matrix test
- **Intent:** Prove the outbox and the recovery rules across a real manager restart over one
  database.
- **Touches:** `delegation/tests/recovery.rs`, `manager/tests/restart.rs` (helpers only).
- **Steps:**
  - Reuse the restart helper: start a manager and the service over a temp database, run a
    delegation with the scripted harness, kill the provider mid-turn, drop the manager and the
    worker, construct new ones over the same database.
  - Assert in order: the child is orphan-settled, the delegation has `recoveries == 1`, the resume
    nudge was sent, the delegation ends `Succeeded` when the scripted child completes, and the
    caller receives exactly one delivered message.
  - Second test: an outbox `deliver` row left open across the restart drains at start and delivers
    once (no duplicate item on the caller).
- **Verification:** `cargo test -p fleet-daemon delegation::recovery`, `make lint`.
- **Done when:** Both tests pass deterministically ten times in a row
  (`cargo test -p fleet-daemon delegation::recovery -- --test-threads=1` in a shell loop).

### P6-T05 — Document recovery in §15 and update the status row
- **Intent:** Keep §15 the specification of what the daemon does after a restart.
- **Touches:** `docs/NATIVE-AGENTS.md`.
- **Steps:**
  - §15 gains "Restart and recovery": the four-step start order (orphan pass, resume-once rule,
    outbox drain, undeliverable pass), the `recoveries` counter, and cancel propagation.
  - §13 row 9: recovery no longer owed.
- **Verification:** `git diff docs/` beside the tests of P6-T04.
- **Done when:** Every assertion in `recovery.rs` is a sentence in §15.

## Verification

```sh
make lint
cargo test -p fleet-daemon delegation
make test
make restart
```

Manual: start a delegation from a Claude tab, run `fleet daemon restart` while the child works,
and confirm the child resumes with the restart nudge and the caller still receives one result.

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean.
- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `make test` passes; the recovery tests pass ten times in a row.
- [ ] `docs/NATIVE-AGENTS.md` §13 and §15 describe recovery as built.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **A resumed child repeats work.** The resume nudge tells it to continue, but a harness that lost
  its context may redo a step. This is the design's accepted cost (one extra turn), documented in
  §15.
- **Propagation through the outbox is asynchronous.** A grandchild may still be stopping when the
  child's stop lands; its delivery becomes `Undeliverable`, which is recorded, not lost.
- **Rollback.** Revert the phase PR; phase 3 behaviour (fail on orphan) returns and no schema
  changes are involved.
