# Fleet e2e harness — Phase 5: the scenario corpus, baselines and the report — Tracker
> Plan: ./fleetd-e2e-harness-2026-09-11-phase-5-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [~] Phase 4 is done, its tracker is closed, and the scenario grammar is declared frozen in `docs/TESTING-HARNESS.md`.
      The grammar is frozen and documented (P4-T07). Phase 4 is *not* closed: P4-T01, P4-T05 and
      P4-T06 are open in its tracker.
- [x] I have run `make lint` and `make test` once on a clean tree to confirm a green baseline.
      (integrate-2b, 2026-09-12: full `make test` green, every suite `test result: ok.`;
      `make lint` clean; `make restart` → "Restarted fleetd".)
- [x] I have re-read `docs/KEYMAP.md` and `docs/UX-SPEC.md`, which the corpus exists to check.
- [~] I am ready to start. The corpus tasks below have not been started; the infrastructure they
      need exists and is proven.

## Tasks
- [~] P5-T01 — Scenario layout and conventions — the *conventions* are written and the runner
      enforces them; the *layout* is not built. `docs/TESTING-HARNESS.md` §7 fixes the corpus
      shape (`scenarios/<surface>/<what-it-checks>.scenario`, `.scenario` or `.txt` recognised so
      `baselines/` and a README can live inside the corpus, directory runs recursive and lexical),
      and §9 is the eight-step "how to add a scenario". Reality: `scenarios/` holds three flat
      files — `agent-approval.scenario`, `daemon-down.scenario`, `pointer-basics.scenario` — and
      none of the `hub/`, `workspace/`, `agents/`, `daemon/`, `board/` directories the layout and
      the baseline key assume. Moving them is corpus-owned work, not docs-owned.
- [ ] P5-T02 — Corpus: hub, navigation and overlays — not started. The only hub coverage is the
      three `click hub.tab[N]` lines inside `pointer-basics.scenario`.
- [ ] P5-T03 — Corpus: workspace, terminal and tabs — not started. `terminal.rows` is now proven
      to populate (phase-2 tracker, P2-T06), so the blocker this task had is gone.
- [ ] P5-T04 — Corpus: native agents — not started; `agent-approval.scenario` is Phase 4's
      acceptance run, not a corpus.
- [ ] P5-T05 — Corpus: degraded and first-run states — not started. `daemon-down.scenario` covers
      the lost/reconnected link only.
- [~] P5-T06 — Golden-image comparison — the code and the policy exist, the images do not.
      `crates/fleet-harness/src/baseline.rs` compares at 8 per channel and 0.2 % of pixels, writes
      `shots/<NNN>-<name>-diff.png`, treats a size change as an error and a missing baseline as a
      pass, and `scenarios/baselines/README.md` documents all of it. `scenarios/baselines/virtual/`
      is **empty**: no baseline has ever been recorded. On this box one cannot be — a virtual-lane
      capture returns the session lock screen (phase-1 tracker, P1-T07).
- [x] P5-T07 — The report, finished — verified 2026-09-12 (docs stage) against a real run.
      `report.md` leads with the verdict and the run identity, then Lines (line number, source,
      ok, duration, what happened), Screenshots with their baseline verdicts, Assertions, Dumps,
      and "Everything else" linking `run.jsonl`, `app.log` and `fleetd.log`. A red run puts the
      failure first with the clause, the value it saw, the busy `idle` counters and links to
      `failure-NNN.{png,json}`. A directory run also writes a suite `report.md` naming the lane,
      how many scenarios were planned and how many ran, failures first. Documented as §8.
