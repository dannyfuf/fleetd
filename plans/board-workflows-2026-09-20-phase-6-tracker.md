# Board workflows, phase 6: app — the board face — Tracker
> Plan: ./board-workflows-2026-09-20-phase-6-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- A contract gap in the plan is a stop-and-ask, not a decision to make alone; record the answer here.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
  (Not tickable by any phase-6 owner, for phase 5's reason: the tree was never clean — the nine
  phases were built in parallel in one worktree — and `make lint`/`make test` belong to the
  integration stage rather than to a task. Ticked here 2026-09-21 by the phase-9 docs audit, which
  reconciled every tracker with what the tree actually holds.)
- [x] I am ready to start.

## Tasks
- [x] P6-T01 — `RunMark`, `BlockedTone`, `CardTile::run`/`.blocked`, `KanbanColumn::action`
  - verified: `cargo test -p fleet-ui-kit` (347 pass, three new `card_tile` tests),
    `cargo clippy -p fleet-ui-kit --all-targets --all-features -- -D warnings` clean,
    `cargo build -p fleet-ui-kit --example gallery_board` clean; every mark and both tones
    have a gallery panel and the live board's "In progress" column carries `⚡`.
- [x] P6-T02 — `CardMarks`, `TileMarks` and `refresh_card_marks`
  - verified: the skeleton had already landed the two structs of contracts §5.2, the
    `BoardState.marks` field and the projection key's `marks: u64`; this task replaced the
    `// CONTRACT STUB (a:marks)` body with the fold and wired its four call sites
    (`apply_board_view`, `apply_card`, a card-called `DelegationChanged`, `AppState::tick`).
    `cargo test -p fleet-app --lib state::board` — 23 pass, eight of them new;
    `cargo test -p fleet-app --lib` — 932 pass, 0 fail; `cargo check -p fleet-app --all-targets`
    clean; `cargo clippy -p fleet-app --all-targets --all-features -- -D warnings` clean.
- [x] P6-T03 — The memo key, `CardRow.run`/`.blocked`, tiles, `⚡` and the header counts
  - verified: `ProjectionKey.marks` was already the skeleton's and was kept. `CardRow` gained
    `run`/`blocked`, filled in `CardRow::of` from the map `build` is handed; `ColumnRows` gained
    `has_action`; `HeaderFacts` gained `working`/`live_limit`/`needs_you` plus both finished
    header strings; `fn tile` chains `.run`/`.blocked` through `when_some`, `KanbanColumn::new`
    chains `.action`, and `fn header` prepends the two counts before the prefix badge. No
    `render` body reads a mark. `cargo test -p fleet-app --lib screens::board` — 15 pass
    (including the new `the_board_model_is_not_rebuilt_when_a_live_childs_headline_changes`);
    `cargo test -p fleet-app --lib board_screen` — 14 pass, three of them new;
    `cargo test -p fleet-app --lib` — 982 pass, 0 fail; `cargo check -p fleet-app --all-targets`
    clean; `cargo clippy -p fleet-app --all-targets --all-features -- -D warnings` clean on the
    third retry (the first two failed only on another owner's in-flight
    `dialogs/card_picker/draft.rs`, never touched).
- [x] P6-T04 — Additive harness marks, `board.summary`, `docs/TESTING-HARNESS.md` §3
  - verified: `cargo test -p fleet-app --lib state::harness` — 42 passed, 0 failed, six of them
    new under `tests::workflow`, including `a_card_states_its_run_and_its_blockers_in_the_frozen_vocabulary`
    (run mark, then `blocked:1`, then the assignee; the automated column's `action`; the
    `1/1 working` summary row) and `a_mark_only_the_clock_changed_moves_the_revision` (a pending
    run turning `stalled` on the tick moves the projection revision, which is what the new
    `ProjectionKey.marks` input buys). `docs/TESTING-HARNESS.md` §3 gained the three list names
    and the mark vocabulary; §2 and the target names are untouched. The throwaway `dump`
    against a live daemon is not run here — P6-T05's scenario is the one that reads it.
