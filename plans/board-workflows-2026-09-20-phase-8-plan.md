# Board workflows, phase 8: app — Board settings becomes rail and pane — Plan
> Tracker: ./board-workflows-2026-09-20-phase-8-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Board settings stops being a 560 px row list and becomes the rail-and-pane shape the global
Settings dialog draws: 720 × 560 with a 180 px rail carrying General, Backend and Columns. Columns
is the first place in the app where a column can be added, renamed, reordered or deleted; `⏎`
drills from the list into one column's rows in the same pane and `esc` goes back, while `n`, `d`,
`J`/`K` and `P` act on the list, `P` applying the workflow preset by id. The seven automation rows
exist only while On enter is not none, and on a context or Jira board they are disabled under the
trailer `Automation is available on worktree boards`, leaving order and names editable. `ctrl-s`
sends one `UpdateBoard` with the whole `statuses` vector; `esc` with unsaved edits asks once; a
refusal keeps the dialog open with the daemon's sentence.

## Sizing call

**Phased, phase 8 of 9.** See ./board-workflows-2026-09-20-roadmap.md. One pull request, depending
only on phase 1's model and running in parallel with phases 6 and 7. Not split further: the rail,
the Columns pane and the save path are one dialog, and half of it would ship a section that cannot
be saved. Phase 9 adds the palette command for the key this phase binds.

## Repository context

- Board settings today: `dialogs/board_settings.rs` (`LABEL_WIDTH` :38), `board_settings/view.rs`
  rows :48-135 (Name :56 … Backend :126, schema :132-135), `Dialog::new("Board settings")` + hint +
  `primary("⏎ Save")` :137-148, error :149-151, actions :154-215, `backend_element` :244-312;
  `schema.rs` `SettingRow` :203, `FIXED_ROWS` :223-231, `BackendRow` :9-27; `draft.rs`; `tests.rs`.
- **`save`** `board_settings/persistence.rs:38-137`: `saving` :44, board-changed :47-56,
  `validate` :57-61, `BoardPatch` :71-98, `UpdateBoard` :103-106, verbatim daemon error :108-118.
- Shape to copy: `dialogs/settings.rs` (`RAIL_WIDTH` :25, `LABEL_WIDTH` :29), `settings/schema.rs`
  `Section` :7-26, `ALL` :29-38, `title()` :43-54, dispatch :176-184; **rail and pane**
  `settings/view.rs:21-61` (rail :21-32, pane :34-52, divider :54-61), `Dialog::new("Settings")`
  :63-66, hints :67-79, dirty title :80-83, error :84-86; `cycle_section` `settings/draft.rs:622`,
  `current_section()` :44-49, `confirm_opens_editing` `settings/view.rs:99-104`.
- Widths `dialogs/mod.rs:127-133`, heights :146-153, `WIDE_W = px(720.0)` :48,
  `SETTINGS_H = px(560.0)` :55.
- Keys: `Dialog > BoardSettings` :521-525, `Dialog > BoardSettingsEditing` `enter` :526;
  `Dialog > Settings` :1068; board contexts :443-469 and :474-499 (`C` free on both, research E §3);
  actions `actions.rs:732-787`; handler `shell/root/board.rs` `settings` :422, registration
  `shell/root/actions.rs:268-302`.
- Core: `workflow_preset()`/`apply_workflow_preset()` (§1.9), `max_live_runs` (§1.2), the refusal
  sentences (§1.7).
- Skills: `gpui-app-shell` (dialog, sections, keys), `gpui-components` (pane list and rows),
  `gpui-styling` (rail, divider, disabled tone), `rust-gpui-testing` with `TESTING-HARNESS.md`
  §3/§9, `zed-quality-review` last. Docs in step: `UX-SPEC.md` :2100-2123 and §3.8.6 :1338-1382,
  `KEYMAP.md`, `BOARD.md` §11.

## Assumptions

- **Columns is a draft:** every list key and row edit mutates a `Vec<Status>`; nothing is sent
  until `ctrl-s`, and validation runs before the request.
- **`C` is a shortcut, not a dialog:** `board::Columns` opens this dialog on Columns; phase 9 adds
  the palette command.
- **A context or Jira board keeps everything but automation**: order, names, categories and colours
  stay editable and the automation rows are disabled, never hidden.

## Out of scope

- The palette command (9); the face and card detail (6, 7); any daemon-side validation change.

## Affected areas

- `crates/fleet-app/src/dialogs/board_settings.rs` and `board_settings/{view.rs, draft.rs,
  schema.rs, persistence.rs, tests.rs}`, `dialogs/mod.rs`, `actions.rs`, `keymap.rs`,
  `shell/root/{board.rs, actions.rs}`, `state/harness/projection.rs`;
  `scenarios/board/workflow-columns*.scenario`; `docs/{UX-SPEC.md, KEYMAP.md, TESTING-HARNESS.md,
  BOARD.md}`.

## Tasks

