# Test-harness review fixes — Roadmap
> Phase plans:
>  - Phase 1: ./harness-review-fixes-2026-09-14-phase-1-plan.md · ./harness-review-fixes-2026-09-14-phase-1-tracker.md
>  - Phase 2: ./harness-review-fixes-2026-09-14-phase-2-plan.md · ./harness-review-fixes-2026-09-14-phase-2-tracker.md
>  - Phase 3: ./harness-review-fixes-2026-09-14-phase-3-plan.md · ./harness-review-fixes-2026-09-14-phase-3-tracker.md
>  - Phase 4: ./harness-review-fixes-2026-09-14-phase-4-plan.md · ./harness-review-fixes-2026-09-14-phase-4-tracker.md

## Summary

The `test-harness` branch added the GUI end-to-end harness: two new crates (`fleet-drive`,
`fleet-harness`), the `UiSnapshot` projection and bridge idle accounting in `fleet-app`, the
named-target registry in `fleet-ui-kit`, the scripted mock peer in `fleet-daemon`, the frozen
contract in `docs/TESTING-HARNESS.md`, and a 42-scenario corpus under `scenarios/`. A two-reviewer
adversarial review of that branch (`plans/issues.md`, backed by `plans/finding_1.md`,
`plans/finding_2.md` and nine validator reports under `plans/validate/`) produced 33 surviving
issues: one P0, two P1, nine P2 and twenty-one P3.

The implementation reports fixes for all 33, final lint and test pass, and the full corpus passes
42/42 on isolated virtual outputs. The Jobs behaviour-removed red proof ran on 2026-09-15 once the
`/tmp` quota was freed (`/tmp/fleet-harness/20260915-174744-jobs-panel`, failed at line 38, then
green after restoration), so `P2-T01` is `[x]` and every phase is done. The
third-agent `error-mid-stream` scenario remains deliberately `.blocked`
because its isolated repetition reached only 4/5; that re-block decision closes the reconciliation
task without pretending the scenario is stable. The unifying theme is that a test harness whose own failures are
silent is worse than no harness: the P0 means the test suite does not build at all, and the two P1s
are both false greens — a Fleet that aborts on quit reports a passing run, and the corpus's only
synchronisation primitive returns before the state it is meant to synchronise has arrived. The P2
and P3 work is the rest of the same shape: oracles that are true by construction, a mock agent that
wedges, a report that loses evidence, and documentation that promises surfaces the code does not
deliver.

## Execution status — 2026-09-15

Overall status is **BLOCKED** on one missing task proof. Evidence exists for 36 of 37 tracker tasks,
all three current-tree exit gates are green, and the full corpus passes 42/42 on the virtual lane.

| Phase | `[x]` | `[~]` | `[ ]` | Lint | Test | Harness |
| --- | ---: | ---: | ---: | --- | --- | --- |
| Phase 1 | 7 | 0 | 0 | GREEN | GREEN | GREEN: 42/42 virtual |
| Phase 2 | 9 | 1 | 0 | GREEN | GREEN | GREEN: 42/42 virtual |
| Phase 3 | 7 | 0 | 0 | GREEN | GREEN | GREEN: 42/42 virtual |
| Phase 4 | 13 | 0 | 0 | GREEN | GREEN | GREEN: 42/42 virtual |

The nested compositor answered as `WAYLAND-1`; the suite used the requested virtual lane and all 42
reports name isolated outputs. An earlier help capture failed because the per-user `/tmp` quota was
exhausted, not because the harness or app regressed; after stale harness scratch was removed, the
whole corpus passed without a repository change.

Evidence snapshot from the supplied reports:

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

These are the final current-tree exit gates. The preserved environmental failure report is
`/tmp/fleet-harness/20260915-171804-scenarios/026-help/report.md`.

Issue IDs from `plans/issues.md` (`I1`…`I33`) are carried into every task title, so any task in any
phase can be traced back to its finding, its validator verdict and its narrowing.

## Why this is phased

Four distinct subsystems change, and the seams between them are real:

- **Nothing is verifiable until the tree builds.** `cargo check -p fleet-app --tests` fails at HEAD,
  so `make test` cannot pass and every test this branch added is currently unrun. That single fix
  gates the verification step of every other task in this initiative, which is why it opens Phase 1
  rather than being scattered into whichever phase happens to run first.
