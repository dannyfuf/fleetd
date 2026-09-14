# Fleet end-to-end testing harness — Roadmap
> Phase plans:
>  - Phase 1: ./fleetd-e2e-harness-2026-09-11-phase-1-plan.md · ./fleetd-e2e-harness-2026-09-11-phase-1-tracker.md
>  - Phase 2: ./fleetd-e2e-harness-2026-09-11-phase-2-plan.md · ./fleetd-e2e-harness-2026-09-11-phase-2-tracker.md
>  - Phase 3: ./fleetd-e2e-harness-2026-09-11-phase-3-plan.md · ./fleetd-e2e-harness-2026-09-11-phase-3-tracker.md
>  - Phase 4: ./fleetd-e2e-harness-2026-09-11-phase-4-plan.md · ./fleetd-e2e-harness-2026-09-11-phase-4-tracker.md
>  - Phase 5: ./fleetd-e2e-harness-2026-09-11-phase-5-plan.md · ./fleetd-e2e-harness-2026-09-11-phase-5-tracker.md

## Summary

Fleet is a GPUI desktop app (`fleet`) over a daemon (`fleetd`). Today an automated reviewer can
drive it only through a minimal scripted-input driver (`FLEET_DRIVE`, `docs/decisions/0007`): it
appends lines to a file that the app polls every 100 ms, it can press keys and type text, and it
takes screenshots by shelling out to the macOS-only `/usr/sbin/screencapture`. It cannot move or
click a mouse, cannot tell you what is on screen, cannot wait for a condition (only `sleep`),
cannot isolate itself from the developer's real `~/.fleet`, and produces no screenshots at all on
Linux — which is now the only development platform.

This initiative builds a real end-to-end harness: an agent or engineer runs one command, and a
hermetic Fleet plus its own `fleetd` boots on an isolated virtual monitor, a scenario script
drives it with keyboard *and* mouse, the app answers structured queries about its own state, the
run asserts on those answers instead of on sleeps, and the result is a directory of screenshots,
state dumps, logs and a readable report.

## Why this is phased

Five distinct subsystems have to change, in an order where each depends on the last, and every
intermediate state has to be independently useful:

- The **transport and launch** work (Phase 1) is a prerequisite for everything: without a
  request/response channel there is nothing to carry an assertion, and without a hermetic launch
  every run pollutes the developer's real state.
- The **assertion surface** (Phase 2) depends on that channel and is itself the prerequisite for
  mouse targeting: a script that says `click worktrees.row[2]` needs the app to publish where that
  row is.
- **Mouse and full input** (Phase 3) depends on Phase 2's snapshot for named targets.
- **Agents, fixtures and fault injection** (Phase 4) is a large, self-contained body of work in the
  daemon and is only worth doing once scenarios can assert on the results.
- The **scenario corpus and baselines** (Phase 5) is the payoff and can only be written against a
  finished vocabulary.

Squeezing this into one plan would produce a document nobody executes; splitting it any finer
would produce phases that ship nothing on their own. Each phase below ends in a state where the
harness is strictly more useful than the day before and the tree is green.

## Phase list

### Phase 1 — A driver I can trust
- **Goal:** replace the poll-a-file driver with a request/response socket, give the harness its own
  runner binary, a hermetic environment and an isolated virtual monitor, and make screenshots work
  on Linux.
- **Shippable state at end of phase:** `fleet-harness run <scenario>` boots an isolated `fleetd`
  plus `fleet` on a dedicated Hyprland headless output, presses keys, captures real PNGs, tears the
  output down, and leaves a run directory. No sleeps-as-synchronisation are removed yet, but every
  command is acknowledged.
- **Plan:** ./fleetd-e2e-harness-2026-09-11-phase-1-plan.md
- **Tracker:** ./fleetd-e2e-harness-2026-09-11-phase-1-tracker.md

### Phase 2 — Assertions and synchronisation
- **Goal:** let the app answer "what is on screen?" as versioned JSON, and let a scenario wait on a
  condition and assert on it instead of sleeping and eyeballing a PNG.
- **Shippable state at end of phase:** scenarios use `await` and `assert`; a failing assertion
  fails the run with a nonzero exit, an automatic screenshot, the offending dump and a readable
  report. Screenshots become evidence, not the primary oracle.
- **Plan:** ./fleetd-e2e-harness-2026-09-11-phase-2-plan.md
- **Tracker:** ./fleetd-e2e-harness-2026-09-11-phase-2-tracker.md

### Phase 3 — Mouse, targets and the rest of human input
- **Goal:** every interaction a human can perform — move, hover, click, double-click, right-click,
  drag, scroll, clipboard, resize, focus loss — addressable by *name* rather than by pixel.
- **Shippable state at end of phase:** a scenario can click a worktree row, drag a board card,
  hover a badge for its tooltip and paste into a dialog, on any window size.
- **Plan:** ./fleetd-e2e-harness-2026-09-11-phase-3-plan.md
- **Tracker:** ./fleetd-e2e-harness-2026-09-11-phase-3-tracker.md

