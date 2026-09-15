# Test-harness review fixes — Phase 2: an oracle that is not true by construction — Plan
> Tracker: ./harness-review-fixes-2026-09-14-phase-2-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

`UiSnapshot` is the harness's oracle: every `assert` and `await` in the 41-scenario corpus reads a
dotted path out of it. A two-reviewer adversarial review found eight places where that oracle
reports something other than what the UI is doing. Two of them make shipped scenarios vacuous — the
Jobs overlay's `focused` and `lists.jobs.selected` are provably pinned at `0` forever, and
`rail-collapse.scenario` never asserts the collapse it exists to pin, on a premise about the
predicate grammar that is simply false. One makes a whole class of assertion an unconditional pass:
`dialog.fields` and `dialog.message` are always empty, so `assert dialog.fields[0] absent` passes
even on a dialog showing a populated field.

This phase makes the snapshot report what the UI actually shows where that is the right fix, and
makes §3 and §11 of `docs/TESTING-HARNESS.md` honest about the places it deliberately will not. The
repo's rule is that `docs/` is authoritative rather than descriptive — when code and a doc disagree,
one of them is a bug and both are fixed in the same commit — so "which side moves" is a decision
each task states rather than a choice left to the implementer.

## Sizing call

**Phased**, and this is phase 2 of 4 — see
[the roadmap](./harness-review-fixes-2026-09-14-roadmap.md). Eight issues: three P2 and five P3.
The work is one focused stretch of a few days because it wants a single reviewer holding the whole
§3 contract in their head — the projection, its memoisation key, and the scenarios that read it —
rather than six people each touching one field. It is not split further because `P2-T01`'s mirrored
cursor and `P2-T07`'s `ProjectionKey` addition are the same mechanism (an input to the projection
that must appear in its key) and reviewing them together is what catches a third one.

## Repository context

- **Project type:** Rust, one Cargo workspace, `resolver = "3"`, edition 2024, toolchain pinned by
  `rust-toolchain.toml`. Twelve crates under `crates/`.
- **Lint command:** `make lint` = `cargo fmt --all -- --check` plus
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **Test command:** `make test` = `cargo build -p fleet-daemon -p fleet-app -p fleet-harness`, then
  `cargo test --workspace` with `FLEET_DAEMON`, `FLEET_APP` and `FLEET_HARNESS_BIN` pointed at
  `target/debug/`.
- **No separate type-check step.** `cargo check --workspace --all-targets` (`make check`) covers it.
- **GUI harness:** `make harness` (whole corpus, `virtual` lane),
  `make harness-one SCENARIO=<path> LANE=virtual|headless|attach`, `make harness-headless` (the
  pixel-free subset, currently empty because every corpus scenario takes a `shot`).
- **The projection:** `crates/fleet-app/src/state/harness/projection.rs` builds `UiSnapshot`;
  `projection_key` at `:157-181` lists the eighteen inputs the memoisation is keyed on, and the
  module's own rule at `:30-32` says every input must appear there. Tests sit in
  `crates/fleet-app/src/state/harness/tests.rs`.
- **The contract:** `docs/TESTING-HARNESS.md` §3 freezes `UiSnapshot` version 1 and its field
  order (pinned by a serialization test); §11 "Known gaps" records every verified place reality
  falls short of §3. Append to §11 in the shape the existing entries use — a verified limit, its
  mechanism, and its workaround.
- **The `virtual` lane needs a live, unlocked Hyprland session and a hand-exported environment.**
  `export XDG_RUNTIME_DIR=/run/user/1000`, probe each directory under `/run/user/1000/hypr/` for the
  signature whose socket actually answers `hyprctl monitors -j` (the newest is frequently dead),
  export it as `HYPRLAND_INSTANCE_SIGNATURE`, and `export WAYLAND_DISPLAY=wayland-1`. On a locked
  screen every `shot` is refused by the empty-output guard (§11).
