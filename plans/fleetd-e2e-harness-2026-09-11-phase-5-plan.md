# Fleet e2e harness — Phase 5: the scenario corpus, baselines and the report — Plan
> Tracker: ./fleetd-e2e-harness-2026-09-11-phase-5-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Phases 1–4 build a harness. This phase makes it a habit. It writes the corpus of scenarios that
actually gets run — keyed to `docs/KEYMAP.md` and `docs/UX-SPEC.md`, so that the documents which are
authoritative in this repo have something checking them — adds tolerance-based golden-image
comparison so a visual regression is caught rather than admired, produces one report an agent or a
human can read in full, wires `make` targets and a headless subset into `make test`, and finishes
the documentation so the harness is the repo's answer to "did this change actually work?".

## Sizing call

**Phased**, phase 5 of 5 — see [the roadmap](./fleetd-e2e-harness-2026-09-11-roadmap.md). A focused
week, most of it scenario writing rather than engineering. It is last because a corpus written
against a moving vocabulary would have to be rewritten; Phase 4's final task freezes that vocabulary.

## Repository context

- **Project type / commands:** as Phase 1 — `make lint`, `make test`, `make ci` (= `lint test
  test-scripts`). `make help` lists targets; `test-scripts` currently skips on non-Darwin.
- **The documents this corpus checks:** `docs/KEYMAP.md` (557 lines, authoritative for every
  binding and key context, with lettered references like [A18], [A21], [A22] that scenarios can cite),
  `docs/UX-SPEC.md` (what every screen shows and why), `docs/DESIGN-SYSTEM.md` (tokens and component
  contracts), `docs/NATIVE-AGENTS.md` §6 (the decision surfaces).
- **Existing GUI-adjacent tests:** `crates/fleet-app/tests/board_flow.rs` and `jobs_panel.rs` drive a
  real daemon from Rust; `crates/fleet-ui-kit` has a gallery used as an acceptance test for
  components. The corpus complements these, it does not replace them.