- **The false-green and quiescence cluster is one primitive.** `I2`, `I3`, `I11`, `I12` and `I17`
  are five faces of "can this harness fail, and does `await idle` mean anything?". `I3` and `I11`
  are the same primitive failing in opposite directions (early return versus indefinite sleep), and
  fixing one without the other trades a flake for a hang. They must land together, and they must
  land before anyone trusts a green run from the phases that follow.
- **The snapshot cluster changes the oracle.** `I4`, `I5`, `I10`, `I28`–`I32` all change either what
  `UiSnapshot` reports or what §3 says it reports. They touch `state/harness/projection.rs`,
  `ProjectionKey` and the scenarios that assert on the result — a body of work that wants one
  reviewer holding the whole §3 contract in their head, not six people each touching one field.
- **The scripted agent player is self-contained.** `I6`, `I7`, `I8`, `I18`, `I22` and `I23` live
  almost entirely under `crates/fleet-harness/src/agent/` and `fixture/plan.rs`. Nothing outside
  that directory depends on them, and nothing they depend on changes in the other phases.
- **The run-directory, report and baseline work is a long tail of small, independent fixes.**
  Thirteen issues, none of which blocks another, all in `fleet-harness`'s infrastructure. Batching
  them into one phase is right because each is an hour of work; splitting them further would be
  ceremony.

Squeezing 33 issues across four subsystems into one plan would produce a document nobody finishes.
Splitting the long tail into per-issue phases would produce trackers nobody opens. Each phase below
ends with `make lint`, `make test` and the harness green, and with the branch strictly more
trustworthy than the day before.

## Phase list

### Phase 1 — A harness that can fail
- **Status (2026-09-15):** GREEN; 7/7 tasks and all current-tree exit gates have evidence.
- **Goal:** restore a compiling test suite, make a non-clean Fleet exit fail its run, and make
  `await idle` mean "the app has applied the answer" in both directions.
- **Shippable state at end of phase:** `make test` passes for the first time on this branch; a Fleet
  that aborts during shutdown turns its scenario red; `await idle` no longer returns on stale state
  and no longer sleeps to its timeout on a busy→idle edge; `await idle exists` parses.
- **Issues closed:** I1 (P0), I2, I3 (P1), I11, I12 (P2), I17 (P3).
- **Plan:** ./harness-review-fixes-2026-09-14-phase-1-plan.md
- **Tracker:** ./harness-review-fixes-2026-09-14-phase-1-tracker.md

### Phase 2 — An oracle that is not true by construction
- **Status (2026-09-15):** DONE; 10/10 tasks are done. The restored Jobs run is green and the
  behaviour-removed red proof failed at line 38 as required, then passed again after restoration.
- **Goal:** make `UiSnapshot` report the state scenarios actually assert on, and make §3 honest
  about the places it will not.
- **Shippable state at end of phase:** the Jobs overlay reports its real selection instead of a
  constant; `dialog.fields`/`dialog.message` are either populated or recorded in §11 so `absent`
  over them stops being an unconditional pass; `rail-collapse.scenario` asserts the collapse it
  exists to pin; `renamed_terminals` is in `ProjectionKey`; §3 documents the scrolled-out-row rule
  and `terminal.rows`'s two trims, and §2 no longer promises whitespace the parser trims.
- **Issues closed:** I4, I5, I10 (P2), I28, I29, I30, I31, I32 (P3).
- **Plan:** ./harness-review-fixes-2026-09-14-phase-2-plan.md
- **Tracker:** ./harness-review-fixes-2026-09-14-phase-2-tracker.md

### Phase 3 — A scripted agent that settles
- **Status (2026-09-15):** GREEN; 7/7 tasks have evidence. `error-mid-stream` remains `.blocked`
  after a 4/5 repetition failed the required 5/5 promotion threshold.
- **Goal:** make the mock provider honour an interrupt, bound its reads, and stop fabricating or
  colliding wire data.
- **Shippable state at end of phase:** an interrupt during an open gate cancels the turn on both
  providers instead of wedging Codex forever; a gate that is never answered fails on a deadline
  instead of hanging a `spawn_blocking` thread tokio cannot cancel; a stale `interrupted` flag no
  longer truncates the next turn; duplicate `tool_call` ids are rejected at validation; a
  single-word command parses as `unknown` instead of an empty path; §5's transcript count matches
  the preset.
