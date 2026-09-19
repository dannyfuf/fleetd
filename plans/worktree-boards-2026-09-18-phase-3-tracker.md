# Worktree-scoped boards — Phase 3: one text input for the whole app — Tracker
> Plan: ./worktree-boards-2026-09-18-phase-3-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline (`make lint && make test && make harness`).
  - verified 2026-09-19 on `ffa76fa`: `make lint` clean; `make test` 79 suites, 3246 passed, 0 failed;
    `make harness` (virtual lane, nested Hyprland) passed 23 scenarios then stopped at
    `agents/unread-mark.scenario` with a capture error ("the harness window is not in" the output)
    while a concurrent cargo build held the lock; treated as an environment flake, re-run before P3-T04.
- [x] I have read `docs/DESIGN-SYSTEM.md` §6.4 and §6.6 and `docs/KEYMAP.md` "Dialogs and text inputs" as they are today.
- [x] I am ready to start.

## Tasks
- [x] P3-T01 — Build the single editing engine
  - verified: `cargo test -p fleet-ui-kit input` (76 passed: 37 engine tests, 16 of them ported
    `MultilineBuffer` parity cases, plus the pre-existing input tests) and `make lint` passed.
    `multiline_input/buffer.rs` is untouched until P3-T04.
- [x] P3-T02 — Build the live `TextInput` component
  - verified: `cargo test -p fleet-ui-kit` (392 passed, including 13 gpui interaction tests in
    `input/live_tests.rs`), `cargo test -p fleet-lazygit` (135 passed), `cargo build --example
    gallery_input`, `make lint` passed. The old live input is exported as `LegacyTextInput` until P3-T07.
- [x] P3-T03 — Replace the editing rows in the keymap with one action family
  - verified: `cargo test -p fleet-app` (876 passed), `make lint`, and `make harness` (59 of 59
    scenarios, virtual lane) passed. 46 `FleetTextInput` rows; the old `filter::`/`palette::`/
    `dialog::` families stay until their surfaces migrate (see the 2026-09-19 scope note).
- [x] P3-T04 — Re-base the agent composer on the shared component
  - verified: `cargo test -p fleet-ui-kit` (382 passed), `cargo test -p fleet-app` (876 passed,
    110 of them agent_thread), `make lint`, and `make harness` (59 of 59, every `agents/*`
    scenario) passed. `multiline_input/{buffer,element,platform}.rs` deleted; `FleetTextInput`
    rows are now 49 (`home`/`end` → row actions, `ctrl-shift-a/e`, `shift-enter`, owner-aware `enter`).
- [x] P3-T05 — Migrate the board dialogs
  - verified: `cargo test -p fleet-ui-kit` (383 passed), `cargo test -p fleet-app` (879 passed),
    `make lint`, and `make harness` (59 of 59) passed. The first harness run caught a real
    regression the unit tests missed: the board filter input lost gpui focus to the shell's
    focus reconciliation, so typed text never reached it; fixed by a shell-level `wanted_input`
    gate plus root-level regression tests that dispatch `/` and type through the real path.
- [~] P3-T06 — Migrate the remaining dialogs, filters and the lazygit overlay
  - split into two sequential runs: (a) Hub filter, palette, kit `FilterBar`/`Palette`, lazygit
    overlay; (b) create worktree, clone, context, rename, hooks, settings, `NumberField`, delete
    `dialogs/input.rs` and the old action families.
  - (a) verified: `cargo test -p fleet-ui-kit` (383), `cargo test -p fleet-lazygit` (135),
    `cargo test -p fleet-app` (874 lib, 0 failed), `make lint`, `make harness` 59 of 59 (agent run
    and an independent orchestrator run). `filter::`/`palette::` editing families deleted.
    `Overlay::Filter` no longer mounts an overlay layer: the editor lives in the pane header, so
    `Filter` wraps the Hub body (APP-CONTRACTS §3). Fixed en route: Hub filter `ctrl-n`/`ctrl-p`
    never moved the selection anchor. `TextInput::set_embedded` is public for 30/44 px rows.
