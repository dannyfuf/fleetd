# PR review boards, phase 1: the review model in fleet-core — Plan
> Tracker: ./pr-review-boards-2026-09-22-phase-1-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.
> For this initiative the tracker's truth is the worktree board `dannyfuf/fleetd#feat-board-workflow-schedule-custom-task` (prefix FEA): the tracker maps task ids to card keys, the cards carry state.

## Summary

Phase 1 adds everything a review board needs to the pure board model in `crates/fleet-core`: a
board kind, a pull-request reference on a card, a board setting that runs each card in its own
worktree, the reviews column preset, new brief sections, idempotent creation of a PR card, and a
queue rule for routing columns. No daemon, wire, CLI or app code changes in this phase, so nothing
a user sees changes. Shared names and sentences are fixed in `./pr-review-boards-2026-09-22-contracts.md`
(C1 to C5); copy them from there verbatim.

## Sizing call

Phased: part of a four-phase roadmap (`./pr-review-boards-2026-09-22-roadmap.md`). This phase is
one focused stretch over one crate and ships as one pull request.

## Repository context

- Rust Cargo workspace; this phase touches `crates/fleet-core` only (plus docs).
- Board model: `crates/fleet-core/src/board/model.rs` (Board around line 13, BoardSettings around
  634, Card around 284, CardRun around 443, `document_version` around 800, `BOARD_DOCUMENT_VERSION`
  around 789).
- Operations: `crates/fleet-core/src/board/ops.rs` (CardDraft around 43, CardPatch around 88),
  `ops/cards.rs` (`create_card`), `ops/query.rs` (`attention` around 145), `ops/validation.rs`.
- Defaults and presets: `crates/fleet-core/src/board/defaults.rs` (`workflow_preset` around 61,
  `render_template` around 153, `new_board`, `new_worktree_board` around 214).
- Automation engine: `crates/fleet-core/src/board/automation.rs` (`re_evaluate` 117,
  `Walk::walk` 191, `start_on_entry` 213, `park`, `advance_dependants` 279, `next_pending` 399,
  `brief` 459); its tests are `board/automation/tests.rs`.
- Board tests: `board/tests.rs` (round trips and version stamping around 381-665), `ops/tests/`.
- Wire goldens that serialise `CardDraft`: `crates/fleet-proto/tests/compatibility.rs` — every
  new draft field must be `skip_serializing_if` so those goldens stay byte-identical.
- Docs: `docs/BOARD.md` §2 (domain model) and §11 (automation); ADRs in `docs/decisions/`, next free
  number **0023**, indexed in `docs/README.md`.
- Verification: `make lint`, `make test`; focused runs with `cargo test -p fleet-core board::`.
- Skills to load first: `rust-workspace-architecture`, `rust-gpui-testing`; `zed-quality-review` before calling a card done.

## Assumptions

- A pull request is identified by `owner/name#number`; GitHub is the only forge in scope.
- A card's pull request is set once, at creation, and never edited.
- Changing the Ready column's behaviour (an unblocked card starts at once) is acceptable and wanted.

## Out of scope

- Any daemon, protocol, CLI or app change (phases 2 to 4).
- Scheduling (phase 3).
- Forges other than GitHub.

## Affected areas

- `crates/fleet-core/src/board/{model.rs,ops.rs,ops/cards.rs,ops/query.rs,ops/validation.rs,defaults.rs,automation.rs,mod exports}`
- `crates/fleet-core/src/board/{tests.rs,automation/tests.rs,ops/tests/*}`
- `docs/BOARD.md`, `docs/decisions/0023-review-boards.md`, `docs/README.md`

## Tasks

