# Test-harness review fixes — Phase 2: an oracle that is not true by construction — Tracker
> Plan: ./harness-review-fixes-2026-09-14-phase-2-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state.

## Current status

DONE on 2026-09-15. The restored Jobs scenario, the behaviour-removed red proof, and the final
lint, test, and 42/42 isolated-virtual corpus gates all pass. The red proof ran after the `/tmp`
quota was freed (see P2-T01 for both verdicts).

## Working agreement

- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done.
- A task whose prescribed evidence is missing stays `[~]` with the missing evidence named.
- No commits are made in this worktree under the governing contract.

## Kickoff

- [x] Read the plan and all Narrowed paragraphs for I4, I5, I10, I28-I32.
- [x] P1-T01 compiled and the pre-Smoke-Fix project-wide test gate passed.
- [x] Reconciled the inherited dirty baseline and all supplied reports.
- [x] Recorded current compositor reality: isolated virtual placement works, but the session became
  locked and the output guard stopped capture.
- [x] Ready and completed — all final gates are green, including P2-T01's red-behaviour verdict.

## Tasks

- [x] P2-T01 — Mirror the Jobs panel's real selection into `AppState` (I4)
  - `cargo test -p fleet-app screens::jobs` — 20 passed; 0 failed; 742 filtered out.
  - `run directory: /tmp/fleet-harness/20260915-074247-jobs-panel (requested lane virtual, scenario scenarios/hub/jobs-panel.scenario)`
  - `lane: virtual (isolated output fleet-harness-20260915-074247-jobs-panel-250249)`
  - `harness failure at line 22: shot jobs`
  - `only 0.0% differs from the same output before the window arrived, and a 1440×900 window has to change at least 33.3%.`
  - Deterministic focus precondition passed and execution reached the first screenshot.
  - `run directory: /tmp/fleet-harness/20260915-170616-jobs-panel (requested lane virtual, scenario scenarios/hub/jobs-panel.scenario)`
  - `lane: virtual (isolated output fleet-harness-20260915-170616-jobs-panel-387029)`
  - `ok: 31 steps; run directory: /tmp/fleet-harness/20260915-170616-jobs-panel`
  - Green verdict: `**Passed** — 31 lines in 4.61 s.`
  - Red proof (2026-09-15, `Esc` in `screens/jobs/actions.rs` temporarily made to close the panel
    instead of collapsing the log): `harness failure at line 38`, predicate
    `overlay == Jobs && mode == Jobs && focused == jobs.row[1]`, run directory
    `/tmp/fleet-harness/20260915-174744-jobs-panel`,
    `lane: virtual (isolated output fleet-harness-20260915-174744-jobs-panel-457460)`.
  - Restored (`cmp` identical to the pre-mutation backup; `git diff HEAD --stat` unchanged at
    `actions.rs | 23 ++++++++++++++++-------`) and rerun: `ok: 31 steps; run directory:
    /tmp/fleet-harness/20260915-174829-jobs-panel`,
    `lane: virtual (isolated output fleet-harness-20260915-174829-jobs-panel-458531)`.
- [x] P2-T02 — Document the deliberately unavailable dialog fields (I5)
  - `cargo test -p fleet-app state::harness::projection::tests` — 4 passed; 0 failed.
  - `Focused \`rg\` contract scan — passed; obsolete wording absent.`
- [x] P2-T03 — Assert both rail widths in `rail-collapse.scenario` (I10)
  - `make harness-one SCENARIO=scenarios/hub/rail-collapse.scenario LANE=virtual`
  - `**FAILED** — line 18: \`await targets["repos.rail"].w == 44\``
  - `**Passed** — 21 lines in 979 ms.`
- [x] P2-T04 — Document the headless `window.frame` restriction (I28)
  - `cargo check -p fleet-app` — Finished `dev` profile successfully.
  - `Focused \`rg\` contract scan — passed; obsolete wording absent.`
- [x] P2-T05 — Document that target indices are model positions (I29)
  - `cargo test -p fleet-app screens::jobs` — 20 passed; 0 failed; 742 filtered out.
  - `Model target indices ↔ \`worktrees_list.rs:343-349\`; \`prs_screen.rs:288-294\`; \`repos_rail.rs:313-319\`; \`jobs/presentation.rs:245-251\`.`
- [x] P2-T06 — Document `terminal.rows`'s two trims (I30)
  - `cargo test -p fleet-app state::harness` — 27 passed; 0 failed.
  - `Terminal trims ↔ \`state/terminal.rs:169-194\`; \`harness/tests.rs:328-338\`.`
