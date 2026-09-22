# Board workflows, phase 6: app — the board face — Plan
> Tracker: ./board-workflows-2026-09-20-phase-6-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

The board face learns four states and nothing else. A tile's key line — a bare text child today
(`card_tile.rs:273`) — becomes a row whose right end carries a run mark (gray spinner, amber dot,
faint check) or `⊘ n` when blocked; a column header gains a muted `⚡` when it has an on-enter
action; the pane header gains `1/1 working · 1 needs you`. None of it is computed in `render`: the
app derives a `CardMarks` map in the update path from the mirror and the cards, behind a `revision`
that moves only when a mark differs, and `ProjectionKey` takes it as a fifth input.

## Sizing call

**Phased, phase 6 of 9.** See ./board-workflows-2026-09-20-roadmap.md. One pull request, depending
on phases 1 and 2 and nothing later. Not split further: the map, the memo key and the three surfaces
are one feature that one scenario exercises together; phase 7 reads the same map.

## Repository context

- Kit: `card_tile.rs` struct :64-83, builders :110-180, **key line** :273 in the body column
  :266-283, meta row :212-263; `kanban_column.rs` builders :83-144, header :163-180, count badge
  :180; `Icon::Zap` `icons.rs:140`; marks to copy `agent/rows/delegation.rs:113-135`; gallery
  `examples/gallery_board.rs` :662, :689, `sections` :936-941. `pane_header.rs::trailing` :111
  exists: the header is app-side composition only.
- App: `state/board.rs` `BoardState` :53-83 (`revision` :83), `touch()` :93, `apply_board_view`
  :128-138, `apply_card` :141-173, `board_is_shown()` :328-332, tests `state/board/tests.rs`; mirror
  `state/agents.rs` :58-62, `delegation(id)` :230, `apply_delegation` :295-325, `DelegationChanged`
  → `state/connection.rs:301`.
- Projection: `screens/board/projection.rs:3-48` (`ProjectionKey` :3-20, `source` :38, rebuild
  :43-48, `now`/`focus` not inputs :22-27); `board_screen/model.rs` `CardRow` :242-267/:272-287,
  `HeaderFacts` :176-197/:205-232, `build` :329-355; `board_screen.rs` `header` :196-251,
  `KanbanColumn::new` :280-285, `fn tile` :323-354.
- Harness: `state/harness/projection.rs` lists :559-597 (columns :573-574, cards :577-589),
  `card_row` :923-934, `ProjectionKey` :46-55/:179-188; `fixture.rs` `Preset` :46-63, `all()` :80,
  `fixture/seed.rs::seed_board` :250.
- Skills: `gpui-components`, `gpui-styling`, `gpui-state-and-memory`, `gpui-performance`,
  `rust-gpui-testing` with `docs/TESTING-HARNESS.md` §3 and §9, `zed-quality-review` last. Docs in
  step: `UX-SPEC.md` :1981/:2008, `DESIGN-SYSTEM.md` :344-352, `BOARD.md` §11.

## Assumptions

- **Derived, never on the wire.** `CardMarks` is rebuilt from `BoardView.cards` and `live_runs`,
  joined to the mirror on `CardRun.id`; without `board.automation` the map is empty.
- **`RunMark`/`BlockedTone` are kit enums** (§5.1): no domain type, no literal colour or size.
  `Pending`/`Working` = `Spinner` small `Tone::Secondary`, `NeedsYou` =
  `StatusDot::small(Tone::Warning)`, `Succeeded` = `Icon::Check` faint. Harness additions are
  additive optional data under §3; §2 is untouched but for the preset enumeration.

## Out of scope

- Card detail (7); Board settings (8); keys, palette and the move confirm (9); any daemon change.

## Affected areas

- `crates/fleet-ui-kit/src/components/{card_tile.rs, kanban_column.rs}`,
  `examples/gallery_board.rs`; `crates/fleet-app/src/{state/board.rs, state/board/tests.rs,
  state/harness/projection.rs, screens/board/{projection.rs, tests.rs}, views/board_screen.rs,
  views/board_screen/model.rs}`; `crates/fleet-harness/src/{fixture.rs, fixture/*}`;
  `scenarios/board/workflow-marks.scenario`, `scenarios/baselines/`, `docs/{UX-SPEC.md,
  DESIGN-SYSTEM.md, TESTING-HARNESS.md, BOARD.md}`.

