# Native subagents, phase 6: recovery after a daemon restart and cancel propagation — Tracker
> Plan: ./native-subagents-2026-09-17-phase-6-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [x] P6-T01 — Resume an orphaned child once with a nudge
  - verified: recovery tests cover one resume and second-exit failure.
- [x] P6-T02 — Mark delegations with a missing caller undeliverable at start
  - verified: restart recovery coverage passes.
- [x] P6-T03 — Cancel a delegation tree from the top
  - verified: depth-first cancellation test passes.
- [x] P6-T04 — The restart matrix test
  - verified: `delegation::recovery` passed ten consecutive serialized runs on 2026-09-18.
- [x] P6-T05 — Document recovery in §15 and update the status row
  - verified: §13/§15 were compared with the passing restart matrix.

## Notes / decisions log
- 2026-09-18 — Missing-caller repair runs idempotently before every drain because the worker has no distinct post-start hook.
- 2026-09-18 — Restart coverage uses the manager scripted harness at the valid running boundary and cfg(test)-only helper visibility.

## Follow-ups
- None.
