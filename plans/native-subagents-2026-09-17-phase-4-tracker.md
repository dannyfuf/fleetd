# Native subagents, phase 4: app rows, child tabs and attach — Tracker
> Plan: ./native-subagents-2026-09-17-phase-4-plan.md
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
- [x] P4-T01 — Teach the harness to spawn a real delegation from a scripted caller
  - verified: `subagent-runs-end-to-end` passes in the headless harness suite.
- [x] P4-T02 — `DelegationRow` and `DelegationResultCard` in the kit
  - verified: fleet-ui-kit row coverage and example build pass.
- [x] P4-T03 — Delegation state, the `attached` set and `reopen` in the app
  - verified: app agent-state tests pass in `make test`.
- [x] P4-T04 — Project the delegation row and the result card into the transcript
  - verified: agent-thread row/view tests and result-card scenario pass.
- [x] P4-T05 — The child tab: title, caller segment, placeholder, `^s u`, `^s x` detaches
  - verified: workspace tests and attach/detach/up-to-caller scenarios pass headlessly.
- [x] P4-T06 — Attention bubbles up from children to the caller
  - verified: state/notification tests and blocked-child scenario pass.
- [x] P4-T07 — Harness scenarios for the row, attach, detach, blocked child and `^s u`
  - verified: all five scenarios passed headlessly; virtual execution was attempted but no compositor is available.
- [x] P4-T08 — Update the four authoritative documents for the surfaces added
  - verified: UX, keymap, native-agent, and harness docs were checked against scenarios and code.

## Notes / decisions log
- 2026-09-18 — Fixed durable-delegation/caller-item ordering in the app and added the permitted harness snapshot fields and fixtures.
- 2026-09-18 — Corrected KEYMAP wording: Enter attaches on delegation rows, while result-card Enter expands and only delegation rows cancel.

## Follow-ups
- Re-run the screen corpus in the virtual lane when a compositor is available; this machine only provided headless coverage.
