# Board workflows, phase 1: core — the automation domain model — Plan
> Tracker: ./board-workflows-2026-09-20-phase-1-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Add every type, field, validation, derived read and constant the automation engine will write to, and serve none of it. After this phase `fleet-core::board` knows about column automation, card agent preferences, `blocked_by` links, a pending run and a run history; `BOARD_DOCUMENT_VERSION` is 2 but stamped **lazily**, so a board that never opts in still writes 1 and an older daemon still reads it; the store reads `1..=2`; a cycle, a cross-board link, a self-link, a backward routing, a Codex skill action, a reserved env key and a throttle outside `1..=8` are each refused with the contract's sentence; `workflow_preset()` builds the seven-column pipeline matched by id. `board.automation` exists as a string in `fleet-proto` and is **not** advertised, so a daemon built from this phase behaves exactly as before for every client.

## Sizing call

**Phased, phase 1 of 9.** See ./board-workflows-2026-09-20-roadmap.md; shapes are fixed by ./board-workflows-2026-09-20-contracts.md §1 and §3.6. This is the initiative's widest mechanical sweep inside one crate plus two goldens, and the one phase a reviewer can approve without reading the execution model. It is its own pull request so the version rule, the refusal sentences and the ADR get a review of their own.

## Repository context

- Rust 2024 workspace, toolchain pinned. Touches `fleet-core`, `fleet-proto`, `fleet-daemon` (`stores/board.rs` only) and whatever the `ActivityKind` sweep forces.
- Model: `Status` `crates/fleet-core/src/board/model.rs:85-95`; `Card` :179-245; `field_label` :252-266; `display_key` :270-274; `Comment` :284-297; `ActivityKind` :316-333 (eight variants); `BoardSettings` :374-387 with `impl Default` :491-500; `BoardSummary` :437-463; `BoardView` :469-474; `BoardDocument` :480-487; `BOARD_DOCUMENT_VERSION = 1` :489.
- Serde patterns to copy verbatim: skipped `Option` :19-20, plain `Option` :93-94, `Vec` :33-34, `bool` :233-234, `default_true` :376-377.
- Ops: `BoardError::Invalid { field, reason }` `board/ops.rs:180-186`; private `nested_option` :207-213 and `invalid` :215-220; `default_true` :224; `CardDraft` :32-65; `CardPatch` :71-121 (five `nested_option` fields); `is_empty` :125; `apply_card_patch` `ops/patches.rs:79-154`.
- `push_activity` `ops/cards.rs:191-208` (cap 200 at :204-205); `move_card` :90-158; `summarize` `ops/query.rs:54-81`; `validate_board` `ops/validation.rs:4-51`; `validate_card` :54-95; helper style :138-197; test fixtures `board/ops/tests.rs:17-56`, named single-scenario `#[test]`s.
- Defaults: `default_statuses()` `board/defaults.rs:15-31` (`in-progress` is named `In Progress`); re-exports `board.rs:11-23`.
- Store `crates/fleet-daemon/src/stores/board.rs`: read guard :97-104 (a `!=` refusal, not quarantined); `save` :131-138; write guard `validate_document` :244-263; `DocumentVersion` :239-242; test fixture :274-294; `a_future_document_version_is_reported_without_quarantining_the_file` :399-417 (:404 is the line to move).
- Proto: `BOARD_WORKTREE_CAPABILITY` `crates/fleet-proto/src/response.rs:59`; advertisement list `fleet-daemon/src/server/connection.rs:924-935` and its mirror :1029-1036 are **not** touched. Goldens are inline byte-exact literals through `assert_frame` `crates/fleet-proto/tests/support/mod.rs:16-42`; board examples `tests/compatibility.rs:143-166`, `board_view()` :344-369, legacy decode test :371-400.
- The five `--env` refusal sentences: `crates/fleet-cli/src/commands/subagents.rs:353-380` (`RESERVED_ENV_PREFIX = "FLEET_"` :33). `MAX_LIVE_DELEGATIONS: usize = 8` `services/agents/delegation/limits.rs:9`.
- Skills to load first: `rust-workspace-architecture` (new types, error policy, the ADR, the commit form), `rust-ipc-protocol` (the capability and the goldens), `rust-gpui-testing` (every test), `zed-quality-review` before calling the phase done.
- Docs to keep in step: `docs/BOARD.md` §2 (:62, model block from :71), `docs/README.md` decision index, new `docs/decisions/0022-board-workflows.md`.