### P1-T01 — Add the review fields to the board model and stamp version 3
- **Intent:** add `BoardKind`, `RunLocation`, `PullRequestRef`, `Card.pull_request`, `CardDraft.pull_request` and `CardRun.worktree_id`, and make a board that uses any of them persist as document version 3.
- **Touches:** `crates/fleet-core/src/board/model.rs`, `crates/fleet-core/src/board/ops.rs`, `crates/fleet-core/src/board/ops/cards.rs`, `crates/fleet-core/src/board/tests.rs`, `docs/BOARD.md` §2, `docs/decisions/0023-review-boards.md` (new), `docs/README.md`.
- **Steps:**
  - In `model.rs`, add `BoardKind { Tasks, Reviews }` and `RunLocation { BoardWorktree, CardWorktree }` exactly as contracts C1 says: snake_case serde, `Default` derive with `#[default]` on the first variant, and the `is_tasks` / `is_board_worktree` helpers used by `skip_serializing_if`. Put a one-line `///` doc on each type and variant, matching the density of the neighbouring types.
  - Add `pub kind: BoardKind` to `Board` and `pub run_location: RunLocation` to `BoardSettings`, both `#[serde(default, skip_serializing_if = …)]`. `BoardSettings` implements `Default` by hand; add the new field there as well, or the serde default and `Default` drift apart. The comment at around line 307 of BOARD.md says so.
  - Add `PullRequestRef { repo: RepoId, number: u64, url: String }` (camelCase). Only the struct and `key()` go in this card; `parse` is P1-T02.
  - Add `pub pull_request: Option<PullRequestRef>` to `Card` next to `worktree_id`, and `pub worktree_id: Option<WorktreeId>` to `CardRun`. Both `#[serde(default, skip_serializing_if = "Option::is_none")]`.
  - In `ops.rs` add `pub pull_request: Option<PullRequestRef>` to `CardDraft`, `#[serde(default, skip_serializing_if = "Option::is_none")]`. The `skip` matters: `fleet-proto`'s `create_card` golden serialises a draft, and it must not grow a `"pullRequest":null`.
  - In `ops/cards.rs::create_card`, copy `draft.pull_request` onto the new card. Today it builds the card field by field, so the compiler will point at the spot.
  - Every place that constructs a `Card`, `CardRun` or `Board` with a struct literal stops compiling. Fix each one with the default value (`None`, `BoardKind::Tasks`, `RunLocation::BoardWorktree`). Run `cargo build --workspace --all-targets` to find them all, including tests in other crates.
  - Set `BOARD_DOCUMENT_VERSION` to 3 and extend `document_version` so it returns 3 when the condition in contracts C1 holds, and otherwise keeps today's 2/1 logic. Update its doc comment: a board that never uses a review field keeps writing 2 or 1.
  - Find the store's accepted range check (`crates/fleet-daemon/src/stores/board.rs`, `read`, around line 92). It compares against `BOARD_DOCUMENT_VERSION`, so version 3 should be accepted automatically. Confirm that, and change nothing else in the daemon.
  - Tests in `board/tests.rs`, next to `a_board_that_never_opted_into_automation_is_written_at_the_version_before_it`:
    - `a_board_that_uses_no_review_field_keeps_its_version`: a v1 board and a v2 board each round-trip and stamp the same version as before.
    - `a_reviews_board_is_written_at_version_3`: `kind = Reviews` stamps 3.
    - `a_card_worktree_board_is_written_at_version_3`.
    - `a_card_with_a_pull_request_is_written_at_version_3`.
    - `a_board_without_review_fields_serialises_byte_for_byte_as_before`: serialise a default board and card to JSON and assert that neither `kind`, `runLocation`, `pullRequest` nor `worktreeId` inside runs appears.
  - Write `docs/decisions/0023-review-boards.md` in the house ADR shape. Look at `0022-board-workflows.md` for tone and headings: title line, **Adopted** paragraph, Context, Decision, Alternatives rejected, Consequences. Record these decisions:
    - the Reviews board is a second board per context, shown in the PR screen's Review tab (reason: the roadmap's "Where the board lives" table);
    - runs execute in each card's own worktree when `run_location` is `CardWorktree`, which is what lifts the "context boards cannot automate" rule for that setting only;
    - a card's pull request is immutable and unique per board;
    - routing columns queue: an unblocked card entering one advances at once or waits in place;
    - version 3 is stamped lazily.

    Add the ADR's row to `docs/README.md` (decisions index, around lines 30-53).
  - In `docs/BOARD.md` §2, add the new fields to the Rust model fences where `Board`, `BoardSettings`, `Card`, `CardRun` and `CardDraft` are listed, each with a one-line comment, and add a short paragraph after the `document_version` rule describing version 3.
- **Verification:** `cargo test -p fleet-core board::`, `cargo test -p fleet-proto`, `make lint`, `make test`.
- **Done when:** the new fields exist, every existing test and golden passes unchanged, and the four version tests pass.