- **Issues closed:** I6, I7, I8 (P2), I18, I22, I23 (P3).
- **Plan:** ./harness-review-fixes-2026-09-14-phase-3-plan.md
- **Tracker:** ./harness-review-fixes-2026-09-14-phase-3-tracker.md

### Phase 4 — A run directory and report that keep their evidence
- **Status (2026-09-15):** GREEN; 13/13 tasks and all current-tree exit gates have evidence.
- **Goal:** close the long tail of infrastructure defects so a run directory is unique, its journal
  is complete, its report shows the right images, and its decoders return errors instead of
  panicking.
- **Shippable state at end of phase:** `align`'s trustworthiness guard actually guards; two runs in
  the same second no longer share a directory and kill each other's socket; the socket-length guard
  measures the longest socket; a hostile PNG returns a named error instead of aborting the run; the
  suite report stops inlining magenta diffs as screenshots; the remaining doc claims match the code.
- **Issues closed:** I9 (P2), I13, I14, I15, I16, I19, I20, I21, I24, I25, I26, I27, I33 (P3).
- **Plan:** ./harness-review-fixes-2026-09-14-phase-4-plan.md
- **Tracker:** ./harness-review-fixes-2026-09-14-phase-4-tracker.md

## Seams between phases

- **Phase 1 → everything.** `P1-T01` (the `DaemonLink::Reconnected` initializer) is a hard
  prerequisite for the *verification step* of every task in phases 2–4: until it lands, `make test`
  cannot pass and no one can tell a new regression from the existing breakage. Do not start Phase 2
  before `P1-T01` is committed. The rest of Phase 1 is a prerequisite for *trusting* a green run,
  not for producing one, so phases 2–4 may proceed once `P1-T01` is in even if `P1-T02`…`P1-T06`
  are still open — but a green corpus run before Phase 1 completes is not evidence of anything
  (`I2`: the run would be green even if Fleet aborted).
- **Phase 1 → Phase 2, on `IdleSnapshot`.** `P1-T03` and `P1-T05` may add a sixth and seventh idle
  input. §3 derives `idle` from its constituents and §2 promises "all six idle fields" on an await
  timeout, so an added counter is additive to the snapshot but *not* invisible to the docs: whoever
  lands `P1-T03`/`P1-T05` updates §2's "six idle fields" sentence in the same commit. Phase 2's §3
  edits must be written against the post-Phase-1 field list.
- **Phase 1 → Phase 2, on `ProjectionKey`.** `P1-T03`'s notify work and `P2-T07`'s
  `renamed_terminals` key addition both touch the projection memoisation. Land Phase 1 first and
  `P2-T07` is a one-line addition; land them concurrently and they conflict in
  `state/harness/projection.rs:157-181`.
- **Phase 2 → Phase 4, on `shot` evidence.** `P2-T03` adds the first real geometry assertions to
  `rail-collapse.scenario`, and `P4-T08` (`I24`) and `P4-T10` (`I26`) are both latent only because
  no baseline exists. Neither phase records a baseline — `scenarios/baselines/virtual/` stays empty
  and §11's entry stands — so the two stay independent. If anyone records a baseline mid-flight,
  `P4-T08` and `P4-T10` become live and must land before the next corpus run is believed.
- **Phase 3 is independent.** Nothing in phases 1, 2 or 4 reads `crates/fleet-harness/src/agent/`,
  and `P3-T06`'s §5 edit does not overlap the §2/§3/§6/§11 edits in the other phases. It can run in
  parallel with Phase 2 by a second engineer.
- **Doc-side seam across all four phases.** `docs/TESTING-HARNESS.md` is the frozen contract and
  every phase edits it. The sections are disjoint by design — Phase 1 owns §2, Phase 2 owns §3 and
  the §11 entries for the projection, Phase 3 owns §5, Phase 4 owns §6 and §8 — but §11 "Known
  gaps" is appended to by three of the four. Append to §11, never restructure it, and keep each new
  entry in the shape the existing ones use (a verified limit, its mechanism, and its workaround).

## Cross-phase risks