- [ ] P3-T07 — Delete the old input families and record the decision
- [ ] P3-T08 — Drive typing, selection and undo in the harness
- [x] P3-T09 — Soft-wrap the multi-line mode of `TextInput`
  - verified: `cargo test -p fleet-ui-kit` (395 passed), `cargo build --example gallery_input`,
    `cargo fmt --check` and kit clippy `-D warnings` passed. Vertical motion and `home`/`end` stay
    logical-line here; the composer rebase (P3-T04) decides visual-row motion.
  - added 2026-09-19: the plan assumed no soft wrap, but the composer shapes `WrappedLine`s at
    its width and the old card-description area word-wraps, so rebasing either on a non-wrapping
    input would regress them. Kit-only; runs before P3-T04 and P3-T05.

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-18 — Plan written. Inventory at planning time (verify before migrating; line numbers drift):
  - Kit: `text_field/state.rs` (`TextFieldState`, byte cursor, no selection/clipboard/undo),
    `text_field/input.rs` (live `TextInput`, IME + click, used only by `fleet-lazygit`),
    `text_area/state.rs` (`TextAreaState`, no IME entity), `multiline_input/buffer.rs`
    (`MultilineBuffer`, grapheme-aware, anchor selection, word motion, marked text),
    `multiline_input/input.rs` (composer: drag, double-click, `cmd-c/x/v`, `alt-`/`cmd-` deletes,
    history, triggers). No undo anywhere.
  - App surfaces: Hub filter (`dialogs/filter.rs`, raw `String`), board filter (`screens/board.rs`),
    palette (`dialogs/palette.rs`), create worktree, clone repo, context, rename terminal, edit hooks,
    settings rows (`dialogs/settings/{draft,view}.rs`), board settings (`board_settings/draft.rs`, char caret rebuilt per key),
    card picker (`card_picker/draft.rs`, same), card create (`card_create.rs`, title char caret + `TextAreaState`),
    card detail (`card_detail/draft.rs`, one `TextAreaState` for three surfaces, "type first then act" hack), composer.
  - Keymap: three editing families (`filter::`, `palette::`, `dialog::`); `ctrl-u` semantics differ per surface;
    `delete`, `home`, `end`, `alt-backspace`, `cmd-backspace`, `cmd-a`, `cmd-z`, paste unbound in text contexts;
    `ctrl-v` asserted never bound.
- 2026-09-19 — P3-T01 landed as `components/input/{buffer,history,tests}.rs` exporting `InputBuffer`,
  `InputMode`, `HISTORY_CAP` (100) and `TYPING_GROUP_WINDOW` (300 ms). Every user mutation takes an
  injected `Instant`; the engine never reads a clock. Tabs: hard tab in multi-line, one space in
  single-line. `ctrl-k` at a multi-line line end joins the next line. Explicit history groups
  (`begin_history_group`/`end_history_group`) are for IME composition.
- 2026-09-19 — P3-T02 landed `TextInput` (`input.rs` + `input/{actions,element,platform,live_tests}.rs`).
  The `text_input::*` action family is defined in the kit (`gpui::actions!`, precedent `list_view.rs`)
  and exported as `fleet_ui_kit::text_input`; the app keymap binds it in P3-T03. Key context is
  `FleetTextInput` with attribute `mode=single_line|multiline`, so `Newline` binds under
  `FleetTextInput && mode == multiline`. Single-line `MoveUp`/`MoveDown`/`Newline` call
  `cx.propagate()`. The clock is `cx.background_executor().now()` so tests drive undo grouping.
  Old `TextInput`/`TextInputEvent` re-exports were renamed `LegacyTextInput`/`LegacyTextInputEvent`
  (lazygit updated) so the new component owns the name from day one.
