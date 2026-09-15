# Fleet e2e harness — Phase 3: mouse, targets and the rest of human input — Plan
> Tracker: ./fleetd-e2e-harness-2026-09-11-phase-3-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Fleet is keyboard-first, but it is not keyboard-only: rows are clicked, tabs are picked, board cards
are dragged, badges have tooltips, and the repos rail collapses under the pointer. The driver today
can press keys and — for `fleet-lazygit` only — send a wheel event at a fraction of the viewport. It
cannot move, click, double-click, right-click, drag or hover, and it has no way to say *what* to
click except by guessing a pixel.

This phase gives the harness the rest of a human's input vocabulary, and makes it addressable by
name. A small element wrapper in `fleet-ui-kit` records the painted bounds of anything a test needs
to reach; those bounds appear in the Phase 2 snapshot as a `targets` map; and mouse commands accept
either a target name or raw coordinates. A scenario says `click worktrees.row[2]`, not `click 412
338`, so it survives a resize, a font change and a layout tweak.

## Sizing call

**Phased**, phase 3 of 5 — see [the roadmap](./fleetd-e2e-harness-2026-09-11-roadmap.md). A shorter
phase than 1, 2 or 4: three to five focused days. It is nonetheless its own phase because it
depends on Phase 2's snapshot for target resolution and because the bounds recorder touches
`fleet-ui-kit`, which has its own rules and its own gallery-as-acceptance-test convention.

## Repository context

- **Project type / commands:** as Phase 1 — `make lint`, `make test`.
- **The only mouse support today** is `Step::Wheel` in `crates/fleet-lazygit/src/drive/support/`
  (moved to `fleet-drive` in Phase 1), which dispatches a `PlatformInput::ScrollWheel` at a fraction
  of the viewport with a fixed 18 px "wheel unit", and is accepted only under `Dialect::Lazygit`.
- **GPUI input dispatch:** `window.dispatch_event(PlatformInput::…)` for mouse events,
  `window.dispatch_keystroke` for keys — the path the existing driver already uses, so bindings and
  hit tests resolve exactly as they do for real input.
- **Bounds are known at paint, not at update.** This is the tension with
  `docs/APP-CONTRACTS.md`'s "render prepares nothing": the rule forbids IO, requests, `cx.notify`
  and heavy CPU in `render`, not recording a rect a frame already computed. The design keeps the
  recorder to one branch on an already-loaded flag and one push into a pre-allocated vector, and
  keeps it inert when harness mode is off.
- **`fleet-ui-kit` rules:** components take no domain types and contain no literal colours, sizes or
  durations (`docs/DESIGN-SYSTEM.md`); every component is exercised by the gallery. A target wrapper
  takes a name and a child, which satisfies both.
- **Surfaces with real pointer behaviour**, from `docs/UX-SPEC.md` and `docs/KEYMAP.md`: the repos
  rail (collapse/expand, `H`), worktree and PR lists, the board (cards move between columns), the
  workspace tab strip, dialogs and the palette, the jobs panel, toasts and the sticky error, and the
  agent popup with its approval cards.

## Assumptions

- Synthetic dispatch is the right level. It is focus-correct, works in the headless lane, and does
  not race the developer's session. Driving the real compositor (`wtype`, `ydotool`) would test
  GPUI's Wayland input layer, which is not what this harness is for.
- Not every element needs a target. Targets are added where scenarios need them, on demand, not
  swept across the component library.
- Target names are stable identifiers that scenarios depend on, so they are named deliberately and
  renamed only with the scenarios that use them.

## Out of scope

- Real OS-level input injection — see the assumption above; revisit only if a bug is traced to the
  platform input layer.
- Touch, pen and gesture input; Fleet has no such affordances.
- Accessibility-tree-driven targeting. GPUI exposes `Window::debug_a11y_tree_json`, but it is only
  captured while a11y is *active* (an AT client must be attached), which makes it an unreliable
  oracle for a harness. Noted here because it is the obvious alternative and was considered; a
  follow-up may revisit it if Fleet adopts a11y roles broadly, since the two goals overlap.

## Affected areas

- `crates/fleet-ui-kit/src/` — the target wrapper and the harness-mode flag it reads.
- `crates/fleet-app/src/views/`, `screens/`, `dialogs/` — target names applied to the surfaces
  scenarios reach.