- **Skills to load before editing** (`CLAUDE.md`'s table): `gpui-state-and-memory` for the mirrored
  cursor and any new projection input, `gpui-performance` for anything touching a `render` path or a
  per-frame projection, `rust-gpui-testing` for every test added here, `zed-quality-review` before
  calling the phase done.
- **Prerequisite:** `P1-T01` must be committed before this phase starts, or `make test` cannot pass
  and no verification here means anything.

## Assumptions

- The "Which side moves" line in each `plans/issues.md` entry is authoritative. This phase moves
  the **code** for `I4`, `I28` and `I31`, the **scenario** for `I10`, the **doc** for `I29`, `I30`
  and `I32`, and leaves `I5` as an explicit decision the implementer records.
- `UiSnapshot` stays at version 1. Populating an already-frozen field (`dialog.fields`) or changing
  what an existing field reports (`lists.jobs`) is not a version bump; adding a field would be, and
  no task here adds one.
- Phase 1 has landed. If `P1-T03` or `P1-T05` added an idle constituent, §3's `idle` description
  already reflects it and this phase does not renumber it.
- `scenarios/baselines/virtual/` stays empty. `P2-T03` adds structural assertions to
  `rail-collapse.scenario`, not a recorded baseline.
- `I31`'s fix is a one-line key addition. The finding's claimed hanging `await` was shown
  unreachable on every in-tree path, so this is a letter-of-the-contract fix plus one narrow
  self-healing race, and it is not worth a larger refactor.

## Out of scope

- Recording screenshot baselines, and the §11 compositor work that would make recording one
  possible.
- Adding CI.
- Splitting `fleet-drive/src/{input,predicate}.rs` or the three `fleet-harness` files past the
  ~900-line rule (§11 records them as a deliberate future refactor).
- Painting `dialog.button[N]` targets. §3's own target table already documents that they are painted
  nowhere, so an empty `buttons` array is accurate — `plans/issues.md` explicitly strikes that third
  of `I5`.
- Phase 1's idle and shutdown work, Phase 3's scripted agent player, Phase 4's run-directory,
  report and baseline long tail.

## Affected areas

Single repository (`fleetd` workspace).

- `crates/fleet-app/src/state/harness/projection.rs` — `focused` (`:319`), `lists.jobs`
  (`:433`), the dialog block (`:367`), the terminal tab label branch (`:641`), and
  `projection_key` (`:157-181`).
- `crates/fleet-app/src/state/snapshot.rs` — `clamp_cursor` (`:87`), the single writer of
  `cursors.jobs`.
- `crates/fleet-app/src/screens/jobs.rs` — `JobsPanelState::cursor` (`:44`) and the `JobFilter`, the
  real selection the projection cannot see.
- `crates/fleet-app/src/drive.rs` — `wait_for` (`:651`) and its doc comment (`:640-648`);
  `Harness::paint` call sites (`:366`, `:482`).
- `crates/fleet-app/src/state/terminal.rs` — the two trims at `:183` and `:191`.
- `crates/fleet-harness/src/scenario.rs` — `raw.trim()` at `:762`, for `I32`'s doc-side decision.
- `crates/fleet-ui-kit` target painters — `worktrees_list.rs:347`, `prs_screen.rs:293`,
  `repos_rail.rs:317`, `jobs/presentation.rs:245` (read, for `I29`'s doc).
- `docs/TESTING-HARNESS.md` — §2 (the `type` whitespace sentence at :117), §3 (target indices,
  `terminal.rows`), §11 (new entries).
- `scenarios/hub/rail-collapse.scenario`, `scenarios/hub/jobs-panel.scenario`.

## Tasks

### P2-T01 — Mirror the Jobs panel's real selection into `AppState` (I4)

- **Intent:** make the Jobs overlay's `focused` and `lists.jobs.selected` track the row the user is
  actually on, so the two scenario lines that read them stop being true by construction.
- **Touches:** `crates/fleet-app/src/state/harness/projection.rs`,
  `crates/fleet-app/src/state/snapshot.rs`, `crates/fleet-app/src/screens/jobs.rs`,
  `scenarios/hub/jobs-panel.scenario`.
- **Steps:**
  - Confirm the proof before changing anything: `projection.rs:319`
    (`focused` → `format!("jobs.row[{}]", self.cursors.jobs)`) and `:433` (`lists.jobs.selected`)
    both read `AppState::cursors.jobs`; that field has exactly three references workspace-wide and
    its single write is `clamp_cursor` (`state/snapshot.rs:87`), a monotonically non-increasing
    function on a field that starts at `0`. It is pinned at `0` forever. The real selection lives in
    `JobsPanelState::cursor` (`screens/jobs.rs:44`).
  - Mirror the panel's selection into `AppState` on every cursor move and every filter change —
    either the visible-row index or the selected `JobId`; prefer the `JobId` if the snapshot can
    express it, because it survives a filter change that the index does not.
  - Build `lists.jobs` from the same `JobFilter`-filtered row set the panel renders. Today it is
    built from the unfiltered `snapshot.jobs`, so `jobs.row[N]` (the click target) and
    `lists.jobs.rows[N]` (the oracle) address different jobs whenever a filter is on.
  - Add the mirrored value to `ProjectionKey` (`projection.rs:157-181`) — the module's rule at
    `:30-32` requires it, and without it the memoised projection will not refresh on a cursor move.
  - Correct the module header's claim that "nothing here is a second source of truth", which this
    mirror makes true again rather than false.
  - Make `scenarios/hub/jobs-panel.scenario:15` and `:29` meaningful: today they are vacuous, so the
    line that exists to prove `Esc` collapsed a log *without* closing the panel proves nothing.
    Assert on a non-zero selection after a cursor move, and on the filtered list after a filter.
  - Add tests: cursor move changes `focused`; filter change re-bases `lists.jobs`; `jobs.row[N]` and
    `lists.jobs.rows[N]` address the same job under a filter.
- **Verification:** `cargo test -p fleet-app <test names>`; `make test`; then
  `make harness-one SCENARIO=scenarios/hub/jobs-panel.scenario LANE=virtual` and the full
  `make harness`. Paste the jobs-panel scenario's report verdict into the tracker.
- **Done when:** moving the Jobs cursor changes `focused`, applying a filter re-bases `lists.jobs`,
  and `jobs-panel.scenario` fails if `Esc` closes the panel.

### P2-T02 — Stop `absent` over `dialog.fields` and `dialog.message` being an unconditional pass (I5)

- **Intent:** close the gap between §3's frozen `dialog` shape and a projection that returns empty
  values unconditionally, either by populating them or by recording the limit in §11.
- **Touches:** `crates/fleet-app/src/state/harness/projection.rs`, `docs/TESTING-HARNESS.md` (§11,
  and §3 if the code moves).
- **Steps:**
  - Confirm the defect: `projection.rs:367` returns `fields: Vec::new(), … message: None`
    unconditionally, while §3 freezes `dialog` as
    `{name, fields:[{name,value,focused}], buttons:[string], message:string|null}`. §11, which
    exists to record exactly this kind of shortfall, has no dialog entry.
  - Understand why it is dangerous rather than merely incomplete: every
    `assert dialog.fields[0] absent` and `assert dialog.message absent` passes unconditionally,
    *including on a dialog that does show a populated field or an error message*. Six dialogs paint
    clickable `dialog.field[N]` targets, so a scenario can click a field it can never read.
  - Do **not** touch `buttons`. `plans/issues.md` strikes that third of the finding: §3's own target
    table already states `dialog.button[N]` is painted nowhere, so an empty `buttons` is accurate.
  - Decide and record in the tracker's decisions log which side moves:
    - **Doc:** add a §11 entry stating that `dialog.fields` and `dialog.message` are always empty
      today, and that dialog content is asserted via `targets["dialog.field[N]"]` plus keystrokes.
      Cheap, honest, and leaves the `absent` trap in place for anyone who does not read §11.
    - **Code:** mirror the open dialog's field triples (`name`, `value`, `focused`) and its message
      into `AppState`, populate them in the projection, and add them to `ProjectionKey`. More work,
      and it is what makes `absent` mean something.
    - Prefer the code fix if the dialog state is reachable from `AppState` without a second source
      of truth; fall back to the doc entry and say why.
  - If the code moves, add a test per dialog kind that a populated field is visible in the snapshot
    and that `absent` is false over it. If the doc moves, add a test that the §11 entry's claim
    holds — that the fields really are empty — so the entry cannot silently rot.
- **Verification:** `cargo test -p fleet-app <test names>`; `make test`; then `make harness` over
  the scenarios that open a dialog. If the doc moved, verify by reading §11 back against
  `projection.rs:367` and pasting both into the tracker.
- **Done when:** either `dialog.fields`/`dialog.message` report the open dialog's content, or §11
  records that they do not and names the working alternative.

### P2-T03 — Assert the rail collapse in `rail-collapse.scenario` (I10)

- **Intent:** make the one Hub affordance whose entire effect is geometry actually assert its
  geometry, and delete the false premise in the scenario header that says it cannot.
- **Touches:** `scenarios/hub/rail-collapse.scenario`.
- **Steps:**
  - Read the false premise at `scenarios/hub/rail-collapse.scenario:7`: it claims the rail width
    "is in the snapshot … but no predicate can read it, because a target name contains the `.` a
    dotted path splits on". That is wrong. §2 freezes a quoted-step form for exactly this case,
    `parse_bracket` (`fleet-drive/src/predicate.rs:413`) implements it, and
    `scenarios/hub/pointer-tabs.scenario:13` already asserts `targets["repos.rail"].w == 240`. The
    rail target is painted on both branches (44 collapsed / 240 expanded).
  - Add `await targets["repos.rail"].w == 44` after the first `key H` and
    `await targets["repos.rail"].w == 240` after the second.
  - Delete the incorrect paragraph from the header and replace it with what the scenario now pins.
  - Confirm the scenario fails if `H` stops collapsing. Today it asserts only what a collapse
    *preserves* — rows, cursor, key context — all of which stay true if `H` does nothing at all, and
    its remaining evidence is a `shot` compared against a baseline that does not exist (§11).
    Temporarily neuter the `H` binding locally, run the scenario, confirm red, and restore.
- **Verification:** `make harness-one SCENARIO=scenarios/hub/rail-collapse.scenario LANE=virtual`
  passes; the same command with the `H` binding neutered fails on the new assertion. Paste both
  verdicts into the tracker — the red one is the evidence that matters.
- **Done when:** `rail-collapse.scenario` asserts both widths and fails if the collapse stops
  happening.

### P2-T04 — Repaint on each headless `await` iteration (I28)

- **Intent:** stop `targets` and `window.frame` freezing during a headless `await`, or document the
  restriction where §11 records it.
- **Touches:** `crates/fleet-app/src/drive.rs`, `docs/TESTING-HARNESS.md` (§11).
- **Steps:**
  - Read the narrowing first — this is latent and unreached, and the fix must not cost more than the
    defect. `wait_for` (`drive.rs:651`) re-runs `project()`, which only *copies*
    `fleet_ui_kit::harness::painted(window)`; nothing paints headlessly unless `Harness::paint` is
    called, and it is called once, before the loop (`drive.rs:366`/`:482`). But `wait_for`'s own doc
    comment (`:640-648`) already tells authors not to await on `targets`/`window`, the one headless
    scenario touching targets uses `assert` (which paints first), only three scenarios run headless,
    and three repro attempts came back negative. Only `window.frame` freezes, not the rest of
    `window`.
  - Choose: call `self.paint(cx)` at the top of each `wait_for` iteration when headless, or add the
    §11 entry. If you take the paint, measure — a repaint per iteration on every headless `await` is
    a cost paid by every scenario to fix a case none of them hit.
  - Either way, correct `wait_for`'s doc comment, which overclaims: it says the values are "read as
    of the moment this call projects them", which is exactly what is not true of `window.frame`
    headlessly.
  - If the doc moves, the §11 entry must name `window.frame` specifically rather than `window`, per
    the narrowing.
- **Verification:** `cargo test -p fleet-app <test name>` if a test is added; `make test`; then
  `make harness-one SCENARIO=<a headless scenario> LANE=headless` for each of the three scenarios
  that run headless. Note the paint cost in the tracker if you took the code fix.
- **Done when:** either a headless `await` sees fresh geometry, or §11 and the doc comment both say
  it does not and name `window.frame` as the field affected.

### P2-T05 — Document that target indices are model positions (I29)

- **Intent:** record the undocumented rule that a scrolled-out row has no target at all.
- **Touches:** `docs/TESTING-HARNESS.md` (§3).
- **Steps:**
  - Confirm the mechanism: `worktrees_list.rs:347`, `prs_screen.rs:293`, `repos_rail.rs:317` and
    `jobs/presentation.rs:245` each pass the index that `uniform_list`/`gpui::list` hands the
    builder, which is the **model** index. Only visible rows are painted, so a row scrolled out of
    view has no entry in `targets` whatsoever.
  - Add that rule to §3 beside the target table: indices are model positions, and
    `targets["<list>.row[N]"]` is absent for a row outside the viewport. A scenario that wants to
    click row 40 must scroll it into view first.
  - Do **not** rewrite §3's "current visual order" sentence as the finding suggests — the narrowing
    records that as an ambiguous sentence the reviewer over-read. The substantive undocumented rule
    is the scrolled-out-row one, and that is the whole of this task.
- **Verification:** read §3 back against the four painter call sites and confirm the sentence
  describes what they do. `make lint` (formatting only — no code changes). Verified: by inspection;
  say so in the tracker.
- **Done when:** §3 states that target indices are model positions and that a scrolled-out row has
  no target.

### P2-T06 — Document `terminal.rows`'s two trims (I30)

- **Intent:** make §3 say that `rows.len()` need not equal `viewport.rows`.
- **Touches:** `docs/TESTING-HARNESS.md` (§3).
- **Steps:**
  - Confirm both trims are deliberate and pinned: `state/terminal.rs:183` truncates trailing spaces
    within a row and `:191` pops trailing empty rows; `state/harness/tests.rs:269-279` asserts
    `rows.len() == 1` with `viewport.rows == 2`. The code is correct; §3 is what is missing.
  - State both trims in §3's `terminal` description, and state the consequence explicitly:
    `rows.len()` is the count of non-empty rows, not the viewport height, so a scenario must not
    index `terminal.rows[viewport.rows - 1]`.
  - Do **not** repeat the finding's right-aligned-column claim — the narrowing refutes it. Interior
    unset cells still become spaces, so column alignment does survive the round trip. Only the
    row-count half is real.
- **Verification:** read §3 back against `state/terminal.rs:183` and `:191` and against the pinning
  test. `make lint`. Verified: by inspection; say so in the tracker.
- **Done when:** §3 documents both trims and the `rows.len() != viewport.rows` consequence.

### P2-T07 — Add `renamed_terminals` to `ProjectionKey` (I31)

- **Intent:** restore the module's own invariant that every projection input appears in its
  memoisation key.
- **Touches:** `crates/fleet-app/src/state/harness/projection.rs`.
- **Steps:**
  - Confirm the violation: `projection.rs:641` branches on `self.renamed_terminals` to choose a tab
    label, and `projection_key` (`:157-181`) lists eighteen inputs and not this one, against the
    module's rule at `:30-32`.
  - Read the narrowing so the fix stays one line: the refutation largely succeeded. All three
    mutators are paired with a keyed change — `mark_renamed` → `close_overlay()` flips five
    `Derived` fields in the same closure, `clear()` → `link_generation`, `retain()` →
    `snapshot_revision` — so the claimed hanging `await` is unreachable on every in-tree path. What
    survives is a letter-of-the-contract violation plus one narrow self-healing race (`Esc` during
    an in-flight rename). Do not refactor the mutators.
  - Add `renamed_terminals` to `ProjectionKey` and to `projection_key`'s construction.
  - Add a test that a rename alone — with no other keyed change — produces a fresh projection, since
    no corpus scenario can reach this path.
- **Verification:** `cargo test -p fleet-app <test name>`; `make test`. No harness run needed; the
  path is unreachable from the corpus, which is why the test is the whole verification.
- **Done when:** `renamed_terminals` is in the key and a rename-only change invalidates the memo,
  pinned by a test.

### P2-T08 — Correct §2's whitespace-preservation claim (I32)

- **Intent:** make §2 describe the parser that exists rather than one that preserves trailing
  whitespace.
- **Touches:** `docs/TESTING-HARNESS.md` (§2), `crates/fleet-harness/src/scenario.rs` (read).
- **Steps:**
  - Confirm the site: `scenario.rs:762` (**not** `:768`, as the finding says) hands `raw.trim()` to
    `parse_instruction`, against §2:117's "arguments after `type` … preserve spaces".
  - Move the **doc**, not the code. `plans/issues.md` is explicit about this: a trailing space in a
    committed scenario file survives no toolchain — no editor, no formatter, no reviewer — so
    preserving it is not a property worth having.
  - Rewrite §2:117 to say that interior spaces are preserved and leading and trailing whitespace is
    trimmed, for both `type` and `clipboard set`.
  - Check whether any corpus scenario depends on a trailing space today; if one does, it is a bug in
    that scenario and it is fixed here.
- **Verification:** read §2 back against `scenario.rs:762`. `make lint`; `make test` for the
  corpus-scenario check. Verified: by inspection plus the scenario grep; paste the grep into the
  tracker.
- **Done when:** §2 describes the trim, and no corpus scenario relies on trailing whitespace.

## Verification

Run from the workspace root, in this order:

```sh
make lint     # cargo fmt --all -- --check + clippy --workspace --all-targets --all-features -D warnings
make test     # builds fleet-daemon, fleet-app, fleet-harness, then cargo test --workspace
make harness  # the whole scenarios/ corpus in the virtual lane
```

`make harness` needs a live, **unlocked** Hyprland session with `WAYLAND_DISPLAY`,
`XDG_RUNTIME_DIR` and a *probed* `HYPRLAND_INSTANCE_SIGNATURE` exported by hand — see Repository
context. This phase changes the oracle every scenario reads, so a corpus run is not optional: a
projection change that breaks one scenario's assertion is exactly the failure `make test` cannot
see. If you cannot get a compositor, say so in the tracker rather than recording the phase as
harness-verified.

## Definition of done

- [ ] Every task `P2-T01`…`P2-T08` is checked off in the tracker, each with pasted verification
      output or a one-line "verified: <how>".
- [ ] `make lint` is clean.
- [ ] `make test` passes.
- [ ] `make harness` runs the full corpus green, or the tracker records exactly why it could not run
      and what was verified instead.
- [ ] `docs/TESTING-HARNESS.md` §2, §3 and §11 match the code: the `type` trim, the model-index
      rule, `terminal.rows`'s two trims, and whichever of `I5`/`I28` was resolved doc-side.
- [ ] `scenarios/hub/jobs-panel.scenario` and `scenarios/hub/rail-collapse.scenario` fail when the
      behaviour they pin is removed — demonstrated, not assumed.
- [ ] Every fix that is latent or unreachable from today's corpus (`I28`, `I31`) carries a test
      written with it.
- [ ] Every new projection input appears in `ProjectionKey`, per the module rule at
      `projection.rs:30-32`.
- [ ] `zed-quality-review` has been run over the phase's diff.
- [ ] The tracker reflects reality, including the `I5` and `I28` code-vs-doc decisions.
- [ ] Follow-ups discovered mid-flight are captured in the tracker's Follow-ups section.

## Risks and rollback

- **Mirroring the Jobs cursor can reintroduce a second source of truth.** `P2-T01` exists because
  `AppState` cannot see `JobsPanelState::cursor`; the fix is a mirror, and a mirror that is written
  in one place and read in another is exactly what the module header warns against. Keep the panel
  the owner and `AppState` the mirror, update it in the same closure as the cursor move, and let the
  test for "cursor move changes `focused`" be the thing that catches a missed write site.
- **A `ProjectionKey` addition that is wrong in the other direction costs frames.** Adding an input
  that changes every frame would defeat the memoisation and put work back in the render path, which
  `docs/APP-CONTRACTS.md` forbids. `renamed_terminals` and a row index are both stable; a `JobId`
  clone per key is not obviously free. Load `gpui-performance` and check what the key costs to
  build.
- **`P2-T03`'s red-run demonstration touches the keymap.** Neutering the `H` binding to prove the
  scenario fails is a local, temporary edit that must not be committed. Do it, capture the verdict,
  and `git checkout` the keymap before committing anything.
- **Doc-only tasks are the easiest to mark done without doing.** `P2-T05`, `P2-T06` and `P2-T08`
  have no command that can fail. Their verification is reading the doc back against the named
  `file:line` and pasting both into the tracker; a bare tick on those three is the most likely way
  this phase ends up wrong.
- **No CI catches any of this.** There is no `.github/workflows` in the repo, so the Verification
  block above is the only gate.
