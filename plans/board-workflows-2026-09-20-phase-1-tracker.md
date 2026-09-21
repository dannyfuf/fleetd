# Board workflows, phase 1: core — the automation domain model — Tracker
> Plan: ./board-workflows-2026-09-20-phase-1-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
      `cargo check --workspace --all-targets` clean on the untouched tree (2026-09-20).
- [x] I am ready to start.

## Tasks
- [x] P1-T01 — Column automation and the board throttle
      verified: `cargo test -p fleet-core board` green; a status with no automation still encodes
      `{id,name,category,color}` exactly, `normalise_automation` drops an emptied block and
      `apply_board_patch` calls it.
- [x] P1-T02 — Card fields, run history, activity, comments and the patch surface
      verified: `cargo test -p fleet-core` (238 lib tests) and `cargo check --workspace
      --all-targets` green; the legacy card encodes byte-identically, the four new fields
      round-trip, `agent`/`blocked_by` patch and clear.
- [x] P1-T03 — Lazy `document_version` and the store's `1..=2` range
      verified: `cargo test -p fleet-daemon --lib stores::board` (9 tests) and
      `cargo test -p fleet-core` green. `load`/`peek` share one `supported_version` range check,
      `save` re-stamps from `document_version`, and four store tests pin the behaviour: a plain
      document writes 1, one automated column writes 2, a v1 fixture loads/saves and stays 1, and
      a version-3 document is refused by the exact sentence without being quarantined. Two
      `fleet-core` tests pin `document_version` itself — every one of the six opt-in facts alone
      takes a document to 2.
- [x] P1-T04 — `validate_automation`, `validate_links` and `validate_env`
      verified: `cargo test -p fleet-core --lib board::ops::tests::validation` green (14 tests).
      `validate_board` now calls `validate_automation`; `validate_env` and `validate_links` are
      pure and called by nobody yet. Every sentence in contracts §1.7 that phase 1 implements is
      asserted verbatim through `BoardError::Invalid` equality: the three routing refusals, the
      two `on_enter` refusals, the five `--env` sentences, `max_live_runs`, and the three
      `blocked_by` sentences (cross-board, self, two- and three-card cycles). A diamond is proved
      not to be a cycle.
- [x] P1-T05 — Derived reads, the workflow preset and `render_template`
      verified: `cargo test -p fleet-core --lib board::ops::tests::automation` green (13 tests).
      `blocks`, `is_satisfied`, `Blocked`/`BlockedTone`, `blocked` are new in `ops/query.rs`;
      `latest_run` and `attention` had already landed with T02's `summarize`. `workflow_preset`,
      `apply_workflow_preset` and `render_template` are in `defaults.rs`, and the preset is proved
      to validate, to add exactly Ready and In review over `default_statuses()`, to leave every
      pre-existing column byte-identical, and to be idempotent.
- [x] P1-T06 — The capability string, the `Card` goldens and the documents
      verified: `cargo test -p fleet-proto --test compatibility` green (13 tests) with every
      pre-existing golden unchanged. `BOARD_AUTOMATION_CAPABILITY` is defined in `response.rs`
      and named by nothing else; the daemon's advertised list and its literal test mirror were
      not touched. Three new goldens: a `Card` carrying agent prefs, a blocker, a pending run, a
      terminal `CardRun` with every optional field and a report comment; a `BoardView` whose
      column carries a full `ColumnAutomation` and whose settings carry `maxLiveRuns`; and
      `a_card_without_automation_fields_encodes_as_it_did_before` pinning the legacy card's bytes.
      Docs: `docs/BOARD.md` §2 (every new type, the lazy-version paragraph) and a new §11
      (model only, with the phase each later part arrives in), `docs/decisions/0022-board-workflows.md`,
      and the `docs/README.md` row after 0021.

## Notes / decisions log
- 2026-09-20 — The `ActivityKind` sweep reached **no** renderer: `cargo check --workspace
  --all-targets` plus `grep -rn "ActivityKind::" crates/` show no exhaustive match outside
  `fleet-core`, so `fleet-app` and `fleet-cli` needed no new arm. What the sweep did reach was
  every struct literal of `Card`, `Status`, `Comment`, `BoardSummary` and `BoardView` (daemon
  services, router and mirror fixtures, app and CLI test fixtures) plus the five `summarize`
  call sites.
