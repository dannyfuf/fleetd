# PR review boards, phase 2: Reviews boards in the daemon, the wire and the CLI — Plan
> Tracker: ./pr-review-boards-2026-09-22-phase-2-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.
> For this initiative the tracker's truth is the worktree board `dannyfuf/fleetd#feat-board-workflow-schedule-custom-task` (prefix FEA).

## Summary

Phase 2 makes the phase-1 model live. The daemon serves one Reviews board per context. It creates
pull-request cards idempotently. On a board whose `run_location` is `CardWorktree`, it runs each
card in that card's own worktree, creating the worktree from the card's pull request on first run.
Two additive requests carry this on the wire behind the `board.reviews` capability, and the CLI
drives it through `fleet board --reviews` and `card new --pr`. At the end of the phase a user can go
Pending review → Reviewing → Reviewed from a terminal. Names and sentences come from
`./pr-review-boards-2026-09-22-contracts.md` (C1, C5, C6, C8).

## Sizing call

Phased: phase 2 of 4 (`./pr-review-boards-2026-09-22-roadmap.md`). It spans the daemon, proto,
client and CLI crates, but it is one feature seam and ships as one pull request.

## Repository context

- **Daemon board service:** `crates/fleet-daemon/src/services/boards.rs` plus `services/boards/`:
  - `lifecycle.rs`: `ensure` around line 77, `ensure_for_worktree` 116, `require_automatable` around 636, `delete_for_context` around 674.
  - `documents.rs`: `context_board` around line 35.
  - `cards.rs`: `create_card`.
  - `worktree.rs`: `create_worktree_from_card` and `create_and_link_worktree`.
  - `automation.rs`: `start_for_card` around 319, `prepare_run` around 376, `record_run` around 411, `changed_files` around 692, `card_request` around 833, `started` around 880, `files_section` around 988.
  - `automation/resume.rs`: `automated_boards` around 145, `release_slot`.
- **Worktrees:** `crates/fleet-daemon/src/services/worktrees/creation.rs`, `create_from_pr` around 78. It is idempotent: it adopts an existing worktree for the same PR slug. It returns `(created, Worktree, Option<JobRecord>)`, where the job is the post-create hooks.
- **Dispatch:** `crates/fleet-daemon/src/services/dispatch.rs`. Board requests are around line 292; `delete_context_cascade` is around 908.
- **Wire:**
  - `crates/fleet-proto/src/request.rs`: board family around lines 428-574.
  - `crates/fleet-proto/src/response.rs`: board payloads around 339-357; `BOARD_AUTOMATION_CAPABILITY` around 67.
  - Goldens: `crates/fleet-proto/tests/compatibility.rs`, hand-written string literals; helper `tests/support/mod.rs` `assert_frame`.
  - Hello capability list: `crates/fleet-daemon/src/server/connection.rs` around 949.
- **Client:** `crates/fleet-client/src/connection.rs`, `required_capability` around 1173. Board client methods live next to the existing `ensure_board`.
- **CLI:**
  - `crates/fleet-cli/src/args.rs`: `BoardArgs` around 1032, `BoardCardCommand` around 1375, `BoardCardFields` around 1511.
  - `crates/fleet-cli/src/commands/board.rs`: selector resolution around 312-360, `card_new` around 917-944, the `Existing` print for `card worktree` around 1297.
  - `crates/fleet-cli/src/envelope.rs`: `BoardCardEnvelope` around 188.
- **Smoke:** `scripts/board-workflow-smoke.sh`, run by `make smoke-workflow`. It runs a private daemon with scripted agents and is the template for a reviews smoke.
- **Verification:** `make lint`, `make test`, `make smoke-workflow`; run `make restart` after daemon changes before any manual test.
- **Skills to load first:**
  - `rust-ipc-protocol` for the wire cards;
  - `rust-async-background-work` for worktree creation outside the gate;
  - `rust-gpui-testing` for tests;
  - `rust-workspace-architecture`;
  - `zed-quality-review` before done.

## Assumptions

