# PR review boards, phase 4: the Review tab board and the Schedules section — Plan
> Tracker: ./pr-review-boards-2026-09-22-phase-4-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.
> For this initiative the tracker's truth is the worktree board `dannyfuf/fleetd#feat-board-workflow-schedule-custom-task` (prefix FEA).

## Summary

The GPUI app catches up with phases 2 and 3:

- The Pull requests screen's **Review** tab stops listing `gh` results and shows the context's
  **Reviews board** instead, with the same kanban, cards, detail and run keys the other boards have.
- A review card shows its pull request.
- Board settings gains a **Schedules** section.
- The board header shows each schedule's next run and last result.
- The context-bar Review chip counts the reviews that are waiting on the user.
- The **Mine** tab is unchanged.
- The harness pins the new surfaces.

Shared names come from `./pr-review-boards-2026-09-22-contracts.md`.

## Sizing call

Phased: phase 4 of 4. It touches only the app, the ui-kit, the docs and the harness. That is one
surface and one pull request, split into seven cards so each one is reviewable.

## Repository context

- **PR screen:**
  - `crates/fleet-app/src/views/prs_screen.rs` (701 lines; `PrRow` around 34-62, header around 257-276, error row 346-364) and `crates/fleet-app/src/views/detail/pull_request.rs`.
  - The tab model is `PrTab { Mine, Review }` in `crates/fleet-core/src/github.rs` around 10-15.
  - Fetching is `crates/fleet-app/src/screens/hub/cache.rs` (`tick_pull_requests` around 417, `fetch_pull_requests` around 300, `review_pr_count` set around 410).
  - The Review chip is read in `shell/chrome.rs` around 32-35. Actions are in `screens/hub/actions.rs` around 443.
- **Hub layout:** `screens/hub/composition.rs` around 9-40; tabs in `screens/hub.rs` around 537-575; `HubTab` in `state/navigation.rs` around 18-25.
- **Board:**
  - `state/board.rs`: `BoardScope { Context, Worktree }` around 30; `BoardState`; the `marks` fold is `AppState::refresh_card_marks`.
  - `screens/board.rs` + `screens/board/{actions,lifecycle,navigation,projection,runs}.rs`: `enter_context_scope` / `enter_worktree_scope` in `lifecycle.rs` around 44-95.
  - `views/board_screen.rs` (the pane; header around 198-268).
  - `dialogs/card_detail*.rs`, `views/board_card_detail*.rs`.
- **Board settings:** `dialogs/board_settings.rs` + `board_settings/{columns,draft,keys,persistence,schema,view}.rs`. `BoardSection { General, Backend, Columns }` is in `schema.rs` around 8-29; `automation_locked` is in `persistence.rs` around 28.
- **UI kit:** `crates/fleet-ui-kit/src/components/{card_tile,kanban_column}.rs`. Kit components take no domain types and no literal colours or sizes (`docs/DESIGN-SYSTEM.md`).
- **Keymap:**
  - `crates/fleet-app/src/keymap.rs`: `Hub > Board` bindings around 444-460; `Hub > Prs` around 637-649 (`tab`/`l` next tab, `h` previous, `enter`/`o` open, `c`, `I`, `y`, `r`, `p`/`q` back); run keys `A`/`X`/`>` only in `Workspace > Native > Board` and `Dialog > CardDetail` (around 507-527).
  - Keymap docs: `docs/KEYMAP.md` (PR screen around 123-134).
- **Specs:** `docs/UX-SPEC.md` §3.5 "Hub — Pull requests screen" (lines 528-630); the "## Board" section (1954-2290); Board settings in §3.8.6 (1340+); `docs/BOARD.md` §8 and §11.8-§11.10; `docs/APP-CONTRACTS.md` ("render prepares nothing").
- **Harness:**
  - `scenarios/board/*.scenario` and `scenarios/workspace/board-tab.scenario`; `scenarios/hub/go-jumps` and `pointer-basics` touch the PR tab.
  - Fixtures are in `crates/fleet-harness/src/fixture/plan.rs` (PRs carry `tab: mine|review` around 166-212), and the fake `gh` is in `fixture/tools.rs` around 294.
  - `docs/TESTING-HARNESS.md` §9 (620-646) holds the scenario rules. The document is **frozen**: a new target name, command or fixture field must follow its own amendment rule. Read it before P4-T07.