- [x] P5-T08 — `make` targets and the headless subset — done by the makefile stage and verified
      2026-09-12 (docs stage). `make harness`, `make harness-headless` and
      `make harness-one SCENARIO=… [LANE=…]` are in the `Makefile`, all three depend on `build`,
      all three pass `FLEET_APP`/`FLEET_DAEMON` from this workspace's own binaries, and
      `HARNESS_DIR`/`HARNESS_ARGS` make them corpus- and flag-agnostic. `make test` now builds
      `fleet-daemon`, `fleet-app` and `fleet-harness` and runs the headless subset through
      `crates/fleet-app/tests/harness_headless.rs`, which selects scenarios textually (excluding
      `shot` and `clipboard`, quarantining `key`/`type` while they are inert headless) and holds
      itself to a 60 s budget. Nothing anywhere lists scenario names, so a new scenario needs no
      Makefile change. Verified: `make harness-one SCENARIO=scenarios/daemon-down.scenario` →
      `ok: 13 steps` EXIT=0; `make harness-headless` → exits 0 with "no pixel-free scenarios under
      scenarios/: every one takes a shot or uses the clipboard", which is the correct answer for
      today's three-scenario corpus and is the wall time there is to record in the notes below:
      none, because the subset is currently empty.
- [x] P5-T09 — Documentation, finished — done 2026-09-12 (docs stage).
      `docs/TESTING-HARNESS.md` is the authority end to end: lanes and their two load-bearing
      limits (§4), the frozen grammar (§1–§2), fixtures (§4), corpus layout (§7), baseline policy
      (§6), the report (§8) and how to add a scenario (§9). Its row in `docs/README.md` was already
      correct and was confirmed. `README.md` gained a Testing section pointing at `make harness`
      and the frozen doc, replacing the stale `FLEET_DRIVE` recipe. `CLAUDE.md` gained a skill-table
      row for the harness and a `make harness` line under "Verify before you say it is done".

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-11 — Record here the wall time `make test` gains from the headless subset (P5-T08). If it
  is more than a few seconds, the subset is too big. **2026-09-12: the subset is empty, so it costs
  `make test` nothing.** All three corpus scenarios take a `shot`, and the two that would otherwise
  qualify are keyboard-driven. The number to record appears with the first pointer-and-assert
  scenario P5-T02 writes.
- 2026-09-11 — `make test-scripts` currently skips on non-Darwin (`bootstrap-zig` is macOS-only).
  Since the dev box is now Linux-only, check whether `make ci` should still call it, and raise it as
  a follow-up rather than changing it inside this phase.
- 2026-09-12 (docs stage) — **Every command written into a doc in this initiative was run.** The
  quickstart in `docs/DEVELOPMENT.md` (`printf … > /tmp/help.scenario` then
  `./target/debug/fleet-harness run /tmp/help.scenario`) exits 0 and writes
  `shots/003-help.png`; `fleet-harness run <file> --lane headless` exits 0 on a scenario without
  a `shot`; `fleet-harness run scenarios/ --lane headless` fails as described above;
  `--continue-on-failure` continues; `resize`/`blur`/`focus`/`advance`/`meta` all answer.
  `clipboard set` was run and **fails**, which is why the docs now say it has no working lane
  rather than describing it as `virtual`-only.
- 2026-09-12 (docs stage) — The scenario corpus and the baseline directory disagree about file
  extensions in the phase plans (`.txt`) and in the tree (`.scenario`). Both run;
  `docs/TESTING-HARNESS.md` §7 now says to prefer `.scenario`, because the extension is what tells
  a reader which files in `scenarios/` are executed.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- **The corpus is three acceptance scenarios, not a corpus.** P5-T02 through P5-T05 are the bulk
  of this phase and none of them has been started. Until they exist, `make harness` proves the
  harness works, not that Fleet works.
- **No baseline has ever been recorded**, so `make harness` can never fail on pixels. Recording
  them needs an unlocked session *and* a lane that verifies what it photographed.
- ~~**`make test`'s harness lane is empty.**~~ Closed 2026-09-12 in the fix round: three
  pixel-free scenarios now qualify — `hub/idle-accounting`, `hub/pointer-tabs` and
  `daemon/link-recovers` — and `crates/fleet-app/tests/harness_headless.rs` runs them in 9.4 s,
  well inside its 60 s budget. It also asserts that the selection is not empty, so a corpus that
  drifts back to all-`shot` fails the test instead of passing vacuously.