- The repository of a reviewed pull request is already a Fleet repository, in any context. Cloning an unknown repository is a follow-up.
- A Reviews board lives on the local daemon. Context boards are never routed to another host today, and that stays true.

## Out of scope

- Scheduling (phase 3) and every app change (phase 4).
- Automatic cloning of unknown repositories, and cleanup of review worktrees after publishing (follow-ups).

## Affected areas

- `crates/fleet-core/src/board/model.rs` (`BoardSummary.kind`), `board/ops/query.rs` (`summarize`)
- `crates/fleet-daemon/src/services/boards/{lifecycle.rs,documents.rs,cards.rs,worktree.rs,automation.rs,automation/resume.rs}`, `services/dispatch.rs`, `server/connection.rs`
- `crates/fleet-proto/src/{request.rs,response.rs}`, `crates/fleet-proto/tests/compatibility.rs`
- `crates/fleet-client/src/connection.rs` and its board methods
- `crates/fleet-cli/src/{args.rs,commands/board.rs,envelope.rs,human.rs}`
- `scripts/reviews-smoke.sh` (new), `Makefile`, `scripts/board-workflow-smoke.sh`
- `docs/BOARD.md` §4, §4.1, §5, §6, §11.4; `.claude/skills/fleet-board-planning/SKILL.md`

## Tasks

### P2-T01 — Add the review requests and the `board.reviews` capability to the wire
- **Intent:** `EnsureReviewsBoard`, `UpsertPullRequestCard`, `ResponseBody::CardUpsert` and `BOARD_REVIEWS_CAPABILITY` exist on the wire with goldens, and the client refuses them against a daemon that does not advertise the capability.
- **Touches:** `crates/fleet-proto/src/request.rs`, `crates/fleet-proto/src/response.rs`, `crates/fleet-proto/tests/compatibility.rs`, `crates/fleet-client/src/connection.rs`, the client's board methods file (grep `fn ensure_board` under `crates/fleet-client/src`), `docs/BOARD.md` §5 and §6.
- **Steps:**
  - Load the `rust-ipc-protocol` skill and read its section on additive variants and capabilities before starting.
  - In `response.rs`, add `pub const BOARD_REVIEWS_CAPABILITY: &str = "board.reviews";` under `BOARD_AUTOMATION_CAPABILITY`, with a doc comment in the same style.
  - Re-export `UpsertOutcome` from `fleet_core::board` wherever the proto crate re-exports board types, or reference it by path.
  - Add `ResponseBody::CardUpsert { card: Card, outcome: UpsertOutcome }`, placed next to `Card(Card)`.
  - In `request.rs`, add `EnsureReviewsBoard { context_id: ContextId }` and `UpsertPullRequestCard { board_id: BoardId, draft: CardDraft, requested_at: Option<String> }` inside the board family. Put the serde attributes on `requested_at` exactly as contracts C6 shows. Give every field a `///` comment, as the neighbours have.
  - Goldens in `compatibility.rs`: one each for `ensure_reviews_board`, `upsert_pull_request_card` with and without `requestedAt`, and the `card_upsert` response. Copy the `create_card` golden around line 146 and hand-write the JSON.
    - Serde tags are snake_case (`"type":"upsert_pull_request_card"`) and fields are camelCase.
    - The draft carries `"pullRequest":{"repo":"o/n","number":12,"url":"https://github.com/o/n/pull/12"}`.
  - Client:
    - Add `ensure_reviews_board(context)` and `upsert_pull_request_card(board, draft, requested_at) -> (Card, UpsertOutcome)`, copying the shape of `ensure_board` and `create_card`.
    - In `required_capability`, map both requests to `BOARD_REVIEWS_CAPABILITY`, so an old daemon gets the existing "run `fleet daemon restart`" error.
    - The client's own hello does **not** need to advertise the new capability: the client advertises only what it wants the daemon to send it. Read the comment at `hello_client`, around line 915, to confirm.
  - Docs: add both rows to the request table in `docs/BOARD.md` §5 (around lines 958-980) and the two client methods to the §6 fence.