- **Verification:** `make lint`, `make test`, `make harness`, and `make run` / the `run` skill to see it.
- **Skills to load first:**
  - `gpui-state-and-memory` (scope, subscriptions, notify);
  - `gpui-components` (tile slots, settings pane rows);
  - `gpui-styling` (every colour and size from tokens);
  - `gpui-performance` (render prepares nothing: fold marks and schedule strings outside render);
  - `gpui-app-shell` (key contexts, focus, dialogs);
  - `rust-gpui-testing` (GPUI tests and harness);
  - `zed-quality-review` before done.

## Assumptions

- The Mine tab keeps its `gh`-backed list, fetch and cache exactly as today.
- On a Reviews board, `p` keeps meaning "back to worktrees", as everywhere on the PR screen; priority is set from the card detail.

## Out of scope

- A multi-board switcher in the Hub.
- Editing a schedule's log or viewing it inside the app. The Schedules section shows the log path, and a follow-up can open it in a terminal tab.

## Affected areas

- `crates/fleet-app/src/{state/board.rs,state/navigation.rs,state/connection.rs,screens/board/*,screens/hub/*,views/prs_screen.rs,views/board_screen*,views/board_card_detail*,dialogs/board_settings*,dialogs/card_detail*,keymap.rs,actions.rs,shell/chrome.rs}`
- `crates/fleet-ui-kit/src/components/card_tile.rs`, the ui-kit gallery
- `crates/fleet-harness/src/fixture/*` if a fixture field is needed; `scenarios/prs/*` (new), `scenarios/board/*`
- `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/BOARD.md` §8 and §11.8-§11.10, `docs/TESTING-HARNESS.md` (only through its amendment rule)

## Tasks

### P4-T01 — Point the board mirror at a context's Reviews board
- **Intent:** the app can load, refresh and act on a Reviews board through a new `BoardScope::Reviews(ContextId)`. The scope is gated on `board.reviews`, and a daemon without it produces a clear message.
- **Touches:** `crates/fleet-app/src/state/board.rs`, `screens/board/lifecycle.rs`, `state/connection.rs` (capability flags; grep `BOARD_AUTOMATION_CAPABILITY` in the app for how capabilities are read), app tests next to the existing board-scope tests, `docs/BOARD.md` §8.
- **Steps:**
  - Load `gpui-state-and-memory` and read `BoardScope` and the scope comments in `state/board.rs`. There is one board mirror, and the scope says which board it shows.
  - Add `BoardScope::Reviews(ContextId)` with a doc line: `The context's Reviews board — EnsureReviewsBoard(context).`
  - Add `enter_reviews_scope(context, state, bridge, cx)` in `lifecycle.rs`, copying `enter_context_scope`. It sends `RequestBody::EnsureReviewsBoard { context_id }` and handles the answer exactly as the context scope does.
  - `BoardChanged` refresh: find where the mirror re-requests after a `BoardChanged` (the research found `EnsureBoard` / `EnsureWorktreeBoard` re-sent on the event) and add the Reviews arm, so the reviews board re-requests `EnsureReviewsBoard`.
  - Capability: add `REVIEW_BOARDS_UNSUPPORTED: &str = "this daemon does not support review boards; run \`fleet daemon restart\`"` next to `WORKTREE_BOARDS_UNSUPPORTED`, and show it the same way when the daemon lacks `board.reviews`.
  - Run marks: `refresh_card_marks` must fold Reviews boards too. Check that nothing in it tests for `BoardScope::Worktree` specifically, and fix any spot that does.
  - GPUI tests (see `rust-gpui-testing`; mirror the existing scope tests):
    - entering the reviews scope sends `EnsureReviewsBoard`;
    - a `BoardChanged` for that board re-requests it;
    - a daemon without the capability shows the sentence and sends nothing.
  - Docs: BOARD.md §8, the scope list.