### P1-T02 — Parse and normalise pull request references
- **Intent:** `PullRequestRef::parse` turns a GitHub URL or `owner/name#n` into a normalised reference, and refuses anything else with the contracts C1 sentence.
- **Touches:** `crates/fleet-core/src/board/model.rs` (or a new `board/pull_request.rs` if `model.rs` is past ~900 lines; check with `wc -l`), `crates/fleet-core/src/board/tests.rs` or a sibling test module.
- **Steps:**
  - Implement `pub fn parse(text: &str) -> Result<PullRequestRef, BoardError>`:
    - trim whitespace;
    - accept `https://github.com/<owner>/<name>/pull/<n>` with an optional `http://` or `www.`, and ignore anything after the number (`/files`, `/commits/…`, `#discussion…`, `?…`);
    - accept `<owner>/<name>#<n>`;
    - build `repo` with `RepoId::try_from(format!("{owner}/{name}"))`, so repository-id validation is reused;
    - `number` must parse as a `u64` greater than 0;
    - always rebuild `url` as `https://github.com/<owner>/<name>/pull/<n>`.
  - Do not add a regex dependency. `split` / `strip_prefix` are enough, and the workspace avoids new dependencies without a reason.
  - Refuse with `BoardError::Invalid { field: "pull_request".into(), reason: "expected a GitHub pull request URL or owner/name#number".into() }`. Check how other `Invalid` errors are built in `ops/validation.rs` and copy that shape.
  - `key()` returns `"<owner>/<name>#<n>"`.
  - Add a table-driven unit test, `pull_request_references_parse_and_normalise`, covering: a plain URL; a URL with `/files`; a URL with `#issuecomment-1`; a URL with a query; the `owner/name#12` form; surrounding whitespace. Assert the normalised `url` and `key()` for each.
  - Add `pull_request_references_refuse_what_is_not_one`, covering: an empty string; `owner/name` with no number; `#12` alone; `https://github.com/o/n/issues/12`; `https://gitlab.com/o/n/pull/1`; number `0`; a non-numeric number. Assert the exact sentence.
- **Verification:** `cargo test -p fleet-core pull_request`, `make lint`.
- **Done when:** both tests pass and the refusal sentence matches contracts C1 character for character.

### P1-T03 — Ship the reviews preset and the Reviews board constructor
- **Intent:** `reviews_preset()`, `reviews_board_id()`, `new_reviews_board()` and the preset instruction constants exist exactly as contracts C2 lists them.
- **Touches:** `crates/fleet-core/src/board/defaults.rs`, the `board` module's `pub use` list, `crates/fleet-core/src/board/tests.rs`, `docs/BOARD.md` §11.6.
- **Steps:**
  - Read `workflow_preset()` (around line 61) and copy its style: one `Status` per column, `color` from the same palette, `automation: Some(ColumnAutomation { … })` only where a column acts.
  - Add the four `PRESET_*_REVIEW*` / `PRESET_*_PUBLISH*` constants with the exact text from contracts C2, next to `PRESET_INSTRUCTIONS_IMPLEMENT`.
  - Write `reviews_preset()` with the five columns of the C2 table:
    - `pending` routes with `advance_when_unblocked: Some("reviewing")`;
    - `reviewing` has `on_enter: Some(Action { kind: ActionKind::Prompt, instructions: PRESET_INSTRUCTIONS_REVIEW_PR, expect: PRESET_EXPECT_REVIEW_PR, .. })` and `on_success: Some("reviewed")`;
    - `published` has the publish prompt and no `on_success`;
    - no column sets an agent provider, so the daemon default (Claude) applies.
  - Write `reviews_board_id(&ContextId) -> BoardId`. Mirror `worktree_board_id` (defaults.rs around line 197): format `reviews-{context}`, truncate to `BOARD_ID_MAX_LEN`, trim trailing `-`, then `BoardId::try_from(..).expect("…")`, with a message saying why the value is always valid.
  - Write `new_reviews_board(context, now)` as C2 says: start from `new_board`, then override id, name, prefix, kind, statuses, `run_location`, `max_live_runs = Some(2)` and `start_on_worktree = false`. Also set `labels` to the two C7 labels `github` and `chat`. Copy the `Label` construction from any test that builds labels, and use a distinct theme colour name for each.
  - Export the new functions and constants from the `board` module the same way `workflow_preset` is exported.
  - Tests:
    - `the_reviews_preset_routes_forward_and_validates`: build `new_reviews_board`, run `validate_board`, and assert the column ids in order and each column's `on_enter` / `on_success` / `advance_when_unblocked`.
    - `a_reviews_board_id_is_derived_from_its_context`: include a 70-character context id to exercise truncation.
    - `a_new_reviews_board_runs_in_card_worktrees_two_at_a_time`.
  - In `docs/BOARD.md` §11.6, add a "The reviews preset" subsection with the C2 table and the constants' names. Leave the long texts to the code, as the existing section does for the implement preset.
