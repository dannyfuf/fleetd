# Board workflows, phase 7: app — the card detail — Plan
> Tracker: ./board-workflows-2026-09-20-phase-7-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

The card detail becomes the place a run is read and a run is configured. A run row sits between the
title and the description whenever the card has a run or a pending one, reading
`{mark} {state word} {elapsed} · {provider} · {model} · {effort}` and, once terminal,
` · {tokens} tok · ${cost}`, with a hint line naming the three run keys. The right pane gains three
zero-suppressed rows — Provider, Model, Effort — that appear only while the card's column has an
action and show `column default` in the muted tone when inherited, plus Blocked by and Blocks.
`Enter` on any of them opens the ordinary `Card property` picker with a new `PickerKind`; the
blocked-by kind is multi-select over every non-archived card but this one, rows that would close a
cycle listed disabled and detailed `would cycle`. Report comments carry a `run {n}` badge instead
of an author and collapse like a delegation result card. An ordinary board shows none of it.

## Sizing call

**Phased, phase 7 of 9.** See ./board-workflows-2026-09-20-roadmap.md. One pull request. It depends
on phase 6 (the `CardMarks` map and `RunMark`) and on phase 1's card model, and is independent of
phase 8. Not split further: the row, the rows and the picker are one dialog the same scenario
drives. Phase 9 binds the keys the hint line names.

## Repository context

- Detail view `views/board_card_detail.rs`: `PropertyTarget` :32-39, `PropertyRow` :43-60
  (`locked` :89), **`property_rows`** :121-265 (Status :147 … custom schema :233-263),
  `is_readonly` :104, `property_row` :269-302, comments :337-369 (header :344, author+age :355-365,
  `MarkdownText` :366), activity :373-393, `title_line` :397-425.
- Dialog `dialogs/card_detail/view.rs`: rows :25-27, left pane :57-73, properties :75-93, **hint
  line** :103-117, `Dialog::new("Card detail")` :119-125, handlers :133-244; width `HELP_W` (880)
  `dialogs/mod.rs:138`.
- Picker `dialogs/card_picker/`: `PickerKind` `draft.rs:5-23` (`label()` :28-39,
  `is_multi_select` :43-45, `card_field()` :54-64), `PickerOption` :69-89, `selected` :103,
  `toggle` :153-168; options `schema.rs:58-160`; view :22-67 (multi hint :54-62), `PICKER_ROWS`
  `card_picker.rs:31-33`; apply `lifecycle.rs:184-190`, :336-345.
- Kit: `FuzzyItem::disabled(bool)` `components/fuzzy_list.rs:113` already exists;
  `DelegationResultCard` `components/agent/rows.rs:310-321`; the collapse constant is app-side,
  `DELEGATION_RESULT_COLLAPSE_LINES: usize = 8` `screens/agent_thread/rows/item.rs:29` (applied
  :281, card :283-290).
- Marks from phase 6: `state/board.rs::CardMarks`/`TileMarks`, `RunMark` from `fleet-ui-kit`.
  Live run facts: `BoardView.live_runs` (§1.4) and the mirror `state/agents.rs` :58-62, :230.
- Harness `state/harness/projection.rs` `dialog` section and `fn card_row` :923-934;
  `docs/TESTING-HARNESS.md` §3 additive rule :193-195.
- Skills: `gpui-components` (rows and picker shape), `gpui-styling` (tones, the mark glyphs),
  `gpui-app-shell` (the dialog, the picker delegate, the hint line), `gpui-performance`
  (row prep out of `render`), `rust-gpui-testing` with `docs/TESTING-HARNESS.md` §3/§9,
  `zed-quality-review` last. Docs in step: `docs/UX-SPEC.md` "Card detail" :2053 and "Card
  property" :2095, `docs/KEYMAP.md` `Dialog > CardDetail` rows, `docs/BOARD.md` §11.

## Assumptions

- **Every string the dialog draws is prepared in the update path** — the run row text, the elapsed
  string, the resolved prefs and their sources — and `render` only lays them out.
- **The picker keeps its shape.** No new dialog, no new key context: four new `PickerKind`
  variants, the existing multi-select machinery, one new flag on `PickerOption`.
- **Catalogue source.** Model and effort rows list the provider catalogue the native-thread model
  menu already uses when any thread of that provider is live, else the configured default, then the
  typed query as a free row (§5.3). The research does not name that catalogue's module — confirm
  with `grep -n "fn models" crates/fleet-app/src` before writing the options.
- **Zero suppression.** Provider/Model/Effort exist only while the column has an action; Blocked by
  and Blocks when non-empty or when any card on the board carries a link.

## Out of scope

- Keys `A`/`X`/`>`/`b`/`m` and their palette commands (phase 9); Board settings (phase 8); anything
  the daemon does with the values written here.

## Affected areas

- `crates/fleet-app/src/views/board_card_detail.rs`, `dialogs/card_detail/{view.rs, draft.rs}`,
  `dialogs/card_picker/{draft.rs, schema.rs, view.rs, lifecycle.rs}`, `state/harness/projection.rs`,
  and the detail/picker tests beside them.
