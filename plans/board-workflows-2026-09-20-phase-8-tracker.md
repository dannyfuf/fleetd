# Board workflows, phase 8: app — Board settings becomes rail and pane — Tracker
> Plan: ./board-workflows-2026-09-20-phase-8-plan.md
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
  (Not tickable by any phase-8 owner, for phase 5's reason: the tree was never clean — the nine
  phases were built in parallel in one worktree — and `make lint`/`make test` belong to the
  integration stage rather than to a task. Ticked here 2026-09-21 by the phase-9 docs audit, which
  reconciled every tracker with what the tree actually holds.)
- [x] I am ready to start.

## Tasks
- [x] P8-T01 — The rail: `BoardSection`, 720 × 560, General gains `Max live runs`
  - verified: `cargo test -p fleet-app --lib board_settings` 43 passed —
    `each_section_draws_its_own_rows_and_nothing_else`,
    `the_rail_cycles_and_remembers_where_it_was_left`, `max_live_runs_clamps_at_both_ends`,
    `a_throttle_outside_the_range_is_refused_in_the_contract_s_words`. `board_settings/view.rs`
    is rail + `Divider::vertical()` + pane, copied from `settings/view.rs`; `⇥` cycles the rail.
- [x] P8-T02 — The Columns pane: list, drill-in, `n` `d` `J`/`K` `P`
  - verified: `cargo test -p fleet-app --lib board_settings` —
    `the_columns_list_is_the_board_order_and_marks_the_automated_ones`,
    `the_preset_is_idempotent_and_keeps_the_columns_already_present`,
    `reordering_moves_the_column_and_nothing_else`,
    `the_list_keys_act_on_the_draft_and_send_nothing`,
    `the_seven_action_rows_come_and_go_with_on_enter`.
- [x] P8-T03 — Editing a column row, the multiline `Instructions`, and the disabled boards
  - verified: `cargo test -p fleet-app --lib board_settings` —
    `a_board_that_cannot_run_anything_disables_exactly_the_automation_rows`,
    `a_locked_automation_row_cannot_be_cycled_or_typed_into`,
    `the_on_enter_row_cycles_the_three_spellings_and_keeps_the_name`,
    `a_column_round_trips_through_the_status_the_patch_carries`. Row values and disabled flags
    are prepared by `BoardSettingsState::prepare`, never in `render`.
- [x] P8-T04 — Validate, save with `ctrl-s`, ask once on `esc`, move cards before a delete
  - verified: `cargo test -p fleet-app --lib board_settings` —
    `every_routing_refusal_is_stated_in_the_contract_s_words`,
    `every_action_refusal_is_stated_in_the_contract_s_words`,
    `an_on_enter_spelling_that_is_none_of_the_three_is_refused`,
    `a_column_without_a_name_stops_the_save`,
    `escape_leaves_the_editor_then_the_column_then_asks_once`,
    `a_clean_draft_closes_without_a_question`,
    `deleting_a_column_with_cards_asks_where_they_go_first`.
