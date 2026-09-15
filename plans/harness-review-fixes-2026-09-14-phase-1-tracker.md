# Test-harness review fixes — Phase 1: a harness that can fail — Tracker
> Plan: ./harness-review-fixes-2026-09-14-phase-1-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state.

## Current status

GREEN on 2026-09-15. All seven tasks have focused evidence, the final `make lint` and `make test`
gates pass, and the current tree's full corpus passed 42/42 on the isolated virtual lane.

## Working agreement

- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done.
- A task is `[x]` only when its implementation has a pasted report command and summary; missing
  exit-gate evidence remains called out separately and is never converted into a green phase.
- No commits are made in this worktree under the governing contract.

## Kickoff

- [x] Read the plan end to end.
- [x] Read I1, I2, I3, I11, I12 and I17, including every **Narrowed** paragraph.
- [x] Reconciled the inherited dirty implementation baseline and the original I1 compiler error.
- [x] Recorded the current environment reality: virtual placement was fixed and remained isolated,
  but the 2026-09-15 Smoke Fix run stopped when the locked-screen guard saw only 0.0% difference.
- [x] Ready and completed — final lint, test, and full-corpus evidence is recorded below.

## Tasks

- [x] P1-T01 — Restore a compiling `fleet-app` test binary (I1)
  - `cargo check -p fleet-app --tests` — Finished `dev` profile successfully.
  - `make test` — `test result: ok. 767 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s`
- [x] P1-T02 — Fail a run whose Fleet does not exit cleanly (I2)
  - `cargo test -p fleet-harness scenario::tests::` — 11 passed; 0 failed.
  - `make harness-one SCENARIO=scenarios/hub/help.scenario LANE=virtual`
  - `**Passed** — 19 lines in 965 ms.`
- [x] P1-T03 — Hold the in-flight claim until the app has applied the answer (I3)
  - `cargo test -p fleet-app bridge::tests` — 17 passed; 0 failed.
  - `| 9 | \`scenarios/hub/idle-after-context-mutation.scenario\` | passed | 12 | 2.83 s | [009-idle-after-context-mutation/report.md](009-idle-after-context-mutation/report.md) |`
- [x] P1-T04 — Notify on every busy→idle edge (I11)
  - `cargo test -p fleet-app shell::root::events::tests` — 8 passed; 0 failed.
  - `cargo test -p fleet-app state::harness` — 28 passed; 0 failed.
- [x] P1-T05 — Make `idle` account for the daemon link (I12)
  - `cargo test -p fleet-app state::harness` — 27 passed; 0 failed.
  - `make harness-one SCENARIO=scenarios/daemon LANE=virtual`
  - `harness suite passed: 7 scenarios`
- [x] P1-T06 — Parse `await idle exists` (I17)
  - `cargo test -p fleet-harness scenario::tests::` — 11 passed; 0 failed.
  - `cargo test -p fleet-drive idle_is_a_path_when_it_is_followed_by_an_operator` — 1 passed; 0 failed.
- [x] P1-T07 — Name `settling_mutations` and `link_opening` when either solely blocks idle
  - `cargo test -p fleet-app state::harness` — 28 passed; 0 failed.
  - `make test` — `test result: ok. 767 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s`

## Notes / decisions log

- 2026-09-15 — I3 uses both binding mechanisms. Mutation-lane claims transfer from
  `in_flight_requests` to `settling_mutations` until `SnapshotChanged`/`Connected` is applied or the
  250 ms grace expires. Reply-lane claims are held until `async_channel::Sender::closed()` proves
  the consumer took or dropped the reply. F1-11 was not implemented.
- 2026-09-15 — I11 nudges every off-thread busy→idle release through the bridge event channel with
  `try_send(BridgeEvent::Nudge)`; the shell turns it into `cx.notify()` while recording.
  `HarnessState::begin_request` and `finish_request` were deleted because they had no production
  callers; tests use the guard API.
- 2026-09-15 — I12 chose code: `link_opening` is true only for `DaemonLink::Starting` and is the
  seventh pending-work input. §2/§3 serialize all eight fields: seven inputs plus derived `idle`.
- 2026-09-15 — I2 uses `QUIT_GRACE = 5 s`, sized to the same order as
  `DEFAULT_AWAIT_TIMEOUT_MS`; timeout, signal, and non-success status fail the step.
- 2026-09-15 — The I3 end-to-end regression is
  `hub/idle-after-context-mutation.scenario`: mutate → `await idle` → daemon-derived assertion.
- 2026-09-15 — The later workspace smoke exposed two retained reply receivers and
  `in_flight_requests=2`; Smoke Fix added
  `taking_one_reply_closes_the_channel_before_later_work`, which passed, but the workspace scenario
  has not been rerun.

## Verification evidence

```text
make lint
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.45s
MAKE_LINT_EXIT=0
make test
test result: ok. 585 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 10.14s
all doctests ran in 0.39s; merged doctests compilation took 0.38s
MAKE_TEST_EXIT=0
TOTALS: 2842 passed; 0 failed; 9 ignored across 79 binaries
make harness
harness suite: /tmp/fleet-harness/20260915-172955-scenarios (42 scenarios, requested lane virtual)
lane: virtual (isolated output fleet-harness-026-help-423142)
lane: virtual (isolated output fleet-harness-042-terminal-text-423142)
suite report: /tmp/fleet-harness/20260915-172955-scenarios/report.md
harness suite passed: 42 scenarios
MAKE_HARNESS_EXIT=0
**Passed** — 42 scenarios in 158.00 s.
| **Lane** | `virtual` |
| **Scenarios** | 42 of 42 ran |
```

These project-wide lines are from the current tree after the retained Smoke Fix edits. All 42
scenario reports name an isolated virtual output; none fell back to `attach`.

## Definition-of-done note

- Tasks: 7 `[x]` / 0 `[~]` / 0 `[ ]`.
- Lint: GREEN — current-tree `make lint` exited 0.
- Test: GREEN — current-tree `make test` exited 0.
- Harness: GREEN — `**Passed** — 42 scenarios in 158.00 s.`; `| **Scenarios** | 42 of 42 ran |`;
  `| **Lane** | \`virtual\` |`; run directory:
  `/tmp/fleet-harness/20260915-172955-scenarios`.

## Follow-ups

- 2026-09-15 — Resolved: the 42/42 full-corpus run covered `return-to-hub` after the reply-receiver
  lifetime fix.
- 2026-09-15 — Resolved: the full virtual corpus passed after the Smoke Fix edits.
- 2026-09-15 — No `.github/workflows` exists, so these gates remain manual.
- 2026-09-15 — No `catch_unwind` protects teardown/report writing from a harness panic.