- **Verification:** `cargo test -p fleet-app board`, `make lint`.
- **Done when:** the three tests pass and existing scope tests are unchanged.

### P4-T02 — Show the Reviews board in the Pull requests screen's Review tab
- **Intent:** selecting Review on the PR screen shows the Reviews board pane in place of the flat list, with board keys working and Tab / Shift-Tab still switching tabs; the Review chip counts cards waiting on the user; the `gh` review-requested fetch is retired.
- **Touches:** `crates/fleet-app/src/views/prs_screen.rs`, `screens/hub/{composition.rs,cache.rs,actions.rs}`, `shell/chrome.rs`, `keymap.rs`, `actions.rs` if new actions are needed, the app tests for the PR screen and the chip, `docs/UX-SPEC.md` §3.5 and §2.3 (chip), `docs/KEYMAP.md`.
- **Steps:**
  - Read UX-SPEC §3.5 in full first: purpose line, column ladder, detail panel, and what it leaves out.
  - Composition: when `HubTab::Prs` is active and the PR tab is `Review`, enter `BoardScope::Reviews(active context)` and render the board pane (the same view the Hub's Board tab renders) in the list and detail area. The repos rail is hidden, as it is on the Board tab: a Reviews board spans repositories. Leaving the tab restores the Hub board's previous scope, following how switching between the Hub's Board tab and the Workspace board already restores scope.
  - Key context: give the board pane the context `Hub > Prs > Board` (nested, so it inherits `Hub > Prs`). Bind the whole `Hub > Board` set there, with these exceptions:
    - `tab` / `shift-tab` stay `prs::NextTab` / `prs::PrevTab`;
    - `h` / `l` become columns, overriding the `Hub > Prs` tab bindings in this context only;
    - `p` / `q` stay `prs::Back`;
    - add the run keys `A`, `X`, `>` (the same actions `Workspace > Native > Board` binds).

    Put every binding in `keymap.rs` in the same table style. `docs/KEYMAP.md` must gain a "Hub › Pull requests › Review board" block listing each key.
  - Retire the review fetch:
    - `fetch_pull_requests` asks only for `PrTab::Mine` when the daemon has `board.reviews`, and for both tabs otherwise, so an old daemon keeps the old list;
    - the Review tab label count becomes the number of cards in `Started` columns without a live run plus `attention_count`, from the board summary the app already holds. Fold it where `review_pr_count` is set today and never compute it in render.
  - The chip (`shell/chrome.rs`) shows the same number. Read the chip's spec at UX-SPEC §2.3 line 148 and update its source sentence.
  - Empty state: a Reviews board with no cards shows `No reviews yet.` plus, when it has no schedule (phase 3, gated on `schedules`), `⏎ add the GitHub review schedule`. `Enter` there opens Board settings on the Schedules section with the starter draft (P4-T05). Put the sentence in UX-SPEC §3.13, next to `No PRs waiting for your review in <scope>.`, which this replaces.
  - Tests:
    - on the Review tab the board scope is Reviews and the flat list is not built;
    - `tab` returns to Mine;
    - `l` moves a column;
    - `p` goes back;
    - the chip count comes from the board;
    - the Mine fetch is unchanged;
    - an old daemon still shows the flat review list.
  - Docs:
    - UX-SPEC §3.5: rewrite the Review tab's paragraphs, keep Mine's, and update the purpose line only if needed;
    - KEYMAP: the new block;
    - BOARD.md §8: that the Reviews board is shown in the PR screen.
- **Verification:** `cargo test -p fleet-app prs`, `cargo test -p fleet-app hub`, `make lint`, `make harness` (existing PR-tab scenarios must still pass; `hub/go-jumps` awaits `lists.prs.rows[0]` on the default Mine tab and must be unaffected).
- **Done when:** the Review tab is the board, every listed test passes, and existing harness scenarios pass.

### P4-T03 — Show a card's pull request on its tile and in its detail
- **Intent:** a review card's tile carries a small `owner/name#123` reference; the card detail has a `Pull request` row with open-in-browser and copy; `b` / `y` work on a selected review card.
- **Touches:** `crates/fleet-ui-kit/src/components/card_tile.rs`, the ui-kit gallery entry for `CardTile`, `crates/fleet-app/src/views/board_screen/model.rs` (projection), `views/board_card_detail*.rs`, `dialogs/card_detail*`, `keymap.rs`, `screens/board/actions.rs`, tests, `docs/BOARD.md` §11.9, `docs/UX-SPEC.md` (Board: card tile and card detail rows), `docs/DESIGN-SYSTEM.md` (if the tile's slot list is documented there).
- **Steps:**
  - Load `gpui-components` and `gpui-styling`.
    - The kit tile must not know what a pull request is: add a generic optional `reference: Option<SharedString>` slot (a short muted mono line under the title, or next to the key; follow what the tile already does with the key).
    - Every colour and size comes from the theme and tokens.
    - Add the slot to the gallery, since the skill treats the gallery as the acceptance test.
  - Projection: fill `reference` with `pull_request.key()` when the card has one. Do it in the projection memo, never in render.
  - Card detail: add a `Pull request` fact row, `owner/name#123 · open ↗`, in the same row style as `Worktree`. `Enter` on it opens the URL, using how the PR screen opens a browser (`screens/hub/actions.rs`, `b`).
  - Keys: bind `b` (open the pull request) and `y` (copy its URL) in `Hub > Prs > Board` and in `Dialog > CardDetail` for review cards. Check that neither key is already bound in those contexts, since `y` may be taken. If a key is taken, pick a free one and record the choice as a comment on the card.
  - Tests:
    - the projection fills the reference for PR cards only;
    - the detail row renders;
    - `b` dispatches an open for the URL;
    - `y` copies it;
    - a gallery snapshot or test if the kit has one.
  - Docs: BOARD.md §11.9 (card detail rows), and the tile and detail sections of UX-SPEC's Board chapter.
- **Verification:** `cargo test -p fleet-ui-kit`, `cargo test -p fleet-app board`, `make lint`.
- **Done when:** a review card shows its PR on the tile and in the detail, and both keys work.

### P4-T04 — Unlock column automation in Board settings for card-worktree boards
- **Intent:** Board settings lets the user edit a Reviews board's columns and actions, and says where its runs execute.
- **Touches:** `crates/fleet-app/src/dialogs/board_settings/{persistence.rs,schema.rs,view.rs}`, tests, `docs/BOARD.md` §11.10, `docs/UX-SPEC.md` (Board settings section).
- **Steps:**
  - In `persistence.rs` around line 28, change `automation_locked` to:

    ```
    (worktree_id.is_none() && run_location.is_board_worktree()) || backend.kind != LOCAL
    ```

    Update the comment: a board runs in its own worktree or in each card's.
  - In General, add a read-only fact row, `Runs in   each card's worktree` or `this worktree`, from `settings.run_location`. It is not editable in v1: changing it on a board with live runs would strand them.
  - Tests:
    - the Columns pane is editable on a Reviews board;
    - it is locked on a plain context board;
    - the General row shows the right text.
  - Docs: BOARD.md §11.10 and UX-SPEC's Board settings description.
- **Verification:** `cargo test -p fleet-app board_settings`, `make lint`.
- **Done when:** a Reviews board's columns can be edited and saved from the app.

### P4-T05 — Add the Schedules section to Board settings
- **Intent:** Board settings has a fourth section, Schedules, that lists a board's schedules and creates, edits, enables or disables, deletes and runs them now, gated on the `schedules` capability.
- **Touches:** `crates/fleet-app/src/dialogs/board_settings/{schema.rs,draft.rs,keys.rs,persistence.rs,view.rs}` and a new `board_settings/schedules.rs` if the section is large (the skill's split threshold is about 900 lines per file), `state/` for a schedules mirror (see below), `state/connection.rs` for the `SchedulesChanged` event, tests, `docs/UX-SPEC.md` (Board settings), `docs/BOARD.md` §12 (app part), `docs/KEYMAP.md` (dialog keys).
- **Steps:**
  - Load `gpui-app-shell` (dialog focus and keys) and read the Columns section's code first. It is the model: a list level and a form level, `Enter` drills in, and `^s` saves (UX-SPEC §3.8.6 explains why `^s` is the save here).
  - State:
    - add a small schedules mirror, `Vec<Schedule>` keyed by board, loaded with `ListSchedules { board_id }` when the dialog opens on a board, and refreshed on `SchedulesChanged { board_id }` for that board;
    - store it next to the board state so the board header (P4-T06) can read it too;
    - no IO in render.
  - Section `Schedules` in `BoardSection`, shown only when the daemon advertises `schedules`.
  - The list level shows one row per schedule: `● name   every 15m   next 14:05   last ✓ 3 created, 5 existing`. The dot is `●` when enabled and `○` when disabled; the last result has an outcome glyph and tone (✓ success, ✗ failed or timed out, ⤼ skipped) plus the summary. Keys:
    - `n` new;
    - `Enter` edit;
    - `space` toggle enabled (sends `UpdateSchedule`);
    - `r` run now (`RunScheduleNow`);
    - `d` delete, with the existing confirm pattern the app uses for destructive actions (grep `confirm` in `dialogs/`).
  - The form level has these rows:
    - `Name` (text);
    - `Provider ◂ claude ▸`;
    - `Model` (free text against the suggested list, exactly as the column form does);
    - `Effort`;
    - `Mode` (cycles the provider's supported modes, default full access);
    - `Cadence ◂ every ▸`, and `Every [15] minutes` or `Once at [2026-09-23 09:00]`;
    - `Timeout [20] minutes`;
    - `Prompt` (multi-line editor, the same widget the column `Instructions` row uses);
    - `Enabled [x]`;
    - read-only `Last runs` (the last five, with outcome, summary and log path).

    `^s` saves with `CreateSchedule` or `UpdateSchedule`. Validation errors from the daemon appear as the red footer line the dialog already uses.
  - Starter: `n` on a Reviews board pre-fills name `GitHub reviews`, every 15, and the prompt `STARTER_PROMPT_GITHUB_REVIEWS`. On other boards the prompt is empty. The P4-T02 empty-state `Enter` opens here in that state.
  - Tests:
    - the section is hidden without the capability;
    - the list renders a schedule's row facts;
    - `space` sends the toggle;
    - `r` sends run now;
    - `n` on a Reviews board pre-fills the starter;
    - `^s` sends a create with every field;
    - a daemon refusal shows in the footer and keeps the dialog open;
    - `SchedulesChanged` refreshes the list.
  - Docs: UX-SPEC (Board settings: the section table gains a Schedules row with its rows and keys), BOARD.md §12 (app part), KEYMAP (dialog keys).
- **Verification:** `cargo test -p fleet-app board_settings`, `make lint`, `make restart` + `make run`, then create a schedule from the app and see it in `fleet schedule --reviews list`.
- **Done when:** a schedule can be created, edited, toggled, run and deleted from the app.

### P4-T06 — Show schedules in the board header
- **Intent:** a board with schedules shows one compact strip in its header, e.g. `⟳ GitHub reviews · next 14:05 · last 3 created`, amber when the last run failed, and `F` runs every enabled schedule of the board now.
- **Touches:** `crates/fleet-app/src/views/board_screen.rs` (header around 198-268), `views/board_screen/model.rs`, the schedules mirror from P4-T05, `keymap.rs`, tests, `docs/UX-SPEC.md` (Board header), `docs/BOARD.md` §11.8, `docs/KEYMAP.md`.
- **Steps:**
  - Precompute the header strings in the board projection whenever the schedules mirror or the clock tick changes, following how `synced <age>` is kept current. Never format times in render.
  - One schedule shows its name, next time and last summary. Several show `⟳ 3 schedules · next 14:05`. A failed or timed-out last run tints the strip with the warning tone from the theme, as the header's conflict counter does.
  - Clicking the strip or pressing `S` opens Board settings on Schedules. `F` runs every enabled schedule now, sending one `RunScheduleNow` each. Check that `S` and `F` are free in `Hub > Board`, `Hub > Prs > Board` and `Workspace > Native > Board`, and choose other keys if not, recording the choice as a card comment.
  - Tests:
    - the strip strings for none, one and several schedules;
    - the failed tone;
    - `F` sends one request per enabled schedule;
    - no strip without the capability.
  - Docs: UX-SPEC Board header description, BOARD.md §11.8, KEYMAP.
- **Verification:** `cargo test -p fleet-app board`, `make lint`.
- **Done when:** the strip shows and updates when a run finishes (`SchedulesChanged`).

### P4-T07 — Pin the Review board and the Schedules section in the harness
- **Intent:** `make harness` covers the Review tab board, a PR card's tile, run keys on the Reviews board, and the Schedules section, each in its own scenario, with a screenshot baseline where pixels matter.
- **Touches:** `scenarios/prs/review-board.scenario` (new), `scenarios/prs/review-card-pr.scenario` (new), `scenarios/board/schedules-section.scenario` (new), `crates/fleet-harness/src/fixture/*` (only through the TESTING-HARNESS amendment rule), `crates/fleet-app/src/drive*` if a snapshot field is needed, `docs/TESTING-HARNESS.md`.
- **Steps:**
  - Read `docs/TESTING-HARNESS.md` in full, especially §9 and the amendment rule; it is frozen. Then read `scenarios/board/workflow-columns-context.scenario` and `workflow-marks.scenario` as models.
  - Decide what each scenario needs:
    - the board's existing targets (`board.column`, `board.filter`, the card targets) should be enough for the board itself;
    - a Reviews board in the fixture needs a fixture field for pre-seeded boards with PR cards, unless the fixture can already seed a board. Check `fixture/plan.rs`. If a new fixture field or snapshot field is needed, add it through the amendment rule and document it in TESTING-HARNESS in the same commit.
  - Scenarios. One behaviour each, fixture on the first line, `await` rather than `wait`, assertions on the snapshot, at most one `shot`, ending with `quit`:
    - `prs/review-board`: open the PR screen, Tab to Review, await the board scope, and assert the five column names. Shot.
    - `prs/review-card-pr`: a seeded PR card's tile shows `owner/name#1`, and its detail shows the Pull request row.
    - `board/schedules-section`: open Board settings on the Reviews board, go to Schedules, press `n`, and assert the starter name and prompt in the form. Shot.
  - Run each scenario in both lanes as the doc says (`make harness`, and `make harness-headless` for any scenario without a shot). Refresh baselines with `make harness HARNESS_ARGS=--update-baselines` only for these new scenarios, and look at every new PNG before committing it.
- **Verification:** `make harness`, `make harness-headless`, `make lint`, `make test`.
- **Done when:** the three scenarios pass, their baselines are committed, and every other scenario still passes.

## Verification

- `make lint`
- `make test`
- `make harness`
- `make harness-headless`
- `make restart` + `make run` for a manual pass of the whole flow: create a schedule from the app, press `F`, watch cards arrive in Pending review, move to Reviewing, land in Reviewed, read a report, move one to Review published, and see the review on GitHub.

## Definition of done

- [ ] Every phase-4 card is in Done on the FEA board.
- [ ] `make lint` clean, `make test` passing, `make harness` passing.
- [ ] UX-SPEC, KEYMAP, BOARD.md and TESTING-HARNESS describe what the app does.
- [ ] The manual end-to-end pass above was done once against real GitHub, and its outcome is recorded as a comment on P4-T07's card.
- [ ] Follow-ups captured as Backlog cards.

## Risks and rollback

- **Key collisions on the nested `Hub > Prs > Board` context:** each binding is listed in KEYMAP and tested; the card records any key it had to change.
- **An old daemon:** every new surface is capability-gated and falls back to today's flat list.
- **Rollback:** revert P4-T02 to restore the flat Review list; the other cards are inert without it.