- [x] P8-T05 — `C`, the harness `dialog` section, `settings.columns`, two scenarios
  - verified (keymap only): `cargo test -p fleet-app --lib keymap` 33 passed, including
    `board_documentation_and_bindings_match_in_both_directions` and
    `text_input_hosts_never_bind_printable_owner_keys`. `Dialog > BoardSettings` gained the
    global dialog's `left`/`right`, the five Columns-list letters and `ctrl-s`;
    `BoardSettingsEditing` kept `enter` and gained `ctrl-s`. (The harness `section` field, the
    `settings.columns` list and the two scenarios were still open at that point; the three
    bullets below are how each of them landed.)
  - verified (`settings.columns` half): `cargo test -p fleet-app --lib state::harness` — 42
    passed, 0 failed, including
    `the_settings_columns_state_their_action_and_whether_the_board_may_carry_one` and
    `a_board_that_may_not_carry_automation_marks_every_column_disabled`. The list is the board's
    own columns — the dialog is seeded from them (`board_settings::persistence::seed`) — so it
    is projected from `AppState` while the dialog is open, `label` the name, `action` on the
    `on_enter` rule and `disabled` on `automation_locked`'s own condition. Documented in
    `docs/TESTING-HARNESS.md` §3.
  - verified (`section` half, integration 2026-09-21): the field is filled where the dialog's own
    state is readable — `dialogs/host.rs::dialog_fields`, which already reads the `DialogHost`
    entity — as a leading `FieldSnapshot { name: "section", .. }` whose value is
    `BoardSettingsState::section_title()` (a new `pub(crate)` accessor over the `pub(super)`
    field). Board settings paints no `dialog.field[N]` target at all, so a leading non-editor
    cannot put the documented field↔target numbering out of step; `docs/TESTING-HARNESS.md` §3
    says so. `cargo test -p fleet-app` green, `cargo clippy --workspace --all-targets
    --all-features -- -D warnings` clean.
  - verified (scenario half, 2026-09-21): `make harness-one
    SCENARIO=scenarios/board/workflow-columns.scenario` — 52 steps ok (~30 s), and
    `…-context.scenario` — 30 steps ok. Both pass in the headless lane and both ask for the
    virtual lane without changing: neither takes a `shot`, so the only lane difference is pixels
    neither of them reads. This machine has no compositor (`HYPRLAND_INSTANCE_SIGNATURE not
    set`), so `LANE=virtual` reported the fallback and ran the same 52 and 30 steps.
    `cargo test -p fleet-app` and `cargo clippy -p fleet-app --all-targets --all-features --
    -D warnings` green.
