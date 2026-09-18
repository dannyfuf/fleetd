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
- [ ] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [ ] I am ready to start.

## Tasks
- [ ] P6-T01 — Resume an orphaned child once with a nudge
- [ ] P6-T02 — Mark delegations with a missing caller undeliverable at start
- [ ] P6-T03 — Cancel a delegation tree from the top
- [ ] P6-T04 — The restart matrix test
- [ ] P6-T05 — Document recovery in §15 and update the status row

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)