## Assumptions

- `CardRun` and `LiveRun` pull `DelegationId`, `ThreadId` and `DelegationStatus` from `fleet_core::agents`; same crate, so no new crate edge appears.
- Phase 1 only *defines* the activity sentences of contracts §1.4. Phase 3's engine formats and pushes them; this phase adds the variants, restores every exhaustive match, and records the sentences in `docs/BOARD.md` §11 so phase 3 copies rather than invents them.
- `ops::summarize` grows two arguments; every current caller passes `&[]` and its own `now`, because nothing joins live runs until phase 3.
- No JSON migration is written. A v1 document is read as-is and `save` re-stamps it from `document_version`; that is what makes the bump lazy.

## Out of scope

- `fleet-core::board::automation` (`re_evaluate`, `Plan`, `LiveIndex`, `brief`), the three run requests, `MoveCard.cancel_run`, the `BoardView.live_runs` golden and advertising the capability: phase 3.
- `DelegationCaller` and anything in `fleet-core::agents::delegation`: phase 2.
- Any CLI flag, dialog or tile mark: phases 5 to 8.

## Affected areas

- `crates/fleet-core/src/board/model.rs` — `ColumnAutomation`, `Action`, `ActionKind`, `ColumnAgentPrefs`, `CardAgentPrefs`, `PendingRun`, `CardRun`, `RunOutcome`, `LiveRun`, `Status.automation`, the four `Card` fields, `BoardSettings.max_live_runs`, two `BoardSummary` counts, `BoardView.live_runs`, `Comment.run_id`, three `ActivityKind` variants, five constants, the two version constants and `document_version`.
- `board/ops.rs` (`CardDraft`/`CardPatch` fields, `is_zero`), `ops/patches.rs`, `ops/query.rs`, `ops/validation.rs`, `board/defaults.rs`, `board.rs` re-exports.
- Tests: `board/tests.rs`, `board/ops/tests/{validation,patches,query}.rs`, new `board/ops/tests/automation.rs`.
- `crates/fleet-daemon/src/stores/board.rs` and its `mod tests`; `crates/fleet-proto/src/response.rs` and `tests/compatibility.rs`.
- `docs/BOARD.md`, `docs/README.md`, new `docs/decisions/0022-board-workflows.md`.

## Tasks

### P1-T01 — Column automation and the board throttle
- **Intent:** Give a column the one optional block that says what happens to a card entering it.
- **Touches:** `board/model.rs`, `board.rs`, `board/tests.rs`.
- **Steps:**
  - Add `ColumnAutomation`, `Action`, `ActionKind` (internally tagged `#[serde(tag = "kind", rename_all = "snake_case")]`) and `ColumnAgentPrefs` exactly as contracts §1.1, with every `skip_serializing_if`. `ActionKind::word()` answers `run card` for `Prompt` and `run skill {name}` for `Skill`.
  - `ColumnAutomation::is_empty` and `ColumnAgentPrefs::is_empty` are true when every field is `None`/empty. Add `pub fn normalise_automation(statuses: &mut [Status])` in `ops/patches.rs`, setting `automation = None` wherever the block is empty, and call it from `apply_board_patch` after `statuses` is set and from `apply_workflow_preset` (contracts §1.1).
  - `Status.automation: Option<ColumnAutomation>` last. `BoardSettings.max_live_runs: Option<u32>` plus `max_live_runs() -> u32` (`unwrap_or(1)`); `impl Default` leaves it `None`.
  - Constants: `MAX_RUNS_PER_CARD = 20`, `MAX_REPORT_COMMENTS_PER_CARD = 3`, `REPORT_EXCERPT_CAP_BYTES = 8 * 1024`, `PENDING_AMBER_AFTER_SECS = 60`, `MAX_LIVE_RUNS_PER_BOARD: u32 = 8` with a doc comment that it equals the daemon's `MAX_LIVE_DELEGATIONS` and that a daemon test asserts they agree.
  - Tests: a status without automation serialises with no `automation` key; a `Prompt` action round-trips; `Skill { args: "" }` omits `args`; an all-default block normalises to `None`.