- `scenarios/board/workflow-detail.scenario`; `docs/{UX-SPEC.md, KEYMAP.md, TESTING-HARNESS.md,
  BOARD.md}`.

## Tasks

### P7-T01 — The run row and report comments that collapse
- **Intent / touches:** The left pane's two new pieces — `views/board_card_detail.rs`,
  `dialogs/card_detail/{view.rs, draft.rs}`.
- **Steps:**
  - Between `title_line` (:397-425) and the description, render a run row when `latest_run(card)`
    or `pending_run` exists: the phase-6 `RunMark` glyph, the state word (live from the delegation —
    `starting`, `running`, `blocked`, `settling`; terminal from `RunOutcome::word()`), elapsed, then
    `· {provider} · {model} · {effort}` dropping a missing part with its separator, and
    ` · {tokens} tok · ${cost:.2}` only when terminal. Pending reads `pending · waiting for a slot`
    or `starting`.
  - No hint line under it: `A attach · X cancel · > re-run` is drawn by phase 9 together with the
    keys it names, so the dialog never shows an affordance that does nothing (contracts §5.3).
  - Comments (:337-369): a comment with `run_id` set renders a `run {n}` badge (`n` is its 1-based
    index in `card.runs`) instead of the author, and collapses at `DELEGATION_RESULT_COLLAPSE_LINES`
    (`rows/item.rs:29`, reused) with `⏎ expand` and the transcript card's per-row expand state.
    The dialog hint line (`card_detail/view.rs:103-117`) is extended by phase 9, not here.
  - Tests: the row for a live, a pending and each terminal outcome; the badge and the eight-line
    collapse; both absent on a card with no run.
- **Verification:** `cargo test -p fleet-app card_detail`, `make lint`.
- **Done when:** The row draws from prepared strings only and no run-specific chrome exists
  elsewhere in the dialog.

### P7-T02 — Provider, Model, Effort, Blocked by and Blocks property rows
- **Intent / touches:** The right pane's five zero-suppressed rows — `views/board_card_detail.rs`
  `property_rows` :121-265.
- **Steps:**
  - Insert after Status (:147): `Provider`, `Model`, `Effort`, each `PropertyTarget::Pick(kind)`,
    present only while the card's column has an action, showing the resolved value (card → column →
    provider default, §2 `resolve_prefs`) with the muted source `column default` when inherited.
  - `Blocked by`: one line per blocker, `{KEY} · {column name}` with a `✓` when the blocker's
    column category is `Completed`, warning tone when it is canceled or archived; `Blocks` is the
    derived inverse from `ops::query::blocks`. Both are `Pick(PickerKind::BlockedBy)` targets.
  - Keep the rows `locked` when `is_readonly` (:104) says the board is, with today's reason, and
    leave the existing row order untouched. Resolved values and sources are computed once where the
    draft is built, never in `property_row` (:269-302).
  - Tests: no new row on a column without an action; `column default` shown for an inherited value
    and absent for a card-level one; a satisfied blocker draws `✓`; `Blocks` lists the dependants.
- **Verification:** `cargo test -p fleet-app board_card_detail`, `make lint`.
- **Done when:** An ordinary board's property list is unchanged and the five rows appear only under
  their conditions.

### P7-T03 — Four `PickerKind`s, disabled rows and the two apply paths
- **Intent / touches:** `dialogs/card_picker/{draft.rs, schema.rs, view.rs, lifecycle.rs}`.
- **Steps:**
  - `PickerKind` (`draft.rs:5-23`) gains `BlockedBy`, `Blocks`, `Provider`, `Model`, `Effort`;
    `label()` answers `Blocked by`, `Blocks`, `Provider`, `Model`, `Effort`; `is_multi_select`
    (:43-45) answers true for `BlockedBy` and `Blocks`; `card_field()` maps them to the fields the patch writes.
  - `PickerOption` (:69-89) gains `disabled: bool` with a `disabled()` builder; the view (:22-67)
    passes it to `FuzzyItem::disabled` (`fuzzy_list.rs:113`) and `toggle`/apply refuse a disabled
    row without closing the dialog.
  - Options (`schema.rs:58-160`): `BlockedBy` lists every non-archived card but this one, seeded
    from `card.blocked_by`, rows `{KEY}  {title}` detailed with the column name, or disabled and
    detailed `would cycle` when selecting it would close one (walk `blocked_by` from the candidate;
    the daemon's refusal stays the authority). `Provider`: `column default`, `claude`, `codex`.
    `Model`/`Effort`: `column default`, then the same list source the agent composer's model
    picker uses (confirm with `grep -rn "SetModel" crates/fleet-app/src/screens/agent_thread`),
    then the typed query as a free row — effort is never validated (contracts §5.3).
  - Apply (`lifecycle.rs:184-190`, :336-345): `BlockedBy` sends one `UpdateCard` with
    `CardPatch.blocked_by = Some(whole set)`; `Blocks` sends one `UpdateCard { blocked_by }` per
    dependant whose membership changed, in key order, stopping at the first refusal with its
    sentence on the error line and leaving the cards already patched (contracts §5.3); the three agent kinds read the card's current
    `CardAgentPrefs`, replace the one field (`column default` clears it) and send
    `CardPatch.agent = Some(Some(prefs))`, or `Some(None)` when all three are cleared. A daemon
    refusal keeps the dialog open with the sentence verbatim, as today.
  - Tests: the four labels; multi-select only for `BlockedBy`; a cycle candidate is listed,
    disabled and not togglable; `column default` clears the field; a typed effort applies.
- **Verification:** `cargo test -p fleet-app card_picker`, `make lint`.
- **Done when:** The picker takes no new dialog or key context and every refusal is the daemon's.

### P7-T04 — Harness `card.runs` and `scenarios/board/workflow-detail.scenario`
- **Intent / touches:** `state/harness/projection.rs`, `docs/TESTING-HARNESS.md` §3,
  `scenarios/board/workflow-detail.scenario`.
- **Steps:**
  - While the detail is open, add list `card.runs`: one row per `CardRun`, oldest first, `label` =
    that run's row text, `badges` = the provider, `marks` = the phase-6 mark word. Add its input to
    the harness `ProjectionKey` so an `await` wakes on it; document in §3, §2 untouched.
  - Scenario on phase 6's `board-workflow` fixture: drive a card through two runs (one succeeded,
    one blocked), open the detail, `await lists["card.runs"].rows[1] exists`, expand a report
    comment with `enter` and assert the expansion, then on the Effort row open the picker, type an
    effort the list does not offer, apply, and assert the row reads it back. No `shot`.