- **The corpus is the regression suite, and it needs a compositor.** `make harness` runs the
  `virtual` lane through `hyprctl output create headless` + `grim`, which needs a live, *unlocked*
  Hyprland session and the Wayland environment exported by hand (see each phase plan's Repository
  context). A locked screen turns every `shot` into an empty-output refusal (§11). An engineer who
  cannot get a compositor can still run `make harness-headless`, but that subset is currently empty
  — every corpus scenario takes a `shot` — so a structural change verified only by `make test` is
  under-verified. Say so in the tracker when it happens rather than claiming a green harness.
- **No CI gates any of this.** There is no `.github/workflows` in the repo at all. Every command in
  every Verification block is a command a human runs locally; nothing runs it for you on push. The
  Definition of done in each phase is therefore the only gate, which is exactly why each tracker
  asks for pasted command output rather than a ticked box.
- **Latent fixes are easy to get wrong and hard to notice.** Roughly a third of the issues are
  marked latent or unreachable from today's corpus (`I19`, `I22`, `I23`, `I24`, `I26`, `I28`,
  `I31`). Their fixes are therefore not exercised by any scenario, so each one needs a unit test
  written *with* it — a latent fix with no test is indistinguishable from no fix. Every such task's
  Verification block names the test to add.
- **Narrowings are load-bearing.** Twenty of the 33 issues were narrowed during validation, and in
  most cases a specific sub-claim was disproved and struck. `plans/issues.md`'s "Dropped or
  narrowed" section lists them explicitly, including one — `F1-11`, merged into `I3` — whose
  proposed fix would *widen* the defect it was filed against. Read the issue text, not just the
  headline, before implementing; the headline is deliberately the un-narrowed version.
- **Three coverage gaps are out of scope and stay open.** No CI, no recorded baselines, and no
  `catch_unwind` in `fleet-harness` (a decoder panic skips `stage.teardown()` and
  `report::write_report`, losing the run directory). These are recorded in `plans/issues.md`'s
  "Notes on coverage" rather than as numbered issues, so this initiative does not close them; each
  phase's Follow-ups section is where to note if one bites you.

## Suggested order

Strictly 1 → 2 → 4, with 3 available to run in parallel from the moment `P1-T01` is committed.

1. **Phase 1 first and in full.** `P1-T01` is a one-line unblock and should be its own commit,
   pushed before anything else starts. The remaining five tasks make the harness capable of failing,
   which is what makes every later phase's green run mean something.
2. **Phase 2 and Phase 3 in parallel** if two engineers are available; Phase 2 first if one. Phase 2
   contains the two remaining oracle defects that make shipped scenarios vacuous (`I4`, `I10`), so
   it has more corpus-facing value than Phase 3's mostly-latent agent fixes.
3. **Phase 4 last.** It is the largest by task count and the smallest by risk: thirteen independent
   fixes, most of them latent, none of them blocking. It is also the phase most amenable to being
   split across several sittings, since no task in it depends on another.

## Follow-ups — report ledger

This append-only ledger preserves every non-`None` `FOLLOW-UPS` entry from every supplied
`out-*.txt` report. Entries already satisfied by later reports remain as historical handoffs; open
items are also summarized in their owning phase tracker.

