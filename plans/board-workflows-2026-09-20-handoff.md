# Board workflows — session handoff (rewritten 2026-09-21, after the nine-phase build)

This file described a session that had paused mid phase 1. It no longer does. All nine phases have
been built, in parallel, by one agent per owned file set in this single worktree
(`/home/df/.fleet/worktrees/dannyfuf/fleetd/feat-workflows-v1`, branch `feat/workflows-v1`). Read
this, then `plans/board-workflows-2026-09-20-roadmap.md`, then whichever phase tracker owns the
part you are about to touch. The trackers are the truth about state; this file is the narrative.

## Where the branch is

- HEAD is **`ab5f195 docs: record the phase-1 landing and gate in its tracker`**, five commits past
  the `origin/main` merge (`f7ac30e`, which brought PR #43 and ADR 0021). Committed so far:
  `ed99c68 proto: define the board.automation capability string`,
  `87f6e3c core: add the board automation domain model` (phase 1),
  `648ddee cli: resolve a bare --worktree from a native agent thread` (the old FEA-10; landed, its
  agent worktree removed) and two docs commits.
- **Everything from phases 2 through 9 is uncommitted in the working tree**: about 159 files and
  16.7k inserted lines, `git status` clean of nothing. Nothing has been staged, stashed, branched
  or reset — the build agents were forbidden every state-changing git command, so the tree is
  exactly what they wrote. The commit plan below is the one thing still owed before a PR.
- A previous handoff's advice to squash a WIP commit no longer applies: `4f54602` was rewritten
  into `87f6e3c`/`ed99c68` and is gone.

## What is in the tree, by phase

Each phase's tracker carries its own verification lines; this is the map.

| Phase | What it built | Where it lives |
| --- | --- | --- |
| 1 | `ColumnAutomation`, card links and run history, lazy `document_version`, the validations, the workflow preset | `fleet-core/src/board/` (**committed**), plus `ops/validation.rs` and `board.rs` edits still in the tree |
| 2 | `DelegationCaller`, slot 007's table rebuild, `run_for_card`, `RunDeliveryHook`, the per-peer filter | `fleet-daemon/src/services/agents/` |
| 3 | The pure engine and the boards service around it: triggers, throttle, outcome table, `resume_automation`, the three requests | `fleet-core/src/board/automation*`, `fleet-daemon/src/services/boards/automation*`, `fleet-proto`, `fleet-client` |
| 4 | The run's changed-file list and the report's `## Files changed since this run started` section | `fleet-daemon/src/services/boards/automation.rs`, `fleet-git` paths it calls |
| 5 | `fleet board columns …`, the card link and run verbs, `card wait`'s exit 2, the smoke script | `fleet-cli/src/`, `scripts/board-workflow-smoke.sh`, `Makefile` |
| 6 | `RunMark`, `BlockedTone`, `CardTile::run`/`.blocked`, `refresh_card_marks`, the pane header | `fleet-ui-kit/src/components/`, `fleet-app/src/state/board.rs`, `views/board_screen/` |
| 7 | The card-detail run row, report folding, the five property rows and four picker kinds | `fleet-app/src/views/board_card_detail*`, `dialogs/card_picker/` |
| 8 | Board settings' rail and its Columns pane | `fleet-app/src/dialogs/board_settings/` |
| 9 | `A` `X` `>` `b` `m` `C`, their palette commands, the two confirms, the end-to-end scenario, the documents | `fleet-app/src/{actions,keymap}.rs`, `screens/board/runs*`, `dialogs/confirm*`, `scenarios/board/`, `docs/`, `.claude/skills/` |

The five new scenarios are `scenarios/board/workflow-{marks,detail,columns,columns-context,chain}
.scenario`; only `workflow-marks` takes a `shot`, so the other four are in the headless lane.

## What is verified, and what is not

**Verified.** `make lint` is clean. Every crate's own suite is green when run alone, and the
documents were audited claim-by-claim against the shipped symbols on 2026-09-21 (phase-9 tracker,
P9-T06). `make smoke-workflow` drives a four-card diamond end to end against a private daemon.

**Not verified, and each for a known reason:**

1. **`make test` is red.** `fleet-app`'s `harness_headless` fails on two board-workflow scenarios,
   deterministically (3/3, including run alone): the board tile never paints a live run's mark.
   The phase-9 tracker's first follow-up has the measurements — a card-called child that goes
   `blocked` does not repaint its tile, and the face can lag a run by the whole of that run. It is
   an app-side wiring bug between the delegation mirror and `refresh_card_marks`; the reducer
   itself is pinned green by unit tests. **This is the one thing standing between the tree and a
   PR.** Owner: whoever owns `state/board.rs`'s mark refresh and the board view's event path.
2. **No phase ever ran a real provider.** Every run in every test, scenario and smoke was a
   `fleet-harness agent` scripted binary replaying a JSON transcript. The manual smoke that is
   owed before the feature is announced — both providers, preset, chain, attach, cancel, re-run —
   is written out in full in the phase-9 tracker's Follow-ups. Do not announce without it.
3. Three pre-existing corpus problems the round surfaced but did not cause: `hub/jobs-panel`
   lines 28/40 over-specify the panel, `harness_headless` inside `cargo test` is over its own
   stated 60 s budget at ~250 s, and a handful of load flakes the phase-9 tracker names one by one.
   All three are the corpus owner's decisions, not this feature's.

## The commit plan

One logical change per commit, `<area>: <imperative lowercase summary>`, the doc update riding
with the code it describes (CLAUDE.md). Commit in this order — each compiles on the one before:

1. `core: add the board automation engine` — `crates/fleet-core/src/board/automation*`, the
   `validation.rs`/`board.rs` edits, `agents/delegation.rs`'s `DelegationCaller` and `Recorded`,
   with `docs/BOARD.md` §2/§11.1–§11.7.
2. `proto: add the card-run requests and the cancel-run field` — `fleet-proto/src/request.rs` and
   its compatibility goldens, with `docs/BOARD.md` §5.
3. `daemon: give a delegation a card caller` — `services/agents/` including migration slot 007 and
   `delegation/tests/card.rs`, with `docs/NATIVE-AGENTS.md` §15.2/§15.7 and
   `docs/research/agents-contracts.md`.
4. `daemon: run a card when it enters a column` — `services/boards/automation*`, `lifecycle.rs`,
   `cards.rs`, `composition.rs`, `maintenance.rs`, `dispatch.rs`, the router arms and
   `tests/boards_automation.rs`, with `docs/BOARD.md` §4.1 and `docs/ARCHITECTURE.md`.
5. `daemon: report what a run changed in the worktree` — phase 4's diff and report section.
6. `client: add the three run methods and their capability check` — `fleet-client`.
7. `cli: add the column verbs and the card run verbs` — `fleet-cli`, `scripts/board-workflow-smoke.sh`,
   `Makefile`, with `docs/BOARD.md` §6 and both `.claude/skills/fleet-*` skills.
8. `ui-kit: mark a card's run and a column's action` — `fleet-ui-kit` and its gallery, with
   `docs/DESIGN-SYSTEM.md`.
9. `app: draw a run on the board face` — phase 6's state and views.
10. `app: state a run on the card detail` — phase 7.
11. `app: configure columns in board settings` — phase 8.
12. `app: act on a run from the board` — phase 9's actions, keymap, palette, confirms, with
    `docs/KEYMAP.md`, `docs/APP-CONTRACTS.md` and `docs/UX-SPEC.md`.
13. `tests: drive the board workflow end to end` — `fleet-harness`'s `board-workflow` preset and
    `scenarios/`, with `docs/TESTING-HARNESS.md`.
14. `docs: record the board-workflows plan` — `plans/`, `docs/README.md`, `TODO.md`,
    `docs/decisions/0022-board-workflows.md` if it is not already carried by (1).

Fix the red scenarios before or inside commit 9, whichever the fix touches. No Claude attribution
trailer on any of them.

## Things not to redo

- The ADR is `docs/decisions/0022-board-workflows.md`; its `docs/README.md` row is written.
- `CardRunStart`/`Cancel`/`Wait` classify by `host_or_local(host_of_card(..))`, not `Target::Local`.
- Line numbers in every phase plan predate the PR #43 merge. Re-grep symbols; the plans were
  written against an older tree and several files have since been split.
- Every tracker's Kickoff "clean-tree baseline" box is deliberately unticked: nobody building in a
  nine-agent shared worktree could tick it, and phase 5 wrote the reason down first.