- **Verification:** `cargo test -p fleet-core reviews`, `make lint`.
- **Done when:** a Reviews board built by the constructor validates and the three tests pass.

### P1-T04 — Put the pull request and the user's notes into the run brief
- **Intent:** a review run's brief names its pull request, substitutes `{pr_url}` / `{pr_repo}` / `{pr_number}`, and includes the user's own comments made since the last run.
- **Touches:** `crates/fleet-core/src/board/defaults.rs` (`render_card_template`), `crates/fleet-core/src/board/automation.rs` (`brief`), wherever column `env` values are rendered with `render_template` (grep `render_template(` across `crates/`), `crates/fleet-core/src/board/automation/tests.rs`, `docs/BOARD.md` §11.1.
- **Steps:**
  - Add `render_card_template(text, key, card)` next to `render_template`. Call `render_template(text, key, &card.title)` first, then, only when `card.pull_request` is `Some`, replace `{pr_url}`, `{pr_repo}` (the `RepoId` as a string) and `{pr_number}`. Leave the placeholders untouched when there is no pull request. Document that in the fn's doc comment, because a Tasks board may legitimately print `{pr_url}` in its instructions.
  - In `brief()`:
    - render the instructions with `render_card_template`;
    - after the description, add `## Pull request` with one line `{repo}#{number} · {url}` when the card has one;
    - then add `## Notes from you`: filter `card.comments` to those with `run_id.is_none()` and `created_at` later than `latest_run(card).and_then(|r| r.ended_at.as_deref())` (all of them when there is no ended run), oldest first, each as `- {author}: {body}`.
    - Check the `Comment` struct for the author field's name and type. Print `you` when it is empty or `None`.
    - Keep the exact blank-line rhythm the existing sections use: one blank line before each `##` heading.
  - Switch the `env` value rendering (daemon side, found by the grep) to `render_card_template`. That rendering lives in `fleet-daemon`, but the change is one call. Keep it in this card so the placeholder set is the same everywhere.
  - Tests, next to the existing `brief` tests:
    - `a_brief_names_the_pull_request`;
    - `pull_request_placeholders_are_left_alone_without_one`;
    - `a_brief_carries_notes_made_after_the_last_run`: with two human comments, one before and one after the last run's `ended_at`, only the later one appears;
    - `a_brief_carries_every_note_when_the_card_never_ran`;
    - `run_reports_are_not_notes`: a comment with `run_id` never appears under `## Notes from you`.
  - Update the §11.1 sentence listing substitutions ("`render_template` substitutes `{key}` and `{title}` …") to name `render_card_template` and the three new placeholders. Describe the two new brief sections in §11.2 or wherever the brief's shape is described (grep `Previous run reports` in BOARD.md).
- **Verification:** `cargo test -p fleet-core brief`, `cargo test -p fleet-daemon` (env rendering), `make lint`.
- **Done when:** the five tests pass, and existing brief tests pass unchanged for cards without a pull request or notes.

