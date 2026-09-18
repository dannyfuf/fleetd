# Native subagents, phase 5: the agents picker — Plan
> Tracker: ./native-subagents-2026-09-17-phase-5-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Make every native thread the daemon knows reachable without a tab. `^s d` opens the command palette
seeded to a new `AGENTS` section, the way `^s W` seeds `sessions`. It lists every thread of the
current worktree, callers first with their children indented under them, then children that run in
another worktree with a `· <worktree>` suffix, and includes threads whose tabs were closed. `⏎`
attaches and selects a hidden thread, selects an attached one, reopens a closed caller, and switches
session first for a child in another worktree. This closes a gap that exists today: a closed thread
has no way back.

## Sizing call

**Phased, phase 5 of 7.** See ./native-subagents-2026-09-17-roadmap.md. Standard on its own: one
palette section, one chord, one selection rule, three scenarios. It is separate from phase 4 because
it needs phase 4's `attached` set and `reopen` path and adds nothing to the strip model.

## Repository context

- Rust workspace; this phase touches `fleet-app`, `fleet-ui-kit` (only if the palette row needs a
  new slot), `scenarios/` and `docs/`.
- Lint: `make lint`. Test: `make test`. Harness: `make harness`, `make harness-one SCENARIO=...`.
- Palette: `crates/fleet-app/src/dialogs/palette.rs` — `is_session_switcher` at line 1472
  (`query.trim() == "sessions"`), the seeded-query path around lines 739 to 760, `go_rows` at
  765 ("sessions first"), `PaletteSection` at 1041, the cap of ten rows and the `n of m` footer
  around 1386. Seeding: `dialogs/host.rs:333` calls `palette::seed`.
- Chord: `keymap.rs:263` maps `ctrl-s W` to `prefix::SessionSwitcher` and line 569 binds it in
  `Workspace > Prefix`; the same rows are repeated inside every agent sub-mode. `ctrl-s d` is free.
- Session switch: `crates/fleet-app/src/state/navigation.rs` and its tests own the active session
  and the switch path the session switcher uses.
- Agent state from phase 4: `state/agents.rs` `attached`, `attach`, `reopen`, `of_worktree`,
  `summaries()` (every thread the daemon listed, across worktrees).
- Harness snapshot: `crates/fleet-app/src/state/harness/projection.rs` already projects the
  palette's rows for the sessions switcher (find with `grep -n palette`); the same list is reused.
- Skills: `gpui-app-shell` (palette, chord), `gpui-components` (row shape), `gpui-performance`
  (the list is rebuilt per keystroke; keep it cheap), `rust-gpui-testing`, `zed-quality-review`.
- Docs: `docs/UX-SPEC.md` §3.9 command palette; `docs/KEYMAP.md`; `docs/NATIVE-AGENTS.md` §12,
  §13, §15.

## Assumptions

- The `AGENTS` section is a seeded mode like `sessions`: typing `agents` in the palette or pressing
  `^s d` lists only agent rows; a bare `:` palette does not mix agent rows into GO. Filtering by
  title and provider applies within the section.
- Row slots: key column (strip index or `·`), glyph (attention mark), primary
  (`↳ codex — <title>` for a child, `claude — <title>` for a caller), secondary (`working · 14m`,
  `blocked · question`, `done · 2h`), trailing (`attach` or `go`). If the kit's palette row has no
  secondary or trailing slot, add them as optional builder methods; no new component.
- Rows across worktrees come from `summaries()`; the current worktree's callers and children come
  first, then other-worktree children under their caller. A caller in another worktree is not
  listed (the session switcher is for that).
- The `agents` preset seeds more than one worktree, or the scenario creates a second one with the
  Hub's create-worktree flow before delegating with `--worktree`. Confirm which at the start of
  P5-T04 and note it in the tracker.

## Out of scope

- Any daemon change.
- Listing terminal (PTY) sessions in the section.
- A sidebar or a persistent thread list.

## Affected areas

- `crates/fleet-app/src/dialogs/palette.rs`, `dialogs/host.rs`, `actions.rs`, `keymap.rs`,
  `state/navigation.rs`, `state/agents.rs`, `state/harness/projection.rs`, tests beside each.
- `crates/fleet-ui-kit/src/components/` palette row (only if a slot is missing).
- `scenarios/agents/subagent-*.scenario`, `scenarios/agents/README.md`.
- `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/NATIVE-AGENTS.md`.

## Tasks

### P5-T01 — The `AGENTS` palette section
- **Intent:** List every native thread with the slots the design names, inside the existing
  palette.
- **Touches:** `crates/fleet-app/src/dialogs/palette.rs`, its tests.
- **Steps:**
  - `is_agents_picker(query)` beside `is_session_switcher`; `agents_rows(state, matcher, limit)`
    building the ordered list from `summaries()`, `attached`, `closed`, the strip indices, and each
    thread's `attention_for` this client's cursor.
  - Reuse the section, cap and footer machinery; the footer reads `n of m` as today.
  - Filter: fuzzy on title and provider name.
  - Tests: order (callers, children, other-worktree children with suffix); a closed caller is
    listed; the key column shows the strip index only when attached; the filter narrows by provider.
