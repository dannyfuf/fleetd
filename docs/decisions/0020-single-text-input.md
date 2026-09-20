# 0020 — One live text input for the whole app

**Adopted** for `fleet-ui-kit`'s `components/input*`, the `text_input::*` action family, every
text surface in `fleet-app`, and the single-line prompt of `fleet-lazygit`. The kit has one
editing engine and one live editor entity; a surface owns an `Entity<TextInput>` and decodes no
editing keys around it.

## Context

The kit carried three input families at once. `TextField` was a `RenderOnce` surface over a
caller-owned string and a caret counted in characters, with a pure `TextFieldState` beside it
that had no selection, no clipboard and no undo. `TextArea` and `TextAreaState` were the same
shape again in two dimensions, with a byte cursor and a preferred column. Only the agent
composer had a live entity with grapheme-aware motion, an anchor selection, IME marked text and
clipboard keys — and no undo. Nothing anywhere had undo.

Because the first two families were presentational, every dialog re-implemented editing by hand.
Card create, card picker and board settings stored a character offset and rebuilt a state object
on every keystroke. Card detail drove three surfaces from one buffer. Settings and card detail
emulated key shadowing by trying to type the pressed letter first and only then running the
action bound to it. The keymap carried three parallel editing families — `filter::`, `palette::`
and `dialog::` — in which `ctrl-u` meant "clear everything" in a dialog and "delete to line
start" in a card editor, while `delete`, `home`, `end`, `alt-backspace`, `cmd-backspace`,
`cmd-a`, `cmd-z` and paste were bound in no text context at all.

Two properties of the toolkit shaped the fix. GPUI dispatches key **bindings** before an
element's `on_key_down`, so a dialog that binds a bare letter makes any field inside it
untypable no matter how carefully the field handles keys; the old "type first, then act" hack
existed only to work around that. And the platform talks to an input in UTF-16 through
`EntityInputHandler`, so IME composition is possible only for a real entity that installs the
handler from `paint` while focused — which no presentational field can do. The GPUI input
example demonstrates both.

## Decision

- **One engine.** `InputBuffer` (`components/input/buffer.rs`) is the single editing model, in
  `InputMode::{SingleLine, Multiline { min_rows, max_rows }}`. It is pure: every user mutation
  takes an injected `Instant` and the engine never reads a clock.
- **One live entity.** `TextInput` owns the focus handle, buffer, selection, undo history,
  platform input bridge, painted-layout cache and scroll position. Inputs are entities **owned by
  the surface**: a dialog creates one when it is seeded and drops it with itself, instead of
  holding a string and a caret and passing both down every frame.
- **Offsets.** Bytes internally, UTF-16 only inside the platform input handler, extended grapheme
  clusters for every "one character" motion or deletion. A caret is never inside a grapheme.
- **The word rule.** Three character classes — word (alphanumeric or `_`), whitespace,
  punctuation. A boundary is where the class changes; a motion never stops onto whitespace, so
  trailing spaces are skipped and the caret lands at the start of the previous word; a line start
  or end is always a boundary. **Deletion is less greedy than motion**: after removing a
  whitespace run of two or more characters it stops, so `ctrl-w` in an indented line does not
  swallow the word before the indent.
- **The line rule, with soft wrap.** Multi-line values soft-wrap at the box width and `min_rows` /
  `max_rows` count **visual** rows. `up`/`down` and unmodified `home`/`end` operate on the visual
  row (`MoveToRowStart`/`MoveToRowEnd` and their selecting twins); `cmd-left`/`cmd-right` and
  `ctrl-a`/`ctrl-e` stay logical-line motions. `ctrl-u` is delete-to-line-start everywhere and
  `ctrl-k` delete-to-line-end; the dialogs' old "clear everything" is gone, because in a
  single-line field with the caret at the end it is the same keystroke with the same result.
- **Boundary propagation.** Vertical motion at the first or last visual row calls `cx.propagate()`
  instead of consuming the key, so the agent thread's `up`/`down` prompt history and a list's
  cursor keys still fire while the editor holds the keyboard. Selection-extending motion stops at
  the boundary rather than propagating.
- **The key-ownership rule.** The input publishes one key-context word, `FleetTextInput`, and the
  whole `text_input::*` family is bound there. A surface that binds bare letters publishes them
  under a **browsing** context word that leaves the chain while one of its inputs is focused, and
  an **editing** word takes its place: `CardDetail`/`CardDetailEditing`,
  `BoardSettings`/`BoardSettingsEditing`, `Settings`/`SettingsEditing`, `Create`/`CreateEditing`.
  The generic `Dialog` rows (`enter`, `escape`, `tab`, `ctrl-n/p`) stay in both. A keymap test
  rejects any single printable key, or `shift-` variant of one, in a host context. The one
  documented exception is `Dialog > CardPicker`'s `space`, which toggles the highlighted card: the
  picker's query is a filter that never contains a space, and its input filters `' '` out of
  typing and paste so the model and the binding agree.
- **`enter` is a key-context attribute.** `FleetTextInput` carries `mode=single_line|multiline`
  and `enter=newline|owner`. Plain `Newline` is bound under
  `FleetTextInput && mode == multiline && enter == newline`; a surface that owns `enter` — the
  agent composer, where `⏎` sends — calls `set_enter_inserts_newline(false)` and publishes
  `owner`. `shift-enter` inserts a newline in every multi-line input.