## Tasks

### P6-T01 — `RunMark`, `BlockedTone`, `CardTile::run`/`.blocked`, `KanbanColumn::action`
- **Intent / touches:** The three builders, drawn in the gallery — `components/{card_tile.rs,
  kanban_column.rs}`, `examples/gallery_board.rs`.
- **Steps:**
  - `pub enum RunMark { Pending, Working, NeedsYou, Succeeded }` and `pub enum BlockedTone { Muted,
    Warning }` in `card_tile.rs`, re-exported from the crate root; `CardTile` gains `pub fn
    run(self, mark: RunMark) -> Self` and `pub fn blocked(self, count: u32, tone: BlockedTone) ->
    Self` beside the builders at :110-180.
  - Wrap the key line (:273) in `div().flex().items_center().w_full()`: key text, flex spacer, then
    the mark, else `Text::data_small(format!("⊘ {count}"))` `.muted()` for `Muted` and
    `Tone::Warning` for `Warning`. A run mark wins over a count; tile height and meta row are
    unchanged; every colour, size and gap comes from the theme.
  - `KanbanColumn` gains `action: bool` with `pub fn action(self, has_action: bool) -> Self`,
    drawing a muted `Icon::Zap` after the count badge (:180). Gallery: `card_tile_section` gains one
    tile per `RunMark` and per `BlockedTone`, `live_board_section` one column `.action(true)`.
- **Verification:** `cargo test -p fleet-ui-kit`, the gallery looked at once, `make lint`.
- **Done when:** Every mark and both tones are in the gallery, with no literal colour or size.

### P6-T02 — `CardMarks`, `TileMarks` and `refresh_card_marks`
- **Intent / touches:** Derive the state once per change in the update path — `state/board.rs`,
  `state/board/tests.rs`, `state/connection.rs`.
