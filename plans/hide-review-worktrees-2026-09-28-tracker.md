# Hide pull-request review worktrees from the Worktrees list — Tracker
> Plan: ./hide-review-worktrees-2026-09-28-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [ ] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline (`make lint`, `make test`, `make harness-headless`).
- [ ] I am ready to start.

## Tasks
- [ ] T01 — Recognise a pull-request checkout in `fleet-core`
- [ ] T02 — Add the session flag, the toggle action, its key, label and help row
- [ ] T03 — Exclude hidden review worktrees from the Hub projection and every count that must agree with it
- [ ] T04 — Say what is hidden, mark what is shown, and keep the empty state honest
- [ ] T05 — Keep the cursor and selection stable across the toggle
- [ ] T06 — Harness: a `review` mark, the new target, a fixture with a review worktree, and a scenario
- [ ] T07 — Quality gate and doc sweep

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-28 — Plan written. Decisions already taken and why:
  - "Review worktree" means `Worktree.base_ref == "pull/<n>/head"`. It is what the daemon writes for every from-PR creation and what `worktree_matches_pr` already keys on, so no field, wire change or migration is needed.
  - Hidden by default, session-only toggle on `v` (to be confirmed against `docs/KEYMAP.md` in T02). Persisting the choice is a follow-up, not part of this change.
  - Subtitle, rail counts and context-bar count all exclude hidden review worktrees so that numbers match rows.
  - CLI `fleet list`, the palette `@` scope and the Workspace switcher keep every worktree.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Persist the show/hide choice per installation (a Settings row), if users keep toggling it every launch.