- 2026-09-15 — `out-A1-state-harness.txt`: A4 should apply the projection-key integration edit.
- 2026-09-15 — `out-A1-state-harness.txt`: Integration should rerun full strict Clippy after the non-owned harness gate wiring lands.
- 2026-09-15 — `out-A2-bridge.txt`: H9: resolve `fixture/tools.rs:192` access to `agent::shell_word`.
- 2026-09-15 — `out-A2-bridge.txt`: H1: pass the new success ordinal into `fixture::jobs::inject` at `scenario.rs:941`.
- 2026-09-15 — `out-A2-bridge.txt`: Rerun the three bridge regressions and full all-target Clippy after those concurrent edits land.
- 2026-09-15 — `out-A3-shell-events.txt`: Rerun full strict Clippy after H1/H9 finish fleet-harness integration.
- 2026-09-15 — `out-A4-projection.txt`: Integrator should key C1’s new `harness.settling_mutations()` projection input and add its regression test.
- 2026-09-15 — `out-A5-jobs-screen.txt`: Full workspace strict Clippy can be retried after the known non-owned fleet-harness gate wiring lands.
- 2026-09-15 — `out-C1-app-contracts.txt`: Retry full strict Clippy after H10/H11 finish the fleet-harness gate-loop wiring.
- 2026-09-15 — `out-C2-harness-contracts.txt`: Rerun clippy and the full harness tests after H3/H10/H11/H4 land.
- 2026-09-15 — `out-H1-scenario.txt`: Integration should rerun strict Clippy after H10/H11 gate wiring lands.
- 2026-09-15 — `out-H1-scenario.txt`: Full harness corpus verification remains for Integration because section 0 forbids `make harness` during this stage.
- 2026-09-15 — `out-H10-agent-codex.txt`: Rerun focused Codex tests and strict fleet-harness Clippy after H1/H9/C2 integration lands.
- 2026-09-15 — `out-H11-agent-claude.txt`: After the integration notes land, rerun the three inline Claude tests and strict fleet-harness Clippy.
- 2026-09-15 — `out-H12-agent-peer.txt`: After the non-owned integration edits land, rerun the peer tests, `cargo check -p fleet-harness --all-targets`, and strict fleet-harness Clippy.
- 2026-09-15 — `out-H14-transcript.txt`: Rerun the focused tests and strict Clippy after the non-owned integrations compile.
- 2026-09-15 — `out-H2-rundir.txt`: Retry strict Clippy after H10/H11 integration.
- 2026-09-15 — `out-H2-rundir.txt`: During integration, run two back-to-back harness invocations and the deferred full test/corpus gates.
- 2026-09-15 — `out-H3-report.txt`: Integrator should rerun strict Clippy and the full harness suite after H10/H11 land.
- 2026-09-15 — `out-H4-baseline.txt`: Add a fuzz target for the hand-written PNG decoder.
- 2026-09-15 — `out-H6-fault.txt`: Finish the non-owned integration edits, rerun strict Clippy, then run the Smoke loop on pre-fix and post-fix revisions and record both pass rates.
- 2026-09-15 — `out-H8-fixture-jobs.txt`: Add a corpus scenario containing two `job success` directives; intentionally omitted per contract.
- 2026-09-15 — `out-H9-fixture-agents.txt`: Apply the integration edits, rerun the targeted tests and strict Clippy, then execute the promoted scenario.
- 2026-09-15 — `out-S1-scenarios.txt`: Integration stage should run both scenarios and perform the required red-behavior demonstrations.
- 2026-09-15 — `out-fix.txt`: Re-run `make harness` in an unlocked Hyprland session for virtual pixel evidence.
- 2026-09-15 — `out-integrate.txt`: Verify must run `make test`, `make harness`, back-to-back/50-run smoke checks, scenario promotion, and red-behavior demonstrations; fuzzing and a two-success-job corpus scenario remain deferred.
- 2026-09-15 — `out-review-closure.txt`: Fix C1–C4, reconcile the trackers, then run and paste the contract’s lint, test, harness, restart-repeat, and promoted-agent evidence.
- 2026-09-15 — `out-review-correctness.txt`: Fix the three findings and add the focused regression tests described above before trusting Cargo or corpus verification.
- 2026-09-15 — `out-review-zed-quality.txt`: Fix ZQ-01–ZQ-11, then run and paste the plan-prescribed targeted tests, `make lint`, `make test`, harness runs, and I15’s deterministic proof or 50-run rates.
- 2026-09-15 — `out-smoke-agents.txt`: Investigate the Hyprland dispatch syntax separately so the virtual lane remains isolated instead of falling back to attach.
- 2026-09-15 — `out-smoke-board.txt`: Smoke fix stage should make named clicks use an actionable clipped point—or reject off-window centers—and add a regression covering a partially scrolled tab.
- 2026-09-15 — `out-smoke-daemon.txt`: Investigate the Hyprland dispatch argument compatibility so future virtual-lane runs remain isolated and compare baselines.
- 2026-09-15 — `out-smoke-env-help.txt`: Smoke fix stage should route headless `--update-baselines` shots to `BaselineOutcome::NotRecorded` without requiring compositor pixels, then rerun this exact headless command.
- 2026-09-15 — `out-smoke-fix.txt`: Unlock the session, correct the jobs-panel focus precondition using `/tmp/fleet-harness/20260915-072103-jobs-panel`, then rerun only jobs-panel, return-to-hub, unread-mark, error-mid-stream, and help before `make lint` and `make test`.
- 2026-09-15 — `out-smoke-hub.txt`: Smoke fix stage should make `jobs-panel.scenario` deterministically create or expose at least two jobs.
- 2026-09-15 — `out-smoke-hub.txt`: Investigate the Hyprland dispatch syntax so the requested virtual lane no longer falls back to attach.
- 2026-09-15 — `out-smoke-red-demos.txt`: Smoke fix stage should make `jobs-panel.scenario` create or trigger at least two retained jobs before selecting row 1, then repeat its red/green Esc proof.
- 2026-09-15 — `out-smoke-red-demos.txt`: Fix the virtual-lane Hyprland dispatch syntax so future runs remain isolated.
- 2026-09-15 — `out-smoke-special.txt`: Smoke fix stage should remove the transient-state dependency and fix apostrophe-safe `HARNESS_ARGS` invocation.
- 2026-09-15 — `out-smoke-special.txt`: Investigate the Hyprland dispatch incompatibility so future virtual runs remain isolated.
- 2026-09-15 — `out-smoke-workspace.txt`: Fix reply-receiver lifetime in workspace PR lookup, then rerun `scenarios/workspace`.
- 2026-09-15 — `out-smoke-workspace.txt`: Investigate virtual-lane compatibility at [lane.rs:609](/home/df/.fleet/worktrees/dannyfuf/fleetd/test-harness/crates/fleet-harness/src/lane.rs:609).
- 2026-09-15 — `out-verify-1.txt`: Continue to Verify round 2; `make harness` was not run as instructed.
- 2026-09-15 — `out-close-precondition.txt`: Unlock the screen, then resume task 1 from `/tmp/fleet-harness/20260915-074247-jobs-panel`.
- 2026-09-15 — `out-close-precondition.txt`: After Jobs passes, continue tasks 2–5 in order.
- 2026-09-15 — `out-A6-drive-doc.txt`, `out-D1-testing-harness-doc.txt`, `out-D2-drive-lib-doc.txt`, `out-smoke-restart-before.txt`, and `out-smoke-restart-after.txt` reported no follow-up.
- 2026-09-15 — `PRECONDITION`: Diagnose why the third agent submission occasionally remains `idle`.
- 2026-09-15 — `PRECONDITION`: Broader phase trackers still require the previously specified full-corpus run; Phase 2 also retains its deliberate red-behaviour proof. **Both resolved: the corpus passed 42/42 and the red proof ran (`20260915-174744-jobs-panel`).**
- 2026-09-15 — `CORPUS`: Re-dispatch this task to a session with a working Bash tool; the brief itself needs no revision. **Resolved by `CORPUS FIX`.**
- 2026-09-15 — `CORPUS`: That session should run the compositor pre-flight first (`hyprctl monitors -j` must answer `WAYLAND-1`) before `make harness`, since that check was never completed here. **Resolved by `CORPUS FIX`.**
- 2026-09-15 — `CORPUS`: Note that `make harness` depends on `build`, so the first run will also pay a `cargo build --workspace` — the "wait for no other cargo process" guard in the brief still applies.
- 2026-09-15 — `CORPUS`: A feedback draft describing the Bash failure has been queued locally for the user to review.
- 2026-09-15 — `CORPUS FIX`: The harness run root will exhaust this box's `/tmp` quota again. Either lower the sweep default, root runs under `$HOME`, or run `make harness-prune SWEEP_DAYS=1` in CI. Evidence: `/home/df/.fleet-wf-scratch/quota.txt`, `/home/df/.fleet-wf-scratch/clean.txt`.
- 2026-09-15 — `CORPUS FIX`: A quota- or space-exhausted write surfaces as `grim failed (exit status: 1): libpng error: Write Error / failed to write png`. If `capture.rs`'s `run_tool` is revisited, name `ENOSPC`/`EDQUOT`. Evidence: `/tmp/fleet-harness/20260915-171804-scenarios/026-help/report.md`.
- 2026-09-15 — `CORPUS FIX`: No screenshot baselines are committed; all 41 shots in the green run report `baseline: none yet`. Recording baselines needs a deliberate churn decision. Evidence: `/tmp/fleet-harness/20260915-172955-scenarios/report.md`.
- 2026-09-15 — `CORPUS FIX`: `/tmp/claude-1000` holds 4.0G across finished sessions. Clearing it would add headroom, but those files were not removed because they belong to another tool/session.