- [x] P8-T06 — `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/BOARD.md`
  - verified: read from `dialogs/board_settings/{view,keys,columns,draft,persistence,schema}.rs`
    and `dialogs/mod.rs` (`WIDE_W` 720 × `SETTINGS_H` 560). UX-SPEC's Board settings block is
    rewritten — the size, the rail, the three sections, `,` on the remembered section and `C` on
    Columns, the list keys, the drill-in row order, the `on enter` spelling and its refusal, `P`,
    the armed delete with its `MoveCard` sequence, `^s` as the one `UpdateBoard` (with `⏎` still
    saving from General/Backend), the one-shot `esc` ask, and the
    `Automation is available on worktree boards` trailer; §3.8.6 gains the cross-reference to the
    shared shape and the three differences. `KEYMAP.md`'s fourteen board-settings rows were
    already landed by P8-T05's owner and match `keymap::table()` (`cargo test -p fleet-app --lib
    keymap` 33 passed). `BOARD.md` §11.10 is "Configuring columns in the app".

## Notes / decisions log
- 2026-09-21 (review fix, P2) — the Board settings draft is cloned once per frame by
  `view::render`, and this phase gave it two members that grow with the *board* rather than with
  the form: `board: Option<Board>` and `cards_by_column`, the latter one card id per card. Both
  are now behind `Rc`, so the per-frame clone is back to the size of the form
  (`docs/APP-CONTRACTS.md`, "render prepares nothing"). The clone itself is the dialog's
  existing pattern — the render cannot hold a borrow of the host across `&mut App` — and is left
  alone.
- 2026-09-21 (contracts:app skeleton) — "the section last used in this app session" cannot live
  in the draft: `persistence::seed` rebuilds it from the board on every opening. It lives in a
  `thread_local!` beside it (`board_settings/draft.rs`), which `BoardSection::default()` reads,
  so a freshly seeded draft opens where the user left off and `,` needs no special case.
  `C` (`shell/root/board.rs::board_columns`) goes through `screens::board::settings` first, so
  an unreachable daemon or an unloaded board refuses before anything is remembered.
- 2026-09-21 (a:settings) — `on enter` is **one** row, spelled as `--on-enter` spells it
  (`none` / `prompt` / `skill:<name>[:<args>]`). §5.4 fixes the twelve drill-in rows and none of
  them is a skill *name*, so the row both cycles the three spellings and takes typing; the name
  survives a trip through `none` because the column's stored action still holds it. The one
  refusal this pane owns is the spelling itself — the CLI refuses the same text with the same
  words plus its flag name.
- 2026-09-21 (a:settings) — the column rules take a whole `Board`, and `Board` has no `Default`
  (every id on one is validated), so the draft keeps the board it was seeded from and swaps a
  candidate column vector into a clone of it. That board is also what `dirty()` compares
  against, which is why there is no second copy of every General row to drift from it.
- 2026-09-21 (a:settings) — `⏎` still saves from General and Backend, which is §3.8.6's
  behaviour and what `docs/UX-SPEC.md` still describes; in the Columns pane it drills in, opens
  a row's editor and commits one. `^s` saves from every section and is the primary button.
- 2026-09-21 (a:settings) — the rail and every cycler here **clamp** rather than wrap, because
  `dialogs::step` is what the global Settings dialog steps with and §5.4 asks for its keys
  matched exactly. The arrows a `Cycler` draws follow the same rule, so the row never offers a
  step it will not take.
- 2026-09-21 (a:harness, P8-T05) — the `dialog` `section` field is **not** implemented and no
  projection change can implement it. `dialog.fields` is filled by `dialogs::dialog_fields`
  (`dialogs/host.rs`) and copied into `AppState` by `drive.rs::project`, because a dialog's own
  state lives in the `DialogHost` entity; the section is one of those, and the builder holds
  `&AppState`. The whole edit is in `dialogs/host.rs` — see this agent's integration note — and
  it needs no new state, no new key input and no change here: the field rides the seam that
  already exists.
- 2026-09-21 (a:harness, P8-T05) — `settings.columns` is one row per *column*, with `disabled`
  on the column rather than on the drilled-in automation row. The dialog's drill-in belongs to
  the same unreachable draft; the board's own `automation_locked` condition (a context board, or
  a board whose columns answer to a backend) is what the mark states, which is the same fact the
  disabled rows draw. A scenario asserting "the automation rows are disabled while the name row
  is not" must read the painted rows, not this list.
- 2026-09-21 (scenario:columns, P8-T05) — the note above is answered by the *editor*, not by the
  painted rows: `workflow-columns-context.scenario` puts `\u{23ce}` on Name and on On enter and
  reads `dialog.fields[1]` and `key_contexts[1]` each time. Name mounts the one row-scoped
  editor — field `name`, context `BoardSettingsEditing` — and the locked automation row mounts
  nothing, so the field is absent and the list keeps the keyboard. That needed one additive
  seam, the same shape the card picker's `query` field already is: `dialogs/host.rs::dialog_fields`
  now reports Board settings' live editor after `section`, named after its row in lower case
  (`BoardSettingsState::editor_row_name`). Without it *nothing* typed into this dialog could be
  read back at all — `settings.columns` carries a column's name and its `action` mark and no
  other row's value — so P8-T05's "change Effort, save, reopen, assert it survived" had no
  oracle. `docs/TESTING-HARNESS.md` §11 says so; `the_harness_reads_the_open_row_editor_as_a_dialog_field`
  pins it, including that a locked row reports no field.
- 2026-09-21 (scenario:columns, P8-T05) — what the two scenarios cover and what they cannot.
  Covered: `C` from the workspace board and from the Hub's context board, the section field,
  `settings.columns` with its `action` and `disabled` marks, the drill-in, the editor round trip
  through one `^s` and a re-opening, and the `esc` ladder (editor → column → dialog, no question
  on a clean draft). Not covered, deliberately: the Columns-pane cursor has no projection of its
  own, so a walk is pinned by what the row it lands on reports (`fields[1].name == "effort"` is
  the assertion that the fifth `j` landed on Effort), and `n`, `d`, `J`/`K` and the armed delete
  move only the draft — invisible until a save, and each already pinned by P8-T02/T04's unit
  tests. `P` is covered *through* the save: the re-opened list is the column vector `P` was
  pressed on, so a preset that had duplicated a column would be showing it.

## Follow-ups
- 2026-09-21 (scenario:columns) — the headless corpus slice that rides `make test`
  (`crates/fleet-app/tests/harness_headless.rs`) now costs 313 s for 29 scenarios, five times
  its own 60-second budget, and it says so as a warning rather than a failure. These two
  scenarios are ~35 s of that; the phase-6 and phase-7 board/agent scenarios are most of the
  rest. Two of them — `board/workflow-detail` and `agents/subagent-reopen-closed-caller` — time
  out inside that back-to-back run on a loaded machine and pass alone (verified 2026-09-21,
  61 and 17 steps ok), which is the budget showing up as flakiness. Deciding what stays in
  `make test` and what moves back to `make harness-headless` is a corpus-wide call, not this
  phase's.