- **Steps:**
  - Per §5.2: `CardMarks { by_card, working, needs_you, revision }` and `TileMarks { run:
    Option<RunMark>, blocked: Option<(u32, BlockedTone)> }` on `BoardState`, with
    `AppState::refresh_card_marks(&mut self, now)`.
  - Per card: `pending_run` first, else `runs.last()` joined to `live_runs` and the mirror —
    `Starting|Running|Settling → Working`, `Blocked → NeedsYou`, terminal `needs_attention() →
    NeedsYou`, `Succeeded → Succeeded` only while the column has no `on_success`, `Cancelled →
    None`; a pending run older than `PENDING_AMBER_AFTER_SECS` is `Stalled`, else `Pending`
    (contracts §5.1).
    Blocked pair from `ops::query::blocked`, `needs_you` from `attention(card, now)`.
  - Build into a local, compare, assign and bump `revision` **only** on inequality; never `touch()`
    (that revision is the document's). Call sites: end of `apply_board_view`, end of `apply_card`,
    after a card-called `DelegationChanged` (`state/connection.rs:301`) guarded by
    `board_is_shown()`, and from the app's existing clock tick — the one that runs `expire_toasts`
    (`notifications.rs:43`; confirm the caller with `grep -rn "expire_toasts" crates/fleet-app/src`)
    — so `Stalled` appears without an event. Format `now` to RFC 3339 once for `attention`.
  - Failed start: `apply_board_view` keeps the previous view's newest run id per card while it swaps
    views; a card whose newest run id is new and `CardRun::failed_to_start()` (`thread_id: None`)
    raises `StickyError { text: detail, job: None, retryable: false }` through
    `screens::board::lifecycle::fail` (:224-233), once per run id (contracts §5.2).
  - Tests: live run → `Working`; blocked child → `NeedsYou`; pending past sixty seconds →
    `Stalled`, before it `Pending`; a new failed-to-start run raises the sticky error once; two unsatisfied blockers → `(2, Muted)`, a canceled one →
    `Warning`; **a headline-only `DelegationChanged` leaves `revision` equal**; no capability →
    empty.
- **Verification:** `cargo test -p fleet-app state::board`, `make lint`.
- **Done when:** The seven tests pass and no mark is computed anywhere else.

### P6-T03 — The memo key, `CardRow.run`/`.blocked`, tiles, `⚡` and the header counts
- **Intent:** Feed the face from the map through the projection: one key input, nothing in `render`.
- **Touches:** `screens/board/{projection.rs, tests.rs}`, `views/board_screen.rs`, its `model.rs`.
- **Steps:**
  - `ProjectionKey` gains `marks: u64` = `CardMarks.revision`, beside `source` (:38) and in the
    rebuild guard (:43-48); `now` and `focus` stay out.
  - `CardRow` gains `run` and `blocked`, filled in `CardRow::of` from the map handed to `build`;
    `HeaderFacts` gains `working`, `live_limit` (`settings.max_live_runs()`) and `needs_you`, both
    header strings built in the projection, not in `header`.
  - `fn tile` chains `.run(..)`/`.blocked(..)` with `when_some`; `KanbanColumn::new` chains
    `.action(column.has_action)` from `automation.on_enter.is_some()` carried on `ColumnRows`; `fn
    header` prepends `1/1 working` (`Tone::Secondary`) and `1 needs you` (`Tone::Warning`) to the
    trailing cluster before `Badge::new(facts.prefix)` (:205), each only when non-zero.
  - Test beside `screens/board/tests.rs:267`: **"board model is not rebuilt when a live child's
    headline changes"** — project, apply a headline-only `DelegationChanged`, assert the key is
    equal and the cached model reused, then change the delegation's status and assert a rebuild.
- **Verification:** `cargo test -p fleet-app screens::board`, `make lint`.
- **Done when:** The memo test passes and no `render` body reads `CardMarks`.

### P6-T04 — Additive harness marks, `board.summary`, `docs/TESTING-HARNESS.md` §3
- **Intent / touches:** Assert all four states without touching the frozen grammar —
  `state/harness/projection.rs`, `docs/TESTING-HARNESS.md` §3.
- **Steps:**
  - `fn card_row` (:923-934): append one of `working`, `pending`, `needs you`, `done` for a run mark
    and `blocked:n` for a count (run mark first), keeping the assignee mark and key badge; column
    rows (:573-574) append `action`.
  - New list `board.summary`: one row whose `label` is the header's `1/1 working · 1 needs you`
    text, absent when both counts are zero. The harness `ProjectionKey` (:46-55, :179-188) gains the
    marks revision, so a mark change wakes a waiting `await` and nothing else does.
  - §3: extend the list-names sentence (:229-241) and state the new `marks` vocabulary under the
    additive rule (:193-195); §2 and target names unchanged.
- **Verification / done when:** `cargo test -p fleet-app harness`, then a throwaway scenario whose
  `dump` (§9.4) shows the marks and the summary row; the docs diff touches §3 only.

### P6-T05 — A fixture that can show a run, and `scenarios/board/workflow-marks.scenario`
- **Intent:** Prove the four states in the real GUI, per the repo rule.
- **Touches:** `fleet-harness/src/fixture.rs`, `fixture/{plan.rs, seed.rs, tests.rs}`,
  `docs/TESTING-HARNESS.md` §2 preset enumeration and §4, `scenarios/board/workflow-marks.scenario`,
  `scenarios/baselines/`.
- **Steps:**
  - Add preset `board-workflow` on the `agents-subagent` precedent (`Preset` :46-63, `as_str`
    :66-78, `all()` :80 becomes eight): the `agents` seeding (scripted provider under the vendor
    names, the `fleet` shim on the child's `PATH`) **plus** a worktree-scoped board with the
    workflow preset columns, `max_live_runs = 1` and four cards — `FLT-1`, `FLT-2`, `FLT-3` in Todo,
    `FLT-4` blocked by the first two — through a worktree-scoped sibling of `seed_board` (:250). No
    card is seeded into an action column: a run started by the seeding daemon does not survive it.
  - Live states come from the running daemon as in
    `scenarios/agents/subagent-runs-end-to-end.scenario`: In progress is claude with a long-running
    child, In review is codex with a child that reports blocked, and moving a card in starts a run.
  - Scenario: `fixture: board-workflow`; `key o`; `key ctrl-s b`; `]` `FLT-1` into In progress and
    `await lists["board.cards"].rows[0].marks[0] == "working"`; `FLT-2` in and await `pending` (the
    throttle is 1); drive `FLT-3` to review and await `needs you`; assert `FLT-4` carries
    `blocked:2`, a column row's `marks[0] == "action"` and `lists["board.summary"].rows[0].label ~=
    "working"`; one `shot workflow-marks`; `quit`. Head the file with its plan reference and the
    virtual-lane-only reason.
- **Verification:** `make harness-one SCENARIO=scenarios/board/workflow-marks.scenario`, then
  `LANE=headless` to confirm the `shot` is the only skip, then `make harness`; baseline only after
  opening the image (§9.8).
- **Done when:** It passes in the virtual lane with all four marks asserted from the snapshot.

### P6-T06 — `docs/UX-SPEC.md` Board chapter and `docs/DESIGN-SYSTEM.md` glyphs
- **Intent / touches:** Documents in step — `docs/UX-SPEC.md` (:1981, :2008),
  `DESIGN-SYSTEM.md` (:344-352), `BOARD.md` §11.
- **Steps:**
  - "The pane": the key line is a row; run mark and `⊘ n` share its right end, run mark winning; the
    column `⚡`; the header counts left of the prefix badge, only while non-zero. "States": one
    sentence per mark naming what the user does next, plus one that a board with no automation is
    unchanged.
  - DESIGN-SYSTEM's status glyph table gains the spinner, the amber dot, the faint check and `⊘`,
    each naming its tone token and pointing at `NATIVE-AGENTS.md` §2 (no new token); `BOARD.md` §11
    gains "On the board face", four sentences, cross-referencing UX-SPEC.
- **Verification / done when:** `git diff docs/` beside the scenario; every assertion in it has a
  sentence in one of the three documents.

## Verification

```sh
make lint && make test && make harness
```

`make harness` is required: this phase changes a screen. Run `workflow-marks.scenario` alone first.

## Definition of done

- [ ] Every task above is `[x]` in the tracker; `make lint` and `cargo check --workspace
      --all-targets` are clean; `make test` passes, including the gallery and memo tests.
- [ ] `make harness` passes with `workflow-marks.scenario` and its baseline; `TESTING-HARNESS.md`
      changed only under §3's additive rule plus the agreed §2/§4 preset enumeration, and
      `UX-SPEC.md`, `DESIGN-SYSTEM.md` and `BOARD.md` describe what runs.
- [ ] No mark is computed in a `render` body; `CardMarks.revision` moves only on a real change, and
      the tracker reflects reality with follow-ups captured.

## Risks and rollback

- **A notify storm from the mirror.** A live child emits `DelegationChanged` per tool call; the
  guards are P6-T02's inequality check and P6-T03's memo test, and a rebuild that survives them is
  an extra `ProjectionKey` input, not a map bug.
- **New surface.** Keep `board-workflow` to what the scenario needs, and check the gallery and the
  `shot` before recording a baseline: wrapping the key line must not change tile height.
- **Rollback.** Revert the PR: the face returns to today's tiles and a phase-3 daemon keeps driving
  the board headlessly.

## Contract resolutions

Resolved in the contracts file on 2026-09-20; the contracts wording wins over any earlier phrasing above.

1. A third additive preset `board-workflow` joins the §2 `fixture:` enumeration exactly as `agents-subagent` did in commit 0b76217 (described under §4 as additive); it seeds what `agents-subagent` seeds plus a worktree board on `acme/api#agent` with the workflow preset, `max_live_runs = 1`, four linked cards, and the two child transcripts (contracts §5.6). P6-T05 owns it; it is not a stop-and-ask.
2. `RunMark` gains `Stalled` (amber dot, harness word `stalled`) for a pending run older than `PENDING_AMBER_AFTER_SECS`; `NeedsYou` keeps its one meaning (contracts §5.1).
3. `refresh_card_marks` is also called from the app's existing clock tick (the one running `expire_toasts`) and formats `now` to RFC 3339 once for `attention` (contracts §5.2).
4. The failed-start sticky error fires in `apply_board_view` for a card whose newest run id is new and `failed_to_start()` (`thread_id: None`) (contracts §1.3, §5.2).