- **Verification:** `cargo test -p fleet-proto`, `cargo test -p fleet-client`, `make lint`.
- **Done when:** the four goldens pass, existing goldens are untouched, and the client maps both requests to the capability.

### P2-T02 — Serve one Reviews board per context
- **Intent:** the daemon answers `EnsureReviewsBoard` by getting or creating the context's Reviews board, keeps the context's task board unaffected, and deletes the Reviews board with its context.
- **Touches:**
  - `crates/fleet-core/src/board/model.rs` (`BoardSummary.kind`), `crates/fleet-core/src/board/ops/query.rs` (or wherever `summarize` lives);
  - `crates/fleet-daemon/src/services/boards/{lifecycle.rs,documents.rs}`, `services/dispatch.rs`, `server/connection.rs`;
  - daemon board tests (grep `ensure(` under `crates/fleet-daemon/src/services/boards` for the test module);
  - `docs/BOARD.md` §4.
- **Steps:**
  - Add `kind: BoardKind` to `BoardSummary` with the same serde attributes as `Board.kind`. Fill it in `summarize`.
  - Fix `documents.rs::context_board`: both of its predicates must also require `doc.board.kind.is_tasks()`. Otherwise the Hub's context board could resolve to the Reviews board once it exists. Add a regression test, `a_reviews_board_is_never_the_context_board`: ensure a Reviews board, then ensure the context board, and assert that they differ and that the context board is a Tasks board.
  - Add `documents.rs::reviews_board(context) -> DaemonResult<Option<BoardId>>`, mirroring `context_board`: try the fast path `reviews_board_id(context)` first, then scan for `context_id == context && worktree_id.is_none() && kind == Reviews`.
  - Add `lifecycle.rs::ensure_reviews(context)`, copying `ensure` line for line. It uses `reviews_board` and `new_reviews_board`, and the same double-check under the gate and `available_board_id` allocation.
  - Dispatch `RequestBody::EnsureReviewsBoard` to it next to `EnsureBoard`, answering `ResponseBody::Board(view)`.
  - Check `delete_for_context`, around line 674. It should delete every board whose `context_id` matches, which would include the Reviews board. Add a test that proves it, and fix the function if it does not.
  - Advertise `BOARD_REVIEWS_CAPABILITY` in the Hello list in `server/connection.rs`, around line 949. Do it in the **last** card of this phase (P2-T07), not here, so the daemon never advertises a half-built feature. Leave a note on this card's completion comment instead.
  - Tests:
    - `ensuring_a_reviews_board_twice_returns_one_board`;
    - `a_reviews_board_uses_the_reviews_preset`;
    - `a_reviews_board_is_never_the_context_board`;
    - `deleting_a_context_deletes_its_reviews_board`;
    - `ensuring_a_reviews_board_for_an_unknown_context_is_not_found`.
  - Docs: in BOARD.md §4, next to the `ensure` / `ensure_for_worktree` description, describe `ensure_reviews`, and add one line that `context_board` never returns a Reviews board.
- **Verification:** `cargo test -p fleet-daemon boards`, `make lint`.
- **Done when:** the five tests pass and the Hub's context board is provably unaffected.

