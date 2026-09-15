# Test-harness review fixes — Phase 3: a scripted agent that settles — Tracker
> Plan: ./harness-review-fixes-2026-09-14-phase-3-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state.

## Current status

GREEN on 2026-09-15. All seven tasks have evidence and the final lint, test, and 42/42
isolated-virtual corpus gates pass. `error-mid-stream` remains deliberately `.blocked`: its 4/5
rate closes the required promotion decision but does not meet the contract's 5/5 threshold.

## Working agreement

- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done.
- A task whose prescribed evidence is missing stays `[~]` with the missing evidence named.
- No commits are made in this worktree under the governing contract.

## Kickoff

- [x] Read the plan and every Narrowed paragraph for I6-I8, I18, I22 and I23.
- [x] P1-T01 compiled and the pre-Smoke-Fix project-wide test gate passed.
- [x] Reconciled the inherited dirty baseline and all supplied build/review/smoke reports.
- [x] Recorded all available unread-mark repetitions and their defect classifications below.
- [x] Recorded current compositor reality: virtual isolation was repaired, then the locked-screen
  guard stopped the remaining Smoke Fix verification.
- [x] Ready and completed — P3-T06's re-block decision and every final gate are recorded below.

## Tasks

- [x] P3-T01 — Notice an interrupt inside both gate loops (I6)
  - `cargo test -p fleet-harness --lib` — 122 passed; 0 failed.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P3-T02 — Give each gate one absolute deadline (I7)
  - `cargo test -p fleet-harness agent::peer::tests` plus five delayed retries — `error: could not compile fleet-harness (lib test) due to 6 previous errors`.
  - `Focused tests — launcher, shims, peer, Codex, Claude, faults, PNG, scenario, report, Jobs, driver, harness state, and headless corpus passed.`
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P3-T03 — Consume `interrupted` at the top of `run_turn` (I8)
  - `cargo test -p fleet-harness --lib` — 122 passed; 0 failed.
  - `make harness-one SCENARIO=scenarios/agents LANE=virtual`
  - `**Passed** — 7 scenarios in 32.47 s.`
- [x] P3-T04 — Reject duplicate `tool_call` ids in `transcript::validate` (I22)
  - `cargo test -p fleet-harness --lib` — 122 passed; 0 failed.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P3-T05 — Fix `command_actions`'s single-word parse (I23)
  - `cargo test -p fleet-harness --lib` — 122 passed; 0 failed.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P3-T06 — Reconcile §5's transcript count with the `agents` preset (I18)
  - `error-mid-stream passes: 4/5`
  - Passes: `/tmp/fleet-harness/20260915-170711-error-mid-stream`,
    `/tmp/fleet-harness/20260915-170717-error-mid-stream`,
    `/tmp/fleet-harness/20260915-170723-error-mid-stream`, and
    `/tmp/fleet-harness/20260915-170815-error-mid-stream`.
  - Failure: `/tmp/fleet-harness/20260915-170728-error-mid-stream`.
  - `**FAILED** — line 23: \`await agents.threads[0].state == failed 40000\``
  - The failed run remained `idle`; the promoted untracked file was removed and the exact line and
    report path were written into `blocked/error-mid-stream.blocked`.
  - Promotion decision: re-blocked. Four passes prove the third fixture can reach `failed`; 4/5 is
    below the required 5/5 stability threshold, so promotion would misrepresent the corpus.
- [x] P3-T07 — Replace five-second wall-clock tests and prove noise/unrelated frames cannot renew gates
  - `Focused tests — launcher, shims, peer, Codex, Claude, faults, PNG, scenario, report, Jobs, driver, harness state, and headless corpus passed.`
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`

## Notes / decisions log

- 2026-09-15 — P3-T02 uses `GATE_BUDGET = 5 s`, sized to
  `DEFAULT_AWAIT_TIMEOUT_MS`, on the gate wait only. One absolute `Instant` covers malformed lines
  and unrelated valid frames; normal between-turn transcript reads remain unbounded. Tests inject
  zero/20 ms budgets and assert protocol outcomes rather than wall time.
- 2026-09-15 — P3-T04 uses one shared namespace for permission, approval, and `tool_call` ids
  because all three identify correlated wire items.
- 2026-09-15 — I18 uses all three starters without a new `AgentKind`: Claude serves the two turns
  from `two-turns.json` followed by the failed turn from `error-mid-stream.json`; Codex serves
  `edit-approval.json`. A prior 2/3 run failed a transient `working` await; the current scenario
  removed that dependency but is still not promotable because the final failed state reached 4/5.
- 2026-09-15 — Unread-mark counts from the supplied reports: hub 5/5; workspace 5/5; board 4/5;
  agents directory 1/1 plus five additional runs 5/5. Aggregate observed result is 20/21. The sole
  failure was classified as the named-click off-window-center harness defect, not the native-agent
  adapter or the stale-interrupt fix.
- 2026-09-15 — Smoke Fix passed
  `a_partially_off_window_target_uses_the_visible_intersection` and
  `a_fully_off_window_target_is_refused`; unread-mark itself has not been rerun after that fix.
- 2026-09-15 — Required promotion decision: 4/5 is not stable, so `error-mid-stream` remains
  blocked. The fixture can reach `failed`, but the scenario does not do so deterministically.

## Verification evidence

```text
error-mid-stream passes: 4/5
**FAILED** — line 23: `await agents.threads[0].state == failed 40000`
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
harness suite passed: 42 scenarios
MAKE_HARNESS_EXIT=0
**Passed** — 42 scenarios in 158.00 s.
| **Lane** | `virtual` |
| **Scenarios** | 42 of 42 ran |
```

The repetition used the isolated virtual lane on all five runs. The project-wide gate lines are
from the current tree; the suite report is
`/tmp/fleet-harness/20260915-172955-scenarios/report.md`.

## Definition-of-done note

- Tasks: 7 `[x]` / 0 `[~]` / 0 `[ ]`.
- Lint: GREEN — current-tree `make lint` exited 0.
- Test: GREEN — current-tree `make test` exited 0.
- Harness: GREEN — `**Passed** — 42 scenarios in 158.00 s.`; `| **Scenarios** | 42 of 42 ran |`;
  `| **Lane** | \`virtual\` |`; run directory:
  `/tmp/fleet-harness/20260915-172955-scenarios`.

## Follow-ups

- 2026-09-15 — Diagnose why the third submission occasionally settles `idle`, then rerun at least
  5/5 before promoting `error-mid-stream`.
- 2026-09-15 — Rerun unread-mark after the clipped-click fix and record the new N/M rate.
- 2026-09-15 — Resolved in part: the full virtual corpus passed 42/42; rerun the agents directory
  with `error-mid-stream` only after its intermittent idle settlement is fixed.
- 2026-09-15 — Keep the known native Codex answered-gate settlement defect in §11 out of this phase.