- [x] P6-T05 — A fixture that can show a run, and `scenarios/board/workflow-marks.scenario`
  - verified (scenario half): `scenarios/board/workflow-marks.scenario` is in and green.
    `make harness-one SCENARIO=scenarios/board/workflow-marks.scenario` (virtual lane, on a
    throwaway nested Hyprland) — **ok: 52 steps**, the `shot` included; the same file with
    `LANE=headless` fails on exactly one line, `shot workflow-marks`, and on nothing else, which
    is the §7 exclusion by construction (the Makefile's own selector lists the file as
    pixel-bound). It drives the real daemon: `]` twice carries AGE-1 from Todo through Ready
    into In Progress, the column's action starts a Claude child, and the face is awaited through
    `working` → `needs you`; AGE-2 behind it is `pending` and then, past
    `PENDING_AMBER_AFTER_SECS`, `stalled` on the app's own clock tick alone; AGE-3 and AGE-4
    carry `blocked:1` and `blocked:2`; In Progress and In review carry `action` and the other
    five columns do not; `board.summary` is absent before the first run, then `1/1 working`,
    `2/1 working` and finally `2/1 working · 1 needs you`. Five of the six run marks are
    covered — `done` cannot appear on this board by design, because both action columns carry
    an `on_success` (`state/board.rs::auto_advances`), and `state::board`'s own unit test owns
    it. The `shot` was opened and shows all of it; **no baseline is recorded**, see the log.
  - verified (fixture half): the `board-workflow` preset is in — `Preset::all()` is eight,
    `plan::board_workflow` seeds `agents-subagent`'s two worktrees and providers plus
    `acme/api#agent`'s board with the workflow columns, `maxLiveRuns` 1 and four Todo cards
    (1 blocks 3; 1 and 2 block 4). `cargo test -p fleet-harness` — 155 pass, two of them new
    (`plan::tests::the_board_workflow_preset_seeds_four_linked_cards_in_one_human_column`,
    `fixture::tests::the_board_workflow_preset_boots_with_an_automated_worktree_board`, which
    boots the preset and reads it back through a second daemon);
    `cargo clippy -p fleet-harness --all-targets --all-features -- -D warnings` clean.
    `docs/TESTING-HARNESS.md` §2's `fixture:` line and one §4 paragraph describe it. The
    scenario, its baseline and the `shot` are still open. *(They are not, now: see above.)*
- [x] P6-T06 — `docs/UX-SPEC.md` Board chapter and `docs/DESIGN-SYSTEM.md` glyphs
  - verified: written against the shipped code, not the reports — `state/board.rs`
    (`run_mark`/`pending_mark`/`auto_advances`/`report_failed_start`), `ui-kit/card_tile.rs`
    (`KeyMark`, the three glyph arms, `BLOCKED_GLYPH`), `kanban_column.rs::action`,
    `views/board_screen.rs::header` (counts left of the prefix badge, secondary then warning),
    `views/board_screen/model.rs::{HeaderFacts, ColumnRows::has_action}`. UX-SPEC "The pane" and
    "States" carry the key-line row, the `⚡` rule, the header counts and a table of the five
    marks; DESIGN-SYSTEM §5.2 gains a `RunMark` table (no new token, one new character `⊘`) and
    §6.7 the `run`/`blocked`/`action` builders; `BOARD.md` §11.8 is "On the board face".

## Notes / decisions log
- 2026-09-21 (i:app-docs) — `DESIGN-SYSTEM.md` §5.2 gained the run marks as a **second table**
  under the `StatusKind` one, not as rows inside it: `RunMark` is a different enum in a different
  component, and listing its five marks as `StatusKind` variants would have been false. The table
  states the same glyph, tone and word triple, plus the `⊘ n` count, and says explicitly that it
  adds no token and no icon — `zap`, `check` and `loader-circle` are all already in §5.1's closed
  set. §6.7's `CardTile` and `KanbanColumn` entries gained the three builders in the same pass,
  because an API block that omits a shipped builder is the drift that rule exists to prevent.
- 2026-09-21 (i:app-docs) — `BOARD.md` §11's preamble said the app surfaces "arrive in phases 6
  to 8"; they have. It now points at §11.8–§11.10 instead. That sentence is outside the app
  subsections this task owns, but a doc that dates itself by phase after the phase has landed is
  wrong, and the fix is one clause.
- 2026-09-21 (contracts:app skeleton) — `CardMarks`/`TileMarks` live in
  `crates/fleet-app/src/state/board.rs` and hold the *kit's* `RunMark`/`BlockedTone`, so the
  mapping P6-T01's note asks for happens in `refresh_card_marks`, once per change, and never in
  a tile. `state.rs:47` still has to re-export the two types for `views/board_screen*`.
- 2026-09-21 (P6-T01) — `RunMark` ships the five variants of contracts §5.1, not the four the
  phase plan's step list names: `Stalled` and `NeedsYou` both draw `StatusDot::small(Tone::Warning)`.