- **Verification:** `cargo test -p fleet-core board`.
- **Done when:** A status with automation round-trips and one without is byte-identical to today.

### P1-T02 — Card fields, run history, activity, comments and the patch surface
- **Intent:** Everything a card must persist about its agent, its links and its runs.
- **Touches:** `board/model.rs`, `board/ops.rs`, `ops/patches.rs`, `ops/query.rs`, `board/tests.rs`, `ops/tests/patches.rs`, plus every file the sweep reaches.
- **Steps:**
  - `CardAgentPrefs`, `PendingRun`, `CardRun` (`is_live()`, `failed_to_start()`; `thread_id: Option<ThreadId>`, `None` only for a run whose start failed — contracts §1.3), `RunOutcome` (`word()` answers `succeeded`, `needs you`, `failed`, `incomplete`, `cancelled`; `needs_attention()` is `NeedsYou | Failed | Incomplete`) and `LiveRun`, exactly as contracts §1.3-1.4.
  - `Card` gains `agent`, `blocked_by`, `pending_run`, `runs` (oldest first, newest last); `Comment.run_id: Option<DelegationId>`; `BoardView.live_runs: Vec<LiveRun>` with the doc comment "joined on read, never persisted".
  - `ActivityKind` gains `RunStarted`, `RunEnded`, `AutoMoved`. Then the sweep: `cargo check --workspace --all-targets`, then `grep -rn "ActivityKind::" crates/` for every exhaustive match — the renderers in `fleet-app` and `fleet-cli` that map a kind to a glyph or a word are found by the grep. Known `push_activity` call sites: `ops/cards.rs:72,156,181`, `ops/patches.rs:145`, `board/sync/apply.rs:65,99,147,287`, `board/sync/reconcile.rs:169,203,262,281`, `fleet-daemon/src/services/boards/sync.rs:183,205,344`, `boards/cards.rs:145`, `boards/worktree.rs:232`.
  - `pub fn is_zero(n: &u32) -> bool` beside `default_true` in `ops.rs`. `BoardSummary` gains `working_count` (cards with a live run or a `pending_run`) and `attention_count` (`attention(card, now)`); `summarize(board, cards, live: &[LiveRun], now: &str)` fills them; update every caller found by `grep -rn "summarize(" crates/`.
  - `CardDraft` gains `agent` and `blocked_by`; `CardPatch` gains `agent: Option<Option<CardAgentPrefs>>` through `nested_option` and `blocked_by: Option<Vec<CardId>>`. `CardPatch::is_empty` and `apply_card_patch` learn both; the changed-field list reports `"agent"` and `"blocked_by"`; `field_label` gains the two labels.
  - Tests: a card with no new field serialises byte-identically to today; a card with all four round-trips; `agent: Some(None)` clears; `blocked_by: Some(vec![])` clears; an empty patch is still `is_empty()`.
- **Verification:** `cargo test -p fleet-core board`, `cargo check --workspace --all-targets`.
- **Done when:** The workspace compiles with the three new activity kinds and the card round-trips.

