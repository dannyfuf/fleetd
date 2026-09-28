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
- [x] T02 — Add the session flag, the toggle action, its key, label and help row
  - verified: `cargo test -p fleet-app` filters keymap (35), action_catalogue (12), help (16), palette (50) and the full lib suite (1300) pass; `real_shell_v_toggles_review_worktrees_in_the_worktrees_list` presses `v` twice on the real shell; `make lint` clean.
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
- 2026-09-28 — T02: the flag is `AppState::show_review_worktrees: bool` in `state.rs` beside `pr_tab`/`scope` (the Hub view state lives on `AppState`, not `state/navigation.rs`), `false` in `AppState::new`, never persisted. T03 reads it for the projection key.
- 2026-09-28 — T02: key `v` in `Hub > Worktrees` confirmed free in every Hub context and in KEYMAP.md (Global, Worktrees pane, Explicitly rejected, Conflicts). It already means the watch pane after `ctrl-s` and a selection in Scroll mode; both are pane-disjoint, so the sentence went into "Conflicts noticed and deliberately kept".
- 2026-09-28 — T02: the catalogue has no state-dependent labels (`ActionInfo` holds `&'static str`), so there is one static label. It is "Show or hide review worktrees" (short "Review worktrees"), not the plan's fallback "Toggle review worktrees", because every existing toggle uses "Show or hide …" (`hub::ToggleDetail`, `prefix::ToggleWatchPane`). Not `.featured()`: the six "Here in Worktrees" labels are pinned.
- 2026-09-28 — T02: the handler is `Shell::toggle_review_worktrees` in `shell/root/routing.rs`, registered on the shell root beside `toggle_rail`, not on the Hub root. The palette overlay is not under the Hub root, so only a shell-root listener is reachable by `window.dispatch_action`; T04's subtitle and empty-state buttons should dispatch `worktrees::ToggleReviewWorktrees` the same way.
- 2026-09-28 — T02: the palette needed its own `Command::ToggleReviewWorktrees` (icon `GitPullRequest`) and a `run_command` arm that dispatches the action; the plan's Touches list omitted both palette files. It is listed on the Worktrees screen whatever the connection, since the flag is local.
- 2026-09-28 — T02: the Help overlay has no hand-written table; its `v` row comes from `keymap::table()` plus the catalogue, so "add the row to the help overlay" needed no edit. `docs/UX-SPEC.md` §3.3's Keyboard line gained `v` now (the key ships in T02); the rest of §3.3 stays with T04. `docs/APP-CONTRACTS.md`'s `AppState` field table gained the flag.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Persist the show/hide choice per installation (a Settings row), if users keep toggling it every launch.
- Add a `fleet_core::github` writer for `pull/<n>/head` so the daemon (`create_pr_local`) and the app (`views/detail/pull_request.rs`, `views/prs_screen.rs`) share the format with `pull_request_checkout` instead of formatting it themselves.