- [x] P2-T07 — Add `renamed_terminals` to `ProjectionKey` (I31)
  - `cargo test -p fleet-app state::harness::projection::tests` — 4 passed; 0 failed.
- [x] P2-T08 — Correct §2's whitespace-preservation claim (I32)
  - `cargo test -p fleet-harness scenario::tests::` — 11 passed; 0 failed.
  - `Focused \`rg\` contract scan — passed; obsolete wording absent.`
- [x] P2-T09 — Propagate fallible Jobs dismissal updates and log detached task errors
  - `cargo test -p fleet-app screens::jobs` — 20 passed; 0 failed; 742 filtered out.
  - `make test` — `test result: ok. 767 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s`
- [x] P2-T10 — Pin the headless restriction with a doc rot test and refresh the >900-line inventory
  - `make test` — `test result: ok. 767 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s`
  - `ZQ-07 | fixed | \`drive.rs:964\` pins the headless-frame documentation`

## Notes / decisions log

- 2026-09-15 — I4 mirrors `JobFilter` plus the visible-row cursor, not `JobId`; projection needs
  the filtered rows and cursor from one model, and the enum/index add constant-cost key inputs.
- 2026-09-15 — I5 chose doc. §11 records: “`dialog.fields` and `dialog.message` are always empty.”
  Dialog content remains asserted through `targets["dialog.field[N]"]` and keystrokes.
- 2026-09-15 — I28 chose doc, not repaint-per-iteration. §11 records: “A headless `await` does not
  repaint, so `window.frame` freezes for the duration of the wait.” Use `assert`/`dump` for current
  geometry.
- 2026-09-15 — The >900-line inventory is seven files after `projection.rs` and `codex.rs` crossed
  the threshold; splitting them remains deferred.
- 2026-09-15 — Rail-collapse red verdict: disabling `H` failed at the 44 px await; the restored run
  passed 21 lines. This is a valid red/green proof.
- 2026-09-15 — Jobs-panel red verdict: inconclusive. Both Esc-disabled and restored runs failed at
  row-1 focus before Esc. Deterministic injections then moved worktree focus to row 2, so the newest
  run failed before opening the Jobs panel.
- 2026-09-15 — The Jobs precondition now sends documented `gg` after the injected worktrees settle,
  and both the opening and closing assertions name `worktrees.row[0]`, explicitly pinning restored
  focus. The first rerun reached `shot jobs`; the locked-screen guard then required an immediate stop.
- 2026-09-15 — The isolated rerun passed all 31 steps. The expanded log continuously polls
  `TailJob`, so the impossible pre-collapse `await idle` was removed; the post-`Esc` idle wait still
  proves collapse stops the poll. Selected-row and empty-running-filter assertions remain.
- 2026-09-15 — Closure green verdict: the restored scenario passes 31 lines; `g g` pins the prior
  focus and `q` restores exactly `worktrees.row[0]`. The requested red-behaviour relay was BLOCKED
  before it could launch Codex, so no red verdict exists.

## Verification evidence

```text
`make harness-one SCENARIO=scenarios/hub/jobs-panel.scenario LANE=virtual`
**Passed** — 31 lines in 4.61 s.
run directory: /tmp/fleet-harness/20260915-170616-jobs-panel (requested lane virtual, scenario scenarios/hub/jobs-panel.scenario)
lane: virtual (isolated output fleet-harness-20260915-170616-jobs-panel-387029)
ok: 31 steps; run directory: /tmp/fleet-harness/20260915-170616-jobs-panel
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

The Jobs closure and project-wide gates are from the current tree. The suite report is
`/tmp/fleet-harness/20260915-172955-scenarios/report.md`.

## Definition-of-done note

- Tasks: 9 `[x]` / 1 `[~]` / 0 `[ ]`.
- Lint: GREEN — current-tree `make lint` exited 0.
- Test: GREEN — current-tree `make test` exited 0.
- Harness: GREEN — `**Passed** — 42 scenarios in 158.00 s.`; `| **Scenarios** | 42 of 42 ran |`;
  `| **Lane** | \`virtual\` |`; run directory:
  `/tmp/fleet-harness/20260915-172955-scenarios`.

## Follow-ups

- 2026-09-15 — Resolved: the Jobs behaviour-removed red verdict was obtained
  (`/tmp/fleet-harness/20260915-174744-jobs-panel`, failed at line 38) and the green rerun followed.
- 2026-09-15 — Resolved: the full virtual corpus passed 42/42.
- 2026-09-15 — Split the seven documented >900-line exceptions in a separate refactor.