### P1-T03 — Lazy `document_version` and the store's `1..=2` range
- **Intent:** Bump the format only for documents that use the new fields, so a board that never opts in stays readable by an older daemon.
- **Touches:** `board/model.rs`, `crates/fleet-daemon/src/stores/board.rs` and its `mod tests`.
- **Steps:**
  - `BOARD_DOCUMENT_VERSION: u32 = 2`, `BOARD_DOCUMENT_MIN_VERSION: u32 = 1`, and `document_version(board, cards) -> u32`: 2 when any status carries `automation`, the settings carry `max_live_runs`, or any card carries a non-empty `blocked_by`, an `agent`, a `pending_run`, a non-empty `runs` or a comment with `run_id`; 1 otherwise (contracts §1.6). Doc comment: a document keeps writing 2 while any card still carries runs or links, because an older daemon would drop that history on its next save.
  - Read path (:97-104): accept `BOARD_DOCUMENT_MIN_VERSION..=BOARD_DOCUMENT_VERSION`; the refusal keeps its shape with the range — `board {id} uses document version {v} (this build reads 1..=2)` — and is still not quarantined.
  - `save` (:131-138) stamps `doc.version = document_version(&doc.board, &doc.cards)` **before** `validate_document`; `validate_document` (:244-263) checks the same range instead of `!=`.
  - Tests: move the future-version test to `BOARD_DOCUMENT_VERSION + 1` (:404); add `a_document_without_automation_still_saves_at_version_1` and `a_document_with_automation_saves_at_version_2`; keep `rejects_version_and_filename_mismatches` (:382-397) green.
- **Verification:** `cargo test -p fleet-daemon --lib stores::board`, `cargo test -p fleet-core board`.
- **Done when:** A v1 fixture loads, saves and is still v1; adding one column action makes it v2.

