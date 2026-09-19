# Worktree-scoped boards — Phase 1: model, daemon, protocol, client and CLI — Tracker
> Plan: ./worktree-boards-2026-09-18-phase-1-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline (`make lint && make test`).
  - verified: orchestrator `make lint` was green (`/tmp/wb-baseline-lint-p1.log`); orchestrator
    `make test` completed with `exit=0` (`/tmp/wb-baseline-test-p3.log`).
- [x] I am ready to start.

## Tasks
- [x] P1-T01 — Add the worktree scope to the core board model
  - verified: `cargo test -p fleet-core board` (117 passed) and `make lint` passed.
- [ ] P1-T02 — Make board lookup and listing scope-aware in the daemon
- [ ] P1-T03 — Add ensure/create for worktree boards to the `Boards` service
- [ ] P1-T04 — Cascade board deletion when a worktree is deleted or pruned
- [ ] P1-T05 — Add the wire requests, the capability string and the dispatch arms
- [ ] P1-T06 — Add the typed client methods
- [ ] P1-T07 — Add the `--worktree` selector to `fleet board`
- [ ] P1-T08 — Record the decision and finish the docs

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-18 — Plan written. Decisions fixed at planning time: scope is a `worktree_id` field on
  `Board` (not a separate store or a scope enum); one board per worktree; board id derived from
  the worktree id but never trusted for lookup; deletion cascade via a late-bound observer on
  `Worktrees`; `board.worktree` capability, `PROTOCOL_VERSION` stays 8.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Expose `DeleteBoard` as `fleet board delete` so an optional worktree board can be detached without deleting the worktree.