- 2026-09-19 — P3-T03 scope adjusted: the old `filter::`/`palette::`/`dialog::` editing families are
  NOT deleted in T03, because every un-migrated dialog still edits through them until P3-T05/T06.
  T03 adds the `FleetTextInput` rows, the browsing/editing context-word split and the docs; each
  old family goes with its last consumer (T05 for the board dialogs, T06 for the rest) and T07
  asserts none remain. Intermediate commits therefore keep every dialog typable.
- 2026-09-19 — P3-T03 key-ownership words: browsing `CardDetail`/`BoardSettings`/`Settings`/`Create`
  and editing `CardDetailEditing`/`BoardSettingsEditing`/`SettingsEditing`/`CreateEditing`, derived
  in `dialogs/host.rs::dialog_key_context` from existing draft state and mirrored into `AppState`
  so `context_chain` stays authoritative. Review caught two rule violations: `:` had been kept under
  `CardDetailEditing` (a typed colon would open the palette) and the card picker had been made
  permanently editing, which unbound its `space` toggle. Fixed: the ownership test now rejects any
  single printable key or `shift-` variant in a host context, and `Dialog > CardPicker` keeps
  `space` as the one documented exception (its query is a filter that never contains a space;
  the migrated input will filter `' '`). The chord resolver strips `&& …` predicates for depth.
- 2026-09-19 — P3-T04 decisions (two deviations from the plan's Assumptions, both because the
  composer soft-wraps): multi-line `up`/`down` and plain `home`/`end` operate on the **visual** row
  (new `MoveToRowStart/End` + `SelectToRowStart/End` actions; `cmd-left/right`, `ctrl-a/e` stay
  logical), and vertical motion at the first/last visual row `cx.propagate()`s so the agent thread's
  own `up`/`down` history rows fire. The keymap decides whether `enter` inserts a newline through a
  new `enter=newline|owner` key-context attribute (`TextInput::set_enter_inserts_newline`); the
  composer publishes `owner`, and `shift-enter` → `Newline` is bound for every multi-line input.
  The app never used `NoAction` shadowing for the composer: it binds owner keys in `Agent > *` and
  routes back into the composer, which the propagation rule preserves.
- 2026-09-19 — P3-T05 decision: the board filter's column moves leave `left`/`right`/`ctrl-b`/`ctrl-f`
  (which the migrated filter input now uses for caret motion, deeper in the chain) and move to
  `tab`/`shift-tab` under `Filter > BoardFilter`. Card picker query gets a `' '` filter so `space`
  stays the toggle. Card detail creates its input when an edit begins and drops it when it ends;
  `tab` inserts a tab through a new `TextInput::insert`.
- 2026-09-19 — Executor switch: Codex hit its usage limit (reset Sep 22) at the start of P3-T06a,
  so P3-T06 onward runs on an Opus agent on the host, which can also run the socket tests and
  the GUI harness itself.
- 2026-09-18 — Decisions fixed at planning time: one engine seeded from the composer buffer; inputs are live
  entities owned by the surface; bytes inside, UTF-16 at the IME boundary, graphemes for motion; word and line rules
  as written in the plan; key-ownership rule via a browsing/editing context word (board-filter precedent), not
  `NoAction`; snapshot undo with a 300 ms group window; macOS-first bindings, `ctrl-v` stays unbound.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- `components/input.rs` is about 1100 lines after P3-T04; split the entity from its chrome/actions
  in P3-T07 per `rust-workspace-architecture`.

- Engine grapheme-boundary lookups scan from the text start (linear per call); fine for dialog-sized
  text, revisit with a boundary cache if a multi-kilobyte input ever feels slow.

- Cursor blink for the focused input (timer-driven, off while typing).
- Linux clipboard parity for text inputs without binding `ctrl-v` (shell-reserved).
- Masked (password) mode for a future secret-bearing settings row.
- Soft wrap in multi-line inputs, if a description ever needs it.
