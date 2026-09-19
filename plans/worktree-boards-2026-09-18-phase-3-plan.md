# Worktree-scoped boards — Phase 3: one text input for the whole app — Plan
> Tracker: ./worktree-boards-2026-09-18-phase-3-tracker.md
> Roadmap: ./worktree-boards-2026-09-18-roadmap.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Every text field in the app today is bare: no selection, no copy or paste outside the agent
composer, no word- or line-wise deletion in most dialogs, no undo anywhere, and no IME
composition in any dialog. The ui-kit carries three separate editing models with three
different key decoders, and the app dialogs re-wire the presentational ones by hand, so every
new surface (the board dialogs included) re-creates the same gaps. This phase builds **one**
editing engine and **one** live input component in `fleet-ui-kit`, single-line and multi-line
as modes of the same thing, with the full editing vocabulary a native text field is expected to
have, and migrates every text surface in `fleet-app` and `fleet-lazygit` onto it. Afterwards a
new feature that needs an input adds one entity and zero key handling.

## Sizing call

**Phased.** This is phase 3 of the roadmap and the largest phase: it introduces a component,
rewrites the keymap's editing rows, and touches sixteen surfaces. It is independent of phase 1
(daemon and CLI) and can run in parallel with it. It should land **before** phase 2 so the
board tab ships with proper inputs and phase 2 does not migrate dialogs twice. Doing it inside
phase 2 was rejected: the component is app-wide and would double that phase's size.

## Repository context

- `crates/fleet-ui-kit` has three input families today, about 5,600 lines in total:
  - `components/text_field.rs` + `text_field/{state,input,element,chrome}.rs`: presentational
    `TextField` (`RenderOnce`, caret in **characters**), pure `TextFieldState` (byte cursor,
    no selection, no clipboard, no undo, newlines stripped), and the live `TextInput` entity
    (IME via `EntityInputHandler`, click-to-place caret, no drag, no clipboard), used only by
    `fleet-lazygit`.
  - `components/text_area.rs` + `text_area/{state,surface,line}.rs`: presentational
    `TextArea` and pure `TextAreaState` (byte cursor, tabs, no selection, no clipboard, no IME
    entity).
  - `components/multiline_input.rs` + `multiline_input/{buffer,input,element,platform}.rs`:
    the agent composer. `MultilineBuffer` already has grapheme-aware motion
    (`unicode-segmentation` is a ui-kit dependency), an anchor-based selection, word motion,
    IME marked text and a goal column; `MultilineInput` has drag, double-click word select,
    `cmd-c/x/v`, `alt-`/`cmd-` word and line deletion, prompt history and `@ $ /` triggers.
    No undo.
- `docs/DESIGN-SYSTEM.md` §6.4 states the current rule: "All input components are
  presentational: the caller owns the string, the caret, the cursor and the focus, and handles
  the keys." That rule is what produced the hand-wiring below. §6.6 documents the composer.