- **Undo is snapshot-based**: text, caret and anchor, capped at `HISTORY_CAP` (100) and grouped by
  a `TYPING_GROUP_WINDOW` of 300 ms so a typing burst is one step. An IME composition opens an
  explicit history group and always collapses into one step. Undo restores the selection.
- **Bindings are macOS-first**, like the rest of the keymap: `cmd-c/x/v/a/z`, `cmd-shift-z`,
  `alt-left/right`, `alt-backspace/delete`, `cmd-left/right`, `cmd-backspace/delete`,
  `cmd-up/down`, plus the cross-platform `ctrl-a/e/b/f/w/u/k/h/d` set, `home`/`end`, the `shift-`
  variant of every motion, and `backspace`/`delete`. **`ctrl-v` is not one of the editor's rows**
  and is bound in no terminal context either: it belongs to the shells Fleet hosts, and two
  keymap tests say so. Paste is `cmd-v`; Linux parity is a follow-up. `fleet-lazygit`, which
  hosts no shell, is the one place that binds `ctrl-v`, in its own prompt context and routed into
  the editor through `TextInput::insert`.
- **One table, two statements of it.** `text_input::default_bindings()` is the whole
  `FleetTextInput` table for a host with no key table of its own — the galleries, `fleet-lazygit`
  standalone, the kit's tests. `fleet-app` states the same rows inside `key_table!`, because that
  macro also feeds the Help overlay and the documentation-drift test, and a test asserts the two
  agree.

## Alternatives rejected

- **Keeping presentational fields with caller-owned keys.** This is what produced the three
  families, the per-dialog key decoders and the missing vocabulary. A presentational field cannot
  install `EntityInputHandler`, so IME composition is impossible in it; and because the caller
  owns the keys, every new surface re-derives the same incomplete edit set. The design-system rule
  flipped instead: read-only text is a `FactRow` or a `Label`, and anything editable is an entity.
- **`gpui::NoAction` shadowing per dialog.** Rebinding each of a dialog's bare letters to
  `NoAction` inside the input's key context would work, but it needs a per-dialog list of letters
  that must be kept in step with the dialog's own bindings forever. The browsing/editing context
  word is one row per dialog and the board filter already proved it.
- **Visual-row motion inside the engine.** Soft wrap is a property of the painted layout, not of
  the text, so teaching `InputBuffer` about it would mean feeding measured geometry into a pure
  model. Visual motion lives in the entity, reads the revision-tagged layout cache, and falls back
  to the logical line when the current revision has not been painted yet.
- **One input reused across modes.** A single entity retargeted at the title, then the
  description, then a comment was how card detail worked before, and it is why "type first, then
  act" existed: the surface had to decide per keystroke which buffer the key meant. One entity per
  editable surface, created when the edit begins and dropped when it ends, removes the question.
- **Two components, single-line and multi-line.** They differ in exactly three ways — what `enter`,
  `tab`/`shift-tab` and `up`/`down` do, and whether a pasted newline becomes a space — which is an
  `InputMode`, not a second component with a second element, a second input handler and a second
  set of tests.

## Consequences

Three keys moved, and KEYMAP, UX-SPEC and APP-CONTRACTS say so. The board filter's column moves
left `left`/`right`/`ctrl-b`/`ctrl-f` to the editor and became `tab`/`shift-tab` under
`Filter > BoardFilter`. Context-dialog delete became `ctrl-shift-d`, because `ctrl-d` is
delete-forward in the input and that dialog has no browsing state to hide it behind. `enter` on a
settings text or number row now opens the row for editing; the second `enter`, arriving under
`SettingsEditing`, saves.

Every editable surface allocates one entity and must focus it: the shell's focus gate asks the
rendered surface which handle it wants (`wanted_input`) and restores it after a dialog closes, so
an input that is drawn but never focused is a bug the gate makes visible rather than a dead field.

A read-only value is no longer a box. Settings and board-settings rows that are not being edited
render as `FactRow`s, so an empty value reads `—` instead of a blank field, and a row's
placeholder and validation rule are visible only in the editor `enter` opens.

The kit exports one input and the design system describes one: `TextField`, `TextFieldState`,
`EditEffect`, `TEXT_FIELD_KEY_CONTEXT`, the previous live `TextInput` re-exported as
`LegacyTextInput`, `TextArea`, `TextAreaState`, `TAB_WIDTH` and `TEXT_AREA_ROWS` are gone, as are
the editing rows of the `filter::`, `palette::` and `dialog::` action families — each kept only
its container actions — and the `dialogs/input.rs` helper.
`MultilineInput` remains, but only as composer chrome — prompt history, owner-routed
`enter` and the `@ $ /` trigger reports — over the shared entity.

Not in this decision, and each a follow-up if wanted: cursor blink, masked (password) mode, a
Linux clipboard paste key that is not `ctrl-v`, and kill-ring yank. `fleet-lazygit` also keeps its
own `Buffer` for the multi-line commit prompt and the operation-menu filter; both are hand-drawn
surfaces that never used the deleted families, and migrating them onto `TextInput` is a separate
piece of work.
