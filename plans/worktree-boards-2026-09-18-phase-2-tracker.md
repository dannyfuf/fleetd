# Worktree-scoped boards — Phase 2: the `fleet://board` Workspace tab — Tracker
> Plan: ./worktree-boards-2026-09-18-phase-2-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [ ] I have read the plan end to end.
- [ ] Phase 1 is merged and `fleet board --worktree show` works against the running daemon.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline (`make lint && make test && make harness`).
- [ ] I am ready to start.

## Tasks
- [ ] P2-T01 — Reserve `fleet://board` as a native command
- [ ] P2-T02 — Give `BoardState` a scope and make the loader scope-aware
- [ ] P2-T03 — Add `ctrl-s b` to open or select the board tab
- [ ] P2-T04 — Render the board pane inside the Workspace
- [ ] P2-T05 — Drive the tab with a harness scenario
- [ ] P2-T06 — Reconcile the UX, keymap and contract docs

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-18 — Plan written. Decisions fixed at planning time: the surface is a `fleet://board`
  native tab in the Workspace (not a Hub scope switcher); one `BoardState` with a scope; the tab
  is created on demand by `ctrl-s b` and not added to the default `windows[]`; no new ui-kit
  components; no tab badge.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Consider a worktree-board open count on the Workspace tab and in the Hub worktrees list, mirroring the Hub Board tab badge.
- Consider adding `fleet://board` to the default `windows[]` if users want the tab to survive sleep/wake without `ctrl-s b`.