- `crates/fleet-app/src/state/harness.rs` — the `targets` map in the snapshot; version bump.
- `crates/fleet-drive/` — mouse, clipboard, window and focus commands.
- `crates/fleet-harness/src/` — the scenario lines for them.
- `docs/TESTING-HARNESS.md`, `docs/DESIGN-SYSTEM.md` (the wrapper's contract).

## Tasks

### P3-T01 — The `harness_target` element wrapper
- **Intent:** one reusable way to say "this rect is reachable by name", with no cost when the harness
  is off.
- **Touches:** `crates/fleet-ui-kit/src/`, `crates/fleet-app/src/state/harness.rs`.
- **Steps:**
  - Load `gpui-components` (element and builder conventions) and `gpui-performance` (the cost
    question) before writing it.
  - Add a builder method usable on any styled element that, when harness mode is on, records
    `(name, bounds)` for the frame into a window-scoped side table; when off, it is a no-op that
    compiles away to a flag check.
  - The side table is cleared at the start of each frame and read by the snapshot builder, so
    `targets` always describes the last painted frame — and the snapshot must report that frame's
    number so a stale target is detectable.
  - Follow `fleet-ui-kit`'s rules: no domain types, no literals, and a gallery entry demonstrating
    it if the kit's conventions call for one.
- **Verification:** `make lint`; `make test`; a `#[gpui::test]` renders a tree with two targets and
  asserts their recorded rects; a second test asserts nothing is recorded with harness mode off.
- **Done when:** any element can be named, and naming it costs nothing in production.

### P3-T02 — Name the surfaces scenarios need
- **Intent:** give the corpus something to click.
- **Touches:** `crates/fleet-app/src/views/`, `screens/`, `dialogs/`, `shell/`.
- **Steps:**
  - Apply targets, using a consistent naming scheme (`<surface>.<part>[<index>]`), to: repos rail
    rows and its collapse control; worktree, PR, board and jobs rows; workspace tab strip entries;
    dialog fields and buttons; palette rows; toasts and the sticky error's retry; the agent popup's
    tabs and approval buttons.
  - Row targets carry the row's stable id in the snapshot alongside the name, so a scenario can
    target by index *or* by id.
  - Record the naming scheme in `docs/TESTING-HARNESS.md` as it is established, not afterwards.
- **Verification:** `make lint`; `make test`; a `dump` on each major screen lists the expected target
  names; the names in the docs and in the dump agree.
- **Done when:** every surface a Phase 5 scenario will touch has a name.

### P3-T03 — `targets` in the snapshot
- **Intent:** publish the map, with enough context to catch a stale read.
- **Touches:** `crates/fleet-app/src/state/harness.rs`.
- **Steps:**
  - Add `targets: { name: {x, y, w, h, frame} }` in logical window coordinates, and bump the
    snapshot `version`.
  - Make `fleet-harness` reject a snapshot whose version it does not know, naming both versions.
- **Verification:** `make lint`; `make test`; the shape test from P2-T01 is updated and still pins
  the format.
- **Done when:** a runner can resolve a name to a rect and know which frame it came from.

### P3-T04 — Mouse commands
- **Intent:** move, click, double-click, right-click, press, release, drag — with correct hover state.
- **Touches:** `crates/fleet-drive/src/`, `crates/fleet-harness/src/`.
- **Steps:**
  - Commands: `move <target|x y>`, `click [left|right|middle] [count] <target|x y>`,
    `press`/`release`, `drag <from> <to> [steps]`, `hover <target> [dwell-ms]`.
  - Resolve target names app-side against the current `targets` map; fail with `ok:false` naming the
    unknown target and listing near matches, never silently click at (0,0).
  - Dispatch `PlatformInput::MouseMove` before every press so hover and cursor state update, let a
    frame settle between phases, and carry `click_count` and modifiers correctly.
  - `drag` emits down, N interpolated moves (default 8), then up, so drag thresholds and previews
    behave as they do under a real hand.
  - Every command replies after the resulting frame has been painted, so a following `dump` sees the
    effect.
- **Verification:** `make lint`; `make test`; a scenario clicks a worktree row and `await`s the
  selection changing; a drag moves a board card between columns and the dump shows the new column.
- **Done when:** the mouse vocabulary is complete and target-addressed.

### P3-T05 — Unify scroll
- **Intent:** one scroll command for both apps, aimed at a target.
- **Touches:** `crates/fleet-drive/src/`.
- **Steps:**
  - Replace `wheel`/`hwheel` and the `Dialect` remnant with `scroll <dx> <dy> [at <target|x y>]`,
    keeping the existing pixel-per-row convention so lazygit scripts behave identically.
  - Update any lazygit scenario or doc that used the old spelling, in the same commit.
- **Verification:** `make lint`; `make test`; scrolling a long worktree list moves the viewport and
  the dump reflects it; the lazygit path still scrolls.
- **Done when:** one scroll command serves both consumers and the dialect split is gone.

### P3-T06 — Clipboard, window and focus
- **Intent:** the remaining things a human does that change app behaviour.
- **Touches:** `crates/fleet-drive/src/`, `crates/fleet-app/src/drive.rs`.
- **Steps:**
  - `clipboard set <text>` / `clipboard get` — Fleet's `y` (copy path/URL) is untestable otherwise.
  - `resize <WxH>` — for responsive behaviour and the rail's collapse threshold.
  - `blur` / `focus` — window activation changes toast and attention behaviour
    (`docs/decisions/0012`).
  - Each replies after the resulting frame, like the mouse commands.
- **Verification:** `make lint`; `make test`; a scenario copies a worktree path with `y` and asserts
  the clipboard contents; a resize scenario asserts the rail collapses.
- **Done when:** clipboard, size and focus are all drivable and assertable.

### P3-T07 — Documentation and a keymap cross-check
- **Intent:** keep the docs authoritative, and prove the new vocabulary against the real spec.
- **Touches:** `docs/TESTING-HARNESS.md`, `docs/DESIGN-SYSTEM.md`.
- **Steps:**
  - Document every new command, the target naming scheme, and the frame-staleness rule.
  - Note the target wrapper's contract in `docs/DESIGN-SYSTEM.md` alongside the kit's other
    component contracts.
  - Write one real scenario that exercises a pointer path end to end (open a worktree by clicking it,
    switch tabs by clicking, close with the keyboard) and keep it as the phase's acceptance test.
  - Load `zed-quality-review` before declaring the phase done.
- **Verification:** `make lint`; `make test`; the acceptance scenario passes in both the `virtual`
  and `headless` lanes (the latter without its `shot` lines).
- **Done when:** the docs describe the input vocabulary that exists, and a pointer-driven scenario
  runs green.

## Verification

```sh
make lint
make test
```

Acceptance run:

```sh
cargo build --workspace
./target/debug/fleet-harness run scenarios/pointer-basics.scenario
./target/debug/fleet-harness run scenarios/pointer-basics.scenario --lane headless
```

Both must exit 0; the `virtual` run leaves screenshots showing the clicked selection.

## Definition of done

- [ ] Every task in the tracker is checked off, with its verification output recorded.
- [ ] `make lint` is clean.
- [ ] `make test` passes on a clean tree.
- [ ] The pointer acceptance scenario passes in both lanes.
- [ ] The target recorder is provably inert when harness mode is off (test, and a read of the paint
      path).
- [ ] `docs/TESTING-HARNESS.md` and `docs/DESIGN-SYSTEM.md` match the code.
- [ ] The tracker reflects reality; follow-ups are recorded.

## Risks and rollback

- **The recorder creeps into the frame budget.** It runs in paint; if it ever allocates per element
  or per frame in production, it is wrong. Keep the side table pre-allocated and the flag check
  first. `gpui-performance` is the reviewer here. Rollback: the wrapper is one method; removing its
  call sites restores the old behaviour.
- **Target names rot.** A renamed target silently breaks scenarios. Mitigated by failing with near
  matches rather than a generic error, and by keeping the naming scheme documented in one place.
- **Coordinates go stale between frames.** Every target carries its frame number; a mouse command
  that resolves a target from an older frame than the current one must re-resolve, not click blind.
- **Synthetic events diverge from real ones.** They share GPUI's dispatch path, so divergence would
  be a GPUI bug rather than a harness bug — but it would be invisible here. Accepted deliberately;
  recorded in the decision record from Phase 1.
