# Board workflows, phase 7: app — the card detail — Tracker
> Plan: ./board-workflows-2026-09-20-phase-7-plan.md
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
  (Not tickable by any phase-7 owner, for phase 5's reason: the tree was never clean — the nine
  phases were built in parallel in one worktree — and `make lint`/`make test` belong to the
  integration stage rather than to a task. Ticked here 2026-09-21 by the phase-9 docs audit, which
  reconciled every tracker with what the tree actually holds.)
- [x] I am ready to start.

## Tasks
- [x] P7-T01 — The run row and report comments that collapse
  - verified: `views/board_card_detail.rs` gained `RunLine`/`run_line`/`run_row` and the report
    badge and fold; `dialogs/card_detail/view.rs` draws the row between the title and the
    description from prepared strings only, and no hint line (phase 9 owns it).
    `cargo test -p fleet-app --lib card_detail` — 28 passed 0 failed, of which 9 are T01's: the
    live row, the four live state words plus the `working` fallback, the owed run winning over a
    finished one, a terminal row with tokens and cost (and a live one without), every outcome
    word with a missing model and effort dropping their separators, a card with no run and none
    owed having no row, the `run {n}` number and the eight-line fold, and `⏎` unfolding before it
    edits a property.
- [x] P7-T02 — Provider, Model, Effort, Blocked by and Blocks property rows
  - verified: `property_rows` now splices `workflow_rows` in directly under Status. Same suite:
    no agent row on a column without an action; an inherited value carrying the muted
    `column default` source and a card-level one carrying none; a skill column showing `claude`
    over a card that asked for Codex; a satisfied blocker checked and a canceled one amber, one
    label for the group; `Blocks` as the reverse of everyone else's blockers, with the em-dash
    row once the board uses links at all; and every workflow row locked — picker target intact —
    on a backend that owns `agent` and `blocked_by`.
- [x] P7-T03 — Four `PickerKind`s, disabled rows and the two apply paths
  - verified: the skeleton's three `// CONTRACT STUB (a:picker)` arms are gone — `schema::
    options` answers for all five kinds, `current_value` answers for the three agent kinds and
    `request_for` builds their patches. `cargo test -p fleet-app --lib card_picker` — 27 passed
    0 failed, of which 11 are new: the five labels and their fields, the `BlockedBy` and `Blocks` rows with
    their disabled cycle rows, the provider rows, the typed model/effort row, the whole-set
    `BlockedBy` patch, the per-dependant `Blocks` sequence in key order, the one-field agent
    replacement, `column default` clearing one field then the whole block, and a `#[gpui::test]`
    that a cycle row refuses `space` while its neighbour takes it.
