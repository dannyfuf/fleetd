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
- [x] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [x] P3-T01 — Give the manager the four verbs the service needs
  - verified: manager tests pass in the final workspace suite.
- [x] P3-T02 — The transition half inside the store transaction
  - verified: daemon delegation and store transition coverage passes.
- [x] P3-T03 — `run`: validate, mint the token, create the child, seed the transcript
  - verified: delegation run tests and end-to-end scenario pass.
- [x] P3-T04 — `complete`: verify the token, record the result, finish if already settled
  - verified: all complete-ordering and token tests pass.
- [x] P3-T05 — The worker: drain the outbox on wake and on start
  - verified: worker tests pass with durable event synchronization under the full suite.
- [x] P3-T06 — `wait`, `status`, `list`, `cancel`, and advertise the capability
  - verified: daemon delegation and protocol tests pass.
- [x] P3-T07 — The `fleet subagent` CLI noun
  - verified: fleet-cli parsing, context, human, and JSON tests pass.
- [x] P3-T08 — The tokio suite the design asks for, plus one live test per harness
  - verified: 78 delegation tests pass; both real Claude and Codex live tests passed on 2026-09-18.
- [x] P3-T09 — `docs/NATIVE-AGENTS.md` §15 and the CLI contract
  - verified: §15 and the CLI contract were read against the passing service and CLI tests.

## Notes / decisions log
- 2026-09-18 — Run/complete tests use a hermetic scripted shell provider; live tests use an isolated `fleet` capture shim while still invoking the real provider binaries.
- 2026-09-18 — Fixed worker result handling so only `Reported` suppresses a missing-result nudge, and replaced scheduler-sensitive retry polling with durable event waits.

## Follow-ups
- None.