- **Verification:** `make harness-one SCENARIO=scenarios/board/workflow-detail.scenario`, then
  `LANE=headless`, then `make harness`.
- **Done when:** The scenario passes in both lanes and every assertion path exists in a real dump.

### P7-T05 — `docs/UX-SPEC.md`, `docs/KEYMAP.md` and `docs/BOARD.md`
- **Intent / touches:** Documents in step, same commit — `docs/UX-SPEC.md` :2053 and :2095,
  `docs/KEYMAP.md` `Dialog > CardDetail`, `docs/BOARD.md` §11.
- **Steps:**
  - UX-SPEC "Card detail": the run row and its order, the pending wording, the hint line, the five
    zero-suppressed rows with the `column default` rule, the report badge and collapse. "Card
    property": the multi-select kind, the disabled `would cycle` row, the free-text effort row.
  - KEYMAP `Dialog > CardDetail`: the rows this phase draws, marked as bound in phase 9 if that
    phase has not merged, so the table never claims a binding that does not exist.
  - `BOARD.md` §11 gains "On the card detail", cross-referencing UX-SPEC.
- **Verification / done when:** `git diff docs/` beside the scenario; every assertion has a
  sentence, and `KEYMAP.md` matches `keymap::table()` exactly.

## Verification

```sh
make lint && make test && make harness
```

`make harness` is required: this phase changes a dialog. Run `make harness-one SCENARIO=…` on
`scenarios/board/workflow-detail.scenario` first, then the corpus.

## Definition of done

- [ ] Every task above is `[x]` in the tracker; `make lint` and `cargo check --workspace
      --all-targets` are clean; `make test` passes.
- [ ] `make harness` passes with `workflow-detail.scenario` in both lanes.
- [ ] `docs/TESTING-HARNESS.md` changed only under §3's additive rule; `UX-SPEC.md`, `KEYMAP.md`
      and `BOARD.md` describe what runs.
- [ ] No string is built in a `render` body; no kit component learned a domain type.
- [ ] The tracker reflects reality; follow-ups are captured.

## Risks and rollback

- **Clippy `-D warnings` on the new `PickerKind` variants.** Four variants break every exhaustive
  match in the picker and its schema at once; budget the sweep in P7-T03 rather than adding a
  catch-all arm, which would hide the next variant.
- **Cycle detection drifts from the daemon.** The picker's `would cycle` is an affordance, not the
  rule; if the two disagree the daemon's refusal wins and is shown verbatim.
- **Rollback.** Revert the PR: the detail returns to today's rows; phase 6's face and the daemon
  are untouched.

## Contract resolutions

Resolved in the contracts file on 2026-09-20; the contracts wording wins over any earlier phrasing above.

1. The run row's hint line `A attach · X cancel · > re-run` is drawn by phase 9 with the keys; P7-T01 draws the row without it (contracts §5.3).
2. `PickerKind::Blocks` exists; applying it sends one `UpdateCard { blocked_by }` per dependant whose membership changed, in key order, stopping at the first refusal with its sentence on the error line (contracts §5.3).
3. The three agent rows read the card's whole `agent`, replace one field and send the whole `CardPatch.agent`; last writer wins, as labels do today (contracts §5.3).
4. `PickerOption` gains `disabled: bool`, mapped onto the kit's `FuzzyItem::disabled` (contracts §5.3).
5. The model/effort catalogue reuses the agent composer's model-picker source; confirm with `grep -rn "SetModel" crates/fleet-app/src/screens/agent_thread` (contracts §5.3).