### P2-T03 — Upsert pull request cards in the daemon
- **Intent:** `UpsertPullRequestCard` runs phase 1's `ops::upsert_pull_request_card` under the board gate, saves once, starts whatever the card's column asks for, and answers the card and the outcome.
- **Touches:** `crates/fleet-daemon/src/services/boards/cards.rs`, `services/dispatch.rs`, the daemon board tests, `docs/BOARD.md` §4.
- **Steps:**
  - In `cards.rs`, add `pub async fn upsert_pull_request_card(&self, board: &BoardId, draft: CardDraft, requested_at: Option<String>) -> DaemonResult<(Card, UpsertOutcome)>`. Model it on `create_card` just above it:
    - take the gate and load the document;
    - on a Reviews board, a pull request's repository may belong to any context, so skip `validate_repo` for `draft.repo_id` there. Set `draft.repo_id = Some(pull_request.repo.clone())` only when that repository is a Fleet repository (`self.state_store.load().await?.repos`), and leave it `None` otherwise. The run refuses later with the C5 sentence, so the card is still created and visible;
    - call the core op, then `validate_links`;
    - `commit` with `seeds = [card id]` for `Created` and `Reopened`, and with **no** seeds for `Existing` (nothing changed, so skip the save entirely and return the card as loaded).
  - Seeding the new card is what makes a card created in *Pending review* advance to *Reviewing* through phase 1's rule 0. Do not add any special case for that.
  - Dispatch `RequestBody::UpsertPullRequestCard` to it and answer `ResponseBody::CardUpsert`.
  - Concurrency: two schedule runs can upsert the same PR at the same moment. The gate serialises them, so the second sees the first card and answers `Existing`. Add a test that runs two upserts concurrently with `tokio::join!` and asserts exactly one `Created`.
  - Tests:
    - `upserting_a_pull_request_card_twice_creates_one`;
    - `a_card_upserted_into_pending_review_starts_reviewing`, using the fake delegation service the existing automation tests use (grep `FakeDelegations` or `automation` in the daemon tests to find the harness) and asserting a `StartRun` for the card;
    - `an_existing_upsert_writes_nothing` (the board's `updated_at` is unchanged);
    - `a_reopened_card_starts_again`;
    - `concurrent_upserts_create_one_card`.
  - Docs: BOARD.md §4, one paragraph on the upsert and its gate discipline.
- **Verification:** `cargo test -p fleet-daemon upsert`, `make lint`.
- **Done when:** the five tests pass.

### P2-T04 — Let a card-worktree board automate, and recover its runs after a restart
- **Intent:** `require_automatable` refuses the "worktree boards only" case only for board-worktree boards; boot recovery and the delivery diff include card-worktree boards.
- **Touches:** `crates/fleet-daemon/src/services/boards/lifecycle.rs` (`require_automatable`, `asks_for_automation`), `services/boards/automation/resume.rs` (`automated_boards`), the daemon automation tests, `docs/BOARD.md` §11.4, `docs/decisions/0023-review-boards.md` (consequences).
- **Steps:**
  - In `require_automatable`, around line 636:
    - run rule 1 (no worktree) only when `board.settings.run_location.is_board_worktree()`;
    - keep rule 2 (local backend only) for every board;
    - run rule 3 (host of the board worktree) only when the board has a worktree. For a card-worktree board, the host check moves to the start path (P2-T05), because each card's worktree can differ.
  - In `resume.rs::automated_boards`, around line 145, change the filter from `worktree_id.is_some()` to `worktree_id.is_some() || !doc.board.settings.run_location.is_board_worktree()`. Update the comment above it, which says automation happens in a worktree and nowhere else: it still happens in a worktree, but the card's rather than the board's.
  - Grep the daemon for every other `worktree_id.is_some()` / `worktree_id.is_none()` near automation code. The research list is `automation.rs:692-703`, `automation.rs:636`, `resume.rs:160` and `lifecycle.rs:637`. Leave the delivery diff for P2-T06, but read it now so you know it exists.
  - Tests:
    - `a_context_board_that_runs_in_card_worktrees_accepts_an_action_column`: patch a column's `on_enter` on a context board with `run_location = CardWorktree`, and expect success;
    - `a_context_board_that_runs_in_the_board_worktree_still_refuses`: the original sentence, verbatim;
    - `a_jira_board_still_refuses_automation`;
    - `restart_recovery_includes_card_worktree_boards`, copying an existing resume test.
  - Docs:
    - BOARD.md §11.4: the `automation` row now says the first sentence applies to boards that run in the board's worktree, and adds the two C5 sentences for card-worktree runs;
    - ADR 0023 Consequences: one sentence that a context board automates only in `CardWorktree` mode.
- **Verification:** `cargo test -p fleet-daemon automation`, `make lint`.
- **Done when:** the four tests pass and every existing refusal test still passes.

### P2-T05 — Run each review card in its own pull request worktree
- **Intent:** on a card-worktree board, the first run of a card creates (or adopts) the worktree for the card's pull request, links it to the card, and runs the delegation in it; every later run reuses it; failures are recorded on the card with the C5 sentences.
- **Touches:** `crates/fleet-daemon/src/services/boards/worktree.rs` (new `ensure_pull_request_worktree`), `services/boards/automation.rs` (`start_for_card`, `card_request`, `RunRow`, `started`, `failed`), the daemon automation tests, `docs/BOARD.md` §4.1 and §11.2.
- **Steps:**
  - Load the `rust-async-background-work` skill. Creating a worktree runs `git fetch` and hooks, which can take minutes, so it must happen **outside** the board gate. `create_and_link_worktree` in `worktree.rs` shows the pattern: drop the guard, create, reload, re-check, write.
  - Add `pub(crate) async fn ensure_pull_request_worktree(&self, board: &BoardId, card: &CardId) -> DaemonResult<()>` in `worktree.rs`:
    1. Under the gate, load the card. Return `Ok(())` when the board is a board-worktree board, or when the card already has a `worktree_id` that still exists in state.
    2. With no pull request, return `BoardError::Invalid { field: "automation", reason: "{KEY} has no worktree to run in; link a pull request or create its worktree first" }` (C5), using the card's `display_key`.
    3. Look the repository up in `state.repos` by `pull_request.repo`, in any context. If it is missing, return the second C5 sentence.
    4. Drop the gate. Call `self.worktrees.create_from_pr(repo_id, number)` (it adopts an existing one), and ignore the post-create job handle. Keep the result's `Worktree`.
    5. Take the gate again and reload the card. If it was deleted or archived meanwhile, return a `Conflict` naming the worktree, exactly as `create_and_link_worktree` does in the same situation (copy its sentences). If another card already links that worktree, return `worktree_owned_by_another_card`.
    6. Otherwise set `card.worktree_id` and `card.repo_id`, push a `WorktreeCreated` activity (copy the one `create_and_link_worktree` pushes), save with `BoardChangeReason::CardChanged`.
  - The worktree host: if the found worktree has `host: Some(host)`, return the unchanged host sentence from C5. `run_for_card` would refuse it anyway, but the card should say why.
  - In `automation.rs::start_for_card`, right after the `automation()` check, call `ensure_pull_request_worktree`. On error, call `prepare_run` to get the `RunRow`, then record `failed(&prepared.row, &error, now)` through `record_run`, exactly as the existing refusal branch does. If `prepare_run` answers `None`, release the reservation and return, as the existing code does. The reservation is held throughout, so a slow clone counts against `max_live_runs`. That is intended.
  - In `card_request`, choose the worktree by `board.settings.run_location`:
    - `BoardWorktree`: today's code;
    - `CardWorktree`: `card.worktree_id.clone()`, or the first C5 sentence as an `Invalid` (reachable only if the card lost its link between the two steps).
  - Add `worktree: WorktreeId` to `RunRow`, filled in `prepare_run` by the same rule (move the choice into one small fn used by both), and copy it into `CardRun.worktree_id` in `started()` and `failed()`.
  - Tests. Use the daemon test fixtures that fake worktree creation; grep `create_from_pr` in `crates/fleet-daemon/src/services/worktrees` tests, and `FakeGit` / `FakeGithub`:
    - `a_review_card_gets_its_pull_request_worktree_on_first_run` (asserts the link, the `WorktreeCreated` entry, and that the delegation request's worktree is the card's);
    - `a_second_run_reuses_the_card_worktree`;
    - `a_card_without_a_pull_request_or_worktree_records_the_refusal`;
    - `a_pull_request_in_an_unknown_repository_records_the_refusal`;
    - `a_card_deleted_while_its_worktree_is_created_is_refused`;
    - `two_cards_run_at_once_in_two_worktrees` (with `max_live_runs = 2`);
    - `a_card_run_records_its_worktree`.
  - Docs:
    - BOARD.md §4.1: describe the pre-start step and its gate discipline, with the same level of detail as the existing start chain description;
    - §11.2: add `CardRun.worktree_id`.
- **Verification:** `cargo test -p fleet-daemon automation`, `cargo test -p fleet-daemon worktree`, `make lint`, `make test`.
- **Done when:** the seven tests pass and a board-worktree board's runs behave exactly as before, with every existing automation test green.

### P2-T06 — Diff each run against the worktree it ran in
- **Intent:** the delivery report's "Files changed since this run started" section reads the run's own worktree, and the "Other runs share this worktree" sentence appears only when runs actually share one.
- **Touches:** `crates/fleet-daemon/src/services/boards/automation.rs` (`changed_files` around 692, the `files_section` call around 636), the daemon automation tests, `docs/BOARD.md` §11.2.
- **Steps:**
  - In `changed_files`, use the delivered run's `CardRun.worktree_id` when it is `Some`, and fall back to `doc.board.worktree_id` (runs recorded before this feature). Find the run with the delegation id being delivered; the function already has the delegation.
  - Pass `shared = doc.board.settings.run_location.is_board_worktree() && doc.board.settings.max_live_runs() > 1` to `files_section`, instead of the bare `max_live_runs() > 1`.
  - Review runs should change nothing. The section shows files only when a run did change something, which is itself useful: a review that edits files broke its instructions. Leave that behaviour as it is.
  - Tests:
    - `a_card_worktree_run_diffs_its_own_worktree`;
    - `card_worktree_runs_never_print_the_shared_sentence`;
    - `a_run_recorded_before_worktree_ids_diffs_the_board_worktree`.
  - Docs: in BOARD.md §11.2, change "it is the thread's first checkpoint tree against a snapshot of the worktree" to name the run's worktree, and state the shared-sentence rule.
- **Verification:** `cargo test -p fleet-daemon automation`, `make lint`.
- **Done when:** the three tests pass.

### P2-T07 — Drive Reviews boards from the CLI and advertise the capability
- **Intent:** `fleet board --reviews` selects the context's Reviews board, `card new --pr/--requested-at` upserts and prints the outcome line, `card show` prints the pull request, and the daemon advertises `board.reviews`.
- **Touches:** `crates/fleet-cli/src/args.rs`, `crates/fleet-cli/src/commands/board.rs`, `crates/fleet-cli/src/envelope.rs`, `crates/fleet-cli/src/human.rs`, CLI tests (grep `commands/board` tests), `crates/fleet-daemon/src/server/connection.rs`, `docs/BOARD.md` §6.
- **Steps:**
  - `BoardArgs`: add `#[arg(long, global = true, conflicts_with_all = ["board", "worktree"])] reviews: bool`. Copy the doc-comment style of `--context`. `--reviews --context X` means X's Reviews board; `--reviews` alone means the active context's.
  - In the selector resolution around lines 312-360, add a branch before the context fallback: resolve the context the way `--context` / the active context already do, then call `client.ensure_reviews_board(context)`.
  - `BoardCardCommand::New`: add `--pr <REF>` and `--requested-at <RFC3339>`. Parse `--pr` with `PullRequestRef::parse` and print its error sentence verbatim. Refuse `--requested-at` without `--pr` with `--requested-at needs --pr`.
    - With `--pr`: build the same `CardDraft` `card_new` builds today, plus `pull_request`; call `upsert_pull_request_card`; print `Created {KEY}` / `Existing {KEY}` / `Reopened {KEY}` as the first line, then the card exactly as `card new` prints it. Copy how `card worktree` prints `Created`/`Existing`, around line 1297.
    - With `--json`: print a `BoardCardUpsertEnvelope { protocol: 1, card, outcome }`, added in `envelope.rs` next to `BoardCardEnvelope`, with a doc comment.
  - `card show` human output: a `pull request  {key}  {url}` fact line when the card has one. Follow the existing fact-line layout in `human.rs`.
  - `board show` header: when the board is a Reviews board, the scope suffix reads `· reviews of <context>`. Look at how the worktree suffix `· worktree …` is built.
  - Advertise `BOARD_REVIEWS_CAPABILITY` in the daemon's Hello list. This is the last card of the phase, so the feature is complete when advertised.
  - Tests, with the existing CLI test harness (arguments parsed against an in-memory or fake daemon; see `commands/board/columns/tests.rs` for the planning-style tests):
    - `--reviews` conflicts with `--board` and `--worktree`;
    - `--requested-at` without `--pr` is refused;
    - a bad `--pr` prints the parse sentence;
    - the three outcome lines print.
  - Docs: add the `--reviews` selector and the two flags to the BOARD.md §6 CLI fence, with the output line.
- **Verification:** `cargo test -p fleet-cli`, `make lint`, `make test`, then `make restart` and by hand:
  ```
  fleet board --reviews show
  fleet board --reviews card new "Try it" --pr <a real PR url in a cloned repo>
  ```
- **Done when:** the tests pass, and the hand run creates a card that moves to Reviewing and gets a PR worktree.

### P2-T08 — Pin the review flow end to end with a smoke script and update the planning skill
- **Intent:** `make smoke-reviews` proves Pending review → Reviewing → Reviewed with scripted agents against a private daemon; the workflow smoke and the planning skill match the new queue rule.
- **Touches:** `scripts/reviews-smoke.sh` (new), `Makefile` (`smoke-reviews`, added to `ci`), `scripts/board-workflow-smoke.sh`, `.claude/skills/fleet-board-planning/SKILL.md`, `docs/DEVELOPMENT.md` (the section that lists the smoke targets; grep `smoke-workflow`).
- **Steps:**
  - Read `scripts/board-workflow-smoke.sh` end to end first. Note how it:
    - builds a private `FLEET_HOME`;
    - starts `fleetd`;
    - installs scripted fake agent binaries that call `fleet subagent complete`;
    - polls `fleet board show --json` with a timeout;
    - cleans up.
  - Write `scripts/reviews-smoke.sh` on the same skeleton:
    - create a context and a local bare-repo "GitHub" fixture, the way the workflow smoke provides its repo;
    - PR worktree creation needs `refs/pull/<n>/head` in the origin, so create that ref in the bare repo with `git update-ref`;
    - `gh` is not available in CI: set the board's `reviewing` column instructions to something the scripted agent ignores. The scripted agent completes with a fixed review report;
    - run `fleet board --reviews card new "Fixture PR" --pr <owner>/<name>#1`;
    - wait for the card to reach `reviewed`, and assert that its latest run succeeded, that its worktree is linked, and that a second `card new --pr` prints `Existing`.
  - Add `smoke-reviews` to the Makefile next to `smoke-workflow`, and add it to `ci`.
  - Update `scripts/board-workflow-smoke.sh` if any step depended on an unblocked card waiting in Ready. Run `make smoke-workflow` to find out.
  - In the planning skill, check the phase-1 edit to "Building a chain" still reads correctly, and add a short "Review boards" subsection: `fleet board --reviews`, `card new --pr`, the five columns, and that moving a card to Review published posts the review.
- **Verification:** `make smoke-reviews`, `make smoke-workflow`, `make lint`, `make test`.
- **Done when:** both smokes pass locally and `make ci` includes the new target.

## Verification

- `make lint`
- `make test`
- `make smoke-workflow`
- `make smoke-reviews`
- `make restart`, then the manual CLI run in P2-T07.

## Definition of done

- [ ] Every phase-2 card is in Done on the FEA board.
- [ ] `make lint` clean and `make test` passing.
- [ ] Both smokes pass.
- [ ] `docs/BOARD.md` §4, §4.1, §5, §6 and §11 describe what the code does.
- [ ] `board.reviews` is advertised only by the build that serves both requests.
- [ ] Follow-ups (auto-clone, worktree cleanup) captured as Backlog cards.

## Risks and rollback

- **Worktree creation holds a run slot for minutes.** This is intended and documented. If it proves painful, a follow-up can create worktrees ahead of time when a card is upserted.
- **A worktree adopted by slug belongs to someone else's work.** `create_from_pr` adopts a same-named worktree. The "owned by another card" check prevents two cards sharing one, and a user's own worktree on that branch is simply reused. Document it in §4.1.
- **Rollback:** each card is one commit. Reverting P2-T07 un-advertises the feature, and the app (phase 4) then hides it.
