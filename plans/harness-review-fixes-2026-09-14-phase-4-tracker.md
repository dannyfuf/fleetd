# Test-harness review fixes — Phase 4: a run directory and report that keep their evidence — Tracker
> Plan: ./harness-review-fixes-2026-09-14-phase-4-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state.

## Current status

GREEN on 2026-09-15. All thirteen tasks have evidence: the apostrophe-path binary invocation passed
with no fake-`gh` errors, both headless shot outcomes match the documented contract, and the final
lint, test, and 42/42 isolated-virtual corpus gates pass.

## Working agreement

- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done.
- A task whose prescribed evidence is missing stays `[~]` with the missing evidence named.
- No commits are made in this worktree under the governing contract.

## Kickoff

- [x] Read the plan and every Narrowed paragraph for I9, I13-I16, I19-I21, I24-I27 and I33.
- [x] P1-T01 compiled and the pre-Smoke-Fix project-wide test gate passed.
- [x] Reconciled the inherited dirty baseline and all supplied reports.
- [x] Confirmed `scenarios/baselines/virtual/` contains no recorded image baseline.
- [x] Recorded current compositor reality: virtual isolation was repaired, then the locked-screen
  guard stopped the remaining capture.
- [x] Ready and completed — all task evidence and final gates are recorded below.

## Tasks

- [x] P4-T01 — Give the journal's command exchange a line number and tighten `align` (I9)
  - `cargo test -p fleet-harness report::tests` — 10 passed; 0 failed.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P4-T02 — Derive the socket-length guard from `HarnessEnv` (I13)
  - `cargo test -p fleet-harness rundir::tests` — 7 passed; 0 failed; 93 filtered out.
- [x] P4-T03 — Make run-directory names collision-proof (I14)
  - `cargo test -p fleet-harness rundir::tests` — 7 passed; 0 failed; 93 filtered out.
  - `Back-to-back help 1 — \`/tmp/fleet-harness/20260915-063658-help\``
  - `Back-to-back help 2 — \`/tmp/fleet-harness/20260915-063703-help\``
- [x] P4-T04 — Gate, adopt, unseal, then release a replacement daemon (I15)
  - `cargo test -p fleet-harness fault::tests` — 8 passed; 0 failed; 93 filtered out.
  - `before: passes=50 failures=0`
  - `after: passes=50 failures=0`
- [x] P4-T05 — Quote the substituted path in the fake `gh`/`acli` (I16)
  - `Focused tests — launcher, shims, peer, Codex, Claude, faults, PNG, scenario, report, Jobs, driver, harness state, and headless corpus passed.`
  - `FLEET_APP=target/debug/fleet FLEET_DAEMON=target/debug/fleetd target/debug/fleet-harness run scenarios/hub/help.scenario --lane virtual --run-dir "/tmp/fleet-harness/o'clock-help"`
  - `**Passed** — 19 lines in 969 ms.`
  - `run directory: /tmp/fleet-harness/o'clock-help (requested lane virtual, scenario scenarios/hub/help.scenario)`
  - `lane: virtual (isolated output fleet-harness-o-clock-help-391457)`
  - `ok: 19 steps; run directory: /tmp/fleet-harness/o'clock-help`
  - `gh error scan: 0 matches`
- [x] P4-T06 — Give injected jobs a per-run ordinal (I19)
  - `cargo test -p fleet-harness --lib` — 122 passed; 0 failed.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P4-T07 — Bound compressed scanlines and widened RGBA allocation (I20 + I21)
  - `cargo test -p fleet-harness baseline::tests` — 19 passed; 0 failed; 82 filtered out.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P4-T08 — Record one failed `shot` exchange with geometry and baseline error (I24)
  - `cargo test -p fleet-harness scenario::tests::` — 11 passed; 0 failed.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P4-T09 — Distinguish “not recorded” from “skipped” and qualify §6 (I25)
  - `make harness-one SCENARIO=scenarios/hub/help.scenario LANE=headless HARNESS_ARGS=--update-baselines`
  - `ok: 19 steps; run directory: /tmp/fleet-harness/20260915-072042-help`
  - `| 14 | \`shot help\` | ok | 37 ms | not recorded (lane: headless) |`
  - Plain headless: `/tmp/fleet-harness/20260915-171119-help` — `**FAILED** — line 14: \`shot help\``
  - Update headless: `/tmp/fleet-harness/20260915-171127-help`
  - `| 14 | \`shot help\` | ok | 36 ms | not recorded (lane: headless) |`
