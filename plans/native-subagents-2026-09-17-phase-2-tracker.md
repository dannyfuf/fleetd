# Native subagents, phase 2: delegation model, wire variants and storage — Tracker
> Plan: ./native-subagents-2026-09-17-phase-2-plan.md
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
- [ ] P2-T01 — Add `DelegationId` and the `Delegation` record family to `fleet-core`
- [ ] P2-T02 — Add `MessageOrigin` and `ItemKind::Delegation`, with reducer exemptions
- [ ] P2-T03 — Add `parent` and `delegation` to the thread record and `parent` to the summary
- [ ] P2-T04 — Migration 3: `delegations`, `delegation_outbox`, and the three `threads` columns
- [ ] P2-T05 — Wire variants, capability string, timeouts and goldens in `fleet-proto`
- [ ] P2-T06 — Describe the new shapes and the migration in `docs/`

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)
