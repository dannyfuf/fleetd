# Native subagents, phase 3: delegation service and the `fleet subagent` CLI — Tracker
> Plan: ./native-subagents-2026-09-17-phase-3-plan.md
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
- [ ] P3-T01 — Give the manager the four verbs the service needs
- [ ] P3-T02 — The transition half inside the store transaction
- [ ] P3-T03 — `run`: validate, mint the token, create the child, seed the transcript
- [ ] P3-T04 — `complete`: verify the token, record the result, finish if already settled
- [ ] P3-T05 — The worker: drain the outbox on wake and on start
- [ ] P3-T06 — `wait`, `status`, `list`, `cancel`, and advertise the capability
- [ ] P3-T07 — The `fleet subagent` CLI noun
- [ ] P3-T08 — The tokio suite the design asks for, plus one live test per harness
- [ ] P3-T09 — `docs/NATIVE-AGENTS.md` §15 and the CLI contract

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)