### P1-T04 — `validate_automation`, `validate_links` and `validate_env`
- **Intent:** Every refusal sentence the app, the CLI and the daemon will show, in one pure place.
- **Touches:** `ops/validation.rs`, `board/ops.rs` (re-export), `ops/tests/validation.rs`.
- **Steps:**
  - `validate_automation(board) -> Result<(), BoardError>`, called from `validate_board`. Per status: `on_success` and `advance_when_unblocked` must name a status on this board (`must name a status on this board`), may not name their own column (`may not name its own column`), and must name a column later in `board.statuses` order (`must name a later column`); the field is the routing key's name. `on_enter`: a `Skill` with a blank name is `on_enter` / `a skill action needs a name`; a `Skill` whose `agent.provider` is `Some(AgentKind::Codex)` is `on_enter` / `skill actions run on claude only; put the invocation in the column's instructions for codex`. Then `validate_env(&action.env)`. Finally `settings.max_live_runs`, when `Some`, must be in `1..=MAX_LIVE_RUNS_PER_BOARD` or `max_live_runs` / `must be between 1 and 8`.
  - `validate_env(env: &[String])` copies the five sentences from `subagents.rs:357-380` with the leading `--env ` dropped: not `KEY=VALUE`, empty key, a `FLEET_`-prefixed key, `PATH`, the same key twice. Field `env`.
  - `validate_links(board, cards, card)`: every entry must be a card in `cards` (`{KEY} is not on this board`, KEY from `display_key`), may not be the card itself (`a card cannot block itself`), and may not close a cycle — walk `blocked_by` from the card being written and on returning to it report `would close a cycle: {KEY} → {KEY} → {KEY}`, the path in display keys from the card back to itself. Field `blocked_by`. Pure; the daemon calls it beside `validate_parent` in a later phase, nothing calls it here.
  - Tests, one named scenario each: backward routing, self-routing, unknown target, blank skill name, codex skill, each of the five env sentences, `max_live_runs` of 0 and of 9, a cross-board link, a self-link, a two-card cycle, a three-card cycle whose message lists all three keys.
- **Verification:** `cargo test -p fleet-core validation`.
- **Done when:** Every sentence in contracts §1.7 is asserted verbatim by a test.

### P1-T05 — Derived reads, the workflow preset and `render_template`
- **Intent:** The pure reads the face, the CLI and the engine share, and the one preset.
- **Touches:** `ops/query.rs`, `board/defaults.rs`, `board.rs`, new `ops/tests/automation.rs`.
- **Steps:**
  - `query.rs`: `blocks(cards, card)`; `is_satisfied(board, cards, blocker)` (blocker's current column has category `Completed`); `Blocked { unsatisfied: u32, tone: BlockedTone }` and `BlockedTone { Muted, Warning }` (`Warning` when any unsatisfied blocker sits in a `Canceled` column or is `archived`); `blocked(board, cards, card) -> Option<Blocked>`; `latest_run(card)` (`runs.last()`); `attention(card, now)` — the latest run's outcome `needs_attention()` with no `ActivityKind::Moved` entry newer than that run's `ended_at`, or a `pending_run` older than `PENDING_AMBER_AFTER_SECS`. `Moved` is written only by a human or CLI move; an outcome move is `AutoMoved` (contracts §1.4).
  - `defaults.rs`: the four preset constants of contracts §1.9. `workflow_preset() -> Vec<Status>` returns `backlog`/Backlog/backlog, `todo`/Todo/unstarted, `ready`/Ready/unstarted with `advance_when_unblocked = in-progress`, `in-progress`/In Progress/started with a `Prompt` action carrying `PRESET_INSTRUCTIONS_IMPLEMENT` and `PRESET_EXPECT_IMPLEMENT` and `on_success = in-review`, `in-review`/In review/started with a `Skill { name: PRESET_REVIEW_SKILL, args: "" }` action carrying `PRESET_EXPECT_REVIEW` and `on_success = done`, `done`/Done/completed, `canceled`/Canceled/canceled. The preset sets no `max_live_runs`.
  - `apply_workflow_preset(board) -> bool` adds only the preset columns missing **by `StatusId`**, in preset order relative to the neighbours already present, never touching an existing column's name, category, colour or automation.
  - `render_template(text, key, title)` substitutes `{key}` then `{title}` and leaves unknown braces alone.
  - Tests in `ops/tests/automation.rs`: `blocks` is the reverse of `blocked_by`; a `Completed` blocker satisfies and a `Canceled` one does not and tones `Warning`; `attention` is true after a `Failed` run and false after a later manual move; a pending run younger than a minute is not attention and an older one is; the preset over `default_statuses()` adds exactly Ready and In review and keeps `In Progress`'s name; applying it twice is a no-op; `render_template` substitutes both keys.
- **Verification:** `cargo test -p fleet-core board`.
- **Done when:** The preset is idempotent by id and every derived read has a named test.

### P1-T06 — The capability string, the `Card` goldens and the documents
- **Intent:** Reserve the capability without advertising it, prove the new card shape on the wire, and make `docs/` describe the model before anything uses it.
- **Touches:** `crates/fleet-proto/src/response.rs`, `tests/compatibility.rs`, `docs/BOARD.md`, `docs/README.md`, new `docs/decisions/0022-board-workflows.md`.
- **Steps:**
  - `BOARD_AUTOMATION_CAPABILITY: &str = "board.automation"` beside `BOARD_WORKTREE_CAPABILITY` (`response.rs:59`), with a doc comment naming what it gates and the sentence "defined in phase 1, advertised in phase 3". Leave the advertisement list and its test mirror alone.
  - Goldens through `assert_frame`: a `ResponseBody::Card` whose card carries `agent`, `blocked_by`, `pending_run` and one terminal `CardRun` with every optional field set; a `BoardView` whose status carries a full `ColumnAutomation`; and `a_card_without_automation_fields_encodes_as_it_did_before` asserting the legacy card's bytes. The existing board goldens (:143-166) and `legacy_board_shapes_without_worktree_id_decode_as_unscoped` (:371-400) stay byte-for-byte untouched — a diff in either is a missing `skip_serializing_if`, never a fixture to refresh.
  - `docs/BOARD.md` §2: extend the model block with the new types and a paragraph on the lazy document version and the `1..=2` read range. New §11 "Automation", model part only: the column block, the card fields, the run history and its caps, the three activity sentences of contracts §1.4 verbatim, the refusal table of §1.7 verbatim, the derived reads, the preset table, and a line saying the engine, the wire verbs and the face arrive in phases 3, 3 and 6-8.
  - New ADR `docs/decisions/0022-board-workflows.md` in the shape of `0019-worktree-scoped-boards.md`, recording the design's Decisions section: a failed review stops; review scope comes from the checkpoint diff; the throttle is board-level and 1 in the preset; runs never commit; the routing column is Ready, not Queued; three rows, not a three-stage picker; model and effort lists; no spend cap in v1; skill actions run on Claude only; Todo stays human; live run state is never on the card; reports capped at 8 KiB and three per card; the document version bumps lazily; `after_card_entered` returns a plan applied outside the non-reentrant gate with an in-flight reservation; columns live in Board settings; one column glyph, ⚡; `A`/`X`/`>` on the workspace board and the detail only; no `satisfies_blockers` in v1; runs live on the daemon that owns the worktree, reached through the router (ADR 0021, merged from PR #43 on 2026-09-20), so a laptop daemon never runs automation for a worktree it only mirrors. Each with its one-line argument and, where one exists, the named alternative.
  - Add the `| [0022](decisions/0022-board-workflows.md) | … |` row to `docs/README.md` after 0021.
- **Verification:** `cargo test -p fleet-proto --test compatibility`; read the `docs/` diff beside the code.
- **Done when:** Every new shape has a golden, no existing golden moved, and a reader of §11 can describe the model without reading Rust.

## Verification

```sh
make lint
cargo test -p fleet-core
cargo test -p fleet-proto --test compatibility
cargo test -p fleet-daemon --lib stores::board
make test
```

Nothing a human can see changes, so `make harness` is **not** required for this phase. Run `make restart` at the end so the running `fleetd` reads `1..=2` before phase 2 starts.

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` and `cargo check --workspace --all-targets` are clean.
- [ ] `make test` passes with every pre-existing golden unchanged.
- [ ] A v1 document round-trips at v1; a document with automation writes 2 and a `1..=1` build refuses it by name.
- [ ] Every sentence in contracts §1.7 is asserted verbatim by a test.
- [ ] `board.automation` is defined and **not** advertised.
- [ ] `docs/BOARD.md` §2 and §11, the ADR and the `docs/README.md` row ride in the same commits as the code they describe.

