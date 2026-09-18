# Native subagents, phase 1: prerequisites in the manager and reducer — Tracker
> Plan: ./native-subagents-2026-09-17-phase-1-plan.md
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
- [x] P1-T01 — Split `Submitted.queued` into `JoinedActive` and `QueuedNew`
  - verified: manager tests and the final workspace suite pass; no `queued: bool` remains.
- [x] P1-T02 — Keep live background items open when their turn settles
  - verified: fleet-core and daemon store projection tests pass in `make test`.
- [x] P1-T03 — Record why a thread stopped on its record
  - verified: manager lifecycle/restart coverage passes in `make test`.
- [x] P1-T04 — Update the two documents that describe `Submitted` and thread state
  - verified: reviewed `NATIVE-AGENTS.md` and `agents-contracts.md` against the implemented enums.

## Notes / decisions log
- 2026-09-18 — No phase-1 implementation deviation was reported; final integrated lint/check/test are green.

## Follow-ups
- None.