- **Fleet's Hub rows register no click handlers** (phase-3 tracker), which constrains what a
  pointer-only scenario can drive: the tab strip is the one Hub control with an `on_select`. The
  second half of this note — "so a hub scenario cannot run in the headless lane at all" — is no
  longer true: the keyboard reaches the focused view headless as of the fix round.
- 2026-09-12, verify round 2 — **P5-T01 through P5-T05 are stale above.** The corpus stages landed
  after those lines were written: `scenarios/` now holds 38 `.scenario` files under `agents/`,
  `board/`, `daemon/`, `hub/` and `workspace/`, plus the three flat acceptance files, two
  `expected-to-fail/` files with their `run.sh`, and two parked `blocked/` files. `make harness`
  ran all 38 green on structured assertions in 2 m 59 s, and both expected-to-fail scenarios still
  fail at the exact line and values their headers document. Whoever owns these rows should tick
  them against that run; this note does not tick them for them.
- 2026-09-12, verify round 2 — **every scenario in the corpus takes a `shot`,** so
  `make harness-headless` selects none of them and `crates/fleet-app/tests/harness_headless.rs`
  runs an empty subset. Closed in the fix round the same day: `hub/idle-accounting.scenario`,
  `hub/pointer-tabs.scenario` and `daemon/link-recovers.scenario` take no screenshot and pass in
  both lanes, and `pointer-tabs` is the pointer acceptance the phase-3 definition of done asks
  for.
- 2026-09-12, fix round — **still open: no baseline has been recorded and none can be here.** The
  session on this box paints a lock surface over every output, including a freshly created one, so
  the empty-output guard refuses every `shot`. `make harness` is therefore red on this machine for
  an environmental reason, with the guard's own message naming the cause. Everything up to each
  `shot` line passes: a `--continue-on-failure` run of all 41 scenarios produced **zero** failures
  that were not the capture guard, and **zero** of the 41 `app.log` files ended in the abort that
  used to close every GPU-backed run. See `docs/TESTING-HARNESS.md` §11.
- 2026-09-12, fix round — **`agents/unread-mark.scenario` is intermittent**, and
  `hub/contexts.scenario` was reported intermittent by the behavioural review. The first is the
  Codex turn-settlement defect in the phase-4 tracker. Both are `await`-then-`assert` shapes, so
  neither is a corpus bug that tightening a timeout would fix.

## Deviations

- 2026-09-11 — **The contracts stage froze this phase's surface before the phase ran.**
  `docs/TESTING-HARNESS.md` was written after the five phase plans and declared authoritative over
  them, so the command set, the scenario grammar, `SNAPSHOT_VERSION = 1`, the target names, the
  fixture names, the transcript shape and the path ownership were all fixed up front to let the
  implementation stages work in parallel. The plans' incremental protocol growth and their
  Phase-3 snapshot-version bump were deliberately dropped. Where a plan and the frozen document
  disagree, the document wins and the plan's text is what is wrong.
- 2026-09-12 — **The corpus is flat.** §7 specifies `scenarios/<surface>/…`; the three scenarios
  that exist sit at `scenarios/`'s root. The baseline key (`<scenario>` = corpus-relative path
  without extension) works either way, so nothing is broken — but the first corpus scenario to be
  added should move the existing three into surface directories rather than join them at the root.
- 2026-09-12 — **`make harness-headless` selects nothing on the integrated tree.** It is green,
  and it is green because all three scenarios take a `shot` and are therefore pixel-bound by the
  Makefile's own rule. Running the corpus directory in the headless lane by hand
  (`fleet-harness run scenarios/ --lane headless`) is red at
  `agent-approval.scenario` line 11, for the app-side reason in the phase-3 tracker's Deviations:
  `key` is inert there. Both facts matter — the target is not lying, and the lane is not covered.
- 2026-09-12 — **P5-T09 was completed before P5-T01 through P5-T06.** The docs stage ran across
  all five phases at once, so §7 and §9 describe a corpus layout that does not exist yet. That is
  deliberate — a stranger has to be able to write the first real scenario from the document alone —
  but it means §7 is a specification for the corpus stage, not a description of the tree.
