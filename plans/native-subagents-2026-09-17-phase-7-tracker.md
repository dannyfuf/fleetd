# Native subagents, phase 7: ADR, doctor line and cross-document audit — Tracker
> Plan: ./native-subagents-2026-09-17-phase-7-plan.md
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
- [x] P7-T01 — Write ADR 0017 and index it
  - verified: ADR 0017 exists, follows the adopted/built/amends shape, and is indexed.
- [x] P7-T02 — `fleet doctor` reports whether a harness child can find `fleet`
  - verified: `make doctor` passed after restart and resolved this worktree's `target/debug/fleet`.
- [x] P7-T03 — Record the deferred items in `TODO.md` and `NATIVE-AGENTS.md` §14
  - verified: every deferred design item has the required impact/start/done fields and §14 entry.
- [x] P7-T04 — Cross-document audit and the final verification set
  - verified: lint/check/test, doctor, ten-run recovery, headless subagent scenarios, and both live providers were exercised.

## Notes / decisions log
- 2026-09-18 — `make doctor` initially found protocol 7 and a missing child PATH; restarting this build with `target/debug` on PATH produced a fully green report.
- 2026-09-18 — The plan's short live-test filter matched zero tests; the documented full module path ran both Claude and Codex tests successfully.

## Follow-ups
- Re-run `make harness` in the virtual lane when a compositor is available; the attempted run fell back headless and stopped at the first screenshot scenario.