### P8-T01 — The rail: `BoardSection`, 720 × 560, General gains `Max live runs`
- **Intent / touches:** The dialog's new frame — `dialogs/mod.rs`, `board_settings/{schema.rs,
  draft.rs, view.rs}`.
- **Steps:**
  - `Dialogs::BoardSettings` takes `WIDE_W` (:48) and `SETTINGS_H` (:55) in `dialogs/mod.rs:127-153`;
    no new constant. `BoardSection { General, Backend, Columns }` in `board_settings/schema.rs` with
    `ALL`, `title()` and a row dispatch mirroring `settings/schema.rs:7-54`; the draft gains
    `section` and `cycle_section`, copied from `settings/draft.rs:622`.
  - Rebuild `board_settings/view.rs:48-148` as rail + `Divider::vertical()` + pane, following
    `settings/view.rs:21-66`: General keeps Name, Prefix, DefaultRepo, StartOnWorktree,
    PushNewCards, ConflictPolicy and gains **`Max live runs`** with the hint `runs share one
    checkout`; Backend keeps `backend_element` (:244-312) and the schema rows.
  - Keymap: `Dialog > BoardSettings` gains the rail keys the global dialog uses, matched exactly,
    plus `ctrl-s`; `BoardSettingsEditing` keeps `enter`. `KEYMAP.md` rides in P8-T06.
- **Verification:** `cargo test -p fleet-app board_settings`, `make lint`.
- **Done when:** The dialog opens at 720 × 560 with three sections and every existing row saves.

### P8-T02 — The Columns pane: list, drill-in, `n` `d` `J`/`K` `P`
- **Intent / touches:** The section that did not exist — `board_settings/{schema.rs, draft.rs,
  view.rs}`.
- **Steps:**
  - List mode: the board's statuses in order, one row each, a muted `⚡` when the column has an
    action, the hint row `n new · d delete · J/K reorder · P preset · ⏎ open`.
  - `n` appends a column with a generated id and an editable name; `d` deletes (see P8-T04 for
    cards); `J`/`K` move the cursor row down/up in the draft; `P` calls
    `fleet_core::board::defaults::apply_workflow_preset` on the draft, adding missing columns by id
    only and reporting when it changed nothing.
  - `⏎` drills in: breadcrumb `Columns › {name}`, rows Name, Category, On enter, then — only while
    On enter is not none — Provider, Model, Effort, Mode, Instructions, Expect, Env, then On success
    and When unblocked. `esc` returns to the list; `esc` from the list closes under P8-T04's dirty
    rule. Values read as the canvas does: `run card` / `run skill deep-review`
    (`ActionKind::word()`), `→ In review`, `off`, `—` for empty Env, instructions cut to one line.
  - Tests: the list order matches the board; `P` is idempotent; `J`/`K` reorder the draft only; the
    seven rows appear and disappear with On enter.
- **Verification:** `cargo test -p fleet-app board_settings`, `make lint`.
- **Done when:** Drilling in and back never leaves the cursor outside the pane, and nothing is sent.

### P8-T03 — Editing a column row, the multiline `Instructions`, and the disabled boards
- **Intent / touches:** `board_settings/{view.rs, draft.rs}`.
- **Steps:**
  - `⏎` on a drill-in row opens editing in `Dialog > BoardSettingsEditing`: Name, Expect, Model and
    Effort are single-line inputs; Category, On enter, Provider, Mode, On success and When unblocked
    cycle over the legal values (`on_enter` = `none | prompt | skill:<name>`); Env is a
    line-per-entry `KEY=VALUE` editor; **Instructions** opens the shared `TextInput` in
    `InputMode::Multiline`, the mode the card description uses (`card_detail/draft.rs:112`).
  - On a context or Jira board every automation row (On enter and its seven, On success, When
    unblocked) renders `locked`/disabled with the trailer `Automation is available on worktree
    boards` under the list; Name, Category, Colour and order stay editable.
  - Nothing in `render` parses or validates: row values and disabled flags are prepared when the
    draft changes. Tests: each editable row round-trips into the draft; a Jira board disables
    exactly the automation rows; the trailer appears once.
- **Verification:** `cargo test -p fleet-app board_settings`, `make lint`.
- **Done when:** Every canvas row is reachable by keyboard and no automation row is editable on a
  board that cannot carry it.

### P8-T04 — Validate, save with `ctrl-s`, ask once on `esc`, move cards before a delete
- **Intent / touches:** `board_settings/{persistence.rs, view.rs, draft.rs}`.
- **Steps:**
  - Validate before any request, with the §1.7 sentences on the error line: a routing target must
    exist, may not be its own column and must be later; a skill action needs a name;
    `max_live_runs` is 1 to 8; an `env` entry passes the five refusals.
  - `ctrl-s` extends `save` (`persistence.rs:38-137`) to put the whole `statuses` vector and the
    settings into one `BoardPatch` (:71-98) and one `UpdateBoard` (:103-106), keeping the `saving`
    guard (:44) and the board-changed check (:47-56); a refusal keeps the dialog open and prints
    the daemon's sentence verbatim (:108-118).
  - Deleting a column that holds cards requires a target chosen in the dialog: one `MoveCard` per
    card first, then the patch; a refused move stops the sequence, keeps the dialog open with that
    sentence and leaves the column in the draft and the already-moved cards where they are (contracts §5.4). `esc` with a dirty draft asks once
    through the dialog's existing confirm affordance; clean `esc` closes as today.
  - Tests: each refusal sentence; one request per save; a delete with cards sends the moves first;
    a refused move stops the sequence.
- **Verification:** `cargo test -p fleet-app board_settings`, `make lint`.
- **Done when:** A save is one `UpdateBoard` and no partial write happens silently.

### P8-T05 — `C`, the harness `dialog` section, `settings.columns`, two scenarios
- **Intent / touches:** `actions.rs`, `keymap.rs`, `shell/root/{board.rs, actions.rs}`,
  `state/harness/projection.rs`, `scenarios/board/workflow-columns.scenario` and its variant.
- **Steps:**
  - Action `board::Columns` in `actions.rs:732-787`, handler beside `settings`
    (`shell/root/board.rs:422`) opening the dialog on Columns, registered at
    `shell/root/actions.rs:268-302`, bound to `C` on both board contexts (free on both, research
    E §3). The palette command is phase 9's.
  - Harness: the `dialog` snapshot gains a `section` entry and the list `settings.columns` (one row
    per column, `label` = the name, `marks` `action` and `disabled`), documented in §3, §2 intact.
  - `workflow-columns.scenario` on the `board-workflow` fixture: `C`, assert `dialog.name ==
    "Board settings"` and the section is Columns, `P`, drill into the review column, change Effort,
    `ctrl-s`, reopen, assert it survived. The variant opens the same dialog on the context board and
    asserts the automation rows are marked `disabled` while the name row is not. No `shot`.
- **Verification / done when:** `make harness-one SCENARIO=…` each, `LANE=headless`, then
  `make harness`; both pass in both lanes and `C` reaches Columns from either board.

### P8-T06 — `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/BOARD.md`
- **Intent / touches:** Documents in step, same commit — `docs/UX-SPEC.md` :2100-2123 and §3.8.6
  :1338-1382, `docs/KEYMAP.md`, `docs/BOARD.md` §11.
- **Steps:**
  - Rewrite the Board settings block in "The other three dialogs": the new size, the rail, the
    three sections, the Columns list and drill-in, the list keys, the save and dirty rules, the
    disabled-automation trailer; §3.8.6 gains a cross-reference to the shared shape.
  - `KEYMAP.md`: `C` on both board contexts and the two dialog contexts' rows, matching
    `keymap::table()` exactly; `BOARD.md` §11 gains "Configuring columns in the app".
- **Verification / done when:** `git diff docs/` beside the scenarios; every assertion has a
  sentence and `KEYMAP.md` matches the table.

## Verification

```sh
make lint && make test && make harness
```

`make harness` is required: this phase changes a dialog and the keymap. Run the two
`scenarios/board/workflow-columns*` scenarios first, then the corpus.

## Definition of done

- [ ] Every task above is `[x]` in the tracker; `make lint` and `cargo check --workspace
      --all-targets` are clean; `make test` passes.
- [ ] `make harness` passes with both new scenarios in both lanes; `TESTING-HARNESS.md` changed
      only under §3's additive rule and `UX-SPEC.md`, `KEYMAP.md`, `BOARD.md` describe what runs.
- [ ] One `UpdateBoard` per save; no row values computed in `render`; no new dialog variant, key
      context or width constant.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **The dialog is already large.** A rail and a drill-in mode will push `board_settings/view.rs`
  past the file-size rule in `rust-workspace-architecture`; split it into
  `view/{general.rs, backend.rs, columns.rs}` when it does, not after.
- **Two `esc` meanings.** In the drill-in, `esc` goes back; in the list it closes. Get the order
  right in one handler and test both, or a user loses edits.
- **Rollback.** Revert the PR: Board settings returns to the 560 px list and the CLI's `columns`
  family is again the only way to edit a column.

## Contract resolutions

Resolved in the contracts file on 2026-09-20; the contracts wording wins over any earlier phrasing above.

1. `⏎` drills into a column from the list and opens a row's editor inside a column; the existing `BoardSettingsEditing` `enter` commits an edit; `ctrl-s` saves and the primary button reads `^s Save`; `esc` goes back a level and asks once at the top level when dirty (contracts §5.4).
2. A refusal mid-way through the delete's card moves stops there, keeps the column, shows the daemon's sentence, and leaves the already-moved cards where they are (contracts §5.4).
3. The harness `dialog` gains a `fields` entry named `section`, no new `DialogSnapshot` key; `,` opens on the section last used in this app session (General first), `C` on Columns (contracts §5.4, §5.6).
