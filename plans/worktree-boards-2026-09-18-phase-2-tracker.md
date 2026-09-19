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
- [x] I have read the plan end to end.
- [x] Phase 1 is merged and `fleet board --worktree show` works against the running daemon.
  - phase 1 is on this branch (all phases ship together per the 2026-09-19 decision); the CLI
    check was done in P1-T07 against a private daemon, not the user's live one.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline (`make lint && make test && make harness`).
  - verified 2026-09-19 on `e5ee8d7` (phase 3 gate): `make lint` clean, `make test` 3272 passed,
    `make harness` 64 of 64.
- [x] I am ready to start.

## Tasks
- [x] P2-T01 — Reserve `fleet://board` as a native command
  - verified: `cargo test -p fleet-core config` (18), `cargo test -p fleet-daemon` (771 lib + 11
    sessions_lifecycle), `cargo test -p fleet-app settings`, workspace clippy clean. The only
    degradation site is `fleet-core::sessions::default_terminals`, now routed through
    `config::proxied_degradation` (lazygit degrades, board never does); the settings schema already
    keyed on `is_native_command`, so a test pins the display instead.
- [~] P2-T02 — Give `BoardState` a scope and make the loader scope-aware
- [ ] P2-T03 — Add `ctrl-s b` to open or select the board tab
- [ ] P2-T04 — Render the board pane inside the Workspace
- [ ] P2-T05 — Drive the tab with a harness scenario
- [ ] P2-T06 — Reconcile the UX, keymap and contract docs

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-19 — Executor: Opus agents on the host (Codex out of credits until Sep 22). P2-T01 and
  P2-T02 run in parallel on disjoint files; the harness runs only from the orchestrator.
- 2026-09-18 — Plan written. Decisions fixed at planning time: the surface is a `fleet://board`
  native tab in the Workspace (not a Hub scope switcher); one `BoardState` with a scope; the tab
  is created on demand by `ctrl-s b` and not added to the default `windows[]`; no new ui-kit
  components; no tab badge.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Consider a worktree-board open count on the Workspace tab and in the Hub worktrees list, mirroring the Hub Board tab badge.
- Consider adding `fleet://board` to the default `windows[]` if users want the tab to survive sleep/wake without `ctrl-s b`.
