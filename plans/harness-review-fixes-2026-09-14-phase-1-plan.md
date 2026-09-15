# Test-harness review fixes — Phase 1: a harness that can fail — Plan
> Tracker: ./harness-review-fixes-2026-09-14-phase-1-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

The GUI end-to-end harness on the `test-harness` branch cannot currently report a failure it is
supposed to catch, and its test suite does not build. `cargo check -p fleet-app --tests` fails at
HEAD on a merge-resolution slip, so `make test` cannot pass and every test the branch adds is unrun.
Separately, a Fleet process that aborts during shutdown — the exact regression this branch fixed
elsewhere — produces a fully green run, because the orderly-shutdown assertion polls for an exit
status microseconds after the `quit` response and never sees one. And `await idle`, the corpus's
only synchronisation primitive after a mutating action (62 uses), returns before the daemon's answer
has been applied to the UI, while a busy→idle transition that nothing else notifies can leave the
waiter asleep until its timeout.

This phase restores the build, makes a non-clean exit fail its run, and makes `await idle` mean what
§2 of `docs/TESTING-HARNESS.md` says it means, in both directions. At the end of it a green corpus
run is evidence for the first time.

## Sizing call

**Phased**, and this is phase 1 of 4 — see
[the roadmap](./harness-review-fixes-2026-09-14-roadmap.md). This phase is the P0 plus both P1s plus
the three remaining issues that share the `idle` primitive with them. It is a focused stretch of a
few days: one one-line unblock, one process-lifecycle change in `fleet-harness`, and three changes
to `fleet-app`'s idle accounting that have to be designed together because two of them are the same
bug failing in opposite directions. It is not split further because fixing `I3` (early return)
without `I11` (indefinite sleep) trades a flake for a hang, and neither is worth landing while the
test suite cannot build to prove it.

## Repository context

- **Project type:** Rust, one Cargo workspace, `resolver = "3"`, edition 2024, toolchain pinned by
  `rust-toolchain.toml`. Twelve crates under `crates/`.
- **Lint command:** `make lint` = `cargo fmt --all -- --check` plus
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **Test command:** `make test` = `cargo build -p fleet-daemon -p fleet-app -p fleet-harness`, then
  `cargo test --workspace` with `FLEET_DAEMON`, `FLEET_APP` and `FLEET_HARNESS_BIN` pointed at
  `target/debug/`. **It fails at HEAD** — see `P1-T01`.
- **No separate type-check step.** `cargo check --workspace --all-targets` (`make check`) covers it.
- **GUI harness:** `make harness` runs the whole `scenarios/` corpus in the `virtual` lane;
  `make harness-one SCENARIO=<path> LANE=virtual|headless|attach` runs one;
  `make harness-headless` runs the pixel-free subset, **which is currently empty** — every corpus
  scenario takes a `shot`, so the Makefile prints an explanation and exits 0.
- **Run `make restart` after changing daemon code** so the running `fleetd` matches the build.
- **The `virtual` lane needs a live, unlocked Hyprland session and a hand-exported environment.**
  A shell started by tooling on this machine has `WAYLAND_DISPLAY`, `DISPLAY` and
  `HYPRLAND_INSTANCE_SIGNATURE` unset, and `hyprctl` then reports "is hyprland running?" even though
  it is. Before `make harness`:
  `export XDG_RUNTIME_DIR=/run/user/1000`, then probe each directory under
  `/run/user/1000/hypr/` for the signature whose socket actually answers `hyprctl monitors -j`
  (the newest is frequently a dead socket), export it as `HYPRLAND_INSTANCE_SIGNATURE`, and
  `export WAYLAND_DISPLAY=wayland-1`. On a locked screen every `shot` is refused by the
  empty-output guard (§11) — unlock first.
- **Authority:** `docs/TESTING-HARNESS.md` is the frozen contract; `docs/README.md` assigns each
  document a domain. When code and a doc disagree, one of them is a bug and both are fixed in the
  same commit (`CLAUDE.md`).
