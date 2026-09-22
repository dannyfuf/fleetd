# Board workflows: the worktree board as a control plane — Roadmap
> Phase plans:
>  - Phase 1: ./board-workflows-2026-09-20-phase-1-plan.md · ./board-workflows-2026-09-20-phase-1-tracker.md
>  - Phase 2: ./board-workflows-2026-09-20-phase-2-plan.md · ./board-workflows-2026-09-20-phase-2-tracker.md
>  - Phase 3: ./board-workflows-2026-09-20-phase-3-plan.md · ./board-workflows-2026-09-20-phase-3-tracker.md
>  - Phase 4: ./board-workflows-2026-09-20-phase-4-plan.md · ./board-workflows-2026-09-20-phase-4-tracker.md
>  - Phase 5: ./board-workflows-2026-09-20-phase-5-plan.md · ./board-workflows-2026-09-20-phase-5-tracker.md
>  - Phase 6: ./board-workflows-2026-09-20-phase-6-plan.md · ./board-workflows-2026-09-20-phase-6-tracker.md
>  - Phase 7: ./board-workflows-2026-09-20-phase-7-plan.md · ./board-workflows-2026-09-20-phase-7-tracker.md
>  - Phase 8: ./board-workflows-2026-09-20-phase-8-plan.md · ./board-workflows-2026-09-20-phase-8-tracker.md
>  - Phase 9: ./board-workflows-2026-09-20-phase-9-plan.md · ./board-workflows-2026-09-20-phase-9-tracker.md
>
> Contracts (shared shapes, authoritative for the build): ./board-workflows-2026-09-20-contracts.md
>
> Source design: the Claude doc "Board workflows: the worktree board as a control plane"
> (https://claude.ai/code/artifact/083bbcb1-091c-4360-ac31-5e1ba8548ddc), reviewed, fact-checked
> against this branch and settled on 2026-09-20. Its Decisions section is binding; every phase plan
> restates the parts it needs so a reader without the doc can execute it.
>
> Board: the cards for these phases live on the worktree board
> `dannyfuf/fleetd#feat-workflows-v1` (prefix `FEA`), one card per phase: FEA-1 to FEA-9 in phase order. The tracker files are the
> backup of that board and carry the task-level detail the cards do not.

## Summary

The worktree board stops mirroring work and starts driving it. A column carries at most one *on
enter* action (run the card's brief as a native subagent, or start a Claude thread with a named
skill); a card carries agent preferences, `blocked_by` links, a `pending_run` marker and a `runs`
history. Moving a card into an action column, by any path, reaches one function in the daemon's
`Boards` service that decides under the board gate and returns a plan applied outside it. A run is a
delegation whose caller is a card, so the child lifecycle, the `fleet subagent complete` report, the
nudges, the once-only recovery and the cancel path are reused verbatim. Dependencies gate only
automatic advancement. Everything the app can do, the CLI can do, so an orchestrating agent builds
and drives a chain without the GUI.

The design doc's implementation plan names nine cards. This roadmap maps them one to one onto nine
phases in the same order, each shippable as one pull request, with the deviations listed below.

## Why this is phased

The work touches every layer: `fleet-core` types and pure functions, a `fleet-daemon` migration
that is the agents store's first table rebuild, a new engine inside `Boards`, three wire requests
and one capability, a new CLI family, ui-kit builders, three app dialogs, the keymap and the GUI
harness. Each layer has its own reviewer checklist under `.claude/skills/` and its own verification
(`make test` versus `make harness`). The critical path is 1 → 2 → 3 → 5: the feature works headless
before any screen is touched, and the CLI is its first real user. Phases 6 and 8 depend only on 1
(and 6 on 2 for the delegation mirror); phase 7 depends on 6; phase 9 closes the loop.

Deviations from the design doc's table, each a shippability call:

1. **The `board.automation` capability string is defined in phase 1 and advertised in phase 3, not
   phase 2.** A client that sees the capability may rely on the new card fields, `BoardView.live_runs`
   and the three run requests; those are served by the engine, not by the delegation caller change.
   Phase 2 still filters card-called records from `DelegationChanged` and `DelegationList` for a
   peer that has not named the capability, because nothing before phase 3 can name it.
2. **The three run requests (`CardRunStart`, `CardRunCancel`, `CardRunWait`), the client methods and
   their goldens land in phase 3 with the engine that serves them.** The doc lists them under
   "Protocol" without a card. Phase 5 consumes them.
3. **Docs ride with their code, not in a docs phase.** `CLAUDE.md` requires the doc update in the
   same commit. Each phase carries its own `docs/` edits; phase 1 writes the ADR because every
   decision it records is already settled; phase 9 audits the result and adds only what has no
   earlier home.
4. **The `fleet-board-planning` and `fleet-subagent-cli` skill sections move from card 9 to
   phase 5**, because they document the CLI verbs phase 5 ships and an orchestrating agent is
   phase 5's acceptance test.
5. **Phase 4 (what a run changed) is scheduled after phase 3, not in parallel with it.** The doc
   marks its dependency on card 2 as soft; here it also needs phase 3's `start_for_card` to know
   which thread's first checkpoint to diff. It remains the one phase that can be cut without
   cutting the feature.

Two repo facts shape the plans and are not in the design doc. First, the agents store's migration
ladder (`crates/fleet-daemon/src/services/agents/store/migrations.rs`) forbids editing a shipped
slot and has no table-rebuild precedent; phase 2 adds slot 007 as the first rebuild, guarded so it
is idempotent and pinned by hash like every slot before it. Second, `test-support` exports no fake
agent provider a `Boards` test could drive; phase 3 uses the scripted provider the GUI harness
already ships (`docs/TESTING-HARNESS.md` §5, `{"type":"shell"}` steps) through a private `fleetd`
for its end-to-end diamond test, and keeps the pure engine in `fleet-core` where table tests need
no provider at all.

## Phase list

### Phase 1 — core: the automation domain model
- **Goal:** Every new type, field, validation and the lazy document version, in `fleet-core`, with
  nothing served and nothing shown. The ADR and `docs/BOARD.md` §2 describe the shapes.
- **Shippable state at end of phase:** A v1 document round-trips at v1. A document with
  automation, links, a pending run or a run history writes version 2 and a v1-only build refuses it
  by name. A cycle, a cross-board link, a self-link, a backward routing, a Codex skill action and a
  reserved env key are each refused with the contract's sentence. The `board.automation` capability
  string exists and is not advertised.
- **Card:** FEA-1 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-1-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-1-tracker.md

### Phase 2 — daemon: card-called delegations
- **Goal:** `DelegationCaller::{Thread, Card}` untagged on the wire, the slot-007 table rebuild,
  `DelegationService::run_for_card`, the three decided worker behaviours, the per-peer event and
  list filter, and byte-exact goldens for both caller shapes.
- **Shippable state at end of phase:** A tokio test creates a card-called delegation against a
  scripted provider, the child reports with `fleet subagent complete`, the record goes `Succeeded`,
  its `Deliver` row invokes a `RunDeliveryHook` and is marked done only after the hook returns, and
  the caller-repair sweep leaves it alone across a worker drain. The legacy fixture still parses.
  Nothing on any board changes yet.
- **Card:** FEA-2 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-2-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-2-tracker.md

### Phase 3 — daemon: the automation engine
- **Goal:** `re_evaluate` and `after_card_entered` in `fleet-core::board::automation` returning a
  `Plan`; the four triggers inside `Boards`; the in-flight reservation; the apply sequence;
  `on_run_delivered`; `resume_automation` with adoption; the slot-release subscriber with its memo;
  the refusals; `BoardView.live_runs`; the three run requests served; the capability advertised.
- **Shippable state at end of phase:** A four-card diamond driven through `MoveCard` against the
  scripted provider with `max_live_runs = 1` reaches Done card by card, no two delegations are ever
  live at once, and each card's run history is exactly right. Restarting the service mid-run
  adopts the live delegation instead of starting a second. A context board, a Jira board and a
  hosted worktree refuse automation with the contract's sentence.
- **Card:** FEA-3 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-3-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-3-tracker.md

### Phase 4 — daemon: what a run changed
- **Goal:** `Checkpoints::changed_since`, `result_files` filled from it for card runs, the
  `## Files changed since this run started` brief section with its shared-worktree sentence, and
  the real file count on `CardRun.files_changed`.
- **Shippable state at end of phase:** In a temporary git worktree: checkpoint, edit two files,
  delete one, and the list reads `M`/`A`/`D` with a count of three; a worktree that is not a git
  repository answers an empty list and the run still succeeds.
- **Card:** FEA-4 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-4-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-4-tracker.md

### Phase 5 — cli: columns, card fields and run verbs
- **Goal:** The `columns` family with `preset workflow` matched by id; `--provider/--model/--effort/
  --clear-agent`; the `blocked-by`/`blocks` flags; `--desc-file`; `board set --max-live-runs`;
  `card run/cancel/runs/attach/wait`; the `board show` marks and the `⊘ n` column; the client-side
  "a run cannot move its own card" refusal; the two skill sections.
- **Shippable state at end of phase:** A shell script against a private `fleetd` with the scripted
  provider builds the preset, creates a four-card chain in Todo, links it, moves it to Ready, and
  `card wait` on the last card exits 0 with every card in Done. This is the feature's first real user.
- **Card:** FEA-5 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-5-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-5-tracker.md

### Phase 6 — app: the board face
- **Goal:** The derived run-mark map behind a narrow revision in the board `ProjectionKey`;
  `CardTile.run(RunMark)` and `.blocked(u32, BlockedTone)` with the key line wrapped in a row; the
  `⚡` builder on `KanbanColumn`; the pane header's `1/1 working · 1 needs you`; the additive
  harness marks.
- **Shippable state at end of phase:** A harness shot shows one working, one pending, one
  needs-you and one blocked card on one board, and a gpui test proves the board model is not
  rebuilt when a live child's headline changes.
- **Card:** FEA-6 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-6-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-6-tracker.md

### Phase 7 — app: the card detail
- **Goal:** The run row, the three zero-suppressed agent rows, Blocked by and Blocks, the
  `PickerKind::BlockedBy` multi-select with `would cycle` shown disabled, report comments with a run
  badge collapsing at eight lines.
- **Shippable state at end of phase:** A harness scenario opens a card with two runs, expands a
  report, and sets an effort the picker does not list by typing it.
- **Card:** FEA-7 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-7-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-7-tracker.md

### Phase 8 — app: Board settings becomes rail and pane
- **Goal:** General / Backend / Columns in the global Settings shape (720 × 560, 180 px rail); the
  Columns pane drilling list → column; `P` for the preset; `C` as the shortcut into Columns; the
  disabled automation rows on a context or Jira board; `docs/UX-SPEC.md` Board chapter moved.
- **Shippable state at end of phase:** A harness scenario presses `C`, `P`, changes the review
  column's effort, saves with `ctrl-s`, reopens and sees it; the same dialog on a context board
  offers order and names but no automation.
- **Card:** FEA-8 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-8-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-8-tracker.md

### Phase 9 — app and docs: keys, palette, confirm, end-to-end scenario
- **Goal:** `A`/`X`/`>` on the workspace board and the detail, `b`/`m` on both boards and the
  detail, the six palette commands, the move-cancels-a-run `ConfirmRequest` variant, the sticky
  error on a failed start, `docs/KEYMAP.md`, `docs/APP-CONTRACTS.md`, the §15.3 sentence in
  `docs/NATIVE-AGENTS.md`, and a cross-document audit.
- **Shippable state at end of phase:** `make harness` runs a scenario that builds a chain from the
  CLI, watches it on the board, attaches a run with `A`, cancels one with `X` and re-runs it with
  `>`. Every authoritative document agrees with the shipped code.
- **Card:** FEA-9 on the worktree board
- **Plan:** ./board-workflows-2026-09-20-phase-9-plan.md
- **Tracker:** ./board-workflows-2026-09-20-phase-9-tracker.md

## Seams between phases

- **Phase 1 → 2.** Phase 1 adds `Comment.run_id`, `CardRun`, `ActivityKind::{RunStarted, RunEnded,
  AutoMoved}` and `BOARD_AUTOMATION_CAPABILITY` as a string. Phase 2 adds `DelegationCaller` in
  `fleet-core::agents::delegation` and needs `BoardId`/`CardId` from `fleet-core::board`, which is
  the same crate, so no new crate edge appears.
- **Phase 2 → 3.** Phase 2 gives the delegation service `run_for_card(CardRunRequest)` and a
  `RunDeliveryHook` trait the worker calls for a `Deliver` row whose caller is a card, marking the
  row done only after the hook returns `Ok`. Phase 3 implements the hook on `Boards` and installs
  it in `composition.rs` after `Boards::new`. Until phase 3, no card-called delegation is ever
  created in production and the hook slot is empty; the worker treats an unset hook as
  "undeliverable: no board service", which a phase-2 test asserts.
- **Phase 3 → 4.** Phase 3 writes `CardRun.files_changed = 0` and sends no files section. Phase 4
  fills both from `Checkpoints::changed_since` and changes no other shape.
- **Phase 3 → 5.** Phase 3 serves `CardRunStart`, `CardRunCancel`, `CardRunWait`, the client
  methods and their budgets. Phase 5 adds only CLI parsing, rendering and the two skills; a phase-5
  CLI against a phase-3 daemon works end to end.
- **Phase 2 → 6.** Phase 6 keys tile marks on `CardRun.id` against the app's delegation mirror,
  which already receives every `DelegationChanged` the peer is allowed to see. Phase 2's filter
  means an app that does not name the capability sees no card-called record and draws no mark,
  which is the correct degraded state.
- **Phase 6 → 7 → 9.** Phase 6 adds `RunMark`, `BlockedTone` and the derived `CardMarks` map on
  `BoardState`; phase 7 reads the same map for the run row; phase 9 binds the keys to actions that
  phases 6 and 7 leave as plain functions on `screens::board::actions`.
- **Phase 8 → 9.** Phase 8 promotes `Dialogs::BoardSettings` and adds the `C` key; phase 9 adds
  the palette command for it with the other five.
- **Harness.** Phase 6 adds the `board-workflow` fixture preset (the third additive preset; it
  joins the §2 `fixture:` enumeration exactly as `agents-subagent` did in commit 0b76217, the one
  §2 line this feature touches), additive marks on `board.cards` rows and a `board.summary` list;
  phase 7 adds `card.runs`; phase 8 adds the `section` field and `settings.columns`. All amend
  `docs/TESTING-HARNESS.md` §3 and §4 under their additive rules.
- **Docs.** `docs/BOARD.md` gains §11 "Automation" in phase 1 (model), grown by phases 3 (engine),
  5 (CLI) and 6 to 8 (app). `docs/NATIVE-AGENTS.md` §15 gains §15.7 "Card callers" in phase 2.
  Phase 9 audits both.

## Cross-phase risks

- **The board gate is non-reentrant.** `Boards::gate` hands out an `OwnedMutexGuard` per board
  (`services/boards.rs:99-105`). Any code that holds it and calls a `pub` verb of `Boards` hangs
  that board forever. The rule for phases 3, 4 and 9: `Boards` never calls its own `pub` verbs;
  only request handlers and the delivery hook acquire the gate, and every start happens after the
  guard is dropped.
- **The transition half runs inside the writer's transaction.** `store/delegations.rs::transition`
  is called with only a `&Transaction` from the projection path. Phase 2 must keep the card-caller
  arm free of any manager or board call; the board write happens in the worker's `Deliver` handler,
  which runs on the tokio side.
- **The first table rebuild.** Slot 007 in the agents migration ladder copies `delegations` into a
  new table with nullable `caller_turn`/`caller_item` and the three new columns. Rules 1 to 6 of
  `migrations.rs` apply: immutable slots, a pinned hash, a `PRAGMA table_info` guard so a re-run is
  a no-op, and a why-comment. The `REQUIRED_DELEGATION_COLUMNS` golden grows by three.
- **Byte-exact goldens.** Every new field carries `#[serde(default, skip_serializing_if = …)]`.
  `DelegationCaller` is untagged so a thread caller encodes byte for byte as today; the
  `agent_compatibility.rs` legacy fixture is the proof and must not be edited.
- **Clippy `-D warnings` across the workspace.** New `ActivityKind`, `PickerKind`, `ConfirmRequest`
  and `Command` variants break every exhaustive match at once. Phases 1, 7 and 9 budget for the sweep.
- **The frozen harness contract.** Phases 6 to 9 add only what `docs/TESTING-HARNESS.md` §3, §4
  and §5 allow, plus the one preset name on the §2 `fixture:` line that the additive-preset
  precedent permits. Any other edit to the §2 grammar or an existing target name is a stop-and-ask.
- **Render prepares nothing.** The run-mark map is computed in the update path when a
  `DelegationChanged` arrives, behind `CardMarks.revision` that moves only when a mark changes.
  A board projection keyed on `delegations_revision` would rebuild on every child tool call and is
  wrong.
- **Real providers.** No phase test runs `claude` or `codex`. Phases 3 and 5 use the scripted
  provider; a manual smoke against a real provider is a follow-up recorded in the phase-9 tracker.

## Suggested order

Strictly 1 → 2 → 3, then 4 and 5 (independent of each other), then 6 → 7, 8 in parallel with 6/7,
then 9. Two people can run 4/5 beside 6/7/8 once 3 has merged: 4 and 5 touch `fleet-daemon` and
`fleet-cli`; 6 to 8 touch `fleet-app` and `fleet-ui-kit`. Phase 9 is last.