- [x] P4-T10 — Keep diff images out of screenshot sections and label the one retained diff (I26)
  - `cargo test -p fleet-harness report::tests` — 10 passed; 0 failed.
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`
- [x] P4-T11 — Surface a directory-listing error in `pngs()` (I27)
  - `cargo test -p fleet-harness report::tests` — 10 passed; 0 failed.
- [x] P4-T12 — Drop the `regex` claim from `fleet-drive`'s feature doc (I33)
  - `cargo tree -p fleet-lazygit -i regex` — completed successfully.
  - `regex v1.13.1`
  - `├── fleet-drive v0.1.0 (/home/df/.fleet/worktrees/dannyfuf/fleetd/test-harness/crates/fleet-drive)`
  - `│   └── fleet-lazygit v0.1.0 (/home/df/.fleet/worktrees/dannyfuf/fleetd/test-harness/crates/fleet-lazygit)`
  - `└── gpui v0.2.2 (https://github.com/zed-industries/zed?tag=v1.18.1#bebe92f4)`
- [x] P4-T13 — Escape apostrophes in `shell_word` and execute the launcher plus both shims via `sh -c`
  - `Focused tests — launcher, shims, peer, Codex, Claude, faults, PNG, scenario, report, Jobs, driver, harness state, and headless corpus passed.`
  - `make test` — `test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s`

## Notes / decisions log

- 2026-09-15 — P4-T03 uses atomic `std::fs::create_dir` claims and retries `-2`, `-3`, … only on
  `AlreadyExists`. Explicit run directories are claimed atomically; §6 documents the optional
  suffix for scenario and suite defaults. No other reader needed a name-format change.
- 2026-09-15 — P4-T04 now uses a parent-controlled pipe: spawn/adopt while sealed, unseal, then
  release the child to `exec fleetd`. The required measured rates were before 50/50 and after 50/50;
  the probabilistic pre-fix race did not reproduce. Focused gated-child tests additionally pin order.
- 2026-09-15 — P4-T07 applies `MAX_DECODED_BYTES = 64 MiB` both to inflated scanlines and widened
  RGBA allocation. A 1920×1080 RGBA capture is 8,294,400 output bytes (8,295,480 including filter
  bytes) and remains below the ceiling; compact 1-bit input widening toward 2 GiB is rejected.
- 2026-09-15 — P4-T10 excludes raw `*-diff.png` files from screenshot sections and renders one
  `Diff:`-labelled image beside the associated screenshot.
- 2026-09-15 — P4-T12 is doc-only. `cargo tree` proves the independent edge
  `regex <- gpui <- fleet-lazygit`, so changing the manifest would save no build time.
- 2026-09-15 — P4-T09's first smoke run was red because headless capture failed before baseline
  classification. Smoke Fix made the exact command pass and the report now says
  `not recorded (lane: headless)`.
- 2026-09-15 — P4-T05 was verified by invoking the binary directly because Make expands
  `HARNESS_ARGS` through the shell. The explicit apostrophe-bearing directory passed in the isolated
  virtual lane and the run logs produced `gh error scan: 0 matches`.

## Verification evidence

```text
ok: 19 steps; run directory: /tmp/fleet-harness/o'clock-help
gh error scan: 0 matches
**FAILED** — line 14: `shot help`
| 14 | `shot help` | ok | 36 ms | not recorded (lane: headless) |
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

These lines are from the current tree. The suite report is
`/tmp/fleet-harness/20260915-172955-scenarios/report.md`.

## Definition-of-done note

- Tasks: 13 `[x]` / 0 `[~]` / 0 `[ ]`.
- Lint: GREEN — current-tree `make lint` exited 0.
- Test: GREEN — current-tree `make test` exited 0.
- Harness: GREEN — `**Passed** — 42 scenarios in 158.00 s.`; `| **Scenarios** | 42 of 42 ran |`;
  `| **Lane** | \`virtual\` |`; run directory:
  `/tmp/fleet-harness/20260915-172955-scenarios`.

## Follow-ups

- 2026-09-15 — Add a fuzz target for the hand-written PNG decoder.
- 2026-09-15 — Add a corpus scenario containing two `job success` directives; omitted by contract.
- 2026-09-15 — Add general `catch_unwind` protection so a panic cannot skip teardown and report writing.
- 2026-09-15 — Record real virtual baselines; `scenarios/baselines/virtual/` is still empty.
- 2026-09-15 — Resolved: the full virtual corpus passed 42/42.
- 2026-09-15 — The `/tmp/fleet-harness` run root will exhaust the per-user `/tmp` quota again;
  lower the seven-day prune horizon, move the root, or schedule `make harness-prune SWEEP_DAYS=1`.
- 2026-09-15 — If capture error handling is revisited, name `ENOSPC`/`EDQUOT` when a shot cannot be
  written; the current `grim`/libpng error resembles a capture defect.