- App surfaces (see the tracker's notes for the full inventory with file:line): Hub filter and
  board filter (raw `String`, append/pop only), palette query, create-worktree branch, clone
  search, context name/owners, rename terminal, edit hooks (a vector of fields), settings
  text/number rows, board settings, card picker, card create (title + description), card
  detail (title/description/comment, one shared buffer), and the agent composer. Card create,
  card picker and board settings store a character offset and rebuild a state object on every
  keystroke. Card detail and settings emulate key shadowing by "try to type the letter first,
  then act".
- Keymap: `crates/fleet-app/src/keymap.rs` (`key_table!`) binds three parallel editing action
  families: `filter::{Backspace,DeleteWord,Clear}`, `palette::{…}`, and
  `dialog::{Backspace,DeleteWord,ClearInput,LineStart,LineEnd,CursorLeft,CursorRight}`.
  Nothing binds `delete`, `home`, `end`, `alt-backspace`, `cmd-backspace`, `cmd-a`, `cmd-z` or
  paste in any text context. `ctrl-u` means "clear everything" in dialogs and "delete to line
  start" in the card editors. A test asserts `ctrl-v` is never bound (it belongs to shells).
  GPUI dispatches key bindings **before** `on_key_down`, which is why bare letters bound in a
  dialog context make a field untypable. Precedent for the fix already exists: while the board
  filter owns the keyboard, `AppState::context_chain` publishes `Filter > BoardFilter` instead
  of `Hub > Board`.
- `dialogs/input.rs` is the current shared helper (`typed_char`, `type_into`, `edit_actions`).
- Reference implementations to learn from, in the GPUI checkout at
  `~/.cargo/git/checkouts/zed-*/*/crates/gpui/examples/input.rs` and
  `examples/view_example/`: `EntityInputHandler` (UTF-16 at the platform boundary, UTF-8
  inside; replace range resolution "explicit → marked → selection"; marked text drawn as an
  underline run), `window.handle_input` registered every frame from `paint` only when focused,
  one `ShapedLine` per line with `x_for_index` / `closest_index_for_x` for caret and hit
  testing, selection painted as a quad under the text, grapheme boundaries via
  `unicode-segmentation`, and `TestAppContext::simulate_input` / `simulate_keystrokes` /
  `dispatch_action` for tests. Learn from them; **do not name their origin in code, comments,
  docs or commits.**
- Gallery: `crates/fleet-ui-kit/examples/gallery_input.rs` (text field, live text input,
  filter bar, palette), `kit_gallery.rs` (text areas), `gallery_agent.rs` (composer). The kit's
  standing rule: a state not in the gallery is not implemented.
- Harness: `docs/TESTING-HARNESS.md` is frozen. `clipboard set/get` fail in every lane today
  by design, so paste cannot be harness-tested; typing, keystrokes and dumps can.
- Verification: `make lint`, `make test`, `make harness`. Skills: `gpui-components`,
  `gpui-state-and-memory`, `gpui-styling`, `gpui-performance`, `gpui-app-shell`,
  `rust-gpui-testing`, `zed-quality-review`.

## Assumptions

- **One engine, one component.** `MultilineBuffer` is the seed of the single model (working
  name `InputBuffer`, in `components/input/buffer.rs`); `TextFieldState` and `TextAreaState`
  are deleted at the end of the phase. `TextInput` is the single live entity, with an
  `InputMode::{SingleLine, Multiline { min_rows, max_rows }}`. The composer becomes a thin
  wrapper over it that adds prompt history and triggers.
- **Inputs are live entities, owned by the surface.** The design-system rule flips: a dialog
  holds an `Entity<TextInput>` (created when the dialog is seeded, dropped with it) instead of
  a string and a caret. The presentational `TextField`/`TextArea` are removed; read-only text is
  a `Label`, not a field.
- **Offsets.** Bytes internally, UTF-16 only inside the platform input handler, grapheme
  clusters for every "one character" motion and deletion. A caret is never inside a grapheme.
- **Word rule.** Three character classes: word (alphanumeric or `_`), whitespace, punctuation.
  A word boundary is where the class changes, but a motion never stops onto whitespace (so
  trailing spaces are skipped and the caret lands at the start of the previous word), and a
  line start or end is always a boundary. Word deletion is less greedy than motion: after
  removing a whitespace run of two or more characters it stops.
- **Line rule.** Single-line: start and end of the text. Multi-line: `home`/`end` and
  `cmd-left`/`cmd-right` use the logical line (no soft wrap exists in these inputs).
  `ctrl-u` means delete to line start everywhere; `ctrl-k` delete to line end. The dialogs'
  old "clear everything" is gone (in a single-line field with the caret at the end it is the
  same keystroke with the same result).
- **Mode differences are exactly three.** In single-line mode `enter`, `tab`, `shift-tab`,
  `up`, `down`, `ctrl-n`, `ctrl-p` are not handled by the input and propagate to the
  container (submit, field navigation, list navigation), and pasted or typed newlines become
  spaces. In multi-line mode `enter` inserts a newline and `up`/`down` move by line; the
  composer alone keeps `enter` = submit and `shift-enter` = newline, as documented today.
- **Bindings are macOS-first**, like the rest of the keymap: `cmd-c/x/v/a/z`, `cmd-shift-z`,
  `alt-left/right`, `alt-backspace/delete`, `cmd-left/right`, `cmd-backspace/delete`,
  `cmd-up/down`, plus the cross-platform `ctrl-a/e/b/f/w/u/k/h/d` set and `home`/`end`,
  `shift-` variants of every motion, `backspace`/`delete`. `ctrl-v` stays unbound; Linux
  clipboard parity (`ctrl-shift-v` or similar) is a follow-up, not this phase.
- **Key ownership rule.** The input publishes its own key context word (`FleetTextInput`) and
  all editing actions are bound there as one `text_input::*` family. A surface that binds bare
  letters (card detail, settings, board settings, card picker) publishes those letters under a
  context word that is **not** in the chain while one of its inputs is focused, following the
  board-filter precedent; the generic `Dialog` rows (`enter`, `escape`, `tab`, `ctrl-n/p`)
  stay. `gpui::NoAction` shadowing was rejected because it needs a per-dialog list of letters.
- **Undo** is snapshot-based (text, caret, anchor), capped, grouped by a 300 ms window so a
  typing burst is one step, with an IME composition always collapsing into one step. Undo
  restores the selection.
- **Mouse.** Click places the caret, drag selects, double-click selects a word, triple-click a
  line, `shift-click` extends. Mouse is not harness-testable pixel-free; gpui tests cover it via
  simulated events.
- **Not in this phase:** cursor blink, soft wrap, masking (passwords), rich text, a spell
  checker, Linux clipboard keys, kill-ring yank. Each is a follow-up if wanted.

## Out of scope

- Any board, daemon or protocol change (phases 1 and 2).
- Terminal-grid copy mode and the PTY paste path (`workspace::PasteClipboard`); they are not
  text inputs.
- Changing what any dialog *does* on confirm; only how its text is edited.
- New dialogs or new fields.

## Affected areas

- `crates/fleet-ui-kit/src/components/{text_field,text_area,multiline_input}*` (replaced by
  `components/input.rs` + `input/{buffer,history,element,platform,tests}.rs` and a slimmer
  `multiline_input` wrapper), `components/{filter_bar,palette,number_field}.rs`,
  `examples/{gallery_input,kit_gallery,gallery_agent}.rs`.
- `crates/fleet-app/src/{actions,keymap}.rs`, `dialogs/input.rs` (deleted), `dialogs/{filter,
  palette,create_worktree,clone_repo,context,rename_terminal,edit_hooks,card_create,
  card_detail/*,card_picker/*,board_settings/*,settings/*}.rs`, `screens/board.rs` (board
  filter), `state/navigation.rs` (`context_chain`), `screens/agent_thread/*` (composer wiring),
  `dialogs/host.rs`, tests.
- `crates/fleet-lazygit/src/root/overlays.rs`, `overlays.rs`.
- `docs/DESIGN-SYSTEM.md` §6.4 §6.6, `docs/KEYMAP.md` (Dialogs and text inputs, Filter,
  Palette, board dialog rows), `docs/APP-CONTRACTS.md` §3, `docs/UX-SPEC.md` §3.8 dialog
  frame, `docs/decisions/0019-single-text-input.md`, scenarios under `scenarios/`.

## Tasks

### P3-T01 — Build the single editing engine
- **Intent:** One pure, exhaustively tested buffer model that every input uses.
- **Touches:** `crates/fleet-ui-kit/src/components/input/buffer.rs`, `input/history.rs`,
  `input/tests.rs` (or inline), seeded from `multiline_input/buffer.rs`.
- **Steps:**
  - Move `MultilineBuffer` to `input/buffer.rs` as the engine; add `InputMode`; make single-line
    mode sanitise inserted text (newlines and carriage returns to spaces, tabs kept or
    expanded per mode).
  - Implement the word rule and the less-greedy word deletion; line and document motions with
    a `select: bool` on every motion; `delete_to_line_start/end`; `select_all`, `select_word_at`,
    `select_line_at`; grapheme `left/right/backspace/delete`; replace-selection-on-insert.
  - Keep the IME surface: `marked` range, `replace_range`, `replace_and_mark`, `unmark`, and
    UTF-8 to UTF-16 range conversion helpers in one place.
  - `history.rs`: snapshot undo/redo with the 300 ms grouping window, a cap, "group until"
    for compositions, and selection restore.
  - Tests use a marked-text notation in strings (a caret marker and selection brackets) so each
    case reads as before/after text; cover multi-byte and combining characters, word rules
    around punctuation and whitespace runs, line boundaries, undo grouping with a fake clock.
- **Verification:** `cargo test -p fleet-ui-kit input`; `make lint`.
- **Done when:** Every editing operation in the Assumptions exists on the engine with a test,
  and the composer's old buffer tests still pass against it.

### P3-T02 — Build the live `TextInput` component
- **Intent:** One entity that renders the engine in single- or multi-line mode with IME,
  mouse, clipboard and focus handled inside the component.
- **Touches:** `crates/fleet-ui-kit/src/components/input.rs`, `input/{element,platform}.rs`,
  `examples/gallery_input.rs`, `docs/DESIGN-SYSTEM.md` §6.4.
- **Steps:**
  - Entity: engine + `FocusHandle` + per-line layout cache keyed by a text revision +
    scroll (horizontal in single-line, rows in multi-line) + drag anchor + read-only flag +
    optional character filter (for numeric rows) + validation flag for the invalid tone.
  - Element: one shaped line per logical line, selection quads under the text, caret quad
    only when focused, marked range as an underline run, placeholder in the muted tone,
    `min_rows`/`max_rows` growth in multi-line mode. Register the platform input handler from
    `paint` every frame while focused. All colours, sizes and durations from tokens.
  - Input: printable text and composition arrive only through the platform handler; control
    keys arrive as `text_input::*` actions (P3-T03); mouse down/move/up/up-out with click
    count; `cmd-c/x/v` through the clipboard API, paste sanitised per mode.
  - API: `new(mode, cx)`, `text()`, `set_text()`, `clear()`, `select_all()`,
    `move_to_end()`, `placeholder()`, `set_read_only()`, `set_invalid()`, `set_filter()`,
    `is_composing()`, `focus_handle()`; events `Changed`, `Submitted` (single-line `enter`
    is still propagated; the event is for callers that subscribe instead), `Blurred`.
  - Gallery: single-line empty/placeholder, filled, focused with caret, with selection, with
    marked text, invalid, read-only, numeric-filtered; multi-line at min rows, grown to max
    rows with scroll, with a multi-line selection.
  - Rewrite DESIGN-SYSTEM §6.4 around the new rule and API; §6.6 shrinks to what the composer
    adds.
- **Verification:** `cargo run -p fleet-ui-kit --example gallery_input`; gpui tests in the
  kit using `simulate_input`, `simulate_keystrokes` and simulated mouse events for typing,
  shift-selection, drag, double-click, copy/paste round trip, undo; `make lint`.
- **Done when:** Every gallery state renders and the component handles all editing without
  the caller touching a key.

### P3-T03 — Replace the editing rows in the keymap with one action family
- **Intent:** Bind every editing key once, under the input's own context, and delete the three
  duplicated families.
- **Touches:** `crates/fleet-app/src/{actions,keymap}.rs`, `docs/KEYMAP.md`,
  `docs/APP-CONTRACTS.md` §3, `state/navigation.rs`.
- **Steps:**
  - Add the `text_input::*` actions (move/select left, right, word, line, document; backspace,
    delete, delete word, delete to line start/end; select all; copy, cut, paste; undo, redo)
    and bind them under `FleetTextInput` per the Assumptions' key list. Bind `enter` for the
    multi-line newline under `FleetTextInput > Multiline` only.
  - Remove `filter::{Backspace,DeleteWord,Clear}`, `palette::{Backspace,DeleteWord,Clear}`,
    `dialog::{Backspace,DeleteWord,ClearInput,LineStart,LineEnd,CursorLeft,CursorRight}` and
    their rows; keep the container rows (`enter`, `escape`, `tab`, `shift-tab`, `ctrl-n/p`,
    `up/down`).
  - Implement the key-ownership rule: add an "editing" variant of the context word for every
    surface that binds bare letters, published by `context_chain` while one of its inputs is
    focused. Add a keymap test that no bare printable is bound in a context word that can be
    in the chain while `FleetTextInput` is.
  - KEYMAP.md: rewrite "Dialogs and text inputs" as the full table, update Filter and Palette
    sections and the board dialog rows; APP-CONTRACTS §3 gains the rule and the new words.
    The keymap drift test enforces the doc.
- **Verification:** `cargo test -p fleet-app keymap`; `make lint`.
- **Done when:** The keymap has exactly one place that says what `alt-backspace` does, and
  the drift and reserved-key tests pass.

### P3-T04 — Re-base the agent composer on the shared component
- **Intent:** Keep the composer's behaviour and API while removing its private engine.
- **Touches:** `crates/fleet-ui-kit/src/components/multiline_input.rs` + `multiline_input/*`,
  `examples/gallery_agent.rs`, `crates/fleet-app/src/screens/agent_thread/*`,
  `screens/workspace/agent.rs`.
- **Steps:**
  - `MultilineInput` wraps `TextInput` in multi-line mode and adds: `enter` submit,
    `shift-enter` newline, history on `up`/`down` at the buffer edges, `@ $ /` trigger
    reporting, read-only and dimmed states. Its events and `MULTILINE_INPUT_KEY_CONTEXT` stay
    so the agent thread wiring is untouched or minimally changed.
  - Delete `multiline_input/{buffer,element,platform}.rs` once the wrapper compiles.
  - Existing composer tests pass; the agent gallery is visually unchanged (compare against the
    harness baselines for agent scenarios).
- **Verification:** `cargo test -p fleet-ui-kit multiline`; `cargo test -p fleet-app
  agent_thread`; `make harness-one SCENARIO=scenarios/agents/popup-open.scenario` and the
  other `scenarios/agents/*`; `make lint`.
- **Done when:** The composer has no editing code of its own and every agent scenario is green.

### P3-T05 — Migrate the board dialogs
- **Intent:** Card create, card detail, card picker and board settings edit through
  `Entity<TextInput>` and lose their hand-rolled carets.
- **Touches:** `crates/fleet-app/src/dialogs/{card_create.rs,card_detail/*,card_picker/*,
  board_settings/*}`, `dialogs/host.rs`, `screens/board.rs` (board filter), tests,
  `docs/APP-CONTRACTS.md` "Board app extension points", `docs/BOARD.md` §8, `docs/UX-SPEC.md`
  §Board card detail.
- **Steps:**
  - Card create: title is a single-line input, description a multi-line input; `ctrl-enter`
    create-and-open stays a dialog action; `tab` moves fields.
  - Card detail: one multi-line input reused for title (single-line mode), description and
    comment; `ctrl-s` save and `escape` cancel stay; the bare-letter rows move under the
    "browsing" context word per P3-T03.
  - Card picker query and board settings text rows become inputs; delete the per-keystroke
    state rebuilds; `space` toggle and `j/k/h/l` cycling stay under the browsing word.
  - Board filter: a single-line input; `Filter > BoardFilter` keeps left/right column moves.
  - Update the `DialogHost` field table in APP-CONTRACTS, BOARD.md §8 and the UX-SPEC card
    detail text (selection, paste, undo now exist).
- **Verification:** `cargo test -p fleet-app dialogs`; `make harness-one` on each
  `scenarios/board/*` scenario; `make lint`.
- **Done when:** No board dialog holds a caret or a string buffer of its own.

### P3-T06 — Migrate the remaining dialogs, filters and the lazygit overlay
- **Intent:** Every other text surface uses the component.
- **Touches:** `crates/fleet-app/src/dialogs/{filter,palette,create_worktree,clone_repo,
  context,rename_terminal,edit_hooks}.rs`, `dialogs/settings/*`, `dialogs/input.rs` (delete),
  `crates/fleet-ui-kit/src/components/{filter_bar,palette,number_field}.rs`,
  `crates/fleet-lazygit/src/root/overlays.rs`, `overlays.rs`, tests, `docs/KEYMAP.md` rows
  for each, `docs/UX-SPEC.md` §3.10 filter bar.
- **Steps:**
  - Hub filter and palette: single-line inputs; `up/down`, `ctrl-n/p`, `enter`, `escape`
    propagate to the container as today; the two-stage `escape` stays.
  - Create worktree: branch input; host cycling moves off `left`/`right` to the container's
    own keys or stays under a browsing word, per P3-T03.
  - Clone, context (two inputs + `tab`), rename terminal, edit hooks (a `Vec<Entity<TextInput>>`
    with the blank-row rule), settings rows (an input materialised when a row enters editing;
    numeric rows use the digit filter; bare `j/k/h/l/space` under the browsing word).
  - `FilterBar` and `PaletteCard` take the input entity (or its rendered element) instead of a
    value and caret; `NumberField` renders the input when editing.
  - lazygit overlay: swap to the new `TextInput` API.
  - Delete `dialogs/input.rs`.
- **Verification:** `cargo test -p fleet-app`; `cargo test -p fleet-lazygit`;
  `make harness-one` on `scenarios/hub/*`, `scenarios/board/filter.scenario`; `make lint`.
- **Done when:** `grep -rn "TextFieldState\|TextAreaState\|typed_char" crates/` returns
  nothing outside the kit's compatibility shims scheduled for deletion in P3-T07.

### P3-T07 — Delete the old input families and record the decision
- **Intent:** Leave one input in the kit and one rule in the design system.
- **Touches:** `crates/fleet-ui-kit/src/components/{text_field*,text_area*}`,
  `components/mod.rs`, `examples/kit_gallery.rs`, `docs/DESIGN-SYSTEM.md`,
  `docs/decisions/0019-single-text-input.md`, `docs/README.md` if it indexes ADRs.
- **Steps:**
  - Remove `TextField`, `TextFieldState`, `TextInput` (old), `TextArea`, `TextAreaState` and
    their gallery sections; move any read-only display to `Label`.
  - ADR 0019: why inputs became live entities, why one engine, the word and line rules, the
    key-ownership rule, undo grouping, what was rejected (presentational fields with
    caller-owned keys; `NoAction` shadowing).
  - DESIGN-SYSTEM: §6.4 final text, inventory tables, the "a state not in the gallery is not
    implemented" list for the input.
  - Run `zed-quality-review` over the phase's diff.
- **Verification:** `make lint`; `make test`; `cargo run -p fleet-ui-kit --example
  gallery_input`.
- **Done when:** The kit exports one input component and the docs describe only it.

### P3-T08 — Drive typing, selection and undo in the harness
- **Intent:** Prove the new editing in the real GUI where the harness can.
- **Touches:** `scenarios/board/card-create-editing.scenario`, `scenarios/hub/filter-editing.scenario`
  (names indicative), `crates/fleet-app/src/drive.rs` only if a dump field is missing,
  `docs/TESTING-HARNESS.md` only if the grammar must change (read it first; it is frozen).
- **Steps:**
  - Card create: type a title, `alt-backspace` removes the last word, `shift-left` twice then
    typing replaces the selection, `cmd-z` restores it; dump the title text. Description:
    `enter` inserts a newline, `cmd-backspace` deletes to line start.
  - Hub filter: type, `ctrl-w`, `ctrl-u`, `home`/`end` behave as the table says while the list
    cursor still moves on `ctrl-n/p`.
  - Note in each scenario that paste is untested in the harness because the clipboard fails in
    every lane by design; the gpui tests in P3-T02 cover it.
  - `make harness` full run; update baselines only for the screens this phase changed.
- **Verification:** `make harness` green with the new scenarios in the report.
- **Done when:** The scenarios pass in the virtual lane and their dumps show the edited text.

## Verification

Run from the repository root:

```sh
make lint
make test
make harness        # required: dialogs, the keymap and the filter bar all change
```

Targeted while iterating: `cargo test -p fleet-ui-kit`, `cargo test -p fleet-app`,
`cargo test -p fleet-lazygit`, `cargo run -p fleet-ui-kit --example gallery_input`,
`make harness-one SCENARIO=<path>`.

## Definition of done

- [ ] Every P3 task is `[x]` in the tracker and the tracker matches the code.
- [ ] `make lint` is clean.
- [ ] `make test` passes (clippy `-D warnings` is this repo's type gate).
- [ ] `make harness` passes, including the agent and board scenarios that existed before.
- [ ] One input component in the kit; `TextFieldState`, `TextAreaState`, `dialogs/input.rs`
      and the `filter::`/`palette::`/`dialog::` editing actions are gone.
- [ ] KEYMAP, DESIGN-SYSTEM, APP-CONTRACTS, UX-SPEC and ADR 0019 agree with the code.
- [ ] No mention of the reference implementation's origin anywhere in the diff.
- [ ] No IO or `cx.notify` in `render`; no bare `.detach()`; no `unwrap`; tokens only in the kit.
- [ ] Follow-ups (cursor blink, Linux clipboard keys, masking) captured in the tracker.

## Risks and rollback

- **Composer regression.** The composer is the one input users type into for minutes at a
  time; P3-T04 runs every agent scenario and keeps the composer's tests. If it regresses,
  revert P3-T04 alone: the wrapper boundary is designed so the old engine can return.
- **Bare-letter dialogs become untypable or unnavigable.** The key-ownership rule and its
  keymap test are the guard; the board scenarios (`move-card`, `navigation`) and the settings
  dialog are where it would show first.
- **IME on macOS.** Only the composer and lazygit had a platform handler before; every dialog
  now does. Test dead keys and a CJK input source manually on macOS before calling P3-T02 done.
- **Performance.** Every keystroke re-shapes one line per logical line; multi-line inputs are
  bounded by `max_rows`. Memoise by text revision; `gpui-performance` applies.
- **Scope creep.** The temptation is to add blink, soft wrap, masking. They are follow-ups.
- **Rollback:** the phase is a sequence of migrations; each task is revertible on its own until
  P3-T07 deletes the old families. Do not start P3-T07 until every surface is migrated and the
  harness is green.
