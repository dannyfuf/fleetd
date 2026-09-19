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
- [ ] I have read the plan end to end.
- [ ] I have run the project-wide verification commands once on a clean tree to confirm a green baseline (`make lint && make test && make harness`).
- [ ] I have read `docs/DESIGN-SYSTEM.md` §6.4 and §6.6 and `docs/KEYMAP.md` "Dialogs and text inputs" as they are today.
- [ ] I am ready to start.

## Tasks
- [ ] P3-T01 — Build the single editing engine
- [ ] P3-T02 — Build the live `TextInput` component
- [ ] P3-T03 — Replace the editing rows in the keymap with one action family
- [ ] P3-T04 — Re-base the agent composer on the shared component
- [ ] P3-T05 — Migrate the board dialogs
- [ ] P3-T06 — Migrate the remaining dialogs, filters and the lazygit overlay
- [ ] P3-T07 — Delete the old input families and record the decision
- [ ] P3-T08 — Drive typing, selection and undo in the harness

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
- 2026-09-18 — Decisions fixed at planning time: one engine seeded from the composer buffer; inputs are live
  entities owned by the surface; bytes inside, UTF-16 at the IME boundary, graphemes for motion; word and line rules
  as written in the plan; key-ownership rule via a browsing/editing context word (board-filter precedent), not
  `NoAction`; snapshot undo with a 300 ms group window; macOS-first bindings, `ctrl-v` stays unbound.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- Cursor blink for the focused input (timer-driven, off while typing).
- Linux clipboard parity for text inputs without binding `ctrl-v` (shell-reserved).
- Masked (password) mode for a future secret-bearing settings row.
- Soft wrap in multi-line inputs, if a description ever needs it.