- **Lanes from Phase 1:** `headless` (full logic, no pixels — the lane for `make test`), `virtual`
  (real rendering on an isolated Hyprland output — the lane for screenshots and baselines), `attach`
  (the developer's own screen, for watching).

## Assumptions

- Every scenario asserts on the snapshot. A scenario whose only check is a screenshot is not allowed,
  because nothing fails when it regresses.
- Golden images are per-lane and tolerance-based. The `virtual` lane pins the output mode, scale and
  window size, which is what makes them comparable at all; even so, an exact-pixel policy would be a
  flake factory.
- The corpus is capped at what a human would actually click. Reducer logic stays in `#[gpui::test]`
  unit tests, where it belongs and runs in milliseconds.

## Out of scope

- Running the corpus in a hosted CI environment. There is no Hyprland there; the headless subset is
  the CI story, and the full corpus is a dev-box command.
- Performance benchmarking. `gpui-performance` owns the frame budget; the harness measures nothing
  about it in this phase.
- Coverage metrics for the corpus.

## Affected areas

- `scenarios/` (new, repo root) — the corpus, plus `scenarios/baselines/<lane>/`.
- `crates/fleet-harness/src/` — baseline comparison, report generation, the test wrapper.
- `crates/fleet-app/tests/` — the headless subset that rides `make test`.
- `Makefile` — `harness` targets.
- `docs/TESTING-HARNESS.md`, `docs/README.md`, `README.md`, `CLAUDE.md`.

## Tasks

### P5-T01 — Scenario layout and conventions
- **Intent:** make the corpus navigable before it is large.
- **Touches:** `scenarios/`, `docs/TESTING-HARNESS.md`.
- **Steps:**
  - One scenario per file, named for the surface it checks, grouped in directories that mirror the
    docs (`scenarios/hub/`, `workspace/`, `agents/`, `daemon/`, `board/`).
  - A header comment in each file naming the document section it checks (for example
    `# KEYMAP.md §Global [A18]`), so a doc change has a findable set of scenarios to update.
  - A `fixture:` line as the first directive, using the Phase 4 presets.
- **Verification:** `make lint`; `make test`; a script or a runner subcommand lists every scenario
  with the doc section it cites, and no scenario lacks one.
- **Done when:** the layout is documented and every scenario declares what it checks.

### P5-T02 — Corpus: hub, navigation and overlays
- **Intent:** cover the surfaces a user meets first.
- **Touches:** `scenarios/hub/`.
- **Steps:**
  - Scenarios for: cursor movement and `gg`/`G`/`ctrl-d`/`ctrl-u`; pane focus cycling and the rule
    that the detail panel is never in the cycle [A1]; the `g*` jumps [A7]; context switching by
    number and `gt`/`gT`; the filter's two-stage escape; the palette; settings; help; the jobs panel;
    the detail toggle; the rail collapse [A22]; `y` copying a path.
  - Each asserts on the snapshot (mode, key contexts, selection, overlay) and takes a screenshot at
    the moment that matters.
- **Verification:** `make lint`; `make test`; every scenario passes in both lanes; each cites a
  keymap section that exists.
- **Done when:** the hub's documented bindings are covered.

### P5-T03 — Corpus: workspace, terminal and tabs
- **Intent:** cover the modal half of the app.
- **Touches:** `scenarios/workspace/`.
- **Steps:**
  - Scenarios for: opening a session; the `ctrl-s` prefix table; tab switching by number and
    `h`/`l`; scroll mode entry and exit; terminal text assertions using Phase 2's `terminal.text`;
    the rule that no bare key is an app affordance over a terminal grid; returning to the hub.
- **Verification:** `make lint`; `make test`; all pass in both lanes; the terminal assertions do not
  sleep.
- **Done when:** the workspace's documented behaviour is covered.

### P5-T04 — Corpus: native agents
- **Intent:** cover the surfaces that change most and break most.
- **Touches:** `scenarios/agents/`.
- **Steps:**
  - Using Phase 4's transcripts: the agent popup opening with `a`/`A` [A21]; a streaming turn; an
    edit approval with its diff; a permission gate and both answers; the unread mark; thread
    completion; an error mid-stream; `ctrl-s` combinations bound inside an agent tab.
  - Include one scenario per open item in `TODO.md` that currently documents the *wrong* behaviour,
    marked as expected-to-fail with a comment naming the TODO entry, so fixing the bug flips the
    scenario green rather than requiring someone to remember to write a test.
- **Verification:** `make lint`; `make test`; the expected-to-fail scenarios fail for the documented
  reason and nothing else; the rest pass.
- **Done when:** the agent surfaces in `docs/NATIVE-AGENTS.md` §6 are covered, and the known gaps
  have scenarios waiting for them.

### P5-T05 — Corpus: degraded and first-run states
- **Intent:** cover what users see on a bad day.
- **Touches:** `scenarios/daemon/`.
- **Steps:**
  - Using Phase 4's fault injection: the daemon-down screen and that `ctrl-q` still works there; the
    reconnect banner and recovery; the doctor report; the first-run screen on an empty fixture; the
    sticky error and `!` focusing it [A18].
- **Verification:** `make lint`; `make test`; all pass, each with a screenshot of the degraded state.
- **Done when:** the degraded surfaces are covered.

### P5-T06 — Golden-image comparison
- **Intent:** catch a visual regression without inventing a flake machine.
- **Touches:** `crates/fleet-harness/src/baseline.rs` (new), `scenarios/baselines/`.
- **Steps:**
  - Compare each `shot` against `scenarios/baselines/<lane>/<scenario>/<NNN>-<name>.png` with a
    tolerance (per-pixel threshold plus a percentage-of-pixels budget), and write a diff image into
    the run directory when it exceeds it.
  - `--update-baselines` rewrites them; baselines are committed so a diff is reviewable.
  - Comparison runs only in the `virtual` lane, and only when a baseline exists — a new scenario is
    not a failure.
  - Report the comparison in `report.md` next to the shot.
- **Verification:** `make lint`; `make test`; a deliberate colour-token change makes the affected
  scenarios fail with a diff image, and `--update-baselines` restores green.
- **Done when:** a visual regression fails a run and shows what changed.

### P5-T07 — The report, finished
- **Intent:** one artifact that answers "did it work?" without opening anything else.
- **Touches:** `crates/fleet-harness/src/report.rs`.
- **Steps:**
  - Extend Phase 2's report: a suite-level summary across scenarios, per-scenario pass/fail with
    duration, inline screenshots and baseline diffs, the assertion timeline, and the failure block
    first when there is one.
  - `fleet-harness run scenarios/` runs a directory and produces one suite report.
- **Verification:** `make lint`; `make test`; a suite run's report is complete and readable; a
  failing suite puts the failure at the top.
- **Done when:** the report is the only thing anyone needs to read.

### P5-T08 — `make` targets and the headless subset
- **Intent:** make running it the path of least resistance.
- **Touches:** `Makefile`, `crates/fleet-app/tests/`.
- **Steps:**
  - `make harness` runs the whole corpus in the `virtual` lane; `make harness-headless` runs the
    subset that needs no pixels; `make harness-one SCENARIO=…` runs one.
  - Add a thin `#[test]` wrapper so the headless subset runs under `make test`, staying fast enough
    not to slow the workspace suite — if it cannot, keep it out and say so in the docs rather than
    letting `make test` get slow.
  - Follow `rust-gpui-testing` for where the wrapper lives and how it is named.
- **Verification:** `make lint`; `make test` (with the subset included, and its added wall time
  recorded in the tracker); `make harness` green on the dev box.
- **Done when:** one `make` target runs the corpus and one runs in CI-shaped conditions.

### P5-T09 — Documentation, finished
- **Intent:** leave the repo's documentation authoritative, as its own rules require.
- **Touches:** `docs/TESTING-HARNESS.md`, `docs/README.md`, `README.md`, `CLAUDE.md`.
- **Steps:**
  - Finish `docs/TESTING-HARNESS.md` as the authority: the lanes, the grammar, the fixtures, the
    corpus layout, the baseline policy, the report, and how to add a scenario.
  - Confirm its row in `docs/README.md`'s domain table, and add a line to the root `README.md` where
    testing is described.
  - Add a row to `CLAUDE.md`'s skill table so the next agent loads the right context before touching
    the harness, and a line under "Verify before you say it is done" pointing at `make harness` for
    changes that a human would see.
  - Load `zed-quality-review` for the final pass over the whole five-phase diff.
- **Verification:** `make lint`; `make test`; `make ci`; every command in the docs was run as written.
- **Done when:** a stranger can add a scenario from the docs alone, and nothing in `docs/` describes
  a harness that does not exist.

## Verification

```sh
make lint
make test
make ci
make harness              # full corpus, virtual lane, with baselines
make harness-headless     # the no-pixels subset
```

## Definition of done

- [ ] Every task in the tracker is checked off, with its verification output recorded.
- [ ] `make lint` is clean.
- [ ] `make test` passes on a clean tree, and the wall-time it gained is recorded in the tracker.
- [ ] `make ci` passes.
- [ ] `make harness` runs the full corpus green, except the scenarios deliberately marked
      expected-to-fail against open `TODO.md` items, each of which fails for its documented reason.
- [ ] Every scenario cites the document section it checks, and every scenario asserts on the
      snapshot rather than only taking a screenshot.
- [ ] Baselines are committed and a deliberate visual change is proven to fail the run.
- [ ] `docs/TESTING-HARNESS.md`, `docs/README.md`, `README.md` and `CLAUDE.md` match the code.
- [ ] The tracker reflects reality; follow-ups are recorded.

## Risks and rollback

- **The corpus rots.** The mitigation is that it runs: `make harness` on the dev box before a PR, and
  the headless subset in `make test`. A scenario that has been failing for a week and is still
  committed is worse than no scenario — delete it or fix it.
- **Baseline churn.** Any theme or font change rewrites many baselines, and a large
  `--update-baselines` diff is unreviewable. Keep screenshots few and deliberate: one per scenario at
  the moment that matters, not one per step.
- **`make test` gets slow.** The headless subset must stay small. If it grows past its budget, move
  it back to `make harness-headless` and say so in the docs.
- **Expected-to-fail scenarios become permanent.** Each one names a `TODO.md` entry; when that entry
  is closed the scenario must flip to expected-to-pass in the same commit. If `TODO.md` no longer
  lists it, the scenario is a bug report that nobody filed.
- **Rollback:** the corpus is additive — `scenarios/`, three `make` targets and one test wrapper.
  Deleting them leaves Phases 1–4 fully functional.