### Phase 4 — Agents, fixtures and fault injection
- **Goal:** make the interesting surfaces reachable deterministically: native Claude/Codex threads
  with approvals and gates, seeded repos and worktrees, a dead or slow daemon, a failed job.
- **Shippable state at end of phase:** the agent conversation, the approval cards, the daemon-down
  screen and the sticky error can all be exercised without a vendor CLI, a network, or a token.
- **Plan:** ./fleetd-e2e-harness-2026-09-11-phase-4-plan.md
- **Tracker:** ./fleetd-e2e-harness-2026-09-11-phase-4-tracker.md

### Phase 5 — The scenario corpus, baselines and the report
- **Goal:** a maintained corpus of scenarios keyed to `docs/KEYMAP.md` and `docs/UX-SPEC.md`,
  golden-image comparison, `make` targets, and documentation that makes the harness authoritative.
- **Shippable state at end of phase:** `make harness` runs the corpus and produces one report an
  agent can read; a headless subset rides `make test`; `docs/TESTING-HARNESS.md` is the authority
  and `docs/decisions/0007` is superseded.
- **Plan:** ./fleetd-e2e-harness-2026-09-11-phase-5-plan.md
- **Tracker:** ./fleetd-e2e-harness-2026-09-11-phase-5-tracker.md

## Seams between phases

- **Phase 1 → 2: the command envelope.** Phase 1 defines a newline-delimited JSON request/response
  envelope (`{"id":N,"cmd":"…","args":{…}}` → `{"id":N,"ok":true,"data":{…}}`) over a unix socket.
  Phase 2 adds `dump`, `await` and `assert` as new `cmd` values and nothing else changes. The
  envelope is additive-only from Phase 1 onward: a new command is a new `cmd` value, never a change
  to the frame shape.
- **Phase 2 → 3: the snapshot is the coordinate authority.** Phase 3 adds a `targets` map (name →
  rect, in logical window coordinates) to the Phase 2 snapshot. Mouse commands accept either a
  target name or raw coordinates; the name is resolved app-side, so the resolution never leaks into
  scenario files. The snapshot's version field is bumped when `targets` lands.
- **Phase 3 → 4: fault and fixture commands are runner-side, not app-side.** Killing the daemon,
  seeding repos and scripting an agent happen in `fleet-harness` and in `fleetd`'s `test-support`
  surface, never through the app socket. The app must not grow a "make the daemon die" command; it
  learns about the death the same way it does in production.
- **Phase 4 → 5: the scenario file format freezes.** Phase 5 writes dozens of scenarios; the line
  grammar, target names and predicate syntax must stop moving first. Phase 4's last task is to
  declare the grammar frozen and record it in `docs/TESTING-HARNESS.md`.
- **Documentation rides with each phase.** `docs/` is authoritative in this repo, not descriptive.
  `docs/decisions/0007-gui-smoke-procedure.md` describes the driver that Phase 1 replaces, so Phase
  1 supersedes it in the same commit. Every later phase updates `docs/TESTING-HARNESS.md` as it
  changes behaviour. No phase ends with a doc that describes a driver that no longer exists.

## Cross-phase risks

- **The harness becomes a second product.** A driver that only the harness exercises rots. Mitigated
  by keeping the app-side surface small (one socket, one snapshot builder, one element wrapper) and
  by making the corpus in Phase 5 the thing that runs, not the thing that is written once.
- **Harness hooks leak into production paths.** The snapshot must be built in update paths and
  memoised (`docs/APP-CONTRACTS.md` forbids work in `render`), and the target-bounds recorder must
  be a single branch on an already-loaded flag. If either shows up in a profile, the design is
  wrong. `gpui-performance` is the skill to load before touching those paths.
- **Screenshot flake.** GPU rendering, font fallback and animation make byte-exact images a trap.
  The plan treats pixels as *evidence for a human or an agent to look at*, and structured dumps as
  the *oracle*. Golden-image comparison (Phase 5) is tolerance-based and per-lane, and no scenario
  is allowed to depend only on an image.
- **The virtual-monitor strategy depends on Hyprland.** `hyprctl output create headless` was
  verified working on Hyprland 0.56.2 on this box (an isolated 1920×1080 output, captured with
  `grim -o`, removed cleanly). If the compositor changes, the lane abstraction from Phase 1 is the
  single place to add another backend, and the headless lane still works with no compositor at all.
- **Scope creep into "test everything".** The harness exists so an agent can check its own work on
  real surfaces. It is not a replacement for `#[gpui::test]` unit tests, which stay the right tool
  for reducer logic. Phase 5's corpus is deliberately capped at the surfaces a human would click.

## Suggested order

Strictly 1 → 2 → 3 → 4 → 5. Phases 3 and 4 are the only pair that could overlap if two people are
working: Phase 4's fixture and agent work touches `fleet-daemon` and `fleet-harness`, while Phase 3
touches `fleet-app` and `fleet-ui-kit`. They share only the scenario grammar, so they can run in
parallel provided Phase 3 lands its grammar additions first.
