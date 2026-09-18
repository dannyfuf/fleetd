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
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [x] P2-T01 — Add `DelegationId` and the `Delegation` record family to `fleet-core`
  - verified: fleet-core delegation round trips pass in the final workspace suite.
- [x] P2-T02 — Add `MessageOrigin` and `ItemKind::Delegation`, with reducer exemptions
  - verified: fleet-core reducer and fleet-app row tests pass in `make test`.
- [x] P2-T03 — Add `parent` and `delegation` to the thread record and `parent` to the summary
  - verified: daemon manager/store round-trip tests pass in `make test`.
- [x] P2-T04 — Migration 3: `delegations`, `delegation_outbox`, and the three `threads` columns
  - verified: 89 daemon store tests and migration coverage pass.
- [x] P2-T05 — Wire variants, capability string, timeouts and goldens in `fleet-proto`
  - verified: fleet-proto and fleet-client compatibility tests pass in `make test`.
- [x] P2-T06 — Describe the new shapes and the migration in `docs/`
  - verified: documentation shapes were compared with the contract and compiled wire types.

## Notes / decisions log
- 2026-09-18 — Store seams required only the permitted mechanical caller-minted `ItemId` update in a delegation worker test.
- 2026-09-18 — The round-2 golden audit reverified the delegation value, request, response, event and legacy shapes.
- 2026-09-18 — The round-3 audit compared every documented native-agent and delegation shape with the byte-exact protocol goldens; all phase-2 boxes remain verified.

## Follow-ups
- 2026-09-18 — Live delegation tests were NOT run on this machine because no vendor CLIs are installed.