- 2026-09-21 (P6-T01) — precedence and zero-suppression live in a private `CardTile::key_mark()`
  returning `Option<KeyMark>`, so the rule "a run mark wins over a count, and `blocked(0, ..)`
  draws nothing" is unit-tested rather than only visible. `RunMark::ALL` / `BlockedTone::ALL`
  exist so the gallery (and P6-T04's mark words) enumerate rather than hand-list.
- 2026-09-21 (P6-T01) — kit `BlockedTone` is its own two-variant enum; it is deliberately *not*
  `fleet_core::board::BlockedTone` (the kit takes no domain type). P6-T03 maps one onto the other.
- 2026-09-21 (P6-T02) — the mapping the note above asks for is a `const fn tone_of` in
  `state/board.rs`, called once per card inside the fold; no tile and no `render` ever sees
  `fleet_core::board::BlockedTone`.
- 2026-09-21 (P6-T02) — `Succeeded` is suppressed when the column **that started the run** has
  an `on_success`, not the column the card is in now. An auto-advanced card would otherwise
  carry a check into the destination column, where every card is finished and the mark says
  nothing; reading the run's own column also keeps the mark stable when a human moves the card
  afterwards.
- 2026-09-21 (P6-T02) — the failed-start sticky error is silent on the *first* view of a board:
  with nothing behind it every run id is new, and a failure from another session is not news
  the moment the board appears. `apply_board_view` compares against the previous view's newest
  run id per card, which is also what makes it fire once per run.
- 2026-09-21 (P6-T02) — that error is written to `AppState::sticky_error` directly, in the same
  `StickyError { job: None, retryable: false }` shape `screens::board::lifecycle::fail` uses:
  `fail` needs an `Entity<AppState>` and a `&mut App`, and `apply_board_view` is pure state
  with neither. If the two ever diverge, `fail` is the definition.
- 2026-09-21 (P6-T02) — the clock tick the plan names lives in `state/connection.rs`
  (`AppState::tick`, the one calling `expire_toasts`), not in `notifications.rs`, so the tick
  hook was added there; it is guarded by `board_is_shown()` and only repaints when the marks
  revision actually moved. `notifications.rs` needed no edit.
- 2026-09-21 (P6-T02) — a live run the mirror and `live_runs` both fail to recognise is drawn
  `Working`: the card is the authority on whether its run has ended, and until the card says
  otherwise a child is out there. Only `DelegationStatus::Blocked` turns it into `NeedsYou`.
- 2026-09-21 (P6-T05, scenario) — **a bug the scenario found, and fixed: the board pane never
  reloaded a board the daemon changed by itself.** `Event::BoardChanged` sets `board_stale` and
  nothing else (`state/connection.rs::apply_daemon_event`). The Hub's board tab consumes that
  flag through the board screen's own observation of `AppState`
  (`screens::board::lifecycle::synchronize`), but that function returned early on anything that
  was not `Screen::Hub { tab: Board }` — so the *Workspace* pane, which is the only surface a
  worktree board is drawn in and therefore the only one automation ever reaches, consumed it
  never. Every daemon-side write was invisible until a hand-typed `r`: the first probe run sat
  on a card whose child had already gone blocked and drew no mark at all, and `board.summary`
  was absent while a run was live. The fix is a new
  `screens::board::refresh_after_daemon_change`, called from the shell's own event loop
  (`shell/root/events.rs`, right after `apply_batch`) rather than from that observation: the
  reload answers an *event*, not a repaint, and hanging it off the observation asked on every
  notify — including the ones a keystroke makes before any board has changed, which left three
  `real_shell_board_*` tests holding leaked `AppState` handles on a request no test daemon was
  ever going to answer. `begin_board_load` already clears the flag and refuses a second claim,
  so a batch of twenty events costs one `EnsureWorktreeBoard`. Regression test:
  `state::board::tests::a_daemon_side_change_leaves_the_worktree_pane_a_load_to_claim`.
  Without this fix no run mark, no header count and no auto-advance would have been visible in
  the app at all — phase 6's whole face was dark on the surface it was built for.
- 2026-09-21 (P6-T05, scenario) — **what the scripted provider cannot hold, and what follows.**
  A scripted child parks on a permission gate for `agent::GATE_BUDGET` — five seconds — and is
  then torn down (`agent/claude.rs`, `agent/peer.rs::read_frame_before`). The run is recorded
  `Cancelled`, and the column immediately starts the card again, for ever: a 90-second probe
  counted 17 restarts and 17 `cancelled` rows on one card. So `needs you` is not a state this
  board *holds*, it is a state it *re-enters* every five seconds, with a sub-second seam at each
  restart where the newest run is cancelled and the tile says nothing. Every predicate about a
  run mark in the scenario is therefore an `await`, never an `assert` — an `assert` landing in
  that seam is a red run about the harness's patience rather than about the board. The scenario
  covers `working`, `needs you`, `pending`, `stalled`, `blocked:n` and `action`; it cannot cover
  `done` (suppressed by `on_success` on both action columns) and it cannot cover a *second*
  live run, because `maxLiveRuns` is 1 and the first card's restart wins the freed slot every
  time — which is why In review is pinned by its `action` mark rather than by a run of its own.
- 2026-09-21 (P6-T05, scenario) — **no baseline is recorded for `workflow-marks`'s shot, and it
  is a deliberate refusal rather than an omission.** The image was opened (§9.8) and is right:
  AGE-1's amber dot, AGE-2's spinner, `⊘ 1` and `⊘ 2` in Todo, the bolt on In Progress and In
  review and on no other column, and `2/1 WORKING` in the pane header. But it is not
  *reproducible*: the restart loop above puts a toast stack and a "N failed" counter into the
  chrome whose size depends on how many restarts happened before the shot, which is orders of
  magnitude outside §6's 0.2%-of-pixels budget. A golden recorded from it would fail the next
  run for a reason that has nothing to do with the board face, and §6 is explicit that pixels
  are regression evidence and the structured assertions are the oracle. Recording one becomes
  the right call the moment a failing action stops being restarted for ever.
- 2026-09-21 (P6-T05, scenario) — the virtual lane needed a compositor this machine's agent
  session does not have (`WAYLAND_DISPLAY` unset). The run used the throwaway nested Hyprland
  §11 names as the workaround, addressed by its own instance signature so the developer's real
  session was never driven. `make harness-one` picks it up from
  `HYPRLAND_INSTANCE_SIGNATURE`/`WAYLAND_DISPLAY`; with neither set the lane falls back to
  headless and the `shot` fails, which is the documented behaviour and not this file's bug.
- 2026-09-21 (P6-T05, fixture) — the board's keys are `AGE-1…AGE-4`, not the `FLT-…` the plan's
  step list guessed: the prefix is `worktree_prefix("agent")`, the daemon's own derivation from
  the slug, and the scenario has to assert those. The worktree is `acme/api#agent`.
- 2026-09-21 (P6-T05, fixture) — the preset board is seeded with `workflow_preset()` *as* its
  statuses rather than by running `apply_workflow_preset` over the shipped five. That function
  never rewrites a column a board already has, so on a fresh board it adds Ready and In review
  around the existing In Progress and leaves it running nothing — the case the CLI's
  `preset_notes` prints a warning for. A fixture whose point is a card that runs cannot ship
  that board. The list is still `fleet-core`'s, so no column, action or route is written in the
  harness. If phase 8 gives the settings dialog a preset button that adopts the action for a
  column the board already had, this replace can become an apply again.
- 2026-09-21 (P6-T05, fixture) — no new transcript was needed: `subagent-child.json` and
  `subagent-child-blocked.json` already exist and `agent::launcher` already serves them to
  whichever provider runs with `FLEET_DELEGATION` set (commit 0b76217). A card run *is* a
  delegation, so Codex completes its card and Claude parks one on `needs you` with no
  preset-specific wiring.
- 2026-09-21 (P6-T03) — `build` takes the view layer's own `BoardMarks`/`TileMark` (kit types
  only), not `state::board::CardMarks`/`TileMarks`: `state.rs` declares `mod board;` privately
  and has not yet re-exported the two names, so `views/board_screen` cannot name them, and an
  owner of P6-T03 may not edit `state.rs`. `screens/board/projection.rs::marks` restates the
  fold field by field inside the rebuild branch — never per frame. Once `state.rs:47` carries
  the re-export the integrator may collapse the pair into one type; nothing else changes.
