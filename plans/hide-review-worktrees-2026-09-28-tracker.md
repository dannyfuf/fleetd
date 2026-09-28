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
- [x] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline (`make lint`, `make test`, `make harness-headless`).
- [ ] I am ready to start.

## Tasks
- [x] T01 — Recognise a pull-request checkout in `fleet-core`
  - verified: `cargo test -p fleet-core github` (5 passed, 0 failed) and `make lint` clean. `fleet_core::github::pull_request_checkout(&Worktree) -> Option<u64>` is the one fleet-core definition of a review worktree; `worktree_matches_pr` calls it.
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
- 2026-09-28 — T01: the helper is `fleet_core::github::pull_request_checkout(worktree: &Worktree) -> Option<u64>`, with a private `parse_pull_head(&str)` so tests feed raw refs. Later tasks call `pull_request_checkout(w).is_some()` to ask "is this a review worktree".
- 2026-09-28 — T01: only the canonical `pull/<n>/head` counts, meaning ASCII digits with no sign and no leading zero (except `0` itself). The old `worktree_matches_pr` compared against `format!("pull/{n}/head")`, so a naive `u64` parse would have widened matching (`pull/+412/head`, `pull/0412/head`) and hidden more worktrees in T03. Matching behaviour is unchanged.
- 2026-09-28 — T01: "one definition" holds within `fleet-core` only. The daemon (`services/worktrees/creation.rs`, `create_pr_local`) and the app (`views/detail/pull_request.rs`, `views/prs_screen.rs`) still build the `pull/<n>/head` string themselves. They write it and do not parse it, so they were left alone.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Persist the show/hide choice per installation (a Settings row), if users keep toggling it every launch.
- Add a `fleet_core::github` writer for `pull/<n>/head` so the daemon (`create_pr_local`) and the app (`views/detail/pull_request.rs`, `views/prs_screen.rs`) share the format with `pull_request_checkout` instead of formatting it themselves.
