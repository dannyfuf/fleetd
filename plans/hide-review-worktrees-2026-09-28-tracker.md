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
- [x] T03 — Exclude hidden review worktrees from the Hub projection and every count that must agree with it
  - verified: `cargo test -p fleet-app` filters hub (77), repos_rail (14), title (20), worktrees_list (25), filter (34) and the full lib suite pass; three projection tests in `screens/hub/tests.rs` and one title test pin the rule; `make lint` clean.
- [x] T04 — Say what is hidden, mark what is shown, and keep the empty state honest
  - verified: `cargo test -p fleet-app` filters worktrees_list (28), first_run (9) and hub (77) pass; `make lint` clean; `make harness-one SCENARIO=scenarios/hub/worktrees-pointer.scenario` ok (43 steps, the worktrees-page shot looks as before since no fixture holds a `pull/` worktree). `make run` not run (it replaces the user's daemon); T06 covers the page end to end.
- [x] T05 — Keep the cursor and selection stable across the toggle
  - verified: `cargo test -p fleet-app --lib screens::hub::tests::` (58 passed) and `cargo test -p fleet-app --lib review` (49 passed), covering five Hub tests that toggle both ways and assert the cursor, anchor, detail row, harness row and inspection target agree after every toggle, plus `real_shell_v_moves_the_cursor_off_a_hidden_review_row_and_keeps_it_there` pressing `j`/`v` on the real shell; `make lint` clean. No production change: `reconcile_index` already clamps.
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
- 2026-09-28 — T03: the partition runs right after the context filter and before the glyph loop, so rows, the subtitle, rail counts, rail glyphs and issue chips all see one slice. `rail_rows` now takes `worktrees: &[&Worktree]` (the context-scoped slice, reviews removed, before the RepoScope retain) instead of `snapshot.worktrees`, which avoids cloning worktrees per revision and keeps other repos' counts when one repo is selected.
- 2026-09-28 — T03: `HubModel.review_hidden` / `DisplayedHub.review_hidden` count hidden review worktrees inside the RepoScope (not the whole context), so the subtitle and T04's empty state agree with the rows under them. It is 0 while reviews are shown; no `review_total` field was added — T04 can count shown reviews from its rows once `WorktreeRow` carries `review`.
- 2026-09-28 — T03: the flag is a `ProjectionKey` field (`show_reviews`), so toggling rebuilds the cached `Rc<HubModel>`. `PrInputs.worktrees` still gets every worktree: the PR screen keeps linking a PR to its review worktree.
- 2026-09-28 — T03: `hub_place` (title bar) applies the same exclusion; it still counts the whole context and ignores RepoScope, as before, so it equals the subtitle only under `All`.
- 2026-09-28 — T03: UX-SPEC §5 invariant 6 ("No surface auto-hides a failure") contradicted the plan's accepted risk. Followed the plan and wrote the exception into invariant 6: a hidden review worktree's failed hook or unreachable host lights no rail glyph or issue chip until `v` shows reviews. The rail rows in §3.2, the section nav in §2.2, the §3.3 subtitle numbers and the §3.10 match count now say hidden reviews are not counted; APP-CONTRACTS `displayed_hub` names `review_hidden`.
- 2026-09-28 — T04: `WorktreeRow.review: Option<u64>` (the PR number, from `pull_request_checkout`) drives a muted `review` chip (`Chip::new().text("review").tone(Tone::Muted)`, no icon) after the dirty words and before the host chip. No ui-kit change: the "muted chip variant" is `Tone::Muted`.
- 2026-09-28 — T04: the hidden/shown segment is not part of the `summary` string. `worktrees_list::review_fact(hidden, shown) -> Option<SharedString>` builds `<k> review worktree(s) hidden` / `<k> review worktree(s)` / nothing, stored as `HubModel.worktree_reviews` beside `worktree_summary` (built in the projection, never in render; `shown` counts review rows before the filter). The view draws it through `PageHeader::fact` after a faint `·`, as a compact ghost `Button` (`worktrees.reviews`) that dispatches `worktrees::ToggleReviewWorktrees`; its tooltip reads `Show review worktrees` / `Hide review worktrees` plus the live key. No `show_kbd`: the page keeps keys in tooltips (ADR 0023).
- 2026-09-28 — T04: `summary(rows, repo, review_hidden)` says `No worktrees of your own yet` when the scope's only worktrees are hidden reviews, instead of the false `No worktrees yet`.
- 2026-09-28 — T04: the empty surface is chosen in `views/worktrees_list.rs` by the pure `empty_surface(query, has_repos, scope_repo, review_hidden)` (plan said composition): Filter, then NoRepos, then the new `EmptySurface::WorktreesReviewsHidden`, then Worktrees/WorktreesRepo. It stays two lines (C3 resolved without a kit change): the fact `No worktrees of your own yet · <k> review worktrees hidden` (the count rides in through the `{}` scope) over one `Show review worktrees` button (`worktrees.empty.reviews`, default style, not primary). `EmptySurface::render` gets no theme for a two-button row, and `New worktree` stays reachable in the header toolbar.
- 2026-09-28 — T04: the "hidden by default, `v` shows it" sentence went into both `docs/BOARD.md`'s Reviews-board intro and UX-SPEC §3.5's Review tab. `docs/TESTING-HARNESS.md` names the new targets `worktrees.reviews` and `worktrees.empty.reviews` now (additive); the `review` mark stays with T06.
- 2026-09-28 — T04: the Linux virtual lane has no recorded baselines for `worktrees-pointer` (`scenarios/baselines/virtual/hub/worktrees-pointer/*` missing), so `make harness-one` there checks steps, not pixels.
- 2026-09-28 — T05: no production change. `navigation::reconcile_index` already lands on a valid row when the anchored row disappears: the cursor keeps its index (clamped to the new last row) and the anchor is overwritten with that row's id, so `v` again keeps the neighbour instead of jumping back to the review row. An empty list (every row a hidden review) keeps the anchor, so showing reviews returns to the review row. `Shell::toggle_review_worktrees` stays a plain flag flip plus notify; it deliberately does not reset the cursor to row 0 the way a filter change (`FilteredPane::reset_cursor` / `forget_anchor`) does, since §3.3 allows but does not require a move and keeping the user's row is the sensible one. UX-SPEC §3.3 "Cursor stability" now names `v` and this rule.
- 2026-09-28 — T05 (C2 from the docs sweep): Back from a review worktree opened from the Reviews board, the PR screen or the palette while reviews are hidden names a row the list does not draw. `reconcile_selection` consumes `pending_worktree_focus` and the cursor stays on the row it was on; reviews are not shown automatically (that would flip a session flag the user did not touch). UX-SPEC §3.1's "Back always lands … on the worktree just left" gained that exception, pinned by `back_to_a_hidden_review_worktree_leaves_the_cursor_where_it_was`. If the scope's only rows are hidden reviews, the pending focus waits (rows are empty) and `v` then lands on it.
- 2026-09-28 — T05: the plan's `cargo test -p fleet-app hub::navigation` matches nothing (navigation.rs has no test module); the tests live in `screens/hub/tests.rs` and `shell/root/tests.rs`. The Hub tests run connected on the list pane so the inspection target is asserted too; `close_requests` closes the sweep and debounced-inspection requests before teardown, or the tasks awaiting them leak the entities.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Persist the show/hide choice per installation (a Settings row), if users keep toggling it every launch.
- Add a `fleet_core::github` writer for `pull/<n>/head` so the daemon (`create_pr_local`) and the app (`views/detail/pull_request.rs`, `views/prs_screen.rs`) share the format with `pull_request_checkout` instead of formatting it themselves.
- Consider showing review worktrees automatically when Back leaves a review worktree opened from the Reviews board, the PR screen or the palette, if landing on another row surprises users (UX-SPEC §3.1 exception added in T05).
