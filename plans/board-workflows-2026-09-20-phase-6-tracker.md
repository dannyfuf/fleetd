# Board workflows, phase 6: app — the board face — Tracker
> Plan: ./board-workflows-2026-09-20-phase-6-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- A contract gap in the plan is a stop-and-ask, not a decision to make alone; record the answer here.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [ ] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [ ] I am ready to start.

## Tasks
- [ ] P6-T01 — `RunMark`, `BlockedTone`, `CardTile::run`/`.blocked`, `KanbanColumn::action`
- [ ] P6-T02 — `CardMarks`, `TileMarks` and `refresh_card_marks`
- [ ] P6-T03 — The memo key, `CardRow.run`/`.blocked`, tiles, `⚡` and the header counts
- [ ] P6-T04 — Additive harness marks, `board.summary`, `docs/TESTING-HARNESS.md` §3
- [ ] P6-T05 — A fixture that can show a run, and `scenarios/board/workflow-marks.scenario`
- [ ] P6-T06 — `docs/UX-SPEC.md` Board chapter and `docs/DESIGN-SYSTEM.md` glyphs

## Notes / decisions log

## Follow-ups