- 2026-09-21 (P6-T03) — `ColumnRows::has_action` is `automation.on_enter.is_some()` alone.
  `on_success` and `advance_when_unblocked` move a card the column is already finished with,
  and a `⚡` promising a run for one of those would lie; pinned by
  `only_an_on_enter_column_carries_an_action`.
- 2026-09-21 (P6-T03) — the header's two counts are composed in `HeaderFacts::of`
  (`working_label`, `needs_you_label`), not in `fn header`, so the render body only places
  strings. `live_limit` is read from the view's own `settings.max_live_runs()` rather than
  carried through the marks, which keeps the denominator a property of the board.
- 2026-09-21 (P6-T04, harness) — a `board.cards` row puts the run mark first, the blocked count
  second and the assignee it already carried last. The plan said "keep the assignee mark" and
  left the order open; leading with the assignee would have made `marks[0]` mean one thing on an
  assigned card and another on an unassigned one, which is exactly the assertion the scenarios
  are written against. No scenario asserted a card's `marks` before this, so nothing moved under
  one.
- 2026-09-21 (P6-T04, harness) — `board.summary`'s label joins the header's two labels with
  ` · `. The header draws them as two `Text`s in a flex gap and has no separator of its own; a
  snapshot row is one string, and §5.6 fixes the text as `1/1 working · 1 needs you`. The two
  halves are the same sentences `HeaderFacts` composes, and the row is absent — not `0/1
  working` — while both counts are zero.