- 2026-09-20 — `summarize(board, cards, live, now)`: the daemon passes `self.now()`, the two
  CLI call sites pass `""` with a comment — they read only the dirty and conflict counts and
  have no RFC 3339 clock there (`now` is epoch seconds in `human.rs`). Phase 5 owns those
  surfaces and should pass a real stamp when it prints the working/attention counts.
- 2026-09-20 — `validate_links`'s unknown-blocker sentence names the **card id**, not a display
  key: a card that is not in `cards` has no `display_key` to compute. Contracts §1.7 writes it as
  `{KEY} is not on this board`; the id is the only name the board has for a card it does not hold.
- 2026-09-20 — The cycle sentence is the path from the card being written back to itself, so a
  two-card cycle is the contract's three-key template (`FLE-1 → FLE-2 → FLE-1`) and a three-card
  cycle is four keys. The plan asks the three-card message to "list all three keys", which it does.
- 2026-09-20 — `apply_workflow_preset` never touches an existing column, per contracts §1.9. The
  consequence worth knowing for phase 5 and 8: applying the preset to a board built from
  `default_statuses()` adds Ready and In review but leaves the shipped `in-progress` column with
  **no** `on_enter` action, because that column already exists. A board wanting the full pipeline
  must either start from `workflow_preset()` or edit `in-progress` afterwards. The CLI's
  `columns preset` verb (phase 5) should say so.
- 2026-09-20 — Two files the plan's "Affected areas" list did not name had to change, both
  because a doc or a doc comment would otherwise have been false:
  `crates/fleet-daemon/src/services/agents/delegation/limits.rs` gains the one test the
  `MAX_LIVE_RUNS_PER_BOARD` doc comment promises ("a daemon-side test asserts the two agree" —
  no phase owned it), and `docs/ARCHITECTURE.md:726` said the board document is "version 1",
  which the lazy stamp makes wrong.
- 2026-09-20 — Drive-by: `docs/BOARD.md` §2's `SyncState` block was missing `readonly_fields`,
  which the code has had since the Jira backend landed and which this phase's new `BoardView`
  golden prints. Added, since §2 is the block this task rewrote.
- 2026-09-20 — `crates/fleet-cli/src/commands/board/tests.rs` needed `rustfmt` (the T02 sweep left
  one `summarize(...)` call unformatted and `make lint` runs `cargo fmt --all -- --check`).
- 2026-09-21 — Orchestrator's `make test` caught one regression the narrow commands missed:
  `services::boards::tests::retiring_a_board_for_a_hosted_worktree_keeps_its_cards_in_the_trashed_document`
  (from PR #43) built its expected document with `version: BOARD_DOCUMENT_VERSION` (now 2) and
  compared it to the trashed file, which the lazy stamp writes at 1. The `stale_hosted_board`
  helper now stamps with `document_version(&board, &cards)`, the same call `save` makes. The
  other `BOARD_DOCUMENT_VERSION` fixtures in that file and in `lifecycle.rs` are only ever
  re-stamped by `save` and compare nothing, so they stay. Two further failures in the same run
  (`real_shell_board_pane_refuses_to_reopen_its_own_worktree` leaked-handle check, and
  `github_service::concurrent_misses_share_fetch`) pass three of three in isolation and touch no
  phase-1 file; both ran while another worktree's build was hogging the CPU.

## Follow-ups
- Phase 3 owns the `automation` refusal sentences of contracts §1.7 (worktree-only, local-only,
  hosted-host); phase 1 fixes the words in `docs/BOARD.md` §11 and implements none of them.
- Phase 3 must add `BOARD_AUTOMATION_CAPABILITY` to `server/connection.rs`'s advertised list
  **and** to the literal list written out in its test mirror, in the same commit as the requests.
- Phase 5 should pass a real RFC 3339 stamp to `summarize` at the two CLI call sites that pass
  `""` today (carried over from T02's note), so `board list`'s WORKING and NEEDS YOU columns count.
- `crates/fleet-core/src/board/model.rs` is now 842 lines, against `rust-workspace-architecture`'s
  ~900-line split threshold. The next phase that adds a type to it should split the automation
  types out into `board/model/automation.rs` rather than push it past the line.