- **Skills to load before editing** (`CLAUDE.md`'s table): `rust-async-background-work` and
  `gpui-state-and-memory` for the `fleet-app` idle work (`P1-T03`, `P1-T04`, `P1-T05`),
  `rust-gpui-testing` for every test added here, `zed-quality-review` before calling the phase done.
- **Source of the issues:** `plans/issues.md`, backed by `plans/finding_1.md`, `plans/finding_2.md`
  and nine validator reports under `plans/validate/`. Each task below names its issue ID.

## Assumptions

- The narrowings in `plans/issues.md` are authoritative over the raw findings. In particular
  `F1-11` — "the in-flight claim is released *after* the reply is delivered" — was refuted in both
  directions and its proposed fix would widen `I3`; it is not implemented here.
- Adding a counter to `IdleSnapshot` is additive and does not bump the `UiSnapshot` version, because
  §3 derives `idle` from its constituents rather than freezing the constituent list. §2's sentence
  promising "all six idle fields" on an await timeout *is* affected and is updated in the same
  commit as whichever task changes the count.
- `scenarios/baselines/virtual/` stays empty in this phase. No baseline is recorded, so `shot`
  verdicts remain `baseline: none yet` and §11's entry stands unchanged.
- The three coverage gaps in `plans/issues.md` "Notes on coverage" — no CI, no recorded baselines,
  no `catch_unwind` in `fleet-harness` — are out of scope for the whole initiative, not just this
  phase.
- `I2`'s fix introduces a grace period for Fleet's exit. Its duration is a new constant in
  `fleet-harness`; the plan does not prescribe a value, and whoever implements it picks one sized
  like the existing scenario `await` default and says so in the tracker.

## Out of scope

- Recording screenshot baselines, or the §11 work needed to make recording one possible.
- Adding CI. There is no `.github/workflows` in the repo and this phase does not create one.
- `catch_unwind` around the scenario runner, so a decoder panic no longer loses the run directory.
- Splitting the five files past the ~900-line rule that §11 records
  (`fleet-harness/src/{report,baseline,scenario}.rs`, `fleet-drive/src/{input,predicate}.rs`).
- Every issue assigned to phases 2, 3 and 4: the projection and §3 cluster, the scripted agent
  player, and the run-directory/report/baseline long tail.

## Affected areas

Single repository (`fleetd` workspace).

- `crates/fleet-app/src/state/harness/tests.rs` — the failing initializer (`P1-T01`).
- `crates/fleet-app/src/state/connection.rs` — `DaemonLink::Reconnected`'s third field, read only
  (`P1-T01`); `DaemonLink::Starting` as a candidate idle input (`P1-T05`).
- `crates/fleet-harness/src/scenario.rs` — the orderly-shutdown assertion and `stop()` (`P1-T02`);
  the `["idle", _]` parse arm (`P1-T06`).
- `crates/fleet-app/src/bridge/requests.rs` — where the `InFlight` claim is dropped (`P1-T03`).
- `crates/fleet-app/src/state/harness.rs` — the `idle` derivation and the `Drop` impls
  (`P1-T03`, `P1-T04`, `P1-T05`).
- `crates/fleet-app/src/drive.rs` — `Harness::wait_for`'s wake source (`P1-T04`).
- `crates/fleet-drive/src/predicate.rs` — the test pinning `idle exists` (`P1-T06`).
- `docs/TESTING-HARNESS.md` — §2 (the idle-field count and, if `P1-T05` moves the doc rather than
  the code, what `idle` says about the link).
- `scenarios/hub/idle-accounting.scenario`, `scenarios/agents/prefix-inside-a-thread.scenario`,
  `scenarios/daemon/first-run.scenario` — scenarios whose prose currently works around `I3`/`I12`.

## Tasks

### P1-T01 — Restore a compiling `fleet-app` test binary (I1)

- **Intent:** add the field a merge resolution missed so the workspace test suite builds again.
- **Touches:** `crates/fleet-app/src/state/harness/tests.rs`, `crates/fleet-app/src/state/connection.rs` (read only).
- **Steps:**
  - Read `DaemonLink::Reconnected` at `crates/fleet-app/src/state/connection.rs:43-48` and note the
    meaning of `reattached: usize` as `45297f3 daemon: keep terminals alive across daemon restarts`
    introduced it.
  - Add `reattached: <n>` to the initializer at `crates/fleet-app/src/state/harness/tests.rs:440`,
    choosing a value the surrounding test can meaningfully assert on rather than a bare `0`.
  - Extend the test's assertions to cover the new field, so the initializer is not merely
    silencing the compiler.
  - Commit this on its own, before starting any other task in any phase — it gates the verification
    step of the entire initiative.
- **Verification:** `cargo check -p fleet-app --tests` exits 0 (it currently fails with
  `error[E0063]: missing field reattached`); then `make test` from a clean tree, which must now
  build and run the `fleet-app` test binary including `harness_headless.rs`, the `idle` regression
  tests and the projection tests. Paste the `make test` summary line into the tracker.
- **Done when:** `make test` passes on an otherwise unmodified tree, and the branch has a green
  baseline for the first time.

### P1-T02 — Fail a run whose Fleet does not exit cleanly (I2)

- **Intent:** make the orderly-shutdown assertion actually observe Fleet's exit status, and stop
  `stop()` discarding it, so an abort during shutdown turns its scenario red.
- **Touches:** `crates/fleet-harness/src/scenario.rs`.
- **Steps:**
  - Read the current shape: the assertion at `scenario.rs:561` is guarded by `app.try_wait()`
    returning `Some(status)`, polled microseconds after the `quit` *response* was written, so
    `ensure!(status.success(), …)` never runs. `crates/fleet-app/src/shell/root/bootstrap.rs:103-111`
    documents that the abort this assertion exists to catch happens in a thread-local destructor
    *after `main` returns* — there is no timing in which `try_wait` sees it.
  - Replace the poll with a bounded wait after a `quit` line: await Fleet's exit under a timeout
    (a new grace constant), and fail the step on a non-success status, on a timeout, and on a
    signal-terminated exit. Name the constant and site it beside the other scenario timeouts.
  - Make `stop()` propagate the status instead of dropping it: `scenario.rs:251`'s
    `Ok(Some(_status)) => return Ok(())` and the `waited.map(|_status| ())` at the second path both
    discard it today. Teardown must be able to turn a bad exit into a run failure.
  - Decide and record what a non-zero exit means for a scenario that *expects* Fleet to die — check
    `scenarios/agents/expected-to-fail/` and `scenarios/daemon/` for any scenario that kills Fleet
    deliberately, and make sure the new assertion does not turn one of them red.
  - Add a regression test in `fleet-harness` driving a fake `fleet` that answers `quit` and then
    exits non-zero, asserting the run reports a failure. `plans/issues.md`'s `I2` repro describes
    exactly this fake (a process speaking the real drive protocol, answering `quit`, then
    `os._exit(134)`) and records that it currently yields `EXIT=0`, `ok: 2 steps`, `**Passed**`.
  - Update §8 if the journal or report gains a new failure shape for a bad exit.
- **Verification:** the new regression test, run as
  `cargo test -p fleet-harness <test name>`; then `make harness-one SCENARIO=scenarios/hub/help.scenario LANE=virtual`
  to confirm a *clean* quit still passes and the grace period does not add noticeable latency; then
  `make harness` for the full corpus, since all 41 scenarios end in `quit` and this assertion is the
  one every scenario relies on. Paste the corpus pass/fail counts into the tracker.
- **Done when:** a Fleet that aborts during shutdown fails its scenario with a message naming the
  exit status, and the unmodified corpus is still green.

### P1-T03 — Hold the in-flight claim until the app has applied the answer (I3)

- **Intent:** make `await idle` return only after a mutation's answer has reached the UI, so a
  scenario written the obvious way cannot read pre-mutation state.
- **Touches:** `crates/fleet-app/src/bridge/requests.rs`, `crates/fleet-app/src/state/harness.rs`,
  `docs/TESTING-HARNESS.md` (§2).
- **Steps:**
  - Read the defect: `run_mutations` drops `_in_flight` when `client.request(*body).await` returns
    on the runtime thread (`bridge/requests.rs:76`), but for `Bridge::send` the resulting
    `SnapshotChanged` still has to cross the event channel and be drained by the shell. `idle`
    (`state/harness.rs:192`) is the AND of five counters, none of which covers "an event is queued
    but not yet applied". `fleet-daemon`'s `SNAPSHOT_COALESCE_WINDOW = 50 ms`
    (`server/broadcast.rs:13`) makes this deterministic rather than a race — the validator measured
    `await idle` returning 4 ms after a mutating keystroke with all five counters zero.
  - Pick one of the two fixes `plans/issues.md` names and record the choice in the tracker's
    decisions log: either move the guard into the reply envelope so the UI-side consumer drops it,
    or add a `pending_events` counter fed by the shell's drain loop and included in
    `IdleSnapshot::new`. The counter is the smaller change; the envelope is the one that cannot
    drift out of sync with a new event path.
  - Do **not** implement `F1-11`'s proposed fix (release the claim after the reply is delivered) —
    `plans/issues.md` records it as refuted in both directions, and it would widen this defect.
  - Update §2's "all six idle fields" sentence and §3's `idle` description if the constituent count
    changes.
  - Rewrite the prose workarounds the corpus currently carries:
    `scenarios/agents/prefix-inside-a-thread.scenario:16-17` and
    `scenarios/hub/idle-accounting.scenario:4-7` both explain in comments that `await idle` is not
    enough. Replace the explanation with the assertion it was standing in for.
  - Add a regression test: mutate, `await idle`, assert on daemon-derived state, and confirm it
    reads the post-mutation value. Today's corpus has 14 `<action> → await idle` pairs and a
    validator confirmed none asserts on daemon-derived state, which is why this is a loaded gun
    rather than a live red — the test is what stops it being reloaded.
- **Verification:** the new regression test via `cargo test -p fleet-app <test name>`; `make test`;
  then `make harness` — the corpus is the population this primitive serves and a change to `idle`
  is exactly the change most likely to make a passing scenario hang or flake.
- **Done when:** a mutation followed by `await idle` followed by an assertion on daemon-derived
  state reads the new value, pinned by a test, and the two scenarios' workaround comments are gone.

### P1-T04 — Notify on every busy→idle edge (I11)

- **Intent:** wake `wait_for` when a counter reaches zero, so `await idle` cannot sleep to its
  timeout and then report the last evaluated snapshot.
- **Touches:** `crates/fleet-app/src/state/harness.rs`, `crates/fleet-app/src/drive.rs`.
- **Steps:**
  - Read the narrowing before implementing. The headline overstates the defect: the mechanism is
    false for three of the five counters — `pending_frame` notifies via `shell/root/focus.rs`,
    `live_toast_timers` via `tick` + `spawn_ticker`, `running_jobs` via `apply_batch`. One location
    the finding cited (`HarnessState::finish_request`) has **no production caller at all**, and both
    quoted early-return paths (`clone_repo.rs:284`, `hub/cache.rs:573`) are guarded and do not
    produce the hang. What survives is `InFlight::Drop` on the bridge thread and
    `schedule_inspection`'s no-target return.
  - Make the decrement itself the notification: have `ArmedDebounce` and `InFlight`
    (`state/harness.rs:253`) carry a weak `Entity<AppState>` and notify on drop, so every busy→idle
    edge raises the signal `drive::Harness::wait_for` listens for via `cx.observe(state)`.
  - Load `gpui-state-and-memory` first: a `Drop` running on the bridge thread cannot touch a GPUI
    entity directly, and the weak handle plus the right update path is the whole substance of this
    task.
  - Check whether `HarnessState::finish_request` should be deleted rather than fixed, given it has
    no production caller — if it is dead code, removing it is in scope here; say so in the tracker.
  - Fix `schedule_inspection`'s no-target return path so it cannot leave an armed debounce
    decremented without a notify.
  - Add a regression test that arms and releases a claim with no other notify in flight, and asserts
    `wait_for` returns promptly rather than at its timeout.
- **Verification:** the new test via `cargo test -p fleet-app <test name>`; `make test`; then
  `make harness` and compare wall-clock duration against a pre-change run — a fixed `I11` should
  make some `await idle` lines *faster*, and a scenario that got slower is a signal worth chasing.
- **Done when:** a busy→idle transition with no unrelated notify wakes `wait_for` immediately,
  pinned by a test.

### P1-T05 — Make `idle` account for the daemon link (I12)

- **Intent:** stop `idle` being structurally true on a Fleet whose bridge has not finished
  connecting, or say plainly in §2 that it is.
- **Touches:** `crates/fleet-app/src/state/harness.rs`,
  `crates/fleet-app/src/state/connection.rs` (read), `docs/TESTING-HARNESS.md` (§2),
  `scenarios/daemon/first-run.scenario`.
- **Steps:**
  - Read the narrowing: the cold-start manifestation does **not** reproduce today, and the
    reviewer's stated reason for why it doesn't is itself wrong — a validator found that
    `synchronize` issues no request while disconnected (`hub/cache.rs:419-426`, `:583-593`), which
    makes the mechanism stronger but its explanation false. The residue is a silent-green contract
    gap, not a live failure.
  - Decide which side moves, and record it in the tracker's decisions log. Either fold "the bridge
    has not finished opening" into the `idle` derivation as an additional input — all five counters
    are zero at `state/harness.rs:184` while `DaemonLink::Starting` is not an input — or state in
    §2 that `idle` is silent about the link and have the corpus pair the first `await idle` with
    `await daemon.link == connected`. `scenarios/daemon/link-recovers.scenario` already does the
    latter by accident, which is the argument for making it the documented idiom.
  - If the code moves: update §2's idle-field count in the same commit, and coordinate with
    `P1-T03` so the two do not each renumber the field list independently.
  - If the doc moves: audit every scenario whose first `await idle` is followed by an
    `assert lists.* …`, starting at `scenarios/daemon/first-run.scenario:16`, and pair each one.
  - Add a regression test either way — for the code fix, that `idle` is false on a `Starting` link;
    for the doc fix, that the corpus contains no unpaired first `await idle` (a lint-style test over
    `scenarios/`).
- **Verification:** the new test; `make test`; `make harness`, paying attention to
  `scenarios/daemon/first-run.scenario` and the rest of `scenarios/daemon/`, which are the
  scenarios that exercise a cold start.
- **Done when:** either `idle` is false while the bridge is still opening, or §2 says it is silent
  about the link and every first `await idle` in the corpus is paired with a link assertion.

### P1-T06 — Parse `await idle exists` (I17)

- **Intent:** resolve the disagreement between the scenario parser and the predicate parser about
  whether `idle` followed by an operator is a path.
- **Touches:** `crates/fleet-harness/src/scenario.rs`, `crates/fleet-drive/src/predicate.rs`
  (test, read).
- **Steps:**
  - Confirm the diagnosis: `scenario.rs:1121`'s arm `["idle", _] => 1` matches any two-token atom
    starting with `idle`, so `await idle exists` is read as the bare `idle` clause plus a timeout of
    `"exists"` and fails with "invalid digit found in string". `await idle exists 3000` parses fine,
    which confirms the arm ordering exactly.
  - Note the contradiction: `fleet-drive/src/predicate.rs:996` has a test *named*
    `idle_is_a_path_when_it_is_followed_by_an_operator` pinning `idle exists` → `Clause::Exists`.
    The two sides of the crate boundary disagree and the predicate side is the one §2 documents.
  - Fix the arm: match the two-token `idle` form only when the second token parses as a number, or
    check the `exists`/`absent` arm first. Prefer whichever reading makes the ambiguity impossible
    rather than merely reordered.
  - Add a scenario-parser test for each of `await idle`, `await idle 3000`, `await idle exists`,
    `await idle absent` and `await idle exists 3000`, so the boundary is pinned on both sides.
- **Verification:** `cargo test -p fleet-harness <test name>` and
  `cargo test -p fleet-drive idle_is_a_path_when_it_is_followed_by_an_operator`; then `make test`.
  No harness run is needed — no shipped scenario uses the form.
- **Done when:** all five `await idle …` forms parse as §2 describes, pinned by tests on both sides
  of the crate boundary.

## Verification

Run from the workspace root, in this order:

```sh
make lint     # cargo fmt --all -- --check + clippy --workspace --all-targets --all-features -D warnings
make test     # builds fleet-daemon, fleet-app, fleet-harness, then cargo test --workspace
make restart  # only if daemon code changed; nothing in this phase should, but check the diff
make harness  # the whole scenarios/ corpus in the virtual lane
```

`make harness` needs a live, **unlocked** Hyprland session with `WAYLAND_DISPLAY`,
`XDG_RUNTIME_DIR` and a *probed* `HYPRLAND_INSTANCE_SIGNATURE` exported by hand — see Repository
context. `make harness-headless` is not a substitute: every corpus scenario takes a `shot`, so that
target currently selects nothing and exits 0 with an explanation. If you cannot get a compositor,
say so in the tracker rather than recording the phase as harness-verified.

## Definition of done

- [ ] Every task `P1-T01`…`P1-T06` is checked off in the tracker, each with pasted verification
      output or a one-line "verified: <how>".
- [ ] `make lint` is clean.
- [ ] `make test` passes — which it does not at the start of this phase.
- [ ] `make harness` runs the full corpus green, or the tracker records exactly why it could not run
      and what was verified instead.
- [ ] `docs/TESTING-HARNESS.md` §2 matches the code: the idle-field count is right, and what `idle`
      says about the daemon link is stated.
- [ ] The `I3` workaround comments in `scenarios/hub/idle-accounting.scenario` and
      `scenarios/agents/prefix-inside-a-thread.scenario` are gone, replaced by assertions.
- [ ] Every fix that is latent or unreachable from today's corpus carries a test written with it.
- [ ] `zed-quality-review` has been run over the phase's diff.
- [ ] The tracker reflects reality, including any decision recorded in its decisions log
      (`I3`'s envelope-vs-counter choice, `I12`'s code-vs-doc choice, `I2`'s grace value).
- [ ] Follow-ups discovered mid-flight are captured in the tracker's Follow-ups section.

## Risks and rollback

- **`I3`'s fix can deadlock the harness.** Holding the in-flight claim until the app applies the
  answer means a dropped or never-applied event leaves `idle` false forever, turning a fast flake
  into every `await idle` in the corpus timing out. Mitigate by landing `P1-T04` (notify on the
  edge) in the same sitting and by running the full corpus, not a single scenario. Rollback is the
  commit revert; the pre-change behaviour is a known, documented false green rather than a hang.
- **`P1-T04` touches `Drop` on a non-GPUI thread.** A weak `Entity<AppState>` notified from the
  bridge thread is exactly the shape `gpui-state-and-memory` exists to constrain; getting it wrong
  produces a "cannot update X while it is already being updated" panic rather than a compile error.
  Load the skill, and if the notify cannot be made safe from `Drop`, fall back to a fallback poll
  in `wait_for` and record the change of approach in the tracker.
- **`P1-T02` can turn the whole corpus red at once.** It is the one assertion all 41 scenarios rely
  on, and if Fleet genuinely does abort on quit in this environment — `bootstrap.rs:103-111` says it
  did, "on every quit, in every Wayland session", before the `log::set_max_level(Off)` fix — the
  corpus goes from 41 green to 41 red on the first run. That is the assertion working. Do not
  weaken it; diagnose the abort, and if it is environmental rather than a Fleet defect, record it in
  §11 and gate the assertion on the lane rather than deleting it.
- **Changing the idle constituent count ripples into the docs.** §2 promises "all six idle fields"
  on an await timeout and §3 derives `idle` from them. `P1-T03` and `P1-T05` can each add a field;
  land them in a deliberate order and update the sentence once, or the two commits will each claim a
  different count.
- **No CI catches any of this.** There is no `.github/workflows` in the repo, so the Verification
  block above is the only gate. A task ticked without pasted output is a task nobody verified.