### P1-T05 — Make routing columns a queue
- **Intent:** a card that enters a routing column with nothing blocking it advances at once, or waits in place for a run slot and is moved when one frees (contracts C4).
- **Touches:** `crates/fleet-core/src/board/automation.rs`, `crates/fleet-core/src/board/ops/query.rs` (`attention`, new `queued`), `crates/fleet-core/src/board/automation/tests.rs`, `docs/BOARD.md` §11.5 and §11.7, `.claude/skills/fleet-board-planning/SKILL.md`.
- **Steps:**
  - Read §11.7 "The engine" in `docs/BOARD.md` and the whole `Walk` impl first. Rule 1 is `start_on_entry`, rule 2 is `advance_dependants`, and a settled seed skips rule 1.
  - Add a private helper `fn try_advance(&mut self, board, cards, index, target: &StatusId, message: String, now) -> Result<(), BoardError>`:
    - if `on_enter(board, target)` is `Some` and `self.live.len() + self.in_flight.len() >= board.settings.max_live_runs() as usize`, call the existing `park(cards, index, target.clone(), now)`. `park` already keeps an older `since` for the same status. Return.
    - otherwise call `move_card`, then `record_auto_move(card, message, now)`, push to `self.plan.moved` and `self.queue`. This is exactly the tail of `advance_dependants` today, so move that code into the helper.
  - Change `advance_dependants` to call `try_advance` with its existing `unblocked by …` message instead of moving directly.
  - Add rule 0, `fn advance_on_entry(&mut self, board, cards, index, now) -> Result<(), BoardError>`, and call it in `walk` right after `start_on_entry` for non-settled cards (the same `if` block). It does nothing when:
    - the card is archived;
    - the card's column has no `advance_when_unblocked`;
    - the card is live or in flight;
    - any blocker is unsatisfied (reuse `is_satisfied`).

    Otherwise it calls `try_advance` with the message `Moved to {target name}: a run slot freed` when the card was queued (`pending_run` is `Some` with a `status_id` different from the card's column), and `Moved to {target name}: nothing blocks it` otherwise.
  - A queued card that `try_advance` moves must lose its queue marker: after the move, `card.status_id == pending_run.status_id`, which is today's plain "parked or starting" state. `start_on_entry` then either starts it (and the daemon clears the marker when it records the run, as now) or parks it again with the same `since`. Check that `park` keeps the old `since` in that case. It compares `status_id`, so it does.
  - Release: `Boards::release_slot` in the daemon seeds `next_pending`'s card through `re_evaluate` (non-settled), so rule 0 moves a queued card with no daemon change. Confirm by reading `crates/fleet-daemon/src/services/boards/automation/resume.rs` `release_slot`. Change nothing there in this card.
  - In `ops/query.rs`, add `pub fn queued(card: &Card) -> bool` (contracts C4). Make `attention()` skip the `pending_run` age check when `queued(card)` is true. Export `queued` next to `attention`.
  - Update every existing test that asserted an unblocked card in a routing column stays put. Search the test files for `advance_when_unblocked`, `ready` and `waits`. Each such assertion now expects the card to move, or to be queued when the board is full. Do not delete a test: change its expectation, and rename it if the name says "waits forever".
  - New tests in `automation/tests.rs`:
    - `an_unblocked_card_entering_a_routing_column_advances_and_starts`;
    - `a_card_created_in_a_routing_column_advances` (seed a freshly pushed card);
    - `a_blocked_card_entering_a_routing_column_stays`;
    - `a_full_board_queues_the_card_in_the_routing_column`: assert the card is still in the routing column, `pending_run.status_id` is the target, and `queued()` is true;
    - `a_queued_card_keeps_its_place_in_line` (re-evaluating does not restamp `since`);
    - `a_freed_slot_moves_the_queued_card_and_starts_it` (simulate by emptying `live`, then seeding the `next_pending` card through `re_evaluate`);
    - `a_queued_card_does_not_raise_attention` (in the `ops/tests` attention tests);
    - `a_dependant_released_onto_a_full_board_waits_in_its_routing_column`.
  - Docs:
    - In BOARD.md §11.7, add "Rule 0, advance on entry" before rule 1, and update the paragraph that says "A card that nothing blocks, standing in a routing column, is reached by no cascade and released by nothing".
    - Change the throttle paragraph to say a card bound for a full column waits in its routing column.
    - In §11.5, add `queued` to the derived-reads table.
    - In §11.3, add the two new `AutoMoved` sentences.
    - In `.claude/skills/fleet-board-planning/SKILL.md`, replace rule 2 under "Building a chain" with the sentence in contracts C4, and fix the example comment "Move the dependants into Ready, where `when-unblocked` will collect them". Their blockers are still in Todo, so they stay put. Keep that explanation.
- **Verification:** `cargo test -p fleet-core automation`, `cargo test -p fleet-core attention`, `cargo test -p fleet-daemon boards` (daemon automation tests may pin the old Ready behaviour; fix their expectations the same way), `make lint`, `make test`.
- **Done when:** the eight new tests pass, no existing test was deleted, and BOARD.md §11 describes rule 0.

### P1-T06 — Create pull request cards idempotently and reopen them on a new request
- **Intent:** a pure `upsert_pull_request_card` operation creates a card for a pull request the board does not hold yet, returns the existing card otherwise, and reopens a completed card when the review is requested again.
- **Touches:** `crates/fleet-core/src/board/ops/cards.rs`, `crates/fleet-core/src/board/ops.rs` (`UpsertOutcome`), `crates/fleet-core/src/board/ops/validation.rs`, `crates/fleet-core/src/board/ops/tests/`, `docs/BOARD.md` §2 (operations list).
- **Steps:**
  - In `ops.rs`, add `pub enum UpsertOutcome { Created, Existing, Reopened }`. Derive what `Priority` derives and use `#[serde(rename_all = "snake_case")]`; phase 2 puts it on the wire.
  - In `ops/cards.rs`, add `pub fn upsert_pull_request_card(board: &mut Board, cards: &mut Vec<Card>, id: CardId, draft: CardDraft, requested_at: Option<&str>, now: &str) -> Result<(UpsertOutcome, usize), BoardError>`. The `usize` is the index of the affected card in `cards`.
    - Refuse a draft without `pull_request`: `BoardError::Invalid { field: "pull_request", reason: "a pull request card needs a pull request" }`.
    - Look for a card with the same `pull_request.key()`, archived cards included.
    - None found: call the existing `create_card`, push the card, return `Created`.
    - Found, in a column whose category is `Completed` or the card is archived, `requested_at` is `Some`, and `requested_at` is later than the card's last completion: reopen, return `Reopened`. The last completion is the `at` of the newest activity entry of kind `Moved` or `AutoMoved`; use `card.updated_at` when there is none. Compare as `chrono::DateTime` parsed from RFC 3339, not as strings, and treat an unparsable `requested_at` as an `Invalid` on field `requested_at` with reason `must be an RFC 3339 time`.
    - Reopening means: unarchive; `move_card` to `draft.status_id` or else the first `Unstarted` column; push an `Updated` activity entry `Review re-requested`. Reuse the helper other ops use to push activity (grep `push_activity`).
    - Found in any other case, including every `Canceled` card, since dismissal is a decision: return `Existing` and change nothing.
  - In `validation.rs::validate_document` (or the board-level validation that already refuses duplicate card numbers), refuse two cards with the same pull-request key: field `pull_request`, reason `{key} is already on this board as {KEY}`. The daemon's store runs this validation on every save, so a race can never persist a duplicate.
  - Tests, one per branch:
    - `upserting_a_new_pull_request_creates_a_card`;
    - `upserting_a_known_pull_request_returns_it_unchanged`;
    - `a_completed_card_reopens_when_the_request_is_newer`;
    - `a_completed_card_stays_when_the_request_is_older`;
    - `a_completed_card_stays_without_a_request_time`;
    - `a_dismissed_card_is_never_reopened`;
    - `an_archived_card_reopens_unarchived`;
    - `a_board_refuses_two_cards_for_one_pull_request`;
    - `an_unparsable_request_time_is_refused`.
  - Document the operation and the reopen rule in BOARD.md §2, where the other `ops::` functions are listed, in the same fence style.
- **Verification:** `cargo test -p fleet-core upsert`, `cargo test -p fleet-core board::`, `make lint`, `make test`.
- **Done when:** the nine tests pass and the duplicate refusal is enforced by validation, not only by the upsert.

## Verification

- `make lint`
- `make test`
- `cargo test -p fleet-core` for the focused loop.

## Definition of done

- [ ] Every phase-1 card is in Done on the FEA board and the tracker maps each id to its card.
- [ ] `make lint` clean.
- [ ] `make test` passing.
- [ ] No golden in `crates/fleet-proto/tests/` changed.
- [ ] `docs/BOARD.md` §2 and §11 and ADR 0023 describe what the code does.
- [ ] Follow-ups captured as Backlog cards.

## Risks and rollback

- **The Ready behaviour change surprises a user of the workflow preset.** The ADR and the skill say so. Rollback is reverting P1-T05 alone, since it touches no persisted shape.
- **Version 3 locks out an older daemon.** It happens only for boards that use a review field, and none can exist before phase 2 ships. Rollback before phase 2 costs nothing.
- **Struct-literal churn across crates.** Mechanical; the compiler finds every site.