- **Verification:** `cargo test -p fleet-app palette`, `make lint`.
- **Done when:** The four tests pass and the section renders in the palette with the seeded query.

### P5-T02 — `^s d` seeds the palette to `AGENTS`
- **Intent:** One chord to the picker from any workspace or agent context.
- **Touches:** `actions.rs`, `keymap.rs`, `dialogs/host.rs`, keymap tests.
- **Steps:**
  - `prefix::AgentsPicker` action; `ctrl-s d` in the chord table beside `ctrl-s W`; bound in
    `Workspace > Prefix` and repeated in every agent sub-mode with the same list the other prefix
    rows use.
  - The handler opens the palette with the query `agents`, exactly as `SessionSwitcher` opens it
    with `sessions`.
  - Test: the keymap table test at `keymap.rs:1255` gains the pair; the prefix-inside-a-thread
    test gains `d`.
- **Verification:** `cargo test -p fleet-app keymap`, `make lint`.
- **Done when:** `^s d` opens the seeded palette in a terminal tab and in an agent tab.

### P5-T03 — Selection: attach, reopen, or switch session then attach
- **Intent:** Make `⏎` do the one right thing for each row state.
- **Touches:** `dialogs/palette.rs` (accept handler), `state/agents.rs`, `state/navigation.rs`,
  tests.
- **Steps:**
  - Attached thread → select it. Hidden child → `attach` then select. Closed caller → `reopen`
    then select. Child in another worktree → switch to that worktree's session through the
    navigation path the session switcher uses, then `attach` and select there; the row's suffix
    already told the user.
  - Focus lands on the thread's composer, as every new-thread path asserts.
  - Tests: each of the four branches; the cross-worktree branch asserts the session changed before
    the attach.
- **Verification:** `cargo test -p fleet-app`, `make lint`.
- **Done when:** All four branches have a passing test and nothing is auto-attached on `Blocked`.

### P5-T04 — Harness scenarios: attach from the picker, reopen a closed caller, other-worktree child
- **Intent:** Prove the picker on the real GUI.
- **Touches:** `scenarios/agents/subagent-attach-from-picker.scenario`,
  `subagent-reopen-closed-caller.scenario`, `subagent-other-worktree-child.scenario`,
  `scenarios/agents/README.md`, `state/harness/projection.rs` if a palette row field is missing.
- **Steps:**
  - Confirm the palette rows are already in the snapshot for the sessions switcher; add any
    optional field the assertions need (`child`, `attached`, `worktree`) under the existing list
    name.
  - Attach from picker: delegate, `^s d`, assert the child row with `attach`, `enter`, assert the
    strip has the child and focus is on its composer.
  - Reopen: `^s x` on the caller, `^s d`, `enter` on the caller row, assert it is back.
  - Other worktree: a caller transcript variant that passes `--worktree <second>`; `^s d`, assert
    the suffix, `enter`, assert the session switched and the child is attached there.
  - README rows for each.
- **Verification:** `make harness-one SCENARIO=...` per file, then `make harness`.
- **Done when:** All three pass in the virtual lane.

### P5-T05 — Document the section and the chord
- **Intent:** Keep the palette and keymap documents authoritative.
- **Touches:** `docs/UX-SPEC.md`, `docs/KEYMAP.md`, `docs/NATIVE-AGENTS.md`.
- **Steps:**
  - `UX-SPEC.md` §3.9 gains the `AGENTS` section with its five slots, the order rule, the
    cross-worktree rule and the nine-tab-ceiling guidance ("attach what you look at, detach when
    done, reach the rest through `^s d`").
  - `KEYMAP.md` gains `^s d` in Workspace and Native agent thread.
  - `NATIVE-AGENTS.md` §12 gains the chord; §13 row 9 drops "picker owed"; §15 UI subsection
    gains the picker paragraph.
- **Verification:** `git diff docs/` beside the scenarios.
- **Done when:** Every scenario assertion has a sentence in one document.

## Verification

```sh
make lint
make test
make harness
```

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean.
- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `make test` passes.
- [ ] `make harness` passes with the three new scenarios.
- [ ] `UX-SPEC.md` §3.9, `KEYMAP.md` and `NATIVE-AGENTS.md` describe the picker as built.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **The palette list is rebuilt on every keystroke.** Keep `agents_rows` a projection over already
  memoised summaries; do not compute attention per row without the cached cursor.
- **A cross-worktree switch has side effects** (the active session changes). The row's suffix is
  the only warning; the scenario asserts it is present before `⏎`.
- **Rollback.** Revert the phase PR; phase 4's row and card remain the only attach paths.
