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
- [ ] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [ ] I am ready to start.

## Tasks
- [ ] P1-T01 — Split `Submitted.queued` into `JoinedActive` and `QueuedNew`
- [ ] P1-T02 — Keep live background items open when their turn settles
- [ ] P1-T03 — Record why a thread stopped on its record
- [ ] P1-T04 — Update the two documents that describe `Submitted` and thread state

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)