- [x] P7-T04 — Harness `card.runs` and `scenarios/board/workflow-detail.scenario`
  - verified (projection half only): `cargo test -p fleet-app --lib state::harness` — 42 passed,
    0 failed. `the_open_card_detail_lists_its_runs_oldest_first` drives a succeeded run and a
    live blocked one: two rows oldest first, no cursor, `badges` the provider, `marks` the mark
    word (`done`, then the card's own `needs you`), and the labels are
    `views::board_card_detail::run_line`'s own text — the builder clones the card once and
    truncates it back a run at a time rather than formatting a second version of the same row.
    `a_card_with_no_run_lists_none` keeps the list absent; a closed dialog has no list at all.
    Documented in `docs/TESTING-HARNESS.md` §3.
  - verified (scenario half): `scenarios/board/workflow-detail.scenario` —
    `make harness-one SCENARIO=scenarios/board/workflow-detail.scenario LANE=headless` passes,
    61 lines in 10 s, three consecutive runs green, and a `LANE=virtual` request runs the same
    61 lines (this machine has no compositor, so the runner falls back to headless and says so;
    the file takes no `shot`, so the two lanes execute the identical set). One card takes two
    real runs — Codex through the prompt column,
    which succeeds and is moved on by `on_success`, then Claude through the skill column, which
    parks on its gate — and `dumps/094-runs.json` is the two rows: `succeeded 5s · codex · 0 tok`
    badged `codex` marked `done`, then `blocked 0s · claude` badged `claude` marked `needs you`.
    `⏎` is pinned twice on the Status row: it leaves the detail standing while the nine-line run
    report is folded and opens that row's picker once it is not. The Effort row takes the typed
    `blistering`, which no catalogue offers, and re-opening the picker reads it back out of the
    card the daemon wrote.
- [x] P7-T05 — `docs/UX-SPEC.md`, `docs/KEYMAP.md` and `docs/BOARD.md`
  - verified: read from `views/board_card_detail.rs` (`run_line`, `run_row`, `run_hints`,
    `workflow_rows`, `agent_row`, `blocked_by_rows`/`blocks_rows`, `link_value`, `comments`,
    `folded_reports`, `REPORT_COLLAPSE_LINES`), `dialogs/card_detail/{view,draft}.rs` and
    `dialogs/card_picker/{schema,draft,lifecycle}.rs`. UX-SPEC "Card detail" states the run row,
    its pending wording, the hint line, the five zero-suppressed rows with `column default`, the
    `run {n}` badge and the eight-line fold; "Card property" states the two link multi-selects,
    the disabled `would cycle` row and the free-text Model/Effort row. `KEYMAP.md`'s
    `Dialog > CardDetail` `enter` row now reads "Expand every folded run report, else edit the
    selected property" — description only, no binding changed, `cargo test -p fleet-app --lib
    keymap` 33 passed (incl. `board_documentation_and_bindings_match_in_both_directions`).
    `BOARD.md` §11.9 is "On the card detail".

## Notes / decisions log
- 2026-09-21 (contracts:app skeleton) — `PickerKind::{BlockedBy, Blocks}` map to `card_field()`
  `"blocked_by"` and the three agent kinds to `"agent"`, so `readonly_message` answers for them
  the way it answers for every other field. The enum carries an `#[expect(dead_code, …)]` that
  fires the moment P7-T03 (or P9-T01's `b`/`m`) constructs one of the five — that attribute is
  the owner's to remove.
- 2026-09-21 (a:picker, P7-T03) — the `#[expect(dead_code, …)]` on `PickerKind` became
  `#[cfg_attr(not(test), expect(dead_code, …))]`. P7-T03 owns the picker, not the surfaces that
  *open* it (P7-T02's property rows, P9-T01's `b`/`m`), so the five variants are constructed by
  the tests and by nothing else yet: a bare `expect` now goes unfulfilled in the `cfg(test)`
  build and a bare lint fires in the other. The attribute still goes unfulfilled — the signal to
  delete it — the moment the first production caller lands. `PickerOption::disabled`'s
  expectation was deleted outright: `schema::link_options` is that caller.
- 2026-09-21 (a:picker, P7-T03) — the `would cycle` rule is the daemon's own
  `ops::validate_links`, run against the one link a row would add (a single probe card rewritten
  per candidate, never a clone per row). The picker and the daemon can therefore not disagree,
  which is what the plan's risk asks for; a refusal that still arrives is shown verbatim.
- 2026-09-21 (a:picker, P7-T03) — Model and Effort read the composer's own source,
  `ThreadProjection::models`, across every thread this client has opened for the card's resolved
  provider (`board::resolve_prefs`, so the catalogue belongs to the harness that would really
  run the card). There is no board-wide catalogue: a provider with no opened thread offers only
  `column default`, which is exactly why the typed row exists and why nothing validates it.
- 2026-09-21 (a:picker, P7-T03) — both link pickers open with a `No blockers` / `Blocks nothing`
  row, the way `Labels` opens with `No labels`: `toggle_value("")` is how a multi-select is
  emptied, and without a row to reach it dropping five links costs five `space`s.
- 2026-09-21 (a:detail, P7-T02) — the `Blocks` row opens `PickerKind::Blocks`, not
  `PickerKind::BlockedBy` as the plan's step says twice. Contracts §5.3 gives `Blocks` its own
  apply path, P7-T03 built it (per-dependant `UpdateCard` in key order, its own `Blocks nothing`
  row), and `⏎` on a row labelled Blocks that opened the *blockers* picker would edit the other
  end of the link from the one the reader is looking at. Read as a slip in the plan; the picker
  kind the row names is the one that writes what the row shows.
- 2026-09-21 (a:detail, P7-T02) — the workflow rows sit between Status and Priority, in the
  order Provider, Model, Effort, Blocked by, Blocks, and a multi-blocker card gets one row per
  blocker with the label on the first only: per-blocker tone is the point of the row (amber for
  a blocker nobody can finish), and one joined line cannot carry it.
- 2026-09-21 (a:detail, P7-T01) — the live state word is `DelegationStatus::word`, so a running
  child reads `working` rather than the plan's parenthetical `running`. That enum's own doc
  says `word` is written for the app's rows while the CLI prints the wire names
  (`fleet-cli/src/human.rs::delegation_status_word`), so this keeps one vocabulary per surface
  instead of a second mapping.
- 2026-09-21 (a:detail, P7-T01) — pending reads `pending {elapsed} · waiting for a slot`, and
  `starting` is what a live run in `DelegationStatus::Starting` reads: those are the plan's two
  wordings, split by which of the two states the card is actually in.
- 2026-09-21 (a:detail, P7-T01) — `⏎` opens every folded report before it goes back to meaning
  "edit the selected property" (`card_detail/draft.rs::enter_target`). Contracts §5.3 fixes the
  fold hint as `⏎ expand`, the detail has no left-pane selection to hang a per-row key on, and
  `⏎` is the only key bound in `Dialog > CardDetail` that could mean it. The hint is drawn only
  while a report is folded, so the key never names an affordance that does nothing, and once
  every report is open the property pane owns `⏎` again. Expansion is per report id and lasts
  as long as the dialog: `seed` rebuilds the draft on every open.
- 2026-09-21 (a:harness, P7-T04) — the builder holds `&AppState` and cannot read the
  `DialogHost`, so `card.runs` is projected for the board's *selected* card, which is the card
  `card_detail::lifecycle::seed` opens the dialog on. `run_line` states a card's latest run, so
  each row is asked for by cloning the card once and truncating its `runs` back one at a time —
  one clone per projection while the detail is open, and no second formatting of a run row.
- 2026-09-21 (a:harness, P7-T04) — a row's `marks` is the phase-6 mark word, and the newest
  row's is the card's own fold, so the tile and the dump can never disagree. Older rows are read
  from their outcome by the same table (succeeded → `done`, needs-you/failed/incomplete →
  `needs you`, cancelled → nothing). A live row's *label* still says `blocked`, because the
  label is `DelegationStatus::word` and the mark is the tile's: the two vocabularies were
  already split by a:detail's deviation 3.
- 2026-09-21 (a:scenario, P7-T04) — the scenario sets the card's provider with `m` before it
  moves the card, and that is not decoration: the preset's two action columns are a *prompt*
  column and a *skill* column, `DEFAULT_PROVIDER` is Claude, and Claude's scripted child parks
  on a gate. Without the pick, both runs are the same parked run. With it, run 1 is Codex —
  which reports and completes, so `on_success` moves the card — and run 2 is Claude, because
  `resolve_prefs` gives a skill action Claude whatever the card asked for. One card, two
  providers, two outcomes, no transcript of its own.
- 2026-09-21 (a:scenario, P7-T04) — **deviation: the card picker now reports its query as
  `dialog.fields[0]` and paints `dialog.field[0]` on it** (`dialogs/host.rs::dialog_fields`,
  `card_picker/view.rs`, `docs/TESTING-HARNESS.md` §3 and §11, one new
  `#[gpui::test]`). The plan's step "assert the row reads it back" had no oracle: the detail's
  property pane is not projected at all, and the picker — the one surface that re-reads the
  card — reported nothing either. This is the documented rule applied rather than bent: the
  picker's whole tab cycle *is* that one editor, so it is exactly the case `dialog_fields`
  already exists for, and the painted target keeps `fields[N]` and `targets["dialog.field[N]"]`
  naming the same field. Additive: no shipped name changed meaning.
- 2026-09-21 (a:scenario, P7-T04) — **what the scenario covers and what it cannot.** The card
  detail projects `card.runs` and nothing else: its property rows, its comments and its fold are
  the dialog host's, and no `AppState` projection reaches them. So the fold is pinned by
  *consequence* — `⏎` twice on the Status row, which leaves the detail standing while a report
  is folded and opens the row's picker once it is not — and the Effort read-back is pinned by
  re-opening the picker, whose `seed` puts a value no row carries into the query. What no
  scenario can assert today is the row's own text (`Effort  blistering`, the muted
  `column default` source, the `run {n}` badge, the `⏎ expand` hint); those stay with
  `card_detail`'s own `#[gpui::test]`s until something projects the pane.
- 2026-09-21 (a:scenario, P7-T04) — two `await idle`s in the file are load-bearing, and the
  second one is a bug this scenario found in its own first draft: the picker's `seed` reads the
  card once, at open, so `⏎`-apply immediately followed by `⏎`-reopen re-opens over the card as
  it was and shows an empty query for ever. The first run passed and the next failed on exactly
  that, 39 ms after the apply. `idle` is false until the app has applied a snapshot covering the
  mutation's revision, which is the documented wait for it (§2). The same reasoning guards the
  provider pick, whose write has to be on the card before the move that starts a run reads it.
- 2026-09-21 (a:scenario, P7-T04) — the `virtual` lane is unavailable on this machine
  (`HYPRLAND_INSTANCE_SIGNATURE` unset), so a `--lane virtual` request falls back to `headless`
  and says so. The file takes no `shot`, so both lanes execute the identical 61 lines and the
  fallback costs this scenario nothing; three consecutive headless runs were green.

## Follow-ups
- (a:scenario) `make test`'s headless slice now costs 195 s against its 60 s budget — 27
  scenarios, of which this phase's is 12 s — and `harness_headless.rs` says so in a warning it
  does not fail on. Two `agents/` scenarios also failed *inside* that back-to-back run
  (`closed-tab-survives-a-daemon-restart`, `subagent-reopen-closed-caller`, both on
  `agents.threads[0].attached`) and both pass when run alone, which is the same load story as
  the `real_shell_*` leak assertions. Somebody has to decide whether the slice keeps growing or
  goes back to `make harness-headless`; it is not phase 7's call to make alone.
- (a:scenario) The card detail's right pane is invisible to the harness: `dialog.fields` carries
  only live editors, and the detail's rows are not editors. Board settings already reports one
  non-editor field (`section`) for exactly this reason. If a later phase wants a scenario to read
  `Effort  blistering` or the muted `column default` source off the pane, the shape to copy is
  that one — and it must stay a *separate* numbering from the detail's own title/description
  editors, which are deliberately unreported, or `dialog.field[N]` stops naming one thing.
- (a:detail) `views/board_card_detail.rs` is 838 lines. It already owns a directory for its
  tests; the next change to it should split the run row and the workflow rows into
  `board_card_detail/{runs.rs,workflow.rs}` before it passes the ~900-line line
  (`rust-workspace-architecture`).
- (a:detail) `REPORT_COLLAPSE_LINES` duplicates `DELEGATION_RESULT_COLLAPSE_LINES`
  (`screens/agent_thread/rows/item.rs:29`), which is private to its module. Make that one
  `pub(crate)` and import it here, so the transcript and the card cannot fold at two different
  places.
- (a:detail) The run mark's glyph table is repeated in `run_glyph` because `CardTile` keeps its
  own private to the kit. A `RunMark::el()` in `fleet-ui-kit` would leave one table.