## Follow-ups
- ~~**`scenarios/board/workflow-marks.scenario` (P6-T05's other half) is still open.**~~ Written,
  green in the virtual lane, and skipped by the headless selector for its `shot` alone.
- ~~**A column action whose run fails is restarted immediately and for ever, and nobody owns the
  policy.**~~ **Fixed 2026-09-21 (review fix F1).** The answer of the three below is the third:
  a terminal run leaves the card for a person. A column runs its action *on entry*, and a run
  that ended where it started leaves a card that entered nothing, so `on_run_delivered` now
  seeds the evaluation through `fleet_core::board::re_evaluate_settled` — rule 1 skipped for the
  seed, rule 2 kept — unless the outcome's `on_success` actually moved the card, which is an
  entry like any other. `>` (`start_run`) stays the one verb that runs a card standing still.
  `docs/BOARD.md` §11.7 now states it, and it is pinned by
  `board::automation::tests::{a_settled_seed_whose_run_ended_in_this_column_starts_nothing,
  no_outcome_restarts_a_card_that_ended_where_it_started}` in `fleet-core` and
  `services::boards::tests::triggers::a_terminal_run_does_not_start_the_column_it_ended_in` in
  the daemon. `scenarios/board/workflow-marks.scenario` keeps `working`, `needs you`, `pending`
  and `2/1 working` — all four are readable inside one twenty-second park now that
  `GATE_BUDGET` is twenty seconds (the follow-up below) — and ends where the board now ends:
  AGE-1's gate expires, its run settles as **cancelled**, its tile draws nothing, *nothing
  restarts it*, and the slot it was holding goes to the card that had been waiting, which runs
  and is carried into In review. Only `stalled` is gone: it is `pending` plus sixty seconds, and
  the wait it needs ends after twenty — that state existed in this corpus *only* because the
  restart loop held the slot for ever. It stays pinned by
  `state::board::tests::an_owed_run_turns_amber_once_the_wait_is_the_story`, which drives the
  clock it is derived from. `workflow-chain` and `workflow-detail` needed no line changed — only
  their comments, which described the loop as a fact of the board. The original report follows.

  **A column action whose run fails is restarted immediately and for ever, and nobody owns the
  policy.** `on_run_delivered` re-evaluates, `start_on_entry` sees an action column with no live
  run and starts the card again; nothing in `fleet-core`'s engine remembers that the previous
  attempt in *this* column just failed. `docs/BOARD.md`'s outcome table says a `Failed` or
  `Cancelled` run means "nothing moves" and is silent on whether it means "and it runs again",
  so this is a contract gap rather than a bug with an obvious fix — a backoff, an attempt cap
  per column visit, and "leave it until a person moves it" are three defensible answers and the
  choice is not one to make inside a scenario task. Measured on `board-workflow`: one card,
  17 restarts and 17 discarded child processes in 90 seconds, each one a board write and a
  delegation record. It is also the one thing standing between `workflow-marks` and a
  recordable baseline. **Owner: whoever owns the automation engine (phase 3).**
- ~~**The harness's five-second `GATE_BUDGET` is what makes a parked child a five-second state.**~~
  **Fixed 2026-09-21 (review fix F1's fallout), exactly as this entry predicted.** With the
  restart loop gone the parked state is no longer re-created every six seconds, and five seconds
  turned out to be shorter than the app's own settle time on a loaded machine —
  `workflow-chain` recorded `working` 4.95 s after the move, leaving no window at all for the
  `needs you` the next line waits on. `agent::GATE_BUDGET` is now **twenty** seconds, four times
  the default await rather than equal to it, and `docs/TESTING-HARNESS.md` §5 states it and says
  what it is the ceiling on. A gate a scenario answers is answered in milliseconds, so the
  number only ever bounds the case where nobody answers.
  The original entry: `docs/TESTING-HARNESS.md` §4 says the preset's Claude child "asks a
  question and parks it"; §5's player gives up on the gate after `agent::GATE_BUDGET` and exits
  1. The scenario is written around that rather than against it, but the two sentences do not
  agree, and if the restart loop above is ever capped, the parked state will also need to
  outlive five seconds for a scenario to photograph it.
