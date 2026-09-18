# Native subagents, phase 5: the agents picker — Tracker
> Plan: ./native-subagents-2026-09-17-phase-5-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [x] P5-T01 — The `AGENTS` palette section
  - verified: 31 palette tests pass.
- [x] P5-T02 — `^s d` seeds the palette to `AGENTS`
  - verified: keymap and prefix tests pass in `make test`.
- [x] P5-T03 — Selection: attach, reopen, or switch session then attach
  - verified: palette branch tests and all three picker scenarios pass headlessly.
- [x] P5-T04 — Harness scenarios: attach from the picker, reopen a closed caller, other-worktree child
  - verified: all three scenarios passed in the final 55-scenario virtual-lane corpus.
- [x] P5-T05 — Document the section and the chord
  - verified: UX, KEYMAP, and NATIVE-AGENTS text matches the tested picker behavior.

## Notes / decisions log
- 2026-09-18 — Fixed cross-worktree picker selection to ensure the destination session before attach; the specialized two-worktree preset avoids duplicating `other`.
- 2026-09-18 — The final full virtual-lane corpus passed, including all three `AGENTS` picker paths.

## Follow-ups
- 2026-09-18 — Live delegation tests were NOT run on this machine because no vendor CLIs are installed.