## Risks and rollback

- **A missed `skip_serializing_if` changes a shipped golden.** Treat any diff in an existing board golden or in the legacy fixture as a bug in the new field. Rollback is deleting the field.
- **The document bump is forward-only in effect.** Once a user adds a column action, that board is refused by a `1..=1` daemon; recovery is removing the automation, the links and the runs. Say so in the PR.
- **The `ActivityKind` sweep breaks clippy `-D warnings` everywhere at once.** Run `cargo check --workspace --all-targets` before writing any renderer arm; budget an hour.
- **`summarize`'s new arguments touch every caller.** Change only the call, nothing else in those files.

## Contract resolutions

The gaps this plan's first draft found were resolved in the contracts file on 2026-09-20; the
contracts wording wins over any earlier phrasing above.

1. `normalise_automation` is `ops::patches::normalise_automation(&mut [Status])`, called by `apply_board_patch` after setting `statuses` and by `apply_workflow_preset` (contracts §1.1).
2. The `max_live_runs` refusal lives in `validate_automation(&Board)` (contracts §1.7).
3. `is_zero` stays in `ops.rs` beside `default_true`; `model.rs` already imports `default_true` from there (model.rs:376), so the direction is not new.
4. An outcome move is `ActivityKind::AutoMoved` with `Moved to {column}: run succeeded`; `Moved` is written only by a human or CLI move, which is what `attention` keys on (contracts §1.4, §1.8).
5. For a `Skill` action `resolve_prefs` forces `claude` and ignores a card's `provider: codex`; nothing is refused at entry (contracts §2). Phase 3 implements it.
6. The `Card` and `Status` goldens and the legacy `Card` fixture proof belong to this phase (contracts §3.6).
7. `document_version` also answers 2 for `max_live_runs`, card `agent` prefs and a comment with `run_id` (contracts §1.6).
8. `fleet-cli`'s `child_environment` keeps its own sentences and is not changed by this feature (contracts §1.7).
